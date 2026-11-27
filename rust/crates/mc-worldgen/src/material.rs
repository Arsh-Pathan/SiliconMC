use crate::density::{EvalCache, OverworldGenerator};
use crate::generated_overworld::{
    OW_BIOME_TEMPS, OW_MAT_CONDS, OW_MAT_RANDOM_NAMES, OW_MAT_RULES, OW_MATERIAL_ROOT,
    OW_NOISE_CLAY_BANDS_OFFSET_IDX, OW_NOISE_SURFACE_IDX, OW_NOISE_SURFACE_SECONDARY_IDX,
    OW_ROOT_CHUNK_SURFACE_LEVEL,
};
use mc_data::{BlockStateId, default_state_id};
use mc_math::{RandomSource, XoroshiroPositionalRandomFactory, floor_f32};
use mc_world::{ChunkSection, OVERWORLD_MIN_SECTION_Y, OVERWORLD_SECTION_COUNT};

#[derive(Debug, Clone, Copy)]
pub enum MatCond {
    AbovePreliminarySurface,
    Hole,
    Steep,
    Temperature,
    Not(usize),
    BiomeMask(u64),
    StoneDepth {
        offset: i32,
        add_surface_depth: bool,
        secondary_depth_range: i32,
        ceiling: bool,
    },
    Water {
        offset: i32,
        surface_depth_multiplier: i32,
        add_stone_depth: bool,
    },
    YAbove {
        anchor_y: i32,
        surface_depth_multiplier: i32,
        add_stone_depth: bool,
    },
    VerticalGradient {
        random_idx: usize,
        true_at_and_below: i32,
        false_at_and_above: i32,
    },
    NoiseThreshold {
        noise_idx: usize,
        min_threshold: f64,
        max_threshold: f64,
        is_3d: bool,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum MatRule {
    Block(u32),
    Bandlands,
    Condition {
        cond: usize,
        then_run: usize,
    },
    Sequence(&'static [usize]),
    OreVein {
        ore_block: u32,
        raw_ore_block: u32,
        filler_block: u32,
        raw_ore_chance: f32,
        density_node: usize,
        richness_node: usize,
        filler_gap_node: usize,
    },
}

/// Vanilla Minecraft 26.3 `MaterialSystem` runtime state (clay bands + named positional randoms).
pub struct MaterialSystemState {
    clay_bands: [u32; 192],
    noise_random: XoroshiroPositionalRandomFactory,
    ore_random: XoroshiroPositionalRandomFactory,
    named_randoms: Vec<XoroshiroPositionalRandomFactory>,
}

impl MaterialSystemState {
    pub fn new(world_random: &XoroshiroPositionalRandomFactory) -> Self {
        let mut band_rng = world_random.from_hash_of("minecraft:clay_bands");
        let clay_bands = generate_clay_bands(&mut band_rng);
        let ore_random = world_random.from_hash_of("minecraft:ore").fork_positional();
        let named_randoms = OW_MAT_RANDOM_NAMES
            .iter()
            .map(|&name| world_random.from_hash_of(name).fork_positional())
            .collect();
        Self {
            clay_bands,
            noise_random: *world_random,
            ore_random,
            named_randoms,
        }
    }

    pub fn apply_to_chunk(
        &self,
        gen_ref: &OverworldGenerator,
        base_x: i32,
        base_z: i32,
        sec_blocks: &mut [Vec<u32>],
        quart_biomes_ow_idx: &[u16], // [sec_idx * 64 + (qy<<4)|(qz<<2)|qx]
        eval_cache: &mut EvalCache,
    ) {
        let min_y = OVERWORLD_MIN_SECTION_Y << 4;
        let height = OVERWORLD_SECTION_COUNT * 16;

        // Compute WORLD_SURFACE_WG heightmap per (x, z)
        let mut heights = [min_y; 256];
        for lz in 0..16 {
            for lx in 0..16 {
                let mut h = min_y;
                for y_rel in (0..height).rev() {
                    let s = sec_blocks[y_rel >> 4][ChunkSection::block_index(lx, y_rel & 15, lz)];
                    if !BlockStateId(s).is_air() {
                        h = min_y + y_rel as i32 + 1;
                        break;
                    }
                }
                heights[lz * 16 + lx] = h;
            }
        }

        let get_block = |blocks: &[Vec<u32>], lx: usize, world_y: i32, lz: usize| -> u32 {
            let y_rel = world_y - min_y;
            if y_rel < 0 || (y_rel as usize) >= height {
                return BlockStateId::AIR.0;
            }
            blocks[(y_rel as usize) >> 4][ChunkSection::block_index(lx, (y_rel as usize) & 15, lz)]
        };

        for lx in 0..16 {
            for lz in 0..16 {
                let block_x = base_x + lx as i32;
                let block_z = base_z + lz as i32;
                let top_y = (heights[lz * 16 + lx]).min(min_y + height as i32 - 1);

                let grad_x = heights[lz * 16 + (lx + 1).min(15)]
                    - heights[lz * 16 + lx.saturating_sub(1)];
                let grad_z = heights[(lz + 1).min(15) * 16 + lx]
                    - heights[lz.saturating_sub(1) * 16 + lx];

                let surf_noise = gen_ref.sample_noise_3d(
                    OW_NOISE_SURFACE_IDX,
                    block_x as f64,
                    0.0,
                    block_z as f64,
                ) as f64;
                let mut col_rng = self.noise_random.at(block_x, 0, block_z);
                let surface_depth =
                    (surf_noise * 2.75 + 3.0 + col_rng.next_double() * 0.25) as i32;
                let surface_secondary = gen_ref.sample_noise_3d(
                    OW_NOISE_SURFACE_SECONDARY_IDX,
                    block_x as f64,
                    0.0,
                    block_z as f64,
                ) as f64;
                let prelim_surf = floor_f32(gen_ref.eval_node_cached(
                    OW_ROOT_CHUNK_SURFACE_LEVEL,
                    block_x,
                    0,
                    block_z,
                    eval_cache,
                ));
                let min_surface_level = prelim_surf + surface_depth - 8;

                let mut stone_above = 0i32;
                let mut water_height = i32::MIN;
                let mut next_ceiling_y = i32::MAX;

                for y in (min_y..=top_y).rev() {
                    let old_id = get_block(sec_blocks, lx, y, lz);
                    let old_props = BlockStateId(old_id).props();
                    if old_props.is_air() {
                        stone_above = 0;
                        water_height = i32::MIN;
                    } else if old_props.has_fluid() {
                        if water_height == i32::MIN {
                            water_height = y + 1;
                        }
                    } else {
                        if next_ceiling_y >= y {
                            next_ceiling_y = min_y - 1024;
                            for ly in (min_y - 1..y).rev() {
                                let ns = BlockStateId(get_block(sec_blocks, lx, ly, lz)).props();
                                if ns.is_air() || ns.has_fluid() {
                                    next_ceiling_y = ly + 1;
                                    break;
                                }
                            }
                        }
                        stone_above += 1;
                        let stone_below = y - next_ceiling_y + 1;

                        let y_rel = (y - min_y) as usize;
                        let sec_idx = y_rel >> 4;
                        let qy = (y_rel & 15) >> 2;
                        let qz = lz >> 2;
                        let qx = lx >> 2;
                        let biome_ow_idx =
                            quart_biomes_ow_idx[sec_idx * 64 + ((qy << 4) | (qz << 2) | qx)]
                                as usize;

                        let col = ColumnCtx {
                            block_x,
                            block_y: y,
                            block_z,
                            grad_x,
                            grad_z,
                            surface_depth,
                            surface_secondary,
                            min_surface_level,
                            stone_above,
                            stone_below,
                            water_height,
                            biome_ow_idx,
                        };
                        if let Some(new_state) =
                            self.eval_rule(OW_MATERIAL_ROOT, &col, gen_ref, eval_cache)
                        {
                            sec_blocks[sec_idx][ChunkSection::block_index(lx, y_rel & 15, lz)] =
                                new_state;
                        }
                    }
                }
            }
        }
    }

    fn eval_rule(
        &self,
        rule_id: usize,
        col: &ColumnCtx,
        gen_ref: &OverworldGenerator,
        eval_cache: &mut EvalCache,
    ) -> Option<u32> {
        match OW_MAT_RULES[rule_id] {
            MatRule::Block(state_id) => Some(state_id),
            MatRule::Bandlands => {
                let off = (gen_ref.sample_noise_3d(
                    OW_NOISE_CLAY_BANDS_OFFSET_IDX,
                    col.block_x as f64,
                    0.0,
                    col.block_z as f64,
                ) * 4.0_f32)
                    .round() as i32;
                let idx = (col.block_y + off).rem_euclid(self.clay_bands.len() as i32) as usize;
                Some(self.clay_bands[idx])
            }
            MatRule::Condition { cond, then_run } => {
                if self.eval_cond(cond, col, gen_ref) {
                    self.eval_rule(then_run, col, gen_ref, eval_cache)
                } else {
                    None
                }
            }
            MatRule::Sequence(children) => {
                for &child in children {
                    if let Some(st) = self.eval_rule(child, col, gen_ref, eval_cache) {
                        return Some(st);
                    }
                }
                None
            }
            MatRule::OreVein {
                ore_block,
                raw_ore_block,
                filler_block,
                raw_ore_chance,
                density_node,
                richness_node,
                filler_gap_node,
            } => {
                let dens = gen_ref.eval_node_cached(
                    density_node,
                    col.block_x,
                    col.block_y,
                    col.block_z,
                    eval_cache,
                );
                if dens <= 0.0_f32 {
                    return None;
                }
                let mut rng = self.ore_random.at(col.block_x, col.block_y, col.block_z);
                if rng.next_float() > dens {
                    return None;
                }
                let rich = gen_ref.eval_node_cached(
                    richness_node,
                    col.block_x,
                    col.block_y,
                    col.block_z,
                    eval_cache,
                );
                if rng.next_float() < rich
                    && gen_ref.eval_node_cached(
                        filler_gap_node,
                        col.block_x,
                        col.block_y,
                        col.block_z,
                        eval_cache,
                    ) < 0.0_f32
                {
                    Some(if rng.next_float() < raw_ore_chance {
                        raw_ore_block
                    } else {
                        ore_block
                    })
                } else {
                    Some(filler_block)
                }
            }
        }
    }

    fn eval_cond(&self, cond_id: usize, col: &ColumnCtx, gen_ref: &OverworldGenerator) -> bool {
        match OW_MAT_CONDS[cond_id] {
            MatCond::AbovePreliminarySurface => col.block_y >= col.min_surface_level,
            MatCond::Hole => col.surface_depth <= 0,
            MatCond::Steep => col.grad_x <= -4 || col.grad_z >= 4,
            MatCond::Temperature => {
                let base_temp = OW_BIOME_TEMPS.get(col.biome_ow_idx).copied().unwrap_or(0.5);
                let adjusted = if col.block_y > 80 {
                    base_temp - ((col.block_y - 80) as f32) * 0.00125_f32
                } else {
                    base_temp
                };
                adjusted < 0.15_f32
            }
            MatCond::Not(inner) => !self.eval_cond(inner, col, gen_ref),
            MatCond::BiomeMask(mask) => ((mask >> col.biome_ow_idx) & 1) != 0,
            MatCond::StoneDepth {
                offset,
                add_surface_depth,
                secondary_depth_range,
                ceiling,
            } => {
                let sd = if ceiling {
                    col.stone_below
                } else {
                    col.stone_above
                };
                let surf = if add_surface_depth {
                    col.surface_depth
                } else {
                    0
                };
                let sec = if secondary_depth_range == 0 {
                    0
                } else {
                    let t = (col.surface_secondary - (-1.0)) / 2.0;
                    (t * (secondary_depth_range as f64)) as i32
                };
                sd <= 1 + offset + surf + sec
            }
            MatCond::Water {
                offset,
                surface_depth_multiplier,
                add_stone_depth,
            } => {
                col.water_height == i32::MIN
                    || col.block_y + if add_stone_depth { col.stone_above } else { 0 }
                        >= col.water_height + offset + col.surface_depth * surface_depth_multiplier
            }
            MatCond::YAbove {
                anchor_y,
                surface_depth_multiplier,
                add_stone_depth,
            } => {
                col.block_y + if add_stone_depth { col.stone_above } else { 0 }
                    >= anchor_y + col.surface_depth * surface_depth_multiplier
            }
            MatCond::VerticalGradient {
                random_idx,
                true_at_and_below,
                false_at_and_above,
            } => {
                if col.block_y <= true_at_and_below {
                    return true;
                }
                if col.block_y >= false_at_and_above {
                    return false;
                }
                let prob = ((col.block_y - true_at_and_below) as f64)
                    / ((false_at_and_above - true_at_and_below) as f64);
                let p = 1.0 - prob;
                let mut rng =
                    self.named_randoms[random_idx].at(col.block_x, col.block_y, col.block_z);
                (rng.next_float() as f64) < p
            }
            MatCond::NoiseThreshold {
                noise_idx,
                min_threshold,
                max_threshold,
                is_3d,
            } => {
                let y = if is_3d { col.block_y as f64 } else { 0.0 };
                let v = gen_ref.sample_noise_3d(
                    noise_idx,
                    col.block_x as f64,
                    y,
                    col.block_z as f64,
                ) as f64;
                v >= min_threshold && v <= max_threshold
            }
        }
    }
}

struct ColumnCtx {
    block_x: i32,
    block_y: i32,
    block_z: i32,
    grad_x: i32,
    grad_z: i32,
    surface_depth: i32,
    surface_secondary: f64,
    min_surface_level: i32,
    stone_above: i32,
    stone_below: i32,
    water_height: i32,
    biome_ow_idx: usize,
}

fn generate_clay_bands(rng: &mut impl RandomSource) -> [u32; 192] {
    let terracotta = default_state_id("minecraft:terracotta")
        .unwrap_or(BlockStateId::STONE)
        .0;
    let orange = default_state_id("minecraft:orange_terracotta")
        .unwrap_or(terracotta_id(terracotta))
        .0;
    let yellow = default_state_id("minecraft:yellow_terracotta")
        .unwrap_or(terracotta_id(terracotta))
        .0;
    let brown = default_state_id("minecraft:brown_terracotta")
        .unwrap_or(terracotta_id(terracotta))
        .0;
    let red = default_state_id("minecraft:red_terracotta")
        .unwrap_or(terracotta_id(terracotta))
        .0;
    let white = default_state_id("minecraft:white_terracotta")
        .unwrap_or(terracotta_id(terracotta))
        .0;
    let light_gray = default_state_id("minecraft:light_gray_terracotta")
        .unwrap_or(terracotta_id(terracotta))
        .0;

    let mut bands = [terracotta; 192];
    let mut i = 0usize;
    while i < bands.len() {
        i += (rng.next_int_bounded(5) + 1) as usize;
        if i < bands.len() {
            bands[i] = orange;
        }
        i += 1;
    }

    let make_bands = |rng: &mut dyn FnMut(i32) -> i32, bands: &mut [u32; 192], base_w: usize, st: u32| {
        let count = 6 + rng(10) as usize; // 6..=15
        for _ in 0..count {
            let width = base_w + rng(3) as usize;
            let start = rng(bands.len() as i32) as usize;
            for p in 0..width {
                if start + p < bands.len() {
                    bands[start + p] = st;
                }
            }
        }
    };

    let mut next_b = |bound: i32| rng.next_int_bounded(bound);
    make_bands(&mut next_b, &mut bands, 1, yellow);
    make_bands(&mut next_b, &mut bands, 2, brown);
    make_bands(&mut next_b, &mut bands, 1, red);

    let white_count = 9 + rng.next_int_bounded(7) as usize; // 9..=15
    let mut placed = 0usize;
    let mut start = 0usize;
    while placed < white_count && start < bands.len() {
        bands[start] = white;
        if start > 1 && rng.next_boolean() {
            bands[start - 1] = light_gray;
        }
        if start + 1 < bands.len() && rng.next_boolean() {
            bands[start + 1] = light_gray;
        }
        start += (rng.next_int_bounded(16) + 4) as usize;
        placed += 1;
    }
    bands
}

#[inline]
const fn terracotta_id(id: u32) -> BlockStateId {
    BlockStateId(id)
}
