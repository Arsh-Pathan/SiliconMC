#!/usr/bin/env python3
"""
Compiles Minecraft 26.3 block loot tables, entity loot tables, and item weapon/tool/armor
combat attributes from `src/main/resources/data/minecraft/loot_table/` and
`generated/reports/minecraft/components/item/` into `rust/crates/mc-data/src/generated/loot_and_tools.rs`.
"""

import json
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DATA_MC = REPO_ROOT / "src" / "main" / "resources" / "data" / "minecraft"
REPORTS_DIR = REPO_ROOT / "generated" / "reports"
OUT_PATH = REPO_ROOT / "rust" / "crates" / "mc-data" / "src" / "generated" / "loot_and_tools.rs"

SLOT_MAP = {
    "head": 5,
    "chest": 6,
    "legs": 7,
    "feet": 8,
}


def load_item_ids() -> dict[str, int]:
    rt = json.loads((REPORTS_DIR / "runtime_tables.json").read_text(encoding="utf-8"))
    return {name: int(iid) for iid, name, _ms, _ps in rt["items"]}


def extract_count_range(entry: dict) -> tuple[int, int]:
    mn, mx = 1, 1
    mods = entry.get("modifier") or entry.get("functions") or []
    if isinstance(mods, dict):
        mods = [mods]
    for m in mods:
        if m.get("type") == "minecraft:set_count" and not m.get("add", False):
            c = m.get("count")
            if isinstance(c, (int, float)):
                mn = mx = max(0, int(c))
            elif isinstance(c, dict):
                t = c.get("type", "")
                if t == "minecraft:uniform":
                    mn = max(0, int(c.get("min", 1)))
                    mx = max(mn, int(c.get("max", mn)))
                elif t == "minecraft:constant":
                    mn = mx = max(0, int(c.get("value", 1)))
    return (mn, mx)


def is_silk_touch_cond(entry: dict) -> bool:
    cond = entry.get("condition") or entry.get("conditions")
    s = json.dumps(cond) if cond is not None else ""
    return "silk_touch" in s or "can_silk_touch" in s


def extract_drops_from_entry(entry: dict, item_ids: dict[str, int]) -> list[tuple[int, int, int]]:
    etype = entry.get("type", "")
    if etype == "minecraft:item":
        if is_silk_touch_cond(entry):
            return []
        iid = item_ids.get(entry.get("name", ""))
        if iid is None or iid == 0:
            return []
        mn, mx = extract_count_range(entry)
        return [(iid, mn, mx)]
    if etype == "minecraft:alternatives":
        # Pick the first non-silk-touch child (standard survival drop)
        for ch in entry.get("children", []):
            drops = extract_drops_from_entry(ch, item_ids)
            if drops:
                return drops
        return []
    if etype in ("minecraft:group", "minecraft:sequence"):
        out = []
        for ch in entry.get("children", []):
            out.extend(extract_drops_from_entry(ch, item_ids))
        return out
    return []


def extract_table_drops(path: Path, item_ids: dict[str, int]) -> list[tuple[int, int, int]]:
    data = json.loads(path.read_text(encoding="utf-8"))
    out: list[tuple[int, int, int]] = []
    for pool in data.get("pools", []):
        # Skip rare killed-by-player-only low-chance pools? Keep standard pools!
        for entry in pool.get("entries", []):
            out.extend(extract_drops_from_entry(entry, item_ids))
    # Deduplicate by item_id keeping first
    seen = set()
    uniq = []
    for d in out:
        if d[0] not in seen:
            seen.add(d[0])
            uniq.append(d)
    return uniq


def main() -> None:
    item_ids = load_item_ids()

    block_loot: list[tuple[str, tuple[int, int, int]]] = []
    for p in sorted((DATA_MC / "loot_table" / "blocks").glob("*.json")):
        bname = f"minecraft:{p.stem}"
        drops = extract_table_drops(p, item_ids)
        if drops:
            block_loot.append((bname, drops[0]))

    entity_loot: list[tuple[str, list[tuple[int, int, int]]]] = []
    for p in sorted((DATA_MC / "loot_table" / "entities").glob("*.json")):
        ename = f"minecraft:{p.stem}"
        drops = extract_table_drops(p, item_ids)
        if drops:
            entity_loot.append((ename, drops[:4]))

    combat_stats: list[tuple[int, float, float, float, int, int]] = []
    comp_dir = REPORTS_DIR / "minecraft" / "components" / "item"
    for p in sorted(comp_dir.glob("*.json")):
        iname = f"minecraft:{p.stem}"
        iid = item_ids.get(iname)
        if iid is None:
            continue
        comps = json.loads(p.read_text(encoding="utf-8")).get("components", {})
        atk = 0.0
        armor = 0.0
        tough = 0.0
        for mod in comps.get("minecraft:attribute_modifiers", []):
            mt = mod.get("type")
            amt = float(mod.get("amount", 0.0))
            if mt == "minecraft:attack_damage":
                atk += amt
            elif mt == "minecraft:armor":
                armor += amt
            elif mt == "minecraft:armor_toughness":
                tough += amt
        eq = comps.get("minecraft:equippable")
        slot = SLOT_MAP.get(eq.get("slot", ""), 0) if isinstance(eq, dict) else 0
        max_dmg = int(comps.get("minecraft:max_damage", 0))
        if atk != 0.0 or armor != 0.0 or tough != 0.0 or max_dmg > 0 or slot > 0:
            combat_stats.append((iid, atk, armor, tough, slot, max_dmg))
    combat_stats.sort()

    lines = [
        "// @generated by scripts/gen_loot_and_tools.py from Minecraft 26.3 loot tables & item components.",
        "// Do not edit manually.",
        "",
        "#[derive(Debug, Clone, Copy)]",
        "pub struct LootDrop {",
        "    pub item_id: u16,",
        "    pub min_count: u8,",
        "    pub max_count: u8,",
        "}",
        "",
        "#[derive(Debug, Clone, Copy, Default)]",
        "pub struct ItemCombatStats {",
        "    pub attack_damage: f32,",
        "    pub armor: f32,",
        "    pub armor_toughness: f32,",
        "    /// PlayerInventory armor slot (5=head, 6=chest, 7=legs, 8=feet, 0=none).",
        "    pub armor_slot: u8,",
        "    pub max_damage: u16,",
        "}",
        "",
        f"static BLOCK_LOOT: [(&str, LootDrop); {len(block_loot)}] = [",
    ]
    for bname, (iid, mn, mx) in block_loot:
        lines.append(
            f"    ({json.dumps(bname)}, LootDrop {{ item_id: {iid}, min_count: {min(mn, 64)}, max_count: {min(mx, 64)} }}),"
        )
    lines.append("];")
    lines.append("")

    lines.append(f"static ENTITY_LOOT: [(&str, &[LootDrop]); {len(entity_loot)}] = [")
    for ename, drops in entity_loot:
        dstr = ", ".join(
            f"LootDrop {{ item_id: {iid}, min_count: {min(mn, 64)}, max_count: {min(mx, 64)} }}"
            for iid, mn, mx in drops
        )
        lines.append(f"    ({json.dumps(ename)}, &[{dstr}]),")
    lines.append("];")
    lines.append("")

    lines.append(f"static ITEM_COMBAT_STATS: [(u16, ItemCombatStats); {len(combat_stats)}] = [")
    for iid, atk, armor, tough, slot, max_dmg in combat_stats:
        lines.append(
            f"    ({iid}, ItemCombatStats {{ attack_damage: {atk!r}_f32, armor: {armor!r}_f32, armor_toughness: {tough!r}_f32, armor_slot: {slot}, max_damage: {max_dmg} }}),"
        )
    lines.append("];")
    lines.append("")

    lines.extend([
        "#[inline]",
        "pub fn block_loot(block_name: &str) -> Option<LootDrop> {",
        "    BLOCK_LOOT",
        "        .binary_search_by_key(&block_name, |(n, _)| *n)",
        "        .ok()",
        "        .map(|i| BLOCK_LOOT[i].1)",
        "}",
        "",
        "#[inline]",
        "pub fn entity_loot(entity_name: &str) -> &'static [LootDrop] {",
        "    ENTITY_LOOT",
        "        .binary_search_by_key(&entity_name, |(n, _)| *n)",
        "        .ok()",
        "        .map(|i| ENTITY_LOOT[i].1)",
        "        .unwrap_or(&[])",
        "}",
        "",
        "#[inline]",
        "pub fn item_combat_stats(item_id: u16) -> ItemCombatStats {",
        "    ITEM_COMBAT_STATS",
        "        .binary_search_by_key(&item_id, |(id, _)| *id)",
        "        .ok()",
        "        .map(|i| ITEM_COMBAT_STATS[i].1)",
        "        .unwrap_or_default()",
        "}",
        "",
    ])

    OUT_PATH.write_text("\n".join(lines), encoding="utf-8")
    print(
        f"Generated {OUT_PATH.name}: {len(block_loot)} block loot tables, "
        f"{len(entity_loot)} entity loot tables, {len(combat_stats)} item combat/armor stats"
    )


if __name__ == "__main__":
    main()
