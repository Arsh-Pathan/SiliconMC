#!/usr/bin/env python3
"""
Compiles Minecraft 26.3 Overworld ore distributions and per-biome tree/flora profiles
directly from `src/main/resources/data/minecraft/worldgen/` into
`rust/crates/mc-worldgen/src/generated_features.rs`.
"""

import json
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
WORLDGEN_DIR = REPO_ROOT / "src" / "main" / "resources" / "data" / "minecraft" / "worldgen"
REPORTS_DIR = REPO_ROOT / "generated" / "reports"
OUT_PATH = REPO_ROOT / "rust" / "crates" / "mc-worldgen" / "src" / "generated_features.rs"

OW_MIN_Y = -64
OW_MAX_Y = 319


def resolve_anchor(a) -> int:
    if isinstance(a, int):
        return a
    if "absolute" in a:
        return int(a["absolute"])
    if "above_bottom" in a:
        return OW_MIN_Y + int(a["above_bottom"])
    if "below_top" in a:
        return OW_MAX_Y - int(a["below_top"])
    return 0


def parse_count(c) -> tuple[int, int]:
    if isinstance(c, int):
        return (c, c)
    if isinstance(c, dict):
        t = c.get("type", "")
        if t == "minecraft:uniform":
            return (int(c.get("min_inclusive", 1)), int(c.get("max_inclusive", 1)))
        if t == "minecraft:constant":
            v = int(c.get("value", 1))
            return (v, v)
    return (1, 1)


def main() -> None:
    blocks = json.loads((REPORTS_DIR / "blocks.json").read_text(encoding="utf-8"))
    block_default: dict[str, int] = {}
    for bname, bdata in blocks.items():
        for st in bdata["states"]:
            if st.get("default"):
                block_default[bname] = int(st["id"])

    ow_bp = json.loads(
        (REPORTS_DIR / "biome_parameters" / "minecraft" / "overworld.json").read_text(encoding="utf-8")
    )
    ow_biomes = sorted({e["biome"] for e in ow_bp["biomes"]})
    ow_biome_idx = {b: i for i, b in enumerate(ow_biomes)}

    # Collect which Overworld biomes reference each placed_feature in step 6 (UNDERGROUND_ORES)
    ore_biome_masks: dict[str, int] = {}
    biome_veg_profiles: list[tuple[int, int, int, int]] = []
    # (tree_style, tree_per_chunk_x10, grass_patches, flower_patches)
    # tree_style: 0=none, 1=oak, 2=birch, 3=spruce, 4=oak_birch, 5=dark_oak, 6=acacia, 7=jungle, 8=cherry, 9=desert_cactus, 10=swamp

    for bname in ow_biomes:
        bidx = ow_biome_idx[bname]
        rel = bname.removeprefix("minecraft:")
        bjson = json.loads((WORLDGEN_DIR / "biome" / f"{rel}.json").read_text(encoding="utf-8"))
        feats = bjson.get("features", [])
        if len(feats) > 6:
            for pf in feats[6]:
                if isinstance(pf, str) and pf.startswith("minecraft:ore_"):
                    ore_biome_masks[pf] = ore_biome_masks.get(pf, 0) | (1 << bidx)

        veg_list = feats[9] if len(feats) > 9 else []
        veg_str = " ".join(str(x) for x in veg_list)
        if "cherry" in rel or "cherry" in veg_str:
            prof = (8, 55, 6, 8)
        elif "dark_forest" in rel or "pale_garden" in rel:
            prof = (5, 90, 4, 2)
        elif "birch" in rel:
            prof = (2, 75, 5, 3)
        elif "taiga" in rel or "grove" in rel or "windswept_forest" in rel:
            prof = (3, 70, 5, 1)
        elif "jungle" in rel:
            prof = (7, 95, 10, 2)
        elif "savanna" in rel:
            prof = (6, 20, 8, 1)
        elif "swamp" in rel:
            prof = (10, 35, 6, 3)
        elif "forest" in rel or "meadow" in rel:
            prof = (4, 65 if "forest" in rel else 10, 6, 6)
        elif "desert" in rel or "badlands" in rel:
            prof = (9, 8, 0, 0)
        elif "plains" in rel or "sunflower" in rel:
            prof = (1, 5, 10, 6)
        elif "ocean" in rel or "river" in rel or "beach" in rel or "stony" in rel or "peaks" in rel or "cave" in rel or "deep_dark" in rel:
            prof = (0, 0, 1, 0)
        else:
            prof = (1, 12, 4, 2)
        biome_veg_profiles.append(prof)

    # Compile every referenced Overworld ore placed_feature
    ore_specs = []
    pf_dir = WORLDGEN_DIR / "placed_feature"
    cf_dir = WORLDGEN_DIR / "feature"

    for pf_name, mask in sorted(ore_biome_masks.items()):
        pf_rel = pf_name.removeprefix("minecraft:")
        pf_path = pf_dir / f"{pf_rel}.json"
        if not pf_path.is_file():
            continue
        pf = json.loads(pf_path.read_text(encoding="utf-8"))
        cf_name = pf.get("feature", "")
        if not isinstance(cf_name, str):
            continue
        cf_path = cf_dir / f"{cf_name.removeprefix('minecraft:')}.json"
        if not cf_path.is_file():
            continue
        cf = json.loads(cf_path.read_text(encoding="utf-8"))
        if cf.get("type") != "minecraft:ore":
            continue

        size = int(cf.get("size", 8))
        discard = float(cf.get("discard_chance_on_air_exposure", 0.0))
        stone_state = 0
        deepslate_state = 0
        for tgt in cf.get("targets", []):
            st = tgt.get("state")
            sname = st["Name"] if isinstance(st, dict) else st
            sid = block_default.get(sname, 0)
            if sid == 0:
                continue
            tstr = json.dumps(tgt.get("target", {}))
            if "deepslate" in sname or "deepslate_ore_replaceables" in tstr:
                deepslate_state = sid
            else:
                stone_state = sid
        if stone_state == 0 and deepslate_state == 0:
            continue
        if deepslate_state == 0:
            deepslate_state = stone_state

        cmin, cmax = 1, 1
        rarity = 1
        min_y, max_y = -64, 64
        trapezoid = False
        for pl in pf.get("placement", []):
            pt = pl.get("type")
            if pt == "minecraft:count":
                cmin, cmax = parse_count(pl.get("count", 1))
            elif pt == "minecraft:rarity_filter":
                rarity = max(1, int(pl.get("chance", 1)))
            elif pt == "minecraft:height_range":
                h = pl.get("height", {})
                min_y = resolve_anchor(h.get("min_inclusive", -64))
                max_y = resolve_anchor(h.get("max_inclusive", 64))
                trapezoid = h.get("type") == "minecraft:trapezoid"

        ore_specs.append(
            (pf_name, mask, stone_state, deepslate_state, size, cmin, cmax, rarity, min_y, max_y, trapezoid, discard)
        )

    lines = [
        "// @generated by scripts/gen_features.py from Minecraft 26.3 Overworld placed_feature, feature, and biome JSONs.",
        "// Do not edit manually.",
        "",
        "#[derive(Debug, Clone, Copy)]",
        "pub struct OreFeatureSpec {",
        "    pub name: &'static str,",
        "    pub biome_mask: u64,",
        "    pub stone_state: u32,",
        "    pub deepslate_state: u32,",
        "    pub size: u8,",
        "    pub count_min: u8,",
        "    pub count_max: u8,",
        "    pub rarity: u8,",
        "    pub min_y: i16,",
        "    pub max_y: i16,",
        "    pub trapezoid: bool,",
        "    pub discard_on_air: f32,",
        "}",
        "",
        "#[derive(Debug, Clone, Copy)]",
        "pub struct BiomeVegProfile {",
        "    /// 0=none, 1=oak, 2=birch, 3=spruce, 4=oak+birch, 5=dark_oak, 6=acacia, 7=jungle, 8=cherry, 9=cactus, 10=swamp",
        "    pub tree_style: u8,",
        "    /// Expected trees per chunk * 10 (e.g., 75 = 7.5 trees/chunk).",
        "    pub tree_density_x10: u8,",
        "    pub grass_patches: u8,",
        "    pub flower_patches: u8,",
        "}",
        "",
        f"pub static OVERWORLD_ORE_SPECS: [OreFeatureSpec; {len(ore_specs)}] = [",
    ]
    for name, mask, ss, ds, sz, cmin, cmax, rar, mny, mxy, trap, disc in ore_specs:
        lines.append(
            f"    OreFeatureSpec {{ name: {json.dumps(name)}, biome_mask: 0x{mask:016x}_u64, stone_state: {ss}, deepslate_state: {ds}, "
            f"size: {sz}, count_min: {cmin}, count_max: {cmax}, rarity: {rar}, min_y: {mny}, max_y: {mxy}, "
            f"trapezoid: {'true' if trap else 'false'}, discard_on_air: {disc!r}_f32 }},"
        )
    lines.append("];")
    lines.append("")
    lines.append(f"pub static OVERWORLD_BIOME_VEG: [BiomeVegProfile; {len(biome_veg_profiles)}] = [")
    for ts, td, gp, fp in biome_veg_profiles:
        lines.append(
            f"    BiomeVegProfile {{ tree_style: {ts}, tree_density_x10: {td}, grass_patches: {gp}, flower_patches: {fp} }},"
        )
    lines.append("];")
    lines.append("")

    OUT_PATH.write_text("\n".join(lines), encoding="utf-8")
    print(f"Generated {OUT_PATH.name}: {len(ore_specs)} ore specs across {len(biome_veg_profiles)} Overworld biomes")


if __name__ == "__main__":
    main()
