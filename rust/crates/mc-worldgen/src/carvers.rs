//! Deterministic Minecraft 26.3 vanilla-style Overworld & Nether world carvers.
//!
//! Matches `src/main/java/net/minecraft/world/level/levelgen/carver/{WorldCarver,CaveWorldCarver,CanyonWorldCarver}.java`
//! and `src/main/resources/data/minecraft/worldgen/carver/{cave,cave_extra_underground,canyon,nether_cave}.json`.

use mc_data::{BlockStateId, CAVE_AIR_STATE_ID, LAVA_STATE_ID, WATER_STATE_ID};
use mc_math::{ChunkPos, HALF_PI, LegacyRandomSource, PI, RandomSource, TWO_PI, cos, floor_f64, sin};
use mc_world::Chunk;

/// Neighbor chunk scan radius (`NoiseBasedChunkGenerator.generateCarvers`: `dx, dz in -8..=8`).
const CARVER_SCAN_RANGE: i32 = 8;
/// Default `WorldCarver.getRange()` used to compute `maxDistance = (range * 2 - 1) * 16 = 112`.
const CARVER_TUNNEL_RANGE: i32 = 4;

/// Overworld aquifer lava flood level (`y <= -54`).
const OVERWORLD_LAVA_LEVEL: i32 = -54;
/// Nether lava sea level (`y <= 31`).
const NETHER_LAVA_LEVEL: i32 = 31;

/// Exact match for `WorldgenRandom.setLargeFeatureSeed(seed, chunkX, chunkZ)`.
#[inline]
pub fn set_carver_seed(rng: &mut LegacyRandomSource, seed: i64, chunk_x: i32, chunk_z: i32) {
    rng.set_seed(seed);
    let x_scale = rng.next_long();
    let z_scale = rng.next_long();
    let result = (chunk_x as i64).wrapping_mul(x_scale)
        ^ (chunk_z as i64).wrapping_mul(z_scale)
        ^ seed;
    rng.set_seed(result);
}

#[derive(Debug, Clone, Copy)]
struct CaveCarverConfig {
    probability: f32,
    min_y: i32,
    max_y: i32,
    count_max: i32,
    thickness_min: f32,
    thickness_max: f32,
    thickness_plateau: f32,
    weird_thickness_bias: bool,
    room_v_rad_min: f32,
    room_v_rad_max: f32,
    h_rad_min: f32,
    h_rad_max: f32,
    v_rad_min: f32,
    v_rad_max: f32,
    start_v_rad_mult: f64,
    floor_min: f32,
    floor_max: f32,
}

#[derive(Debug, Clone, Copy)]
struct CanyonCarverConfig {
    probability: f32,
    min_y: i32,
    max_y: i32,
    v_rot_min: f32,
    v_rot_max: f32,
    distance_factor_min: f32,
    distance_factor_max: f32,
    thickness_min: f32,
    thickness_max: f32,
    thickness_plateau: f32,
    width_smoothness: i32,
    h_rad_factor_min: f32,
    h_rad_factor_max: f32,
    v_rad_default_factor: f32,
    v_rad_center_factor: f32,
    y_scale: f64,
}

/// Vanilla 26.3 `data/minecraft/worldgen/carver/cave.json` (Overworld min_y = -64, above_bottom 8 -> -56..=180).
const OVERWORLD_CAVE: CaveCarverConfig = CaveCarverConfig {
    probability: 0.15,
    min_y: -56,
    max_y: 180,
    count_max: 14,
    thickness_min: 0.0,
    thickness_max: 3.0,
    thickness_plateau: 1.0,
    weird_thickness_bias: true,
    room_v_rad_min: 0.1,
    room_v_rad_max: 0.9,
    h_rad_min: 0.7,
    h_rad_max: 1.4,
    v_rad_min: 0.8,
    v_rad_max: 1.3,
    start_v_rad_mult: 1.0,
    floor_min: -1.0,
    floor_max: -0.4,
};

/// Vanilla 26.3 `data/minecraft/worldgen/carver/cave_extra_underground.json` (-56..=47).
const OVERWORLD_CAVE_EXTRA_UNDERGROUND: CaveCarverConfig = CaveCarverConfig {
    probability: 0.07,
    min_y: -56,
    max_y: 47,
    count_max: 14,
    thickness_min: 0.0,
    thickness_max: 3.0,
    thickness_plateau: 1.0,
    weird_thickness_bias: true,
    room_v_rad_min: 0.1,
    room_v_rad_max: 0.9,
    h_rad_min: 0.7,
    h_rad_max: 1.4,
    v_rad_min: 0.8,
    v_rad_max: 1.3,
    start_v_rad_mult: 1.0,
    floor_min: -1.0,
    floor_max: -0.4,
};

/// Vanilla 26.3 `data/minecraft/worldgen/carver/canyon.json` (10..=67).
const OVERWORLD_CANYON: CanyonCarverConfig = CanyonCarverConfig {
    probability: 0.01,
    min_y: 10,
    max_y: 67,
    v_rot_min: -0.125,
    v_rot_max: 0.125,
    distance_factor_min: 0.75,
    distance_factor_max: 1.0,
    thickness_min: 0.0,
    thickness_max: 6.0,
    thickness_plateau: 2.0,
    width_smoothness: 3,
    h_rad_factor_min: 0.75,
    h_rad_factor_max: 1.0,
    v_rad_default_factor: 1.0,
    v_rad_center_factor: 0.0,
    y_scale: 3.0,
};

/// Vanilla 26.3 `data/minecraft/worldgen/carver/nether_cave.json` (clamped safely to Y 10..=116).
const NETHER_CAVE: CaveCarverConfig = CaveCarverConfig {
    probability: 0.2,
    min_y: 10,
    max_y: 116,
    count_max: 9,
    thickness_min: 0.0,
    thickness_max: 6.0,
    thickness_plateau: 2.0,
    weird_thickness_bias: false,
    room_v_rad_min: 0.5,
    room_v_rad_max: 0.5,
    h_rad_min: 1.0,
    h_rad_max: 1.0,
    v_rad_min: 1.0,
    v_rad_max: 1.0,
    start_v_rad_mult: 5.0,
    floor_min: -0.7,
    floor_max: -0.7,
};

/// Compact per-chunk bitmask recording carved coordinates (`NoiseBasedChunkGenerator` `CarvingMask`).
struct CarvingMask {
    min_y: i32,
    max_y: i32,
    height: usize,
    bits: Vec<u64>,
    any_set: bool,
}

impl CarvingMask {
    fn new(min_y: i32, max_y: i32) -> Self {
        let height = if max_y >= min_y {
            (max_y - min_y + 1) as usize
        } else {
            0
        };
        let total_bits = 256 * height;
        let words = total_bits.div_ceil(64);
        Self {
            min_y,
            max_y,
            height,
            bits: vec![0u64; words],
            any_set: false,
        }
    }

    #[inline]
    fn bit_index(&self, lx: usize, world_y: i32, lz: usize) -> Option<usize> {
        if lx >= 16 || lz >= 16 || world_y < self.min_y || world_y > self.max_y {
            return None;
        }
        let y_rel = (world_y - self.min_y) as usize;
        Some(((lx << 4) | lz) * self.height + y_rel)
    }

    #[inline]
    fn carve(&mut self, lx: usize, world_y: i32, lz: usize) {
        if let Some(idx) = self.bit_index(lx, world_y, lz) {
            self.bits[idx >> 6] |= 1u64 << (idx & 63);
            self.any_set = true;
        }
    }

    #[inline]
    fn is_carved(&self, lx: usize, world_y: i32, lz: usize) -> bool {
        match self.bit_index(lx, world_y, lz) {
            Some(idx) => (self.bits[idx >> 6] & (1u64 << (idx & 63))) != 0,
            None => false,
        }
    }
}

/// Exact match for `VeryBiasedToBottomInt.sample(random)` with `min_inclusive = 0`.
#[inline]
fn sample_very_biased_to_bottom(rng: &mut LegacyRandomSource, max_inclusive: i32) -> i32 {
    let r1 = rng.next_int_bounded(max_inclusive + 1) + 1;
    let r2 = rng.next_int_bounded(r1) + 1;
    rng.next_int_bounded(r2)
}

/// Exact match for `UniformFloat.sample(random)` (or `ConstantFloat` when `min == max`).
#[inline]
fn sample_uniform_or_const(rng: &mut LegacyRandomSource, min: f32, max: f32) -> f32 {
    if (max - min).abs() <= f32::EPSILON {
        min
    } else {
        min + rng.next_float() * (max - min)
    }
}

/// Exact match for `TrapezoidFloat.sample(random)`.
#[inline]
fn sample_trapezoid(rng: &mut LegacyRandomSource, min: f32, max: f32, plateau: f32) -> f32 {
    let range = max - min;
    let plateau_start = (range - plateau) * 0.5_f32;
    let plateau_end = range - plateau_start;
    min + rng.next_float() * plateau_end + rng.next_float() * plateau_start
}

/// Exact match for `WorldCarver.canReach`.
#[inline]
fn can_reach(
    chunk_pos: ChunkPos,
    x: f64,
    z: f64,
    current_step: i32,
    total_steps: i32,
    thickness: f32,
) -> bool {
    let x_mid = ((chunk_pos.x << 4) + 8) as f64;
    let z_mid = ((chunk_pos.z << 4) + 8) as f64;
    let xd = x - x_mid;
    let zd = z - z_mid;
    let remaining = (total_steps - current_step) as f64;
    let rr = (thickness + 2.0_f32 + 16.0_f32) as f64;
    xd * xd + zd * zd - remaining * remaining <= rr * rr
}

/// Exact match for `WorldCarver.carveEllipsoid`.
fn carve_ellipsoid(
    chunk_pos: ChunkPos,
    x: f64,
    y: f64,
    z: f64,
    horizontal_radius: f64,
    vertical_radius: f64,
    mask: &mut CarvingMask,
    mut should_skip: impl FnMut(f64, f64, f64, i32) -> bool,
) {
    if horizontal_radius <= 0.0 || vertical_radius <= 0.0 {
        return;
    }
    let center_x = ((chunk_pos.x << 4) + 8) as f64;
    let center_z = ((chunk_pos.z << 4) + 8) as f64;
    let max_delta = 16.0 + horizontal_radius * 2.0;
    if (x - center_x).abs() > max_delta || (z - center_z).abs() > max_delta {
        return;
    }

    let chunk_min_x = chunk_pos.x << 4;
    let chunk_min_z = chunk_pos.z << 4;
    let min_x_idx = (floor_f64(x - horizontal_radius) - chunk_min_x - 1).max(0);
    let max_x_idx = (floor_f64(x + horizontal_radius) - chunk_min_x).min(15);
    let min_y = (floor_f64(y - vertical_radius) - 1).max(mask.min_y);
    let max_y = (floor_f64(y + vertical_radius) + 1).min(mask.max_y);
    let min_z_idx = (floor_f64(z - horizontal_radius) - chunk_min_z - 1).max(0);
    let max_z_idx = (floor_f64(z + horizontal_radius) - chunk_min_z).min(15);

    for x_idx in min_x_idx..=max_x_idx {
        let world_x = chunk_min_x + x_idx;
        let xd = (world_x as f64 + 0.5 - x) / horizontal_radius;

        for z_idx in min_z_idx..=max_z_idx {
            let world_z = chunk_min_z + z_idx;
            let zd = (world_z as f64 + 0.5 - z) / horizontal_radius;
            if xd * xd + zd * zd >= 1.0 {
                continue;
            }
            let mut world_y = max_y;
            while world_y > min_y {
                let yd = (world_y as f64 - 0.5 - y) / vertical_radius;
                if !should_skip(xd, yd, zd, world_y) {
                    mask.carve(x_idx as usize, world_y, z_idx as usize);
                }
                world_y -= 1;
            }
        }
    }
}

#[inline]
fn cave_should_skip(xd: f64, yd: f64, zd: f64, floor_level: f64) -> bool {
    yd <= floor_level || xd * xd + yd * yd + zd * zd >= 1.0
}

fn run_cave_carver(
    cfg: &CaveCarverConfig,
    rng: &mut LegacyRandomSource,
    target_pos: ChunkPos,
    source_pos: ChunkPos,
    mask: &mut CarvingMask,
) {
    if rng.next_float() > cfg.probability {
        return;
    }

    let y_lo = cfg.min_y.max(mask.min_y);
    let y_hi = cfg.max_y.min(mask.max_y);
    if y_lo > y_hi {
        return;
    }

    let max_distance = (CARVER_TUNNEL_RANGE * 2 - 1) * 16;
    let cave_count = sample_very_biased_to_bottom(rng, cfg.count_max);

    for _ in 0..cave_count {
        let x = ((source_pos.x << 4) + rng.next_int_bounded(16)) as f64;
        let y = (y_lo + rng.next_int_bounded(y_hi - y_lo + 1)) as f64;
        let z = ((source_pos.z << 4) + rng.next_int_bounded(16)) as f64;

        let h_rad_mult = sample_uniform_or_const(rng, cfg.h_rad_min, cfg.h_rad_max) as f64;
        let v_rad_mult = sample_uniform_or_const(rng, cfg.v_rad_min, cfg.v_rad_max) as f64;
        let start_v_rad_mult = cfg.start_v_rad_mult;
        let floor_level = sample_uniform_or_const(rng, cfg.floor_min, cfg.floor_max) as f64;

        let mut tunnels = 1;
        if rng.next_int_bounded(4) == 0 {
            let room_y_scale =
                sample_uniform_or_const(rng, cfg.room_v_rad_min, cfg.room_v_rad_max) as f64;
            let room_thickness = 1.0_f32 + rng.next_float() * 6.0_f32;
            let h_rad = 1.5 + (sin(HALF_PI as f64) * room_thickness) as f64;
            let v_rad = h_rad * room_y_scale;
            carve_ellipsoid(
                target_pos,
                x + 1.0,
                y,
                z,
                h_rad,
                v_rad,
                mask,
                |xd, yd, zd, _| cave_should_skip(xd, yd, zd, floor_level),
            );
            tunnels += rng.next_int_bounded(4);
        }

        for _ in 0..tunnels {
            let h_rot = rng.next_float() * TWO_PI;
            let v_rot = (rng.next_float() - 0.5_f32) / 4.0_f32;
            let mut thickness =
                sample_trapezoid(rng, cfg.thickness_min, cfg.thickness_max, cfg.thickness_plateau);
            if cfg.weird_thickness_bias && rng.next_int_bounded(10) == 0 {
                thickness *= rng.next_float() * rng.next_float() * 3.0_f32 + 1.0_f32;
            }
            let distance = max_distance - rng.next_int_bounded(max_distance / 4);
            let tunnel_seed = rng.next_long();

            carve_cave_tunnel(
                target_pos,
                tunnel_seed,
                x,
                y,
                z,
                h_rad_mult,
                v_rad_mult,
                thickness,
                h_rot,
                v_rot,
                0,
                distance,
                start_v_rad_mult,
                floor_level,
                mask,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn carve_cave_tunnel(
    target_pos: ChunkPos,
    tunnel_seed: i64,
    mut x: f64,
    mut y: f64,
    mut z: f64,
    h_rad_mult: f64,
    v_rad_mult: f64,
    thickness: f32,
    mut h_rot: f32,
    mut v_rot: f32,
    start_step: i32,
    dist: i32,
    y_scale: f64,
    floor_level: f64,
    mask: &mut CarvingMask,
) {
    let mut trng = LegacyRandomSource::new(tunnel_seed);
    let split_point = trng.next_int_bounded((dist / 2).max(1)) + dist / 4;
    let steep = trng.next_int_bounded(6) == 0;
    let mut y_rota = 0.0_f32;
    let mut x_rota = 0.0_f32;

    for current_step in start_step..dist {
        let h_rad = 1.5 + (sin((PI * current_step as f32 / dist as f32) as f64) * thickness) as f64;
        let v_rad = h_rad * y_scale;
        let cos_x = cos(v_rot as f64);
        x += (cos(h_rot as f64) * cos_x) as f64;
        y += sin(v_rot as f64) as f64;
        z += (sin(h_rot as f64) * cos_x) as f64;

        v_rot *= if steep { 0.92_f32 } else { 0.7_f32 };
        v_rot += x_rota * 0.1_f32;
        h_rot += y_rota * 0.1_f32;
        x_rota *= 0.9_f32;
        y_rota *= 0.75_f32;
        x_rota += (trng.next_float() - trng.next_float()) * trng.next_float() * 2.0_f32;
        y_rota += (trng.next_float() - trng.next_float()) * trng.next_float() * 4.0_f32;

        if current_step == split_point && thickness > 1.0_f32 {
            let left_seed = trng.next_long();
            let left_thick = trng.next_float() * 0.5_f32 + 0.5_f32;
            carve_cave_tunnel(
                target_pos,
                left_seed,
                x,
                y,
                z,
                h_rad_mult,
                v_rad_mult,
                left_thick,
                h_rot - HALF_PI,
                v_rot / 3.0_f32,
                current_step,
                dist,
                1.0,
                floor_level,
                mask,
            );
            let right_seed = trng.next_long();
            let right_thick = trng.next_float() * 0.5_f32 + 0.5_f32;
            carve_cave_tunnel(
                target_pos,
                right_seed,
                x,
                y,
                z,
                h_rad_mult,
                v_rad_mult,
                right_thick,
                h_rot + HALF_PI,
                v_rot / 3.0_f32,
                current_step,
                dist,
                1.0,
                floor_level,
                mask,
            );
            return;
        }

        if trng.next_int_bounded(4) != 0 {
            if !can_reach(target_pos, x, z, current_step, dist, thickness) {
                return;
            }
            carve_ellipsoid(
                target_pos,
                x,
                y,
                z,
                h_rad * h_rad_mult,
                v_rad * v_rad_mult,
                mask,
                |xd, yd, zd, _| cave_should_skip(xd, yd, zd, floor_level),
            );
        }
    }
}

fn run_canyon_carver(
    cfg: &CanyonCarverConfig,
    rng: &mut LegacyRandomSource,
    target_pos: ChunkPos,
    source_pos: ChunkPos,
    chunk_min_y: i32,
    chunk_height: usize,
    mask: &mut CarvingMask,
) {
    if rng.next_float() > cfg.probability {
        return;
    }

    let y_lo = cfg.min_y.max(mask.min_y);
    let y_hi = cfg.max_y.min(mask.max_y);
    if y_lo > y_hi {
        return;
    }

    let max_distance = (CARVER_TUNNEL_RANGE * 2 - 1) * 16;
    let mut x = ((source_pos.x << 4) + rng.next_int_bounded(16)) as f64;
    let mut y = (y_lo + rng.next_int_bounded(y_hi - y_lo + 1)) as f64;
    let mut z = ((source_pos.z << 4) + rng.next_int_bounded(16)) as f64;

    let mut h_rot = rng.next_float() * TWO_PI;
    let mut v_rot = sample_uniform_or_const(rng, cfg.v_rot_min, cfg.v_rot_max);
    let y_scale = cfg.y_scale;
    let thickness = sample_trapezoid(rng, cfg.thickness_min, cfg.thickness_max, cfg.thickness_plateau);
    let dist_factor =
        sample_uniform_or_const(rng, cfg.distance_factor_min, cfg.distance_factor_max);
    let distance = ((max_distance as f32) * dist_factor) as i32;
    if distance <= 0 {
        return;
    }
    let tunnel_seed = rng.next_long();

    let mut trng = LegacyRandomSource::new(tunnel_seed);
    let mut width_factors = vec![1.0_f32; chunk_height];
    let mut width_factor = 1.0_f32;
    for (y_idx, slot) in width_factors.iter_mut().enumerate() {
        if y_idx == 0 || trng.next_int_bounded(cfg.width_smoothness) == 0 {
            width_factor = 1.0_f32 + trng.next_float() * trng.next_float();
        }
        *slot = width_factor * width_factor;
    }

    let mut y_rota = 0.0_f32;
    let mut x_rota = 0.0_f32;

    for current_step in 0..distance {
        let mut h_rad =
            1.5 + (sin((current_step as f32 * PI / distance as f32) as f64) * thickness) as f64;
        let mut v_rad = h_rad * y_scale;
        h_rad *= sample_uniform_or_const(&mut trng, cfg.h_rad_factor_min, cfg.h_rad_factor_max)
            as f64;

        let vertical_mult = 1.0_f32 - (0.5_f32 - current_step as f32 / distance as f32).abs() * 2.0_f32;
        let v_factor = cfg.v_rad_default_factor + cfg.v_rad_center_factor * vertical_mult;
        let rand_between = 0.75_f32 + trng.next_float() * 0.25_f32;
        v_rad = (v_factor as f64) * v_rad * (rand_between as f64);

        let xc = cos(v_rot as f64);
        let xs = sin(v_rot as f64);
        x += (cos(h_rot as f64) * xc) as f64;
        y += xs as f64;
        z += (sin(h_rot as f64) * xc) as f64;

        v_rot *= 0.7_f32;
        v_rot += x_rota * 0.05_f32;
        h_rot += y_rota * 0.05_f32;
        x_rota *= 0.8_f32;
        y_rota *= 0.5_f32;
        x_rota += (trng.next_float() - trng.next_float()) * trng.next_float() * 2.0_f32;
        y_rota += (trng.next_float() - trng.next_float()) * trng.next_float() * 4.0_f32;

        if trng.next_int_bounded(4) != 0 {
            if !can_reach(target_pos, x, z, current_step, distance, thickness) {
                return;
            }
            carve_ellipsoid(
                target_pos,
                x,
                y,
                z,
                h_rad,
                v_rad,
                mask,
                |xd, yd, zd, world_y| {
                    let y_idx = world_y - chunk_min_y - 1;
                    if y_idx < 0 || (y_idx as usize) >= width_factors.len() {
                        return true;
                    }
                    let wf = width_factors[y_idx as usize] as f64;
                    (xd * xd + zd * zd) * wf + (yd * yd) / 6.0 >= 1.0
                },
            );
        }
    }
}

#[inline]
fn is_overworld_carvable(state: BlockStateId) -> bool {
    if state == BlockStateId::BEDROCK || state.is_air() || state.is_fluid_block() {
        return false;
    }
    if state == BlockStateId::STONE
        || state == BlockStateId::DIRT
        || state == BlockStateId::GRASS_BLOCK
    {
        return true;
    }
    let name = state.block_name();
    matches!(
        name,
        "minecraft:deepslate"
            | "minecraft:granite"
            | "minecraft:diorite"
            | "minecraft:andesite"
            | "minecraft:tuff"
            | "minecraft:calcite"
            | "minecraft:dripstone_block"
            | "minecraft:coarse_dirt"
            | "minecraft:podzol"
            | "minecraft:rooted_dirt"
            | "minecraft:mud"
            | "minecraft:mycelium"
            | "minecraft:clay"
            | "minecraft:gravel"
            | "minecraft:sand"
            | "minecraft:red_sand"
            | "minecraft:sandstone"
            | "minecraft:red_sandstone"
            | "minecraft:terracotta"
            | "minecraft:packed_ice"
            | "minecraft:snow_block"
            | "minecraft:snow"
            | "minecraft:moss_block"
            | "minecraft:pale_moss_block"
    ) || name.ends_with("_terracotta")
}

#[inline]
fn is_nether_carvable(state: BlockStateId) -> bool {
    if state == BlockStateId::BEDROCK || state.is_air() || state.is_fluid_block() {
        return false;
    }
    matches!(
        state.block_name(),
        "minecraft:netherrack"
            | "minecraft:soul_sand"
            | "minecraft:soul_soil"
            | "minecraft:basalt"
            | "minecraft:blackstone"
            | "minecraft:crimson_nylium"
            | "minecraft:warped_nylium"
            | "minecraft:gravel"
            | "minecraft:magma_block"
    )
}

#[inline]
fn has_adjacent_fluid_barrier(
    chunk: &Chunk,
    lx: usize,
    world_y: i32,
    lz: usize,
    water_only: bool,
) -> bool {
    let check = |st: BlockStateId| -> bool {
        if water_only {
            st.0 == WATER_STATE_ID || st.block_name() == "minecraft:water"
        } else {
            st.is_fluid_block()
        }
    };

    if world_y + 1 < chunk.max_y() && check(chunk.get_block(lx, world_y + 1, lz)) {
        return true;
    }
    if lx > 0 && check(chunk.get_block(lx - 1, world_y, lz)) {
        return true;
    }
    if lx < 15 && check(chunk.get_block(lx + 1, world_y, lz)) {
        return true;
    }
    if lz > 0 && check(chunk.get_block(lx, world_y, lz - 1)) {
        return true;
    }
    if lz < 15 && check(chunk.get_block(lx, world_y, lz + 1)) {
        return true;
    }
    false
}

#[inline]
fn set_block_fast(
    chunk: &mut Chunk,
    lx: usize,
    world_y: i32,
    lz: usize,
    state: BlockStateId,
) -> bool {
    let sec_y = world_y >> 4;
    let idx = sec_y - chunk.min_section_y;
    if idx < 0 || (idx as usize) >= chunk.sections.len() {
        return false;
    }
    let old = chunk.sections[idx as usize].set_block(lx, (world_y & 15) as usize, lz, state);
    old != state
}

/// Apply deterministic vanilla 26.3 Overworld carvers (`cave`, `cave_extra_underground`, `canyon`)
/// into `chunk`.
///
/// Never carves bedrock or breaches water/aquifer walls. Carved blocks at `y <= -54` are filled
/// with lava; blocks above `-54` are filled with `cave_air`.
pub fn carve_overworld_chunk(chunk: &mut Chunk, seed: i64) {
    let min_y = chunk.min_y();
    let max_y = chunk.max_y();
    let height = chunk.height();
    if height < 16 {
        return;
    }

    // Protect bedrock floor (min_y) and top 7 blocks (matching NoiseBasedChunkGenerator).
    let mask_min_y = min_y + 1;
    let mask_max_y = max_y - 1 - 7;
    if mask_min_y >= mask_max_y {
        return;
    }

    let mut mask = CarvingMask::new(mask_min_y, mask_max_y);
    let target_pos = chunk.pos;
    let mut rng = LegacyRandomSource::new(0);

    for dx in -CARVER_SCAN_RANGE..=CARVER_SCAN_RANGE {
        for dz in -CARVER_SCAN_RANGE..=CARVER_SCAN_RANGE {
            let source_pos = ChunkPos::new(target_pos.x + dx, target_pos.z + dz);

            // Carver 0: minecraft:cave
            set_carver_seed(&mut rng, seed, source_pos.x, source_pos.z);
            run_cave_carver(&OVERWORLD_CAVE, &mut rng, target_pos, source_pos, &mut mask);

            // Carver 1: minecraft:cave_extra_underground
            set_carver_seed(&mut rng, seed.wrapping_add(1), source_pos.x, source_pos.z);
            run_cave_carver(
                &OVERWORLD_CAVE_EXTRA_UNDERGROUND,
                &mut rng,
                target_pos,
                source_pos,
                &mut mask,
            );

            // Carver 2: minecraft:canyon
            set_carver_seed(&mut rng, seed.wrapping_add(2), source_pos.x, source_pos.z);
            run_canyon_carver(
                &OVERWORLD_CANYON,
                &mut rng,
                target_pos,
                source_pos,
                min_y,
                height,
                &mut mask,
            );
        }
    }

    if !mask.any_set {
        return;
    }

    let cave_air = BlockStateId(CAVE_AIR_STATE_ID);
    let lava = BlockStateId(LAVA_STATE_ID);
    let mut any_changed = false;

    for lx in 0..16 {
        for lz in 0..16 {
            let mut carved_grass = false;
            for world_y in (mask.min_y..=mask.max_y).rev() {
                if !mask.is_carved(lx, world_y, lz) {
                    continue;
                }
                if world_y <= min_y {
                    continue;
                }
                let cur = chunk.get_block(lx, world_y, lz);
                if !is_overworld_carvable(cur) {
                    continue;
                }

                let fill_lava = world_y <= OVERWORLD_LAVA_LEVEL;
                if has_adjacent_fluid_barrier(chunk, lx, world_y, lz, fill_lava) {
                    continue;
                }

                if cur == BlockStateId::GRASS_BLOCK {
                    carved_grass = true;
                }

                let replacement = if fill_lava { lava } else { cave_air };
                if set_block_fast(chunk, lx, world_y, lz, replacement) {
                    any_changed = true;
                }

                // Transfer grass surface to exposed dirt floor just below carved grass entrances.
                if carved_grass
                    && !fill_lava
                    && world_y - 1 > min_y
                    && !mask.is_carved(lx, world_y - 1, lz)
                    && chunk.get_block(lx, world_y - 1, lz) == BlockStateId::DIRT
                {
                    if set_block_fast(chunk, lx, world_y - 1, lz, BlockStateId::GRASS_BLOCK) {
                        any_changed = true;
                    }
                    carved_grass = false;
                }
            }
        }
    }

    if any_changed {
        chunk.recalc_all_heightmaps();
    }
}

/// Apply deterministic vanilla 26.3 Nether carver (`nether_cave`) into `chunk`.
///
/// Preserves floor and ceiling bedrock layers (`y <= 4` and `y >= 123`) and fills carved spaces
/// at `y <= 31` with lava and above `31` with `cave_air`.
pub fn carve_nether_chunk(chunk: &mut Chunk, seed: i64) {
    let min_y = chunk.min_y();
    let max_y = chunk.max_y();
    if chunk.height() < 16 {
        return;
    }

    let safe_min_y = (min_y + 5).max(1);
    let safe_max_y = max_y - 6;
    if safe_min_y >= safe_max_y {
        return;
    }

    let mut mask = CarvingMask::new(safe_min_y, safe_max_y);
    let target_pos = chunk.pos;
    let mut rng = LegacyRandomSource::new(0);

    for dx in -CARVER_SCAN_RANGE..=CARVER_SCAN_RANGE {
        for dz in -CARVER_SCAN_RANGE..=CARVER_SCAN_RANGE {
            let source_pos = ChunkPos::new(target_pos.x + dx, target_pos.z + dz);
            set_carver_seed(&mut rng, seed, source_pos.x, source_pos.z);
            run_cave_carver(&NETHER_CAVE, &mut rng, target_pos, source_pos, &mut mask);
        }
    }

    if !mask.any_set {
        return;
    }

    let cave_air = BlockStateId(CAVE_AIR_STATE_ID);
    let lava = BlockStateId(LAVA_STATE_ID);
    let mut any_changed = false;

    for lx in 0..16 {
        for lz in 0..16 {
            for world_y in (mask.min_y..=mask.max_y).rev() {
                if !mask.is_carved(lx, world_y, lz) {
                    continue;
                }
                let cur = chunk.get_block(lx, world_y, lz);
                if !is_nether_carvable(cur) {
                    continue;
                }
                let fill_lava = world_y <= NETHER_LAVA_LEVEL;
                if has_adjacent_fluid_barrier(chunk, lx, world_y, lz, fill_lava) {
                    continue;
                }
                let replacement = if fill_lava { lava } else { cave_air };
                if set_block_fast(chunk, lx, world_y, lz, replacement) {
                    any_changed = true;
                }
            }
        }
    }

    if any_changed {
        chunk.recalc_all_heightmaps();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_data::{TOTAL_BLOCK_STATES, default_state_id};
    use mc_world::{
        ChunkSection, OVERWORLD_MIN_SECTION_Y, OVERWORLD_SECTION_COUNT, PaletteStrategy,
        PalettedStorage,
    };
    use std::collections::BTreeMap;

    fn make_solid_overworld_chunk(pos: ChunkPos) -> Chunk {
        let block_strat = PaletteStrategy::for_block_states(TOTAL_BLOCK_STATES);
        let deepslate = default_state_id("minecraft:deepslate")
            .unwrap_or(BlockStateId::STONE)
            .0;
        let min_y = OVERWORLD_MIN_SECTION_Y << 4;

        let mut sections = Vec::with_capacity(OVERWORLD_SECTION_COUNT);
        for sec_idx in 0..OVERWORLD_SECTION_COUNT {
            let sec_y = OVERWORLD_MIN_SECTION_Y + sec_idx as i32;
            let mut flat = vec![BlockStateId::AIR.0; 4096];
            for ly in 0..16 {
                let wy = (sec_y << 4) + ly as i32;
                let st = if wy == min_y {
                    BlockStateId::BEDROCK.0
                } else if wy < 0 {
                    deepslate
                } else if wy < 64 {
                    BlockStateId::STONE.0
                } else if wy < 67 {
                    BlockStateId::DIRT.0
                } else if wy == 67 {
                    BlockStateId::GRASS_BLOCK.0
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
            min_section_y: OVERWORLD_MIN_SECTION_Y,
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

    fn make_solid_nether_chunk(pos: ChunkPos) -> Chunk {
        let block_strat = PaletteStrategy::for_block_states(TOTAL_BLOCK_STATES);
        let netherrack = default_state_id("minecraft:netherrack")
            .unwrap_or(BlockStateId::STONE)
            .0;

        let mut sections = Vec::with_capacity(8);
        for sec_idx in 0..8 {
            let sec_y = sec_idx as i32;
            let mut flat = vec![netherrack; 4096];
            for ly in 0..16 {
                let wy = (sec_y << 4) + ly as i32;
                let st = if wy <= 4 || wy >= 123 {
                    BlockStateId::BEDROCK.0
                } else {
                    netherrack
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
    fn overworld_carver_is_deterministic_carves_cave_air_and_lava_and_preserves_bedrock() {
        let seed = 263_2026_i64;
        let cave_air = BlockStateId(CAVE_AIR_STATE_ID);
        let lava = BlockStateId(LAVA_STATE_ID);

        let mut total_cave_air = 0usize;
        let mut total_lava = 0usize;

        for cx in 0..4 {
            for cz in 0..4 {
                let pos = ChunkPos::new(cx, cz);
                let mut c1 = make_solid_overworld_chunk(pos);
                let mut c2 = c1.clone();

                carve_overworld_chunk(&mut c1, seed);
                carve_overworld_chunk(&mut c2, seed);

                for lx in 0..16 {
                    for lz in 0..16 {
                        assert_eq!(
                            c1.get_block(lx, -64, lz),
                            BlockStateId::BEDROCK,
                            "Bedrock at y=-64 must never be carved"
                        );
                        for wy in -64..=100 {
                            let s1 = c1.get_block(lx, wy, lz);
                            let s2 = c2.get_block(lx, wy, lz);
                            assert_eq!(s1, s2, "Carver must be deterministic at ({lx},{wy},{lz})");
                            if s1 == cave_air {
                                assert!(wy > OVERWORLD_LAVA_LEVEL, "cave_air must not appear at y <= -54");
                                total_cave_air += 1;
                            } else if s1 == lava {
                                assert!(wy <= OVERWORLD_LAVA_LEVEL, "lava must only fill y <= -54");
                                total_lava += 1;
                            }
                        }
                    }
                }
            }
        }

        assert!(total_cave_air > 500, "Expected carved cave_air blocks, got {total_cave_air}");
        assert!(total_lava >= 20, "Expected carved lava blocks at y <= -54, got {total_lava}");
    }

    #[test]
    fn overworld_carver_respects_water_walls() {
        let seed = 99_8877_i64;
        let water = BlockStateId(WATER_STATE_ID);
        let cave_air = BlockStateId(CAVE_AIR_STATE_ID);

        let mut chunk = make_solid_overworld_chunk(ChunkPos::new(1, 1));
        for y in 40..64 {
            for lz in 0..16 {
                for lx in 0..16 {
                    set_block_fast(&mut chunk, lx, y, lz, water);
                }
            }
        }
        carve_overworld_chunk(&mut chunk, seed);

        // No cave_air should sit directly beneath the water floor at y = 39.
        for lz in 0..16 {
            for lx in 0..16 {
                assert_eq!(chunk.get_block(lx, 40, lz), water);
                assert_ne!(
                    chunk.get_block(lx, 39, lz),
                    cave_air,
                    "Carver breached floor directly beneath water at ({lx}, 39, {lz})"
                );
            }
        }
    }

    #[test]
    fn nether_carver_carves_netherrack_and_preserves_bedrock_floor_and_roof() {
        let seed = 777_42_i64;
        let cave_air = BlockStateId(CAVE_AIR_STATE_ID);
        let lava = BlockStateId(LAVA_STATE_ID);

        let mut total_cave_air = 0usize;
        let mut total_lava = 0usize;

        for cx in 0..3 {
            for cz in 0..3 {
                let mut chunk = make_solid_nether_chunk(ChunkPos::new(cx, cz));
                carve_nether_chunk(&mut chunk, seed);

                for lx in 0..16 {
                    for lz in 0..16 {
                        assert_eq!(chunk.get_block(lx, 0, lz), BlockStateId::BEDROCK);
                        assert_eq!(chunk.get_block(lx, 127, lz), BlockStateId::BEDROCK);
                        for wy in 0..128 {
                            let st = chunk.get_block(lx, wy, lz);
                            if st == cave_air {
                                assert!(wy > NETHER_LAVA_LEVEL);
                                total_cave_air += 1;
                            } else if st == lava {
                                assert!(wy <= NETHER_LAVA_LEVEL);
                                total_lava += 1;
                            }
                        }
                    }
                }
            }
        }

        assert!(total_cave_air > 500, "Expected Nether carved cave_air, got {total_cave_air}");
        assert!(total_lava > 50, "Expected Nether carved lava at y <= 31, got {total_lava}");
    }
}
