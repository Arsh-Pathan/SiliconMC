//! Deterministic Minecraft 26.3 natural structure placement (`RandomSpreadStructurePlacement`)
//! backed by real vanilla `.nbt` `StructureTemplate` assets.

use mc_data::BlockStateId;
use mc_math::{BlockPos, ChunkPos, LegacyRandomSource, RandomSource, floor_div};
use mc_world::{BlockWorld, Chunk, StructureRotation, StructureTemplate};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// Placement specification for a natural vanilla Minecraft 26.3 structure set.
#[derive(Debug, Clone, Copy)]
pub struct StructurePlacementRule {
    pub name: &'static str,
    pub template_id: &'static str,
    pub spacing: i32,
    pub separation: i32,
    pub salt: i32,
    pub allowed_biomes: &'static [&'static str],
    pub y_offset: i32,
}

/// Curated Minecraft 26.3 Overworld structure rules using exact `worldgen/structure_set/*.json`
/// `spacing`, `separation`, and `salt` values and real vanilla `.nbt` templates.
pub static OVERWORLD_STRUCTURE_RULES: &[StructurePlacementRule] = &[
    StructurePlacementRule {
        name: "igloo",
        template_id: "igloo/top",
        spacing: 32,
        separation: 8,
        salt: 14_357_618,
        allowed_biomes: &[
            "minecraft:snowy_plains",
            "minecraft:snowy_taiga",
            "minecraft:snowy_slopes",
        ],
        y_offset: 0,
    },
    StructurePlacementRule {
        name: "village_plains",
        template_id: "village/plains/houses/plains_small_house_1",
        spacing: 34,
        separation: 8,
        salt: 10_387_312,
        allowed_biomes: &[
            "minecraft:plains",
            "minecraft:meadow",
            "minecraft:sunflower_plains",
        ],
        y_offset: 0,
    },
    StructurePlacementRule {
        name: "village_desert",
        template_id: "village/desert/houses/desert_small_house_1",
        spacing: 34,
        separation: 8,
        salt: 10_387_312,
        allowed_biomes: &["minecraft:desert"],
        y_offset: 0,
    },
    StructurePlacementRule {
        name: "village_savanna",
        template_id: "village/savanna/houses/savanna_small_house_1",
        spacing: 34,
        separation: 8,
        salt: 10_387_312,
        allowed_biomes: &["minecraft:savanna", "minecraft:savanna_plateau"],
        y_offset: 0,
    },
    StructurePlacementRule {
        name: "village_taiga",
        template_id: "village/taiga/houses/taiga_small_house_1",
        spacing: 34,
        separation: 8,
        salt: 10_387_312,
        allowed_biomes: &[
            "minecraft:taiga",
            "minecraft:old_growth_pine_taiga",
            "minecraft:old_growth_spruce_taiga",
        ],
        y_offset: 0,
    },
    StructurePlacementRule {
        name: "village_snowy",
        template_id: "village/snowy/houses/snowy_small_house_1",
        spacing: 34,
        separation: 8,
        salt: 10_387_312,
        allowed_biomes: &["minecraft:snowy_plains"],
        y_offset: 0,
    },
    StructurePlacementRule {
        name: "pillager_outpost",
        template_id: "pillager_outpost/watchtower",
        spacing: 32,
        separation: 8,
        salt: 165_745_296,
        allowed_biomes: &[
            "minecraft:plains",
            "minecraft:desert",
            "minecraft:savanna",
            "minecraft:snowy_plains",
            "minecraft:taiga",
            "minecraft:meadow",
            "minecraft:grove",
            "minecraft:snowy_slopes",
            "minecraft:frozen_peaks",
            "minecraft:jagged_peaks",
            "minecraft:stony_peaks",
        ],
        y_offset: 0,
    },
    StructurePlacementRule {
        name: "desert_pyramid",
        template_id: "village/desert/houses/desert_temple_1",
        spacing: 32,
        separation: 8,
        salt: 14_357_617,
        allowed_biomes: &["minecraft:desert"],
        y_offset: 0,
    },
    StructurePlacementRule {
        name: "desert_well",
        template_id: "desert_well/well",
        spacing: 24,
        separation: 6,
        salt: 14_357_619,
        allowed_biomes: &["minecraft:desert"],
        y_offset: -1,
    },
    StructurePlacementRule {
        name: "swamp_hut",
        template_id: "abandoned_camp/camp/swamp/campsite_swamp_1",
        spacing: 32,
        separation: 8,
        salt: 14_357_620,
        allowed_biomes: &["minecraft:swamp", "minecraft:mangrove_swamp"],
        y_offset: 0,
    },
    StructurePlacementRule {
        name: "abandoned_camp_forest",
        template_id: "abandoned_camp/camp/forest/campsite_forest_1",
        spacing: 37,
        separation: 8,
        salt: 91_231_127,
        allowed_biomes: &["minecraft:forest", "minecraft:flower_forest"],
        y_offset: 0,
    },
    StructurePlacementRule {
        name: "abandoned_camp_birch",
        template_id: "abandoned_camp/camp/birch_forest/campsite_birch_forest_1",
        spacing: 37,
        separation: 8,
        salt: 91_231_127,
        allowed_biomes: &[
            "minecraft:birch_forest",
            "minecraft:old_growth_birch_forest",
        ],
        y_offset: 0,
    },
    StructurePlacementRule {
        name: "abandoned_camp_cherry",
        template_id: "abandoned_camp/camp/cherry_grove/campsite_cherry_grove_1",
        spacing: 37,
        separation: 8,
        salt: 91_231_127,
        allowed_biomes: &["minecraft:cherry_grove"],
        y_offset: 0,
    },
    StructurePlacementRule {
        name: "ruined_portal",
        template_id: "ruined_portal/portal_1",
        spacing: 40,
        separation: 15,
        salt: 34_222_645,
        allowed_biomes: &[],
        y_offset: 0,
    },
];

/// Curated Minecraft 26.3 Nether structure rules (`nether_fossils` & `ruined_portals`).
pub static NETHER_STRUCTURE_RULES: &[StructurePlacementRule] = &[
    StructurePlacementRule {
        name: "ruined_portal_nether",
        template_id: "ruined_portal/portal_1",
        spacing: 40,
        separation: 15,
        salt: 34_222_645,
        allowed_biomes: &[
            "minecraft:nether_wastes",
            "minecraft:crimson_forest",
            "minecraft:warped_forest",
            "minecraft:soul_sand_valley",
            "minecraft:basalt_deltas",
        ],
        y_offset: 0,
    },
    StructurePlacementRule {
        name: "nether_fossil",
        template_id: "nether_fossils/fossil_1",
        spacing: 2,
        separation: 1,
        salt: 14_357_921,
        allowed_biomes: &["minecraft:soul_sand_valley"],
        y_offset: 1,
    },
];

fn variants_for_rule(rule: &StructurePlacementRule) -> &'static [&'static str] {
    match rule.name {
        "igloo" => &["igloo/top"],
        "village_plains" => &[
            "village/plains/houses/plains_small_house_1",
            "village/plains/houses/plains_small_house_2",
            "village/plains/houses/plains_small_house_3",
            "village/plains/houses/plains_small_house_4",
            "village/plains/houses/plains_small_house_5",
            "village/plains/houses/plains_medium_house_1",
            "village/plains/houses/plains_library_1",
            "village/plains/houses/plains_weaponsmith_1",
        ],
        "village_desert" => &[
            "village/desert/houses/desert_small_house_1",
            "village/desert/houses/desert_small_house_2",
            "village/desert/houses/desert_small_house_3",
            "village/desert/houses/desert_small_house_4",
            "village/desert/houses/desert_medium_house_1",
            "village/desert/houses/desert_library_1",
            "village/desert/houses/desert_weaponsmith_1",
        ],
        "village_savanna" => &[
            "village/savanna/houses/savanna_small_house_1",
            "village/savanna/houses/savanna_small_house_2",
            "village/savanna/houses/savanna_small_house_3",
            "village/savanna/houses/savanna_small_house_4",
            "village/savanna/houses/savanna_medium_house_1",
            "village/savanna/houses/savanna_library_1",
            "village/savanna/houses/savanna_weaponsmith_1",
        ],
        "village_taiga" => &[
            "village/taiga/houses/taiga_small_house_1",
            "village/taiga/houses/taiga_small_house_2",
            "village/taiga/houses/taiga_small_house_3",
            "village/taiga/houses/taiga_small_house_4",
            "village/taiga/houses/taiga_medium_house_1",
            "village/taiga/houses/taiga_library_1",
            "village/taiga/houses/taiga_weaponsmith_1",
        ],
        "village_snowy" => &[
            "village/snowy/houses/snowy_small_house_1",
            "village/snowy/houses/snowy_small_house_2",
            "village/snowy/houses/snowy_small_house_3",
            "village/snowy/houses/snowy_small_house_4",
            "village/snowy/houses/snowy_medium_house_1",
            "village/snowy/houses/snowy_library_1",
            "village/snowy/houses/snowy_weapon_smith_1",
        ],
        "pillager_outpost" => &[
            "pillager_outpost/watchtower",
            "pillager_outpost/watchtower_overgrown",
        ],
        "desert_pyramid" => &[
            "village/desert/houses/desert_temple_1",
            "village/desert/houses/desert_temple_2",
        ],
        "desert_well" => &["desert_well/well"],
        "swamp_hut" => &[
            "abandoned_camp/camp/swamp/campsite_swamp_1",
            "abandoned_camp/camp/swamp/campsite_swamp_2",
            "abandoned_camp/camp/swamp/campsite_swamp_3",
            "abandoned_camp/camp/swamp/campsite_swamp_4",
        ],
        "abandoned_camp_forest" => &[
            "abandoned_camp/camp/forest/campsite_forest_1",
            "abandoned_camp/camp/forest/campsite_forest_2",
            "abandoned_camp/camp/forest/campsite_forest_3",
            "abandoned_camp/camp/forest/campsite_forest_4",
        ],
        "abandoned_camp_birch" => &[
            "abandoned_camp/camp/birch_forest/campsite_birch_forest_1",
            "abandoned_camp/camp/birch_forest/campsite_birch_forest_2",
            "abandoned_camp/camp/birch_forest/campsite_birch_forest_3",
            "abandoned_camp/camp/birch_forest/campsite_birch_forest_4",
        ],
        "abandoned_camp_cherry" => &[
            "abandoned_camp/camp/cherry_grove/campsite_cherry_grove_1",
            "abandoned_camp/camp/cherry_grove/campsite_cherry_grove_2",
            "abandoned_camp/camp/cherry_grove/campsite_cherry_grove_3",
            "abandoned_camp/camp/cherry_grove/campsite_cherry_grove_4",
        ],
        "ruined_portal" => &[
            "ruined_portal/portal_1",
            "ruined_portal/portal_2",
            "ruined_portal/portal_3",
            "ruined_portal/portal_4",
            "ruined_portal/portal_5",
            "ruined_portal/portal_6",
            "ruined_portal/portal_7",
            "ruined_portal/portal_8",
            "ruined_portal/portal_9",
            "ruined_portal/portal_10",
        ],
        "ruined_portal_nether" => &[
            "ruined_portal/portal_1",
            "ruined_portal/portal_2",
            "ruined_portal/portal_3",
        ],
        "nether_fossil" => &[
            "nether_fossils/fossil_1",
            "nether_fossils/fossil_2",
            "nether_fossils/fossil_3",
            "nether_fossils/fossil_4",
        ],
        _ => &[],
    }
}

/// Process-wide cache of parsed `.nbt` `StructureTemplate`s for zero-I/O chunk generation.
static TEMPLATE_CACHE: OnceLock<Mutex<HashMap<String, StructureTemplate>>> = OnceLock::new();

fn load_cached_template(data_dir: &Path, template_id: &str) -> Option<StructureTemplate> {
    let cache = TEMPLATE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = format!("{}::{}", data_dir.display(), template_id);
    if let Ok(guard) = cache.lock() {
        if let Some(tpl) = guard.get(&key) {
            return Some(tpl.clone());
        }
    }
    let loaded = StructureTemplate::load_from_data_dir(data_dir, template_id).ok()?;
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, loaded.clone());
    }
    Some(loaded)
}

/// Exact match for vanilla `RandomSpreadStructurePlacement.getPotentialStructureChunk`
/// using `WorldgenRandom.setLargeFeatureWithSalt` (`LINEAR` spread).
pub fn potential_structure_chunk(
    seed: i64,
    chunk_x: i32,
    chunk_z: i32,
    spacing: i32,
    separation: i32,
    salt: i32,
) -> ChunkPos {
    if spacing <= 0 || spacing <= separation {
        return ChunkPos::new(chunk_x, chunk_z);
    }
    let spaced_grid_x = floor_div(chunk_x, spacing);
    let spaced_grid_z = floor_div(chunk_z, spacing);
    let feature_seed = (spaced_grid_x as i64)
        .wrapping_mul(341_873_128_712_i64)
        .wrapping_add((spaced_grid_z as i64).wrapping_mul(132_897_987_541_i64))
        .wrapping_add(seed)
        .wrapping_add(salt as i64);
    let mut rng = LegacyRandomSource::new(feature_seed);
    let limit = spacing - separation;
    let spread_x = rng.next_int_bounded(limit);
    let spread_z = rng.next_int_bounded(limit);
    ChunkPos::new(
        spaced_grid_x.wrapping_mul(spacing).wrapping_add(spread_x),
        spaced_grid_z.wrapping_mul(spacing).wrapping_add(spread_z),
    )
}

/// Returns `true` iff `(chunk_x, chunk_z)` is the exact `RandomSpreadStructurePlacement` start
/// chunk for `(spacing, separation, salt)` under world `seed`.
pub fn is_structure_start_chunk(
    seed: i64,
    chunk_x: i32,
    chunk_z: i32,
    spacing: i32,
    separation: i32,
    salt: i32,
) -> bool {
    if spacing <= 0 || spacing <= separation {
        return false;
    }
    let potential = potential_structure_chunk(seed, chunk_x, chunk_z, spacing, separation, salt);
    potential.x == chunk_x && potential.z == chunk_z
}

#[inline]
fn biome_matches(rule: &StructurePlacementRule, center_biome: &str) -> bool {
    if rule.allowed_biomes.is_empty() {
        return true;
    }
    let clean = center_biome
        .strip_prefix("minecraft:")
        .unwrap_or(center_biome);
    rule.allowed_biomes.iter().any(|&allowed| {
        allowed == center_biome || allowed.strip_prefix("minecraft:").unwrap_or(allowed) == clean
    })
}

fn rotated_footprint_bounds(
    size: (i32, i32, i32),
    rotation: StructureRotation,
) -> (i32, i32, i32, i32) {
    let mx = (size.0 - 1).max(0);
    let mz = (size.2 - 1).max(0);
    let corners = [(0, 0), (mx, 0), (0, mz), (mx, mz)];
    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_z = i32::MAX;
    let mut max_z = i32::MIN;
    for (cx, cz) in corners {
        let (rx, _, rz) = rotation.transform(cx, 0, cz);
        min_x = min_x.min(rx);
        max_x = max_x.max(rx);
        min_z = min_z.min(rz);
        max_z = max_z.max(rz);
    }
    (min_x, max_x, min_z, max_z)
}

/// Find solid non-fluid ground Y in an Overworld chunk (ignoring leaves and vegetation).
fn find_overworld_column_surface(chunk: &Chunk, lx: usize, lz: usize) -> Option<i32> {
    let min_y = chunk.min_y();
    let max_y = chunk.max_y();
    for wy in (min_y + 1..max_y).rev() {
        let st = chunk.get_block(lx, wy, lz);
        if st.is_air() {
            continue;
        }
        if st.is_fluid_block() {
            return None;
        }
        if st.props().is_motion_blocking_no_leaves() {
            return Some(wy);
        }
    }
    None
}

/// Compute a stable Overworld placement Y across the footprint inside `chunk`.
fn compute_overworld_surface_y(
    chunk: &Chunk,
    local_min_x: i32,
    local_max_x: i32,
    local_min_z: i32,
    local_max_z: i32,
) -> Option<i32> {
    let cx = ((local_min_x + local_max_x) / 2).clamp(0, 15) as usize;
    let cz = ((local_min_z + local_max_z) / 2).clamp(0, 15) as usize;
    let center_y = find_overworld_column_surface(chunk, cx, cz)?;

    let x0 = local_min_x.clamp(0, 15) as usize;
    let x1 = local_max_x.clamp(0, 15) as usize;
    let z0 = local_min_z.clamp(0, 15) as usize;
    let z1 = local_max_z.clamp(0, 15) as usize;

    let mut min_surf = center_y;
    for &(sx, sz) in &[(x0, z0), (x1, z0), (x0, z1), (x1, z1)] {
        if let Some(sy) = find_overworld_column_surface(chunk, sx, sz) {
            min_surf = min_surf.min(sy);
        }
    }
    Some(min_surf)
}

/// Find a Nether cavern floor above the lava sea (`y >= 32`) with open air headroom above it.
fn find_nether_cavern_floor(chunk: &Chunk, lx: usize, lz: usize, headroom: i32) -> Option<i32> {
    let min_y = chunk.min_y().max(32);
    let max_y = (chunk.max_y() - headroom - 6).min(115);
    if min_y >= max_y {
        return None;
    }

    for wy in min_y..=max_y {
        let floor = chunk.get_block(lx, wy, lz);
        if floor == BlockStateId::BEDROCK
            || floor.is_air()
            || floor.is_fluid_block()
            || !floor.props().is_solid()
        {
            continue;
        }
        let mut clear = true;
        for dy in 1..=headroom {
            if !chunk.get_block(lx, wy + dy, lz).is_air() {
                clear = false;
                break;
            }
        }
        if clear {
            return Some(wy);
        }
    }
    None
}

/// Single-chunk `BlockWorld` adapter used by `StructureTemplate::place`.
struct ChunkBlockWorld<'a> {
    chunk: &'a mut Chunk,
    chunk_min_x: i32,
    chunk_min_z: i32,
    min_y: i32,
    max_y: i32,
}

impl<'a> BlockWorld for ChunkBlockWorld<'a> {
    fn get_block_state(&mut self, pos: BlockPos) -> BlockStateId {
        let lx = pos.x - self.chunk_min_x;
        let lz = pos.z - self.chunk_min_z;
        if !(0..16).contains(&lx)
            || !(0..16).contains(&lz)
            || pos.y < self.min_y
            || pos.y >= self.max_y
        {
            return BlockStateId::AIR;
        }
        self.chunk.get_block(lx as usize, pos.y, lz as usize)
    }

    fn set_block_state(&mut self, pos: BlockPos, state: BlockStateId) -> bool {
        let lx = pos.x - self.chunk_min_x;
        let lz = pos.z - self.chunk_min_z;
        if !(0..16).contains(&lx)
            || !(0..16).contains(&lz)
            || pos.y < self.min_y
            || pos.y >= self.max_y
        {
            return false;
        }
        if state.block_name() == "minecraft:structure_block" {
            return false;
        }
        if self.chunk.get_block(lx as usize, pos.y, lz as usize) == BlockStateId::BEDROCK {
            return false;
        }
        let sec_idx = ((pos.y >> 4) - self.chunk.min_section_y) as usize;
        self.chunk.sections[sec_idx].set_block(
            lx as usize,
            (pos.y & 15) as usize,
            lz as usize,
            state,
        );
        true
    }
}

fn choose_template_and_rotation(
    rule: &StructurePlacementRule,
    seed: i64,
    pos: ChunkPos,
) -> (&'static str, StructureRotation) {
    let pick_seed = (pos.x as i64)
        .wrapping_mul(341_873_128_712_i64)
        .wrapping_add((pos.z as i64).wrapping_mul(132_897_987_541_i64))
        .wrapping_add(seed)
        .wrapping_add((rule.salt as i64).wrapping_mul(31));
    let mut rng = LegacyRandomSource::new(pick_seed);

    let rotation = match rng.next_int_bounded(4) {
        1 => StructureRotation::Clockwise90,
        2 => StructureRotation::Clockwise180,
        3 => StructureRotation::CounterClockwise90,
        _ => StructureRotation::None,
    };

    let variants = variants_for_rule(rule);
    let template_id = if variants.is_empty() {
        rule.template_id
    } else {
        variants[rng.next_int_bounded(variants.len() as i32) as usize]
    };

    (template_id, rotation)
}

fn place_template_in_chunk(
    chunk: &mut Chunk,
    template: &StructureTemplate,
    rotation: StructureRotation,
    origin_y: i32,
) -> usize {
    let chunk_min_x = chunk.pos.x << 4;
    let chunk_min_z = chunk.pos.z << 4;
    let min_y = chunk.min_y();
    let max_y = chunk.max_y();

    let (min_rx, max_rx, min_rz, max_rz) = rotated_footprint_bounds(template.size, rotation);
    let foot_x = max_rx - min_rx + 1;
    let foot_z = max_rz - min_rz + 1;
    let local_start_x = (16 - foot_x) / 2;
    let local_start_z = (16 - foot_z) / 2;

    let origin = BlockPos::new(
        chunk_min_x + local_start_x - min_rx,
        origin_y,
        chunk_min_z + local_start_z - min_rz,
    );

    let (placed, block_entities) = {
        let mut world = ChunkBlockWorld {
            chunk,
            chunk_min_x,
            chunk_min_z,
            min_y,
            max_y,
        };
        template.place(&mut world, origin, rotation)
    };

    let placed_count = placed.len();
    for be in block_entities {
        chunk.block_entities.insert(be.pos, be);
    }
    if placed_count > 0 {
        chunk.recalc_all_heightmaps();
    }
    placed_count
}

/// Evaluate vanilla 26.3 Overworld structure sets for `chunk` and place any matching `.nbt`
/// structure into `chunk`'s 16x16 footprint (including block entities).
///
/// Returns the total number of blocks placed into `chunk` (`0` if no structure spawned).
pub fn place_natural_overworld_structures(
    chunk: &mut Chunk,
    seed: i64,
    center_biome: &str,
    data_dir: &Path,
) -> usize {
    let pos = chunk.pos;
    for rule in OVERWORLD_STRUCTURE_RULES {
        if !biome_matches(rule, center_biome) {
            continue;
        }
        if !is_structure_start_chunk(
            seed,
            pos.x,
            pos.z,
            rule.spacing,
            rule.separation,
            rule.salt,
        ) {
            continue;
        }

        let (template_id, rotation) = choose_template_and_rotation(rule, seed, pos);
        let Some(template) = load_cached_template(data_dir, template_id) else {
            continue;
        };

        let (min_rx, max_rx, min_rz, max_rz) = rotated_footprint_bounds(template.size, rotation);
        let foot_x = max_rx - min_rx + 1;
        let foot_z = max_rz - min_rz + 1;
        let local_min_x = (16 - foot_x) / 2;
        let local_min_z = (16 - foot_z) / 2;

        let Some(surface_y) = compute_overworld_surface_y(
            chunk,
            local_min_x,
            local_min_x + foot_x - 1,
            local_min_z,
            local_min_z + foot_z - 1,
        ) else {
            continue;
        };

        let origin_y = (surface_y + rule.y_offset).max(chunk.min_y() + 1);
        if origin_y + template.size.1 >= chunk.max_y() {
            continue;
        }

        let placed = place_template_in_chunk(chunk, &template, rotation, origin_y);
        if placed > 0 {
            return placed;
        }
    }
    0
}

/// Evaluate vanilla 26.3 Nether structure sets (`nether_fossils`, `ruined_portal_nether`) for
/// `chunk` and place any matching `.nbt` structure on a solid cavern floor above lava level.
///
/// Returns the total number of blocks placed into `chunk` (`0` if no structure spawned).
pub fn place_natural_nether_structures(
    chunk: &mut Chunk,
    seed: i64,
    center_biome: &str,
    data_dir: &Path,
) -> usize {
    let pos = chunk.pos;
    for rule in NETHER_STRUCTURE_RULES {
        if !biome_matches(rule, center_biome) {
            continue;
        }
        if !is_structure_start_chunk(
            seed,
            pos.x,
            pos.z,
            rule.spacing,
            rule.separation,
            rule.salt,
        ) {
            continue;
        }

        let (template_id, rotation) = choose_template_and_rotation(rule, seed, pos);
        let Some(template) = load_cached_template(data_dir, template_id) else {
            continue;
        };

        let headroom = template.size.1.clamp(4, 14);
        let Some(floor_y) = find_nether_cavern_floor(chunk, 8, 8, headroom) else {
            continue;
        };

        let origin_y = (floor_y + rule.y_offset).max(chunk.min_y() + 5);
        if origin_y + template.size.1 >= chunk.max_y() - 5 {
            continue;
        }

        let placed = place_template_in_chunk(chunk, &template, rotation, origin_y);
        if placed > 0 {
            return placed;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_data::{TOTAL_BLOCK_STATES, default_state_id};
    use mc_oracle::OracleClient;
    use mc_world::{ChunkSection, PaletteStrategy, PalettedStorage};
    use std::collections::BTreeMap;

    fn repo_data_dir() -> std::path::PathBuf {
        OracleClient::repo_root()
            .join("src")
            .join("main")
            .join("resources")
            .join("data")
    }

    fn make_nether_cavern_chunk(pos: ChunkPos) -> Chunk {
        let block_strat = PaletteStrategy::for_block_states(TOTAL_BLOCK_STATES);
        let soul_sand = default_state_id("minecraft:soul_sand")
            .unwrap_or(BlockStateId::DIRT)
            .0;
        let netherrack = default_state_id("minecraft:netherrack")
            .unwrap_or(BlockStateId::STONE)
            .0;

        let mut sections = Vec::with_capacity(8);
        for sec_idx in 0..8 {
            let sec_y = sec_idx as i32;
            let mut flat = vec![BlockStateId::AIR.0; 4096];
            for ly in 0..16 {
                let wy = (sec_y << 4) + ly as i32;
                let st = if wy <= 2 || wy >= 125 {
                    BlockStateId::BEDROCK.0
                } else if wy < 38 {
                    netherrack
                } else if wy == 38 {
                    soul_sand
                } else if wy >= 100 {
                    netherrack
                } else {
                    BlockStateId::AIR.0
                };
                for lz in 0..16 {
                    for lx in 0..16 {
                        flat[ChunkSection::block_index(lx, ly, lz)] = st;
                    }
                }
            }
            let mut sec = ChunkSection {
                y: sec_y as i8,
                non_empty_block_count: 0,
                fluid_count: 0,
                block_states: PalettedStorage::from_flat(&flat, block_strat),
                biomes: PalettedStorage::single(0, 64),
                sky_light: None,
                block_light: None,
            };
            sec.recalc_counts();
            sections.push(sec);
        }

        let mut chunk = Chunk {
            pos,
            min_section_y: 0,
            status: "minecraft:full".into(),
            sections,
            heightmaps: BTreeMap::new(),
            extra_sky_light_below: None,
            extra_sky_light_above: None,
            extra_block_light_below: None,
            extra_block_light_above: None,
            block_entities: BTreeMap::new(),
            dirty: false,
        };
        chunk.recalc_all_heightmaps();
        chunk
    }

    #[test]
    fn random_spread_start_chunk_is_deterministic_and_unique_per_grid_cell() {
        let seed = 123_456_789_i64;
        let spacing = 32;
        let separation = 8;
        let salt = 14_357_618;

        for gx in -2..=2 {
            for gz in -2..=2 {
                let mut hits = Vec::new();
                for dx in 0..spacing {
                    for dz in 0..spacing {
                        let cx = gx * spacing + dx;
                        let cz = gz * spacing + dz;
                        if is_structure_start_chunk(seed, cx, cz, spacing, separation, salt) {
                            assert!(dx < spacing - separation);
                            assert!(dz < spacing - separation);
                            hits.push((cx, cz));
                        }
                    }
                }
                assert_eq!(
                    hits.len(),
                    1,
                    "Expected exactly 1 start chunk in grid ({gx},{gz}), got {hits:?}"
                );
                let (hx, hz) = hits[0];
                assert!(is_structure_start_chunk(seed, hx, hz, spacing, separation, salt));
            }
        }
    }

    #[test]
    fn overworld_structure_places_real_vanilla_igloo_and_village_house_nbt() {
        let data_dir = repo_data_dir();
        let seed = 263_2026_i64;

        // 1. Real vanilla igloo/top.nbt placed in snowy_plains
        let igloo_pos = potential_structure_chunk(seed, 0, 0, 32, 8, 14_357_618);
        assert!(is_structure_start_chunk(
            seed,
            igloo_pos.x,
            igloo_pos.z,
            32,
            8,
            14_357_618
        ));
        let mut igloo_chunk = Chunk::new_flat_overworld(igloo_pos, 0);
        let placed = place_natural_overworld_structures(
            &mut igloo_chunk,
            seed,
            "minecraft:snowy_plains",
            &data_dir,
        );
        assert!(placed > 100, "Expected >100 igloo blocks placed, got {placed}");

        let snow_block = default_state_id("minecraft:snow_block").unwrap();
        let mut snow_count = 0usize;
        for lz in 0..16 {
            for lx in 0..16 {
                assert_eq!(igloo_chunk.get_block(lx, -64, lz), BlockStateId::BEDROCK);
                for wy in -63..-40 {
                    if igloo_chunk.get_block(lx, wy, lz) == snow_block {
                        snow_count += 1;
                    }
                }
            }
        }
        assert!(snow_count > 20, "Expected real igloo snow_block walls, got {snow_count}");

        // 2. Real vanilla plains village house placed in plains
        let village_pos = potential_structure_chunk(seed, 0, 0, 34, 8, 10_387_312);
        let mut village_chunk = Chunk::new_flat_overworld(village_pos, 0);
        let v_placed = place_natural_overworld_structures(
            &mut village_chunk,
            seed,
            "minecraft:plains",
            &data_dir,
        );
        assert!(v_placed > 50, "Expected >50 plains village blocks placed, got {v_placed}");
    }

    #[test]
    fn nether_structure_places_real_vanilla_nether_fossil_nbt() {
        let data_dir = repo_data_dir();
        let seed = 263_2026_i64;

        // Pick an even chunk that is a nether_fossil start (spacing=2, separation=1)
        // and not a ruined_portal_nether start.
        let mut fossil_pos = ChunkPos::new(0, 0);
        for k in 0..10 {
            let candidate = ChunkPos::new(k * 2, 0);
            if is_structure_start_chunk(seed, candidate.x, candidate.z, 2, 1, 14_357_921)
                && !is_structure_start_chunk(seed, candidate.x, candidate.z, 40, 15, 34_222_645)
            {
                fossil_pos = candidate;
                break;
            }
        }

        let mut chunk = make_nether_cavern_chunk(fossil_pos);
        let placed = place_natural_nether_structures(
            &mut chunk,
            seed,
            "minecraft:soul_sand_valley",
            &data_dir,
        );
        assert!(placed > 10, "Expected >10 nether fossil blocks placed, got {placed}");

        let bone_block = default_state_id("minecraft:bone_block").unwrap();
        let mut bone_count = 0usize;
        for wy in 38..80 {
            for lz in 0..16 {
                for lx in 0..16 {
                    if chunk.get_block(lx, wy, lz).block_name() == bone_block.block_name() {
                        bone_count += 1;
                    }
                }
            }
        }
        assert!(bone_count > 5, "Expected real bone_block ribs from nether_fossils/*.nbt, got {bone_count}");
    }
}
