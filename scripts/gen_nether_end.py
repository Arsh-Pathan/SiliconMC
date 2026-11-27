#!/usr/bin/env python3
"""
Compiles Minecraft 26.3 Nether biomes, MaterialRules, and MaterialConditions
from `src/main/resources/data/minecraft/worldgen/` and `generated/reports/`
into `rust/crates/mc-worldgen/src/generated_nether.rs`.
"""

import json
import struct
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DATA_MC = REPO_ROOT / "src" / "main" / "resources" / "data" / "minecraft"
WORLDGEN_DIR = DATA_MC / "worldgen"
REPORTS_DIR = REPO_ROOT / "generated" / "reports"
OUT_PATH = REPO_ROOT / "rust" / "crates" / "mc-worldgen" / "src" / "generated_nether.rs"

NETHER_MIN_Y = 0
NETHER_HEIGHT = 128
NETHER_MAX_Y = NETHER_MIN_Y + NETHER_HEIGHT - 1


def quantize_coord(val: float) -> int:
    f32_val = struct.unpack("<f", struct.pack("<f", val))[0]
    prod = struct.unpack("<f", struct.pack("<f", f32_val * 10000.0))[0]
    return int(prod)


def fmt_f64(val: float) -> str:
    bits = struct.unpack("<Q", struct.pack("<d", float(val)))[0]
    return f"f64::from_bits(0x{bits:016x}_u64)"


def resolve_anchor(anchor: dict) -> int:
    if "absolute" in anchor:
        return int(anchor["absolute"])
    if "above_bottom" in anchor:
        return NETHER_MIN_Y + int(anchor["above_bottom"])
    if "below_top" in anchor:
        return NETHER_MAX_Y - int(anchor["below_top"])
    raise ValueError(f"Unknown anchor: {anchor}")


def main() -> None:
    blocks = json.loads((REPORTS_DIR / "blocks.json").read_text(encoding="utf-8"))
    block_default: dict[str, int] = {}
    for bname, bdata in blocks.items():
        for st in bdata["states"]:
            if st.get("default"):
                block_default[bname] = int(st["id"])

    nether_bp = json.loads(
        (REPORTS_DIR / "biome_parameters" / "minecraft" / "nether.json").read_text(encoding="utf-8")
    )
    biome_names = sorted({e["biome"] for e in nether_bp["biomes"]})
    biome_idx = {name: i for i, name in enumerate(biome_names)}

    mr_dir = WORLDGEN_DIR / "material_rule"
    named_mrs = {
        "minecraft:" + p.relative_to(mr_dir).as_posix()[:-5]: json.loads(p.read_text(encoding="utf-8"))
        for p in sorted(mr_dir.rglob("*.json"))
    }
    mc_dir = WORLDGEN_DIR / "material_condition"
    named_mcs = {
        "minecraft:" + p.relative_to(mc_dir).as_posix()[:-5]: json.loads(p.read_text(encoding="utf-8"))
        for p in sorted(mc_dir.rglob("*.json"))
    }

    noise_ids: list[str] = ["minecraft:surface", "minecraft:surface_secondary"]
    noise_idx: dict[str, int] = {n: i for i, n in enumerate(noise_ids)}

    def intern_noise(nid: str) -> int:
        if nid not in noise_idx:
            noise_idx[nid] = len(noise_ids)
            noise_ids.append(nid)
        return noise_idx[nid]

    random_names: list[str] = []
    random_idx: dict[str, int] = {}

    def intern_random(rname: str) -> int:
        if rname not in random_idx:
            random_idx[rname] = len(random_names)
            random_names.append(rname)
        return random_idx[rname]

    mat_conds: list[str] = []
    mat_rules: list[str] = []

    def compile_cond(raw) -> int:
        if isinstance(raw, str):
            return compile_cond(named_mcs[raw])
        t = raw["type"]
        if t == "minecraft:hole":
            expr = "MatCond::Hole"
        elif t == "minecraft:not":
            inner = compile_cond(raw["invert"])
            expr = f"MatCond::Not({inner})"
        elif t == "minecraft:biome":
            b = raw["biome_is"]
            blist = [b] if isinstance(b, str) else b
            mask = 0
            for bn in blist:
                if bn in biome_idx:
                    mask |= 1 << biome_idx[bn]
            expr = f"MatCond::BiomeMask(0x{mask:016x}_u64)"
        elif t == "minecraft:stone_depth":
            offset = int(raw.get("offset", 0))
            add_sd = "true" if raw.get("add_surface_depth", False) else "false"
            sec_range = int(raw.get("secondary_depth_range", 0))
            ceil = "true" if raw.get("surface_type") == "ceiling" else "false"
            expr = f"MatCond::StoneDepth {{ offset: {offset}, add_surface_depth: {add_sd}, secondary_depth_range: {sec_range}, ceiling: {ceil} }}"
        elif t == "minecraft:y_above":
            ay = resolve_anchor(raw["anchor"])
            sdm = int(raw.get("surface_depth_multiplier", 0))
            asd = "true" if raw.get("add_stone_depth", False) else "false"
            expr = f"MatCond::YAbove {{ anchor_y: {ay}, surface_depth_multiplier: {sdm}, add_stone_depth: {asd} }}"
        elif t == "minecraft:vertical_gradient":
            ri = intern_random(raw["random_name"])
            tb = resolve_anchor(raw["true_at_and_below"])
            fa = resolve_anchor(raw["false_at_and_above"])
            expr = f"MatCond::VerticalGradient {{ random_idx: {ri}, true_at_and_below: {tb}, false_at_and_above: {fa} }}"
        elif t == "minecraft:noise_threshold":
            ni = intern_noise(raw["noise"])
            mn = fmt_f64(raw["min_threshold"])
            mx = fmt_f64(raw["max_threshold"])
            is_3d = "true" if raw.get("noise") in ("minecraft:soul_sand_layer", "minecraft:gravel_layer", "minecraft:netherrack", "minecraft:nether_wart") else "false"
            expr = f"MatCond::NoiseThreshold {{ noise_idx: {ni}, min_threshold: {mn}, max_threshold: {mx}, is_3d: {is_3d} }}"
        else:
            raise ValueError(f"Unsupported Nether MatCond: {t}")
        idx = len(mat_conds)
        mat_conds.append(expr)
        return idx

    def compile_rule(raw) -> int:
        if isinstance(raw, str):
            return compile_rule(named_mrs[raw])
        t = raw["type"]
        if t == "minecraft:block":
            rs = raw["result_state"]
            bname = rs["Name"] if isinstance(rs, dict) else rs
            sid = block_default[bname]
            expr = f"MatRule::Block({sid})"
        elif t == "minecraft:condition":
            c = compile_cond(raw["if_true"])
            tr = compile_rule(raw["then_run"])
            expr = f"MatRule::Condition {{ cond: {c}, then_run: {tr} }}"
        elif t == "minecraft:sequence":
            children = [compile_rule(s) for s in raw["sequence"]]
            expr = "MatRule::Sequence(&[" + ", ".join(str(x) for x in children) + "])"
        else:
            raise ValueError(f"Unsupported Nether MatRule: {t}")
        idx = len(mat_rules)
        mat_rules.append(expr)
        return idx

    root_rule = compile_rule("minecraft:nether")

    lines = [
        "// @generated by scripts/gen_nether_end.py from Minecraft 26.3 Nether datapacks.",
        "// Do not edit manually.",
        "",
        "use crate::material::{MatCond, MatRule};",
        "use mc_data::overworld_biomes::ClimateEntry;",
        "",
        f"pub const NETHER_MATERIAL_ROOT: usize = {root_rule};",
        "pub const NETHER_NOISE_SURFACE_IDX: usize = 0;",
        "pub const NETHER_NOISE_SURFACE_SECONDARY_IDX: usize = 1;",
        "",
        f"pub static NETHER_BIOME_NAMES: [&str; {len(biome_names)}] = [",
    ]
    for b in biome_names:
        lines.append(f"    {json.dumps(b)},")
    lines.append("];")
    lines.append("")
    lines.append(f"pub static NETHER_CLIMATE_ENTRIES: [ClimateEntry; {len(nether_bp['biomes'])}] = [")
    for e in nether_bp["biomes"]:
        p = e["parameters"]
        t = quantize_coord(p["temperature"])
        h = quantize_coord(p["humidity"])
        c = quantize_coord(p["continentalness"])
        er = quantize_coord(p["erosion"])
        d = quantize_coord(p["depth"])
        w = quantize_coord(p["weirdness"])
        o = quantize_coord(p["offset"])
        bi = biome_idx[e["biome"]]
        lines.append(
            f"    ClimateEntry {{ params: [({t}, {t}), ({h}, {h}), ({c}, {c}), ({er}, {er}), ({d}, {d}), ({w}, {w}), ({o}, {o})], biome_idx: {bi} }},"
        )
    lines.append("];")
    lines.append("")
    lines.append(f"pub static NETHER_NOISE_IDS: [&str; {len(noise_ids)}] = [")
    for n in noise_ids:
        lines.append(f"    {json.dumps(n)},")
    lines.append("];")
    lines.append("")
    lines.append(f"pub static NETHER_MAT_RANDOM_NAMES: [&str; {len(random_names)}] = [")
    for rn in random_names:
        lines.append(f"    {json.dumps(rn)},")
    lines.append("];")
    lines.append("")
    lines.append(f"pub static NETHER_MAT_CONDS: [MatCond; {len(mat_conds)}] = [")
    for expr in mat_conds:
        lines.append(f"    {expr},")
    lines.append("];")
    lines.append("")
    lines.append(f"pub static NETHER_MAT_RULES: [MatRule; {len(mat_rules)}] = [")
    for expr in mat_rules:
        lines.append(f"    {expr},")
    lines.append("];")
    lines.append("")

    OUT_PATH.write_text("\n".join(lines), encoding="utf-8")
    print(
        f"Generated {OUT_PATH.name}: {len(biome_names)} biomes, {len(noise_ids)} noises, "
        f"{len(mat_rules)} MatRules, {len(mat_conds)} MatConds"
    )


if __name__ == "__main__":
    main()
