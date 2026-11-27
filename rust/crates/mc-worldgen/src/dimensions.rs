use crate::climate::{ClimateParameter, TargetPoint};
use crate::generated_nether::{
    NETHER_BIOME_NAMES, NETHER_CLIMATE_ENTRIES, NETHER_MAT_CONDS, NETHER_MAT_RANDOM_NAMES,
    NETHER_MAT_RULES, NETHER_MATERIAL_ROOT, NETHER_NOISE_IDS, NETHER_NOISE_SURFACE_IDX,
    NETHER_NOISE_SURFACE_SECONDARY_IDX,
};
use crate::material::{MatCond, MatRule};
use crate::noise::{BlendedNoise, NoiseStack, NormalNoise, SimplexNoise};
use mc_data::noises::vanilla_noise_spec;
use mc_data::{BlockStateId, LoadedDatapacks, TOTAL_BLOCK_STATES, default_state_id};
use mc_math::{
    ChunkPos, LegacyPositionalRandomFactory, LegacyRandomSource, RandomSource, floor_f32, lerp3_f32,
};
use mc_world::{Chunk, ChunkSection, LightEngine, PaletteStrategy, PalettedStorage};
use std::collections::BTreeMap;

pub const NETHER_MIN_SECTION_Y: i32 = 0;
pub const NETHER_SECTION_COUNT: usize = 8; // Y = 0..128
pub const END_MIN_SECTION_Y: i32 = 0;
pub const END_SECTION_COUNT: usize = 8; // Y = 0..128

#[inline]
fn gradient_y(y: i32, from_y: i32, to_y: i32, from_v: f32, to_v: f32) -> f32 {
    if y <= from_y {
        from_v
    } else if y >= to_y {
        to_v
    } else {
        let t = (y - from_y) as f32 / (to_y - from_y) as f32;
        from_v + t * (to_v - from_v)
    }
}

#[inline]
fn squeeze_f32(x: f32) -> f32 {
    let c = x.clamp(-1.0, 1.0);
    c / 2.0 - c * c * c / 24.0
}

#[inline]
fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + t * (b - a)
}

/// Complete Minecraft 26.3 Nether terrain, biome, and MaterialSystem generator.
pub struct NetherGenerator {
    base_noise: BlendedNoise,
    temp_noise: NoiseStack,
    veg_noise: NoiseStack,
    mat_noises: Vec<NoiseStack>,
    mat_randoms: Vec<LegacyPositionalRandomFactory>,
    netherrack: u32,
    lava: u32,
}

impl NetherGenerator {
    pub fn new(seed: i64) -> Self {
        let mut base_rng = LegacyRandomSource::new(seed);
        let base_noise = BlendedNoise::new(&mut base_rng, 0.25, 0.375, 80.0, 60.0, 8.0);

        let temp_spec = vanilla_noise_spec("minecraft:nether/temperature").unwrap();
        let veg_spec = vanilla_noise_spec("minecraft:nether/vegetation").unwrap();
        let temp_noise = NormalNoise::from_spec(temp_spec)
            .instantiate_for_legacy_nether_biome(temp_spec, &mut LegacyRandomSource::new(seed));
        let veg_noise = NormalNoise::from_spec(veg_spec).instantiate_for_legacy_nether_biome(
            veg_spec,
            &mut LegacyRandomSource::new(seed.wrapping_add(1)),
        );

        let world_pos_rng = LegacyRandomSource::new(seed).fork_positional();
        let mat_noises = NETHER_NOISE_IDS
            .iter()
            .map(|&nid| {
                let nn = NormalNoise::from_vanilla_id(nid).unwrap();
                let mut r = world_pos_rng.from_hash_of(nid);
                nn.instantiate_legacy(&mut r)
            })
            .collect();
        let mat_randoms = NETHER_MAT_RANDOM_NAMES
            .iter()
            .map(|&name| world_pos_rng.from_hash_of(name).fork_positional())
            .collect();

        let netherrack = default_state_id("minecraft:netherrack")
            .unwrap_or(BlockStateId::STONE)
            .0;
        let lava = default_state_id("minecraft:lava")
            .unwrap_or(BlockStateId::AIR)
            .0;

        Self {
            base_noise,
            temp_noise,
            veg_noise,
            mat_noises,
            mat_randoms,
            netherrack,
            lava,
        }
    }

    /// Exact Minecraft 26.3 Nether pre-interpolation density at `(x, y, z)`.
    #[inline]
    fn sample_cell_density(&self, x: i32, y: i32, z: i32) -> f32 {
        let base = self.base_noise.sample(x, y, z);
        let a_top = gradient_y(y, 104, 128, 1.0, 0.0);
        let top_lerped = lerp_f32(0.9375, base, a_top);
        let a_bot = gradient_y(y, -8, 24, 0.0, 1.0);
        lerp_f32(2.5, top_lerped, a_bot) * 0.64
    }

    pub fn sample_biome_idx(&self, x: i32, z: i32) -> u16 {
        let t = self.temp_noise.get_3d(x as f64 * 0.25, 0.0, z as f64 * 0.25);
        let v = self.veg_noise.get_3d(x as f64 * 0.25, 0.0, z as f64 * 0.25);
        let target = TargetPoint::from_floats(t, v, 0.0, 0.0, 0.0, 0.0).to_array();
        let mut best_dist = i64::MAX;
        let mut best_biome = 0u16;
        for entry in &NETHER_CLIMATE_ENTRIES {
            let mut dist = 0i64;
            for d in 0..7 {
                let p = ClimateParameter::new(entry.params[d].0, entry.params[d].1);
                let delta = p.distance(target[d]);
                dist += delta * delta;
            }
            if dist < best_dist {
                best_dist = dist;
                best_biome = entry.biome_idx;
            }
        }
        best_biome
    }

    pub fn sample_biome_name(&self, x: i32, z: i32) -> &'static str {
        NETHER_BIOME_NAMES[self.sample_biome_idx(x, z) as usize]
    }

    fn eval_cond(
        &self,
        cond_idx: usize,
        x: i32,
        y: i32,
        z: i32,
        biome_idx: u16,
        surface_depth: i32,
        stone_depth_above: i32,
        stone_depth_below: i32,
    ) -> bool {
        match NETHER_MAT_CONDS[cond_idx] {
            MatCond::Hole => surface_depth <= 0,
            MatCond::Not(inner) => !self.eval_cond(
                inner,
                x,
                y,
                z,
                biome_idx,
                surface_depth,
                stone_depth_above,
                stone_depth_below,
            ),
            MatCond::BiomeMask(mask) => (mask & (1u64 << biome_idx)) != 0,
            MatCond::StoneDepth {
                offset,
                add_surface_depth,
                secondary_depth_range,
                ceiling,
            } => {
                let depth = if ceiling {
                    stone_depth_below
                } else {
                    stone_depth_above
                };
                let sec = if secondary_depth_range == 0 {
                    0
                } else {
                    let s = self.mat_noises[NETHER_NOISE_SURFACE_SECONDARY_IDX]
                        .get_3d(x as f64, 0.0, z as f64);
                    let mapped = (s + 0.5).clamp(0.0, 1.0);
                    floor_f32(mapped * secondary_depth_range as f32)
                };
                depth <= 1 + offset + if add_surface_depth { surface_depth } else { 0 } + sec
            }
            MatCond::YAbove {
                anchor_y,
                surface_depth_multiplier,
                add_stone_depth,
            } => {
                y + if add_stone_depth { stone_depth_above } else { 0 }
                    >= anchor_y + surface_depth * surface_depth_multiplier
            }
            MatCond::VerticalGradient {
                random_idx,
                true_at_and_below,
                false_at_and_above,
            } => {
                if y <= true_at_and_below {
                    true
                } else if y >= false_at_and_above {
                    false
                } else {
                    let p = (false_at_and_above - y) as f32
                        / (false_at_and_above - true_at_and_below) as f32;
                    self.mat_randoms[random_idx].at(x, y, z).next_float() < p
                }
            }
            MatCond::NoiseThreshold {
                noise_idx,
                min_threshold,
                max_threshold,
                is_3d,
            } => {
                let v = if is_3d {
                    self.mat_noises[noise_idx].get_3d(x as f64, y as f64, z as f64) as f64
                } else {
                    self.mat_noises[noise_idx].get_3d(x as f64, 0.0, z as f64) as f64
                };
                v >= min_threshold && v <= max_threshold
            }
            _ => false,
        }
    }

    fn eval_rule(
        &self,
        rule_idx: usize,
        x: i32,
        y: i32,
        z: i32,
        biome_idx: u16,
        surface_depth: i32,
        stone_depth_above: i32,
        stone_depth_below: i32,
    ) -> Option<u32> {
        match NETHER_MAT_RULES[rule_idx] {
            MatRule::Block(state) => Some(state),
            MatRule::Condition { cond, then_run } => {
                if self.eval_cond(
                    cond,
                    x,
                    y,
                    z,
                    biome_idx,
                    surface_depth,
                    stone_depth_above,
                    stone_depth_below,
                ) {
                    self.eval_rule(
                        then_run,
                        x,
                        y,
                        z,
                        biome_idx,
                        surface_depth,
                        stone_depth_above,
                        stone_depth_below,
                    )
                } else {
                    None
                }
            }
            MatRule::Sequence(children) => {
                for &child in children {
                    if let Some(st) = self.eval_rule(
                        child,
                        x,
                        y,
                        z,
                        biome_idx,
                        surface_depth,
                        stone_depth_above,
                        stone_depth_below,
                    ) {
                        return Some(st);
                    }
                }
                None
            }
            _ => None,
        }
    }

    pub fn generate_chunk(&self, pos: ChunkPos, datapacks: &LoadedDatapacks) -> Chunk {
        let block_strat = PaletteStrategy::for_block_states(TOTAL_BLOCK_STATES);
        let biome_strat = PaletteStrategy::for_biomes(datapacks.biome_count());
        let base_x = pos.x << 4;
        let base_z = pos.z << 4;

        // 1. 4x8x4 cell grid (5 x 17 x 5 = 425 cells for 16x128x16 chunk!)
        let mut grid = vec![0.0_f32; 5 * 17 * 5];
        for cz in 0..5 {
            for cx in 0..5 {
                let wx = base_x + (cx as i32) * 4;
                let wz = base_z + (cz as i32) * 4;
                for cy in 0..17 {
                    let wy = (cy as i32) * 8;
                    grid[(cz * 5 + cx) * 17 + cy] = self.sample_cell_density(wx, wy, wz);
                }
            }
        }

        // 2. Trilinear interpolation -> Netherrack / Lava (y < 32) / Air
        let mut sec_blocks: Vec<Vec<u32>> = (0..NETHER_SECTION_COUNT)
            .map(|_| vec![BlockStateId::AIR.0; 4096])
            .collect();

        for lz in 0..16 {
            let cz = lz >> 2;
            let fz = (lz & 3) as f32 * 0.25;
            for lx in 0..16 {
                let cx = lx >> 2;
                let fx = (lx & 3) as f32 * 0.25;
                let i00 = (cz * 5 + cx) * 17;
                let i10 = (cz * 5 + cx + 1) * 17;
                let i01 = ((cz + 1) * 5 + cx) * 17;
                let i11 = ((cz + 1) * 5 + cx + 1) * 17;
                for y in 0..128 {
                    let cy = y >> 3;
                    let fy = (y & 7) as f32 * 0.125;
                    let d = squeeze_f32(lerp3_f32(
                        fx,
                        fy,
                        fz,
                        grid[i00 + cy],
                        grid[i10 + cy],
                        grid[i00 + cy + 1],
                        grid[i10 + cy + 1],
                        grid[i01 + cy],
                        grid[i11 + cy],
                        grid[i01 + cy + 1],
                        grid[i11 + cy + 1],
                    ));
                    let st = if d > 0.0 {
                        self.netherrack
                    } else if y < 32 {
                        self.lava
                    } else {
                        BlockStateId::AIR.0
                    };
                    sec_blocks[y >> 4][ChunkSection::block_index(lx, y & 15, lz)] = st;
                }
            }
        }

        // 3. Biomes (4x4x4 per section; Nether biomes are 2D across Y)
        let mut col_biomes = [0u16; 16];
        for qz in 0..4 {
            for qx in 0..4 {
                col_biomes[qz * 4 + qx] =
                    self.sample_biome_idx(base_x + (qx as i32) * 4, base_z + (qz as i32) * 4);
            }
        }
        let mut sec_biomes_flat = Vec::with_capacity(NETHER_SECTION_COUNT);
        for _ in 0..NETHER_SECTION_COUNT {
            let mut bflat = vec![0u32; 64];
            for qy in 0..4 {
                for qz in 0..4 {
                    for qx in 0..4 {
                        let bidx = col_biomes[qz * 4 + qx];
                        let reg_id = datapacks.biome_id(NETHER_BIOME_NAMES[bidx as usize]);
                        bflat[(qy << 4) | (qz << 2) | qx] = reg_id;
                    }
                }
            }
            sec_biomes_flat.push(bflat);
        }

        // 4. Apply compiled Nether MaterialRules (bedrock floor/roof, soul sand, basalt, nylium, gravel)
        for lz in 0..16 {
            let wz = base_z + lz as i32;
            for lx in 0..16 {
                let wx = base_x + lx as i32;
                let biome_idx = col_biomes[(lz >> 2) * 4 + (lx >> 2)];
                let sd_noise = self.mat_noises[NETHER_NOISE_SURFACE_IDX]
                    .get_3d(wx as f64, 0.0, wz as f64);
                let surface_depth = floor_f32(sd_noise * 2.75 + 3.0);

                // Compute stone_depth_above (descending) and stone_depth_below (ascending)
                let mut above = [0i32; 128];
                let mut below = [0i32; 128];
                let mut run = 0i32;
                for y in (0..128).rev() {
                    let s = sec_blocks[y >> 4][ChunkSection::block_index(lx, y & 15, lz)];
                    if s == self.netherrack {
                        run += 1;
                        above[y] = run;
                    } else {
                        run = 0;
                    }
                }
                run = 0;
                for y in 0..128 {
                    let s = sec_blocks[y >> 4][ChunkSection::block_index(lx, y & 15, lz)];
                    if s == self.netherrack {
                        run += 1;
                        below[y] = run;
                    } else {
                        run = 0;
                    }
                }

                for y in 0..128 {
                    let s = sec_blocks[y >> 4][ChunkSection::block_index(lx, y & 15, lz)];
                    if s != self.netherrack && y > 4 && y < 123 {
                        continue;
                    }
                    if let Some(rep) = self.eval_rule(
                        NETHER_MATERIAL_ROOT,
                        wx,
                        y as i32,
                        wz,
                        biome_idx,
                        surface_depth,
                        above[y],
                        below[y],
                    ) {
                        sec_blocks[y >> 4][ChunkSection::block_index(lx, y & 15, lz)] = rep;
                    }
                }
            }
        }

        let mut sections = Vec::with_capacity(NETHER_SECTION_COUNT);
        for (sec_idx, (blocks, bflat)) in sec_blocks
            .into_iter()
            .zip(sec_biomes_flat.into_iter())
            .enumerate()
        {
            let mut sec = ChunkSection {
                y: (NETHER_MIN_SECTION_Y + sec_idx as i32) as i8,
                non_empty_block_count: 0,
                fluid_count: 0,
                block_states: PalettedStorage::from_flat(&blocks, block_strat),
                biomes: PalettedStorage::from_flat(&bflat, biome_strat),
                sky_light: None,
                block_light: None,
            };
            sec.recalc_counts();
            sections.push(sec);
        }

        let mut chunk = Chunk {
            pos,
            min_section_y: NETHER_MIN_SECTION_Y,
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
        LightEngine::compute_initial_chunk_light(&mut chunk);
        chunk
    }
}

/// Complete Minecraft 26.3 The End terrain & biome generator (`EndIslandFunction` + `BlendedNoise`).
pub struct EndGenerator {
    base_noise: BlendedNoise,
    island_noise: SimplexNoise,
    end_stone: u32,
}

impl EndGenerator {
    pub fn new(seed: i64) -> Self {
        let mut base_rng = LegacyRandomSource::new(seed);
        let base_noise = BlendedNoise::new(&mut base_rng, 0.25, 0.25, 80.0, 160.0, 4.0);

        let mut island_rng = LegacyRandomSource::new(seed);
        for _ in 0..17292 {
            island_rng.next_int();
        }
        let island_noise = SimplexNoise::new(&mut island_rng, true);

        let end_stone = default_state_id("minecraft:end_stone")
            .unwrap_or(BlockStateId::STONE)
            .0;
        Self {
            base_noise,
            island_noise,
            end_stone,
        }
    }

    /// Exact port of `EndIslandFunction.getHeightValue` + `(val - 8.0) / 128.0`.
    pub fn sample_outer_islands(&self, block_x: i32, block_z: i32) -> f32 {
        let section_x = block_x / 8;
        let section_z = block_z / 8;
        let chunk_x = section_x / 2;
        let chunk_z = section_z / 2;
        let sub_x = section_x % 2;
        let sub_z = section_z % 2;
        let mut doffs = -100.0_f32;

        for xo in -12..=12 {
            for zo in -12..=12 {
                let tcx = (chunk_x + xo) as i64;
                let tcz = (chunk_z + zo) as i64;
                if tcx * tcx + tcz * tcz > 4096
                    && self.island_noise.get_2d(tcx as f64, tcz as f64) < -0.9_f32
                {
                    let island_size =
                        ((tcx as f32).abs() * 3439.0 + (tcz as f32).abs() * 147.0) % 13.0 + 9.0;
                    let xd = (sub_x - xo * 2) as f32;
                    let zd = (sub_z - zo * 2) as f32;
                    let new_doffs = (100.0 - (xd * xd + zd * zd).sqrt() * island_size)
                        .clamp(-100.0, 80.0);
                    doffs = doffs.max(new_doffs);
                }
            }
        }
        (doffs - 8.0) / 128.0
    }

    /// Exact port of `minecraft:end/islands` density function.
    pub fn sample_islands(&self, x: i32, z: i32) -> f32 {
        let dist = ((x as f32) * (x as f32) + (z as f32) * (z as f32)).sqrt();
        let main_island = ((100.0 - dist).clamp(-100.0, 80.0) - 8.0) * 0.0078125;
        main_island.max(self.sample_outer_islands(x, z))
    }

    #[inline]
    fn sample_cell_density(&self, x: i32, y: i32, z: i32, island_val: f32) -> f32 {
        let sloped_cheese = island_val + self.base_noise.sample(x, y, z);
        let a_top = gradient_y(y, 56, 312, 1.0, 0.0);
        let top_lerped = lerp_f32(-23.4375, sloped_cheese, a_top);
        let a_bot = gradient_y(y, 4, 32, 0.0, 1.0);
        lerp_f32(-0.234375, top_lerped, a_bot) * 0.64
    }

    /// Exact port of `TheEndBiomeSource.getNoiseBiome`.
    pub fn sample_biome_name(&self, block_x: i32, block_z: i32) -> &'static str {
        let cx = (block_x >> 4) as i64;
        let cz = (block_z >> 4) as i64;
        if cx * cx + cz * cz <= 4096 {
            return "minecraft:the_end";
        }
        let wx = ((block_x >> 4) * 2 + 1) * 8;
        let wz = ((block_z >> 4) * 2 + 1) * 8;
        let h = self.sample_islands(wx, wz) as f64;
        if h > 0.25 {
            "minecraft:end_highlands"
        } else if h >= -0.0625 {
            "minecraft:end_midlands"
        } else if h < -0.21875 {
            "minecraft:small_end_islands"
        } else {
            "minecraft:end_barrens"
        }
    }

    pub fn generate_chunk(&self, pos: ChunkPos, datapacks: &LoadedDatapacks) -> Chunk {
        let block_strat = PaletteStrategy::for_block_states(TOTAL_BLOCK_STATES);
        let biome_strat = PaletteStrategy::for_biomes(datapacks.biome_count());
        let base_x = pos.x << 4;
        let base_z = pos.z << 4;

        // 1. 8x4x8 cell grid (3 x 33 x 3 = 297 cells for 16x128x16 chunk!)
        let mut grid = vec![0.0_f32; 3 * 33 * 3];
        for cz in 0..3 {
            for cx in 0..3 {
                let wx = base_x + (cx as i32) * 8;
                let wz = base_z + (cz as i32) * 8;
                let island_val = self.sample_islands(wx, wz);
                for cy in 0..33 {
                    let wy = (cy as i32) * 4;
                    grid[(cz * 3 + cx) * 33 + cy] =
                        self.sample_cell_density(wx, wy, wz, island_val);
                }
            }
        }

        // 2. Trilinear interpolation -> end_stone or air
        let mut sec_blocks: Vec<Vec<u32>> = (0..END_SECTION_COUNT)
            .map(|_| vec![BlockStateId::AIR.0; 4096])
            .collect();

        for lz in 0..16 {
            let cz = lz >> 3;
            let fz = (lz & 7) as f32 * 0.125;
            for lx in 0..16 {
                let cx = lx >> 3;
                let fx = (lx & 7) as f32 * 0.125;
                let i00 = (cz * 3 + cx) * 33;
                let i10 = (cz * 3 + cx + 1) * 33;
                let i01 = ((cz + 1) * 3 + cx) * 33;
                let i11 = ((cz + 1) * 3 + cx + 1) * 33;
                for y in 0..128 {
                    let cy = y >> 2;
                    let fy = (y & 3) as f32 * 0.25;
                    let d = squeeze_f32(lerp3_f32(
                        fx,
                        fy,
                        fz,
                        grid[i00 + cy],
                        grid[i10 + cy],
                        grid[i00 + cy + 1],
                        grid[i10 + cy + 1],
                        grid[i01 + cy],
                        grid[i11 + cy],
                        grid[i01 + cy + 1],
                        grid[i11 + cy + 1],
                    ));
                    if d > 0.0 {
                        sec_blocks[y >> 4][ChunkSection::block_index(lx, y & 15, lz)] =
                            self.end_stone;
                    }
                }
            }
        }

        let bname = self.sample_biome_name(base_x, base_z);
        let biome_id = datapacks.biome_id(bname);
        let mut sections = Vec::with_capacity(END_SECTION_COUNT);
        for (sec_idx, blocks) in sec_blocks.into_iter().enumerate() {
            let mut sec = ChunkSection {
                y: (END_MIN_SECTION_Y + sec_idx as i32) as i8,
                non_empty_block_count: 0,
                fluid_count: 0,
                block_states: PalettedStorage::from_flat(&blocks, block_strat),
                biomes: PalettedStorage::single(biome_id, 64),
                sky_light: None,
                block_light: None,
            };
            sec.recalc_counts();
            sections.push(sec);
        }

        let _ = biome_strat;
        let mut chunk = Chunk {
            pos,
            min_section_y: END_MIN_SECTION_Y,
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
        LightEngine::compute_initial_chunk_light(&mut chunk);
        chunk
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_oracle::OracleClient;

    #[test]
    fn nether_and_end_generators_produce_valid_vanilla_chunks() {
        let data_dir = OracleClient::repo_root()
            .join("src")
            .join("main")
            .join("resources")
            .join("data");
        let datapacks = LoadedDatapacks::load_from_data_dir(&data_dir).unwrap();

        let nether = NetherGenerator::new(263_2026);
        let nc = nether.generate_chunk(ChunkPos::new(0, 0), &datapacks);
        assert_eq!(nc.sections.len(), 8);
        assert_eq!(nc.get_block(0, 0, 0), BlockStateId::BEDROCK);
        assert_eq!(nc.get_block(0, 127, 0), BlockStateId::BEDROCK);
        assert!(NETHER_BIOME_NAMES.contains(&nether.sample_biome_name(0, 0)));

        let end = EndGenerator::new(263_2026);
        let ec = end.generate_chunk(ChunkPos::new(0, 0), &datapacks);
        assert_eq!(ec.sections.len(), 8);
        assert_eq!(end.sample_biome_name(0, 0), "minecraft:the_end");
        // Central End island must have solid end_stone blocks around (0, 50..65, 0)
        let end_stone = default_state_id("minecraft:end_stone").unwrap();
        let solid_count: usize = (0..128)
            .filter(|&y| ec.get_block(0, y, 0) == end_stone)
            .count();
        assert!(solid_count > 10, "Expected main End island at (0,0), got {solid_count} end_stone");
        // Outer islands far from origin
        let outer = end.sample_biome_name(2000, 0);
        assert_ne!(outer, "minecraft:the_end");
    }
}
