//! Minecraft 26.3 Dispenser / Dropper redstone execution and Bed sleeping / night-skip mechanics.

use crate::{ItemStack, item_damage, set_item_damage};
use mc_data::BlockStateId;
use mc_data::block_behaviors::{
    KIND_BARREL, KIND_BED, KIND_BLAST_FURNACE, KIND_CHEST, KIND_DISPENSER, KIND_DROPPER,
    KIND_FURNACE, KIND_HOPPER, KIND_SMOKER, LAVA_STATES, WATER_STATES, block_behavior,
};
use mc_data::block_extra::{FIRE_STATE_ID, is_tnt};
use mc_data::loot_and_tools::item_combat_stats;
use mc_math::{BlockPos, RandomSource, Vec3, XoroshiroRandomSource};
use std::collections::HashMap;

// ============================================================================
// Part 1: Dispenser & Dropper Execution
// ============================================================================

/// Number of inventory slots in a vanilla Dispenser or Dropper (`DispenserBlockEntity.CONTAINER_SIZE`).
pub const DISPENSER_SLOTS: usize = 9;

/// Default accuracy parameter for `DefaultDispenseItemBehavior.spawnItem` and `ProjectileItem.DispenseConfig.DEFAULT`.
const DEFAULT_ACCURACY: f64 = 6.0;

/// Vanilla scatter coefficient used by `DefaultDispenseItemBehavior` and `Projectile.getMovementToShoot`.
const SCATTER_PER_ACCURACY: f64 = 0.0172275;

/// Vanilla scatter deviation used by `FireChargeItem` and `WindChargeItem` (`0.0172275 * 6.6666665`).
const CHARGE_SCATTER: f64 = 0.11485;

/// Fallback durability for `minecraft:flint_and_steel` if table lookup returns 0.
const FLINT_AND_STEEL_DEFAULT_MAX_DAMAGE: i32 = 64;

/// Result of executing a Dispenser or Dropper tick against its 9-slot inventory.
#[derive(Debug, Clone, PartialEq)]
pub enum DispenserAction {
    /// No item could be dispensed (empty inventory or failed optional action): play level event `1001`.
    EmptyClick { pos: BlockPos },
    /// Eject an item entity out of the front face with vanilla velocity + deterministic scatter.
    DropItem {
        pos: Vec3,
        vel: Vec3,
        stack: ItemStack,
    },
    /// Dropper pushing 1 item directly into a container block in front of it.
    TransferToContainer {
        target_pos: BlockPos,
        stack: ItemStack,
    },
    /// Dispenser firing a projectile entity (arrow, spectral_arrow, snowball, egg, small_fireball, wind_charge, experience_bottle).
    SpawnProjectile {
        kind_name: &'static str,
        pos: Vec3,
        vel: Vec3,
        damage: f32,
    },
    /// Dispenser igniting a primed TNT entity at `pos` (from dispensing TNT or flint_and_steel on TNT).
    IgniteTnt { pos: BlockPos },
    /// Dispenser placing a block state in front (water source, lava source, or fire).
    PlaceBlock {
        pos: BlockPos,
        state: BlockStateId,
    },
    /// Dispenser picking up a water or lava source block at `pos` (caller should clear `pos` to air).
    PickupFluid { pos: BlockPos },
    /// Dispenser spawning a mob from a `*_spawn_egg` item (`entity_type` is `"minecraft:<mob>"`).
    SpawnMobEgg { entity_type: String, pos: Vec3 },
}

/// Convert a Minecraft 26.3 block facing index (`0..=5`) into its `(dx, dy, dz)` unit step.
///
/// Matches `mc-world::redstone::DIRS`:
/// `0 = Down (0, -1, 0)`, `1 = Up (0, 1, 0)`, `2 = North (0, 0, -1)`,
/// `3 = South (0, 0, 1)`, `4 = West (-1, 0, 0)`, `5 = East (1, 0, 0)`.
#[inline]
pub const fn facing_offset(facing: u8) -> (i32, i32, i32) {
    match facing {
        0 => (0, -1, 0),
        1 => (0, 1, 0),
        2 => (0, 0, -1),
        3 => (0, 0, 1),
        4 => (-1, 0, 0),
        5 => (1, 0, 0),
        _ => (0, 0, 0),
    }
}

/// Returns `true` if `state` is a `minecraft:dispenser`.
#[inline]
pub fn is_dispenser(state: BlockStateId) -> bool {
    block_behavior(state.0).kind == KIND_DISPENSER
}

/// Returns `true` if `state` is a `minecraft:dropper`.
#[inline]
pub fn is_dropper(state: BlockStateId) -> bool {
    block_behavior(state.0).kind == KIND_DROPPER
}

/// Returns `true` if `state` is any colored bed block (`KIND_BED`).
#[inline]
pub fn is_bed(state: BlockStateId) -> bool {
    block_behavior(state.0).kind == KIND_BED
}

/// Returns the facing direction (`0..=5`) of a dispenser or dropper block state, if applicable.
#[inline]
pub fn dispenser_facing(state: BlockStateId) -> Option<u8> {
    let beh = block_behavior(state.0);
    if (beh.kind == KIND_DISPENSER || beh.kind == KIND_DROPPER) && beh.facing < 6 {
        Some(beh.facing)
    } else {
        None
    }
}

/// Returns `true` if `state` is a container block that a Dropper can push items into.
#[inline]
pub fn is_container_block(state: BlockStateId) -> bool {
    matches!(
        block_behavior(state.0).kind,
        KIND_CHEST
            | KIND_BARREL
            | KIND_FURNACE
            | KIND_BLAST_FURNACE
            | KIND_SMOKER
            | KIND_HOPPER
            | KIND_DISPENSER
            | KIND_DROPPER
    )
}

/// Exact vanilla `DispenserBlockEntity.getRandomSlot` uniform reservoir sampling over non-empty slots.
///
/// Uses `XoroshiroRandomSource` seeded deterministically from `seed` so consecutive tick seeds
/// yield uncorrelated uniform samples across all non-empty slots.
pub fn pick_random_non_empty_slot(slots: &[ItemStack], seed: u64) -> Option<usize> {
    let mut rng = XoroshiroRandomSource::new(seed as i64);
    let mut chosen: Option<usize> = None;
    let mut non_empty_count: i32 = 0;

    for (idx, stack) in slots.iter().enumerate() {
        if !stack.is_empty() {
            non_empty_count += 1;
            if rng.next_int_bounded(non_empty_count) == 0 {
                chosen = Some(idx);
            }
        }
    }

    chosen
}

#[inline]
fn random_triangle(rng: &mut XoroshiroRandomSource, mode: f64, deviation: f64) -> f64 {
    mode + deviation * (rng.next_double() - rng.next_double())
}

#[inline]
fn dispense_position(pos: BlockPos, facing: u8, scale: f64, y_offset: f64) -> Vec3 {
    let (dx, dy, dz) = facing_offset(facing);
    Vec3::new(
        pos.x as f64 + 0.5 + scale * dx as f64,
        pos.y as f64 + 0.5 + scale * dy as f64 + y_offset,
        pos.z as f64 + 0.5 + scale * dz as f64,
    )
}

fn take_one(slot: &mut ItemStack) -> ItemStack {
    let mut one = slot.clone();
    one.count = 1;
    if slot.count <= 1 {
        *slot = ItemStack::EMPTY;
    } else {
        slot.count -= 1;
    }
    one
}

/// Insert a remainder stack into `slots`, merging into a matching stack first or filling an empty slot.
fn insert_remainder(slots: &mut [ItemStack], remainder: ItemStack) -> bool {
    if remainder.is_empty() {
        return true;
    }
    let max_stack = remainder.max_stack_size();
    if !remainder.has_components() && max_stack > 1 {
        for slot in slots.iter_mut() {
            if !slot.is_empty()
                && slot.item_id == remainder.item_id
                && !slot.has_components()
                && slot.count.saturating_add(remainder.count) <= max_stack
            {
                slot.count += remainder.count;
                return true;
            }
        }
    }
    for slot in slots.iter_mut() {
        if slot.is_empty() {
            *slot = remainder;
            return true;
        }
    }
    false
}

/// Vanilla `DefaultDispenseItemBehavior.consumeWithRemainder`:
/// if chosen slot has count 1, replaces it in place with `remainder`;
/// otherwise decrements the slot and inserts `remainder` into an available slot.
fn consume_with_remainder(slots: &mut [ItemStack], slot_idx: usize, remainder: ItemStack) {
    if slots[slot_idx].count <= 1 {
        slots[slot_idx] = remainder;
    } else {
        slots[slot_idx].count -= 1;
        let _ = insert_remainder(slots, remainder);
    }
}

/// Vanilla `DefaultDispenseItemBehavior.spawnItem` position and velocity calculation.
fn make_drop_action(
    pos: BlockPos,
    facing: u8,
    stack: ItemStack,
    rng: &mut XoroshiroRandomSource,
) -> DispenserAction {
    let (dx, dy, dz) = facing_offset(facing);
    let y_adjust = if dy != 0 { -0.125 } else { -0.15625 };
    let spawn_pos = dispense_position(pos, facing, 0.7, y_adjust);
    let pow = rng.next_double() * 0.1 + 0.2;
    let dev = SCATTER_PER_ACCURACY * DEFAULT_ACCURACY;
    let vel = Vec3::new(
        random_triangle(rng, dx as f64 * pow, dev),
        random_triangle(rng, 0.2, dev),
        random_triangle(rng, dz as f64 * pow, dev),
    );
    DispenserAction::DropItem {
        pos: spawn_pos,
        vel,
        stack,
    }
}

/// Vanilla `ProjectileDispenseBehavior` / `Projectile.getMovementToShoot` launch calculation.
fn make_standard_projectile(
    kind_name: &'static str,
    pos: BlockPos,
    facing: u8,
    power: f64,
    uncertainty: f64,
    damage: f32,
    rng: &mut XoroshiroRandomSource,
) -> DispenserAction {
    let (dx, dy, dz) = facing_offset(facing);
    let spawn_pos = dispense_position(pos, facing, 0.7, 0.1);
    let dev = SCATTER_PER_ACCURACY * uncertainty;
    let vel = Vec3::new(
        (dx as f64 + random_triangle(rng, 0.0, dev)) * power,
        (dy as f64 + random_triangle(rng, 0.0, dev)) * power,
        (dz as f64 + random_triangle(rng, 0.0, dev)) * power,
    );
    DispenserAction::SpawnProjectile {
        kind_name,
        pos: spawn_pos,
        vel,
        damage,
    }
}

/// Vanilla `FireChargeItem` / `WindChargeItem` dispenser launch calculation.
fn make_charge_projectile(
    kind_name: &'static str,
    pos: BlockPos,
    facing: u8,
    normalize: bool,
    damage: f32,
    rng: &mut XoroshiroRandomSource,
) -> DispenserAction {
    let (dx, dy, dz) = facing_offset(facing);
    let spawn_pos = dispense_position(pos, facing, 1.0, 0.0);
    let mut vx = random_triangle(rng, dx as f64, CHARGE_SCATTER);
    let mut vy = random_triangle(rng, dy as f64, CHARGE_SCATTER);
    let mut vz = random_triangle(rng, dz as f64, CHARGE_SCATTER);
    if normalize {
        let len = (vx * vx + vy * vy + vz * vz).sqrt();
        if len > 1.0e-9 {
            vx /= len;
            vy /= len;
            vz /= len;
        }
    }
    DispenserAction::SpawnProjectile {
        kind_name,
        pos: spawn_pos,
        vel: Vec3::new(vx, vy, vz),
        damage,
    }
}

/// Damage a durability tool (such as `minecraft:flint_and_steel`) by 1, breaking it at `max_damage`.
fn damage_tool_in_slot(slot: &mut ItemStack) {
    let stats_max = item_combat_stats(slot.item_id).max_damage as i32;
    let max_damage = if stats_max > 0 {
        stats_max
    } else {
        FLINT_AND_STEEL_DEFAULT_MAX_DAMAGE
    };
    let next_damage = item_damage(slot).saturating_add(1);
    if next_damage >= max_damage {
        if slot.count <= 1 {
            *slot = ItemStack::EMPTY;
        } else {
            slot.count -= 1;
            set_item_damage(slot, 0);
        }
    } else {
        set_item_damage(slot, next_damage);
    }
}

/// Execute a Dispenser or Dropper firing from its inventory `slots` at `pos` facing `facing`.
///
/// Mutates `slots` in place (consuming items, swapping filled/empty buckets, or damaging tools)
/// and returns the world action for the caller to apply.
pub fn dispense_from_slots(
    is_dropper: bool,
    pos: BlockPos,
    facing: u8,
    slots: &mut [ItemStack],
    front_state: BlockStateId,
    front_has_container: bool,
    seed: u64,
) -> DispenserAction {
    let Some(slot_idx) = pick_random_non_empty_slot(slots, seed) else {
        return DispenserAction::EmptyClick { pos };
    };

    let (dx, dy, dz) = facing_offset(facing);
    let front_pos = pos.offset(dx, dy, dz);
    // Derive an independent RNG stream from `seed` for scatter/velocity.
    let mut scatter_rng = XoroshiroRandomSource::new(seed.wrapping_add(0x9E37_79B9_7F4A_7C15) as i64);

    if is_dropper {
        let stack = take_one(&mut slots[slot_idx]);
        if front_has_container {
            return DispenserAction::TransferToContainer {
                target_pos: front_pos,
                stack,
            };
        }
        return make_drop_action(pos, facing, stack, &mut scatter_rng);
    }

    let item_name = slots[slot_idx].info().map(|i| i.name).unwrap_or("");
    let front_replaceable = front_state.is_air() || front_state.props().can_be_replaced();

    match item_name {
        // Projectiles (`ProjectileDispenseBehavior`)
        "minecraft:arrow" | "minecraft:tipped_arrow" => {
            take_one(&mut slots[slot_idx]);
            make_standard_projectile("minecraft:arrow", pos, facing, 1.1, 6.0, 2.0, &mut scatter_rng)
        }
        "minecraft:spectral_arrow" => {
            take_one(&mut slots[slot_idx]);
            make_standard_projectile(
                "minecraft:spectral_arrow",
                pos,
                facing,
                1.1,
                6.0,
                2.0,
                &mut scatter_rng,
            )
        }
        "minecraft:snowball" => {
            take_one(&mut slots[slot_idx]);
            make_standard_projectile(
                "minecraft:snowball",
                pos,
                facing,
                1.1,
                6.0,
                0.0,
                &mut scatter_rng,
            )
        }
        "minecraft:egg" | "minecraft:blue_egg" | "minecraft:brown_egg" => {
            take_one(&mut slots[slot_idx]);
            make_standard_projectile("minecraft:egg", pos, facing, 1.1, 6.0, 0.0, &mut scatter_rng)
        }
        "minecraft:experience_bottle" => {
            take_one(&mut slots[slot_idx]);
            make_standard_projectile(
                "minecraft:experience_bottle",
                pos,
                facing,
                1.375,
                3.0,
                0.0,
                &mut scatter_rng,
            )
        }
        "minecraft:fire_charge" => {
            take_one(&mut slots[slot_idx]);
            make_charge_projectile(
                "minecraft:small_fireball",
                pos,
                facing,
                true,
                5.0,
                &mut scatter_rng,
            )
        }
        "minecraft:wind_charge" => {
            take_one(&mut slots[slot_idx]);
            make_charge_projectile(
                "minecraft:wind_charge",
                pos,
                facing,
                false,
                1.0,
                &mut scatter_rng,
            )
        }

        // TNT priming
        "minecraft:tnt" => {
            if front_replaceable {
                take_one(&mut slots[slot_idx]);
                DispenserAction::IgniteTnt { pos: front_pos }
            } else {
                DispenserAction::EmptyClick { pos }
            }
        }

        // Filled fluid buckets -> place source block and return empty bucket
        "minecraft:water_bucket" | "minecraft:lava_bucket" => {
            if front_replaceable {
                let state = if item_name == "minecraft:water_bucket" {
                    BlockStateId(WATER_STATES[0])
                } else {
                    BlockStateId(LAVA_STATES[0])
                };
                let empty_bucket =
                    ItemStack::from_name("minecraft:bucket", 1).unwrap_or(ItemStack::EMPTY);
                consume_with_remainder(slots, slot_idx, empty_bucket);
                DispenserAction::PlaceBlock {
                    pos: front_pos,
                    state,
                }
            } else {
                let stack = take_one(&mut slots[slot_idx]);
                make_drop_action(pos, facing, stack, &mut scatter_rng)
            }
        }

        // Empty bucket -> pick up water or lava source block
        "minecraft:bucket" => {
            let filled_name = if front_state.0 == WATER_STATES[0] {
                Some("minecraft:water_bucket")
            } else if front_state.0 == LAVA_STATES[0] {
                Some("minecraft:lava_bucket")
            } else {
                None
            };
            if let Some(name) = filled_name {
                let filled = ItemStack::from_name(name, 1).unwrap_or(ItemStack::EMPTY);
                consume_with_remainder(slots, slot_idx, filled);
                DispenserAction::PickupFluid { pos: front_pos }
            } else {
                let stack = take_one(&mut slots[slot_idx]);
                make_drop_action(pos, facing, stack, &mut scatter_rng)
            }
        }

        // Flint and steel -> place fire into air, or prime TNT block in front, damaging durability
        "minecraft:flint_and_steel" => {
            if front_state.is_air() {
                damage_tool_in_slot(&mut slots[slot_idx]);
                DispenserAction::PlaceBlock {
                    pos: front_pos,
                    state: BlockStateId(FIRE_STATE_ID),
                }
            } else if is_tnt(front_state.0) {
                damage_tool_in_slot(&mut slots[slot_idx]);
                DispenserAction::IgniteTnt { pos: front_pos }
            } else {
                DispenserAction::EmptyClick { pos }
            }
        }

        // Spawn eggs (`*_spawn_egg` -> `minecraft:<mob>`)
        name if name.ends_with("_spawn_egg") => {
            let stripped = name.strip_suffix("_spawn_egg").unwrap_or(name);
            let entity_type = if stripped.contains(':') {
                stripped.to_string()
            } else {
                format!("minecraft:{stripped}")
            };
            take_one(&mut slots[slot_idx]);
            DispenserAction::SpawnMobEgg {
                entity_type,
                pos: Vec3::new(
                    front_pos.x as f64 + 0.5,
                    front_pos.y as f64,
                    front_pos.z as f64 + 0.5,
                ),
            }
        }

        // Default fallback: eject 1 item out the front face
        _ => {
            let stack = take_one(&mut slots[slot_idx]);
            make_drop_action(pos, facing, stack, &mut scatter_rng)
        }
    }
}

// ============================================================================
// Part 2: Bed Sleeping & Night Skip (`SleepManager`)
// ============================================================================

/// Full Minecraft day-night cycle length in ticks (`24000` ticks = 20 minutes).
pub const DAY_LENGTH_TICKS: i64 = 24_000;

/// Exact vanilla deep-sleep duration (`Player.SLEEP_DURATION = 100` ticks = 5 seconds).
pub const VANILLA_SLEEP_TICKS: u16 = 100;

/// Accelerated sleep duration (1 second) for custom server configurations or fast tests.
pub const FAST_SLEEP_TICKS: u16 = 20;

/// Vanilla Nether/End bed explosion power (`BedBlock.destroyOnUse`: `5.0F`).
pub const BED_EXPLOSION_POWER: f32 = 5.0;

/// Result of a player right-clicking a bed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BedInteractResult {
    /// Bed used in the Nether or End: caller should remove the bed and trigger a power-5.0 explosion.
    ExplodeInDimension { pos: BlockPos },
    /// Bed used in the Overworld when it is daytime and not thundering:
    /// sets the player's respawn point to `spawn_pos` and notifies that sleeping is only allowed at night or during thunderstorms.
    SetSpawnAndNotNight { spawn_pos: BlockPos },
    /// Player is outside vanilla bed reach (`|dx| > 3.0`, `|dy| > 2.0`, or `|dz| > 3.0` from bed bottom-center).
    TooFar,
    /// Player entered the bed and started sleeping.
    StartedSleeping {
        bed_pos: BlockPos,
        sleeping_count: usize,
        required_count: usize,
    },
}

/// Outcome of ticking `SleepManager` for one server tick.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SleepTickResult {
    /// `true` if enough players reached deep sleep and the night should be skipped.
    pub skip_night: bool,
    /// Next morning `day_time` (`day_time + (24000 - day_time.rem_euclid(24000))`) when `skip_night` is `true`.
    pub new_day_time: Option<i64>,
    /// `true` if weather should be cleared (`is_raining || is_thundering`) on night skip.
    pub clear_weather: bool,
    /// Sorted list of player entity IDs woken up on this tick (either by night skip or natural dawn).
    pub woke_players: Vec<i32>,
}

/// Server-side tracker for sleeping Overworld players and vanilla `SleepStatus` night-skip logic.
#[derive(Debug, Clone, Default)]
pub struct SleepManager {
    /// `player_eid -> (bed_pos, sleep_ticks)`
    sleepers: HashMap<i32, (BlockPos, u16)>,
}

impl SleepManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Exact vanilla check for whether a bed can be slept in at the current Overworld time & weather.
    ///
    /// - Thundering: always allowed (even during daytime).
    /// - Raining: allowed when `time_of_day` in `12010..=23991`.
    /// - Clear: allowed when `time_of_day` in `12542..=23459`.
    #[inline]
    pub fn is_night_or_thunder(day_time: i64, is_raining: bool, is_thundering: bool) -> bool {
        if is_thundering {
            return true;
        }
        let time_of_day = day_time.rem_euclid(DAY_LENGTH_TICKS);
        if is_raining {
            (12_010..=23_991).contains(&time_of_day)
        } else {
            (12_542..=23_459).contains(&time_of_day)
        }
    }

    /// Exact vanilla `SleepStatus.sleepersNeeded(int percentage)` calculation for `playersSleepingPercentage`.
    ///
    /// - `sleeping_percentage > 100`: returns `usize::MAX` (night skipping disabled).
    /// - `sleeping_percentage <= 0`: returns `1` (a single sleeping player skips the night).
    /// - `1..=100`: `ceil(percentage * total_overworld_players / 100.0).max(1)`.
    #[inline]
    pub fn required_sleepers(total_overworld_players: usize, sleeping_percentage: i32) -> usize {
        if sleeping_percentage > 100 {
            return usize::MAX;
        }
        if sleeping_percentage <= 0 {
            return 1;
        }
        let product = (sleeping_percentage as u128).saturating_mul(total_overworld_players as u128);
        let needed = product.div_ceil(100).max(1);
        needed.min(usize::MAX as u128) as usize
    }

    /// Handle a player attempting to sleep in a bed at `bed_pos`.
    #[allow(clippy::too_many_arguments)]
    pub fn try_sleep(
        &mut self,
        player_eid: i32,
        player_pos: Vec3,
        bed_pos: BlockPos,
        is_overworld: bool,
        day_time: i64,
        is_raining: bool,
        is_thundering: bool,
        total_overworld_players: usize,
        sleeping_percentage: i32,
    ) -> BedInteractResult {
        // 1. Non-Overworld beds (Nether / End) explode immediately on use (`BedRule.DESTROY_ON_USE`).
        if !is_overworld {
            return BedInteractResult::ExplodeInDimension { pos: bed_pos };
        }

        // 2. Vanilla `ServerPlayer.isReachableBedBlock`: |dx| <= 3.0, |dy| <= 2.0, |dz| <= 3.0 from bottom-center.
        let bed_cx = bed_pos.x as f64 + 0.5;
        let bed_cy = bed_pos.y as f64;
        let bed_cz = bed_pos.z as f64 + 0.5;
        if (player_pos.x - bed_cx).abs() > 3.0
            || (player_pos.y - bed_cy).abs() > 2.0
            || (player_pos.z - bed_cz).abs() > 3.0
        {
            return BedInteractResult::TooFar;
        }

        // 3. Overworld daytime (non-thunderstorm): still sets spawn point, cannot enter sleep.
        if !Self::is_night_or_thunder(day_time, is_raining, is_thundering) {
            return BedInteractResult::SetSpawnAndNotNight { spawn_pos: bed_pos };
        }

        // 4. Enter bed (`sleepCounter = 0` unless already sleeping in this bed).
        self.sleepers
            .entry(player_eid)
            .and_modify(|(pos, ticks)| {
                if *pos != bed_pos {
                    *pos = bed_pos;
                    *ticks = 0;
                }
            })
            .or_insert((bed_pos, 0));

        let sleeping_count = self.sleepers.len();
        let required_count = Self::required_sleepers(total_overworld_players, sleeping_percentage);

        BedInteractResult::StartedSleeping {
            bed_pos,
            sleeping_count,
            required_count,
        }
    }

    /// Wake a single player (e.g. "Leave Bed", damage, disconnect). Returns `true` if they were sleeping.
    pub fn wake_player(&mut self, player_eid: i32) -> bool {
        self.sleepers.remove(&player_eid).is_some()
    }

    /// Returns `true` if `player_eid` is currently sleeping.
    #[inline]
    pub fn is_sleeping(&self, player_eid: i32) -> bool {
        self.sleepers.contains_key(&player_eid)
    }

    /// Number of currently sleeping players.
    #[inline]
    pub fn sleeping_count(&self) -> usize {
        self.sleepers.len()
    }

    /// Returns the bed position for a sleeping player, if sleeping.
    #[inline]
    pub fn bed_pos(&self, player_eid: i32) -> Option<BlockPos> {
        self.sleepers.get(&player_eid).map(|(pos, _)| *pos)
    }

    /// Returns the accumulated `sleep_ticks` for a sleeping player, if sleeping.
    #[inline]
    pub fn sleep_ticks(&self, player_eid: i32) -> Option<u16> {
        self.sleepers.get(&player_eid).map(|(_, ticks)| *ticks)
    }

    /// Advance sleeping state by 1 tick using vanilla's 100-tick (`VANILLA_SLEEP_TICKS`) deep-sleep threshold.
    pub fn tick(
        &mut self,
        total_overworld_players: usize,
        sleeping_percentage: i32,
        day_time: i64,
        is_raining: bool,
        is_thundering: bool,
    ) -> SleepTickResult {
        self.tick_with_threshold(
            total_overworld_players,
            sleeping_percentage,
            day_time,
            is_raining,
            is_thundering,
            VANILLA_SLEEP_TICKS,
        )
    }

    /// Advance sleeping state by 1 tick with a configurable deep-sleep threshold (`required_ticks`).
    ///
    /// - If daytime arrives naturally (not night and not thundering), wakes all sleepers without skipping time.
    /// - Otherwise increments every sleeper's `sleep_ticks` (capped at `max(100, required_ticks)`).
    /// - When at least `required_sleepers` players have `sleep_ticks >= required_ticks`,
    ///   triggers `skip_night = true`, advances to next morning (`day_time + (24000 - day_time.rem_euclid(24000))`),
    ///   sets `clear_weather = is_raining || is_thundering`, clears all sleepers, and returns their IDs.
    pub fn tick_with_threshold(
        &mut self,
        total_overworld_players: usize,
        sleeping_percentage: i32,
        day_time: i64,
        is_raining: bool,
        is_thundering: bool,
        required_ticks: u16,
    ) -> SleepTickResult {
        if self.sleepers.is_empty() {
            return SleepTickResult::default();
        }

        // Natural dawn (or thunderstorm ended during day): wake all sleeping players.
        if !Self::is_night_or_thunder(day_time, is_raining, is_thundering) {
            let mut woke_players: Vec<i32> = self.sleepers.keys().copied().collect();
            woke_players.sort_unstable();
            self.sleepers.clear();
            return SleepTickResult {
                skip_night: false,
                new_day_time: None,
                clear_weather: false,
                woke_players,
            };
        }

        let cap = VANILLA_SLEEP_TICKS.max(required_ticks);
        for (_, ticks) in self.sleepers.values_mut() {
            *ticks = ticks.saturating_add(1).min(cap);
        }

        let needed = Self::required_sleepers(total_overworld_players, sleeping_percentage);
        let deep_sleepers = self
            .sleepers
            .values()
            .filter(|(_, ticks)| *ticks >= required_ticks)
            .count();

        if deep_sleepers >= needed {
            let rem = day_time.rem_euclid(DAY_LENGTH_TICKS);
            let next_morning = day_time + (DAY_LENGTH_TICKS - rem);
            let mut woke_players: Vec<i32> = self.sleepers.keys().copied().collect();
            woke_players.sort_unstable();
            self.sleepers.clear();
            SleepTickResult {
                skip_night: true,
                new_day_time: Some(next_morning),
                clear_weather: is_raining || is_thundering,
                woke_players,
            }
        } else {
            SleepTickResult::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_data::block_extra::TNT_STATE_ID;
    use mc_data::items::item_by_name;

    fn empty_dispenser_slots() -> [ItemStack; DISPENSER_SLOTS] {
        std::array::from_fn(|_| ItemStack::EMPTY)
    }

    #[test]
    fn facing_offsets_match_minecraft_direction_order() {
        assert_eq!(facing_offset(0), (0, -1, 0)); // Down
        assert_eq!(facing_offset(1), (0, 1, 0)); // Up
        assert_eq!(facing_offset(2), (0, 0, -1)); // North
        assert_eq!(facing_offset(3), (0, 0, 1)); // South
        assert_eq!(facing_offset(4), (-1, 0, 0)); // West
        assert_eq!(facing_offset(5), (1, 0, 0)); // East
        assert_eq!(facing_offset(99), (0, 0, 0));
    }

    #[test]
    fn random_slot_selection_is_uniform_over_non_empty_slots() {
        let mut slots = empty_dispenser_slots();
        assert_eq!(pick_random_non_empty_slot(&slots, 42), None);

        slots[2] = ItemStack::from_name("minecraft:arrow", 4).unwrap();
        for s in 0..20 {
            assert_eq!(pick_random_non_empty_slot(&slots, s), Some(2));
        }

        slots[5] = ItemStack::from_name("minecraft:snowball", 4).unwrap();
        slots[8] = ItemStack::from_name("minecraft:egg", 4).unwrap();

        let mut counts = [0usize; DISPENSER_SLOTS];
        for s in 0..3000u64 {
            let idx = pick_random_non_empty_slot(&slots, s).unwrap();
            assert!(matches!(idx, 2 | 5 | 8), "empty slot {idx} should never be picked");
            counts[idx] += 1;
        }
        // Each of the 3 slots should be picked roughly 1000 times (allow wide chi-square margin).
        for idx in [2, 5, 8] {
            assert!(
                (850..=1150).contains(&counts[idx]),
                "slot {idx} picked {} times out of 3000",
                counts[idx]
            );
        }
    }

    #[test]
    fn dispenser_empty_returns_empty_click() {
        let mut slots = empty_dispenser_slots();
        let pos = BlockPos::new(5, 64, 5);
        let action = dispense_from_slots(false, pos, 5, &mut slots, BlockStateId::AIR, false, 1);
        assert_eq!(action, DispenserAction::EmptyClick { pos });
    }

    #[test]
    fn dispenser_fires_projectiles_with_correct_kind_and_velocity() {
        let pos = BlockPos::new(0, 64, 0);
        let cases = [
            ("minecraft:arrow", "minecraft:arrow", 2.0_f32),
            ("minecraft:tipped_arrow", "minecraft:arrow", 2.0_f32),
            ("minecraft:spectral_arrow", "minecraft:spectral_arrow", 2.0_f32),
            ("minecraft:snowball", "minecraft:snowball", 0.0_f32),
            ("minecraft:egg", "minecraft:egg", 0.0_f32),
            ("minecraft:experience_bottle", "minecraft:experience_bottle", 0.0_f32),
            ("minecraft:fire_charge", "minecraft:small_fireball", 5.0_f32),
            ("minecraft:wind_charge", "minecraft:wind_charge", 1.0_f32),
        ];

        for (item_name, expected_kind, expected_dmg) in cases {
            let mut slots = empty_dispenser_slots();
            slots[0] = ItemStack::from_name(item_name, 3).unwrap();
            // Facing East (+X = 5)
            let action =
                dispense_from_slots(false, pos, 5, &mut slots, BlockStateId::AIR, false, 123);
            match action {
                DispenserAction::SpawnProjectile {
                    kind_name,
                    pos: spawn_pos,
                    vel,
                    damage,
                } => {
                    assert_eq!(kind_name, expected_kind, "item={item_name}");
                    assert_eq!(damage, expected_dmg, "item={item_name}");
                    assert!(spawn_pos.x > 1.0, "expected east spawn x for {item_name}");
                    assert!(vel.x > 0.7, "expected strong +X vel for {item_name}: {vel:?}");
                }
                other => panic!("expected SpawnProjectile for {item_name}, got {other:?}"),
            }
            assert_eq!(slots[0].count, 2, "should consume 1 {item_name}");
        }
    }

    #[test]
    fn dispenser_tnt_ignites_into_air_and_fails_into_solid() {
        let pos = BlockPos::new(10, 64, 10);
        let mut slots = empty_dispenser_slots();
        slots[0] = ItemStack::from_name("minecraft:tnt", 2).unwrap();

        // Solid stone in front -> EmptyClick, does not consume TNT
        let blocked = dispense_from_slots(false, pos, 3, &mut slots, BlockStateId::STONE, false, 7);
        assert_eq!(blocked, DispenserAction::EmptyClick { pos });
        assert_eq!(slots[0].count, 2);

        // Air in front -> IgniteTnt at South neighbor, consumes 1
        let ignited = dispense_from_slots(false, pos, 3, &mut slots, BlockStateId::AIR, false, 7);
        assert_eq!(
            ignited,
            DispenserAction::IgniteTnt {
                pos: BlockPos::new(10, 64, 11)
            }
        );
        assert_eq!(slots[0].count, 1);
    }

    #[test]
    fn dispenser_water_and_lava_buckets_place_fluid_and_return_empty_bucket() {
        let pos = BlockPos::new(0, 70, 0);
        let empty_bucket_id = item_by_name("minecraft:bucket").unwrap().id;

        // Water bucket into air
        let mut slots = empty_dispenser_slots();
        slots[0] = ItemStack::from_name("minecraft:water_bucket", 1).unwrap();
        let action = dispense_from_slots(false, pos, 2, &mut slots, BlockStateId::AIR, false, 99);
        assert_eq!(
            action,
            DispenserAction::PlaceBlock {
                pos: BlockPos::new(0, 70, -1),
                state: BlockStateId(WATER_STATES[0]),
            }
        );
        assert_eq!(slots[0].item_id, empty_bucket_id);
        assert_eq!(slots[0].count, 1);

        // Lava bucket into air
        let mut slots = empty_dispenser_slots();
        slots[4] = ItemStack::from_name("minecraft:lava_bucket", 1).unwrap();
        let action = dispense_from_slots(false, pos, 1, &mut slots, BlockStateId::AIR, false, 5);
        assert_eq!(
            action,
            DispenserAction::PlaceBlock {
                pos: BlockPos::new(0, 71, 0),
                state: BlockStateId(LAVA_STATES[0]),
            }
        );
        assert_eq!(slots[4].item_id, empty_bucket_id);
        assert_eq!(slots[4].count, 1);

        // Water bucket facing solid stone falls back to DropItem
        let mut slots = empty_dispenser_slots();
        slots[0] = ItemStack::from_name("minecraft:water_bucket", 1).unwrap();
        let fallback =
            dispense_from_slots(false, pos, 5, &mut slots, BlockStateId::STONE, false, 5);
        assert!(matches!(fallback, DispenserAction::DropItem { .. }));
        assert!(slots[0].is_empty());
    }

    #[test]
    fn dispenser_empty_bucket_picks_up_water_and_lava_sources() {
        let pos = BlockPos::new(3, 64, 3);
        let water_bucket_id = item_by_name("minecraft:water_bucket").unwrap().id;
        let lava_bucket_id = item_by_name("minecraft:lava_bucket").unwrap().id;
        let empty_bucket_id = item_by_name("minecraft:bucket").unwrap().id;

        // Single empty bucket -> replaced in place with lava_bucket
        let mut slots = empty_dispenser_slots();
        slots[0] = ItemStack::from_name("minecraft:bucket", 1).unwrap();
        let action = dispense_from_slots(
            false,
            pos,
            5,
            &mut slots,
            BlockStateId(LAVA_STATES[0]),
            false,
            11,
        );
        assert_eq!(
            action,
            DispenserAction::PickupFluid {
                pos: BlockPos::new(4, 64, 3)
            }
        );
        assert_eq!(slots[0].item_id, lava_bucket_id);
        assert_eq!(slots[0].count, 1);

        // Stacked empty buckets (count = 5) -> decrements stack and places water_bucket in an empty slot
        let mut slots = empty_dispenser_slots();
        slots[0] = ItemStack::from_name("minecraft:bucket", 5).unwrap();
        let action = dispense_from_slots(
            false,
            pos,
            4,
            &mut slots,
            BlockStateId(WATER_STATES[0]),
            false,
            12,
        );
        assert_eq!(
            action,
            DispenserAction::PickupFluid {
                pos: BlockPos::new(2, 64, 3)
            }
        );
        assert_eq!(slots[0].item_id, empty_bucket_id);
        assert_eq!(slots[0].count, 4);
        assert_eq!(slots[1].item_id, water_bucket_id);
        assert_eq!(slots[1].count, 1);

        // Flowing water (not source!) -> falls back to dropping the bucket
        let mut slots = empty_dispenser_slots();
        slots[0] = ItemStack::from_name("minecraft:bucket", 1).unwrap();
        let action = dispense_from_slots(
            false,
            pos,
            4,
            &mut slots,
            BlockStateId(WATER_STATES[1]),
            false,
            13,
        );
        assert!(matches!(action, DispenserAction::DropItem { .. }));
        assert!(slots[0].is_empty());
    }

    #[test]
    fn dispenser_flint_and_steel_places_fire_primes_tnt_and_breaks_at_max_damage() {
        let pos = BlockPos::new(0, 64, 0);
        let mut slots = empty_dispenser_slots();
        slots[0] = ItemStack::from_name("minecraft:flint_and_steel", 1).unwrap();

        // Air -> places fire and deals 1 durability damage
        let a1 = dispense_from_slots(false, pos, 5, &mut slots, BlockStateId::AIR, false, 1);
        assert_eq!(
            a1,
            DispenserAction::PlaceBlock {
                pos: BlockPos::new(1, 64, 0),
                state: BlockStateId(FIRE_STATE_ID),
            }
        );
        assert!(!slots[0].is_empty());
        assert_eq!(item_damage(&slots[0]), 1);

        // TNT block in front -> primes TNT and deals 1 durability damage
        let a2 = dispense_from_slots(
            false,
            pos,
            5,
            &mut slots,
            BlockStateId(TNT_STATE_ID),
            false,
            2,
        );
        assert_eq!(
            a2,
            DispenserAction::IgniteTnt {
                pos: BlockPos::new(1, 64, 0)
            }
        );
        assert_eq!(item_damage(&slots[0]), 2);

        // Solid stone -> EmptyClick, no durability lost
        let a3 = dispense_from_slots(false, pos, 5, &mut slots, BlockStateId::STONE, false, 3);
        assert_eq!(a3, DispenserAction::EmptyClick { pos });
        assert_eq!(item_damage(&slots[0]), 2);

        // At 63/64 damage -> last use places fire and breaks the flint and steel
        set_item_damage(&mut slots[0], 63);
        let a4 = dispense_from_slots(false, pos, 5, &mut slots, BlockStateId::AIR, false, 4);
        assert_eq!(
            a4,
            DispenserAction::PlaceBlock {
                pos: BlockPos::new(1, 64, 0),
                state: BlockStateId(FIRE_STATE_ID),
            }
        );
        assert!(slots[0].is_empty());
    }

    #[test]
    fn dispenser_spawn_egg_strips_suffix_and_spawns_mob() {
        let pos = BlockPos::new(8, 64, 8);
        let mut slots = empty_dispenser_slots();
        slots[0] = ItemStack::from_name("minecraft:zombie_spawn_egg", 2).unwrap();

        let action = dispense_from_slots(false, pos, 3, &mut slots, BlockStateId::AIR, false, 77);
        assert_eq!(
            action,
            DispenserAction::SpawnMobEgg {
                entity_type: "minecraft:zombie".to_string(),
                pos: Vec3::new(8.5, 64.0, 9.5),
            }
        );
        assert_eq!(slots[0].count, 1);
    }

    #[test]
    fn dropper_transfers_into_container_or_drops_item_without_firing() {
        let pos = BlockPos::new(1, 64, 1);
        let arrow_id = item_by_name("minecraft:arrow").unwrap().id;

        // With container in front: transfers arrow directly instead of shooting it
        let mut slots = empty_dispenser_slots();
        slots[0] = ItemStack::from_name("minecraft:arrow", 3).unwrap();
        let transfer =
            dispense_from_slots(true, pos, 5, &mut slots, BlockStateId::STONE, true, 100);
        assert_eq!(
            transfer,
            DispenserAction::TransferToContainer {
                target_pos: BlockPos::new(2, 64, 1),
                stack: ItemStack::new(arrow_id, 1),
            }
        );
        assert_eq!(slots[0].count, 2);

        // Without container in front: drops arrow as item entity instead of shooting it
        let dropped =
            dispense_from_slots(true, pos, 5, &mut slots, BlockStateId::AIR, false, 101);
        match dropped {
            DispenserAction::DropItem {
                pos: drop_pos,
                vel,
                stack,
            } => {
                assert_eq!(stack, ItemStack::new(arrow_id, 1));
                assert!(drop_pos.x > 2.0);
                assert!(vel.x > 0.0);
            }
            other => panic!("expected DropItem from dropper, got {other:?}"),
        }
        assert_eq!(slots[0].count, 1);
    }

    #[test]
    fn bed_explodes_in_nether_and_end() {
        let mut mgr = SleepManager::new();
        let bed = BlockPos::new(0, 64, 0);
        let res = mgr.try_sleep(
            1,
            Vec3::new(0.5, 64.0, 1.0),
            bed,
            false,
            18_000,
            false,
            false,
            1,
            100,
        );
        assert_eq!(res, BedInteractResult::ExplodeInDimension { pos: bed });
        assert_eq!(mgr.sleeping_count(), 0);
    }

    #[test]
    fn bed_checks_reach_and_daytime_spawnpoint() {
        let mut mgr = SleepManager::new();
        let bed = BlockPos::new(10, 64, 10);

        // Too far horizontally (> 3.0 from 10.5)
        let far = mgr.try_sleep(
            1,
            Vec3::new(14.0, 64.0, 10.5),
            bed,
            true,
            18_000,
            false,
            false,
            1,
            100,
        );
        assert_eq!(far, BedInteractResult::TooFar);

        // Too far vertically (> 2.0 from 64.0)
        let high = mgr.try_sleep(
            1,
            Vec3::new(10.5, 66.5, 10.5),
            bed,
            true,
            18_000,
            false,
            false,
            1,
            100,
        );
        assert_eq!(high, BedInteractResult::TooFar);

        // Daytime in Overworld (clear weather): sets spawn point, cannot sleep
        let day = mgr.try_sleep(
            1,
            Vec3::new(10.5, 64.0, 11.5),
            bed,
            true,
            6_000,
            false,
            false,
            1,
            100,
        );
        assert_eq!(day, BedInteractResult::SetSpawnAndNotNight { spawn_pos: bed });
        assert!(!mgr.is_sleeping(1));

        // Daytime thunderstorm: sleeping IS allowed!
        let storm = mgr.try_sleep(
            1,
            Vec3::new(10.5, 64.0, 11.5),
            bed,
            true,
            6_000,
            true,
            true,
            1,
            100,
        );
        assert_eq!(
            storm,
            BedInteractResult::StartedSleeping {
                bed_pos: bed,
                sleeping_count: 1,
                required_count: 1,
            }
        );
        assert!(mgr.is_sleeping(1));
    }

    #[test]
    fn night_and_weather_time_boundaries_match_vanilla() {
        // Clear weather: 12542..=23459
        assert!(!SleepManager::is_night_or_thunder(12_541, false, false));
        assert!(SleepManager::is_night_or_thunder(12_542, false, false));
        assert!(SleepManager::is_night_or_thunder(23_459, false, false));
        assert!(!SleepManager::is_night_or_thunder(23_460, false, false));

        // Raining: 12010..=23991
        assert!(!SleepManager::is_night_or_thunder(12_009, true, false));
        assert!(SleepManager::is_night_or_thunder(12_010, true, false));
        assert!(SleepManager::is_night_or_thunder(23_991, true, false));
        assert!(!SleepManager::is_night_or_thunder(23_992, true, false));

        // Multi-day modulo wrapping
        assert!(SleepManager::is_night_or_thunder(24_000 * 3 + 18_000, false, false));
        assert!(!SleepManager::is_night_or_thunder(24_000 * 3 + 6_000, false, false));

        // Thundering anytime
        assert!(SleepManager::is_night_or_thunder(1_000, true, true));
    }

    #[test]
    fn single_player_100_tick_night_skip_and_weather_clear() {
        let mut mgr = SleepManager::new();
        let bed = BlockPos::new(0, 64, 0);
        let day_time = 24_000 + 13_000; // Day 1, 13000 ticks

        let start = mgr.try_sleep(
            7,
            Vec3::new(0.5, 64.0, 0.5),
            bed,
            true,
            day_time,
            true,
            false,
            1,
            100,
        );
        assert_eq!(
            start,
            BedInteractResult::StartedSleeping {
                bed_pos: bed,
                sleeping_count: 1,
                required_count: 1,
            }
        );

        // Ticks 1..=99 do not skip yet
        for t in 1..VANILLA_SLEEP_TICKS {
            let r = mgr.tick(1, 100, day_time + t as i64, true, false);
            assert!(!r.skip_night, "should not skip on tick {t}");
            assert!(r.woke_players.is_empty());
            assert_eq!(mgr.sleep_ticks(7), Some(t));
        }

        // Tick 100 triggers night skip -> next morning (48000), clears rain, wakes player 7
        let final_time = day_time + VANILLA_SLEEP_TICKS as i64;
        let skip = mgr.tick(1, 100, final_time, true, false);
        assert!(skip.skip_night);
        assert_eq!(skip.new_day_time, Some(48_000));
        assert!(skip.clear_weather);
        assert_eq!(skip.woke_players, vec![7]);
        assert_eq!(mgr.sleeping_count(), 0);
        assert!(!mgr.is_sleeping(7));
    }

    #[test]
    fn players_sleeping_percentage_50_100_0_and_over_100() {
        assert_eq!(SleepManager::required_sleepers(4, 50), 2);
        assert_eq!(SleepManager::required_sleepers(3, 50), 2);
        assert_eq!(SleepManager::required_sleepers(2, 50), 1);
        assert_eq!(SleepManager::required_sleepers(4, 100), 4);
        assert_eq!(SleepManager::required_sleepers(4, 0), 1);
        assert_eq!(SleepManager::required_sleepers(4, -10), 1);
        assert_eq!(SleepManager::required_sleepers(4, 101), usize::MAX);

        // 50% with 4 players -> needs 2 sleepers
        let mut mgr = SleepManager::new();
        let bed1 = BlockPos::new(0, 64, 0);
        let bed2 = BlockPos::new(2, 64, 0);
        let night = 15_000;

        let r1 = mgr.try_sleep(1, Vec3::new(0.5, 64.0, 0.5), bed1, true, night, false, false, 4, 50);
        assert_eq!(
            r1,
            BedInteractResult::StartedSleeping {
                bed_pos: bed1,
                sleeping_count: 1,
                required_count: 2,
            }
        );

        // 100 ticks with only 1 of 2 required sleepers -> does NOT skip
        for _ in 0..100 {
            let tick_res = mgr.tick(4, 50, night, false, false);
            assert!(!tick_res.skip_night);
        }
        assert_eq!(mgr.sleep_ticks(1), Some(100));

        // 2nd player enters bed -> after 100 ticks, skips night and wakes both players!
        let r2 = mgr.try_sleep(2, Vec3::new(2.5, 64.0, 0.5), bed2, true, night, false, false, 4, 50);
        assert_eq!(
            r2,
            BedInteractResult::StartedSleeping {
                bed_pos: bed2,
                sleeping_count: 2,
                required_count: 2,
            }
        );
        for _ in 1..100 {
            assert!(!mgr.tick(4, 50, night, false, false).skip_night);
        }
        let skip = mgr.tick(4, 50, night, false, false);
        assert!(skip.skip_night);
        assert_eq!(skip.new_day_time, Some(24_000));
        assert!(!skip.clear_weather);
        assert_eq!(skip.woke_players, vec![1, 2]);

        // >100% disables night skipping completely, until dawn naturally wakes everyone
        let mut mgr2 = SleepManager::new();
        mgr2.try_sleep(10, Vec3::new(0.5, 64.0, 0.5), bed1, true, night, false, false, 1, 101);
        for _ in 0..150 {
            let r = mgr2.tick(1, 101, night, false, false);
            assert!(!r.skip_night);
            assert!(r.woke_players.is_empty());
        }
        // Morning arrives naturally (day_time = 23500, clear weather) -> wakes sleepers without skipping
        let dawn = mgr2.tick(1, 101, 23_500, false, false);
        assert!(!dawn.skip_night);
        assert_eq!(dawn.new_day_time, None);
        assert_eq!(dawn.woke_players, vec![10]);
        assert_eq!(mgr2.sleeping_count(), 0);
    }

    #[test]
    fn wake_player_and_fast_sleep_threshold() {
        let mut mgr = SleepManager::new();
        let bed = BlockPos::new(0, 64, 0);
        mgr.try_sleep(42, Vec3::new(0.5, 64.0, 0.5), bed, true, 14_000, false, false, 1, 100);
        assert!(mgr.wake_player(42));
        assert!(!mgr.wake_player(42));
        assert!(!mgr.is_sleeping(42));

        // Fast sleep threshold helper
        mgr.try_sleep(42, Vec3::new(0.5, 64.0, 0.5), bed, true, 14_000, false, false, 1, 100);
        for _ in 1..FAST_SLEEP_TICKS {
            assert!(
                !mgr.tick_with_threshold(1, 100, 14_000, false, false, FAST_SLEEP_TICKS)
                    .skip_night
            );
        }
        let skip = mgr.tick_with_threshold(1, 100, 14_000, false, false, FAST_SLEEP_TICKS);
        assert!(skip.skip_night);
        assert_eq!(skip.new_day_time, Some(24_000));
        assert_eq!(skip.woke_players, vec![42]);
    }
}
