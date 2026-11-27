use crate::generated_features::{OVERWORLD_BIOME_VEG, OVERWORLD_ORE_SPECS};
use mc_data::{BlockStateId, default_state_id};
use mc_math::{RandomSource, XoroshiroRandomSource};
use mc_world::{ChunkSection, OVERWORLD_MIN_SECTION_Y, OVERWORLD_SECTION_COUNT};

#[inline]
fn get_blk(sec_blocks: &[Vec<u32>], lx: i32, world_y: i32, lz: i32) -> Option<u32> {
    if !(0..16).contains(&lx) || !(0..16).contains(&lz) {
        return None;
    }
    let y_rel = world_y - (OVERWORLD_MIN_SECTION_Y << 4);
    if y_rel < 0 || (y_rel as usize) >= OVERWORLD_SECTION_COUNT * 16 {
        return None;
    }
    let yu = y_rel as usize;
    Some(sec_blocks[yu >> 4][ChunkSection::block_index(lx as usize, yu & 15, lz as usize)])
}

#[inline]
fn set_blk(sec_blocks: &mut [Vec<u32>], lx: i32, world_y: i32, lz: i32, state: u32) {
    if !(0..16).contains(&lx) || !(0..16).contains(&lz) {
        return;
    }
    let y_rel = world_y - (OVERWORLD_MIN_SECTION_Y << 4);
    if y_rel < 0 || (y_rel as usize) >= OVERWORLD_SECTION_COUNT * 16 {
        return;
    }
    let yu = y_rel as usize;
    sec_blocks[yu >> 4][ChunkSection::block_index(lx as usize, yu & 15, lz as usize)] = state;
}

/// Deterministic vanilla-style Overworld chunk decoration pass:
/// 1. All Python-compiled vanilla ore veins (coal, iron, copper, gold, redstone, lapis, diamond, emerald, stone variants)
/// 2. Biome-authentic trees (oak, birch, spruce, dark oak, acacia, jungle, cherry, swamp, desert cactus)
/// 3. Ground vegetation (short grass, poppies, dandelions, dead bushes)
pub fn decorate_overworld_chunk(
    world_seed: i64,
    base_x: i32,
    base_z: i32,
    sec_blocks: &mut [Vec<u32>],
    quart_biomes_ow_idx: &[u16],
) {
    let mut rng = XoroshiroRandomSource::new(
        world_seed
            ^ ((base_x as i64).wrapping_mul(341873128712_i64))
            ^ ((base_z as i64).wrapping_mul(132897987541_i64)),
    );

    let stone_id = BlockStateId::STONE.0;
    let deepslate_id = default_state_id("minecraft:deepslate").unwrap_or(BlockStateId::STONE).0;
    let grass_id = BlockStateId::GRASS_BLOCK.0;
    let dirt_id = BlockStateId::DIRT.0;
    let sand_id = default_state_id("minecraft:sand").unwrap_or(BlockStateId::DIRT).0;
    let red_sand_id = default_state_id("minecraft:red_sand").unwrap_or(BlockStateId(sand_id)).0;

    let min_y = OVERWORLD_MIN_SECTION_Y << 4;
    let max_y = min_y + (OVERWORLD_SECTION_COUNT as i32) * 16 - 1;

    // 1. Place compiled vanilla Overworld ore veins
    let center_biome = quart_biomes_ow_idx[8 * 64 + (2 << 2) + 2];
    for spec in &OVERWORLD_ORE_SPECS {
        if (spec.biome_mask & (1u64 << center_biome)) == 0 {
            continue;
        }
        if spec.rarity > 1 && rng.next_int_bounded(spec.rarity as i32) != 0 {
            continue;
        }
        let span = (spec.count_max.saturating_sub(spec.count_min)) as i32;
        let count = spec.count_min as i32
            + if span > 0 {
                rng.next_int_bounded(span + 1)
            } else {
                0
            };

        let y_lo = (spec.min_y as i32).max(min_y + 1);
        let y_hi = (spec.max_y as i32).min(max_y - 1);
        if y_lo >= y_hi {
            continue;
        }

        for _ in 0..count {
            let cx = rng.next_int_bounded(16);
            let cz = rng.next_int_bounded(16);
            let cy = if spec.trapezoid {
                let half = ((y_hi - y_lo) / 2).max(1);
                y_lo + rng.next_int_bounded(half + 1) + rng.next_int_bounded(half + 1)
            } else {
                y_lo + rng.next_int_bounded(y_hi - y_lo + 1)
            };

            let sz = (spec.size as i32).clamp(1, 32);
            let radius = ((sz as f32).cbrt() * 0.75).max(1.0);
            let ri = radius.ceil() as i32;
            let mut placed = 0;

            for dy in -ri..=ri {
                for dz in -ri..=ri {
                    for dx in -ri..=ri {
                        if placed >= sz {
                            break;
                        }
                        let dist2 = (dx * dx + dy * dy + dz * dz) as f32;
                        if dist2 > radius * radius + 0.5 {
                            continue;
                        }
                        let bx = cx + dx;
                        let by = cy + dy;
                        let bz = cz + dz;
                        let Some(cur) = get_blk(sec_blocks, bx, by, bz) else {
                            continue;
                        };
                        let replacement = if cur == stone_id && spec.stone_state != 0 {
                            spec.stone_state
                        } else if cur == deepslate_id && spec.deepslate_state != 0 {
                            spec.deepslate_state
                        } else {
                            continue;
                        };
                        if spec.discard_on_air > 0.0
                            && rng.next_float() < spec.discard_on_air
                            && is_air_exposed(sec_blocks, bx, by, bz)
                        {
                            continue;
                        }
                        set_blk(sec_blocks, bx, by, bz, replacement);
                        placed += 1;
                    }
                }
            }
        }
    }

    // 2. Compute surface height per column for vegetation & trees
    let mut surf_y = [min_y; 256];
    for lz in 0..16 {
        for lx in 0..16 {
            for y in (min_y..=max_y).rev() {
                let st = get_blk(sec_blocks, lx as i32, y, lz as i32).unwrap_or(0);
                if !BlockStateId(st).is_air() {
                    surf_y[lz * 16 + lx] = y;
                    break;
                }
            }
        }
    }

    let surf_biome_idx = |lx: usize, lz: usize, wy: i32| -> usize {
        let sec_idx = ((wy - min_y) >> 4).clamp(0, OVERWORLD_SECTION_COUNT as i32 - 1) as usize;
        let qy = (((wy - min_y) & 15) >> 2) as usize;
        let qz = (lz >> 2).min(3);
        let qx = (lx >> 2).min(3);
        quart_biomes_ow_idx[sec_idx * 64 + (qy << 4) + (qz << 2) + qx] as usize
    };

    let center_sy = surf_y[8 * 16 + 8];
    let veg = OVERWORLD_BIOME_VEG
        .get(surf_biome_idx(8, 8, center_sy))
        .copied()
        .unwrap_or(OVERWORLD_BIOME_VEG[0]);

    // 3. Place biome-authentic trees inside safe chunk interior (lx, lz in 2..14 so canopies stay inside chunk!)
    if veg.tree_style > 0 && veg.tree_density_x10 > 0 {
        let whole = (veg.tree_density_x10 / 10) as i32;
        let frac = (veg.tree_density_x10 % 10) as i32;
        let tree_count = whole + if rng.next_int_bounded(10) < frac { 1 } else { 0 };

        for _ in 0..tree_count.min(10) {
            let lx = 2 + rng.next_int_bounded(12);
            let lz = 2 + rng.next_int_bounded(12);
            let sy = surf_y[(lz as usize) * 16 + (lx as usize)];
            if sy <= 62 || sy >= max_y - 12 {
                continue;
            }
            let ground = get_blk(sec_blocks, lx, sy, lz).unwrap_or(0);
            if veg.tree_style == 9 {
                // Desert cactus
                if ground == sand_id || ground == red_sand_id {
                    let cactus = default_state_id("minecraft:cactus").unwrap_or(BlockStateId::AIR).0;
                    if cactus != 0 {
                        let h = 1 + rng.next_int_bounded(3);
                        for dy in 1..=h {
                            set_blk(sec_blocks, lx, sy + dy, lz, cactus);
                        }
                    }
                }
                continue;
            }
            if ground != grass_id && ground != dirt_id {
                continue;
            }
            let style = if veg.tree_style == 4 {
                if rng.next_int_bounded(3) == 0 { 2 } else { 1 }
            } else {
                veg.tree_style
            };
            place_tree(sec_blocks, &mut rng, lx, sy + 1, lz, style);
            set_blk(sec_blocks, lx, sy, lz, dirt_id);
        }
    }

    // 4. Ground flora (short_grass, dandelion, poppy, dead_bush)
    let short_grass = default_state_id("minecraft:short_grass").unwrap_or(BlockStateId::AIR).0;
    let dandelion = default_state_id("minecraft:dandelion").unwrap_or(BlockStateId::AIR).0;
    let poppy = default_state_id("minecraft:poppy").unwrap_or(BlockStateId::AIR).0;
    let dead_bush = default_state_id("minecraft:dead_bush").unwrap_or(BlockStateId::AIR).0;

    for _ in 0..(veg.grass_patches as i32 * 4) {
        let lx = rng.next_int_bounded(16);
        let lz = rng.next_int_bounded(16);
        let sy = surf_y[(lz as usize) * 16 + (lx as usize)];
        if sy > 62 && sy < max_y - 1 {
            let g = get_blk(sec_blocks, lx, sy, lz).unwrap_or(0);
            let above = get_blk(sec_blocks, lx, sy + 1, lz).unwrap_or(1);
            if g == grass_id && BlockStateId(above).is_air() && short_grass != 0 {
                set_blk(sec_blocks, lx, sy + 1, lz, short_grass);
            }
        }
    }
    for _ in 0..(veg.flower_patches as i32 * 2) {
        let lx = rng.next_int_bounded(16);
        let lz = rng.next_int_bounded(16);
        let sy = surf_y[(lz as usize) * 16 + (lx as usize)];
        if sy > 62 && sy < max_y - 1 {
            let g = get_blk(sec_blocks, lx, sy, lz).unwrap_or(0);
            let above = get_blk(sec_blocks, lx, sy + 1, lz).unwrap_or(1);
            if g == grass_id && BlockStateId(above).is_air() {
                let fl = if rng.next_boolean() { dandelion } else { poppy };
                if fl != 0 {
                    set_blk(sec_blocks, lx, sy + 1, lz, fl);
                }
            }
        }
    }
    if veg.tree_style == 9 && dead_bush != 0 {
        for _ in 0..3 {
            let lx = rng.next_int_bounded(16);
            let lz = rng.next_int_bounded(16);
            let sy = surf_y[(lz as usize) * 16 + (lx as usize)];
            if sy > 62 && sy < max_y - 1 {
                let g = get_blk(sec_blocks, lx, sy, lz).unwrap_or(0);
                let above = get_blk(sec_blocks, lx, sy + 1, lz).unwrap_or(1);
                if (g == sand_id || g == red_sand_id) && BlockStateId(above).is_air() {
                    set_blk(sec_blocks, lx, sy + 1, lz, dead_bush);
                }
            }
        }
    }
}

fn is_air_exposed(sec_blocks: &[Vec<u32>], x: i32, y: i32, z: i32) -> bool {
    for (dx, dy, dz) in [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)] {
        if let Some(s) = get_blk(sec_blocks, x + dx, y + dy, z + dz) {
            if BlockStateId(s).is_air() {
                return true;
            }
        }
    }
    false
}

fn place_tree(
    sec_blocks: &mut [Vec<u32>],
    rng: &mut XoroshiroRandomSource,
    x: i32,
    y: i32,
    z: i32,
    style: u8,
) {
    let (log_name, leaves_name, base_h, rand_h, conifer) = match style {
        2 => ("minecraft:birch_log", "minecraft:birch_leaves", 5, 2, false),
        3 => ("minecraft:spruce_log", "minecraft:spruce_leaves", 6, 3, true),
        5 => ("minecraft:dark_oak_log", "minecraft:dark_oak_leaves", 5, 2, false),
        6 => ("minecraft:acacia_log", "minecraft:acacia_leaves", 5, 2, false),
        7 => ("minecraft:jungle_log", "minecraft:jungle_leaves", 6, 4, false),
        8 => ("minecraft:cherry_log", "minecraft:cherry_leaves", 5, 2, false),
        _ => ("minecraft:oak_log", "minecraft:oak_leaves", 4, 2, false),
    };
    let log = default_state_id(log_name).unwrap_or(BlockStateId::AIR).0;
    let leaves = default_state_id(leaves_name).unwrap_or(BlockStateId::AIR).0;
    if log == 0 || leaves == 0 {
        return;
    }

    let height = base_h + rng.next_int_bounded(rand_h + 1);
    // Check trunk space is air
    for dy in 0..=height {
        if let Some(cur) = get_blk(sec_blocks, x, y + dy, z) {
            if !BlockStateId(cur).is_air() {
                return;
            }
        }
    }

    for dy in 0..height {
        set_blk(sec_blocks, x, y + dy, z, log);
    }

    if conifer {
        // Layered spruce canopy
        let top = y + height;
        set_blk(sec_blocks, x, top + 1, z, leaves);
        for &(dy, rad) in &[(0i32, 1i32), (-1, 2), (-2, 1), (-3, 2)] {
            for dz in -rad..=rad {
                for dx in -rad..=rad {
                    if dx.abs() == rad && dz.abs() == rad && rad > 1 {
                        continue;
                    }
                    if let Some(cur) = get_blk(sec_blocks, x + dx, top + dy, z + dz) {
                        if BlockStateId(cur).is_air() {
                            set_blk(sec_blocks, x + dx, top + dy, z + dz, leaves);
                        }
                    }
                }
            }
        }
    } else {
        // Classic Minecraft blob canopy (BlobFoliagePlacer radius=2, height=3)
        let top = y + height;
        for dy in -2i32..=1 {
            let rad: i32 = if dy >= 0 { 1 } else { 2 };
            for dz in -rad..=rad {
                for dx in -rad..=rad {
                    if dx.abs() == rad && dz.abs() == rad && (dy == 1 || rng.next_boolean()) {
                        continue;
                    }
                    if let Some(cur) = get_blk(sec_blocks, x + dx, top + dy, z + dz) {
                        if BlockStateId(cur).is_air() {
                            set_blk(sec_blocks, x + dx, top + dy, z + dz, leaves);
                        }
                    }
                }
            }
        }
    }
}
