#!/usr/bin/env python3
"""
Compiles Minecraft 26.3's complete Overworld terrain pipeline:
  - NoiseRouter (8 DensityFunction roots)
  - Aquifers (6 DensityFunction roots)
  - MaterialRules (all 37 reachable Overworld rules, 8 named MaterialConditions, Copper & Iron OreVeins)
  - Biome base temperatures (for TemperatureCondition)
into a deduplicated, zero-allocation static Rust module (`rust/crates/mc-worldgen/src/generated_overworld.rs`).
"""

import json
import struct
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DATA_MC = REPO_ROOT / "src" / "main" / "resources" / "data" / "minecraft"
WORLDGEN_DIR = DATA_MC / "worldgen"
REPORTS_DIR = REPO_ROOT / "generated" / "reports"
OUT_PATH = REPO_ROOT / "rust" / "crates" / "mc-worldgen" / "src" / "generated_overworld.rs"

OW_MIN_Y = -64
OW_HEIGHT = 384
OW_MAX_Y = OW_MIN_Y + OW_HEIGHT - 1


def to_f32(val: float) -> float:
    return struct.unpack("<f", struct.pack("<f", float(val)))[0]


def f32_bits(val: float) -> int:
    return struct.unpack("<I", struct.pack("<f", float(val)))[0]


def fmt_f32(val: float) -> str:
    bits = f32_bits(val)
    return f"f32::from_bits(0x{bits:08x}_u32)"


def fmt_f64(val: float) -> str:
    bits = struct.unpack("<Q", struct.pack("<d", float(val)))[0]
    return f"f64::from_bits(0x{bits:016x}_u64)"


def resolve_vertical_anchor(anchor: dict) -> int:
    if "absolute" in anchor:
        return int(anchor["absolute"])
    if "above_bottom" in anchor:
        return OW_MIN_Y + int(anchor["above_bottom"])
    if "below_top" in anchor:
        return OW_MAX_Y - int(anchor["below_top"])
    raise ValueError(f"Unknown VerticalAnchor: {anchor}")


class OverworldCompiler:
    def __init__(self) -> None:
        # Load block state lookup from generated/reports/blocks.json
        blocks = json.loads((REPORTS_DIR / "blocks.json").read_text(encoding="utf-8"))
        self.block_default: dict[str, int] = {}
        self.block_by_key: dict[tuple[str, tuple[tuple[str, str], ...]], int] = {}
        for bname, bdata in blocks.items():
            for st in bdata["states"]:
                sid = int(st["id"])
                props = tuple(sorted((st.get("properties") or {}).items()))
                self.block_by_key[(bname, props)] = sid
                if st.get("default"):
                    self.block_default[bname] = sid

        # Load named DensityFunctions
        df_dir = WORLDGEN_DIR / "density_function"
        self.named_dfs: dict[str, object] = {}
        for p in sorted(df_dir.rglob("*.json")):
            key = "minecraft:" + p.relative_to(df_dir).as_posix()[:-5]
            self.named_dfs[key] = json.loads(p.read_text(encoding="utf-8"))

        # Load named MaterialRules & MaterialConditions
        mr_dir = WORLDGEN_DIR / "material_rule"
        self.named_mrs: dict[str, object] = {}
        for p in sorted(mr_dir.rglob("*.json")):
            key = "minecraft:" + p.relative_to(mr_dir).as_posix()[:-5]
            self.named_mrs[key] = json.loads(p.read_text(encoding="utf-8"))

        mc_dir = WORLDGEN_DIR / "material_condition"
        self.named_mcs: dict[str, object] = {}
        for p in sorted(mc_dir.rglob("*.json")):
            key = "minecraft:" + p.relative_to(mc_dir).as_posix()[:-5]
            self.named_mcs[key] = json.loads(p.read_text(encoding="utf-8"))

        # Load biome tags for resolving `#minecraft:...` inside BiomeCondition
        btag_dir = DATA_MC / "tags" / "worldgen" / "biome"
        self.biome_tags: dict[str, list[str]] = {}
        if btag_dir.is_dir():
            for p in sorted(btag_dir.rglob("*.json")):
                tkey = "#minecraft:" + p.relative_to(btag_dir).as_posix()[:-5]
                raw = json.loads(p.read_text(encoding="utf-8"))
                self.biome_tags[tkey] = raw.get("values", [])

        # Load Overworld biome base temperatures for TemperatureCondition
        ow_bp = json.loads((REPORTS_DIR / "biome_parameters" / "minecraft" / "overworld.json").read_text(encoding="utf-8"))
        self.ow_biome_names = sorted({e["biome"] for e in ow_bp["biomes"]})
        self.ow_biome_idx = {name: i for i, name in enumerate(self.ow_biome_names)}
        self.biome_temps: list[float] = []
        for bname in self.ow_biome_names:
            rel = bname.removeprefix("minecraft:")
            bjson = json.loads((WORLDGEN_DIR / "biome" / f"{rel}.json").read_text(encoding="utf-8"))
            self.biome_temps.append(float(bjson.get("temperature", 0.5)))

        # Deduplicated pools
        self.nodes: list[str] = []
        self.node_by_key: dict[tuple, int] = {}
        self.node_const_val: dict[int, float] = {}

        self.splines: list[str] = []
        self.spline_by_key: dict[tuple, int] = {}

        self.noise_ids: list[str] = []
        self.noise_idx: dict[str, int] = {}

        self.blended_specs: list[tuple[float, float, float, float, float]] = []
        self.blended_idx: dict[tuple, int] = {}

        self.mat_random_names: list[str] = []
        self.mat_random_idx: dict[str, int] = {}

        self.mat_conds: list[str] = []
        self.mat_cond_by_key: dict[tuple, int] = {}

        self.mat_rules: list[str] = []
        self.mat_rule_by_key: dict[tuple, int] = {}

    def resolve_block_state(self, raw) -> int:
        if isinstance(raw, str):
            return self.block_default[raw]
        name = raw["Name"]
        props = tuple(sorted((raw.get("Properties") or {}).items()))
        if not props:
            return self.block_default[name]
        return self.block_by_key.get((name, props), self.block_default[name])

    def expand_biome_set(self, raw) -> list[int]:
        items = raw if isinstance(raw, list) else [raw]
        out: set[int] = set()
        stack = list(items)
        while stack:
            cur = stack.pop()
            if isinstance(cur, dict):
                cur = cur["id"]
            if cur.startswith("#"):
                stack.extend(self.biome_tags.get(cur, []))
            elif cur in self.ow_biome_idx:
                out.add(self.ow_biome_idx[cur])
        return sorted(out)

    def intern_noise(self, noise_id: str) -> int:
        if noise_id not in self.noise_idx:
            self.noise_idx[noise_id] = len(self.noise_ids)
            self.noise_ids.append(noise_id)
        return self.noise_idx[noise_id]

    def intern_random_name(self, name: str) -> int:
        if name not in self.mat_random_idx:
            self.mat_random_idx[name] = len(self.mat_random_names)
            self.mat_random_names.append(name)
        return self.mat_random_idx[name]

    def intern_blended(self, xz_s: float, y_s: float, xz_f: float, y_f: float, smear: float) -> int:
        key = tuple(struct.unpack("<Q", struct.pack("<d", x))[0] for x in (xz_s, y_s, xz_f, y_f, smear))
        if key not in self.blended_idx:
            self.blended_idx[key] = len(self.blended_specs)
            self.blended_specs.append((xz_s, y_s, xz_f, y_f, smear))
        return self.blended_idx[key]

    def intern_node(self, key: tuple, rust_expr: str, const_val: float | None = None) -> int:
        if key in self.node_by_key:
            return self.node_by_key[key]
        idx = len(self.nodes)
        self.nodes.append(rust_expr)
        self.node_by_key[key] = idx
        if const_val is not None:
            self.node_const_val[idx] = const_val
        return idx

    def intern_spline(self, key: tuple, rust_expr: str) -> int:
        if key in self.spline_by_key:
            return self.spline_by_key[key]
        idx = len(self.splines)
        self.splines.append(rust_expr)
        self.spline_by_key[key] = idx
        return idx

    def intern_cond(self, key: tuple, rust_expr: str) -> int:
        if key in self.mat_cond_by_key:
            return self.mat_cond_by_key[key]
        idx = len(self.mat_conds)
        self.mat_conds.append(rust_expr)
        self.mat_cond_by_key[key] = idx
        return idx

    def intern_rule(self, key: tuple, rust_expr: str) -> int:
        if key in self.mat_rule_by_key:
            return self.mat_rule_by_key[key]
        idx = len(self.mat_rules)
        self.mat_rules.append(rust_expr)
        self.mat_rule_by_key[key] = idx
        return idx

    def compile_spline(self, obj) -> int:
        if isinstance(obj, (int, float)):
            return self.intern_spline(("const", f32_bits(obj)), f"SplineNode::Constant({fmt_f32(obj)})")
        coord_id = self.compile_df(obj["coordinate"])
        pts = obj["points"]
        locs = [f32_bits(p["location"]) for p in pts]
        vals = [self.compile_spline(p["value"]) for p in pts]
        derivs = [f32_bits(p["derivative"]) for p in pts]
        key = ("mp", coord_id, tuple(locs), tuple(vals), tuple(derivs))
        locs_s = ", ".join(fmt_f32(p["location"]) for p in pts)
        vals_s = ", ".join(str(v) for v in vals)
        derivs_s = ", ".join(fmt_f32(p["derivative"]) for p in pts)
        expr = (
            f"SplineNode::Multipoint {{ coord: {coord_id}, "
            f"locations: &[{locs_s}], values: &[{vals_s}], derivatives: &[{derivs_s}] }}"
        )
        return self.intern_spline(key, expr)

    def compile_df(self, obj) -> int:
        if isinstance(obj, (int, float)):
            v = to_f32(obj)
            return self.intern_node(("const", f32_bits(v)), f"DfNode::Constant({fmt_f32(v)})", const_val=v)
        if isinstance(obj, str):
            return self.compile_df(self.named_dfs[obj])

        t = obj["type"]
        if t == "minecraft:blend_alpha":
            return self.compile_df(1.0)
        if t in ("minecraft:blend_offset", "minecraft:beardifier"):
            return self.compile_df(0.0)
        if t == "minecraft:blend_density":
            return self.compile_df(obj["input"])
        if t == "minecraft:cache":
            inner = self.compile_df(obj["input"])
            return self.intern_node(("cache", inner), f"DfNode::Cache({inner})")

        unary_map = {
            "minecraft:abs": "UnaryOp::Abs",
            "minecraft:square": "UnaryOp::Square",
            "minecraft:cube": "UnaryOp::Cube",
            "minecraft:sqrt": "UnaryOp::Sqrt",
            "minecraft:half_negative": "UnaryOp::HalfNegative",
            "minecraft:quarter_negative": "UnaryOp::QuarterNegative",
            "minecraft:reciprocal": "UnaryOp::Reciprocal",
            "minecraft:negate": "UnaryOp::Negate",
            "minecraft:squeeze": "UnaryOp::Squeeze",
        }
        if t in unary_map:
            inp = self.compile_df(obj["input"])
            op = unary_map[t]
            return self.intern_node(("unary", op, inp), f"DfNode::Unary {{ op: {op}, input: {inp} }}")

        if t in ("minecraft:add", "minecraft:sub", "minecraft:mul", "minecraft:div", "minecraft:min", "minecraft:max"):
            left = self.compile_df(obj["left"])
            right = self.compile_df(obj["right"])
            lc = self.node_const_val.get(left)
            rc = self.node_const_val.get(right)
            if t == "minecraft:add":
                if lc is not None:
                    return self.intern_node(("cadd", right, f32_bits(lc)), f"DfNode::ConstAdd {{ input: {right}, c: {fmt_f32(lc)} }}")
                if rc is not None:
                    return self.intern_node(("cadd", left, f32_bits(rc)), f"DfNode::ConstAdd {{ input: {left}, c: {fmt_f32(rc)} }}")
                return self.intern_node(("bin", "Add", left, right), f"DfNode::Binary {{ op: BinaryOp::Add, left: {left}, right: {right} }}")
            if t == "minecraft:sub":
                if lc is not None:
                    return self.intern_node(("csub", f32_bits(lc), right), f"DfNode::ConstSub {{ c: {fmt_f32(lc)}, right: {right} }}")
                if rc is not None:
                    neg_rc = to_f32(-rc)
                    return self.intern_node(("cadd", left, f32_bits(neg_rc)), f"DfNode::ConstAdd {{ input: {left}, c: {fmt_f32(neg_rc)} }}")
                return self.intern_node(("bin", "Sub", left, right), f"DfNode::Binary {{ op: BinaryOp::Sub, left: {left}, right: {right} }}")
            if t == "minecraft:mul":
                if lc is not None:
                    return self.intern_node(("cmul", right, f32_bits(lc)), f"DfNode::ConstMul {{ input: {right}, c: {fmt_f32(lc)} }}")
                if rc is not None:
                    return self.intern_node(("cmul", left, f32_bits(rc)), f"DfNode::ConstMul {{ input: {left}, c: {fmt_f32(rc)} }}")
                return self.intern_node(("bin", "Mul", left, right), f"DfNode::Binary {{ op: BinaryOp::Mul, left: {left}, right: {right} }}")
            if t == "minecraft:div":
                if lc is not None:
                    return self.intern_node(("cdiv", f32_bits(lc), right), f"DfNode::ConstDiv {{ c: {fmt_f32(lc)}, right: {right} }}")
                if rc is not None:
                    inv = to_f32(1.0 / rc)
                    return self.intern_node(("cmul", left, f32_bits(inv)), f"DfNode::ConstMul {{ input: {left}, c: {fmt_f32(inv)} }}")
                return self.intern_node(("bin", "Div", left, right), f"DfNode::Binary {{ op: BinaryOp::Div, left: {left}, right: {right} }}")
            if t == "minecraft:min":
                return self.intern_node(("bin", "Min", left, right), f"DfNode::Binary {{ op: BinaryOp::Min, left: {left}, right: {right} }}")
            if t == "minecraft:max":
                return self.intern_node(("bin", "Max", left, right), f"DfNode::Binary {{ op: BinaryOp::Max, left: {left}, right: {right} }}")

        if t == "minecraft:clamp":
            inp = self.compile_df(obj["input"])
            lo = to_f32(obj["min"])
            hi = to_f32(obj["max"])
            return self.intern_node(
                ("clamp", inp, f32_bits(lo), f32_bits(hi)),
                f"DfNode::Clamp {{ input: {inp}, min: {fmt_f32(lo)}, max: {fmt_f32(hi)} }}",
            )
        if t == "minecraft:lerp":
            a = self.compile_df(obj["alpha"])
            f1 = self.compile_df(obj["first"])
            f2 = self.compile_df(obj["second"])
            return self.intern_node(("lerp", a, f1, f2), f"DfNode::Lerp {{ alpha: {a}, first: {f1}, second: {f2} }}")
        if t == "minecraft:range_choice":
            inp = self.compile_df(obj["input"])
            lo = to_f32(obj["min_inclusive"])
            hi = to_f32(obj["max_exclusive"])
            ir = self.compile_df(obj["when_in_range"])
            orr = self.compile_df(obj["when_out_of_range"])
            return self.intern_node(
                ("rc", inp, f32_bits(lo), f32_bits(hi), ir, orr),
                f"DfNode::RangeChoice {{ input: {inp}, min_inclusive: {fmt_f32(lo)}, max_exclusive: {fmt_f32(hi)}, when_in_range: {ir}, when_out_of_range: {orr} }}",
            )
        if t == "minecraft:interval_select":
            inp = self.compile_df(obj["input"])
            th = [to_f32(x) for x in obj["thresholds"]]
            fns = [self.compile_df(f) for f in obj["functions"]]
            th_s = ", ".join(fmt_f32(x) for x in th)
            fns_s = ", ".join(str(x) for x in fns)
            return self.intern_node(
                ("isel", inp, tuple(f32_bits(x) for x in th), tuple(fns)),
                f"DfNode::IntervalSelect {{ input: {inp}, thresholds: &[{th_s}], functions: &[{fns_s}] }}",
            )
        if t == "minecraft:gradient":
            axis_map = {"x": "Axis::X", "y": "Axis::Y", "z": "Axis::Z"}
            tile_map = {
                "clamp_to_edge": "TilingMode::ClampToEdge",
                "repeat": "TilingMode::Repeat",
                "mirrored_repeat": "TilingMode::MirroredRepeat",
            }
            axis = axis_map[obj["axis"]]
            tiling = tile_map[obj.get("tiling", "clamp_to_edge")]
            fc = int(obj["from_coordinate"])
            tc = int(obj["to_coordinate"])
            fv = to_f32(obj["from_value"])
            tv = to_f32(obj["to_value"])
            return self.intern_node(
                ("grad", axis, tiling, fc, tc, f32_bits(fv), f32_bits(tv)),
                f"DfNode::Gradient {{ axis: {axis}, tiling: {tiling}, from_coord: {fc}, to_coord: {tc}, from_val: {fmt_f32(fv)}, to_val: {fmt_f32(tv)} }}",
            )
        if t == "minecraft:noise":
            nid = self.intern_noise(obj["noise"])
            xz_s = float(obj["xz_scale"])
            y_s = float(obj["y_scale"])
            sx = self.compile_df(obj["shift_x"]) if "shift_x" in obj else None
            sy = self.compile_df(obj["shift_y"]) if "shift_y" in obj else None
            sz = self.compile_df(obj["shift_z"]) if "shift_z" in obj else None
            sx_s = f"Some({sx})" if sx is not None else "None"
            sy_s = f"Some({sy})" if sy is not None else "None"
            sz_s = f"Some({sz})" if sz is not None else "None"
            return self.intern_node(
                ("noise", nid, struct.unpack("<Q", struct.pack("<d", xz_s))[0], struct.unpack("<Q", struct.pack("<d", y_s))[0], sx, sy, sz),
                f"DfNode::Noise {{ noise_idx: {nid}, xz_scale: {fmt_f64(xz_s)}, y_scale: {fmt_f64(y_s)}, shift_x: {sx_s}, shift_y: {sy_s}, shift_z: {sz_s} }}",
            )
        if t == "minecraft:shift_a":
            nid = self.intern_noise(obj["noise"])
            return self.intern_node(("shift_a", nid), f"DfNode::ShiftA({nid})")
        if t == "minecraft:shift_b":
            nid = self.intern_noise(obj["noise"])
            return self.intern_node(("shift_b", nid), f"DfNode::ShiftB({nid})")
        if t == "minecraft:old_blended_noise":
            bid = self.intern_blended(
                float(obj["xz_scale"]),
                float(obj["y_scale"]),
                float(obj["xz_factor"]),
                float(obj["y_factor"]),
                float(obj["smear_scale_multiplier"]),
            )
            return self.intern_node(("blended", bid), f"DfNode::OldBlendedNoise({bid})")
        if t == "minecraft:interpolated":
            inp = self.compile_df(obj["input"])
            cxz = int(obj["cell_size_xz"])
            cy = int(obj["cell_size_y"])
            return self.intern_node(("interp", inp, cxz, cy), f"DfNode::Interpolated {{ input: {inp}, cell_size_xz: {cxz}, cell_size_y: {cy} }}")
        if t == "minecraft:find_top_surface":
            dens = self.compile_df(obj["density"])
            upper = self.compile_df(obj["upper_bound"])
            lower = int(obj["lower_bound"])
            ch = int(obj["cell_height"])
            return self.intern_node(
                ("fts", dens, upper, lower, ch),
                f"DfNode::FindTopSurface {{ density: {dens}, upper_bound: {upper}, lower_bound: {lower}, cell_height: {ch} }}",
            )
        if t == "minecraft:spline":
            sp_id = self.compile_spline(obj["spline"])
            return self.intern_node(("spline", sp_id), f"DfNode::Spline({sp_id})")
        if t == "minecraft:slice":
            axis_map = {"x": "Axis::X", "y": "Axis::Y", "z": "Axis::Z"}
            axis = axis_map[obj["axis"]]
            coord = int(obj["coordinate"])
            inp = self.compile_df(obj["input"])
            return self.intern_node(("slice", axis, coord, inp), f"DfNode::Slice {{ axis: {axis}, coord: {coord}, input: {inp} }}")
        raise ValueError(f"Unsupported DensityFunction type: {t}")

    def compile_mat_cond(self, obj) -> int:
        if isinstance(obj, str):
            return self.compile_mat_cond(self.named_mcs[obj])
        t = obj["type"]
        if t == "minecraft:above_preliminary_surface":
            return self.intern_cond(("above_ps",), "MatCond::AbovePreliminarySurface")
        if t == "minecraft:hole":
            return self.intern_cond(("hole",), "MatCond::Hole")
        if t == "minecraft:steep":
            return self.intern_cond(("steep",), "MatCond::Steep")
        if t == "minecraft:temperature":
            return self.intern_cond(("temp",), "MatCond::Temperature")
        if t == "minecraft:not":
            inner = self.compile_mat_cond(obj["invert"])
            return self.intern_cond(("not", inner), f"MatCond::Not({inner})")
        if t == "minecraft:biome":
            b_ids = self.expand_biome_set(obj["biome_is"])
            # Pack into u64 bitmask (Overworld has 56 biomes <= 64!)
            mask = 0
            for b in b_ids:
                mask |= 1 << b
            return self.intern_cond(("biome", mask), f"MatCond::BiomeMask(0x{mask:016x}_u64)")
        if t == "minecraft:stone_depth":
            off = int(obj["offset"])
            add_sd = bool(obj["add_surface_depth"])
            sec_range = int(obj["secondary_depth_range"])
            ceiling = obj["surface_type"] == "ceiling"
            return self.intern_cond(
                ("sd", off, add_sd, sec_range, ceiling),
                f"MatCond::StoneDepth {{ offset: {off}, add_surface_depth: {str(add_sd).lower()}, secondary_depth_range: {sec_range}, ceiling: {str(ceiling).lower()} }}",
            )
        if t == "minecraft:water":
            off = int(obj["offset"])
            mult = int(obj["surface_depth_multiplier"])
            add_st = bool(obj["add_stone_depth"])
            return self.intern_cond(
                ("water", off, mult, add_st),
                f"MatCond::Water {{ offset: {off}, surface_depth_multiplier: {mult}, add_stone_depth: {str(add_st).lower()} }}",
            )
        if t == "minecraft:y_above":
            ay = resolve_vertical_anchor(obj["anchor"])
            mult = int(obj["surface_depth_multiplier"])
            add_st = bool(obj["add_stone_depth"])
            return self.intern_cond(
                ("yabove", ay, mult, add_st),
                f"MatCond::YAbove {{ anchor_y: {ay}, surface_depth_multiplier: {mult}, add_stone_depth: {str(add_st).lower()} }}",
            )
        if t == "minecraft:vertical_gradient":
            ridx = self.intern_random_name(obj["random_name"])
            lo = resolve_vertical_anchor(obj["true_at_and_below"])
            hi = resolve_vertical_anchor(obj["false_at_and_above"])
            return self.intern_cond(
                ("vgrad", ridx, lo, hi),
                f"MatCond::VerticalGradient {{ random_idx: {ridx}, true_at_and_below: {lo}, false_at_and_above: {hi} }}",
            )
        if t == "minecraft:noise_threshold":
            nidx = self.intern_noise(obj["noise"])
            mn = float(obj["min_threshold"])
            mx = float(obj["max_threshold"])
            is_3d = bool(obj.get("is_3d", False))
            return self.intern_cond(
                ("nth", nidx, struct.unpack("<Q", struct.pack("<d", mn))[0], struct.unpack("<Q", struct.pack("<d", mx))[0], is_3d),
                f"MatCond::NoiseThreshold {{ noise_idx: {nidx}, min_threshold: {fmt_f64(mn)}, max_threshold: {fmt_f64(mx)}, is_3d: {str(is_3d).lower()} }}",
            )
        raise ValueError(f"Unsupported MaterialCondition type: {t}")

    def compile_mat_rule(self, obj) -> int:
        if isinstance(obj, str):
            return self.compile_mat_rule(self.named_mrs[obj])
        t = obj["type"]
        if t == "minecraft:block":
            sid = self.resolve_block_state(obj["result_state"])
            return self.intern_rule(("block", sid), f"MatRule::Block({sid})")
        if t == "minecraft:bandlands":
            return self.intern_rule(("bandlands",), "MatRule::Bandlands")
        if t == "minecraft:condition":
            cid = self.compile_mat_cond(obj["if_true"])
            rid = self.compile_mat_rule(obj["then_run"])
            return self.intern_rule(("cond", cid, rid), f"MatRule::Condition {{ cond: {cid}, then_run: {rid} }}")
        if t == "minecraft:sequence":
            children = [self.compile_mat_rule(r) for r in obj["sequence"]]
            cs = ", ".join(str(x) for x in children)
            return self.intern_rule(("seq", tuple(children)), f"MatRule::Sequence(&[{cs}])")
        if t == "minecraft:ore_vein":
            ore = self.resolve_block_state(obj["ore_block"])
            raw = self.resolve_block_state(obj["raw_ore_block"])
            filler = self.resolve_block_state(obj["filler_block"])
            chance = to_f32(obj["raw_ore_chance"])
            dens = self.compile_df(obj["density"])
            rich = self.compile_df(obj["richness"])
            gap = self.compile_df(obj["filler_gap"])
            return self.intern_rule(
                ("ore", ore, raw, filler, f32_bits(chance), dens, rich, gap),
                f"MatRule::OreVein {{ ore_block: {ore}, raw_ore_block: {raw}, filler_block: {filler}, raw_ore_chance: {fmt_f32(chance)}, density_node: {dens}, richness_node: {rich}, filler_gap_node: {gap} }}",
            )
        raise ValueError(f"Unsupported MaterialRule type: {t}")


def main() -> None:
    ow = json.loads((WORLDGEN_DIR / "noise_settings" / "overworld.json").read_text(encoding="utf-8"))
    router = ow["noise_router"]
    aq = ow["aquifers"]
    c = OverworldCompiler()

    # Ensure MaterialSystem's built-in noises are always interned too!
    surface_noise_idx = c.intern_noise("minecraft:surface")
    surface_sec_noise_idx = c.intern_noise("minecraft:surface_secondary")
    clay_bands_offset_idx = c.intern_noise("minecraft:clay_bands_offset")

    roots = {
        "TEMPERATURE": c.compile_df(router["temperature"]),
        "VEGETATION": c.compile_df(router["vegetation"]),
        "CONTINENTS": c.compile_df(router["continents"]),
        "EROSION": c.compile_df(router["erosion"]),
        "DEPTH": c.compile_df(router["depth"]),
        "RIDGES": c.compile_df(router["ridges"]),
        "CHUNK_SURFACE_LEVEL": c.compile_df(router["chunk_surface_level"]),
        "FINAL_DENSITY": c.compile_df(router["final_density"]),
        "AQ_BARRIER": c.compile_df(aq["barrier"]),
        "AQ_FLOODEDNESS": c.compile_df(aq["fluid_level_floodedness"]),
        "AQ_SPREAD": c.compile_df(aq["fluid_level_spread"]),
        "AQ_LAVA": c.compile_df(aq["lava"]),
        "AQ_EXCLUSION": c.compile_df(aq["exclusion"]),
        "AQ_SURFACE_LEVEL": c.compile_df(aq["surface_level"]),
    }

    ow_mat_root = c.compile_mat_rule(ow["material_rule"])

    lines = [
        "// @generated by scripts/gen_overworld_density.py from Minecraft 26.3 Overworld worldgen JSONs.",
        "// Do not edit manually.",
        "",
        "use crate::density::{Axis, BinaryOp, BlendedNoiseSpec, DfNode, SplineNode, TilingMode, UnaryOp};",
        "use crate::material::{MatCond, MatRule};",
        "",
    ]
    for name, idx in roots.items():
        lines.append(f"pub const OW_ROOT_{name}: usize = {idx};")
    lines.append(f"pub const OW_MATERIAL_ROOT: usize = {ow_mat_root};")
    lines.append(f"pub const OW_NOISE_SURFACE_IDX: usize = {surface_noise_idx};")
    lines.append(f"pub const OW_NOISE_SURFACE_SECONDARY_IDX: usize = {surface_sec_noise_idx};")
    lines.append(f"pub const OW_NOISE_CLAY_BANDS_OFFSET_IDX: usize = {clay_bands_offset_idx};")
    lines.append("")

    lines.append(f"pub static OW_NOISE_IDS: [&str; {len(c.noise_ids)}] = [")
    for nid in c.noise_ids:
        lines.append(f"    {json.dumps(nid)},")
    lines.append("];")
    lines.append("")

    lines.append(f"pub static OW_BLENDED_SPECS: [BlendedNoiseSpec; {len(c.blended_specs)}] = [")
    for xz_s, y_s, xz_f, y_f, smear in c.blended_specs:
        lines.append(
            f"    BlendedNoiseSpec {{ xz_scale: {fmt_f64(xz_s)}, y_scale: {fmt_f64(y_s)}, xz_factor: {fmt_f64(xz_f)}, y_factor: {fmt_f64(y_f)}, smear_scale_multiplier: {fmt_f64(smear)} }},"
        )
    lines.append("];")
    lines.append("")

    lines.append(f"pub static OW_SPLINES: [SplineNode; {len(c.splines)}] = [")
    for expr in c.splines:
        lines.append(f"    {expr},")
    lines.append("];")
    lines.append("")

    lines.append(f"pub static OW_NODES: [DfNode; {len(c.nodes)}] = [")
    for expr in c.nodes:
        lines.append(f"    {expr},")
    lines.append("];")
    lines.append("")

    lines.append(f"pub static OW_MAT_RANDOM_NAMES: [&str; {len(c.mat_random_names)}] = [")
    for rn in c.mat_random_names:
        lines.append(f"    {json.dumps(rn)},")
    lines.append("];")
    lines.append("")

    lines.append(f"pub static OW_BIOME_TEMPS: [f32; {len(c.biome_temps)}] = [")
    for t in c.biome_temps:
        lines.append(f"    {fmt_f32(t)},")
    lines.append("];")
    lines.append("")

    lines.append(f"pub static OW_MAT_CONDS: [MatCond; {len(c.mat_conds)}] = [")
    for expr in c.mat_conds:
        lines.append(f"    {expr},")
    lines.append("];")
    lines.append("")

    lines.append(f"pub static OW_MAT_RULES: [MatRule; {len(c.mat_rules)}] = [")
    for expr in c.mat_rules:
        lines.append(f"    {expr},")
    lines.append("];")
    lines.append("")

    OUT_PATH.write_text("\n".join(lines), encoding="utf-8")
    print(
        f"Generated {OUT_PATH.name}: {len(c.nodes)} DfNodes, {len(c.splines)} SplineNodes, "
        f"{len(c.noise_ids)} noises, {len(c.mat_rules)} MatRules, {len(c.mat_conds)} MatConds."
    )


if __name__ == "__main__":
    main()
