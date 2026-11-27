#!/usr/bin/env python3
"""
Generates O(1) exact Minecraft 26.3 block-state property & redstone/interactive state transition
tables from `generated/reports/blocks.json` -> `rust/crates/mc-data/src/generated/block_behaviors.rs`.
"""

import json
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
BLOCKS_JSON = REPO_ROOT / "generated" / "reports" / "blocks.json"
OUT_PATH = REPO_ROOT / "rust" / "crates" / "mc-data" / "src" / "generated" / "block_behaviors.rs"

FACING_MAP = {"down": 0, "up": 1, "north": 2, "south": 3, "west": 4, "east": 5}

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


def classify_kind(block_name: str, props: dict[str, str]) -> int:
    # 0=Normal, 1=RedstoneWire, 2=RedstoneTorch, 3=RedstoneLamp, 4=Lever, 5=Button,
    # 6=Repeater, 7=Door, 8=Trapdoor, 9=FenceGate, 10=PressurePlate, 11=RedstoneBlock, 12=FallingBlock
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
    return 0


def main() -> None:
    blocks = json.loads(BLOCKS_JSON.read_text(encoding="utf-8"))
    total_states = sum(len(b["states"]) for b in blocks.values())

    # Per-state compact record:
    # kind: u8, meta: u8 (power/delay/flags), facing: u8 (0..5, 255=none),
    # toggle_lit: u32 (u32::MAX if none), toggle_powered: u32, toggle_open: u32
    records: list[tuple[int, int, int, int, int, int]] = [(0, 0, 255, 0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF_FFFF)] * total_states
    wire_power_table: dict[int, list[int]] = {}  # state_id -> [id_p0..id_p15] (grouped by base props)
    wire_variants: list[list[int]] = []
    wire_state_to_variant: list[tuple[int, int]] = []  # (state_id, variant_idx)
    water_states = [0] * 16
    lava_states = [0] * 16
    piston_states = [[0, 0] for _ in range(6)]
    sticky_piston_states = [[0, 0] for _ in range(6)]
    piston_head_states = [[0, 0] for _ in range(6)]

    for block_name, bdata in blocks.items():
        # Build lookup from canonical sorted property tuple -> state id within this block
        lookup: dict[tuple[tuple[str, str], ...], int] = {}
        for st in bdata["states"]:
            props = st.get("properties") or {}
            key = tuple(sorted(props.items()))
            lookup[key] = int(st["id"])

        def find_with(props: dict[str, str], k: str, v: str) -> int:
            if k not in props:
                return 0xFFFF_FFFF
            np = dict(props)
            np[k] = v
            return lookup.get(tuple(sorted(np.items())), 0xFFFF_FFFF)

        if block_name == "minecraft:redstone_wire":
            # Group by non-power properties
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

        for st in bdata["states"]:
            sid = int(st["id"])
            props = st.get("properties") or {}
            kind = classify_kind(block_name, props)

            # meta byte:
            # bits 0..3: redstone power (0..15) or repeater delay (1..4)
            # bit 4: is_lit
            # bit 5: is_powered
            # bit 6: is_open
            # bit 7: is_upper_half (doors) or wall_torch
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
            if props.get("powered") == "true" or props.get("enabled") == "true":
                meta |= 1 << 5
            if props.get("open") == "true" or props.get("extended") == "true":
                meta |= 1 << 6
            if props.get("half") == "upper" or block_name == "minecraft:redstone_wall_torch":
                meta |= 1 << 7

            facing = FACING_MAP.get(props.get("facing", ""), 255)
            if block_name == "minecraft:piston" and facing < 6:
                piston_states[facing][1 if props.get("extended") == "true" else 0] = sid
            elif block_name == "minecraft:sticky_piston" and facing < 6:
                sticky_piston_states[facing][1 if props.get("extended") == "true" else 0] = sid
            elif block_name == "minecraft:piston_head" and facing < 6 and props.get("short") == "false":
                piston_head_states[facing][1 if props.get("type") == "sticky" else 0] = sid

            t_lit = find_with(props, "lit", "false" if props.get("lit") == "true" else "true")
            t_pow = find_with(props, "powered", "false" if props.get("powered") == "true" else "true")
            if "extended" in props:
                t_open = find_with(props, "extended", "false" if props.get("extended") == "true" else "true")
            else:
                t_open = find_with(props, "open", "false" if props.get("open") == "true" else "true")
            records[sid] = (kind, meta, facing, t_lit, t_pow, t_open)

    wire_state_to_variant.sort()

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
        "",
        f"pub const WATER_STATES: [u32; 16] = [{', '.join(str(x) for x in water_states)}];",
        f"pub const LAVA_STATES: [u32; 16] = [{', '.join(str(x) for x in lava_states)}];",
        f"pub const PISTON_STATES: [[u32; 2]; 6] = [{', '.join(f'[{a}, {b}]' for a, b in piston_states)}];",
        f"pub const STICKY_PISTON_STATES: [[u32; 2]; 6] = [{', '.join(f'[{a}, {b}]' for a, b in sticky_piston_states)}];",
        f"pub const PISTON_HEAD_STATES: [[u32; 2]; 6] = [{', '.join(f'[{a}, {b}]' for a, b in piston_head_states)}];",
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
    ])
    OUT_PATH.write_text("\n".join(lines), encoding="utf-8")
    print(f"Generated {OUT_PATH.name}: {len(records)} block behaviors, {len(wire_variants)} redstone wire variants")


if __name__ == "__main__":
    main()
