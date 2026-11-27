use crate::Chunk;
use mc_data::BlockStateId;
use mc_math::{BlockPos, ChunkPos};
use std::collections::{HashMap, VecDeque};

const NEIGHBORS: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

/// Tracks which chunk light sections were modified during a lighting update,
/// keyed by `ChunkPos` -> `(sky_section_mask, block_section_mask)`.
#[derive(Debug, Default, Clone)]
pub struct DirtyLightSections {
    pub chunks: HashMap<ChunkPos, (u64, u64)>,
}

impl DirtyLightSections {
    pub fn mark_sky(&mut self, pos: ChunkPos, sec_idx: usize) {
        let entry = self.chunks.entry(pos).or_insert((0, 0));
        entry.0 |= 1u64 << sec_idx;
    }

    pub fn mark_block(&mut self, pos: ChunkPos, sec_idx: usize) {
        let entry = self.chunks.entry(pos).or_insert((0, 0));
        entry.1 |= 1u64 << sec_idx;
    }
}

/// Abstract access over either a single `Chunk` or a loaded chunk map so lighting works
/// identically for single-chunk generation and cross-chunk live edits.
pub trait ChunkLightView {
    fn min_y(&self) -> i32;
    fn max_y(&self) -> i32;
    fn has_chunk(&self, pos: ChunkPos) -> bool;
    fn get_block(&self, pos: BlockPos) -> BlockStateId;
    fn get_sky_light(&self, pos: BlockPos) -> u8;
    fn set_sky_light(&mut self, pos: BlockPos, level: u8) -> Option<(ChunkPos, usize)>;
    fn get_block_light(&self, pos: BlockPos) -> u8;
    fn set_block_light(&mut self, pos: BlockPos, level: u8) -> Option<(ChunkPos, usize)>;
}

impl ChunkLightView for Chunk {
    fn min_y(&self) -> i32 {
        self.min_y()
    }
    fn max_y(&self) -> i32 {
        self.max_y()
    }
    fn has_chunk(&self, pos: ChunkPos) -> bool {
        pos == self.pos
    }
    fn get_block(&self, pos: BlockPos) -> BlockStateId {
        if pos.chunk_pos() != self.pos {
            return BlockStateId::AIR;
        }
        Chunk::get_block(self, (pos.x & 15) as usize, pos.y, (pos.z & 15) as usize)
    }
    fn get_sky_light(&self, pos: BlockPos) -> u8 {
        if pos.chunk_pos() != self.pos {
            return 0;
        }
        Chunk::get_sky_light(self, (pos.x & 15) as usize, pos.y, (pos.z & 15) as usize)
    }
    fn set_sky_light(&mut self, pos: BlockPos, level: u8) -> Option<(ChunkPos, usize)> {
        if pos.chunk_pos() != self.pos {
            return None;
        }
        Chunk::set_sky_light(self, (pos.x & 15) as usize, pos.y, (pos.z & 15) as usize, level)
            .map(|idx| (self.pos, idx))
    }
    fn get_block_light(&self, pos: BlockPos) -> u8 {
        if pos.chunk_pos() != self.pos {
            return 0;
        }
        Chunk::get_block_light(self, (pos.x & 15) as usize, pos.y, (pos.z & 15) as usize)
    }
    fn set_block_light(&mut self, pos: BlockPos, level: u8) -> Option<(ChunkPos, usize)> {
        if pos.chunk_pos() != self.pos {
            return None;
        }
        Chunk::set_block_light(self, (pos.x & 15) as usize, pos.y, (pos.z & 15) as usize, level)
            .map(|idx| (self.pos, idx))
    }
}

impl ChunkLightView for HashMap<ChunkPos, Chunk> {
    fn min_y(&self) -> i32 {
        self.values().next().map(|c| c.min_y()).unwrap_or(-64)
    }
    fn max_y(&self) -> i32 {
        self.values().next().map(|c| c.max_y()).unwrap_or(320)
    }
    fn has_chunk(&self, pos: ChunkPos) -> bool {
        self.contains_key(&pos)
    }
    fn get_block(&self, pos: BlockPos) -> BlockStateId {
        let Some(c) = self.get(&pos.chunk_pos()) else {
            return BlockStateId::AIR;
        };
        c.get_block((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize)
    }
    fn get_sky_light(&self, pos: BlockPos) -> u8 {
        let Some(c) = self.get(&pos.chunk_pos()) else {
            return 0;
        };
        c.get_sky_light((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize)
    }
    fn set_sky_light(&mut self, pos: BlockPos, level: u8) -> Option<(ChunkPos, usize)> {
        let cp = pos.chunk_pos();
        let c = self.get_mut(&cp)?;
        c.set_sky_light((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize, level)
            .map(|idx| (cp, idx))
    }
    fn get_block_light(&self, pos: BlockPos) -> u8 {
        let Some(c) = self.get(&pos.chunk_pos()) else {
            return 0;
        };
        c.get_block_light((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize)
    }
    fn set_block_light(&mut self, pos: BlockPos, level: u8) -> Option<(ChunkPos, usize)> {
        let cp = pos.chunk_pos();
        let c = self.get_mut(&cp)?;
        c.set_block_light((pos.x & 15) as usize, pos.y, (pos.z & 15) as usize, level)
            .map(|idx| (cp, idx))
    }
}

/// Vanilla-faithful BFS BlockLight & SkyLight propagation engine using Minecraft 26.3's
/// exact per-state `light_emission`, `light_dampening`, and `propagates_skylight_down` tables.
pub struct LightEngine;

impl LightEngine {
    /// Compute initial skylight and blocklight for a freshly generated chunk.
    pub fn compute_initial_chunk_light(chunk: &mut Chunk) {
        let min_y = chunk.min_y();
        let max_y = chunk.max_y();
        let base_x = chunk.pos.x << 4;
        let base_z = chunk.pos.z << 4;

        // Reset light arrays
        for sec in &mut chunk.sections {
            sec.sky_light = Some(vec![0u8; 2048]);
            sec.block_light = None;
        }
        chunk.extra_sky_light_above = Some(vec![0xFF; 2048]);

        let mut sky_queue: VecDeque<(BlockPos, u8)> = VecDeque::new();
        let mut block_queue: VecDeque<(BlockPos, u8)> = VecDeque::new();

        // 1. Direct skylight column pass + seed blocklight emitters
        for lz in 0..16 {
            for lx in 0..16 {
                let mut sky = 15u8;
                let mut direct = true;
                for y in (min_y..max_y).rev() {
                    let st = chunk.get_block(lx, y, lz);
                    let props = st.props();

                    if props.light_emission > 0 {
                        let bp = BlockPos::new(base_x + lx as i32, y, base_z + lz as i32);
                        chunk.set_block_light(lx, y, lz, props.light_emission);
                        block_queue.push_back((bp, props.light_emission));
                    }

                    if direct {
                        if props.propagates_skylight_down() && props.light_dampening == 0 {
                            sky = 15;
                        } else {
                            direct = false;
                            let damp = props.light_dampening.max(1);
                            sky = sky.saturating_sub(damp);
                        }
                    } else if sky > 0 {
                        let damp = props.light_dampening.max(1);
                        sky = sky.saturating_sub(damp);
                    }

                    if sky > 0 {
                        chunk.set_sky_light(lx, y, lz, sky);
                        if !direct || y < max_y - 1 {
                            // Only need to flood-fill where neighbors might be darker
                            if !direct {
                                sky_queue.push_back((BlockPos::new(base_x + lx as i32, y, base_z + lz as i32), sky));
                            }
                        }
                    }
                }
            }
        }

        // Seed horizontal edges adjacent to darker columns for direct-skylight blocks
        for lz in 0..16 {
            for lx in 0..16 {
                for y in min_y..max_y {
                    let level = chunk.get_sky_light(lx, y, lz);
                    if level <= 1 {
                        continue;
                    }
                    let bp = BlockPos::new(base_x + lx as i32, y, base_z + lz as i32);
                    for (dx, dy, dz) in NEIGHBORS {
                        let nx = lx as i32 + dx;
                        let ny = y + dy;
                        let nz = lz as i32 + dz;
                        if (0..16).contains(&nx) && (0..16).contains(&nz) && ny >= min_y && ny < max_y {
                            if chunk.get_sky_light(nx as usize, ny, nz as usize) + 1 < level {
                                sky_queue.push_back((bp, level));
                                break;
                            }
                        }
                    }
                }
            }
        }

        let mut dirty = DirtyLightSections::default();
        Self::bfs_spread_sky(chunk, &mut sky_queue, &mut dirty);
        Self::bfs_spread_block(chunk, &mut block_queue, &mut dirty);
    }

    /// Incrementally relight both BlockLight and SkyLight after the block at `pos` changed.
    /// Returns all modified light sections across all touched chunks for `ClientboundLightUpdatePacket`.
    pub fn update_at<V: ChunkLightView>(view: &mut V, pos: BlockPos) -> DirtyLightSections {
        let mut dirty = DirtyLightSections::default();
        Self::update_block_light_at(view, pos, &mut dirty);
        Self::update_sky_light_at(view, pos, &mut dirty);
        dirty
    }

    fn update_block_light_at<V: ChunkLightView>(
        view: &mut V,
        pos: BlockPos,
        dirty: &mut DirtyLightSections,
    ) {
        let min_y = view.min_y();
        let max_y = view.max_y();
        if pos.y < min_y || pos.y >= max_y {
            return;
        }

        let props = view.get_block(pos).props();
        let new_emit = props.light_emission;
        let old_light = view.get_block_light(pos);

        let mut remove_q: VecDeque<(BlockPos, u8)> = VecDeque::new();
        let mut add_q: VecDeque<(BlockPos, u8)> = VecDeque::new();

        if old_light > 0 {
            if let Some((cp, idx)) = view.set_block_light(pos, 0) {
                dirty.mark_block(cp, idx);
            }
            remove_q.push_back((pos, old_light));
        }

        // BFS darkness wave
        while let Some((cur, prev_val)) = remove_q.pop_front() {
            for (dx, dy, dz) in NEIGHBORS {
                let np = cur.offset(dx, dy, dz);
                if np.y < min_y || np.y >= max_y || !view.has_chunk(np.chunk_pos()) {
                    continue;
                }
                let nval = view.get_block_light(np);
                if nval == 0 {
                    continue;
                }
                if nval < prev_val {
                    if let Some((cp, idx)) = view.set_block_light(np, 0) {
                        dirty.mark_block(cp, idx);
                    }
                    remove_q.push_back((np, nval));
                    // Re-seed if neighbor itself is a light emitter
                    let nemit = view.get_block(np).props().light_emission;
                    if nemit > 0 {
                        if let Some((cp, idx)) = view.set_block_light(np, nemit) {
                            dirty.mark_block(cp, idx);
                        }
                        add_q.push_back((np, nemit));
                    }
                } else {
                    // Frontier source to refill cleared region
                    add_q.push_back((np, nval));
                }
            }
        }

        // Seed self if new block emits light, OR pull from brightest neighbor if block became transparent
        if new_emit > 0 {
            if let Some((cp, idx)) = view.set_block_light(pos, new_emit) {
                dirty.mark_block(cp, idx);
            }
            add_q.push_back((pos, new_emit));
        } else {
            // Check neighbors to pull light into `pos`
            for (dx, dy, dz) in NEIGHBORS {
                let np = pos.offset(dx, dy, dz);
                if np.y >= min_y && np.y < max_y && view.has_chunk(np.chunk_pos()) {
                    let nval = view.get_block_light(np);
                    if nval > 1 {
                        add_q.push_back((np, nval));
                    }
                }
            }
        }

        Self::bfs_spread_block(view, &mut add_q, dirty);
    }

    fn update_sky_light_at<V: ChunkLightView>(
        view: &mut V,
        pos: BlockPos,
        dirty: &mut DirtyLightSections,
    ) {
        let min_y = view.min_y();
        let max_y = view.max_y();
        if pos.y < min_y || pos.y >= max_y {
            return;
        }

        let mut remove_q: VecDeque<(BlockPos, u8)> = VecDeque::new();
        let mut add_q: VecDeque<(BlockPos, u8)> = VecDeque::new();

        // Check direct skylight coming from directly above `pos`
        let above_sky = if pos.y + 1 >= max_y {
            15u8
        } else {
            view.get_sky_light(pos.offset(0, 1, 0))
        };

        // Trace vertical column downward from `pos.y` while skylight state changes
        let mut cur_sky = above_sky;
        for y in (min_y..=pos.y).rev() {
            let bp = BlockPos::new(pos.x, y, pos.z);
            let props = view.get_block(bp).props();
            let old = view.get_sky_light(bp);

            let direct_val = if cur_sky == 15 && props.propagates_skylight_down() && props.light_dampening == 0 {
                15
            } else {
                cur_sky.saturating_sub(props.light_dampening.max(1))
            };
            cur_sky = direct_val;

            if direct_val > old {
                if let Some((cp, idx)) = view.set_sky_light(bp, direct_val) {
                    dirty.mark_sky(cp, idx);
                }
                add_q.push_back((bp, direct_val));
            } else if direct_val < old {
                if let Some((cp, idx)) = view.set_sky_light(bp, 0) {
                    dirty.mark_sky(cp, idx);
                }
                remove_q.push_back((bp, old));
            } else {
                // Reached equilibrium downward
                if y < pos.y {
                    break;
                }
            }
        }

        // Darkness BFS
        while let Some((cur, prev_val)) = remove_q.pop_front() {
            for (dx, dy, dz) in NEIGHBORS {
                let np = cur.offset(dx, dy, dz);
                if np.y < min_y || np.y >= max_y || !view.has_chunk(np.chunk_pos()) {
                    continue;
                }
                let nval = view.get_sky_light(np);
                if nval == 0 {
                    continue;
                }
                // Special case: direct skylight moving straight down doesn't drop from 15 to 14
                let clears_down_direct = dy == -1 && prev_val == 15 && nval == 15;
                if nval < prev_val || clears_down_direct {
                    if let Some((cp, idx)) = view.set_sky_light(np, 0) {
                        dirty.mark_sky(cp, idx);
                    }
                    remove_q.push_back((np, nval));
                } else {
                    add_q.push_back((np, nval));
                }
            }
        }

        // Pull from neighbors around `pos` in case `pos` opened up
        for (dx, dy, dz) in NEIGHBORS {
            let np = pos.offset(dx, dy, dz);
            if np.y >= min_y && np.y < max_y && view.has_chunk(np.chunk_pos()) {
                let nval = view.get_sky_light(np);
                if nval > 1 {
                    add_q.push_back((np, nval));
                }
            }
        }

        Self::bfs_spread_sky(view, &mut add_q, dirty);
    }

    fn bfs_spread_block<V: ChunkLightView>(
        view: &mut V,
        queue: &mut VecDeque<(BlockPos, u8)>,
        dirty: &mut DirtyLightSections,
    ) {
        let min_y = view.min_y();
        let max_y = view.max_y();
        while let Some((cur, val)) = queue.pop_front() {
            if val <= 1 {
                continue;
            }
            for (dx, dy, dz) in NEIGHBORS {
                let np = cur.offset(dx, dy, dz);
                if np.y < min_y || np.y >= max_y || !view.has_chunk(np.chunk_pos()) {
                    continue;
                }
                let props = view.get_block(np).props();
                let damp = props.light_dampening.max(1);
                let next = val.saturating_sub(damp);
                if next > view.get_block_light(np) {
                    if let Some((cp, idx)) = view.set_block_light(np, next) {
                        dirty.mark_block(cp, idx);
                    }
                    queue.push_back((np, next));
                }
            }
        }
    }

    fn bfs_spread_sky<V: ChunkLightView>(
        view: &mut V,
        queue: &mut VecDeque<(BlockPos, u8)>,
        dirty: &mut DirtyLightSections,
    ) {
        let min_y = view.min_y();
        let max_y = view.max_y();
        while let Some((cur, val)) = queue.pop_front() {
            if val == 0 {
                continue;
            }
            for (dx, dy, dz) in NEIGHBORS {
                let np = cur.offset(dx, dy, dz);
                if np.y < min_y || np.y >= max_y || !view.has_chunk(np.chunk_pos()) {
                    continue;
                }
                let props = view.get_block(np).props();
                let next = if dy == -1
                    && val == 15
                    && props.propagates_skylight_down()
                    && props.light_dampening == 0
                {
                    15
                } else {
                    val.saturating_sub(props.light_dampening.max(1))
                };
                if next > view.get_sky_light(np) {
                    if let Some((cp, idx)) = view.set_sky_light(np, next) {
                        dirty.mark_sky(cp, idx);
                    }
                    queue.push_back((np, next));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_data::default_state_id;

    #[test]
    fn torch_emits_block_light_and_extinguishes_cleanly() {
        let mut chunk = Chunk::new_flat_overworld(ChunkPos::new(0, 0), 0);
        // Above grass (-61) is air at y=-60; skylight should be 15, blocklight 0
        assert_eq!(chunk.get_sky_light(8, -60, 8), 15);
        assert_eq!(chunk.get_sky_light(8, -61, 8), 0); // inside grass_block
        assert_eq!(chunk.get_block_light(8, -60, 8), 0);

        // Place a torch (emission 14) at (8, -60, 8)
        let torch = default_state_id("minecraft:torch").unwrap();
        chunk.set_block(8, -60, 8, torch);
        let dirty = LightEngine::update_at(&mut chunk, BlockPos::new(8, -60, 8));
        assert!(!dirty.chunks.is_empty());
        assert_eq!(chunk.get_block_light(8, -60, 8), 14);
        assert_eq!(chunk.get_block_light(9, -60, 8), 13);
        assert_eq!(chunk.get_block_light(10, -60, 8), 12);
        assert_eq!(chunk.get_block_light(8, -55, 8), 9);

        // Break the torch -> blocklight must return to 0 everywhere!
        chunk.set_block(8, -60, 8, BlockStateId::AIR);
        LightEngine::update_at(&mut chunk, BlockPos::new(8, -60, 8));
        assert_eq!(chunk.get_block_light(8, -60, 8), 0);
        assert_eq!(chunk.get_block_light(9, -60, 8), 0);
        assert_eq!(chunk.get_block_light(8, -55, 8), 0);
    }

    #[test]
    fn roof_blocks_skylight_and_removal_restores_it() {
        let mut chunk = Chunk::new_flat_overworld(ChunkPos::new(0, 0), 0);
        let stone = BlockStateId::STONE;
        // Build a 3x3 stone roof at y=-58 centered at (8, -58, 8)
        for dx in -1..=1 {
            for dz in -1..=1 {
                let x = (8 + dx) as usize;
                let z = (8 + dz) as usize;
                chunk.set_block(x, -58, z, stone);
                LightEngine::update_at(&mut chunk, BlockPos::new(x as i32, -58, z as i32));
            }
        }
        // Directly under center of 3x3 roof at (8, -59, 8): skylight came sideways from 2 blocks away -> 13!
        assert_eq!(chunk.get_sky_light(8, -59, 8), 13);
        // Remove center block -> direct 15 returns!
        chunk.set_block(8, -58, 8, BlockStateId::AIR);
        LightEngine::update_at(&mut chunk, BlockPos::new(8, -58, 8));
        assert_eq!(chunk.get_sky_light(8, -59, 8), 15);
    }
}
