use crate::BlockWorld;
use mc_data::BlockStateId;
use mc_data::block_behaviors::{KIND_LAVA, block_behavior};
use mc_data::block_extra::{
    CACTUS_STATE_ID, DIRT_STATE_ID, FIRE_BASE_STATES, GRASS_BLOCK_STATE_ID,
    NETHER_PORTAL_X_STATE_ID, NETHER_PORTAL_Z_STATE_ID, OBSIDIAN_STATE_ID, SUGAR_CANE_STATE_ID,
    TREE_LEAVES_STATES, TREE_LOG_STATES, cactus_age, cactus_with_age, crop_is_mature, crop_kind,
    crop_next_age, farmland_moisture, farmland_with_moisture, fire_age, fire_odds, fire_with_age,
    is_fire, is_flammable, is_grass_block, is_infiniburn, is_leaves, is_log_or_wood,
    is_nether_portal, is_persistent_leaves, is_sapling, pitcher_crop_upper_for_lower,
    sapling_next_stage, sugar_cane_age, sugar_cane_with_age,
};
use mc_math::BlockPos;
use mc_math::rng::{LegacyRandomSource, RandomSource, mix_stafford13};

const CARDINAL_DIRS: [(i32, i32, i32); 6] = [
    (0, -1, 0),
    (0, 1, 0),
    (0, 0, -1),
    (0, 0, 1),
    (-1, 0, 0),
    (1, 0, 0),
];

const PORTAL_MIN_WIDTH: i32 = 2;
const PORTAL_MAX_WIDTH: i32 = 21;
const PORTAL_MIN_HEIGHT: i32 = 3;
const PORTAL_MAX_HEIGHT: i32 = 21;

/// Runs vanilla-style random block ticks across all 16-block vertical sections of chunk `(chunk_x, chunk_z)`
/// within `[min_y, max_y)`, sampling `random_tick_speed` blocks per section using Minecraft's LCG coordinate step.
///
/// Returns every `(BlockPos, new_state)` modified during this tick pass.
pub fn run_chunk_random_ticks<W: BlockWorld>(
    world: &mut W,
    chunk_x: i32,
    chunk_z: i32,
    min_y: i32,
    max_y: i32,
    random_tick_speed: u32,
    seed: u64,
) -> Vec<(BlockPos, BlockStateId)> {
    let mut changes = Vec::new();
    if random_tick_speed == 0 || min_y >= max_y {
        return changes;
    }

    let min_x = chunk_x << 4;
    let min_z = chunk_z << 4;
    let mixed = mix_stafford13(seed);
    let mut rand_value: i32 = (mixed as i32) ^ ((mixed >> 32) as i32);
    let mut rng = LegacyRandomSource::new(mixed as i64);

    let min_section = min_y.div_euclid(16);
    let max_section = (max_y - 1).div_euclid(16);

    for section_y in min_section..=max_section {
        let sec_min_y = section_y * 16;
        for _ in 0..random_tick_speed {
            rand_value = rand_value.wrapping_mul(3).wrapping_add(1_013_904_223);
            let val = rand_value >> 2;
            let lx = val & 15;
            let ly = (val >> 16) & 15;
            let lz = (val >> 8) & 15;
            let pos = BlockPos::new(min_x + lx, sec_min_y + ly, min_z + lz);
            if pos.y < min_y || pos.y >= max_y {
                continue;
            }
            tick_block_random(world, pos, &mut rng, &mut changes);
        }
    }

    changes
}

fn tick_block_random<W: BlockWorld>(
    world: &mut W,
    pos: BlockPos,
    rng: &mut LegacyRandomSource,
    changes: &mut Vec<(BlockPos, BlockStateId)>,
) {
    let state = world.get_block_state(pos);
    if state.is_air() {
        return;
    }

    // 1. Grass block spread and death under solid opaque blocks
    if is_grass_block(state.0) {
        tick_grass_block(world, pos, rng, changes);
        return;
    }

    // 2. Farmland moisture hydration & drying
    if let Some(moisture) = farmland_moisture(state.0) {
        tick_farmland(world, pos, state, moisture, changes);
        return;
    }

    // 3. Crops (wheat, carrots, potatoes, beetroots, nether wart, torchflower, pitcher, sweet berry, cocoa, bamboo sapling)
    if let Some(next_id) = crop_next_age(state.0) {
        tick_crop(world, pos, state, next_id, rng, changes);
        return;
    }

    // 4. Sugar cane vertical growth up to height 3
    if let Some(age) = sugar_cane_age(state.0) {
        tick_sugar_cane(world, pos, state, age, changes);
        return;
    }

    // 5. Cactus vertical growth up to height 3
    if let Some(age) = cactus_age(state.0) {
        tick_cactus(world, pos, state, age, changes);
        return;
    }

    // 6. Sapling stage progression & miniature vanilla tree growth
    if let Some(species) = is_sapling(state.0) {
        tick_sapling(world, pos, state, species, rng, changes);
        return;
    }

    // 7. Non-persistent leaf decay when no log/wood within Manhattan distance 6
    if is_leaves(state.0) && !is_persistent_leaves(state.0) {
        tick_leaf_decay(world, pos, changes);
        return;
    }

    // 8. Fire spread to flammable neighbors & burnout
    if let Some(age) = fire_age(state.0) {
        tick_fire(world, pos, state, age, rng, changes);
    }
}

#[inline]
fn can_grass_survive(above: BlockStateId) -> bool {
    let props = above.props();
    if props.has_fluid() {
        return false;
    }
    !(props.light_dampening >= 15 && props.can_occlude())
}

fn tick_grass_block<W: BlockWorld>(
    world: &mut W,
    pos: BlockPos,
    rng: &mut LegacyRandomSource,
    changes: &mut Vec<(BlockPos, BlockStateId)>,
) {
    let above = world.get_block_state(pos.offset(0, 1, 0));
    if !can_grass_survive(above) {
        let dirt = BlockStateId(DIRT_STATE_ID);
        world.set_block_state(pos, dirt);
        changes.push((pos, dirt));
        return;
    }

    let grass = BlockStateId(GRASS_BLOCK_STATE_ID);
    for _ in 0..4 {
        let dx = rng.next_int_bounded(3) - 1;
        let dy = rng.next_int_bounded(5) - 3;
        let dz = rng.next_int_bounded(3) - 1;
        let target = pos.offset(dx, dy, dz);
        if world.get_block_state(target).0 == DIRT_STATE_ID {
            let target_above = world.get_block_state(target.offset(0, 1, 0));
            if can_grass_survive(target_above) {
                world.set_block_state(target, grass);
                changes.push((target, grass));
            }
        }
    }
}

#[inline]
fn is_water_block_or_waterlogged(state: BlockStateId) -> bool {
    state.props().has_fluid() && block_behavior(state.0).kind != KIND_LAVA
}

fn is_near_water<W: BlockWorld>(world: &mut W, pos: BlockPos) -> bool {
    for dy in 0..=1 {
        for dx in -4..=4 {
            for dz in -4..=4 {
                if is_water_block_or_waterlogged(world.get_block_state(pos.offset(dx, dy, dz))) {
                    return true;
                }
            }
        }
    }
    false
}

fn tick_farmland<W: BlockWorld>(
    world: &mut W,
    pos: BlockPos,
    state: BlockStateId,
    moisture: u8,
    changes: &mut Vec<(BlockPos, BlockStateId)>,
) {
    if is_near_water(world, pos) {
        if moisture < 7 {
            if let Some(wet_id) = farmland_with_moisture(state.0, 7) {
                let wet = BlockStateId(wet_id);
                world.set_block_state(pos, wet);
                changes.push((pos, wet));
            }
        }
    } else if moisture > 0 {
        if let Some(drier_id) = farmland_with_moisture(state.0, moisture - 1) {
            let drier = BlockStateId(drier_id);
            world.set_block_state(pos, drier);
            changes.push((pos, drier));
        }
    } else {
        let above = world.get_block_state(pos.offset(0, 1, 0));
        let maintains = crop_next_age(above.0).is_some() || crop_is_mature(above.0);
        if !maintains {
            let dirt = BlockStateId(DIRT_STATE_ID);
            world.set_block_state(pos, dirt);
            changes.push((pos, dirt));
        }
    }
}

fn farmland_crop_growth_speed<W: BlockWorld>(
    world: &mut W,
    pos: BlockPos,
    crop_name: &str,
) -> f32 {
    let mut speed = 1.0_f32;
    let below = pos.offset(0, -1, 0);

    for dx in -1..=1 {
        for dz in -1..=1 {
            let mut tile_speed = 0.0_f32;
            let soil = world.get_block_state(below.offset(dx, 0, dz));
            if let Some(moisture) = farmland_moisture(soil.0) {
                tile_speed = if moisture > 0 { 3.0_f32 } else { 1.0_f32 };
            }
            if dx != 0 || dz != 0 {
                tile_speed /= 4.0_f32;
            }
            speed += tile_speed;
        }
    }

    let same = |w: &mut W, p: BlockPos| w.get_block_state(p).block_name() == crop_name;
    let horiz = same(world, pos.offset(-1, 0, 0)) || same(world, pos.offset(1, 0, 0));
    let vert = same(world, pos.offset(0, 0, -1)) || same(world, pos.offset(0, 0, 1));
    if horiz && vert {
        speed /= 2.0_f32;
    } else {
        let diag = same(world, pos.offset(-1, 0, -1))
            || same(world, pos.offset(1, 0, -1))
            || same(world, pos.offset(1, 0, 1))
            || same(world, pos.offset(-1, 0, 1));
        if diag {
            speed /= 2.0_f32;
        }
    }

    speed
}

fn tick_crop<W: BlockWorld>(
    world: &mut W,
    pos: BlockPos,
    state: BlockStateId,
    next_id: u32,
    rng: &mut LegacyRandomSource,
    changes: &mut Vec<(BlockPos, BlockStateId)>,
) {
    let kind = crop_kind(state.0);
    let should_grow = match kind {
        1 => {
            let speed = farmland_crop_growth_speed(world, pos, state.block_name());
            let bound = ((25.0_f32 / speed) as i32) + 1;
            rng.next_int_bounded(bound.max(1)) == 0
        }
        2 => rng.next_int_bounded(10) == 0,
        3 => rng.next_int_bounded(5) == 0,
        4 => world.get_block_state(pos.offset(0, 1, 0)).is_air() && rng.next_int_bounded(3) == 0,
        _ => false,
    };

    if !should_grow {
        return;
    }

    if let Some(upper_id) = pitcher_crop_upper_for_lower(next_id) {
        let above_pos = pos.offset(0, 1, 0);
        let above = world.get_block_state(above_pos);
        if !above.is_air() && above.block_name() != "minecraft:pitcher_crop" {
            return;
        }
        let next_state = BlockStateId(next_id);
        let upper_state = BlockStateId(upper_id);
        world.set_block_state(pos, next_state);
        world.set_block_state(above_pos, upper_state);
        changes.push((pos, next_state));
        changes.push((above_pos, upper_state));
    } else {
        let next_state = BlockStateId(next_id);
        world.set_block_state(pos, next_state);
        changes.push((pos, next_state));
    }
}

fn tick_sugar_cane<W: BlockWorld>(
    world: &mut W,
    pos: BlockPos,
    state: BlockStateId,
    age: u8,
    changes: &mut Vec<(BlockPos, BlockStateId)>,
) {
    let above_pos = pos.offset(0, 1, 0);
    if !world.get_block_state(above_pos).is_air() {
        return;
    }

    let mut height = 1;
    while sugar_cane_age(world.get_block_state(pos.offset(0, -height, 0)).0).is_some() {
        height += 1;
        if height >= 3 {
            return;
        }
    }

    if age == 15 {
        let base = BlockStateId(SUGAR_CANE_STATE_ID);
        world.set_block_state(above_pos, base);
        world.set_block_state(pos, base);
        changes.push((above_pos, base));
        changes.push((pos, base));
    } else if let Some(next_id) = sugar_cane_with_age(state.0, age + 1) {
        let next = BlockStateId(next_id);
        world.set_block_state(pos, next);
        changes.push((pos, next));
    }
}

fn tick_cactus<W: BlockWorld>(
    world: &mut W,
    pos: BlockPos,
    state: BlockStateId,
    age: u8,
    changes: &mut Vec<(BlockPos, BlockStateId)>,
) {
    let above_pos = pos.offset(0, 1, 0);
    if !world.get_block_state(above_pos).is_air() {
        return;
    }

    let mut height = 1;
    while cactus_age(world.get_block_state(pos.offset(0, -height, 0)).0).is_some() {
        height += 1;
        if height >= 3 {
            return;
        }
    }

    if age == 15 {
        let base = BlockStateId(CACTUS_STATE_ID);
        world.set_block_state(above_pos, base);
        world.set_block_state(pos, base);
        changes.push((above_pos, base));
        changes.push((pos, base));
    } else if let Some(next_id) = cactus_with_age(state.0, age + 1) {
        let next = BlockStateId(next_id);
        world.set_block_state(pos, next);
        changes.push((pos, next));
    }
}

fn tick_sapling<W: BlockWorld>(
    world: &mut W,
    pos: BlockPos,
    state: BlockStateId,
    species: u8,
    rng: &mut LegacyRandomSource,
    changes: &mut Vec<(BlockPos, BlockStateId)>,
) {
    if rng.next_int_bounded(7) != 0 {
        return;
    }

    if let Some(stage1_id) = sapling_next_stage(state.0) {
        let stage1 = BlockStateId(stage1_id);
        world.set_block_state(pos, stage1);
        changes.push((pos, stage1));
        return;
    }

    let idx = (species as usize).min(TREE_LOG_STATES.len() - 1);
    let log_state = BlockStateId(TREE_LOG_STATES[idx]);
    let leaves_state = BlockStateId(TREE_LEAVES_STATES[idx]);

    let trunk_height = match species {
        1 => 5 + rng.next_int_bounded(2), // spruce: 5..=6
        2 => 5 + rng.next_int_bounded(3), // birch: 5..=7
        _ => 4 + rng.next_int_bounded(3), // oak & others: 4..=6
    };

    // Require clear vertical column above the sapling
    for dy in 1..=(trunk_height + 1) {
        let st = world.get_block_state(pos.offset(0, dy, 0));
        if !st.is_air() && !is_leaves(st.0) {
            return;
        }
    }

    // Turn grass directly beneath the trunk into dirt (matching vanilla TreeFeature)
    let below_pos = pos.offset(0, -1, 0);
    if is_grass_block(world.get_block_state(below_pos).0) {
        let dirt = BlockStateId(DIRT_STATE_ID);
        world.set_block_state(below_pos, dirt);
        changes.push((below_pos, dirt));
    }

    // Place vertical trunk
    for dy in 0..trunk_height {
        let p = pos.offset(0, dy, 0);
        world.set_block_state(p, log_state);
        changes.push((p, log_state));
    }

    // Place species canopy (only into air blocks)
    if species == 1 {
        // Conical spruce foliage
        let layers: [(i32, i32, bool); 4] = [
            (trunk_height - 3, 2, true),
            (trunk_height - 2, 1, false),
            (trunk_height - 1, 1, true),
            (trunk_height, 0, false),
        ];
        for (dy, radius, skip_corners) in layers {
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    if skip_corners && radius > 0 && dx.abs() == radius && dz.abs() == radius {
                        continue;
                    }
                    let lp = pos.offset(dx, dy, dz);
                    if world.get_block_state(lp).is_air() {
                        world.set_block_state(lp, leaves_state);
                        changes.push((lp, leaves_state));
                    }
                }
            }
        }
    } else {
        // Classic oak/birch blob canopy: two 5x5 layers, one 3x3 layer, and a top cross
        for dy in (trunk_height - 3)..=trunk_height {
            let top_offset = dy - trunk_height;
            let radius = 1 - top_offset / 2;
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    if dx.abs() == radius
                        && dz.abs() == radius
                        && (top_offset == 0 || rng.next_int_bounded(2) == 0)
                    {
                        continue;
                    }
                    let lp = pos.offset(dx, dy, dz);
                    if world.get_block_state(lp).is_air() {
                        world.set_block_state(lp, leaves_state);
                        changes.push((lp, leaves_state));
                    }
                }
            }
        }
    }
}

fn has_log_within_manhattan_6<W: BlockWorld>(world: &mut W, origin: BlockPos) -> bool {
    for dist in 1..=6_i32 {
        for dx in -dist..=dist {
            let rem_yz = dist - dx.abs();
            for dy in -rem_yz..=rem_yz {
                let dz_abs = rem_yz - dy.abs();
                if is_log_or_wood(world.get_block_state(origin.offset(dx, dy, dz_abs)).0) {
                    return true;
                }
                if dz_abs != 0
                    && is_log_or_wood(world.get_block_state(origin.offset(dx, dy, -dz_abs)).0)
                {
                    return true;
                }
            }
        }
    }
    false
}

fn tick_leaf_decay<W: BlockWorld>(
    world: &mut W,
    pos: BlockPos,
    changes: &mut Vec<(BlockPos, BlockStateId)>,
) {
    if !has_log_within_manhattan_6(world, pos) {
        world.set_block_state(pos, BlockStateId::AIR);
        changes.push((pos, BlockStateId::AIR));
    }
}

fn has_flammable_neighbor<W: BlockWorld>(world: &mut W, pos: BlockPos) -> bool {
    for (dx, dy, dz) in CARDINAL_DIRS {
        if is_flammable(world.get_block_state(pos.offset(dx, dy, dz)).0) {
            return true;
        }
    }
    false
}

fn max_neighbor_ignite_odds<W: BlockWorld>(world: &mut W, pos: BlockPos) -> u8 {
    let mut best = 0_u8;
    for (dx, dy, dz) in CARDINAL_DIRS {
        let (ignite, _) = fire_odds(world.get_block_state(pos.offset(dx, dy, dz)).0);
        if ignite > best {
            best = ignite;
        }
    }
    best
}

fn tick_fire<W: BlockWorld>(
    world: &mut W,
    pos: BlockPos,
    state: BlockStateId,
    age: u8,
    rng: &mut LegacyRandomSource,
    changes: &mut Vec<(BlockPos, BlockStateId)>,
) {
    let below = world.get_block_state(pos.offset(0, -1, 0));
    let below_solid = below.props().can_occlude() && below.props().is_solid();
    let flammable_adj = has_flammable_neighbor(world, pos);

    if !below_solid && !flammable_adj {
        world.set_block_state(pos, BlockStateId::AIR);
        changes.push((pos, BlockStateId::AIR));
        return;
    }

    let infiniburn = is_infiniburn(below.0);
    let new_age = (age + (rng.next_int_bounded(3) / 2) as u8).min(15);
    if new_age != age {
        if let Some(aged_id) = fire_with_age(state.0, new_age) {
            let aged = BlockStateId(aged_id);
            world.set_block_state(pos, aged);
            changes.push((pos, aged));
        }
    }

    if !infiniburn {
        if !flammable_adj {
            if !below_solid || new_age > 3 {
                world.set_block_state(pos, BlockStateId::AIR);
                changes.push((pos, BlockStateId::AIR));
            }
            return;
        }
        if new_age == 15 && rng.next_int_bounded(4) == 0 && !is_flammable(below.0) {
            world.set_block_state(pos, BlockStateId::AIR);
            changes.push((pos, BlockStateId::AIR));
            return;
        }
    }

    // Consume adjacent flammable blocks
    let burn_checks: [(i32, i32, i32, i32); 6] = [
        (1, 0, 0, 300),
        (-1, 0, 0, 300),
        (0, -1, 0, 250),
        (0, 1, 0, 250),
        (0, 0, -1, 300),
        (0, 0, 1, 300),
    ];
    for (dx, dy, dz, chance) in burn_checks {
        let np = pos.offset(dx, dy, dz);
        let (_, burn_odds) = fire_odds(world.get_block_state(np).0);
        if burn_odds > 0 && rng.next_int_bounded(chance) < burn_odds as i32 {
            if rng.next_int_bounded(new_age as i32 + 10) < 5 {
                let spread_age = (new_age + (rng.next_int_bounded(5) / 4) as u8).min(15);
                let fire_st = BlockStateId(FIRE_BASE_STATES[spread_age as usize]);
                world.set_block_state(np, fire_st);
                changes.push((np, fire_st));
            } else {
                world.set_block_state(np, BlockStateId::AIR);
                changes.push((np, BlockStateId::AIR));
            }
        }
    }

    // Spread fire to nearby air blocks adjacent to flammable blocks
    for xx in -1..=1_i32 {
        for zz in -1..=1_i32 {
            for yy in -1..=4_i32 {
                if xx == 0 && yy == 0 && zz == 0 {
                    continue;
                }
                let mut rate = 100_i32;
                if yy > 1 {
                    rate += (yy - 1) * 100;
                }
                let test_pos = pos.offset(xx, yy, zz);
                if !world.get_block_state(test_pos).is_air() {
                    continue;
                }
                let ignite = max_neighbor_ignite_odds(world, test_pos);
                if ignite > 0 {
                    let odds = (ignite as i32 + 40 + 14) / (new_age as i32 + 30);
                    if odds > 0 && rng.next_int_bounded(rate) <= odds {
                        let spread_age =
                            (new_age + (rng.next_int_bounded(5) / 4) as u8).min(15);
                        let fire_st = BlockStateId(FIRE_BASE_STATES[spread_age as usize]);
                        world.set_block_state(test_pos, fire_st);
                        changes.push((test_pos, fire_st));
                    }
                }
            }
        }
    }
}

#[inline]
fn is_portal_empty(state: BlockStateId) -> bool {
    state.is_air() || is_fire(state.0) || is_nether_portal(state.0)
}

#[inline]
fn is_portal_frame(state: BlockStateId) -> bool {
    state.0 == OBSIDIAN_STATE_ID
}

fn distance_until_edge_above_frame<W: BlockWorld>(
    world: &mut W,
    origin: BlockPos,
    dx: i32,
    dz: i32,
) -> i32 {
    for width in 0..=PORTAL_MAX_WIDTH {
        let p = origin.offset(dx * width, 0, dz * width);
        let st = world.get_block_state(p);
        if !is_portal_empty(st) {
            if is_portal_frame(st) {
                return width;
            }
            return 0;
        }
        let below = world.get_block_state(p.offset(0, -1, 0));
        if !is_portal_frame(below) {
            return 0;
        }
    }
    0
}

fn try_ignite_portal_axis<W: BlockWorld>(
    world: &mut W,
    start: BlockPos,
    axis_x: bool,
) -> Option<Vec<(BlockPos, BlockStateId)>> {
    if !is_portal_empty(world.get_block_state(start)) {
        return None;
    }

    // Right direction along the portal plane: X-axis uses (-1, 0), Z-axis uses (0, 1)
    let (rx, rz) = if axis_x { (-1, 0) } else { (0, 1) };

    // Drop to the lowest interior block above the bottom obsidian bar
    let mut base = start;
    let min_scan_y = start.y - PORTAL_MAX_HEIGHT;
    while base.y > min_scan_y && is_portal_empty(world.get_block_state(base.offset(0, -1, 0))) {
        base = base.offset(0, -1, 0);
    }

    // Walk left (-right) to locate the bottom-left interior corner
    let dist_left = distance_until_edge_above_frame(world, base, -rx, -rz);
    if dist_left <= 0 {
        return None;
    }
    let bottom_left = base.offset(-rx * (dist_left - 1), 0, -rz * (dist_left - 1));

    // Measure interior width along +right (also validates bottom obsidian bar & right pillar base)
    let width = distance_until_edge_above_frame(world, bottom_left, rx, rz);
    if !(PORTAL_MIN_WIDTH..=PORTAL_MAX_WIDTH).contains(&width) {
        return None;
    }

    // Measure interior height while checking left & right obsidian pillars and empty interior
    let mut height = 0_i32;
    let mut existing_portal_blocks = 0_i32;
    'height_loop: while height < PORTAL_MAX_HEIGHT {
        let left_frame = bottom_left.offset(-rx, height, -rz);
        if !is_portal_frame(world.get_block_state(left_frame)) {
            break;
        }
        let right_frame = bottom_left.offset(rx * width, height, rz * width);
        if !is_portal_frame(world.get_block_state(right_frame)) {
            break;
        }
        for i in 0..width {
            let p = bottom_left.offset(rx * i, height, rz * i);
            let st = world.get_block_state(p);
            if !is_portal_empty(st) {
                break 'height_loop;
            }
            if is_nether_portal(st.0) {
                existing_portal_blocks += 1;
            }
        }
        height += 1;
    }

    if !(PORTAL_MIN_HEIGHT..=PORTAL_MAX_HEIGHT).contains(&height) {
        return None;
    }
    if existing_portal_blocks != 0 {
        return None;
    }

    // Validate top obsidian bar across the full width
    for i in 0..width {
        let top_frame = bottom_left.offset(rx * i, height, rz * i);
        if !is_portal_frame(world.get_block_state(top_frame)) {
            return None;
        }
    }

    // Fill interior with oriented nether portal blocks
    let portal_state = BlockStateId(if axis_x {
        NETHER_PORTAL_X_STATE_ID
    } else {
        NETHER_PORTAL_Z_STATE_ID
    });
    let mut placed = Vec::with_capacity((width * height) as usize);
    for dy in 0..height {
        for i in 0..width {
            let p = bottom_left.offset(rx * i, dy, rz * i);
            world.set_block_state(p, portal_state);
            placed.push((p, portal_state));
        }
    }

    Some(placed)
}

/// Validates whether `start` lies inside a complete obsidian Nether portal frame (interior 2..=21 wide
/// by 3..=21 tall, oriented along either X or Z) and fills the interior with `minecraft:nether_portal`.
pub fn try_ignite_nether_portal<W: BlockWorld>(
    world: &mut W,
    start: BlockPos,
) -> Option<Vec<(BlockPos, BlockStateId)>> {
    if let Some(placed) = try_ignite_portal_axis(world, start, true) {
        return Some(placed);
    }
    try_ignite_portal_axis(world, start, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_data::block_behaviors::WATER_STATES;
    use mc_data::block_extra::{FARMLAND_DRY_STATE_ID, FIRE_STATE_ID};
    use mc_data::default_state_id;
    use std::collections::HashMap;

    struct MapWorld(HashMap<BlockPos, BlockStateId>);

    impl MapWorld {
        fn new() -> Self {
            Self(HashMap::new())
        }
    }

    impl BlockWorld for MapWorld {
        fn get_block_state(&mut self, pos: BlockPos) -> BlockStateId {
            self.0.get(&pos).copied().unwrap_or(BlockStateId::AIR)
        }

        fn set_block_state(&mut self, pos: BlockPos, state: BlockStateId) -> bool {
            if state.is_air() {
                self.0.remove(&pos);
            } else {
                self.0.insert(pos, state);
            }
            true
        }
    }

    fn build_portal_frame_x(
        world: &mut MapWorld,
        x0: i32,
        y0: i32,
        z: i32,
        outer_w: i32,
        outer_h: i32,
    ) {
        let obs = BlockStateId(OBSIDIAN_STATE_ID);
        for x in x0..(x0 + outer_w) {
            world.set_block_state(BlockPos::new(x, y0, z), obs);
            world.set_block_state(BlockPos::new(x, y0 + outer_h - 1, z), obs);
        }
        for y in y0..(y0 + outer_h) {
            world.set_block_state(BlockPos::new(x0, y, z), obs);
            world.set_block_state(BlockPos::new(x0 + outer_w - 1, y, z), obs);
        }
    }

    fn build_portal_frame_z(
        world: &mut MapWorld,
        x: i32,
        y0: i32,
        z0: i32,
        outer_w: i32,
        outer_h: i32,
    ) {
        let obs = BlockStateId(OBSIDIAN_STATE_ID);
        for z in z0..(z0 + outer_w) {
            world.set_block_state(BlockPos::new(x, y0, z), obs);
            world.set_block_state(BlockPos::new(x, y0 + outer_h - 1, z), obs);
        }
        for y in y0..(y0 + outer_h) {
            world.set_block_state(BlockPos::new(x, y, z0), obs);
            world.set_block_state(BlockPos::new(x, y, z0 + outer_w - 1), obs);
        }
    }

    #[test]
    fn nether_portal_4x5_x_axis_ignites() {
        let mut world = MapWorld::new();
        build_portal_frame_x(&mut world, 0, 64, 0, 4, 5);
        // Simulate flint & steel fire placed inside before ignition
        world.set_block_state(BlockPos::new(1, 65, 0), BlockStateId(FIRE_STATE_ID));

        let placed = try_ignite_nether_portal(&mut world, BlockPos::new(1, 65, 0))
            .expect("4x5 X-axis portal should ignite");
        assert_eq!(placed.len(), 2 * 3);
        for x in 1..=2 {
            for y in 65..=67 {
                assert_eq!(
                    world.get_block_state(BlockPos::new(x, y, 0)),
                    BlockStateId(NETHER_PORTAL_X_STATE_ID)
                );
            }
        }
    }

    #[test]
    fn nether_portal_5x6_z_axis_ignites() {
        let mut world = MapWorld::new();
        build_portal_frame_z(&mut world, 10, 64, 0, 5, 6);

        let placed = try_ignite_nether_portal(&mut world, BlockPos::new(10, 67, 2))
            .expect("5x6 Z-axis portal should ignite from mid-interior");
        assert_eq!(placed.len(), 3 * 4);
        for z in 1..=3 {
            for y in 65..=68 {
                assert_eq!(
                    world.get_block_state(BlockPos::new(10, y, z)),
                    BlockStateId(NETHER_PORTAL_Z_STATE_ID)
                );
            }
        }
    }

    #[test]
    fn nether_portal_rejects_broken_frame() {
        let mut world = MapWorld::new();
        build_portal_frame_x(&mut world, 0, 64, 0, 4, 5);
        // Remove one top obsidian block
        world.set_block_state(BlockPos::new(2, 68, 0), BlockStateId::AIR);

        assert!(
            try_ignite_nether_portal(&mut world, BlockPos::new(1, 65, 0)).is_none(),
            "Broken portal frame must not ignite"
        );
        assert!(world.get_block_state(BlockPos::new(1, 65, 0)).is_air());
    }

    #[test]
    fn grass_spreads_to_dirt_and_dies_under_stone() {
        let mut world = MapWorld::new();
        // Patch of dirt surrounding grass at (8, 64, 8)
        for x in 6..=10 {
            for z in 6..=10 {
                world.set_block_state(BlockPos::new(x, 64, z), BlockStateId(DIRT_STATE_ID));
            }
        }
        world.set_block_state(BlockPos::new(8, 64, 8), BlockStateId(GRASS_BLOCK_STATE_ID));

        // Suffocated grass block at (2, 64, 2) with solid stone directly above it
        let buried = BlockPos::new(2, 64, 2);
        world.set_block_state(buried, BlockStateId(GRASS_BLOCK_STATE_ID));
        world.set_block_state(buried.offset(0, 1, 0), BlockStateId::STONE);

        for tick in 0..400_u64 {
            run_chunk_random_ticks(&mut world, 0, 0, 64, 80, 64, tick + 1);
        }

        assert_eq!(
            world.get_block_state(buried),
            BlockStateId(DIRT_STATE_ID),
            "Grass under solid opaque stone should decay into dirt"
        );

        let mut grass_count = 0;
        for x in 6..=10 {
            for z in 6..=10 {
                if world.get_block_state(BlockPos::new(x, 64, z)).0 == GRASS_BLOCK_STATE_ID {
                    grass_count += 1;
                }
            }
        }
        assert!(
            grass_count > 1,
            "Grass should spread onto nearby sunlit dirt (got {grass_count})"
        );
    }

    #[test]
    fn farmland_hydrates_near_water_and_dries_without_water() {
        let mut world = MapWorld::new();
        let wet_farm_pos = BlockPos::new(4, 64, 4);
        let dry_farm_pos = BlockPos::new(14, 64, 14);
        world.set_block_state(wet_farm_pos, BlockStateId(FARMLAND_DRY_STATE_ID));
        world.set_block_state(BlockPos::new(7, 64, 4), BlockStateId(WATER_STATES[0]));

        let damp_farmland = farmland_with_moisture(FARMLAND_DRY_STATE_ID, 2).unwrap();
        world.set_block_state(dry_farm_pos, BlockStateId(damp_farmland));

        for tick in 0..500_u64 {
            run_chunk_random_ticks(&mut world, 0, 0, 64, 80, 64, tick + 1000);
        }

        assert_eq!(
            farmland_moisture(world.get_block_state(wet_farm_pos).0),
            Some(7),
            "Farmland within 4 blocks of water should hydrate to moisture 7"
        );
        assert_eq!(
            world.get_block_state(dry_farm_pos),
            BlockStateId(DIRT_STATE_ID),
            "Farmland far from water with no crop above should dry out to dirt"
        );
    }

    #[test]
    fn crop_grows_on_hydrated_farmland() {
        let mut world = MapWorld::new();
        let wet_farmland = BlockStateId(farmland_with_moisture(FARMLAND_DRY_STATE_ID, 7).unwrap());
        let wheat_0 = default_state_id("minecraft:wheat").unwrap();

        for dx in -1..=1 {
            for dz in -1..=1 {
                world.set_block_state(BlockPos::new(8 + dx, 64, 8 + dz), wet_farmland);
            }
        }
        world.set_block_state(BlockPos::new(10, 64, 8), BlockStateId(WATER_STATES[0]));
        let crop_pos = BlockPos::new(8, 65, 8);
        world.set_block_state(crop_pos, wheat_0);

        for tick in 0..800_u64 {
            run_chunk_random_ticks(&mut world, 0, 0, 64, 80, 64, tick + 5000);
        }

        let final_state = world.get_block_state(crop_pos);
        assert_ne!(final_state, wheat_0, "Wheat on wet farmland should grow");
        assert!(
            crop_next_age(final_state.0).is_some() || crop_is_mature(final_state.0),
            "Wheat state should remain a valid wheat crop age"
        );
    }

    #[test]
    fn sugar_cane_grows_up_to_height_three() {
        let mut world = MapWorld::new();
        let base = BlockPos::new(5, 64, 5);
        world.set_block_state(base, BlockStateId(SUGAR_CANE_STATE_ID));

        for tick in 0..4000_u64 {
            run_chunk_random_ticks(&mut world, 0, 0, 64, 80, 128, tick + 9000);
        }

        assert!(sugar_cane_age(world.get_block_state(base).0).is_some());
        assert!(sugar_cane_age(world.get_block_state(base.offset(0, 1, 0)).0).is_some());
        assert!(sugar_cane_age(world.get_block_state(base.offset(0, 2, 0)).0).is_some());
        assert!(
            world.get_block_state(base.offset(0, 3, 0)).is_air(),
            "Sugar cane should cap natural growth at height 3"
        );
    }

    #[test]
    fn leaf_decay_respects_logs_and_persistence() {
        let mut world = MapWorld::new();
        let non_persistent_leaf = BlockStateId(TREE_LEAVES_STATES[0]);
        let isolated = BlockPos::new(1, 70, 1);
        let supported = BlockPos::new(10, 70, 10);
        world.set_block_state(isolated, non_persistent_leaf);
        world.set_block_state(supported, non_persistent_leaf);
        world.set_block_state(supported.offset(3, 0, 0), BlockStateId(TREE_LOG_STATES[0]));

        for tick in 0..400_u64 {
            run_chunk_random_ticks(&mut world, 0, 0, 64, 80, 64, tick + 20_000);
        }

        assert!(
            world.get_block_state(isolated).is_air(),
            "Isolated non-persistent leaves should decay"
        );
        assert_eq!(
            world.get_block_state(supported),
            non_persistent_leaf,
            "Leaves within Manhattan distance 6 of a log should not decay"
        );
    }
}
