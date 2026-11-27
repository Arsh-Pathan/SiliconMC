#!/usr/bin/env python3
"""
Compiles Minecraft 26.3 gameplay tables for workstations from vanilla data JSONs and reports:
  - Enchantments (`src/main/resources/data/minecraft/enchantment/*.json`, tags, & item components)
  - Brewing recipes (`src/main/resources/data/minecraft/recipe/brewing/*.json` & potion registry)
  - Smithing transform & trim recipes (`src/main/resources/data/minecraft/recipe/*smithing*.json`)
  - Enchantability, enchantable item tags, exclusive sets, and anvil repairable materials

Outputs `rust/crates/mc-data/src/generated/gameplay_tables.rs`.
"""

import json
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DATA_MC = REPO_ROOT / "src" / "main" / "resources" / "data" / "minecraft"
REPORTS_DIR = REPO_ROOT / "generated" / "reports"
OUT_PATH = (
    REPO_ROOT
    / "rust"
    / "crates"
    / "mc-data"
    / "src"
    / "generated"
    / "gameplay_tables.rs"
)


def load_registries() -> dict:
    return json.loads((REPORTS_DIR / "registries.json").read_text(encoding="utf-8"))


def load_item_ids(registries: dict) -> dict[str, int]:
    entries = registries["minecraft:item"]["entries"]
    return {name: int(meta["protocol_id"]) for name, meta in entries.items()}


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


def load_enchantment_tags() -> dict[str, list[str]]:
    tags_dir = DATA_MC / "tags" / "enchantment"
    raw_tags: dict[str, list] = {}
    for p in tags_dir.rglob("*.json"):
        rel = p.relative_to(tags_dir).with_suffix("").as_posix()
        tag_name = f"minecraft:{rel}"
        raw_tags[tag_name] = json.loads(p.read_text(encoding="utf-8")).get("values", [])

    cache: dict[str, list[str]] = {}

    def resolve(tag_name: str, visiting: set[str]) -> list[str]:
        if tag_name in cache:
            return cache[tag_name]
        if tag_name in visiting:
            return []
        visiting.add(tag_name)
        out: set[str] = set()
        for entry in raw_tags.get(tag_name, []):
            ref = entry["id"] if isinstance(entry, dict) else entry
            if ref.startswith("#"):
                out.update(resolve(ref[1:], visiting))
            else:
                out.add(ref)
        visiting.remove(tag_name)
        res = sorted(out)
        cache[tag_name] = res
        return res

    for name in list(raw_tags.keys()):
        resolve(name, set())
    return cache


def resolve_item_spec(
    raw, item_ids: dict[str, int], item_tags: dict[str, list[int]]
) -> list[int]:
    if raw is None:
        return []
    if isinstance(raw, str):
        if raw.startswith("#"):
            return list(item_tags.get(raw[1:], []))
        iid = item_ids.get(raw)
        return [iid] if iid is not None else []
    if isinstance(raw, list):
        out: set[int] = set()
        for elem in raw:
            out.update(resolve_item_spec(elem, item_ids, item_tags))
        return sorted(out)
    if isinstance(raw, dict):
        if "item" in raw:
            return resolve_item_spec(raw["item"], item_ids, item_tags)
        if "tag" in raw:
            return resolve_item_spec("#" + raw["tag"], item_ids, item_tags)
    return []


def fmt_u16_slice(vals: list[int]) -> str:
    return "&[" + ", ".join(str(v) for v in vals) + "]"


def fmt_str_slice(vals: list[str]) -> str:
    return "&[" + ", ".join(json.dumps(v) for v in vals) + "]"


def main() -> None:
    registries = load_registries()
    item_ids = load_item_ids(registries)
    item_tags = load_item_tags(item_ids)
    ench_tags = load_enchantment_tags()

    # ------------------------------------------------------------------
    # 1. Enchantments (src/main/resources/data/minecraft/enchantment/*.json)
    # ------------------------------------------------------------------
    treasure_set = set(ench_tags.get("minecraft:treasure", []))

    enchantments: list[dict] = []
    referenced_item_tags: set[str] = set()

    for p in sorted((DATA_MC / "enchantment").glob("*.json")):
        ench_id = f"minecraft:{p.stem}"
        d = json.loads(p.read_text(encoding="utf-8"))
        supported_tag = str(d["supported_items"])
        primary_tag = str(d.get("primary_items") or supported_tag)
        if supported_tag.startswith("#"):
            referenced_item_tags.add(supported_tag)
        if primary_tag.startswith("#"):
            referenced_item_tags.add(primary_tag)

        excl_raw = d.get("exclusive_set")
        excl_list: list[str] = []
        if isinstance(excl_raw, str):
            if excl_raw.startswith("#"):
                excl_list = list(ench_tags.get(excl_raw[1:], []))
            else:
                excl_list = [excl_raw]
        elif isinstance(excl_raw, list):
            for elem in excl_raw:
                if isinstance(elem, str):
                    if elem.startswith("#"):
                        excl_list.extend(ench_tags.get(elem[1:], []))
                    else:
                        excl_list.append(elem)
            excl_list = sorted(set(excl_list))

        min_cost = d["min_cost"]
        max_cost = d["max_cost"]
        enchantments.append(
            {
                "id": ench_id,
                "max_level": int(d["max_level"]),
                "weight": int(d["weight"]),
                "anvil_cost": int(d["anvil_cost"]),
                "min_cost_base": int(min_cost["base"]),
                "min_cost_per_level": int(min_cost["per_level_above_first"]),
                "max_cost_base": int(max_cost["base"]),
                "max_cost_per_level": int(max_cost["per_level_above_first"]),
                "supported_tag": supported_tag,
                "primary_tag": primary_tag,
                "is_treasure": ench_id in treasure_set,
                "exclusive": excl_list,
            }
        )
    enchantments.sort(key=lambda e: e["id"])

    # Resolve enchantable item tags referenced by EnchantmentSpec
    tag_table: list[tuple[str, list[int]]] = []
    for tag in sorted(referenced_item_tags):
        ids = item_tags.get(tag[1:], [])
        tag_table.append((tag, ids))

    # Extract item enchantability and repairable materials from component reports
    comp_dir = REPORTS_DIR / "minecraft" / "components" / "item"
    enchantability_entries: list[tuple[int, int]] = []
    repairable_entries: list[tuple[int, list[int]]] = []
    for p in sorted(comp_dir.glob("*.json")):
        iname = f"minecraft:{p.stem}"
        iid = item_ids.get(iname)
        if iid is None:
            continue
        comps = json.loads(p.read_text(encoding="utf-8")).get("components", {})
        if "minecraft:enchantable" in comps:
            val = int(comps["minecraft:enchantable"].get("value", 0))
            if val > 0:
                enchantability_entries.append((iid, val))
        if "minecraft:repairable" in comps:
            mats = resolve_item_spec(
                comps["minecraft:repairable"].get("items"), item_ids, item_tags
            )
            if mats:
                repairable_entries.append((iid, mats))
    enchantability_entries.sort()
    repairable_entries.sort()

    # ------------------------------------------------------------------
    # 2. Brewing recipes (src/main/resources/data/minecraft/recipe/brewing/*.json)
    # ------------------------------------------------------------------
    brewing_recipes: list[tuple[int, int, int, str, str]] = []
    reagent_set: set[int] = set()
    brewing_dir = DATA_MC / "recipe" / "brewing"
    for p in sorted(brewing_dir.rglob("*.json")):
        d = json.loads(p.read_text(encoding="utf-8"))
        if d.get("type") != "minecraft:brewing":
            continue
        inp = d["input"]
        reag = d["reagent"]
        out = d["output"]

        in_items = resolve_item_spec(inp.get("item"), item_ids, item_tags)
        reag_items = resolve_item_spec(reag.get("item"), item_ids, item_tags)
        out_id = item_ids.get(out.get("id", ""))
        if not in_items or not reag_items or out_id is None:
            continue

        in_pot_raw = inp.get("potion_contents", {}).get("potions", "")
        in_pots = [in_pot_raw] if isinstance(in_pot_raw, str) else list(in_pot_raw)
        out_pot = (
            out.get("components", {})
            .get("minecraft:potion_contents", {})
            .get("potion", "")
        )

        for bi in in_items:
            for ri in reag_items:
                reagent_set.add(ri)
                for ip in in_pots:
                    brewing_recipes.append((bi, ri, out_id, str(ip), str(out_pot)))

    brewing_recipes.sort(key=lambda r: (r[0], r[1], r[3], r[2], r[4]))
    brewing_reagents = sorted(reagent_set)
    brewing_inputs = sorted(item_tags.get("minecraft:brewing_potion_inputs", []))

    potion_entries = sorted(
        (
            (name, int(meta["protocol_id"]))
            for name, meta in registries["minecraft:potion"]["entries"].items()
        ),
        key=lambda x: x[0],
    )
    potion_by_id = sorted(potion_entries, key=lambda x: x[1])

    # ------------------------------------------------------------------
    # 3. Smithing recipes (src/main/resources/data/minecraft/recipe/*smithing*.json)
    # ------------------------------------------------------------------
    smithing_transforms: list[tuple[int, int, int, int]] = []
    smithing_trims: list[tuple[int, str]] = []
    trim_base_set: set[int] = set()
    trim_add_set: set[int] = set()

    for p in sorted((DATA_MC / "recipe").glob("*smithing*.json")):
        d = json.loads(p.read_text(encoding="utf-8"))
        rtype = d.get("type", "")
        if rtype == "minecraft:smithing_transform":
            templates = resolve_item_spec(d.get("template"), item_ids, item_tags)
            bases = resolve_item_spec(d.get("base"), item_ids, item_tags)
            additions = resolve_item_spec(d.get("addition"), item_ids, item_tags)
            res = d.get("result")
            res_name = res.get("id") if isinstance(res, dict) else res
            res_id = item_ids.get(res_name) if isinstance(res_name, str) else None
            if res_id is None:
                continue
            for t in templates:
                for b in bases:
                    for a in additions:
                        smithing_transforms.append((t, b, a, res_id))
        elif rtype == "minecraft:smithing_trim":
            templates = resolve_item_spec(d.get("template"), item_ids, item_tags)
            bases = resolve_item_spec(d.get("base"), item_ids, item_tags)
            additions = resolve_item_spec(d.get("addition"), item_ids, item_tags)
            pattern = str(d.get("pattern", ""))
            trim_base_set.update(bases)
            trim_add_set.update(additions)
            for t in templates:
                smithing_trims.append((t, pattern))

    smithing_transforms.sort()
    smithing_trims.sort()
    trimmable_armor = sorted(trim_base_set)
    trim_materials = sorted(trim_add_set)

    # ------------------------------------------------------------------
    # 4. Relevant DataComponentType protocol IDs
    # ------------------------------------------------------------------
    dc_entries = registries["minecraft:data_component_type"]["entries"]
    dc_map = {
        "DAMAGE": int(dc_entries["minecraft:damage"]["protocol_id"]),
        "CUSTOM_NAME": int(dc_entries["minecraft:custom_name"]["protocol_id"]),
        "ENCHANTMENTS": int(dc_entries["minecraft:enchantments"]["protocol_id"]),
        "REPAIR_COST": int(dc_entries["minecraft:repair_cost"]["protocol_id"]),
        "STORED_ENCHANTMENTS": int(
            dc_entries["minecraft:stored_enchantments"]["protocol_id"]
        ),
        "POTION_CONTENTS": int(
            dc_entries["minecraft:potion_contents"]["protocol_id"]
        ),
        "TRIM": int(dc_entries["minecraft:trim"]["protocol_id"]),
    }

    # ------------------------------------------------------------------
    # Emit Rust source
    # ------------------------------------------------------------------
    lines: list[str] = [
        "// @generated by scripts/gen_gameplay_tables.py from Minecraft 26.3 enchantments, brewing, smithing JSONs & registries.",
        "// Do not edit manually.",
        "",
        "pub mod data_components {",
    ]
    for const_name, pid in dc_map.items():
        lines.append(f"    pub const {const_name}: i32 = {pid};")
    lines.extend(
        [
            "}",
            "",
            "#[derive(Debug, Clone, Copy, PartialEq, Eq)]",
            "pub struct EnchantmentSpec {",
            "    pub id: &'static str,",
            "    pub max_level: u8,",
            "    pub weight: u16,",
            "    pub anvil_cost: u8,",
            "    pub min_cost_base: i32,",
            "    pub min_cost_per_level: i32,",
            "    pub max_cost_base: i32,",
            "    pub max_cost_per_level: i32,",
            "    pub supported_tag: &'static str,",
            "    pub is_treasure: bool,",
            "}",
            "",
            "impl EnchantmentSpec {",
            "    /// Vanilla `Enchantment.Cost.calculate(level)` for minimum enchanting power.",
            "    #[inline]",
            "    pub const fn min_cost(self, level: u8) -> i32 {",
            "        self.min_cost_base + self.min_cost_per_level * (level as i32 - 1)",
            "    }",
            "",
            "    /// Vanilla `Enchantment.Cost.calculate(level)` for maximum enchanting power.",
            "    #[inline]",
            "    pub const fn max_cost(self, level: u8) -> i32 {",
            "        self.max_cost_base + self.max_cost_per_level * (level as i32 - 1)",
            "    }",
            "}",
            "",
            "#[derive(Debug, Clone, Copy, PartialEq, Eq)]",
            "pub struct BrewingRecipe {",
            "    pub base_item: u16,",
            "    pub ingredient_item: u16,",
            "    pub result_item: u16,",
            "    pub input_potion: &'static str,",
            "    pub output_potion: &'static str,",
            "}",
            "",
            "#[derive(Debug, Clone, Copy, PartialEq, Eq)]",
            "pub struct SmithingRecipe {",
            "    pub template_item: u16,",
            "    pub base_item: u16,",
            "    pub addition_item: u16,",
            "    pub result_item: u16,",
            "}",
            "",
            "pub static ENCHANTMENTS: &[EnchantmentSpec] = &[",
        ]
    )

    for e in enchantments:
        lines.append(
            f"    EnchantmentSpec {{ "
            f"id: {json.dumps(e['id'])}, "
            f"max_level: {e['max_level']}, "
            f"weight: {e['weight']}, "
            f"anvil_cost: {e['anvil_cost']}, "
            f"min_cost_base: {e['min_cost_base']}, "
            f"min_cost_per_level: {e['min_cost_per_level']}, "
            f"max_cost_base: {e['max_cost_base']}, "
            f"max_cost_per_level: {e['max_cost_per_level']}, "
            f"supported_tag: {json.dumps(e['supported_tag'])}, "
            f"is_treasure: {'true' if e['is_treasure'] else 'false'} }},"
        )
    lines.append("];")
    lines.append("")

    # Parallel metadata indexed identically to ENCHANTMENTS
    lines.append(
        f"static ENCHANTMENT_PRIMARY_TAGS: [&str; {len(enchantments)}] = ["
    )
    for e in enchantments:
        lines.append(f"    {json.dumps(e['primary_tag'])},")
    lines.append("];")
    lines.append("")

    lines.append(
        f"static ENCHANTMENT_EXCLUSIVE_SETS: [&[&str]; {len(enchantments)}] = ["
    )
    for e in enchantments:
        lines.append(f"    {fmt_str_slice(e['exclusive'])},")
    lines.append("];")
    lines.append("")

    lines.append(f"static ENCHANTABLE_ITEM_TAGS: [(&str, &[u16]); {len(tag_table)}] = [")
    for tag, ids in tag_table:
        lines.append(f"    ({json.dumps(tag)}, {fmt_u16_slice(ids)}),")
    lines.append("];")
    lines.append("")

    lines.append(
        f"static ITEM_ENCHANTABILITY: [(u16, u8); {len(enchantability_entries)}] = ["
    )
    for iid, val in enchantability_entries:
        lines.append(f"    ({iid}, {val}),")
    lines.append("];")
    lines.append("")

    lines.append(
        f"static ITEM_REPAIR_MATERIALS: [(u16, &[u16]); {len(repairable_entries)}] = ["
    )
    for iid, mats in repairable_entries:
        lines.append(f"    ({iid}, {fmt_u16_slice(mats)}),")
    lines.append("];")
    lines.append("")

    lines.append("pub static BREWING_RECIPES: &[BrewingRecipe] = &[")
    for bi, ri, out_id, in_pot, out_pot in brewing_recipes:
        lines.append(
            f"    BrewingRecipe {{ "
            f"base_item: {bi}, "
            f"ingredient_item: {ri}, "
            f"result_item: {out_id}, "
            f"input_potion: {json.dumps(in_pot)}, "
            f"output_potion: {json.dumps(out_pot)} }},"
        )
    lines.append("];")
    lines.append("")

    lines.append(
        f"static BREWING_REAGENTS: [u16; {len(brewing_reagents)}] = [{', '.join(str(x) for x in brewing_reagents)}];"
    )
    lines.append(
        f"static BREWING_POTION_INPUTS: [u16; {len(brewing_inputs)}] = [{', '.join(str(x) for x in brewing_inputs)}];"
    )
    lines.append("")

    lines.append(f"static POTION_BY_NAME: [(&str, i32); {len(potion_entries)}] = [")
    for name, pid in potion_entries:
        lines.append(f"    ({json.dumps(name)}, {pid}),")
    lines.append("];")
    lines.append("")

    lines.append(f"static POTION_BY_ID: [(i32, &str); {len(potion_by_id)}] = [")
    for name, pid in potion_by_id:
        lines.append(f"    ({pid}, {json.dumps(name)}),")
    lines.append("];")
    lines.append("")

    lines.append("pub static SMITHING_RECIPES: &[SmithingRecipe] = &[")
    for t, b, a, r in smithing_transforms:
        lines.append(
            f"    SmithingRecipe {{ template_item: {t}, base_item: {b}, addition_item: {a}, result_item: {r} }},"
        )
    lines.append("];")
    lines.append("")

    lines.append("pub static SMITHING_TRIM_TEMPLATES: &[(u16, &str)] = &[")
    for t, pat in smithing_trims:
        lines.append(f"    ({t}, {json.dumps(pat)}),")
    lines.append("];")
    lines.append("")

    lines.append(
        f"pub static SMITHING_TRIMMABLE_ARMOR: &[u16] = {fmt_u16_slice(trimmable_armor)};"
    )
    lines.append(
        f"pub static SMITHING_TRIM_MATERIALS: &[u16] = {fmt_u16_slice(trim_materials)};"
    )
    lines.append("")

    lines.extend(
        [
            "#[inline]",
            "fn normalize_mc_match(canonical: &str, query: &str) -> bool {",
            "    canonical == query || canonical.strip_prefix(\"minecraft:\") == Some(query)",
            "}",
            "",
            "/// Index of an enchantment inside `ENCHANTMENTS` (accepts both `\"minecraft:sharpness\"` and `\"sharpness\"`).",
            "#[inline]",
            "pub fn enchantment_index(id: &str) -> Option<usize> {",
            "    if let Ok(idx) = ENCHANTMENTS.binary_search_by_key(&id, |e| e.id) {",
            "        return Some(idx);",
            "    }",
            "    if !id.contains(':') {",
            "        return ENCHANTMENTS.iter().position(|e| e.id.strip_prefix(\"minecraft:\") == Some(id));",
            "    }",
            "    None",
            "}",
            "",
            "/// Look up a Minecraft 26.3 enchantment definition by ID (accepts `\"minecraft:sharpness\"` or `\"sharpness\"`).",
            "#[inline]",
            "pub fn enchantment_by_id(id: &str) -> Option<&'static EnchantmentSpec> {",
            "    enchantment_index(id).map(|idx| &ENCHANTMENTS[idx])",
            "}",
            "",
            "/// Check whether `item_id` belongs to a resolved `#minecraft:enchantable/...` item tag.",
            "#[inline]",
            "pub fn item_in_enchantable_tag(tag: &str, item_id: u16) -> bool {",
            "    if item_id == 0 {",
            "        return false;",
            "    }",
            "    let Ok(idx) = ENCHANTABLE_ITEM_TAGS.binary_search_by_key(&tag, |(t, _)| *t) else {",
            "        return false;",
            "    };",
            "    ENCHANTABLE_ITEM_TAGS[idx].1.binary_search(&item_id).is_ok()",
            "}",
            "",
            "/// Vanilla `Enchantment.isSupportedItem`: can this enchantment be applied to `item_id` (e.g., via Anvil)?",
            "#[inline]",
            "pub fn enchantment_supports_item(enchantment_id: &str, item_id: u16) -> bool {",
            "    let Some(spec) = enchantment_by_id(enchantment_id) else {",
            "        return false;",
            "    };",
            "    item_in_enchantable_tag(spec.supported_tag, item_id)",
            "}",
            "",
            "/// Vanilla `Enchantment.isPrimaryItem`: can this enchantment be rolled on `item_id` at an Enchanting Table?",
            "#[inline]",
            "pub fn enchantment_is_primary_for_item(enchantment_id: &str, item_id: u16) -> bool {",
            "    let Some(idx) = enchantment_index(enchantment_id) else {",
            "        return false;",
            "    };",
            "    let spec = &ENCHANTMENTS[idx];",
            "    let primary = ENCHANTMENT_PRIMARY_TAGS[idx];",
            "    item_in_enchantable_tag(spec.supported_tag, item_id)",
            "        && (primary == spec.supported_tag || item_in_enchantable_tag(primary, item_id))",
            "}",
            "",
            "/// Vanilla `Enchantment.areCompatible`: returns `false` if equal or mutually exclusive.",
            "pub fn enchantments_compatible(a_id: &str, b_id: &str) -> bool {",
            "    let (Some(ia), Some(ib)) = (enchantment_index(a_id), enchantment_index(b_id)) else {",
            "        return a_id != b_id;",
            "    };",
            "    if ia == ib {",
            "        return false;",
            "    }",
            "    let a_canon = ENCHANTMENTS[ia].id;",
            "    let b_canon = ENCHANTMENTS[ib].id;",
            "    let a_excl = ENCHANTMENT_EXCLUSIVE_SETS[ia];",
            "    let b_excl = ENCHANTMENT_EXCLUSIVE_SETS[ib];",
            "    a_excl.binary_search(&b_canon).is_err() && b_excl.binary_search(&a_canon).is_err()",
            "}",
            "",
            "/// Vanilla `DataComponents.ENCHANTABLE` value for `item_id` (0 if not enchantable).",
            "#[inline]",
            "pub fn item_enchantability(item_id: u16) -> u8 {",
            "    ITEM_ENCHANTABILITY",
            "        .binary_search_by_key(&item_id, |(id, _)| *id)",
            "        .ok()",
            "        .map(|i| ITEM_ENCHANTABILITY[i].1)",
            "        .unwrap_or(0)",
            "}",
            "",
            "/// Vanilla `ItemStack.isValidRepairItem` backed by Minecraft 26.3 `minecraft:repairable` item components.",
            "#[inline]",
            "pub fn is_valid_repair_item(tool_item_id: u16, material_item_id: u16) -> bool {",
            "    if tool_item_id == 0 || material_item_id == 0 {",
            "        return false;",
            "    }",
            "    let Ok(idx) = ITEM_REPAIR_MATERIALS.binary_search_by_key(&tool_item_id, |(id, _)| *id) else {",
            "        return false;",
            "    };",
            "    ITEM_REPAIR_MATERIALS[idx].1.binary_search(&material_item_id).is_ok()",
            "}",
            "",
            "/// Match a Minecraft 26.3 brewing recipe by base bottle item ID, input potion ID, and reagent item ID.",
            "pub fn match_brewing(",
            "    base_item: u16,",
            "    input_potion: &str,",
            "    ingredient_item: u16,",
            ") -> Option<&'static BrewingRecipe> {",
            "    if base_item == 0 || ingredient_item == 0 {",
            "        return None;",
            "    }",
            "    BREWING_RECIPES.iter().find(|r| {",
            "        r.base_item == base_item",
            "            && r.ingredient_item == ingredient_item",
            "            && normalize_mc_match(r.input_potion, input_potion)",
            "    })",
            "}",
            "",
            "/// Returns `true` if `item_id` is a valid brewing reagent in any 26.3 brewing recipe.",
            "#[inline]",
            "pub fn is_brewing_reagent(item_id: u16) -> bool {",
            "    BREWING_REAGENTS.binary_search(&item_id).is_ok()",
            "}",
            "",
            "/// Returns `true` if `item_id` belongs to `#minecraft:brewing_potion_inputs`.",
            "#[inline]",
            "pub fn is_brewing_potion_input(item_id: u16) -> bool {",
            "    BREWING_POTION_INPUTS.binary_search(&item_id).is_ok()",
            "}",
            "",
            "/// Resolve a potion ID string (e.g., `\"minecraft:awkward\"`) to its 26.3 `minecraft:potion` registry protocol ID.",
            "#[inline]",
            "pub fn potion_protocol_id(potion_name: &str) -> Option<i32> {",
            "    if let Ok(idx) = POTION_BY_NAME.binary_search_by_key(&potion_name, |(n, _)| *n) {",
            "        return Some(POTION_BY_NAME[idx].1);",
            "    }",
            "    if !potion_name.contains(':') {",
            "        return POTION_BY_NAME",
            "            .iter()",
            "            .find(|(n, _)| n.strip_prefix(\"minecraft:\") == Some(potion_name))",
            "            .map(|(_, id)| *id);",
            "    }",
            "    None",
            "}",
            "",
            "/// Resolve a 26.3 `minecraft:potion` protocol ID back to its canonical namespaced string.",
            "#[inline]",
            "pub fn potion_name_by_protocol_id(id: i32) -> Option<&'static str> {",
            "    POTION_BY_ID",
            "        .binary_search_by_key(&id, |(pid, _)| *pid)",
            "        .ok()",
            "        .map(|i| POTION_BY_ID[i].1)",
            "}",
            "",
            "/// Match a Minecraft 26.3 armor trim smithing recipe, returning the trim pattern ID (e.g., `\"minecraft:bolt\"`).",
            "#[inline]",
            "pub fn match_smithing_trim(template: u16, base: u16, addition: u16) -> Option<&'static str> {",
            "    let idx = SMITHING_TRIM_TEMPLATES",
            "        .binary_search_by_key(&template, |(t, _)| *t)",
            "        .ok()?;",
            "    if SMITHING_TRIMMABLE_ARMOR.binary_search(&base).is_ok()",
            "        && SMITHING_TRIM_MATERIALS.binary_search(&addition).is_ok()",
            "    {",
            "        Some(SMITHING_TRIM_TEMPLATES[idx].1)",
            "    } else {",
            "        None",
            "    }",
            "}",
            "",
            "/// Match a Minecraft 26.3 smithing recipe (`smithing_transform` -> upgraded item ID, or `smithing_trim` -> `base`).",
            "pub fn match_smithing(template: u16, base: u16, addition: u16) -> Option<u16> {",
            "    if template == 0 || base == 0 || addition == 0 {",
            "        return None;",
            "    }",
            "    if let Some(r) = SMITHING_RECIPES.iter().find(|r| {",
            "        r.template_item == template && r.base_item == base && r.addition_item == addition",
            "    }) {",
            "        return Some(r.result_item);",
            "    }",
            "    if match_smithing_trim(template, base, addition).is_some() {",
            "        return Some(base);",
            "    }",
            "    None",
            "}",
            "",
        ]
    )

    OUT_PATH.write_text("\n".join(lines), encoding="utf-8")
    print(
        f"Generated {OUT_PATH.name}: "
        f"{len(enchantments)} enchantments ({sum(1 for e in enchantments if e['is_treasure'])} treasure), "
        f"{len(tag_table)} enchantable tags, "
        f"{len(enchantability_entries)} enchantable items, "
        f"{len(repairable_entries)} repairable items, "
        f"{len(brewing_recipes)} brewing recipes ({len(brewing_reagents)} reagents, {len(potion_entries)} potions), "
        f"{len(smithing_transforms)} smithing transform recipes, "
        f"{len(smithing_trims)} smithing trim templates "
        f"({len(trimmable_armor)} trimmable armor x {len(trim_materials)} trim materials)"
    )


if __name__ == "__main__":
    main()
