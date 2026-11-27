use mc_math::BlockPos;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ScheduledTickKind {
    BlockTick,
    ButtonUnpress,
    RedstoneTorchUpdate,
    RepeaterUpdate,
    ComparatorUpdate,
    ObserverPulseStart,
    ObserverPulseEnd,
    PressurePlateRelease,
    DispenserFire,
    FallingBlockCheck,
    FluidTick,
}

/// Deterministic scheduled block tick matching Minecraft's `SavedTick` / `ScheduledTick` ordering:
/// `(trigger_tick, priority, sub_tick_order)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledTick {
    pub trigger_tick: u64,
    pub priority: i8,
    pub sub_tick_order: u64,
    pub pos: BlockPos,
    pub kind: ScheduledTickKind,
}

impl Ord for ScheduledTick {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.trigger_tick
            .cmp(&other.trigger_tick)
            .then_with(|| self.priority.cmp(&other.priority))
            .then_with(|| self.sub_tick_order.cmp(&other.sub_tick_order))
    }
}

impl PartialOrd for ScheduledTick {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Default)]
pub struct TickScheduler {
    heap: BinaryHeap<Reverse<ScheduledTick>>,
    pending: HashSet<(BlockPos, ScheduledTickKind)>,
    next_sub_order: u64,
}

impl TickScheduler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn schedule(
        &mut self,
        current_tick: u64,
        delay_ticks: u64,
        priority: i8,
        pos: BlockPos,
        kind: ScheduledTickKind,
    ) {
        if !self.pending.insert((pos, kind)) {
            return;
        }
        let order = self.next_sub_order;
        self.next_sub_order += 1;
        self.heap.push(Reverse(ScheduledTick {
            trigger_tick: current_tick + delay_ticks.max(1),
            priority,
            sub_tick_order: order,
            pos,
            kind,
        }));
    }

    pub fn drain_due(&mut self, current_tick: u64) -> Vec<ScheduledTick> {
        let mut due = Vec::new();
        while let Some(&Reverse(top)) = self.heap.peek() {
            if top.trigger_tick > current_tick {
                break;
            }
            let Reverse(item) = self.heap.pop().unwrap();
            self.pending.remove(&(item.pos, item.kind));
            due.push(item);
        }
        due
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}
