#!/usr/bin/env python3
"""
Generates O(1) exact Minecraft 26.3 block-state property, redstone/interactive state transition,
and placement orientation tables from `generated/reports/blocks.json`
-> `rust/crates/mc-data/src/generated/block_behaviors.rs`.
"""

import json
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
BLOCKS_JSON = REPO_ROOT / "generated" / "reports" / "blocks.json"
OUT_PATH = REPO_ROOT / "rust" / "crates" / "mc-data" / "src" / "generated" / "block_behaviors.rs"

FACING_MAP = {"down": 0, "up": 1, "north": 2, "south": 3, "west": 4, "east": 5}
HORIZ_NAMES = ("north", "south", "west", "east")
ALL_DIRS = ("down", "up", "north", "south", "west", "east")

FALLING_BLOCKS = {
    "minecraft:sand",
    "minecraft:red_sand",
    "minecraft:gravel",
    "minecraft:suspicious_sand",
    "minecraft:suspicious_gravel",
    "minecraft:anvil",
    "minecraft:chipped_anvil",
    "minecraft:damaged_anvil",
    "minecraft:dragon_egg",
    "minecraft:pointed_dripstone",
}

# Blocks whose `facing` points AWAY from player (matching player's look direction) rather than toward player.
FACE_ALONG_PLAYER = {
    "minecraft:repeater",
    "minecraft:comparator",
}


def classify_kind(block_name: str, props: dict[str, str]) -> int:
    if block_name == "minecraft:redstone_wire":
        return 1
    if block_name in ("minecraft:redstone_torch", "minecraft:redstone_wall_torch"):
        return 2
    if block_name == "minecraft:redstone_lamp":
        return 3
    if block_name == "minecraft:lever":
        return 4
    if block_name.endswith("_button"):
        return 5
    if block_name == "minecraft:repeater":
        return 6
    if block_name.endswith("_door") and "half" in props:
        return 7
    if block_name.endswith("_trapdoor"):
        return 8
    if block_name.endswith("_fence_gate"):
        return 9
    if block_name.endswith("_pressure_plate"):
        return 10
    if block_name == "minecraft:redstone_block":
        return 11
    if block_name in FALLING_BLOCKS or block_name.endswith("_concrete_powder"):
        return 12
    if block_name in ("minecraft:chest", "minecraft:trapped_chest"):
        return 13
    if block_name == "minecraft:barrel":
        return 14
    if block_name == "minecraft:furnace":
        return 15
    if block_name == "minecraft:blast_furnace":
        return 16
    if block_name == "minecraft:smoker":
        return 17
    if block_name == "minecraft:crafting_table":
        return 18
    if block_name == "minecraft:water":
        return 19
    if block_name == "minecraft:lava":
        return 20
    if block_name == "minecraft:piston":
        return 21
    if block_name == "minecraft:sticky_piston":
        return 22
    if block_name == "minecraft:piston_head":
        return 23
    if block_name == "minecraft:hopper":
        return 24
    if block_name == "minecraft:comparator":
        return 25
    if block_name == "minecraft:observer":
        return 26
    if block_name == "minecraft:dispenser":
        return 27
    if block_name == "minecraft:dropper":
        return 28
    if block_name == "minecraft:nether_portal":
        return 29
    if block_name == "minecraft:end_portal":
        return 30
    return 0


def main() -> None:
    blocks = json.loads(BLOCKS_JSON.read_text(encoding="utf-8"))
    total_states = sum(len(b["states"]) for b in blocks.values())

    records: list[tuple[int, int, int, int, int, int]] = [
        (0, 0, 255, 0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF_FFFF)
    ] * total_states
    wire_variants: list[list[int]] = []
    wire_state_to_variant: list[tuple[int, int]] = []
    water_states = [0] * 16
    lava_states = [0] * 16
    piston_states = [[0, 0] for _ in range(6)]
    sticky_piston_states = [[0, 0] for _ in range(6)]
    piston_head_states = [[0, 0] for _ in range(6)]
    hopper_facing_states = [0] * 6

    repeater_delay_cycle: list[tuple[int, int]] = []
    comparator_mode_toggle: list[tuple[int, int]] = []

    # Placement orientation tables keyed by default state id
    axis_variants: list[tuple[int, list[int]]] = []
    slab_variants: list[tuple[int, list[int]]] = []  # default_id -> [bottom, top, double]
    slab_merge: list[tuple[int, int]] = []  # (bottom_or_top_id) -> double_id
    facing6_variants: list[tuple[int, list[int], bool]] = []  # default_id -> ([D,U,N,S,W,E], face_player)
    facing4_variants: list[tuple[int, list[int], bool]] = []  # default_id -> ([N,S,W,E], face_player)
    stair_variants: list[tuple[int, list[int], list[int]]] = []  # default_id -> ([bot N,S,W,E], [top N,S,W,E])
    trapdoor_variants: list[tuple[int, list[int], list[int]]] = []  # default_id -> ([bot N,S,W,E], [top N,S,W,E])
    door_variants: list[tuple[int, list[int], list[int]]] = []  # default_id -> ([lower N,S,W,E], [upper N,S,W,E])
    bed_variants: list[tuple[int, list[int], list[int]]] = []  # default_id -> ([foot N,S,W,E], [head N,S,W,E])
    wall_torch_map: list[tuple[int, list[int]]] = []  # floor_default_id -> [wall N,S,W,E]

    default_by_name: dict[str, int] = {}
    lookup_by_name: dict[str, dict[tuple[tuple[str, str], ...], int]] = {}
    def_props_by_name: dict[str, dict[str, str]] = {}

    for block_name, bdata in blocks.items():
        lookup: dict[tuple[tuple[str, str], ...], int] = {}
        def_id = int(bdata["states"][0]["id"])
        def_props: dict[str, str] = bdata["states"][0].get("properties") or {}
        for st in bdata["states"]:
            props = st.get("properties") or {}
            sid = int(st["id"])
            lookup[tuple(sorted(props.items()))] = sid
            if st.get("default"):
                def_id = sid
                def_props = props
        default_by_name[block_name] = def_id
        lookup_by_name[block_name] = lookup
        def_props_by_name[block_name] = def_props

    for block_name, bdata in blocks.items():
        lookup = lookup_by_name[block_name]
        def_id = default_by_name[block_name]
        def_props = def_props_by_name[block_name]
        prop_defs = bdata.get("properties") or {}

        def find_with(props: dict[str, str], overrides: dict[str, str]) -> int:
            np = dict(props)
            for k, v in overrides.items():
                if k not in np:
                    return 0xFFFF_FFFF
                np[k] = v
            return lookup.get(tuple(sorted(np.items())), 0xFFFF_FFFF)

        if block_name == "minecraft:redstone_wire":
            groups: dict[tuple, list[int]] = {}
            for st in bdata["states"]:
                props = st.get("properties") or {}
                sid = int(st["id"])
                pw = int(props["power"])
                base = tuple(sorted((k, v) for k, v in props.items() if k != "power"))
                if base not in groups:
                    groups[base] = [sid] * 16
                groups[base][pw] = sid
            for _base, arr in sorted(groups.items()):
                vidx = len(wire_variants)
                wire_variants.append(arr)
                for sid in arr:
                    wire_state_to_variant.append((sid, vidx))

        # Populate placement orientation tables from default properties
        if block_name == "minecraft:hopper":
            for i, dname in enumerate(ALL_DIRS):
                if dname == "up":
                    hopper_facing_states[i] = find_with(def_props, {"facing": "down"})
                else:
                    hopper_facing_states[i] = find_with(def_props, {"facing": dname})
        elif block_name.endswith("_slab") and set(prop_defs.get("type", [])) >= {"bottom", "top", "double"}:
            bot = find_with(def_props, {"type": "bottom"})
            top = find_with(def_props, {"type": "top"})
            dbl = find_with(def_props, {"type": "double"})
            if bot != 0xFFFF_FFFF and top != 0xFFFF_FFFF and dbl != 0xFFFF_FFFF:
                slab_variants.append((def_id, [bot, top, dbl]))
                # Also map waterlogged=false/true bottom/top states -> double
                for st in bdata["states"]:
                    sp = st.get("properties") or {}
                    if sp.get("type") in ("bottom", "top"):
                        d_id = find_with(sp, {"type": "double"})
                        if d_id != 0xFFFF_FFFF:
                            slab_merge.append((int(st["id"]), d_id))
        elif block_name.endswith("_stairs") and "facing" in prop_defs and "half" in prop_defs:
            bot = [find_with(def_props, {"facing": d, "half": "bottom"}) for d in HORIZ_NAMES]
            top = [find_with(def_props, {"facing": d, "half": "top"}) for d in HORIZ_NAMES]
            if all(x != 0xFFFF_FFFF for x in bot + top):
                stair_variants.append((def_id, bot, top))
        elif block_name.endswith("_trapdoor") and "facing" in prop_defs and "half" in prop_defs:
            bot = [find_with(def_props, {"facing": d, "half": "bottom"}) for d in HORIZ_NAMES]
            top = [find_with(def_props, {"facing": d, "half": "top"}) for d in HORIZ_NAMES]
            if all(x != 0xFFFF_FFFF for x in bot + top):
                trapdoor_variants.append((def_id, bot, top))
        elif block_name.endswith("_door") and set(prop_defs.get("half", [])) >= {"lower", "upper"} and "facing" in prop_defs:
            lo = [find_with(def_props, {"facing": d, "half": "lower"}) for d in HORIZ_NAMES]
            hi = [find_with(def_props, {"facing": d, "half": "upper"}) for d in HORIZ_NAMES]
            if all(x != 0xFFFF_FFFF for x in lo + hi):
                door_variants.append((def_id, lo, hi))
        elif block_name.endswith("_bed") and set(prop_defs.get("part", [])) >= {"foot", "head"} and "facing" in prop_defs:
            ft = [find_with(def_props, {"facing": d, "part": "foot"}) for d in HORIZ_NAMES]
            hd = [find_with(def_props, {"facing": d, "part": "head"}) for d in HORIZ_NAMES]
            if all(x != 0xFFFF_FFFF for x in ft + hd):
                bed_variants.append((def_id, ft, hd))
        elif set(prop_defs.get("axis", [])) == {"x", "y", "z"}:
            arr = [find_with(def_props, {"axis": a}) for a in ("x", "y", "z")]
            if all(x != 0xFFFF_FFFF for x in arr):
                axis_variants.append((def_id, arr))
        elif set(prop_defs.get("facing", [])) == set(ALL_DIRS):
            arr = [find_with(def_props, {"facing": d}) for d in ALL_DIRS]
            if all(x != 0xFFFF_FFFF for x in arr):
                # Pistons, observers, dispensers, droppers, barrels face toward the player (opposite look)
                face_player = block_name != "minecraft:observer"
                # Note: in vanilla, observers look AWAY from player's facing? Actually Observer's face points AT the
                # block you click (or toward the player = opposite of player's look direction, output at back!).
                # Wait: in vanilla ObserverBlock.getStateForPlacement returns FACING = context.getNearestLookingDirection()
                # (the red dot output points toward the player, face points away!).
                facing6_variants.append((def_id, arr, face_player))
        elif set(prop_defs.get("facing", [])) == set(HORIZ_NAMES) and "wall_" not in block_name:
            arr = [find_with(def_props, {"facing": d}) for d in HORIZ_NAMES]
            if all(x != 0xFFFF_FFFF for x in arr):
                # Repeaters/comparators/fence_gates align with player facing; chests/furnaces/anvils face player
                face_player = (
                    block_name not in FACE_ALONG_PLAYER
                    and not block_name.endswith("_fence_gate")
                )
                facing4_variants.append((def_id, arr, face_player))

        # Per-state behavior records
        for st in bdata["states"]:
            sid = int(st["id"])
            props = st.get("properties") or {}
            kind = classify_kind(block_name, props)

            meta = 0
            if "power" in props:
                meta |= int(props["power"]) & 0x0F
            elif "delay" in props:
                meta |= int(props["delay"]) & 0x0F
            elif "level" in props and block_name in ("minecraft:water", "minecraft:lava"):
                lv = int(props["level"]) & 0x0F
                meta |= lv
                if block_name == "minecraft:water":
                    water_states[lv] = sid
                else:
                    lava_states[lv] = sid
            if props.get("lit") == "true":
                meta |= 1 << 4
            if props.get("powered") == "true" or props.get("enabled") == "true" or props.get("triggered") == "true":
                meta |= 1 << 5
            if props.get("open") == "true" or props.get("extended") == "true":
                meta |= 1 << 6
            if (
                props.get("half") == "upper"
                or block_name == "minecraft:redstone_wall_torch"
                or props.get("mode") == "subtract"
            ):
                meta |= 1 << 7

            facing = FACING_MAP.get(props.get("facing", ""), 255)
            if block_name == "minecraft:piston" and facing < 6:
                piston_states[facing][1 if props.get("extended") == "true" else 0] = sid
            elif block_name == "minecraft:sticky_piston" and facing < 6:
                sticky_piston_states[facing][1 if props.get("extended") == "true" else 0] = sid
            elif block_name == "minecraft:piston_head" and facing < 6 and props.get("short") == "false":
                piston_head_states[facing][1 if props.get("type") == "sticky" else 0] = sid

            if block_name == "minecraft:repeater" and "delay" in props:
                cur_d = int(props["delay"])
                nxt_d = (cur_d % 4) + 1
                nxt_id = find_with(props, {"delay": str(nxt_d)})
                if nxt_id != 0xFFFF_FFFF:
                    repeater_delay_cycle.append((sid, nxt_id))
            elif block_name == "minecraft:comparator" and "mode" in props:
                nxt_m = "subtract" if props["mode"] == "compare" else "compare"
                nxt_id = find_with(props, {"mode": nxt_m})
                if nxt_id != 0xFFFF_FFFF:
                    comparator_mode_toggle.append((sid, nxt_id))

            t_lit = find_with(props, {"lit": "false" if props.get("lit") == "true" else "true"})
            if "powered" in props:
                t_pow = find_with(props, {"powered": "false" if props.get("powered") == "true" else "true"})
            elif "triggered" in props:
                t_pow = find_with(props, {"triggered": "false" if props.get("triggered") == "true" else "true"})
            else:
                t_pow = 0xFFFF_FFFF
            if "extended" in props:
                t_open = find_with(props, {"extended": "false" if props.get("extended") == "true" else "true"})
            else:
                t_open = find_with(props, {"open": "false" if props.get("open") == "true" else "true"})
            records[sid] = (kind, meta, facing, t_lit, t_pow, t_open)

    # Wall torch mappings (floor torch default -> wall torch [N, S, W, E])
    for floor_name, wall_name in (
        ("minecraft:torch", "minecraft:wall_torch"),
        ("minecraft:soul_torch", "minecraft:soul_wall_torch"),
        ("minecraft:redstone_torch", "minecraft:redstone_wall_torch"),
        ("minecraft:copper_torch", "minecraft:copper_wall_torch"),
    ):
        if floor_name in default_by_name and wall_name in lookup_by_name:
            wlookup = lookup_by_name[wall_name]
            wdef = def_props_by_name[wall_name]
            arr = []
            for d in HORIZ_NAMES:
                np = dict(wdef)
                np["facing"] = d
                arr.append(wlookup.get(tuple(sorted(np.items())), 0xFFFF_FFFF))
            if all(x != 0xFFFF_FFFF for x in arr):
                wall_torch_map.append((default_by_name[floor_name], arr))

    wire_state_to_variant.sort()
    repeater_delay_cycle.sort()
    comparator_mode_toggle.sort()
    axis_variants.sort()
    slab_variants.sort()
    slab_merge.sort()
    facing6_variants.sort()
    facing4_variants.sort()
    stair_variants.sort()
    trapdoor_variants.sort()
    door_variants.sort()
    bed_variants.sort()
    wall_torch_map.sort()

    lines = [
        "// @generated by scripts/gen_block_behaviors.py from generated/reports/blocks.json.",
        "// Do not edit manually.",
        "",
        "pub const KIND_NORMAL: u8 = 0;",
        "pub const KIND_REDSTONE_WIRE: u8 = 1;",
        "pub const KIND_REDSTONE_TORCH: u8 = 2;",
        "pub const KIND_REDSTONE_LAMP: u8 = 3;",
        "pub const KIND_LEVER: u8 = 4;",
        "pub const KIND_BUTTON: u8 = 5;",
        "pub const KIND_REPEATER: u8 = 6;",
        "pub const KIND_DOOR: u8 = 7;",
        "pub const KIND_TRAPDOOR: u8 = 8;",
        "pub const KIND_FENCE_GATE: u8 = 9;",
        "pub const KIND_PRESSURE_PLATE: u8 = 10;",
        "pub const KIND_REDSTONE_BLOCK: u8 = 11;",
        "pub const KIND_FALLING_BLOCK: u8 = 12;",
        "pub const KIND_CHEST: u8 = 13;",
        "pub const KIND_BARREL: u8 = 14;",
        "pub const KIND_FURNACE: u8 = 15;",
        "pub const KIND_BLAST_FURNACE: u8 = 16;",
        "pub const KIND_SMOKER: u8 = 17;",
        "pub const KIND_CRAFTING_TABLE: u8 = 18;",
        "pub const KIND_WATER: u8 = 19;",
        "pub const KIND_LAVA: u8 = 20;",
        "pub const KIND_PISTON: u8 = 21;",
        "pub const KIND_STICKY_PISTON: u8 = 22;",
        "pub const KIND_PISTON_HEAD: u8 = 23;",
        "pub const KIND_HOPPER: u8 = 24;",
        "pub const KIND_COMPARATOR: u8 = 25;",
        "pub const KIND_OBSERVER: u8 = 26;",
        "pub const KIND_DISPENSER: u8 = 27;",
        "pub const KIND_DROPPER: u8 = 28;",
        "pub const KIND_NETHER_PORTAL: u8 = 29;",
        "pub const KIND_END_PORTAL: u8 = 30;",
        "",
        f"pub const WATER_STATES: [u32; 16] = [{', '.join(str(x) for x in water_states)}];",
        f"pub const LAVA_STATES: [u32; 16] = [{', '.join(str(x) for x in lava_states)}];",
        f"pub const PISTON_STATES: [[u32; 2]; 6] = [{', '.join(f'[{a}, {b}]' for a, b in piston_states)}];",
        f"pub const STICKY_PISTON_STATES: [[u32; 2]; 6] = [{', '.join(f'[{a}, {b}]' for a, b in sticky_piston_states)}];",
        f"pub const PISTON_HEAD_STATES: [[u32; 2]; 6] = [{', '.join(f'[{a}, {b}]' for a, b in piston_head_states)}];",
        f"pub const HOPPER_FACING_STATES: [u32; 6] = [{', '.join(str(x) for x in hopper_facing_states)}];",
        f"pub const HOPPER_DEFAULT_STATE: u32 = {default_by_name.get('minecraft:hopper', 0)};",
        "",
        "#[derive(Debug, Clone, Copy)]",
        "pub struct BlockBehavior {",
        "    pub kind: u8,",
        "    pub meta: u8,",
        "    pub facing: u8,",
        "    pub toggle_lit: u32,",
        "    pub toggle_powered: u32,",
        "    pub toggle_open: u32,",
        "}",
        "",
        "impl BlockBehavior {",
        "    #[inline] pub const fn power(self) -> u8 { self.meta & 0x0F }",
        "    #[inline] pub const fn delay(self) -> u8 { self.meta & 0x0F }",
        "    #[inline] pub const fn is_lit(self) -> bool { (self.meta & (1 << 4)) != 0 }",
        "    #[inline] pub const fn is_powered(self) -> bool { (self.meta & (1 << 5)) != 0 }",
        "    #[inline] pub const fn is_open(self) -> bool { (self.meta & (1 << 6)) != 0 }",
        "    #[inline] pub const fn is_upper_or_wall(self) -> bool { (self.meta & (1 << 7)) != 0 }",
        "    #[inline] pub const fn is_subtract_mode(self) -> bool { (self.meta & (1 << 7)) != 0 }",
        "    #[inline] pub const fn toggled_lit(self) -> Option<u32> { if self.toggle_lit == u32::MAX { None } else { Some(self.toggle_lit) } }",
        "    #[inline] pub const fn toggled_powered(self) -> Option<u32> { if self.toggle_powered == u32::MAX { None } else { Some(self.toggle_powered) } }",
        "    #[inline] pub const fn toggled_open(self) -> Option<u32> { if self.toggle_open == u32::MAX { None } else { Some(self.toggle_open) } }",
        "}",
        "",
        f"pub static BLOCK_BEHAVIORS: [BlockBehavior; {len(records)}] = [",
    ]
    for kind, meta, facing, t_lit, t_pow, t_open in records:
        sl = "u32::MAX" if t_lit == 0xFFFF_FFFF else str(t_lit)
        sp = "u32::MAX" if t_pow == 0xFFFF_FFFF else str(t_pow)
        so = "u32::MAX" if t_open == 0xFFFF_FFFF else str(t_open)
        lines.append(
            f"    BlockBehavior {{ kind: {kind}, meta: {meta}, facing: {facing}, toggle_lit: {sl}, toggle_powered: {sp}, toggle_open: {so} }},"
        )
    lines.append("];")
    lines.append("")

    lines.append(f"static WIRE_POWER_VARIANTS: [[u32; 16]; {len(wire_variants)}] = [")
    for arr in wire_variants:
        lines.append("    [" + ", ".join(str(x) for x in arr) + "],")
    lines.append("];")
    lines.append("")
    lines.append(f"static WIRE_STATE_TO_VARIANT: [(u32, u16); {len(wire_state_to_variant)}] = [")
    for sid, vidx in wire_state_to_variant:
        lines.append(f"    ({sid}, {vidx}),")
    lines.append("];")
    lines.append("")

    lines.append(f"static REPEATER_DELAY_CYCLE: [(u32, u32); {len(repeater_delay_cycle)}] = [")
    for a, b in repeater_delay_cycle:
        lines.append(f"    ({a}, {b}),")
    lines.append("];")
    lines.append("")

    lines.append(f"static COMPARATOR_MODE_TOGGLE: [(u32, u32); {len(comparator_mode_toggle)}] = [")
    for a, b in comparator_mode_toggle:
        lines.append(f"    ({a}, {b}),")
    lines.append("];")
    lines.append("")

    lines.append(f"static AXIS_VARIANTS: [(u32, [u32; 3]); {len(axis_variants)}] = [")
    for d, arr in axis_variants:
        lines.append(f"    ({d}, [{arr[0]}, {arr[1]}, {arr[2]}]),")
    lines.append("];")
    lines.append("")

    lines.append(f"static SLAB_VARIANTS: [(u32, [u32; 3]); {len(slab_variants)}] = [")
    for d, arr in slab_variants:
        lines.append(f"    ({d}, [{arr[0]}, {arr[1]}, {arr[2]}]),")
    lines.append("];")
    lines.append("")

    lines.append(f"static SLAB_MERGE_DOUBLE: [(u32, u32); {len(slab_merge)}] = [")
    for s, dbl in slab_merge:
        lines.append(f"    ({s}, {dbl}),")
    lines.append("];")
    lines.append("")

    lines.append(f"static FACING6_VARIANTS: [(u32, [u32; 6], bool); {len(facing6_variants)}] = [")
    for d, arr, fp in facing6_variants:
        fp_s = "true" if fp else "false"
        lines.append(f"    ({d}, [{', '.join(str(x) for x in arr)}], {fp_s}),")
    lines.append("];")
    lines.append("")

    lines.append(f"static FACING4_VARIANTS: [(u32, [u32; 4], bool); {len(facing4_variants)}] = [")
    for d, arr, fp in facing4_variants:
        fp_s = "true" if fp else "false"
        lines.append(f"    ({d}, [{', '.join(str(x) for x in arr)}], {fp_s}),")
    lines.append("];")
    lines.append("")

    lines.append(f"static STAIR_VARIANTS: [(u32, [[u32; 4]; 2]); {len(stair_variants)}] = [")
    for d, bot, top in stair_variants:
        lines.append(f"    ({d}, [[{', '.join(str(x) for x in bot)}], [{', '.join(str(x) for x in top)}]]),")
    lines.append("];")
    lines.append("")

    lines.append(f"static TRAPDOOR_VARIANTS: [(u32, [[u32; 4]; 2]); {len(trapdoor_variants)}] = [")
    for d, bot, top in trapdoor_variants:
        lines.append(f"    ({d}, [[{', '.join(str(x) for x in bot)}], [{', '.join(str(x) for x in top)}]]),")
    lines.append("];")
    lines.append("")

    lines.append(f"static DOOR_VARIANTS: [(u32, [[u32; 4]; 2]); {len(door_variants)}] = [")
    for d, lo, hi in door_variants:
        lines.append(f"    ({d}, [[{', '.join(str(x) for x in lo)}], [{', '.join(str(x) for x in hi)}]]),")
    lines.append("];")
    lines.append("")

    lines.append(f"static BED_VARIANTS: [(u32, [[u32; 4]; 2]); {len(bed_variants)}] = [")
    for d, ft, hd in bed_variants:
        lines.append(f"    ({d}, [[{', '.join(str(x) for x in ft)}], [{', '.join(str(x) for x in hd)}]]),")
    lines.append("];")
    lines.append("")

    lines.append(f"static WALL_TORCH_MAP: [(u32, [u32; 4]); {len(wall_torch_map)}] = [")
    for d, arr in wall_torch_map:
        lines.append(f"    ({d}, [{', '.join(str(x) for x in arr)}]),")
    lines.append("];")
    lines.append("")

    lines.extend([
        "#[inline]",
        "pub fn block_behavior(state_id: u32) -> BlockBehavior {",
        "    BLOCK_BEHAVIORS.get(state_id as usize).copied().unwrap_or(BLOCK_BEHAVIORS[0])",
        "}",
        "",
        "/// Return the exact `redstone_wire` BlockStateId with the same connections and `power = new_power.min(15)`.",
        "pub fn redstone_wire_with_power(state_id: u32, new_power: u8) -> Option<u32> {",
        "    let idx = WIRE_STATE_TO_VARIANT.binary_search_by_key(&state_id, |(s, _)| *s).ok()?;",
        "    let vidx = WIRE_STATE_TO_VARIANT[idx].1 as usize;",
        "    Some(WIRE_POWER_VARIANTS[vidx][(new_power.min(15)) as usize])",
        "}",
        "",
        "/// Cycle a redstone repeater's delay (`1 -> 2 -> 3 -> 4 -> 1`).",
        "pub fn cycle_repeater_delay(state_id: u32) -> Option<u32> {",
        "    let idx = REPEATER_DELAY_CYCLE.binary_search_by_key(&state_id, |(s, _)| *s).ok()?;",
        "    Some(REPEATER_DELAY_CYCLE[idx].1)",
        "}",
        "",
        "/// Toggle a redstone comparator's mode (`compare <-> subtract`).",
        "pub fn toggle_comparator_mode(state_id: u32) -> Option<u32> {",
        "    let idx = COMPARATOR_MODE_TOGGLE.binary_search_by_key(&state_id, |(s, _)| *s).ok()?;",
        "    Some(COMPARATOR_MODE_TOGGLE[idx].1)",
        "}",
        "",
        "/// Result of resolving vanilla block placement orientation & multi-block secondary placement.",
        "#[derive(Debug, Clone, Copy)]",
        "pub struct PlacementOutcome {",
        "    /// Exact BlockStateId to place at the primary target position.",
        "    pub primary_state: u32,",
        "    /// If true, replaces the clicked block directly (e.g. merging a half-slab into a double slab).",
        "    pub replace_clicked: bool,",
        "    /// Optional secondary block `(dx, dy, dz, state_id)` for 2-block structures (doors, beds).",
        "    pub secondary: Option<(i32, i32, i32, u32)>,",
        "}",
        "",
        "/// Horizontal facing index: 0=North(-Z), 1=South(+Z), 2=West(-X), 3=East(+X).",
        "#[inline]",
        "fn horizontal_from_yaw(yaw: f32) -> usize {",
        "    // Minecraft yaw: 0=South(+Z), 90=West(-X), 180=North(-Z), 270=East(+X)",
        "    let q = ((yaw * 4.0 / 360.0 + 0.5).floor() as i32).rem_euclid(4);",
        "    match q {",
        "        0 => 1, // South",
        "        1 => 2, // West",
        "        2 => 0, // North",
        "        _ => 3, // East",
        "    }",
        "}",
        "",
        "#[inline]",
        "const fn opposite_horiz(h: usize) -> usize {",
        "    match h {",
        "        0 => 1,",
        "        1 => 0,",
        "        2 => 3,",
        "        _ => 2,",
        "    }",
        "}",
        "",
        "#[inline]",
        "const fn horiz_offset(h: usize) -> (i32, i32, i32) {",
        "    match h {",
        "        0 => (0, 0, -1),",
        "        1 => (0, 0, 1),",
        "        2 => (-1, 0, 0),",
        "        _ => (1, 0, 0),",
        "    }",
        "}",
        "",
        "/// Resolve exact Minecraft 26.3 block placement orientation from hit face, click Y, player yaw/pitch,",
        "/// and clicked block state (for slab merging).",
        "pub fn resolve_placement(",
        "    default_state: u32,",
        "    hit_face: u8,",
        "    click_y: f32,",
        "    player_yaw: f32,",
        "    player_pitch: f32,",
        "    clicked_state: u32,",
        ") -> PlacementOutcome {",
        "    let look_h = horizontal_from_yaw(player_yaw);",
        "    let toward_h = opposite_horiz(look_h);",
        "    let is_top = hit_face == 0 || (hit_face != 1 && click_y > 0.5);",
        "",
        "    // 1. Slab placement & double-slab merging",
        "    if let Ok(idx) = SLAB_VARIANTS.binary_search_by_key(&default_state, |(d, _)| *d) {",
        "        let [bot, top, dbl] = SLAB_VARIANTS[idx].1;",
        "        if let Ok(midx) = SLAB_MERGE_DOUBLE.binary_search_by_key(&clicked_state, |(s, _)| *s) {",
        "            if SLAB_MERGE_DOUBLE[midx].1 == dbl {",
        "                return PlacementOutcome {",
        "                    primary_state: dbl,",
        "                    replace_clicked: true,",
        "                    secondary: None,",
        "                };",
        "            }",
        "        }",
        "        return PlacementOutcome {",
        "            primary_state: if is_top { top } else { bot },",
        "            replace_clicked: false,",
        "            secondary: None,",
        "        };",
        "    }",
        "",
        "    // 2. Doors (places lower half + upper half directly above)",
        "    if let Ok(idx) = DOOR_VARIANTS.binary_search_by_key(&default_state, |(d, _)| *d) {",
        "        let [lo, hi] = DOOR_VARIANTS[idx].1;",
        "        return PlacementOutcome {",
        "            primary_state: lo[look_h],",
        "            replace_clicked: false,",
        "            secondary: Some((0, 1, 0, hi[look_h])),",
        "        };",
        "    }",
        "",
        "    // 3. Beds (places foot half + head half in player look direction)",
        "    if let Ok(idx) = BED_VARIANTS.binary_search_by_key(&default_state, |(d, _)| *d) {",
        "        let [ft, hd] = BED_VARIANTS[idx].1;",
        "        let (dx, dy, dz) = horiz_offset(look_h);",
        "        return PlacementOutcome {",
        "            primary_state: ft[look_h],",
        "            replace_clicked: false,",
        "            secondary: Some((dx, dy, dz, hd[look_h])),",
        "        };",
        "    }",
        "",
        "    // 4. Stairs (bottom/top + player look facing)",
        "    if let Ok(idx) = STAIR_VARIANTS.binary_search_by_key(&default_state, |(d, _)| *d) {",
        "        let [bot, top] = STAIR_VARIANTS[idx].1;",
        "        return PlacementOutcome {",
        "            primary_state: if is_top { top[look_h] } else { bot[look_h] },",
        "            replace_clicked: false,",
        "            secondary: None,",
        "        };",
        "    }",
        "",
        "    // 5. Trapdoors (bottom/top + facing toward player)",
        "    if let Ok(idx) = TRAPDOOR_VARIANTS.binary_search_by_key(&default_state, |(d, _)| *d) {",
        "        let [bot, top] = TRAPDOOR_VARIANTS[idx].1;",
        "        return PlacementOutcome {",
        "            primary_state: if is_top { top[toward_h] } else { bot[toward_h] },",
        "            replace_clicked: false,",
        "            secondary: None,",
        "        };",
        "    }",
        "",
        "    // 6. Wall torches when clicking horizontal faces (N/S/W/E = 2..=5)",
        "    if (2..=5).contains(&hit_face) {",
        "        if let Ok(idx) = WALL_TORCH_MAP.binary_search_by_key(&default_state, |(d, _)| *d) {",
        "            return PlacementOutcome {",
        "                primary_state: WALL_TORCH_MAP[idx].1[(hit_face - 2) as usize],",
        "                replace_clicked: false,",
        "                secondary: None,",
        "            };",
        "        }",
        "    }",
        "",
        "    // 7. Hopper (points into clicked face or down)",
        "    if default_state == HOPPER_DEFAULT_STATE {",
        "        let opp = match hit_face {",
        "            2 => 3,",
        "            3 => 2,",
        "            4 => 5,",
        "            5 => 4,",
        "            _ => 0,",
        "        };",
        "        return PlacementOutcome {",
        "            primary_state: HOPPER_FACING_STATES[opp],",
        "            replace_clicked: false,",
        "            secondary: None,",
        "        };",
        "    }",
        "",
        "    // 8. Axis blocks (logs, pillars, basalt, chains)",
        "    if let Ok(idx) = AXIS_VARIANTS.binary_search_by_key(&default_state, |(d, _)| *d) {",
        "        let axis_idx = match hit_face {",
        "            4 | 5 => 0, // X",
        "            0 | 1 => 1, // Y",
        "            _ => 2,     // Z",
        "        };",
        "        return PlacementOutcome {",
        "            primary_state: AXIS_VARIANTS[idx].1[axis_idx],",
        "            replace_clicked: false,",
        "            secondary: None,",
        "        };",
        "    }",
        "",
        "    // 9. 6-way directional blocks (pistons, observers, dispensers, droppers, barrels)",
        "    if let Ok(idx) = FACING6_VARIANTS.binary_search_by_key(&default_state, |(d, _, _)| *d) {",
        "        let (_, arr, face_player) = FACING6_VARIANTS[idx];",
        "        let look_dir: usize = if player_pitch > 45.0 {",
        "            0 // looking down",
        "        } else if player_pitch < -45.0 {",
        "            1 // looking up",
        "        } else {",
        "            look_h + 2",
        "        };",
        "        let dir = if face_player {",
        "            match look_dir { 0 => 1, 1 => 0, 2 => 3, 3 => 2, 4 => 5, _ => 4 }",
        "        } else {",
        "            look_dir",
        "        };",
        "        return PlacementOutcome {",
        "            primary_state: arr[dir],",
        "            replace_clicked: false,",
        "            secondary: None,",
        "        };",
        "    }",
        "",
        "    // 10. 4-way horizontal blocks (chests, furnaces, repeaters, comparators, fence gates, anvils)",
        "    if let Ok(idx) = FACING4_VARIANTS.binary_search_by_key(&default_state, |(d, _, _)| *d) {",
        "        let (_, arr, face_player) = FACING4_VARIANTS[idx];",
        "        let h = if face_player { toward_h } else { look_h };",
        "        return PlacementOutcome {",
        "            primary_state: arr[h],",
        "            replace_clicked: false,",
        "            secondary: None,",
        "        };",
        "    }",
        "",
        "    PlacementOutcome {",
        "        primary_state: default_state,",
        "        replace_clicked: false,",
        "        secondary: None,",
        "    }",
        "}",
        "",
    ])

    OUT_PATH.write_text("\n".join(lines), encoding="utf-8")
    print(
        f"Generated {OUT_PATH.name}: {len(records)} behaviors | "
        f"axis={len(axis_variants)}, slabs={len(slab_variants)}, stairs={len(stair_variants)}, "
        f"trapdoors={len(trapdoor_variants)}, doors={len(door_variants)}, beds={len(bed_variants)}, "
        f"facing6={len(facing6_variants)}, facing4={len(facing4_variants)}, wall_torches={len(wall_torch_map)}"
    )


if __name__ == "__main__":
    main()
