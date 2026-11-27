use crate::{ScheduledTick, ScheduledTickKind, TickScheduler, fluid_tick_delay, tick_fluid};
use mc_data::BlockStateId;
use mc_data::block_behaviors::{
    KIND_BUTTON, KIND_DOOR, KIND_FALLING_BLOCK, KIND_FENCE_GATE, KIND_LAVA, KIND_LEVER,
    KIND_PISTON, KIND_PISTON_HEAD, KIND_PRESSURE_PLATE, KIND_REDSTONE_BLOCK, KIND_REDSTONE_LAMP,
    KIND_REDSTONE_TORCH, KIND_REDSTONE_WIRE, KIND_REPEATER, KIND_STICKY_PISTON, KIND_TRAPDOOR,
    KIND_WATER, PISTON_HEAD_STATES, block_behavior, redstone_wire_with_power,
};
use mc_math::BlockPos;
use std::collections::{HashSet, VecDeque};

/// 6 cardinal offsets in Minecraft Direction order: Down(0), Up(1), North(2), South(3), West(4), East(5).
const DIRS: [(i32, i32, i32, u8); 6] = [
    (0, -1, 0, 0),
    (0, 1, 0, 1),
    (0, 0, -1, 2),
    (0, 0, 1, 3),
    (-1, 0, 0, 4),
    (1, 0, 0, 5),
];

#[inline]
fn opposite_dir(d: u8) -> u8 {
    match d {
        0 => 1,
        1 => 0,
        2 => 3,
        3 => 2,
        4 => 5,
        5 => 4,
        _ => 0,
    }
}

#[inline]
fn dir_offset(d: u8) -> (i32, i32, i32) {
    match d {
        0 => (0, -1, 0),
        1 => (0, 1, 0),
        2 => (0, 0, -1),
        3 => (0, 0, 1),
        4 => (-1, 0, 0),
        5 => (1, 0, 0),
        _ => (0, 0, 0),
    }
}

/// Mutable block access trait implemented by `WorldStore` (and unit-test worlds).
pub trait BlockWorld {
    fn get_block_state(&mut self, pos: BlockPos) -> BlockStateId;
    fn set_block_state(&mut self, pos: BlockPos, state: BlockStateId) -> bool;
}

/// Direct signal emitted by the block at `src_pos` toward `toward_dir` (0..=5).
pub fn direct_signal_toward<W: BlockWorld>(
    world: &mut W,
    src_pos: BlockPos,
    toward_dir: u8,
) -> u8 {
    let st = world.get_block_state(src_pos);
    let beh = block_behavior(st.0);
    match beh.kind {
        KIND_REDSTONE_BLOCK => 15,
        KIND_LEVER | KIND_BUTTON | KIND_PRESSURE_PLATE => {
            if beh.is_powered() {
                15
            } else {
                0
            }
        }
        KIND_REDSTONE_TORCH => {
            if !beh.is_lit() {
                0
            } else {
                // Vanilla RedstoneTorchBlock/RedstoneWallTorchBlock do NOT emit signal
                // back into the block they are attached to.
                let attached_dir = if beh.is_upper_or_wall() && beh.facing < 6 {
                    opposite_dir(beh.facing)
                } else {
                    0 // Down
                };
                if toward_dir == attached_dir { 0 } else { 15 }
            }
        }
        KIND_REPEATER => {
            // Repeater outputs in opposite of its `facing` direction
            if beh.is_powered() && opposite_dir(beh.facing) == toward_dir {
                15
            } else {
                0
            }
        }
        KIND_REDSTONE_WIRE => beh.power(),
        _ => 0,
    }
}

/// Strong signal emitted by `src_pos` into an adjacent solid conductor block.
fn strong_signal_into<W: BlockWorld>(world: &mut W, src_pos: BlockPos, toward_dir: u8) -> u8 {
    let st = world.get_block_state(src_pos);
    let beh = block_behavior(st.0);
    match beh.kind {
        KIND_LEVER | KIND_BUTTON | KIND_PRESSURE_PLATE => {
            if beh.is_powered() {
                15
            } else {
                0
            }
        }
        KIND_REDSTONE_TORCH => {
            if beh.is_lit() && toward_dir == 1 {
                15
            } else {
                0
            }
        }
        KIND_REPEATER => {
            if beh.is_powered() && opposite_dir(beh.facing) == toward_dir {
                15
            } else {
                0
            }
        }
        _ => 0,
    }
}

/// Compute highest redstone signal (0..=15) received by `pos` from its 6 neighbors
/// (including strong-powered solid conductor blocks).
pub fn best_neighbor_signal<W: BlockWorld>(world: &mut W, pos: BlockPos) -> u8 {
    let mut best = 0u8;
    for &(dx, dy, dz, dir_to_neighbor) in &DIRS {
        let np = pos.offset(dx, dy, dz);
        let from_neighbor_dir = opposite_dir(dir_to_neighbor);
        best = best.max(direct_signal_toward(world, np, from_neighbor_dir));

        // Solid conductor block check (receives strong power from any of its neighbors)
        let nst = world.get_block_state(np);
        if nst.props().can_occlude() && block_behavior(nst.0).kind == 0 {
            for &(dx2, dy2, dz2, d2) in &DIRS {
                let np2 = np.offset(dx2, dy2, dz2);
                if np2 == pos {
                    continue;
                }
                best = best.max(strong_signal_into(world, np2, opposite_dir(d2)));
            }
        }
        if best == 15 {
            break;
        }
    }
    best
}

/// Handle right-click player interaction with levers, buttons, doors, trapdoors, and fence gates.
/// Returns `Some(changed_blocks)` if the block intercepted the right-click (suppressing block placement).
pub fn interact_block<W: BlockWorld>(
    world: &mut W,
    scheduler: &mut TickScheduler,
    current_tick: u64,
    pos: BlockPos,
) -> Option<Vec<(BlockPos, BlockStateId)>> {
    let st = world.get_block_state(pos);
    let beh = block_behavior(st.0);
    let mut seeds = Vec::new();
    let mut changed = Vec::new();

    match beh.kind {
        KIND_LEVER => {
            let next = BlockStateId(beh.toggled_powered()?);
            world.set_block_state(pos, next);
            changed.push((pos, next));
            seeds.push(pos);
        }
        KIND_BUTTON => {
            if beh.is_powered() {
                return Some(Vec::new());
            }
            let next = BlockStateId(beh.toggled_powered()?);
            world.set_block_state(pos, next);
            changed.push((pos, next));
            let delay = if st.block_name().contains("stone") || st.block_name().contains("blackstone") {
                20
            } else {
                30
            };
            scheduler.schedule(current_tick, delay, -1, pos, ScheduledTickKind::ButtonUnpress);
            seeds.push(pos);
        }
        KIND_DOOR => {
            let next = BlockStateId(beh.toggled_open()?);
            world.set_block_state(pos, next);
            changed.push((pos, next));
            let other_pos = if beh.is_upper_or_wall() {
                pos.offset(0, -1, 0)
            } else {
                pos.offset(0, 1, 0)
            };
            let other_st = world.get_block_state(other_pos);
            let other_beh = block_behavior(other_st.0);
            if other_beh.kind == KIND_DOOR {
                if let Some(other_next) = other_beh.toggled_open() {
                    let ns = BlockStateId(other_next);
                    world.set_block_state(other_pos, ns);
                    changed.push((other_pos, ns));
                }
            }
            return Some(changed);
        }
        KIND_TRAPDOOR | KIND_FENCE_GATE => {
            let next = BlockStateId(beh.toggled_open()?);
            world.set_block_state(pos, next);
            changed.push((pos, next));
            return Some(changed);
        }
        _ => return None,
    }

    let cascaded = propagate_neighbor_updates(world, scheduler, current_tick, &seeds);
    changed.extend(cascaded);
    Some(changed)
}

/// Enqueue `pos`, its 6 cardinal neighbors, and (for any adjacent solid conductor block)
/// that conductor's 6 neighbors so strong-powered blocks notify attached torches/wires/lamps.
fn enqueue_with_conductors<W: BlockWorld>(
    world: &mut W,
    queue: &mut VecDeque<BlockPos>,
    queued: &mut HashSet<BlockPos>,
    origin: BlockPos,
) {
    if queued.insert(origin) {
        queue.push_back(origin);
    }
    for &(dx, dy, dz, _) in &DIRS {
        let n = origin.offset(dx, dy, dz);
        if queued.insert(n) {
            queue.push_back(n);
        }
        let nst = world.get_block_state(n);
        if nst.props().can_occlude() && block_behavior(nst.0).kind == 0 {
            for &(dx2, dy2, dz2, _) in &DIRS {
                let n2 = n.offset(dx2, dy2, dz2);
                if queued.insert(n2) {
                    queue.push_back(n2);
                }
            }
        }
    }
}

/// Propagate deterministic redstone & neighbor updates starting from `seeds` (placed/broken/toggled positions).
/// Returns every `(BlockPos, new_state)` mutated during propagation.
pub fn propagate_neighbor_updates<W: BlockWorld>(
    world: &mut W,
    scheduler: &mut TickScheduler,
    current_tick: u64,
    seeds: &[BlockPos],
) -> Vec<(BlockPos, BlockStateId)> {
    let mut queue: VecDeque<BlockPos> = VecDeque::new();
    let mut queued: HashSet<BlockPos> = HashSet::new();
    let mut changed: Vec<(BlockPos, BlockStateId)> = Vec::new();

    for &s in seeds {
        enqueue_with_conductors(world, &mut queue, &mut queued, s);
    }

    let mut steps = 0usize;
    while let Some(pos) = queue.pop_front() {
        queued.remove(&pos);
        steps += 1;
        if steps > 4096 {
            break;
        }

        let st = world.get_block_state(pos);
        let beh = block_behavior(st.0);

        match beh.kind {
            KIND_REDSTONE_WIRE => {
                // Compute wire power: max(direct non-wire power, max(neighbor wire power - 1))
                let mut target_power = 0u8;
                for &(dx, dy, dz, dir_to_n) in &DIRS {
                    let np = pos.offset(dx, dy, dz);
                    let nst = world.get_block_state(np);
                    let nbeh = block_behavior(nst.0);
                    if nbeh.kind == KIND_REDSTONE_WIRE {
                        target_power = target_power.max(nbeh.power().saturating_sub(1));
                    } else {
                        target_power = target_power.max(direct_signal_toward(
                            world,
                            np,
                            opposite_dir(dir_to_n),
                        ));
                        // Solid conductor block carrying strong power
                        if nst.props().can_occlude() && nbeh.kind == 0 {
                            for &(dx2, dy2, dz2, d2) in &DIRS {
                                let np2 = np.offset(dx2, dy2, dz2);
                                if np2 != pos {
                                    target_power = target_power
                                        .max(strong_signal_into(world, np2, opposite_dir(d2)));
                                }
                            }
                        }
                    }
                }
                if target_power != beh.power() {
                    if let Some(new_id) = redstone_wire_with_power(st.0, target_power) {
                        let ns = BlockStateId(new_id);
                        world.set_block_state(pos, ns);
                        changed.push((pos, ns));
                        enqueue_with_conductors(world, &mut queue, &mut queued, pos);
                    }
                }
            }
            KIND_REDSTONE_LAMP => {
                let powered = best_neighbor_signal(world, pos) > 0;
                if powered != beh.is_lit() {
                    if let Some(new_id) = beh.toggled_lit() {
                        let ns = BlockStateId(new_id);
                        world.set_block_state(pos, ns);
                        changed.push((pos, ns));
                    }
                }
            }
            KIND_REDSTONE_TORCH => {
                let attached_pos = if beh.is_upper_or_wall() && beh.facing < 6 {
                    let (dx, dy, dz) = dir_offset(opposite_dir(beh.facing));
                    pos.offset(dx, dy, dz)
                } else {
                    pos.offset(0, -1, 0)
                };
                let input_powered = best_neighbor_signal(world, attached_pos) > 0;
                let should_be_lit = !input_powered;
                if should_be_lit != beh.is_lit() {
                    scheduler.schedule(
                        current_tick,
                        2,
                        -1,
                        pos,
                        ScheduledTickKind::RedstoneTorchUpdate,
                    );
                }
            }
            KIND_REPEATER => {
                let (dx, dy, dz) = dir_offset(beh.facing);
                let back_pos = pos.offset(dx, dy, dz);
                let input_powered = direct_signal_toward(world, back_pos, opposite_dir(beh.facing)) > 0
                    || (world.get_block_state(back_pos).props().can_occlude()
                        && best_neighbor_signal(world, back_pos) > 0);
                if input_powered != beh.is_powered() {
                    let delay_ticks = (beh.delay().clamp(1, 4) as u64) * 2;
                    scheduler.schedule(
                        current_tick,
                        delay_ticks,
                        -2,
                        pos,
                        ScheduledTickKind::RepeaterUpdate,
                    );
                }
            }
            KIND_DOOR | KIND_TRAPDOOR | KIND_FENCE_GATE => {
                let sig = best_neighbor_signal(world, pos) > 0;
                if sig != beh.is_open() {
                    if let Some(new_id) = beh.toggled_open() {
                        let ns = BlockStateId(new_id);
                        world.set_block_state(pos, ns);
                        changed.push((pos, ns));
                        if beh.kind == KIND_DOOR {
                            let other_pos = if beh.is_upper_or_wall() {
                                pos.offset(0, -1, 0)
                            } else {
                                pos.offset(0, 1, 0)
                            };
                            let ost = world.get_block_state(other_pos);
                            let obeh = block_behavior(ost.0);
                            if obeh.kind == KIND_DOOR && obeh.is_open() != sig {
                                if let Some(oid) = obeh.toggled_open() {
                                    let ons = BlockStateId(oid);
                                    world.set_block_state(other_pos, ons);
                                    changed.push((other_pos, ons));
                                }
                            }
                        }
                    }
                }
            }
            KIND_FALLING_BLOCK => {
                let below = world.get_block_state(pos.offset(0, -1, 0));
                if below.is_air() || below.props().can_be_replaced() {
                    scheduler.schedule(
                        current_tick,
                        2,
                        0,
                        pos,
                        ScheduledTickKind::FallingBlockCheck,
                    );
                }
            }
            KIND_WATER | KIND_LAVA => {
                scheduler.schedule(
                    current_tick,
                    fluid_tick_delay(beh.kind, false),
                    0,
                    pos,
                    ScheduledTickKind::FluidTick,
                );
            }
            KIND_PISTON | KIND_STICKY_PISTON => {
                let is_sticky = beh.kind == KIND_STICKY_PISTON;
                let powered = best_neighbor_signal(world, pos) > 0;
                if powered && !beh.is_open() && beh.facing < 6 {
                    // Attempt push (up to 12 blocks in facing direction)
                    let (dx, dy, dz) = dir_offset(beh.facing);
                    let mut chain = Vec::new();
                    let mut can_push = true;
                    for dist in 1..=13 {
                        let p = pos.offset(dx * dist, dy * dist, dz * dist);
                        let s = world.get_block_state(p);
                        if s.is_air() || s.props().can_be_replaced() {
                            break;
                        }
                        if dist == 13 || is_piston_immovable(s) {
                            can_push = false;
                            break;
                        }
                        chain.push((p, s));
                    }
                    if can_push {
                        if let Some(ext_id) = beh.toggled_open() {
                            // Shift pushed blocks from farthest back to closest
                            for (bp, bs) in chain.into_iter().rev() {
                                let dst = bp.offset(dx, dy, dz);
                                world.set_block_state(dst, bs);
                                changed.push((dst, bs));
                                enqueue_with_conductors(world, &mut queue, &mut queued, dst);
                            }
                            let head_pos = pos.offset(dx, dy, dz);
                            let head_st =
                                BlockStateId(PISTON_HEAD_STATES[beh.facing as usize][usize::from(is_sticky)]);
                            world.set_block_state(head_pos, head_st);
                            changed.push((head_pos, head_st));
                            let ext_st = BlockStateId(ext_id);
                            world.set_block_state(pos, ext_st);
                            changed.push((pos, ext_st));
                            enqueue_with_conductors(world, &mut queue, &mut queued, head_pos);
                        }
                    }
                } else if !powered && beh.is_open() && beh.facing < 6 {
                    // Retract piston (and pull 1 block if sticky piston!)
                    let (dx, dy, dz) = dir_offset(beh.facing);
                    if let Some(ret_id) = beh.toggled_open() {
                        let ret_st = BlockStateId(ret_id);
                        world.set_block_state(pos, ret_st);
                        changed.push((pos, ret_st));
                        let head_pos = pos.offset(dx, dy, dz);
                        let pulled_pos = pos.offset(dx * 2, dy * 2, dz * 2);
                        let pulled_st = world.get_block_state(pulled_pos);
                        if is_sticky && !pulled_st.is_air() && !pulled_st.props().can_be_replaced() && !is_piston_immovable(pulled_st) {
                            world.set_block_state(head_pos, pulled_st);
                            changed.push((head_pos, pulled_st));
                            world.set_block_state(pulled_pos, BlockStateId::AIR);
                            changed.push((pulled_pos, BlockStateId::AIR));
                            enqueue_with_conductors(world, &mut queue, &mut queued, pulled_pos);
                        } else if block_behavior(world.get_block_state(head_pos).0).kind == KIND_PISTON_HEAD {
                            world.set_block_state(head_pos, BlockStateId::AIR);
                            changed.push((head_pos, BlockStateId::AIR));
                        }
                        enqueue_with_conductors(world, &mut queue, &mut queued, head_pos);
                    }
                }
            }
            _ => {}
        }
    }

    changed
}

/// Execute a single due `ScheduledTick`, returning `(changed_blocks, optional_spawned_falling_block)`.
pub fn execute_scheduled_tick<W: BlockWorld>(
    world: &mut W,
    scheduler: &mut TickScheduler,
    current_tick: u64,
    tick: ScheduledTick,
) -> (Vec<(BlockPos, BlockStateId)>, Option<(BlockPos, BlockStateId)>) {
    let st = world.get_block_state(tick.pos);
    let beh = block_behavior(st.0);
    let mut changed = Vec::new();
    let mut falling = None;

    match tick.kind {
        ScheduledTickKind::ButtonUnpress => {
            if beh.kind == KIND_BUTTON && beh.is_powered() {
                if let Some(new_id) = beh.toggled_powered() {
                    let ns = BlockStateId(new_id);
                    world.set_block_state(tick.pos, ns);
                    changed.push((tick.pos, ns));
                    changed.extend(propagate_neighbor_updates(
                        world,
                        scheduler,
                        current_tick,
                        &[tick.pos],
                    ));
                }
            }
        }
        ScheduledTickKind::RedstoneTorchUpdate => {
            if beh.kind == KIND_REDSTONE_TORCH {
                let attached_pos = if beh.is_upper_or_wall() && beh.facing < 6 {
                    let (dx, dy, dz) = dir_offset(opposite_dir(beh.facing));
                    tick.pos.offset(dx, dy, dz)
                } else {
                    tick.pos.offset(0, -1, 0)
                };
                let should_be_lit = best_neighbor_signal(world, attached_pos) == 0;
                if should_be_lit != beh.is_lit() {
                    if let Some(new_id) = beh.toggled_lit() {
                        let ns = BlockStateId(new_id);
                        world.set_block_state(tick.pos, ns);
                        changed.push((tick.pos, ns));
                        changed.extend(propagate_neighbor_updates(
                            world,
                            scheduler,
                            current_tick,
                            &[tick.pos],
                        ));
                    }
                }
            }
        }
        ScheduledTickKind::RepeaterUpdate => {
            if beh.kind == KIND_REPEATER {
                let (dx, dy, dz) = dir_offset(beh.facing);
                let back_pos = tick.pos.offset(dx, dy, dz);
                let input_powered = direct_signal_toward(world, back_pos, opposite_dir(beh.facing)) > 0
                    || (world.get_block_state(back_pos).props().can_occlude()
                        && best_neighbor_signal(world, back_pos) > 0);
                if input_powered != beh.is_powered() {
                    if let Some(new_id) = beh.toggled_powered() {
                        let ns = BlockStateId(new_id);
                        world.set_block_state(tick.pos, ns);
                        changed.push((tick.pos, ns));
                        changed.extend(propagate_neighbor_updates(
                            world,
                            scheduler,
                            current_tick,
                            &[tick.pos],
                        ));
                    }
                }
            }
        }
        ScheduledTickKind::FallingBlockCheck => {
            if beh.kind == KIND_FALLING_BLOCK {
                let below = world.get_block_state(tick.pos.offset(0, -1, 0));
                if below.is_air() || below.props().can_be_replaced() {
                    world.set_block_state(tick.pos, BlockStateId::AIR);
                    changed.push((tick.pos, BlockStateId::AIR));
                    falling = Some((tick.pos, st));
                    changed.extend(propagate_neighbor_updates(
                        world,
                        scheduler,
                        current_tick,
                        &[tick.pos],
                    ));
                }
            }
        }
        ScheduledTickKind::BlockTick => {
            changed.extend(propagate_neighbor_updates(
                world,
                scheduler,
                current_tick,
                &[tick.pos],
            ));
        }
        ScheduledTickKind::FluidTick => {
            changed.extend(tick_fluid(world, scheduler, current_tick, tick.pos, false));
        }
    }

    (changed, falling)
}

fn is_piston_immovable(st: BlockStateId) -> bool {
    if st.props().destroy_speed < 0.0 {
        return true; // bedrock, barrier, end_portal_frame, etc.
    }
    let b = block_behavior(st.0);
    if (b.kind == KIND_PISTON || b.kind == KIND_STICKY_PISTON) && b.is_open() {
        return true;
    }
    if b.kind == KIND_PISTON_HEAD {
        return true;
    }
    matches!(
        st.block_name(),
        "minecraft:obsidian"
            | "minecraft:crying_obsidian"
            | "minecraft:respawn_anchor"
            | "minecraft:reinforced_deepslate"
            | "minecraft:chest"
            | "minecraft:trapped_chest"
            | "minecraft:ender_chest"
            | "minecraft:furnace"
            | "minecraft:blast_furnace"
            | "minecraft:smoker"
            | "minecraft:hopper"
            | "minecraft:barrel"
            | "minecraft:spawner"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_data::default_state_id;
    use std::collections::HashMap;

    struct MapWorld(HashMap<BlockPos, BlockStateId>);
    impl BlockWorld for MapWorld {
        fn get_block_state(&mut self, pos: BlockPos) -> BlockStateId {
            self.0.get(&pos).copied().unwrap_or(BlockStateId::AIR)
        }
        fn set_block_state(&mut self, pos: BlockPos, state: BlockStateId) -> bool {
            self.0.insert(pos, state);
            true
        }
    }

    #[test]
    fn lever_powers_wire_chain_and_lights_redstone_lamp() {
        let mut w = MapWorld(HashMap::new());
        let mut sched = TickScheduler::new();

        let lever = default_state_id("minecraft:lever").unwrap();
        let wire = default_state_id("minecraft:redstone_wire").unwrap();
        let lamp = default_state_id("minecraft:redstone_lamp").unwrap();

        w.set_block_state(BlockPos::new(0, 0, 0), lever);
        w.set_block_state(BlockPos::new(1, 0, 0), wire);
        w.set_block_state(BlockPos::new(2, 0, 0), wire);
        w.set_block_state(BlockPos::new(3, 0, 0), lamp);

        // Toggle lever ON
        let changed = interact_block(&mut w, &mut sched, 1, BlockPos::new(0, 0, 0)).unwrap();
        assert!(!changed.is_empty());
        assert_eq!(block_behavior(w.get_block_state(BlockPos::new(1, 0, 0)).0).power(), 15);
        assert_eq!(block_behavior(w.get_block_state(BlockPos::new(2, 0, 0)).0).power(), 14);
        let lit_lamp = w.get_block_state(BlockPos::new(3, 0, 0));
        assert!(block_behavior(lit_lamp.0).is_lit());
        assert_eq!(lit_lamp.props().light_emission, 15);

        // Toggle lever OFF -> wire decays to 0 and lamp turns OFF!
        interact_block(&mut w, &mut sched, 2, BlockPos::new(0, 0, 0)).unwrap();
        assert_eq!(block_behavior(w.get_block_state(BlockPos::new(1, 0, 0)).0).power(), 0);
        assert_eq!(block_behavior(w.get_block_state(BlockPos::new(2, 0, 0)).0).power(), 0);
        assert!(!block_behavior(w.get_block_state(BlockPos::new(3, 0, 0)).0).is_lit());
    }

    #[test]
    fn redstone_torch_inverts_after_2_ticks_and_button_unpresses() {
        let mut w = MapWorld(HashMap::new());
        let mut sched = TickScheduler::new();

        let btn = default_state_id("minecraft:stone_button").unwrap();
        let torch = default_state_id("minecraft:redstone_torch").unwrap();
        w.set_block_state(BlockPos::new(0, 0, 0), btn);
        w.set_block_state(BlockPos::new(1, 0, 0), BlockStateId::STONE);
        w.set_block_state(BlockPos::new(1, 1, 0), torch);
        assert!(block_behavior(w.get_block_state(BlockPos::new(1, 1, 0)).0).is_lit());

        // Press stone button at tick 10 -> schedules torch turn-off at tick 12 and button unpress at tick 30
        interact_block(&mut w, &mut sched, 10, BlockPos::new(0, 0, 0)).unwrap();
        assert!(block_behavior(w.get_block_state(BlockPos::new(0, 0, 0)).0).is_powered());

        for t in 11..=12 {
            for due in sched.drain_due(t) {
                execute_scheduled_tick(&mut w, &mut sched, t, due);
            }
        }
        assert!(!block_behavior(w.get_block_state(BlockPos::new(1, 1, 0)).0).is_lit());

        for t in 13..=32 {
            for due in sched.drain_due(t) {
                execute_scheduled_tick(&mut w, &mut sched, t, due);
            }
        }
        assert!(!block_behavior(w.get_block_state(BlockPos::new(0, 0, 0)).0).is_powered());
        assert!(block_behavior(w.get_block_state(BlockPos::new(1, 1, 0)).0).is_lit());
    }

    #[test]
    fn sticky_piston_pushes_and_retracts_block_chain() {
        use mc_data::block_behaviors::STICKY_PISTON_STATES;
        let mut w = MapWorld(HashMap::new());
        let mut sched = TickScheduler::new();

        let lever = default_state_id("minecraft:lever").unwrap();
        // Sticky piston facing East (dir 5), retracted
        let spiston = BlockStateId(STICKY_PISTON_STATES[5][0]);
        w.set_block_state(BlockPos::new(0, 0, 0), lever);
        w.set_block_state(BlockPos::new(1, 0, 0), spiston);
        w.set_block_state(BlockPos::new(2, 0, 0), BlockStateId::STONE);
        w.set_block_state(BlockPos::new(3, 0, 0), BlockStateId::DIRT);

        // Power lever ON -> piston extends, pushing stone to x=3 and dirt to x=4!
        interact_block(&mut w, &mut sched, 1, BlockPos::new(0, 0, 0)).unwrap();
        assert!(block_behavior(w.get_block_state(BlockPos::new(1, 0, 0)).0).is_open());
        assert_eq!(block_behavior(w.get_block_state(BlockPos::new(2, 0, 0)).0).kind, KIND_PISTON_HEAD);
        assert_eq!(w.get_block_state(BlockPos::new(3, 0, 0)), BlockStateId::STONE);
        assert_eq!(w.get_block_state(BlockPos::new(4, 0, 0)), BlockStateId::DIRT);

        // Power lever OFF -> sticky piston retracts and pulls stone back from x=3 to x=2!
        interact_block(&mut w, &mut sched, 2, BlockPos::new(0, 0, 0)).unwrap();
        assert!(!block_behavior(w.get_block_state(BlockPos::new(1, 0, 0)).0).is_open());
        assert_eq!(w.get_block_state(BlockPos::new(2, 0, 0)), BlockStateId::STONE);
        assert_eq!(w.get_block_state(BlockPos::new(3, 0, 0)), BlockStateId::AIR);
        assert_eq!(w.get_block_state(BlockPos::new(4, 0, 0)), BlockStateId::DIRT);
    }
}
