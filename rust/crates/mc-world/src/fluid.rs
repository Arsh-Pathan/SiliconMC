use crate::{BlockWorld, ScheduledTickKind, TickScheduler};
use mc_data::block_behaviors::{KIND_LAVA, KIND_WATER, LAVA_STATES, WATER_STATES, block_behavior};
use mc_data::{BlockStateId, default_state_id};
use mc_math::BlockPos;

const HORIZ: [(i32, i32); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];

#[inline]
fn fluid_kind_and_level(st: BlockStateId) -> Option<(u8, u8)> {
    let beh = block_behavior(st.0);
    if beh.kind == KIND_WATER || beh.kind == KIND_LAVA {
        Some((beh.kind, beh.power()))
    } else {
        None
    }
}

#[inline]
fn fluid_state(kind: u8, level: u8) -> BlockStateId {
    let lv = (level & 15) as usize;
    if kind == KIND_WATER {
        BlockStateId(WATER_STATES[lv])
    } else {
        BlockStateId(LAVA_STATES[lv])
    }
}

#[inline]
pub fn fluid_tick_delay(kind: u8, is_nether: bool) -> u64 {
    if kind == KIND_WATER {
        5
    } else if is_nether {
        10
    } else {
        30
    }
}

#[inline]
fn can_fluid_enter(st: BlockStateId, kind: u8) -> bool {
    if st.is_air() {
        return true;
    }
    let beh = block_behavior(st.0);
    if beh.kind == kind {
        return false;
    }
    st.props().can_be_replaced() && !st.props().has_fluid()
}

/// Execute one vanilla Minecraft 26.3 fluid flow tick at `pos`.
/// Implements water/lava spreading, falling, decay, infinite water source conversion,
/// and water+lava obsidian/cobblestone/stone generation.
pub fn tick_fluid<W: BlockWorld>(
    world: &mut W,
    scheduler: &mut TickScheduler,
    current_tick: u64,
    pos: BlockPos,
    is_nether: bool,
) -> Vec<(BlockPos, BlockStateId)> {
    let mut changed = Vec::new();
    let st = world.get_block_state(pos);
    let Some((kind, cur_level)) = fluid_kind_and_level(st) else {
        return changed;
    };

    let delay = fluid_tick_delay(kind, is_nether);
    let drop_off: u8 = if kind == KIND_WATER || is_nether { 1 } else { 2 };

    // 1. Check water <-> lava liquid mixing at this block!
    if kind == KIND_LAVA {
        let mut touches_water = false;
        for (dx, dz) in HORIZ {
            if let Some((KIND_WATER, _)) = fluid_kind_and_level(world.get_block_state(pos.offset(dx, 0, dz))) {
                touches_water = true;
                break;
            }
        }
        if !touches_water {
            if let Some((KIND_WATER, _)) = fluid_kind_and_level(world.get_block_state(pos.offset(0, 1, 0))) {
                touches_water = true;
            }
        }
        if touches_water {
            let result = if cur_level == 0 {
                default_state_id("minecraft:obsidian").unwrap_or(BlockStateId::STONE)
            } else {
                default_state_id("minecraft:cobblestone").unwrap_or(BlockStateId::STONE)
            };
            world.set_block_state(pos, result);
            changed.push((pos, result));
            schedule_adjacent_fluids(world, scheduler, current_tick, pos, is_nether);
            return changed;
        }
    } else if kind == KIND_WATER {
        // Water next to lava triggers obsidian/cobblestone conversion on the lava block
        for (dx, dz) in HORIZ {
            let np = pos.offset(dx, 0, dz);
            if let Some((KIND_LAVA, lava_lv)) = fluid_kind_and_level(world.get_block_state(np)) {
                let result = if lava_lv == 0 {
                    default_state_id("minecraft:obsidian").unwrap_or(BlockStateId::STONE)
                } else {
                    default_state_id("minecraft:cobblestone").unwrap_or(BlockStateId::STONE)
                };
                world.set_block_state(np, result);
                changed.push((np, result));
                schedule_adjacent_fluids(world, scheduler, current_tick, np, is_nether);
            }
        }
    }

    // 2. Recompute expected fluid level for non-source blocks (or infinite water source conversion)
    let mut active_level = cur_level;
    if cur_level != 0 {
        let above = world.get_block_state(pos.offset(0, 1, 0));
        let has_same_above = matches!(fluid_kind_and_level(above), Some((k, _)) if k == kind);

        let mut best_horiz_level = 255u8;
        let mut source_neighbors = 0u8;
        for (dx, dz) in HORIZ {
            if let Some((k, lv)) = fluid_kind_and_level(world.get_block_state(pos.offset(dx, 0, dz))) {
                if k == kind {
                    if lv == 0 {
                        source_neighbors += 1;
                    }
                    let eff = if lv >= 8 { 0 } else { lv };
                    best_horiz_level = best_horiz_level.min(eff);
                }
            }
        }

        let below_st = world.get_block_state(pos.offset(0, -1, 0));
        let supported_below = below_st.props().is_solid()
            || matches!(fluid_kind_and_level(below_st), Some((k, 0)) if k == kind);

        let new_level_opt = if kind == KIND_WATER && source_neighbors >= 2 && supported_below {
            Some(0u8) // Infinite water source conversion!
        } else if has_same_above {
            Some(8u8) // Falling fluid
        } else if best_horiz_level < 8 {
            let next_lv = best_horiz_level.saturating_add(drop_off);
            if next_lv < 8 { Some(next_lv) } else { None }
        } else {
            None
        };

        match new_level_opt {
            None => {
                world.set_block_state(pos, BlockStateId::AIR);
                changed.push((pos, BlockStateId::AIR));
                schedule_adjacent_fluids(world, scheduler, current_tick, pos, is_nether);
                return changed;
            }
            Some(nl) if nl != cur_level => {
                let ns = fluid_state(kind, nl);
                world.set_block_state(pos, ns);
                changed.push((pos, ns));
                active_level = nl;
                scheduler.schedule(current_tick, delay, 0, pos, ScheduledTickKind::FluidTick);
                schedule_adjacent_fluids(world, scheduler, current_tick, pos, is_nether);
            }
            _ => {}
        }
    }

    // 3. Spread downward first (vanilla FlowingFluid.spread)
    let below_pos = pos.offset(0, -1, 0);
    let below_st = world.get_block_state(below_pos);
    if kind == KIND_LAVA {
        if let Some((KIND_WATER, _)) = fluid_kind_and_level(below_st) {
            world.set_block_state(below_pos, BlockStateId::STONE);
            changed.push((below_pos, BlockStateId::STONE));
            schedule_adjacent_fluids(world, scheduler, current_tick, below_pos, is_nether);
            return changed;
        }
    }
    if can_fluid_enter(below_st, kind) {
        let falling = fluid_state(kind, 8);
        world.set_block_state(below_pos, falling);
        changed.push((below_pos, falling));
        scheduler.schedule(current_tick, delay, 0, below_pos, ScheduledTickKind::FluidTick);
        // Only source blocks also spread sideways when falling straight down
        if active_level != 0 {
            return changed;
        }
    }

    // 4. Spread horizontally
    let eff_level = if active_level >= 8 { 0 } else { active_level };
    let spread_level = eff_level.saturating_add(drop_off);
    if spread_level < 8 {
        for (dx, dz) in HORIZ {
            let np = pos.offset(dx, 0, dz);
            let nst = world.get_block_state(np);
            if can_fluid_enter(nst, kind) {
                let ns = fluid_state(kind, spread_level);
                world.set_block_state(np, ns);
                changed.push((np, ns));
                scheduler.schedule(current_tick, delay, 0, np, ScheduledTickKind::FluidTick);
            } else if let Some((nk, nlv)) = fluid_kind_and_level(nst) {
                if nk == kind && nlv > spread_level && nlv < 8 {
                    let ns = fluid_state(kind, spread_level);
                    world.set_block_state(np, ns);
                    changed.push((np, ns));
                    scheduler.schedule(current_tick, delay, 0, np, ScheduledTickKind::FluidTick);
                }
            }
        }
    }

    changed
}

pub fn schedule_adjacent_fluids<W: BlockWorld>(
    world: &mut W,
    scheduler: &mut TickScheduler,
    current_tick: u64,
    origin: BlockPos,
    is_nether: bool,
) {
    for (dx, dy, dz) in [
        (0, 0, 0),
        (0, -1, 0),
        (0, 1, 0),
        (0, 0, -1),
        (0, 0, 1),
        (-1, 0, 0),
        (1, 0, 0),
    ] {
        let p = origin.offset(dx, dy, dz);
        if let Some((kind, _)) = fluid_kind_and_level(world.get_block_state(p)) {
            scheduler.schedule(
                current_tick,
                fluid_tick_delay(kind, is_nether),
                0,
                p,
                ScheduledTickKind::FluidTick,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct TestWorld(HashMap<BlockPos, BlockStateId>);
    impl BlockWorld for TestWorld {
        fn get_block_state(&mut self, pos: BlockPos) -> BlockStateId {
            self.0.get(&pos).copied().unwrap_or(BlockStateId::AIR)
        }
        fn set_block_state(&mut self, pos: BlockPos, state: BlockStateId) -> bool {
            self.0.insert(pos, state);
            true
        }
    }

    #[test]
    fn water_spreads_converts_lava_to_obsidian_and_cobblestone_and_forms_infinite_source() {
        let mut w = TestWorld(HashMap::new());
        let mut sched = TickScheduler::new();
        // Stone floor at y = 0
        for x in -4..=6 {
            for z in -2..=2 {
                w.set_block_state(BlockPos::new(x, 0, z), BlockStateId::STONE);
            }
        }
        let water_src = BlockStateId(WATER_STATES[0]);
        let lava_src = BlockStateId(LAVA_STATES[0]);
        let lava_flow = BlockStateId(LAVA_STATES[2]);
        let obsidian = default_state_id("minecraft:obsidian").unwrap();
        let cobble = default_state_id("minecraft:cobblestone").unwrap();

        // Water at (0, 1, 0), lava source at (1, 1, 0), flowing lava at (-1, 1, 0)
        w.set_block_state(BlockPos::new(0, 1, 0), water_src);
        w.set_block_state(BlockPos::new(1, 1, 0), lava_src);
        w.set_block_state(BlockPos::new(-1, 1, 0), lava_flow);

        tick_fluid(&mut w, &mut sched, 1, BlockPos::new(0, 1, 0), false);
        assert_eq!(w.get_block_state(BlockPos::new(1, 1, 0)), obsidian);
        assert_eq!(w.get_block_state(BlockPos::new(-1, 1, 0)), cobble);

        // Infinite water source: sources at (3, 1, 0) and (5, 1, 0) -> (4, 1, 0) becomes source!
        w.set_block_state(BlockPos::new(3, 1, 0), water_src);
        w.set_block_state(BlockPos::new(5, 1, 0), water_src);
        schedule_adjacent_fluids(&mut w, &mut sched, 10, BlockPos::new(3, 1, 0), false);
        schedule_adjacent_fluids(&mut w, &mut sched, 10, BlockPos::new(5, 1, 0), false);
        for t in 11..=30 {
            for due in sched.drain_due(t) {
                if due.kind == ScheduledTickKind::FluidTick {
                    tick_fluid(&mut w, &mut sched, t, due.pos, false);
                }
            }
        }
        assert_eq!(w.get_block_state(BlockPos::new(4, 1, 0)), water_src);
    }
}
