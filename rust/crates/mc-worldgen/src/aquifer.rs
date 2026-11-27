use crate::density::{EvalCache, OverworldGenerator};
use crate::generated_overworld::{
    OW_ROOT_AQ_BARRIER, OW_ROOT_AQ_EXCLUSION, OW_ROOT_AQ_FLOODEDNESS, OW_ROOT_AQ_LAVA,
    OW_ROOT_AQ_SPREAD, OW_ROOT_AQ_SURFACE_LEVEL,
};
use mc_data::{BlockStateId, default_state_id};
use mc_math::{BlockPos, RandomSource, XoroshiroPositionalRandomFactory, floor_div, floor_f32};
use std::collections::HashMap;

const WAY_BELOW_MIN_Y: i32 = -64 - 1024;
const SEA_LEVEL: i32 = 63;
const LAVA_LEVEL: i32 = -54;

const SURFACE_OFFSETS_IN_CHUNKS: [(i32, i32); 13] = [
    (0, 0),
    (-2, -1),
    (-1, -1),
    (0, -1),
    (1, -1),
    (-3, 0),
    (-2, 0),
    (-1, 0),
    (1, 0),
    (-2, 1),
    (-1, 1),
    (0, 1),
    (1, 1),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidStatus {
    pub fluid_level: i32,
    pub fluid_state: u32,
}

impl FluidStatus {
    #[inline]
    pub const fn at(self, block_y: i32) -> u32 {
        if block_y < self.fluid_level {
            self.fluid_state
        } else {
            BlockStateId::AIR.0
        }
    }
}

#[inline]
fn grid_x(block_x: i32) -> i32 {
    block_x >> 4
}
#[inline]
fn from_grid_x(gx: i32, off: i32) -> i32 {
    (gx << 4) + off
}
#[inline]
fn grid_y(block_y: i32) -> i32 {
    floor_div(block_y, 12)
}
#[inline]
fn from_grid_y(gy: i32, off: i32) -> i32 {
    gy * 12 + off
}
#[inline]
fn grid_z(block_z: i32) -> i32 {
    block_z >> 4
}
#[inline]
fn from_grid_z(gz: i32, off: i32) -> i32 {
    (gz << 4) + off
}
#[inline]
fn similarity(d1: i32, d2: i32) -> f64 {
    1.0 - ((d2 - d1) as f64) / 25.0
}
#[inline]
fn quantize_i32(val: f64, step: i32) -> i32 {
    floor_div(val.floor() as i32, step) * step
}
#[inline]
fn map_f64(v: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    let t = (v - from_min) / (from_max - from_min);
    to_min + t * (to_max - to_min)
}
#[inline]
fn clamped_map_f64(v: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    let t = ((v - from_min) / (from_max - from_min)).clamp(0.0, 1.0);
    to_min + t * (to_max - to_min)
}

/// Exact port of Minecraft 26.3's `Aquifer.NoiseBasedAquifer` for a 16x384x16 Overworld chunk.
pub struct NoiseBasedAquifer<'a> {
    gen_ref: &'a OverworldGenerator,
    rng_factory: XoroshiroPositionalRandomFactory,
    water_state: u32,
    lava_state: u32,
    min_grid_x: i32,
    min_grid_y: i32,
    min_grid_z: i32,
    grid_size_x: i32,
    grid_size_z: i32,
    skip_sampling_above_y: i32,
    aquifer_cache: Vec<Option<FluidStatus>>,
    aquifer_locations: Vec<i64>,
    surface_level_cache: HashMap<(i32, i32), i32>,
}

impl<'a> NoiseBasedAquifer<'a> {
    pub fn new(
        gen_ref: &'a OverworldGenerator,
        min_block_x: i32,
        min_block_y: i32,
        min_block_z: i32,
        max_block_x: i32,
        max_block_y: i32,
        max_block_z: i32,
        eval_cache: &mut EvalCache,
    ) -> Self {
        let water_state = default_state_id("minecraft:water")
            .unwrap_or(BlockStateId::AIR)
            .0;
        let lava_state = default_state_id("minecraft:lava")
            .unwrap_or(BlockStateId::AIR)
            .0;

        let min_grid_x = grid_x(min_block_x - 5);
        let max_grid_x = grid_x(max_block_x - 5) + 1;
        let grid_size_x = max_grid_x - min_grid_x + 1;

        let min_grid_y = grid_y(min_block_y + 1) - 1;
        let max_grid_y = grid_y(max_block_y + 1) + 1;
        let grid_size_y = max_grid_y - min_grid_y + 1;

        let min_grid_z = grid_z(min_block_z - 5);
        let max_grid_z = grid_z(max_block_z - 5) + 1;
        let grid_size_z = max_grid_z - min_grid_z + 1;

        let total = (grid_size_x * grid_size_y * grid_size_z) as usize;
        let mut me = Self {
            gen_ref,
            rng_factory: gen_ref.aquifer_random_factory(),
            water_state,
            lava_state,
            min_grid_x,
            min_grid_y,
            min_grid_z,
            grid_size_x,
            grid_size_z,
            skip_sampling_above_y: 320,
            aquifer_cache: vec![None; total],
            aquifer_locations: vec![i64::MAX; total],
            surface_level_cache: HashMap::with_capacity(64),
        };

        // Pre-compute max surface level over aquifer grid bounds
        let min_qx = from_grid_x(min_grid_x, 0) >> 2;
        let max_qx = from_grid_x(max_grid_x, 9) >> 2;
        let min_qz = from_grid_z(min_grid_z, 0) >> 2;
        let max_qz = from_grid_z(max_grid_z, 9) >> 2;
        let mut max_surf = i32::MIN;
        for qz in min_qz..=max_qz {
            let bz = qz << 2;
            for qx in min_qx..=max_qx {
                let bx = qx << 2;
                let sl = floor_f32(
                    gen_ref.eval_node_cached(OW_ROOT_AQ_SURFACE_LEVEL, bx, 0, bz, eval_cache),
                );
                me.surface_level_cache.insert((bx, bz), sl);
                if sl > max_surf {
                    max_surf = sl;
                }
            }
        }
        let max_adj = max_surf + 8;
        let skip_gy = grid_y(max_adj + 12) + 1;
        me.skip_sampling_above_y = from_grid_y(skip_gy, 11) - 1;
        me
    }

    #[inline]
    fn global_fluid(&self, block_y: i32) -> FluidStatus {
        if block_y < LAVA_LEVEL {
            FluidStatus {
                fluid_level: LAVA_LEVEL,
                fluid_state: self.lava_state,
            }
        } else {
            FluidStatus {
                fluid_level: SEA_LEVEL,
                fluid_state: self.water_state,
            }
        }
    }

    #[inline]
    fn grid_index(&self, gx: i32, gy: i32, gz: i32) -> usize {
        let x = gx - self.min_grid_x;
        let y = gy - self.min_grid_y;
        let z = gz - self.min_grid_z;
        ((y * self.grid_size_z + z) * self.grid_size_x + x) as usize
    }

    fn surface_level(&mut self, block_x: i32, block_z: i32, eval_cache: &mut EvalCache) -> i32 {
        let qx = (block_x >> 2) << 2;
        let qz = (block_z >> 2) << 2;
        if let Some(&v) = self.surface_level_cache.get(&(qx, qz)) {
            return v;
        }
        let v = floor_f32(
            self.gen_ref
                .eval_node_cached(OW_ROOT_AQ_SURFACE_LEVEL, qx, 0, qz, eval_cache),
        );
        self.surface_level_cache.insert((qx, qz), v);
        v
    }

    /// Returns `None` if solid terrain (density + barrier > 0), or `Some(block_state_id)` for fluid/air.
    pub fn compute_substance(
        &mut self,
        block_x: i32,
        block_y: i32,
        block_z: i32,
        density: f64,
        eval_cache: &mut EvalCache,
    ) -> Option<u32> {
        if density > 0.0 {
            return None;
        }
        let global = self.global_fluid(block_y);
        if block_y > self.skip_sampling_above_y {
            return Some(global.at(block_y));
        }
        if global.at(block_y) == self.lava_state {
            return Some(self.lava_state);
        }

        let x_anchor = grid_x(block_x - 5);
        let y_anchor = grid_y(block_y + 1);
        let z_anchor = grid_z(block_z - 5);

        let mut d1 = i32::MAX;
        let mut d2 = i32::MAX;
        let mut d3 = i32::MAX;
        let mut idx1 = 0usize;
        let mut idx2 = 0usize;
        let mut idx3 = 0usize;

        for x1 in 0..=1 {
            for y1 in -1..=1 {
                for z1 in 0..=1 {
                    let gx = x_anchor + x1;
                    let gy = y_anchor + y1;
                    let gz = z_anchor + z1;
                    let index = self.grid_index(gx, gy, gz);
                    let loc = if self.aquifer_locations[index] != i64::MAX {
                        self.aquifer_locations[index]
                    } else {
                        let mut rng = self.rng_factory.at(gx, gy, gz);
                        let bp = BlockPos::new(
                            from_grid_x(gx, rng.next_int_bounded(10)),
                            from_grid_y(gy, rng.next_int_bounded(9)),
                            from_grid_z(gz, rng.next_int_bounded(10)),
                        );
                        let packed = bp.as_long();
                        self.aquifer_locations[index] = packed;
                        packed
                    };
                    let pos = BlockPos::from_long(loc);
                    let dx = pos.x - block_x;
                    let dy = pos.y - block_y;
                    let dz = pos.z - block_z;
                    let dist = dx * dx + dy * dy + dz * dz;
                    if d1 >= dist {
                        idx3 = idx2;
                        idx2 = idx1;
                        idx1 = index;
                        d3 = d2;
                        d2 = d1;
                        d1 = dist;
                    } else if d2 >= dist {
                        idx3 = idx2;
                        idx2 = index;
                        d3 = d2;
                        d2 = dist;
                    } else if d3 >= dist {
                        idx3 = index;
                        d3 = dist;
                    }
                }
            }
        }

        let s1 = self.get_aquifer_status(idx1, eval_cache);
        let sim12 = similarity(d1, d2);
        let fluid_state = s1.at(block_y);
        if sim12 <= 0.0 {
            return Some(fluid_state);
        }
        if fluid_state == self.water_state && self.global_fluid(block_y - 1).at(block_y - 1) == self.lava_state {
            return Some(fluid_state);
        }

        let mut barrier_noise_val = f64::NAN;
        let s2 = self.get_aquifer_status(idx2, eval_cache);
        let b12 = sim12 * self.calculate_pressure(block_x, block_y, block_z, &mut barrier_noise_val, s1, s2, eval_cache);
        if density + b12 > 0.0 {
            return None;
        }

        let s3 = self.get_aquifer_status(idx3, eval_cache);
        let sim13 = similarity(d1, d3);
        if sim13 > 0.0 {
            let b13 = sim12 * sim13 * self.calculate_pressure(block_x, block_y, block_z, &mut barrier_noise_val, s1, s3, eval_cache);
            if density + b13 > 0.0 {
                return None;
            }
        }

        let sim23 = similarity(d2, d3);
        if sim23 > 0.0 {
            let b23 = sim12 * sim23 * self.calculate_pressure(block_x, block_y, block_z, &mut barrier_noise_val, s2, s3, eval_cache);
            if density + b23 > 0.0 {
                return None;
            }
        }

        Some(fluid_state)
    }

    #[allow(clippy::too_many_arguments)]
    fn calculate_pressure(
        &self,
        block_x: i32,
        block_y: i32,
        block_z: i32,
        barrier_noise_val: &mut f64,
        s1: FluidStatus,
        s2: FluidStatus,
        eval_cache: &mut EvalCache,
    ) -> f64 {
        let t1 = s1.at(block_y);
        let t2 = s2.at(block_y);
        if (t1 == self.lava_state && t2 == self.water_state)
            || (t1 == self.water_state && t2 == self.lava_state)
        {
            return 2.0;
        }
        let diff = (s1.fluid_level - s2.fluid_level).abs();
        if diff == 0 {
            return 0.0;
        }
        let avg_y = 0.5 * ((s1.fluid_level + s2.fluid_level) as f64);
        let above = (block_y as f64) + 0.5 - avg_y;
        let base = (diff as f64) / 2.0;
        let dist_mid = base - above.abs();
        let gradient = if above > 0.0 {
            if dist_mid > 0.0 {
                dist_mid / 1.5
            } else {
                dist_mid / 2.5
            }
        } else {
            let cp = 3.0 + dist_mid;
            if cp > 0.0 {
                cp / 3.0
            } else {
                cp / 10.0
            }
        };

        let noise_val = if (-2.0..=2.0).contains(&gradient) {
            if barrier_noise_val.is_nan() {
                let v = self
                    .gen_ref
                    .eval_node_cached(OW_ROOT_AQ_BARRIER, block_x, block_y, block_z, eval_cache)
                    as f64;
                *barrier_noise_val = v;
                v
            } else {
                *barrier_noise_val
            }
        } else {
            0.0
        };
        2.0 * (noise_val + gradient)
    }

    fn get_aquifer_status(&mut self, index: usize, eval_cache: &mut EvalCache) -> FluidStatus {
        if let Some(st) = self.aquifer_cache[index] {
            return st;
        }
        let pos = BlockPos::from_long(self.aquifer_locations[index]);
        let st = self.compute_fluid(pos.x, pos.y, pos.z, eval_cache);
        self.aquifer_cache[index] = Some(st);
        st
    }

    fn compute_fluid(&mut self, x: i32, y: i32, z: i32, eval_cache: &mut EvalCache) -> FluidStatus {
        let global = self.global_fluid(y);
        let mut lowest_surf = i32::MAX;
        let top_cell = y + 12;
        let bot_cell = y - 12;
        let mut center_under_global = false;

        for (ox, oz) in SURFACE_OFFSETS_IN_CHUNKS {
            let sx = x + (ox << 4);
            let sz = z + (oz << 4);
            let surf = self.surface_level(sx, sz, eval_cache);
            let adj = surf + 8;
            let start = ox == 0 && oz == 0;
            if start && bot_cell > adj {
                return global;
            }
            let pokes_above = top_cell > adj;
            if pokes_above || start {
                let g_at_surf = self.global_fluid(adj);
                if g_at_surf.at(adj) != BlockStateId::AIR.0 {
                    if start {
                        center_under_global = true;
                    }
                    if pokes_above {
                        return g_at_surf;
                    }
                }
            }
            lowest_surf = lowest_surf.min(surf);
        }

        let fluid_level =
            self.compute_surface_level(x, y, z, global, lowest_surf, center_under_global, eval_cache);
        let fluid_state = self.compute_fluid_type(x, y, z, global, fluid_level, eval_cache);
        FluidStatus {
            fluid_level,
            fluid_state,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn compute_surface_level(
        &self,
        x: i32,
        y: i32,
        z: i32,
        global: FluidStatus,
        lowest_surf: i32,
        center_under_global: bool,
        eval_cache: &mut EvalCache,
    ) -> i32 {
        let excl = self
            .gen_ref
            .eval_node_cached(OW_ROOT_AQ_EXCLUSION, x, y, z, eval_cache) as f64;
        let (part_flood, full_flood) = if excl > 0.0 {
            (-1.0, -1.0)
        } else {
            let dist_below = (lowest_surf + 8) - y;
            let factor = if center_under_global {
                clamped_map_f64(dist_below as f64, 0.0, 64.0, 1.0, 0.0)
            } else {
                0.0
            };
            let nval = (self
                .gen_ref
                .eval_node_cached(OW_ROOT_AQ_FLOODEDNESS, x, y, z, eval_cache) as f64)
                .clamp(-1.0, 1.0);
            let full_th = map_f64(factor, 1.0, 0.0, -0.3, 0.8);
            let part_th = map_f64(factor, 1.0, 0.0, -0.8, 0.4);
            (nval - part_th, nval - full_th)
        };

        if full_flood > 0.0 {
            global.fluid_level
        } else if part_flood > 0.0 {
            let cx = floor_div(x, 16);
            let cy = floor_div(y, 40);
            let cz = floor_div(z, 16);
            let mid_y = cy * 40 + 20;
            let spread = (self
                .gen_ref
                .eval_node_cached(OW_ROOT_AQ_SPREAD, cx, cy, cz, eval_cache)
                * 10.0_f32) as f64;
            let q = quantize_i32(spread, 3);
            lowest_surf.min(mid_y + q)
        } else {
            WAY_BELOW_MIN_Y
        }
    }

    fn compute_fluid_type(
        &self,
        x: i32,
        y: i32,
        z: i32,
        global: FluidStatus,
        fluid_level: i32,
        eval_cache: &mut EvalCache,
    ) -> u32 {
        let mut ftype = global.fluid_state;
        if fluid_level <= -10 && fluid_level != WAY_BELOW_MIN_Y && global.fluid_state != self.lava_state {
            let cx = floor_div(x, 64);
            let cy = floor_div(y, 40);
            let cz = floor_div(z, 64);
            let lava_val = self
                .gen_ref
                .eval_node_cached(OW_ROOT_AQ_LAVA, cx, cy, cz, eval_cache) as f64;
            if lava_val.abs() > 0.3 {
                ftype = self.lava_state;
            }
        }
        ftype
    }
}
