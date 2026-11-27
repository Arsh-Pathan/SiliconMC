#!/usr/bin/env python3
"""
Compiles Minecraft 26.3 biome natural mob spawn tables from
`src/main/resources/data/minecraft/worldgen/biome/*.json` and canonical vanilla
mob attribute profiles into `rust/crates/mc-data/src/generated/mob_spawns.rs`.
"""

from __future__ import annotations

import json
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
BIOME_DIR = REPO_ROOT / "src" / "main" / "resources" / "data" / "minecraft" / "worldgen" / "biome"
OUT_PATH = REPO_ROOT / "rust" / "crates" / "mc-data" / "src" / "generated" / "mob_spawns.rs"


# (entity_type, max_health, move_speed, attack_damage, xp_reward, hostile, burns_in_sun, breed_item)
MOB_PROFILES: list[tuple[str, float, float, float, int, bool, bool, str]] = [
    ("minecraft:allay", 20.0, 0.1, 0.0, 0, False, False, ""),
    ("minecraft:armadillo", 12.0, 0.14, 0.0, 3, False, False, "minecraft:spider_eye"),
    ("minecraft:axolotl", 14.0, 0.2, 2.0, 3, False, False, "minecraft:tropical_fish_bucket"),
    ("minecraft:bat", 6.0, 0.2, 0.0, 0, False, False, ""),
    ("minecraft:bee", 10.0, 0.3, 2.0, 3, False, False, "minecraft:dandelion"),
    ("minecraft:blaze", 20.0, 0.23, 6.0, 10, True, False, ""),
    ("minecraft:bogged", 16.0, 0.25, 2.0, 5, True, True, ""),
    ("minecraft:breeze", 30.0, 0.63, 3.0, 10, True, False, ""),
    ("minecraft:camel", 32.0, 0.09, 0.0, 3, False, False, "minecraft:cactus"),
    ("minecraft:camel_husk", 32.0, 0.09, 0.0, 3, False, False, ""),
    ("minecraft:cat", 10.0, 0.3, 3.0, 3, False, False, "minecraft:cod"),
    ("minecraft:cave_spider", 12.0, 0.3, 2.0, 5, True, False, ""),
    ("minecraft:chicken", 4.0, 0.25, 0.0, 3, False, False, "minecraft:wheat_seeds"),
    ("minecraft:cod", 3.0, 0.2, 0.0, 3, False, False, ""),
    ("minecraft:copper_golem", 12.0, 0.2, 0.0, 0, False, False, ""),
    ("minecraft:cow", 10.0, 0.2, 0.0, 3, False, False, "minecraft:wheat"),
    ("minecraft:creaking", 1.0, 0.4, 3.0, 0, True, False, ""),
    ("minecraft:creeper", 20.0, 0.25, 0.0, 5, True, False, ""),
    ("minecraft:dolphin", 10.0, 1.2, 3.0, 3, False, False, ""),
    ("minecraft:donkey", 22.0, 0.175, 0.0, 3, False, False, "minecraft:golden_carrot"),
    ("minecraft:drowned", 20.0, 0.23, 3.0, 5, True, True, ""),
    ("minecraft:elder_guardian", 80.0, 0.3, 8.0, 10, True, False, ""),
    ("minecraft:ender_dragon", 200.0, 0.7, 10.0, 500, True, False, ""),
    ("minecraft:enderman", 40.0, 0.3, 7.0, 5, True, False, ""),
    ("minecraft:endermite", 8.0, 0.25, 2.0, 3, True, False, ""),
    ("minecraft:evoker", 24.0, 0.5, 6.0, 10, True, False, ""),
    ("minecraft:fox", 10.0, 0.3, 2.0, 3, False, False, "minecraft:sweet_berries"),
    ("minecraft:frog", 10.0, 0.2, 0.0, 3, False, False, "minecraft:slime_ball"),
    ("minecraft:ghast", 10.0, 0.2, 6.0, 5, True, False, ""),
    ("minecraft:glow_squid", 10.0, 0.2, 0.0, 3, False, False, ""),
    ("minecraft:goat", 10.0, 0.2, 2.0, 3, False, False, "minecraft:wheat"),
    ("minecraft:guardian", 30.0, 0.5, 6.0, 10, True, False, ""),
    ("minecraft:happy_ghast", 20.0, 0.05, 0.0, 3, False, False, "minecraft:snowball"),
    ("minecraft:hoglin", 40.0, 0.3, 6.0, 5, True, False, "minecraft:crimson_fungus"),
    ("minecraft:horse", 22.0, 0.225, 0.0, 3, False, False, "minecraft:golden_carrot"),
    ("minecraft:husk", 20.0, 0.23, 3.0, 5, True, False, ""),
    ("minecraft:iron_golem", 100.0, 0.25, 15.0, 0, False, False, ""),
    ("minecraft:llama", 22.0, 0.25, 1.0, 3, False, False, "minecraft:hay_block"),
    ("minecraft:magma_cube", 16.0, 0.4, 6.0, 4, True, False, ""),
    ("minecraft:mooshroom", 10.0, 0.2, 0.0, 3, False, False, "minecraft:wheat"),
    ("minecraft:mule", 22.0, 0.175, 0.0, 3, False, False, ""),
    ("minecraft:nautilus", 10.0, 0.3, 0.0, 3, False, False, ""),
    ("minecraft:ocelot", 10.0, 0.3, 3.0, 3, False, False, "minecraft:cod"),
    ("minecraft:panda", 20.0, 0.15, 6.0, 3, False, False, "minecraft:bamboo"),
    ("minecraft:parched", 16.0, 0.25, 2.0, 5, True, False, ""),
    ("minecraft:parrot", 6.0, 0.2, 0.0, 3, False, False, ""),
    ("minecraft:phantom", 20.0, 0.3, 2.0, 5, True, True, ""),
    ("minecraft:pig", 10.0, 0.25, 0.0, 3, False, False, "minecraft:carrot"),
    ("minecraft:piglin", 16.0, 0.35, 5.0, 5, True, False, ""),
    ("minecraft:piglin_brute", 50.0, 0.35, 7.0, 20, True, False, ""),
    ("minecraft:pillager", 24.0, 0.35, 5.0, 5, True, False, ""),
    ("minecraft:polar_bear", 30.0, 0.25, 6.0, 3, False, False, ""),
    ("minecraft:pufferfish", 3.0, 0.2, 0.0, 3, False, False, ""),
    ("minecraft:rabbit", 3.0, 0.3, 0.0, 3, False, False, "minecraft:carrot"),
    ("minecraft:ravager", 100.0, 0.3, 12.0, 20, True, False, ""),
    ("minecraft:salmon", 3.0, 0.2, 0.0, 3, False, False, ""),
    ("minecraft:sheep", 8.0, 0.23, 0.0, 3, False, False, "minecraft:wheat"),
    ("minecraft:shulker", 30.0, 0.0, 4.0, 5, True, False, ""),
    ("minecraft:silverfish", 8.0, 0.25, 1.0, 5, True, False, ""),
    ("minecraft:skeleton", 20.0, 0.25, 2.0, 5, True, True, ""),
    ("minecraft:skeleton_horse", 15.0, 0.2, 0.0, 3, False, False, ""),
    ("minecraft:slime", 16.0, 0.4, 4.0, 4, True, False, ""),
    ("minecraft:sniffer", 14.0, 0.09, 0.0, 3, False, False, "minecraft:torchflower_seeds"),
    ("minecraft:snow_golem", 4.0, 0.2, 0.0, 0, False, False, ""),
    ("minecraft:spider", 16.0, 0.3, 2.0, 5, True, False, ""),
    ("minecraft:squid", 10.0, 0.2, 0.0, 3, False, False, ""),
    ("minecraft:stray", 20.0, 0.25, 2.0, 5, True, True, ""),
    ("minecraft:strider", 20.0, 0.175, 0.0, 3, False, False, "minecraft:warped_fungus"),
    ("minecraft:sulfur_cube", 16.0, 0.3, 4.0, 4, True, False, ""),
    ("minecraft:tadpole", 6.0, 0.2, 0.0, 0, False, False, ""),
    ("minecraft:trader_llama", 22.0, 0.25, 1.0, 3, False, False, "minecraft:hay_block"),
    ("minecraft:tropical_fish", 3.0, 0.2, 0.0, 3, False, False, ""),
    ("minecraft:turtle", 30.0, 0.25, 0.0, 3, False, False, "minecraft:seagrass"),
    ("minecraft:vex", 14.0, 0.4, 4.0, 3, True, False, ""),
    ("minecraft:villager", 20.0, 0.5, 0.0, 0, False, False, ""),
    ("minecraft:vindicator", 24.0, 0.35, 5.0, 5, True, False, ""),
    ("minecraft:wandering_trader", 20.0, 0.5, 0.0, 0, False, False, ""),
    ("minecraft:warden", 500.0, 0.3, 30.0, 5, True, False, ""),
    ("minecraft:witch", 26.0, 0.25, 0.0, 5, True, False, ""),
    ("minecraft:wither", 300.0, 0.6, 8.0, 50, True, False, ""),
    ("minecraft:wither_skeleton", 20.0, 0.25, 4.0, 5, True, False, ""),
    ("minecraft:wolf", 8.0, 0.3, 4.0, 3, False, False, "minecraft:cooked_beef"),
    ("minecraft:zoglin", 40.0, 0.3, 6.0, 5, True, False, ""),
    ("minecraft:zombie", 20.0, 0.23, 3.0, 5, True, True, ""),
    ("minecraft:zombie_horse", 15.0, 0.2, 0.0, 5, False, True, ""),
    ("minecraft:zombie_nautilus", 15.0, 1.1, 3.0, 5, True, True, ""),
    ("minecraft:zombie_villager", 20.0, 0.23, 3.0, 5, True, True, ""),
    ("minecraft:zombified_piglin", 20.0, 0.23, 5.0, 5, False, False, ""),
]


def parse_count(entry: dict) -> tuple[int, int]:
    """Parse 26.3 `count` (int or uniform IntProvider) or legacy `minCount`/`maxCount`."""
    if "count" in entry:
        c = entry["count"]
        if isinstance(c, int):
            v = max(1, min(255, c))
            return (v, v)
        if isinstance(c, dict):
            mn = int(c.get("min_inclusive", c.get("min", 1)))
            mx = int(c.get("max_inclusive", c.get("max", mn)))
            mn = max(1, min(255, mn))
            mx = max(mn, min(255, mx))
            return (mn, mx)
    mn = int(entry.get("minCount", 1))
    mx = int(entry.get("maxCount", mn))
    mn = max(1, min(255, mn))
    mx = max(mn, min(255, mx))
    return (mn, mx)


def parse_spawn_entry(raw: dict) -> tuple[str, int, int, int] | None:
    """Handle 26.3 `{type, weight, count}`, legacy `{type, weight, minCount, maxCount}`,
    and weighted-wrapper `{weight, data: {type, minCount, maxCount}}`."""
    if "data" in raw and isinstance(raw["data"], dict):
        data = raw["data"]
        entity_type = str(data.get("type", ""))
        weight = int(raw.get("weight", data.get("weight", 1)))
        mn, mx = parse_count(data)
    else:
        entity_type = str(raw.get("type", ""))
        weight = int(raw.get("weight", 1))
        mn, mx = parse_count(raw)

    if not entity_type or weight <= 0:
        return None
    if ":" not in entity_type:
        entity_type = f"minecraft:{entity_type}"
    return (entity_type, min(65535, weight), mn, mx)


def extract_spawns_by_category(biome_data: dict) -> dict[str, list[dict]]:
    """Extract category -> raw entries from Minecraft 26.3 biome attributes or legacy `spawners`."""
    attrs = biome_data.get("attributes")
    if isinstance(attrs, dict):
        nms = attrs.get("minecraft:gameplay/natural_mob_spawns")
        if isinstance(nms, dict):
            arg = nms.get("argument", nms)
            if isinstance(arg, dict):
                sbc = arg.get("spawns_by_category")
                if isinstance(sbc, dict):
                    return sbc
    spawners = biome_data.get("spawners")
    if isinstance(spawners, dict):
        return spawners
    return {}


def parse_category(sbc: dict[str, list[dict]], categories: tuple[str, ...]) -> list[tuple[str, int, int, int]]:
    out: list[tuple[str, int, int, int]] = []
    for cat in categories:
        for raw in sbc.get(cat, []):
            if isinstance(raw, dict):
                parsed = parse_spawn_entry(raw)
                if parsed is not None:
                    out.append(parsed)
    return out


def format_entries(entries: list[tuple[str, int, int, int]]) -> str:
    if not entries:
        return "&[]"
    parts = [
        f"MobSpawnEntry {{ entity_type: {json.dumps(et)}, weight: {w}, min_count: {mn}, max_count: {mx} }}"
        for et, w, mn, mx in entries
    ]
    return "&[" + ", ".join(parts) + "]"


def format_f32(val: float) -> str:
    s = f"{val!r}"
    if "." not in s and "e" not in s and "E" not in s:
        s += ".0"
    return f"{s}_f32"


def main() -> None:
    biome_files = sorted(BIOME_DIR.glob("*.json"))
    if not biome_files:
        raise SystemExit(f"No biome JSON files found in {BIOME_DIR}")

    tables: list[tuple[str, list, list, list, list]] = []
    total_monsters = 0
    total_creatures = 0
    total_water = 0
    total_ambient = 0
    unique_entities: set[str] = set()

    for path in biome_files:
        biome_name = f"minecraft:{path.stem}"
        data = json.loads(path.read_text(encoding="utf-8"))
        sbc = extract_spawns_by_category(data)

        monsters = parse_category(sbc, ("monster",))
        creatures = parse_category(sbc, ("creature",))
        water = parse_category(
            sbc,
            ("water_creature", "water_ambient", "underground_water_creature", "axolotls"),
        )
        ambient = parse_category(sbc, ("ambient",))

        total_monsters += len(monsters)
        total_creatures += len(creatures)
        total_water += len(water)
        total_ambient += len(ambient)
        for lst in (monsters, creatures, water, ambient):
            for et, _, _, _ in lst:
                unique_entities.add(et)

        tables.append((biome_name, monsters, creatures, water, ambient))

    tables.sort(key=lambda row: row[0])
    profiles = sorted(MOB_PROFILES, key=lambda row: row[0])

    # Sanity-check that every entity spawned in biomes has a MobProfile entry
    profile_names = {row[0] for row in profiles}
    missing = sorted(unique_entities - profile_names)
    if missing:
        raise SystemExit(f"Missing MobProfile definitions for spawned entities: {missing}")

    lines: list[str] = [
        "// @generated by scripts/gen_mob_spawns.py from Minecraft 26.3 worldgen biome JSONs.",
        "// Do not edit manually.",
        "",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq)]",
        "pub struct MobSpawnEntry {",
        "    pub entity_type: &'static str,",
        "    pub weight: u16,",
        "    pub min_count: u8,",
        "    pub max_count: u8,",
        "}",
        "",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq)]",
        "pub struct BiomeSpawnTable {",
        "    pub biome: &'static str,",
        "    pub monsters: &'static [MobSpawnEntry],",
        "    pub creatures: &'static [MobSpawnEntry],",
        "    pub water: &'static [MobSpawnEntry],",
        "}",
        "",
        "#[derive(Debug, Clone, Copy, PartialEq)]",
        "pub struct MobProfile {",
        "    pub entity_type: &'static str,",
        "    pub max_health: f32,",
        "    pub move_speed: f32,",
        "    pub attack_damage: f32,",
        "    pub xp_reward: i32,",
        "    pub hostile: bool,",
        "    pub burns_in_sun: bool,",
        "    pub breed_item: &'static str,",
        "}",
        "",
        "const DEFAULT_MOB_PROFILE: MobProfile = MobProfile {",
        '    entity_type: "minecraft:unknown",',
        "    max_health: 20.0_f32,",
        "    move_speed: 0.25_f32,",
        "    attack_damage: 2.0_f32,",
        "    xp_reward: 3,",
        "    hostile: false,",
        "    burns_in_sun: false,",
        '    breed_item: "",',
        "};",
        "",
        f"pub static BIOME_SPAWN_TABLES: [BiomeSpawnTable; {len(tables)}] = [",
    ]

    for biome_name, monsters, creatures, water, _ambient in tables:
        lines.append("    BiomeSpawnTable {")
        lines.append(f"        biome: {json.dumps(biome_name)},")
        lines.append(f"        monsters: {format_entries(monsters)},")
        lines.append(f"        creatures: {format_entries(creatures)},")
        lines.append(f"        water: {format_entries(water)},")
        lines.append("    },")
    lines.append("];")
    lines.append("")

    lines.append(f"pub static BIOME_AMBIENT_SPAWNS: [(&str, &[MobSpawnEntry]); {len(tables)}] = [")
    for biome_name, _monsters, _creatures, _water, ambient in tables:
        lines.append(f"    ({json.dumps(biome_name)}, {format_entries(ambient)}),")
    lines.append("];")
    lines.append("")

    lines.append(f"pub static MOB_PROFILES: [MobProfile; {len(profiles)}] = [")
    for entity_type, hp, spd, dmg, xp, hostile, burns, breed in profiles:
        h_str = "true" if hostile else "false"
        b_str = "true" if burns else "false"
        lines.append(
            f"    MobProfile {{ entity_type: {json.dumps(entity_type)}, "
            f"max_health: {format_f32(hp)}, move_speed: {format_f32(spd)}, "
            f"attack_damage: {format_f32(dmg)}, xp_reward: {xp}, "
            f"hostile: {h_str}, burns_in_sun: {b_str}, breed_item: {json.dumps(breed)} }},"
        )
    lines.append("];")
    lines.append("")

    lines.extend(
        [
            "#[inline]",
            "fn strip_minecraft_ns(name: &str) -> Option<&str> {",
            '    if let Some(short) = name.strip_prefix("minecraft:") {',
            "        Some(short)",
            "    } else if name.contains(':') {",
            "        None",
            "    } else {",
            "        Some(name)",
            "    }",
            "}",
            "",
            "/// Binary-search the biome spawn table for `biome` (accepts `\"minecraft:plains\"` or `\"plains\"`).",
            "#[inline]",
            "pub fn biome_spawn_table(biome: &str) -> Option<&'static BiomeSpawnTable> {",
            "    let short = strip_minecraft_ns(biome)?;",
            "    BIOME_SPAWN_TABLES",
            '        .binary_search_by_key(&short, |t| t.biome.strip_prefix("minecraft:").unwrap_or(t.biome))',
            "        .ok()",
            "        .map(|idx| &BIOME_SPAWN_TABLES[idx])",
            "}",
            "",
            "/// Binary-search the ambient spawn entries (e.g. bats) for `biome`.",
            "#[inline]",
            "pub fn biome_ambient_spawns(biome: &str) -> &'static [MobSpawnEntry] {",
            "    let Some(short) = strip_minecraft_ns(biome) else {",
            "        return &[];",
            "    };",
            "    BIOME_AMBIENT_SPAWNS",
            '        .binary_search_by_key(&short, |(b, _)| b.strip_prefix("minecraft:").unwrap_or(b))',
            "        .ok()",
            "        .map(|idx| BIOME_AMBIENT_SPAWNS[idx].1)",
            "        .unwrap_or(&[])",
            "}",
            "",
            "/// Look up the vanilla Minecraft 26.3 attribute profile for `entity_type`",
            "/// (accepts `\"minecraft:zombie\"` or `\"zombie\"`).",
            "#[inline]",
            "pub fn mob_profile(entity_type: &str) -> MobProfile {",
            "    let Some(short) = strip_minecraft_ns(entity_type) else {",
            "        return DEFAULT_MOB_PROFILE;",
            "    };",
            "    MOB_PROFILES",
            '        .binary_search_by_key(&short, |p| p.entity_type.strip_prefix("minecraft:").unwrap_or(p.entity_type))',
            "        .ok()",
            "        .map(|idx| MOB_PROFILES[idx])",
            "        .unwrap_or(DEFAULT_MOB_PROFILE)",
            "}",
            "",
        ]
    )

    OUT_PATH.write_text("\n".join(lines), encoding="utf-8")
    print(
        f"Generated {OUT_PATH.name}: {len(tables)} biomes "
        f"({total_monsters} monster, {total_creatures} creature, {total_water} water, {total_ambient} ambient entries; "
        f"{len(unique_entities)} unique spawned entities), {len(profiles)} mob profiles"
    )


if __name__ == "__main__":
    main()
