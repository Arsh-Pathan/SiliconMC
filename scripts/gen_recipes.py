#!/usr/bin/env python3
"""
Compiles all Minecraft 26.3 data-driven crafting, cooking, stonecutting recipes,
exact item cooking fuel burn times, food nutrition/saturation, and MenuType /
BlockEntityType protocol IDs into zero-allocation static Rust tables in
`rust/crates/mc-data/src/generated/recipes.rs`.
"""

import json
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DATA_MC = REPO_ROOT / "src" / "main" / "resources" / "data" / "minecraft"
REPORTS_DIR = REPO_ROOT / "generated" / "reports"
OUT_PATH = REPO_ROOT / "rust" / "crates" / "mc-data" / "src" / "generated" / "recipes.rs"


def load_item_ids() -> dict[str, int]:
    rt = json.loads((REPORTS_DIR / "runtime_tables.json").read_text(encoding="utf-8"))
    return {name: int(iid) for iid, name, _max_stack, _placed in rt["items"]}


def load_item_tags(item_ids: dict[str, int]) -> dict[str, list[int]]:
    tags_dir = DATA_MC / "tags" / "item"
    raw_tags: dict[str, list] = {}
    for p in tags_dir.rglob("*.json"):
        rel = p.relative_to(tags_dir).with_suffix("").as_posix()
        tag_name = f"minecraft:{rel}"
        raw_tags[tag_name] = json.loads(p.read_text(encoding="utf-8")).get("values", [])

    cache: dict[str, list[int]] = {}

    def resolve(tag_name: str, visiting: set[str]) -> list[int]:
        if tag_name in cache:
            return cache[tag_name]
        if tag_name in visiting:
            return []
        visiting.add(tag_name)
        out: set[int] = set()
        for entry in raw_tags.get(tag_name, []):
            ref = entry["id"] if isinstance(entry, dict) else entry
            if ref.startswith("#"):
                out.update(resolve(ref[1:], visiting))
            elif ref in item_ids:
                out.add(item_ids[ref])
        visiting.remove(tag_name)
        res = sorted(out)
        cache[tag_name] = res
        return res

    for name in list(raw_tags.keys()):
        resolve(name, set())
    return cache


def resolve_ingredient(raw, item_ids: dict[str, int], item_tags: dict[str, list[int]]) -> tuple[int, ...]:
    if raw is None:
        return ()
    if isinstance(raw, str):
        if raw.startswith("#"):
            return tuple(item_tags.get(raw[1:], ()))
        iid = item_ids.get(raw)
        return (iid,) if iid is not None else ()
    if isinstance(raw, list):
        out: set[int] = set()
        for elem in raw:
            out.update(resolve_ingredient(elem, item_ids, item_tags))
        return tuple(sorted(out))
    if isinstance(raw, dict):
        if "item" in raw:
            return resolve_ingredient(raw["item"], item_ids, item_tags)
        if "tag" in raw:
            return resolve_ingredient("#" + raw["tag"], item_ids, item_tags)
    return ()


def shrink_pattern(pattern: list[str]) -> list[str]:
    if not pattern:
        return []
    top = 0
    while top < len(pattern) and not pattern[top].strip():
        top += 1
    bottom = len(pattern) - 1
    while bottom >= top and not pattern[bottom].strip():
        bottom -= 1
    if top > bottom:
        return []
    sliced = pattern[top : bottom + 1]
    left = min(len(line) - len(line.lstrip(" ")) for line in sliced if line.strip())
    right = max(len(line.rstrip(" ")) for line in sliced if line.strip())
    return [line[left:right].ljust(right - left) for line in sliced]


def eval_cooking_int_provider(ref, cooking_providers: dict) -> int:
    if isinstance(ref, (int, float)):
        return int(ref)
    if isinstance(ref, str):
        key = ref.split("minecraft:cooking/")[-1]
        return eval_cooking_int_provider(cooking_providers.get(key, 0), cooking_providers)
    if isinstance(ref, dict):
        t = ref.get("type")
        if t == "minecraft:div":
            # Standard furnace (fast_cooking = false -> right = 1)
            return eval_cooking_int_provider(ref["left"], cooking_providers)
    return 0


def main() -> None:
    item_ids = load_item_ids()
    item_tags = load_item_tags(item_ids)
    registries = json.loads((REPORTS_DIR / "registries.json").read_text(encoding="utf-8"))

    # Load cooking int providers for exact 26.3 fuel burn times
    cooking_providers = {}
    for p in (DATA_MC / "context_int_provider" / "cooking").glob("*.json"):
        cooking_providers[p.stem] = json.loads(p.read_text(encoding="utf-8"))

    # Extract fuel burn times and food stats from generated item component reports
    fuel_entries: list[tuple[int, int]] = []
    food_entries: list[tuple[int, int, float]] = []
    comp_dir = REPORTS_DIR / "minecraft" / "components" / "item"
    for p in sorted(comp_dir.glob("*.json")):
        iname = f"minecraft:{p.stem}"
        iid = item_ids.get(iname)
        if iid is None:
            continue
        comps = json.loads(p.read_text(encoding="utf-8")).get("components", {})
        if "minecraft:cooking_fuel" in comps:
            bt = eval_cooking_int_provider(comps["minecraft:cooking_fuel"].get("burn_time"), cooking_providers)
            if bt > 0:
                fuel_entries.append((iid, bt))
        if "minecraft:food" in comps:
            fd = comps["minecraft:food"]
            food_entries.append((iid, int(fd.get("nutrition", 0)), float(fd.get("saturation", 0.0))))
    fuel_entries.sort()
    food_entries.sort()

    # Deduplicated ingredient slice pool
    ing_pool: list[int] = []
    ing_index: dict[tuple[int, ...], tuple[int, int]] = {}  # tuple -> (start, len)

    def intern_ing(vals: tuple[int, ...]) -> tuple[int, int]:
        if not vals:
            return (0, 0)
        if vals in ing_index:
            return ing_index[vals]
        start = len(ing_pool)
        ing_pool.extend(vals)
        span = (start, len(vals))
        ing_index[vals] = span
        return span

    shaped_recipes = []
    shapeless_recipes = []
    cooking_recipes = []  # (kind: 0=smelting, 1=blasting, 2=smoking, 3=campfire, ing_start, ing_len, out_id, out_count, cook_time, xp)
    stonecutting_recipes = []

    cooking_kind_map = {
        "minecraft:smelting": 0,
        "minecraft:blasting": 1,
        "minecraft:smoking": 2,
        "minecraft:campfire_cooking": 3,
    }

    for rpath in sorted((DATA_MC / "recipe").glob("*.json")):
        data = json.loads(rpath.read_text(encoding="utf-8"))
        rtype = data.get("type", "")
        res = data.get("result")
        if isinstance(res, dict):
            out_name = res.get("id")
            out_count = int(res.get("count", 1))
        elif isinstance(res, str):
            out_name = res
            out_count = 1
        else:
            continue
        out_id = item_ids.get(out_name)
        if out_id is None:
            continue

        if rtype == "minecraft:crafting_shaped":
            shrunk = shrink_pattern(data.get("pattern", []))
            if not shrunk:
                continue
            h = len(shrunk)
            w = len(shrunk[0])
            if w > 3 or h > 3:
                continue
            key_map = data.get("key", {})
            cells: list[tuple[int, int]] = []
            valid = True
            for line in shrunk:
                for ch in line:
                    if ch == " ":
                        cells.append((0, 0))
                    else:
                        vals = resolve_ingredient(key_map.get(ch), item_ids, item_tags)
                        if not vals:
                            valid = False
                            break
                        cells.append(intern_ing(vals))
                if not valid:
                    break
            if not valid:
                continue
            while len(cells) < 9:
                cells.append((0, 0))
            shaped_recipes.append((w, h, cells, out_id, out_count))

        elif rtype == "minecraft:crafting_shapeless":
            ings_raw = data.get("ingredients", [])
            if not ings_raw or len(ings_raw) > 9:
                continue
            ings: list[tuple[int, int]] = []
            valid = True
            for raw_ing in ings_raw:
                vals = resolve_ingredient(raw_ing, item_ids, item_tags)
                if not vals:
                    valid = False
                    break
                ings.append(intern_ing(vals))
            if not valid:
                continue
            n_ings = len(ings)
            while len(ings) < 9:
                ings.append((0, 0))
            shapeless_recipes.append((n_ings, ings, out_id, out_count))

        elif rtype in cooking_kind_map:
            vals = resolve_ingredient(data.get("ingredient"), item_ids, item_tags)
            if not vals:
                continue
            start, length = intern_ing(vals)
            cook_time = int(data.get("cookingtime", 200))
            xp = float(data.get("experience", 0.0))
            cooking_recipes.append((cooking_kind_map[rtype], start, length, out_id, out_count, cook_time, xp))

        elif rtype == "minecraft:stonecutting":
            vals = resolve_ingredient(data.get("ingredient"), item_ids, item_tags)
            if not vals:
                continue
            start, length = intern_ing(vals)
            stonecutting_recipes.append((start, length, out_id, out_count))

    menu_entries = registries["minecraft:menu"]["entries"]
    be_entries = registries["minecraft:block_entity_type"]["entries"]

    lines = [
        "// @generated by scripts/gen_recipes.py from Minecraft 26.3 recipe JSONs, item tags, and component reports.",
        "// Do not edit manually.",
        "",
        "pub mod menu_types {",
    ]
    for name, meta in sorted(menu_entries.items(), key=lambda kv: kv[1]["protocol_id"]):
        const_name = name.split(":")[-1].upper()
        lines.append(f"    pub const {const_name}: i32 = {meta['protocol_id']};")
    lines.append("}")
    lines.append("")
    lines.append("pub mod block_entity_types {")
    for name, meta in sorted(be_entries.items(), key=lambda kv: kv[1]["protocol_id"]):
        const_name = name.split(":")[-1].upper()
        lines.append(f"    pub const {const_name}: i32 = {meta['protocol_id']};")
    lines.append("}")
    lines.append("")

    lines.extend([
        "pub const COOK_SMELTING: u8 = 0;",
        "pub const COOK_BLASTING: u8 = 1;",
        "pub const COOK_SMOKING: u8 = 2;",
        "pub const COOK_CAMPFIRE: u8 = 3;",
        "",
        "#[derive(Debug, Clone, Copy)]",
        "pub struct IngredientSpan { pub start: u32, pub len: u16 }",
        "",
        "impl IngredientSpan {",
        "    #[inline]",
        "    pub fn is_empty(self) -> bool { self.len == 0 }",
        "    #[inline]",
        "    pub fn matches(self, item_id: u16) -> bool {",
        "        if self.len == 0 { return item_id == 0; }",
        "        if item_id == 0 { return false; }",
        "        let s = self.start as usize;",
        "        INGREDIENT_POOL[s..s + self.len as usize].binary_search(&item_id).is_ok()",
        "    }",
        "}",
        "",
        "#[derive(Debug, Clone, Copy)]",
        "pub struct ShapedRecipe {",
        "    pub width: u8,",
        "    pub height: u8,",
        "    pub cells: [IngredientSpan; 9],",
        "    pub result_item: u16,",
        "    pub result_count: u8,",
        "}",
        "",
        "#[derive(Debug, Clone, Copy)]",
        "pub struct ShapelessRecipe {",
        "    pub len: u8,",
        "    pub ingredients: [IngredientSpan; 9],",
        "    pub result_item: u16,",
        "    pub result_count: u8,",
        "}",
        "",
        "#[derive(Debug, Clone, Copy)]",
        "pub struct CookingRecipe {",
        "    pub kind: u8,",
        "    pub ingredient: IngredientSpan,",
        "    pub result_item: u16,",
        "    pub result_count: u8,",
        "    pub cook_time: u16,",
        "    pub experience: f32,",
        "}",
        "",
        "#[derive(Debug, Clone, Copy)]",
        "pub struct StonecuttingRecipe {",
        "    pub ingredient: IngredientSpan,",
        "    pub result_item: u16,",
        "    pub result_count: u8,",
        "}",
        "",
        "#[derive(Debug, Clone, Copy)]",
        "pub struct FoodInfo {",
        "    pub nutrition: u8,",
        "    pub saturation: f32,",
        "}",
        "",
        f"pub static INGREDIENT_POOL: [u16; {len(ing_pool)}] = [",
    ])
    for i in range(0, len(ing_pool), 24):
        chunk = ", ".join(str(x) for x in ing_pool[i : i + 24])
        lines.append(f"    {chunk},")
    lines.append("];")
    lines.append("")

    def fmt_span(s: tuple[int, int]) -> str:
        return f"IngredientSpan {{ start: {s[0]}, len: {s[1]} }}"

    lines.append(f"pub static SHAPED_RECIPES: [ShapedRecipe; {len(shaped_recipes)}] = [")
    for w, h, cells, out_id, out_cnt in shaped_recipes:
        cstr = ", ".join(fmt_span(c) for c in cells)
        lines.append(f"    ShapedRecipe {{ width: {w}, height: {h}, cells: [{cstr}], result_item: {out_id}, result_count: {out_cnt} }},")
    lines.append("];")
    lines.append("")

    lines.append(f"pub static SHAPELESS_RECIPES: [ShapelessRecipe; {len(shapeless_recipes)}] = [")
    for n, ings, out_id, out_cnt in shapeless_recipes:
        istr = ", ".join(fmt_span(c) for c in ings)
        lines.append(f"    ShapelessRecipe {{ len: {n}, ingredients: [{istr}], result_item: {out_id}, result_count: {out_cnt} }},")
    lines.append("];")
    lines.append("")

    lines.append(f"pub static COOKING_RECIPES: [CookingRecipe; {len(cooking_recipes)}] = [")
    for kind, st, ln, out_id, out_cnt, ct, xp in cooking_recipes:
        lines.append(
            f"    CookingRecipe {{ kind: {kind}, ingredient: {fmt_span((st, ln))}, result_item: {out_id}, result_count: {out_cnt}, cook_time: {ct}, experience: {xp!r}_f32 }},"
        )
    lines.append("];")
    lines.append("")

    lines.append(f"pub static STONECUTTING_RECIPES: [StonecuttingRecipe; {len(stonecutting_recipes)}] = [")
    for st, ln, out_id, out_cnt in stonecutting_recipes:
        lines.append(
            f"    StonecuttingRecipe {{ ingredient: {fmt_span((st, ln))}, result_item: {out_id}, result_count: {out_cnt} }},"
        )
    lines.append("];")
    lines.append("")

    lines.append(f"static FUEL_BURN_TIMES: [(u16, u16); {len(fuel_entries)}] = [")
    for iid, bt in fuel_entries:
        lines.append(f"    ({iid}, {bt}),")
    lines.append("];")
    lines.append("")

    lines.append(f"static FOOD_ITEMS: [(u16, FoodInfo); {len(food_entries)}] = [")
    for iid, nut, sat in food_entries:
        lines.append(f"    ({iid}, FoodInfo {{ nutrition: {nut}, saturation: {sat!r}_f32 }}),")
    lines.append("];")
    lines.append("")

    lines.extend([
        "/// Standard furnace burn time in ticks for `item_id` (0 if not fuel).",
        "#[inline]",
        "pub fn fuel_burn_time(item_id: u16) -> u16 {",
        "    FUEL_BURN_TIMES",
        "        .binary_search_by_key(&item_id, |(id, _)| *id)",
        "        .ok()",
        "        .map(|i| FUEL_BURN_TIMES[i].1)",
        "        .unwrap_or(0)",
        "}",
        "",
        "/// Food nutrition/saturation for `item_id` if edible.",
        "#[inline]",
        "pub fn food_info(item_id: u16) -> Option<FoodInfo> {",
        "    FOOD_ITEMS",
        "        .binary_search_by_key(&item_id, |(id, _)| *id)",
        "        .ok()",
        "        .map(|i| FOOD_ITEMS[i].1)",
        "}",
        "",
        "/// Match a cooking recipe of `kind` (`COOK_SMELTING`, `COOK_BLASTING`, `COOK_SMOKING`, `COOK_CAMPFIRE`).",
        "pub fn match_cooking(kind: u8, input_item: u16) -> Option<&'static CookingRecipe> {",
        "    if input_item == 0 { return None; }",
        "    COOKING_RECIPES.iter().find(|r| r.kind == kind && r.ingredient.matches(input_item))",
        "}",
        "",
        "/// Match a `grid_w x grid_h` (2x2 or 3x3) crafting grid of item IDs (0 = empty) against all",
        "/// vanilla Minecraft 26.3 shaped (with horizontal mirror) and shapeless crafting recipes.",
        "pub fn match_crafting(grid_w: usize, grid_h: usize, grid: &[u16]) -> Option<(u16, u8)> {",
        "    if grid.len() < grid_w * grid_h { return None; }",
        "    // Compute tight bounding box of non-empty items",
        "    let mut min_x = grid_w;",
        "    let mut max_x = 0usize;",
        "    let mut min_y = grid_h;",
        "    let mut max_y = 0usize;",
        "    let mut non_empty = [0u16; 9];",
        "    let mut count = 0usize;",
        "    for y in 0..grid_h {",
        "        for x in 0..grid_w {",
        "            let id = grid[y * grid_w + x];",
        "            if id != 0 {",
        "                min_x = min_x.min(x);",
        "                max_x = max_x.max(x);",
        "                min_y = min_y.min(y);",
        "                max_y = max_y.max(y);",
        "                if count < 9 {",
        "                    non_empty[count] = id;",
        "                    count += 1;",
        "                }",
        "            }",
        "        }",
        "    }",
        "    if count == 0 { return None; }",
        "    let bw = max_x - min_x + 1;",
        "    let bh = max_y - min_y + 1;",
        "",
        "    // 1. Shaped recipes (normal and horizontally mirrored, matching ShapedRecipePattern.matches)",
        "    for r in &SHAPED_RECIPES {",
        "        if r.width as usize != bw || r.height as usize != bh { continue; }",
        "        let mut ok_normal = true;",
        "        let mut ok_mirror = true;",
        "        for ry in 0..bh {",
        "            for rx in 0..bw {",
        "                let g_id = grid[(min_y + ry) * grid_w + (min_x + rx)];",
        "                let c_norm = r.cells[ry * bw + rx];",
        "                if !c_norm.matches(g_id) { ok_normal = false; }",
        "                let c_mir = r.cells[ry * bw + (bw - 1 - rx)];",
        "                if !c_mir.matches(g_id) { ok_mirror = false; }",
        "                if !ok_normal && !ok_mirror { break; }",
        "            }",
        "            if !ok_normal && !ok_mirror { break; }",
        "        }",
        "        if ok_normal || ok_mirror {",
        "            return Some((r.result_item, r.result_count));",
        "        }",
        "    }",
        "",
        "    // 2. Shapeless recipes (exact bipartite backtracking match, max 9 items)",
        "    for r in &SHAPELESS_RECIPES {",
        "        if r.len as usize != count { continue; }",
        "        let mut used = [false; 9];",
        "        if match_shapeless_bt(&r.ingredients[..count], &non_empty[..count], 0, &mut used) {",
        "            return Some((r.result_item, r.result_count));",
        "        }",
        "    }",
        "    None",
        "}",
        "",
        "fn match_shapeless_bt(ings: &[IngredientSpan], items: &[u16], idx: usize, used: &mut [bool; 9]) -> bool {",
        "    if idx == ings.len() { return true; }",
        "    let ing = ings[idx];",
        "    for i in 0..items.len() {",
        "        if !used[i] && ing.matches(items[i]) {",
        "            used[i] = true;",
        "            if match_shapeless_bt(ings, items, idx + 1, used) {",
        "                return true;",
        "            }",
        "            used[i] = false;",
        "        }",
        "    }",
        "    false",
        "}",
        "",
    ])

    OUT_PATH.write_text("\n".join(lines), encoding="utf-8")
    print(
        f"Generated {OUT_PATH.name}: {len(shaped_recipes)} shaped, {len(shapeless_recipes)} shapeless, "
        f"{len(cooking_recipes)} cooking, {len(stonecutting_recipes)} stonecutting, "
        f"{len(fuel_entries)} fuels, {len(food_entries)} foods, pool={len(ing_pool)}"
    )


if __name__ == "__main__":
    main()
