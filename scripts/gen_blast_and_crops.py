#!/usr/bin/env python3
"""
Generates exact Minecraft 26.3 blast resistance, crop progression, farmland moisture,
sapling/tree, leaf/log, flammability, fire/sugar-cane/cactus age tables, and key
block-state constants from decompiled Minecraft 26.3 Java sources + `generated/reports/blocks.json`
-> `rust/crates/mc-data/src/generated/block_extra.rs`.
"""

import json
import math
import re
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
BLOCKS_JSON = REPO_ROOT / "generated" / "reports" / "blocks.json"
BLOCKS_JAVA = REPO_ROOT / "src" / "main" / "java" / "net" / "minecraft" / "world" / "level" / "block" / "Blocks.java"
FIRE_JAVA = REPO_ROOT / "src" / "main" / "java" / "net" / "minecraft" / "world" / "level" / "block" / "FireBlock.java"
BLOCK_IDS_JAVA = REPO_ROOT / "src" / "main" / "java" / "net" / "minecraft" / "references" / "BlockIds.java"
BLOCK_ITEM_IDS_JAVA = REPO_ROOT / "src" / "main" / "java" / "net" / "minecraft" / "references" / "BlockItemIds.java"
BLOCK_TAGS_DIR = REPO_ROOT / "src" / "main" / "resources" / "data" / "minecraft" / "tags" / "block"
OUT_PATH = REPO_ROOT / "rust" / "crates" / "mc-data" / "src" / "generated" / "block_extra.rs"

DYE_COLORS = [
    "white", "orange", "magenta", "light_blue",
    "yellow", "lime", "pink", "gray",
    "light_gray", "cyan", "purple", "blue",
    "brown", "green", "red", "black",
]

COPPER_PREFIXES = [
    "", "exposed_", "weathered_", "oxidized_",
    "waxed_", "waxed_exposed_", "waxed_weathered_", "waxed_oxidized_",
]

COPPER_BLOCK_NAMES = [
    "copper_block", "exposed_copper", "weathered_copper", "oxidized_copper",
    "waxed_copper_block", "waxed_exposed_copper", "waxed_weathered_copper", "waxed_oxidized_copper",
]

FLOAT_PAT = r"-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?[fFdD]?"


def fmt_f32(val: float) -> str:
    if math.isnan(val):
        return "f32::NAN"
    if math.isinf(val):
        return "f32::INFINITY" if val > 0 else "f32::NEG_INFINITY"
    s = repr(float(val))
    if "." not in s and "e" not in s and "E" not in s:
        s += ".0"
    return f"{s}_f32"


def parse_float(s: str) -> float:
    return float(s.rstrip("fFdD"))


def resolve_tag(tag_name: str, seen: set[str] | None = None) -> set[str]:
    if seen is None:
        seen = set()
    bare = tag_name.removeprefix("#").removeprefix("minecraft:")
    if bare in seen:
        return set()
    seen.add(bare)
    tf = BLOCK_TAGS_DIR / f"{bare}.json"
    if not tf.exists():
        return set()
    data = json.loads(tf.read_text(encoding="utf-8"))
    out: set[str] = set()
    for entry in data.get("values", []):
        if isinstance(entry, dict):
            entry = entry.get("id", "")
        if entry.startswith("#"):
            out |= resolve_tag(entry, seen)
        elif entry:
            out.add(entry if ":" in entry else f"minecraft:{entry}")
    return out


def parse_id_maps() -> tuple[dict[str, str], dict[str, list[str]]]:
    """Parse BlockIds.java and BlockItemIds.java into scalar and collection name maps."""
    scalar: dict[str, str] = {}
    collections: dict[str, list[str]] = {}

    for prefix, path in (("BlockIds", BLOCK_IDS_JAVA), ("BlockItemIds", BLOCK_ITEM_IDS_JAVA)):
        txt = path.read_text(encoding="utf-8")
        # Scalar ids: BlockItemId.create("block") or BlockItemId.create("block", "item") or create("block")
        for m in re.finditer(
            r'public static final (?:BlockItemId|ResourceKey<Block>)\s+(\w+)\s*=\s*(?:BlockItemId\.)?create\("([^"]+)"(?:\s*,\s*"[^"]+")?\)',
            txt,
        ):
            scalar[f"{prefix}.{m.group(1)}"] = f"minecraft:{m.group(2)}"
        # Colored collections (supports nested generics like ColorCollection<ResourceKey<Block>>)
        for m in re.finditer(
            r'public static final ColorCollection<.*?>\s+(\w+)\s*=\s*createSimpleColored\("([^"]+)"\)',
            txt,
        ):
            base = m.group(2)
            collections[f"{prefix}.{m.group(1)}"] = [f"minecraft:{c}_{base}" for c in DYE_COLORS]
        # Copper collections
        for m in re.finditer(
            r'public static final WeatheringCopperCollection<.*?>\s+(\w+)\s*=\s*createSimpleCopper\("([^"]+)"\)',
            txt,
        ):
            base = m.group(2)
            collections[f"{prefix}.{m.group(1)}"] = [f"minecraft:{p}{base}" for p in COPPER_PREFIXES]
        if "COPPER_BLOCK" in txt and f"{prefix}.COPPER_BLOCK" not in collections and prefix == "BlockItemIds":
            collections["BlockItemIds.COPPER_BLOCK"] = [f"minecraft:{n}" for n in COPPER_BLOCK_NAMES]

    return scalar, collections


def eval_properties_resistance(expr: str, field_res: dict[str, float]) -> float:
    """Evaluate a BlockBehaviour.Properties expression chain to its exact blast resistance."""
    res = 0.0

    # Base properties constructor / helper
    if re.search(r"\blogProperties\s*\(", expr) or re.search(r"\bnetherStemProperties\s*\(", expr):
        res = 2.0
    elif re.search(r"\bleavesProperties\s*\(", expr):
        res = 0.2
    elif re.search(r"\bshulkerBoxProperties\s*\(", expr):
        res = 2.0
    elif re.search(r"\bpistonProperties\s*\(", expr):
        res = 1.5
    elif re.search(r"\bbuttonProperties\s*\(", expr):
        res = 0.5
    elif re.search(r"\bflowerPotProperties\s*\(", expr):
        res = 0.0
    elif re.search(r"\bcandleProperties\s*\(", expr):
        res = 0.1
    else:
        m_copy = re.search(r"\bof(?:Full|Legacy)Copy\s*\(\s*([A-Z0-9_]+)(?:\.[a-zA-Z0-9_().]+)?\s*\)", expr)
        if m_copy:
            base_field = m_copy.group(1)
            if base_field in field_res:
                res = field_res[base_field]

    # Apply chained modifiers in exact source order
    mod_pat = re.compile(
        rf"\.(strength|explosionResistance|instabreak)\s*\(\s*(?:({FLOAT_PAT})\s*(?:,\s*({FLOAT_PAT})\s*)?)?\)"
    )
    for m in mod_pat.finditer(expr):
        method = m.group(1)
        if method == "instabreak":
            res = 0.0
        elif method == "strength":
            a = parse_float(m.group(2))
            b = parse_float(m.group(3)) if m.group(3) is not None else a
            res = max(0.0, b)
        elif method == "explosionResistance":
            a = parse_float(m.group(2))
            res = max(0.0, a)

    # InfestedBlock constructor always sets explosionResistance(0.75F)
    if re.search(r"\bnew\s+Infested(?:RotatedPillar)?Block\s*\(", expr):
        res = 0.75

    return res


def parse_blast_resistances() -> tuple[dict[str, float], dict[str, str], dict[str, list[str]]]:
    """
    Parse Blocks.java top-to-bottom to extract exact vanilla 26.3 blast resistance
    for every single block. Also returns field_name -> block_id and collection_field -> [block_ids].
    """
    scalar_ids, collection_ids = parse_id_maps()
    txt = BLOCKS_JAVA.read_text(encoding="utf-8")

    block_res: dict[str, float] = {}
    field_res: dict[str, float] = {}
    field_to_block: dict[str, str] = {}
    collection_to_blocks: dict[str, list[str]] = {}

    decl_start_pat = re.compile(
        r"public static final (Block|ColorCollection<Block>|WeatheringCopperCollection<Block>)\s+(\w+)\s*=\s*"
    )

    def extract_rhs(start_idx: int) -> str:
        depth_paren = 0
        depth_brace = 0
        i = start_idx
        n = len(txt)
        while i < n:
            ch = txt[i]
            if ch == "(":
                depth_paren += 1
            elif ch == ")":
                depth_paren -= 1
            elif ch == "{":
                depth_brace += 1
            elif ch == "}":
                depth_brace -= 1
            elif ch == ";" and depth_paren == 0 and depth_brace == 0:
                return txt[start_idx:i]
            i += 1
        return txt[start_idx:]

    for m in decl_start_pat.finditer(txt):
        kind, field_name = m.group(1), m.group(2)
        rhs = extract_rhs(m.end())

        if kind == "Block":
            # Identify the BlockIds / BlockItemIds reference
            m_id = re.search(r"\b(Block(?:Item)?Ids\.\w+)\b", rhs)
            if not m_id:
                continue
            id_key = m_id.group(1)
            block_name = scalar_ids.get(id_key)
            if block_name is None:
                raise RuntimeError(f"Unknown scalar id {id_key} for field {field_name}")

            # Check registerLegacyStair / registerStair / registerSlab / registerWall shortcuts
            m_stair = re.match(r"register(?:Legacy)?Stair\s*\(\s*[^,]+,\s*(\w+)\s*\)", rhs.strip())
            m_wall = re.match(r"registerWall\s*\(\s*[^,]+,\s*(\w+)\s*\)", rhs.strip())
            m_slab = re.match(
                rf"registerSlab\s*\(\s*[^,]+,\s*(\w+)(?:\s*,\s*({FLOAT_PAT})(?:\s*,\s*({FLOAT_PAT}))?)?\s*\)",
                rhs.strip(),
            )
            if m_stair:
                res = field_res[m_stair.group(1)]
            elif m_wall:
                res = field_res[m_wall.group(1)]
            elif m_slab:
                base = m_slab.group(1)
                if m_slab.group(3) is not None:
                    res = max(0.0, parse_float(m_slab.group(3)))
                else:
                    # registerSlab(id, base) or registerSlab(id, base, destroyTime) preserves base explosionResistance
                    res = field_res[base]
            else:
                res = eval_properties_resistance(rhs, field_res)

            field_res[field_name] = res
            block_res[block_name] = res
            field_to_block[field_name] = block_name
        else:
            # ColorCollection<Block> or WeatheringCopperCollection<Block>
            m_id = re.search(r"\b(Block(?:Item)?Ids\.\w+)\b", rhs)
            if not m_id:
                continue
            id_key = m_id.group(1)
            names = collection_ids.get(id_key)
            if names is None:
                raise RuntimeError(f"Unknown collection id {id_key} for field {field_name}")

            # Check zipMap wrappers like registerStair(id, WOOL.pick(color)) or registerSlab(id, CONCRETE.pick(color))
            m_zip_base = re.search(r"register(?:Legacy)?(?:Stair|Slab|Wall)\s*\(\s*\w+\s*,\s*(\w+)\.", rhs)
            if m_zip_base:
                res = field_res[m_zip_base.group(1)]
            else:
                res = eval_properties_resistance(rhs, field_res)

            field_res[field_name] = res
            collection_to_blocks[field_name] = names
            for bn in names:
                block_res[bn] = res

    return block_res, field_to_block, collection_to_blocks


def parse_flammable(
    field_to_block: dict[str, str],
    collection_to_blocks: dict[str, list[str]],
) -> dict[str, tuple[int, int]]:
    """Parse FireBlock.bootStrap() for exact vanilla 26.3 (ignite_odds, burn_odds)."""
    txt = FIRE_JAVA.read_text(encoding="utf-8")
    m_boot = re.search(r"public static void bootStrap\(\)\s*\{(.*)\}\s*\}", txt, re.S)
    if not m_boot:
        raise RuntimeError("Could not locate FireBlock.bootStrap()")
    body = m_boot.group(1)

    odds: dict[str, tuple[int, int]] = {}
    for m in re.finditer(r"fire\.setFlammable\s*\(\s*Blocks\.(\w+)\s*,\s*(\d+)\s*,\s*(\d+)\s*\)", body):
        field, ignite, burn = m.group(1), int(m.group(2)), int(m.group(3))
        bn = field_to_block.get(field)
        if bn is None:
            raise RuntimeError(f"Unknown Blocks.{field} in FireBlock.bootStrap()")
        odds[bn] = (ignite, burn)

    for m in re.finditer(
        r"Blocks\.(\w+)\.forEach\s*\(\s*\w+\s*->\s*fire\.setFlammable\s*\(\s*\w+\s*,\s*(\d+)\s*,\s*(\d+)\s*\)\s*\)",
        body,
    ):
        coll, ignite, burn = m.group(1), int(m.group(2)), int(m.group(3))
        for bn in collection_to_blocks[coll]:
            odds[bn] = (ignite, burn)

    return odds


def format_u8_array(name: str, data: list[int], per_line: int = 32) -> list[str]:
    lines = [f"static {name}: [u8; {len(data)}] = ["]
    for i in range(0, len(data), per_line):
        chunk = ", ".join(str(v) for v in data[i : i + per_line])
        lines.append(f"    {chunk},")
    lines.append("];")
    return lines


def main() -> None:
    blocks = json.loads(BLOCKS_JSON.read_text(encoding="utf-8"))
    total_states = sum(len(b["states"]) for b in blocks.values())

    block_res, field_to_block, collection_to_blocks = parse_blast_resistances()
    flammable_odds = parse_flammable(field_to_block, collection_to_blocks)

    missing_res = [bn for bn in blocks if bn not in block_res]
    if missing_res:
        raise RuntimeError(f"Missing blast resistance for {len(missing_res)} blocks: {missing_res[:20]}")

    # Build lookup structures per block
    default_state: dict[str, int] = {}
    state_lookup: dict[str, dict[tuple[tuple[str, str], ...], int]] = {}
    for bn, bdata in blocks.items():
        lu: dict[tuple[tuple[str, str], ...], int] = {}
        def_id = int(bdata["states"][0]["id"])
        for st in bdata["states"]:
            sid = int(st["id"])
            props = st.get("properties") or {}
            lu[tuple(sorted(props.items()))] = sid
            if st.get("default"):
                def_id = sid
        default_state[bn] = def_id
        state_lookup[bn] = lu

    def get_state(bn: str, props: dict[str, str] | None = None) -> int:
        if not props:
            return default_state[bn]
        return state_lookup[bn][tuple(sorted(props.items()))]

    # Exact vanilla tags for leaves & logs
    leaves_blocks = resolve_tag("leaves")
    logs_blocks = resolve_tag("logs")
    # Also ensure *_wood / *_hyphae are in logs_blocks (vanilla #minecraft:logs already includes all of them!)
    infiniburn_blocks = {"minecraft:netherrack", "minecraft:magma_block"}

    # Build unique blast resistance palette + per-state index
    unique_res = sorted(set(block_res.values()))
    res_to_idx = {val: i for i, val in enumerate(unique_res)}
    if len(unique_res) > 255:
        raise RuntimeError(f"Too many unique blast resistance values ({len(unique_res)}) for u8 palette")

    # Build unique fire (ignite_odds, burn_odds) palette + per-state index
    unique_fire_odds = sorted({(0, 0)} | set(flammable_odds.values()))
    fire_odds_to_idx = {val: i for i, val in enumerate(unique_fire_odds)}

    state_blast_idx = [0] * total_states
    state_fire_odds_idx = [0] * total_states
    state_flags = [0] * total_states

    # Flag bits for state_flags:
    # bit 0: is_leaves
    # bit 1: is_persistent_leaves
    # bit 2: is_log_or_wood
    # bit 3: is_flammable (waterlogged=false and ignite_odds > 0)
    # bit 4: is_tnt
    # bit 5: is_grass_block
    # bit 6: is_fire (fire or soul_fire)
    # bit 7: is_infiniburn
    FLAG_LEAVES = 1 << 0
    FLAG_PERSISTENT_LEAVES = 1 << 1
    FLAG_LOG_OR_WOOD = 1 << 2
    FLAG_FLAMMABLE = 1 << 3
    FLAG_TNT = 1 << 4
    FLAG_GRASS_BLOCK = 1 << 5
    FLAG_FIRE = 1 << 6
    FLAG_INFINIBURN = 1 << 7

    for bn, bdata in blocks.items():
        r_idx = res_to_idx[block_res[bn]]
        base_fodds = flammable_odds.get(bn, (0, 0))
        is_leaf_blk = bn in leaves_blocks
        is_log_blk = bn in logs_blocks
        is_tnt_blk = bn == "minecraft:tnt"
        is_grass_blk = bn == "minecraft:grass_block"
        is_fire_blk = bn in ("minecraft:fire", "minecraft:soul_fire")
        is_inf_blk = bn in infiniburn_blocks

        for st in bdata["states"]:
            sid = int(st["id"])
            props = st.get("properties") or {}
            state_blast_idx[sid] = r_idx

            waterlogged = props.get("waterlogged") == "true"
            fodds = (0, 0) if waterlogged else base_fodds
            state_fire_odds_idx[sid] = fire_odds_to_idx[fodds]

            fl = 0
            if is_leaf_blk:
                fl |= FLAG_LEAVES
                if props.get("persistent") == "true":
                    fl |= FLAG_PERSISTENT_LEAVES
            if is_log_blk:
                fl |= FLAG_LOG_OR_WOOD
            if fodds[0] > 0:
                fl |= FLAG_FLAMMABLE
            if is_tnt_blk:
                fl |= FLAG_TNT
            if is_grass_blk:
                fl |= FLAG_GRASS_BLOCK
            if is_fire_blk:
                fl |= FLAG_FIRE
            if is_inf_blk:
                fl |= FLAG_INFINIBURN
            state_flags[sid] = fl

    # Crop next-age, maturity, and crop kind tables
    crop_next_entries: list[tuple[int, int]] = []
    crop_mature_states: list[int] = []
    crop_kind_entries: list[tuple[int, int]] = []  # 1=farmland, 2=nether_wart, 3=berry/cocoa, 4=bamboo_sapling
    pitcher_upper_entries: list[tuple[int, int]] = []  # lower(age 3|4) -> upper(same age)

    simple_age_crops = [
        ("minecraft:wheat", 7, 1),
        ("minecraft:carrots", 7, 1),
        ("minecraft:potatoes", 7, 1),
        ("minecraft:beetroots", 3, 1),
        ("minecraft:nether_wart", 3, 2),
        ("minecraft:sweet_berry_bush", 3, 3),
    ]
    for bn, max_age, kind_id in simple_age_crops:
        for age in range(max_age):
            cur = get_state(bn, {"age": str(age)})
            nxt = get_state(bn, {"age": str(age + 1)})
            crop_next_entries.append((cur, nxt))
            crop_kind_entries.append((cur, kind_id))
        crop_mature_states.append(get_state(bn, {"age": str(max_age)}))

    # Torchflower crop: age 0 -> age 1 -> minecraft:torchflower
    tf0 = get_state("minecraft:torchflower_crop", {"age": "0"})
    tf1 = get_state("minecraft:torchflower_crop", {"age": "1"})
    tf_full = default_state["minecraft:torchflower"]
    crop_next_entries.append((tf0, tf1))
    crop_next_entries.append((tf1, tf_full))
    crop_kind_entries.append((tf0, 1))
    crop_kind_entries.append((tf1, 1))
    crop_mature_states.append(tf_full)

    # Pitcher crop: lower half grows 0->1->2->3->4; age 3 & 4 also have upper half
    for age in range(4):
        cur = get_state("minecraft:pitcher_crop", {"age": str(age), "half": "lower"})
        nxt = get_state("minecraft:pitcher_crop", {"age": str(age + 1), "half": "lower"})
        crop_next_entries.append((cur, nxt))
        crop_kind_entries.append((cur, 1))
    for age in (3, 4):
        low = get_state("minecraft:pitcher_crop", {"age": str(age), "half": "lower"})
        up = get_state("minecraft:pitcher_crop", {"age": str(age), "half": "upper"})
        pitcher_upper_entries.append((low, up))
    crop_mature_states.append(get_state("minecraft:pitcher_crop", {"age": "4", "half": "lower"}))
    crop_mature_states.append(get_state("minecraft:pitcher_crop", {"age": "4", "half": "upper"}))

    # Cocoa: age 0..2 preserving facing
    for facing in ("north", "south", "west", "east"):
        for age in range(2):
            cur = get_state("minecraft:cocoa", {"age": str(age), "facing": facing})
            nxt = get_state("minecraft:cocoa", {"age": str(age + 1), "facing": facing})
            crop_next_entries.append((cur, nxt))
            crop_kind_entries.append((cur, 3))
        crop_mature_states.append(get_state("minecraft:cocoa", {"age": "2", "facing": facing}))

    # Bamboo sapling -> bamboo
    bs_id = default_state["minecraft:bamboo_sapling"]
    bamboo_id = default_state["minecraft:bamboo"]
    crop_next_entries.append((bs_id, bamboo_id))
    crop_kind_entries.append((bs_id, 4))

    crop_next_entries.sort()
    crop_mature_states.sort()
    crop_kind_entries.sort()
    pitcher_upper_entries.sort()

    # Saplings: 0=oak, 1=spruce, 2=birch, 3=jungle, 4=acacia, 5=dark_oak, 6=cherry, 7=pale_oak, 8=poplar
    sapling_specs = [
        (0, "minecraft:oak_sapling", "minecraft:oak_log", "minecraft:oak_leaves"),
        (1, "minecraft:spruce_sapling", "minecraft:spruce_log", "minecraft:spruce_leaves"),
        (2, "minecraft:birch_sapling", "minecraft:birch_log", "minecraft:birch_leaves"),
        (3, "minecraft:jungle_sapling", "minecraft:jungle_log", "minecraft:jungle_leaves"),
        (4, "minecraft:acacia_sapling", "minecraft:acacia_log", "minecraft:acacia_leaves"),
        (5, "minecraft:dark_oak_sapling", "minecraft:dark_oak_log", "minecraft:dark_oak_leaves"),
        (6, "minecraft:cherry_sapling", "minecraft:cherry_log", "minecraft:cherry_leaves"),
        (7, "minecraft:pale_oak_sapling", "minecraft:pale_oak_log", "minecraft:pale_oak_leaves"),
        (8, "minecraft:poplar_sapling", "minecraft:poplar_log", "minecraft:yellow_poplar_leaves"),
    ]
    sapling_entries: list[tuple[int, int]] = []
    sapling_next_stage_entries: list[tuple[int, int]] = []
    tree_log_states: list[int] = []
    tree_leaves_states: list[int] = []
    for species, sap_bn, log_bn, leaf_bn in sapling_specs:
        s0 = get_state(sap_bn, {"stage": "0"})
        s1 = get_state(sap_bn, {"stage": "1"})
        sapling_entries.append((s0, species))
        sapling_entries.append((s1, species))
        sapling_next_stage_entries.append((s0, s1))
        tree_log_states.append(get_state(log_bn, {"axis": "y"}))
        tree_leaves_states.append(get_state(leaf_bn, {"distance": "1", "persistent": "false", "waterlogged": "false"}))
    sapling_entries.sort()
    sapling_next_stage_entries.sort()

    # Farmland moisture states (0..=7)
    farmland_states = [get_state("minecraft:farmland", {"moisture": str(m)}) for m in range(8)]

    # Sugar cane & cactus age states (0..=15)
    sugar_cane_states = [get_state("minecraft:sugar_cane", {"age": str(a)}) for a in range(16)]
    cactus_states = [get_state("minecraft:cactus", {"age": str(a)}) for a in range(16)]

    # Fire age tables (all 512 fire states -> (age, base_state_index_in_age_group))
    fire_base_states = [
        get_state(
            "minecraft:fire",
            {"age": str(a), "east": "false", "north": "false", "south": "false", "up": "false", "west": "false"},
        )
        for a in range(16)
    ]
    fire_state_info: list[tuple[int, int, int]] = []  # (state_id, age, next_age_state_id or state_id)
    fire_states_by_faces: dict[tuple[str, str, str, str, str], list[int]] = {}
    for st in blocks["minecraft:fire"]["states"]:
        sid = int(st["id"])
        p = st["properties"]
        key = (p["east"], p["north"], p["south"], p["up"], p["west"])
        if key not in fire_states_by_faces:
            fire_states_by_faces[key] = [0] * 16
        fire_states_by_faces[key][int(p["age"])] = sid
    fire_groups: list[list[int]] = []
    fire_state_to_group_age: list[tuple[int, int, int]] = []  # (sid, group_idx, age)
    for key in sorted(fire_states_by_faces):
        g_idx = len(fire_groups)
        arr = fire_states_by_faces[key]
        fire_groups.append(arr)
        for age, sid in enumerate(arr):
            fire_state_to_group_age.append((sid, g_idx, age))
    fire_state_to_group_age.sort()

    # Key constants
    tnt_id = default_state["minecraft:tnt"]
    obsidian_id = default_state["minecraft:obsidian"]
    fire_id = default_state["minecraft:fire"]
    soul_fire_id = default_state["minecraft:soul_fire"]
    portal_x_id = get_state("minecraft:nether_portal", {"axis": "x"})
    portal_z_id = get_state("minecraft:nether_portal", {"axis": "z"})
    grass_id = default_state["minecraft:grass_block"]
    dirt_id = default_state["minecraft:dirt"]
    farmland_dry_id = farmland_states[0]
    sugar_cane_id = sugar_cane_states[0]
    cactus_id = cactus_states[0]

    # Emit Rust 2024 source
    lines: list[str] = [
        "// @generated by scripts/gen_blast_and_crops.py from Minecraft 26.3 Blocks.java, FireBlock.java, block tags & blocks.json.",
        "// Do not edit manually.",
        "",
        f"pub const TNT_STATE_ID: u32 = {tnt_id};",
        f"pub const OBSIDIAN_STATE_ID: u32 = {obsidian_id};",
        f"pub const FIRE_STATE_ID: u32 = {fire_id};",
        f"pub const SOUL_FIRE_STATE_ID: u32 = {soul_fire_id};",
        f"pub const NETHER_PORTAL_X_STATE_ID: u32 = {portal_x_id};",
        f"pub const NETHER_PORTAL_Z_STATE_ID: u32 = {portal_z_id};",
        f"pub const GRASS_BLOCK_STATE_ID: u32 = {grass_id};",
        f"pub const DIRT_STATE_ID: u32 = {dirt_id};",
        f"pub const FARMLAND_DRY_STATE_ID: u32 = {farmland_dry_id};",
        f"pub const SUGAR_CANE_STATE_ID: u32 = {sugar_cane_id};",
        f"pub const CACTUS_STATE_ID: u32 = {cactus_id};",
        "",
        f"pub const FARMLAND_STATES: [u32; 8] = {farmland_states};",
        f"pub const SUGAR_CANE_STATES: [u32; 16] = {sugar_cane_states};",
        f"pub const CACTUS_STATES: [u32; 16] = {cactus_states};",
        f"pub const FIRE_BASE_STATES: [u32; 16] = {fire_base_states};",
        f"pub const TREE_LOG_STATES: [u32; {len(tree_log_states)}] = {tree_log_states};",
        f"pub const TREE_LEAVES_STATES: [u32; {len(tree_leaves_states)}] = {tree_leaves_states};",
        "",
        "const FLAG_LEAVES: u8 = 1 << 0;",
        "const FLAG_PERSISTENT_LEAVES: u8 = 1 << 1;",
        "const FLAG_LOG_OR_WOOD: u8 = 1 << 2;",
        "const FLAG_FLAMMABLE: u8 = 1 << 3;",
        "const FLAG_TNT: u8 = 1 << 4;",
        "const FLAG_GRASS_BLOCK: u8 = 1 << 5;",
        "const FLAG_FIRE: u8 = 1 << 6;",
        "const FLAG_INFINIBURN: u8 = 1 << 7;",
        "",
        f"static BLAST_RESISTANCE_PALETTE: [f32; {len(unique_res)}] = [",
        "    " + ", ".join(fmt_f32(v) for v in unique_res) + ",",
        "];",
        "",
        f"static FIRE_ODDS_PALETTE: [(u8, u8); {len(unique_fire_odds)}] = [",
        "    " + ", ".join(f"({ig}, {bu})" for ig, bu in unique_fire_odds) + ",",
        "];",
        "",
        f"static CROP_NEXT_AGE: [(u32, u32); {len(crop_next_entries)}] = [",
    ]
    for i in range(0, len(crop_next_entries), 8):
        chunk = ", ".join(f"({a}, {b})" for a, b in crop_next_entries[i : i + 8])
        lines.append(f"    {chunk},")
    lines.append("];")
    lines.append("")

    lines.append(f"static CROP_MATURE: [u32; {len(crop_mature_states)}] = [")
    for i in range(0, len(crop_mature_states), 12):
        chunk = ", ".join(str(v) for v in crop_mature_states[i : i + 12])
        lines.append(f"    {chunk},")
    lines.append("];")
    lines.append("")

    lines.append(f"static CROP_KIND: [(u32, u8); {len(crop_kind_entries)}] = [")
    for i in range(0, len(crop_kind_entries), 8):
        chunk = ", ".join(f"({a}, {k})" for a, k in crop_kind_entries[i : i + 8])
        lines.append(f"    {chunk},")
    lines.append("];")
    lines.append("")

    lines.append(f"static PITCHER_UPPER: [(u32, u32); {len(pitcher_upper_entries)}] = [")
    for a, b in pitcher_upper_entries:
        lines.append(f"    ({a}, {b}),")
    lines.append("];")
    lines.append("")

    lines.append(f"static SAPLING_SPECIES: [(u32, u8); {len(sapling_entries)}] = [")
    for i in range(0, len(sapling_entries), 8):
        chunk = ", ".join(f"({sid}, {sp})" for sid, sp in sapling_entries[i : i + 8])
        lines.append(f"    {chunk},")
    lines.append("];")
    lines.append("")

    lines.append(f"static SAPLING_NEXT_STAGE: [(u32, u32); {len(sapling_next_stage_entries)}] = [")
    for i in range(0, len(sapling_next_stage_entries), 6):
        chunk = ", ".join(f"({a}, {b})" for a, b in sapling_next_stage_entries[i : i + 6])
        lines.append(f"    {chunk},")
    lines.append("];")
    lines.append("")

    lines.append(f"static FIRE_AGE_GROUPS: [[u32; 16]; {len(fire_groups)}] = [")
    for arr in fire_groups:
        lines.append(f"    {arr},")
    lines.append("];")
    lines.append("")

    lines.append(f"static FIRE_STATE_LOOKUP: [(u32, u8, u8); {len(fire_state_to_group_age)}] = [")
    for i in range(0, len(fire_state_to_group_age), 8):
        chunk = ", ".join(f"({sid}, {g}, {age})" for sid, g, age in fire_state_to_group_age[i : i + 8])
        lines.append(f"    {chunk},")
    lines.append("];")
    lines.append("")

    lines.extend(format_u8_array("STATE_BLAST_IDX", state_blast_idx))
    lines.append("")
    lines.extend(format_u8_array("STATE_FIRE_ODDS_IDX", state_fire_odds_idx))
    lines.append("")
    lines.extend(format_u8_array("STATE_EXTRA_FLAGS", state_flags))
    lines.append("")

    # Public API functions
    lines.extend([
        "#[inline]",
        "pub fn blast_resistance(state_id: u32) -> f32 {",
        "    let idx = STATE_BLAST_IDX.get(state_id as usize).copied().unwrap_or(0) as usize;",
        "    BLAST_RESISTANCE_PALETTE[idx]",
        "}",
        "",
        "#[inline]",
        "pub fn crop_next_age(state_id: u32) -> Option<u32> {",
        "    CROP_NEXT_AGE",
        "        .binary_search_by_key(&state_id, |(k, _)| *k)",
        "        .ok()",
        "        .map(|i| CROP_NEXT_AGE[i].1)",
        "}",
        "",
        "#[inline]",
        "pub fn crop_is_mature(state_id: u32) -> bool {",
        "    CROP_MATURE.binary_search(&state_id).is_ok()",
        "}",
        "",
        "/// Crop growth category: 1 = farmland crop, 2 = nether wart, 3 = sweet berry/cocoa, 4 = bamboo sapling, 0 = none.",
        "#[inline]",
        "pub fn crop_kind(state_id: u32) -> u8 {",
        "    CROP_KIND",
        "        .binary_search_by_key(&state_id, |(k, _)| *k)",
        "        .ok()",
        "        .map(|i| CROP_KIND[i].1)",
        "        .unwrap_or(0)",
        "}",
        "",
        "#[inline]",
        "pub fn pitcher_crop_upper_for_lower(lower_state_id: u32) -> Option<u32> {",
        "    PITCHER_UPPER",
        "        .binary_search_by_key(&lower_state_id, |(k, _)| *k)",
        "        .ok()",
        "        .map(|i| PITCHER_UPPER[i].1)",
        "}",
        "",
        "/// Returns sapling species id: 0=oak, 1=spruce, 2=birch, 3=jungle, 4=acacia, 5=dark_oak, 6=cherry, 7=pale_oak, 8=poplar.",
        "#[inline]",
        "pub fn is_sapling(state_id: u32) -> Option<u8> {",
        "    SAPLING_SPECIES",
        "        .binary_search_by_key(&state_id, |(k, _)| *k)",
        "        .ok()",
        "        .map(|i| SAPLING_SPECIES[i].1)",
        "}",
        "",
        "/// Advances a stage-0 sapling to stage-1; returns `None` when already stage-1 or not a sapling.",
        "#[inline]",
        "pub fn sapling_next_stage(state_id: u32) -> Option<u32> {",
        "    SAPLING_NEXT_STAGE",
        "        .binary_search_by_key(&state_id, |(k, _)| *k)",
        "        .ok()",
        "        .map(|i| SAPLING_NEXT_STAGE[i].1)",
        "}",
        "",
        "#[inline]",
        "pub fn farmland_moisture(state_id: u32) -> Option<u8> {",
        "    FARMLAND_STATES",
        "        .iter()",
        "        .position(|&s| s == state_id)",
        "        .map(|idx| idx as u8)",
        "}",
        "",
        "#[inline]",
        "pub fn farmland_with_moisture(state_id: u32, moisture: u8) -> Option<u32> {",
        "    if farmland_moisture(state_id).is_some() {",
        "        Some(FARMLAND_STATES[moisture.min(7) as usize])",
        "    } else {",
        "        None",
        "    }",
        "}",
        "",
        "#[inline]",
        "pub fn is_leaves(state_id: u32) -> bool {",
        "    (STATE_EXTRA_FLAGS.get(state_id as usize).copied().unwrap_or(0) & FLAG_LEAVES) != 0",
        "}",
        "",
        "#[inline]",
        "pub fn is_persistent_leaves(state_id: u32) -> bool {",
        "    (STATE_EXTRA_FLAGS.get(state_id as usize).copied().unwrap_or(0) & FLAG_PERSISTENT_LEAVES) != 0",
        "}",
        "",
        "#[inline]",
        "pub fn is_log_or_wood(state_id: u32) -> bool {",
        "    (STATE_EXTRA_FLAGS.get(state_id as usize).copied().unwrap_or(0) & FLAG_LOG_OR_WOOD) != 0",
        "}",
        "",
        "#[inline]",
        "pub fn is_flammable(state_id: u32) -> bool {",
        "    (STATE_EXTRA_FLAGS.get(state_id as usize).copied().unwrap_or(0) & FLAG_FLAMMABLE) != 0",
        "}",
        "",
        "/// Returns vanilla `(ignite_odds, burn_odds)` from `FireBlock.bootStrap()` (0, 0 when waterlogged or non-flammable).",
        "#[inline]",
        "pub fn fire_odds(state_id: u32) -> (u8, u8) {",
        "    let idx = STATE_FIRE_ODDS_IDX.get(state_id as usize).copied().unwrap_or(0) as usize;",
        "    FIRE_ODDS_PALETTE[idx]",
        "}",
        "",
        "#[inline]",
        "pub fn is_tnt(state_id: u32) -> bool {",
        "    (STATE_EXTRA_FLAGS.get(state_id as usize).copied().unwrap_or(0) & FLAG_TNT) != 0",
        "}",
        "",
        "#[inline]",
        "pub fn is_grass_block(state_id: u32) -> bool {",
        "    (STATE_EXTRA_FLAGS.get(state_id as usize).copied().unwrap_or(0) & FLAG_GRASS_BLOCK) != 0",
        "}",
        "",
        "#[inline]",
        "pub fn is_fire(state_id: u32) -> bool {",
        "    (STATE_EXTRA_FLAGS.get(state_id as usize).copied().unwrap_or(0) & FLAG_FIRE) != 0",
        "}",
        "",
        "#[inline]",
        "pub fn is_infiniburn(state_id: u32) -> bool {",
        "    (STATE_EXTRA_FLAGS.get(state_id as usize).copied().unwrap_or(0) & FLAG_INFINIBURN) != 0",
        "}",
        "",
        "#[inline]",
        "pub fn is_nether_portal(state_id: u32) -> bool {",
        "    state_id == NETHER_PORTAL_X_STATE_ID || state_id == NETHER_PORTAL_Z_STATE_ID",
        "}",
        "",
        "#[inline]",
        "pub fn fire_age(state_id: u32) -> Option<u8> {",
        "    FIRE_STATE_LOOKUP",
        "        .binary_search_by_key(&state_id, |(k, _, _)| *k)",
        "        .ok()",
        "        .map(|i| FIRE_STATE_LOOKUP[i].2)",
        "}",
        "",
        "#[inline]",
        "pub fn fire_with_age(state_id: u32, age: u8) -> Option<u32> {",
        "    FIRE_STATE_LOOKUP",
        "        .binary_search_by_key(&state_id, |(k, _, _)| *k)",
        "        .ok()",
        "        .map(|i| FIRE_AGE_GROUPS[FIRE_STATE_LOOKUP[i].1 as usize][age.min(15) as usize])",
        "}",
        "",
        "#[inline]",
        "pub fn sugar_cane_age(state_id: u32) -> Option<u8> {",
        "    SUGAR_CANE_STATES",
        "        .iter()",
        "        .position(|&s| s == state_id)",
        "        .map(|idx| idx as u8)",
        "}",
        "",
        "#[inline]",
        "pub fn sugar_cane_with_age(state_id: u32, age: u8) -> Option<u32> {",
        "    if sugar_cane_age(state_id).is_some() {",
        "        Some(SUGAR_CANE_STATES[age.min(15) as usize])",
        "    } else {",
        "        None",
        "    }",
        "}",
        "",
        "#[inline]",
        "pub fn cactus_age(state_id: u32) -> Option<u8> {",
        "    CACTUS_STATES",
        "        .iter()",
        "        .position(|&s| s == state_id)",
        "        .map(|idx| idx as u8)",
        "}",
        "",
        "#[inline]",
        "pub fn cactus_with_age(state_id: u32, age: u8) -> Option<u32> {",
        "    if cactus_age(state_id).is_some() {",
        "        Some(CACTUS_STATES[age.min(15) as usize])",
        "    } else {",
        "        None",
        "    }",
        "}",
        "",
    ])

    OUT_PATH.write_text("\n".join(lines), encoding="utf-8")
    print(f"Generated {OUT_PATH}: {total_states} states, {len(unique_res)} unique blast resistances, "
          f"{len(flammable_odds)} flammable blocks, {len(crop_next_entries)} crop transitions.")
    # Sanity-check representative vanilla blast resistance values
    samples = [
        "minecraft:air", "minecraft:bedrock", "minecraft:barrier", "minecraft:command_block",
        "minecraft:end_portal", "minecraft:obsidian", "minecraft:crying_obsidian",
        "minecraft:respawn_anchor", "minecraft:netherite_block", "minecraft:ancient_debris",
        "minecraft:water", "minecraft:lava", "minecraft:stone", "minecraft:cobblestone",
        "minecraft:bricks", "minecraft:deepslate", "minecraft:iron_block", "minecraft:diamond_block",
        "minecraft:oak_planks", "minecraft:oak_log", "minecraft:dirt", "minecraft:sand",
        "minecraft:gravel", "minecraft:glass", "minecraft:oak_leaves", "minecraft:tnt",
        "minecraft:infested_stone", "minecraft:oak_stairs", "minecraft:stone_slab",
        "minecraft:white_wool", "minecraft:oxidized_copper", "minecraft:heavy_core",
    ]
    for s in samples:
        print(f"  {s:32s} -> {block_res[s]}")


if __name__ == "__main__":
    main()
