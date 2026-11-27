//! Server-side orchestration for live Mob AI, breeding/taming, villager professions,
//! natural mob spawning, and Boat/Minecart vehicles (Minecraft 26.3 / Protocol 777).
//!
//! Uses a side-table [`LiveEntityManager`] keyed by `entity_id: i32` so [`WorldEntity`]
//! stays lean while supporting full mob AI state, vehicle occupancy, and villager tiers.

use std::collections::HashMap;

use mc_data::mob_spawns::mob_profile;
use mc_data::static_registry_protocol_id;
use mc_math::Vec3;
use mc_world::{
    CREATURE_CAP, EntityKind, MONSTER_CAP, MobAiAction, MobAiState, VehicleKind, VehicleState,
    VillagerProfession, WorldEntity, feed_mob_for_breeding_or_taming, select_natural_spawns,
    step_mob_ai, step_vehicle, try_breed_pair,
};
use uuid::Uuid;

const ALL_VILLAGER_PROFESSIONS: [VillagerProfession; 8] = [
    VillagerProfession::Farmer,
    VillagerProfession::Librarian,
    VillagerProfession::Armorer,
    VillagerProfession::Toolsmith,
    VillagerProfession::Weaponsmith,
    VillagerProfession::Cleric,
    VillagerProfession::Butcher,
    VillagerProfession::Mason,
];

/// Vanilla villager career XP thresholds for levels 1..=5 (Novice..=Master).
const VILLAGER_LEVEL_XP: [i32; 5] = [0, 10, 70, 150, 250];

const BREED_ATTRACT_RADIUS_SQ: f64 = 8.0 * 8.0;
const BREED_PAIR_RADIUS_SQ: f64 = 3.5 * 3.5;
const NATURAL_SPAWN_MIN_DIST: f64 = 24.0;
const NATURAL_SPAWN_MAX_DIST: f64 = 64.0;

#[inline]
fn short_id(name: &str) -> &str {
    name.strip_prefix("minecraft:").unwrap_or(name)
}

#[inline]
fn canonical_id(name: &str) -> String {
    if name.contains(':') {
        name.to_string()
    } else {
        format!("minecraft:{name}")
    }
}

#[inline]
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

#[inline]
fn deterministic_villager_profession(eid: i32, seed: u64) -> (VillagerProfession, u8) {
    let h = splitmix64(seed ^ ((eid as u32 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)));
    let idx = (h as usize) % ALL_VILLAGER_PROFESSIONS.len();
    (ALL_VILLAGER_PROFESSIONS[idx], idx as u8)
}

#[inline]
fn yaw_from_horizontal_delta(dx: f64, dz: f64) -> f32 {
    (-dx).atan2(dz).to_degrees() as f32
}

#[inline]
fn horiz_dist_sq(a: Vec3, b: Vec3) -> f64 {
    let dx = a.x - b.x;
    let dz = a.z - b.z;
    dx * dx + dz * dz
}

#[inline]
fn find_nearest_player(pos: Vec3, players: &[(i32, Vec3)]) -> Option<(i32, Vec3)> {
    players.iter().copied().min_by(|a, b| {
        let da = pos.distance_to_sqr(a.1);
        let db = pos.distance_to_sqr(b.1);
        da.total_cmp(&db)
    })
}

#[inline]
fn apply_mob_horizontal_velocity(entity: &mut WorldEntity, vx: f64, vz: f64) {
    entity.vel.x = vx;
    entity.vel.z = vz;
    if vx * vx + vz * vz > 1e-8 {
        entity.yaw = yaw_from_horizontal_delta(vx, vz);
    }
    if let EntityKind::Mob {
        ai_cooldown,
        wander_dir,
        ..
    } = &mut entity.kind
    {
        *wander_dir = (vx, vz);
        *ai_cooldown = (*ai_cooldown).max(10);
    }
}

/// Aggregated gameplay events emitted during a single server tick of [`LiveEntityManager::step_all_mobs`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MobStepOutput {
    /// `(target_player_eid, damage)`
    pub melee_attacks: Vec<(i32, f32)>,
    /// `(shooter_eid, origin, target)`
    pub arrows_to_spawn: Vec<(i32, Vec3, Vec3)>,
    /// `(creeper_eid, pos, power)`
    pub creeper_explosions: Vec<(i32, Vec3, f32)>,
    /// `(mob_eid, damage)`
    pub sun_burn_mobs: Vec<(i32, f32)>,
    /// `(entity_type, pos)`
    pub babies_to_spawn: Vec<(String, Vec3)>,
    /// `(mob_eid, new_pos)`
    pub teleported_mobs: Vec<(i32, Vec3)>,
}

/// Outcome of a player right-clicking a mob or rideable vehicle via [`LiveEntityManager::interact_entity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobInteractOutcome {
    None,
    ConsumedItem {
        hearts: bool,
        tamed: bool,
        toggled_sit: bool,
    },
    OpenVillagerTrade {
        villager_eid: i32,
        profession: VillagerProfession,
        level: i32,
        xp: i32,
    },
    MountedVehicle {
        vehicle_eid: i32,
        passengers: Vec<i32>,
    },
}

/// Side-table state manager for live mobs, villager careers, and rideable boats/minecarts,
/// keyed by `entity_id: i32`.
#[derive(Debug, Default)]
pub struct LiveEntityManager {
    pub ai: HashMap<i32, MobAiState>,
    pub vehicles: HashMap<i32, VehicleState>,
    pub villager_professions: HashMap<i32, (VillagerProfession, i32, i32)>,
    pub rider_to_vehicle: HashMap<i32, i32>,
}

impl LiveEntityManager {
    /// Create an empty [`LiveEntityManager`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Lazily create and return [`MobAiState`] for `eid`.
    ///
    /// When `mob_type` is `"minecraft:villager"` (or `"villager"`), also deterministically
    /// initializes `(VillagerProfession, level=1, xp=0)` in [`Self::villager_professions`].
    pub fn ensure_mob_ai(&mut self, eid: i32, mob_type: &str, seed: u64) -> &mut MobAiState {
        let mut prof_idx_opt = None;
        if short_id(mob_type) == "villager" {
            let (prof, idx) = deterministic_villager_profession(eid, seed);
            self.villager_professions.entry(eid).or_insert((prof, 1, 0));
            prof_idx_opt = Some(idx);
        }
        self.ai.entry(eid).or_insert_with(|| {
            let mut state = MobAiState::default();
            if let Some(idx) = prof_idx_opt {
                state.profession = idx;
            }
            state
        })
    }

    /// Remove all tracked AI, villager, and vehicle state for `eid`.
    ///
    /// - If `eid` was a vehicle with a mounted rider, detaches the rider and returns `Some(rider_eid)`.
    /// - If `eid` was a rider on a vehicle, detaches `eid` from its vehicle and returns `None`.
    pub fn remove_entity(&mut self, eid: i32) -> Option<i32> {
        self.ai.remove(&eid);
        self.villager_professions.remove(&eid);

        let mut dismounted_rider = None;
        if let Some(vehicle) = self.vehicles.remove(&eid) {
            if let Some(rider_eid) = vehicle.passenger_eid {
                if self.rider_to_vehicle.get(&rider_eid) == Some(&eid) {
                    self.rider_to_vehicle.remove(&rider_eid);
                }
                dismounted_rider = Some(rider_eid);
            }
            self.rider_to_vehicle.retain(|_, &mut vid| vid != eid);
        }

        if let Some(vehicle_eid) = self.rider_to_vehicle.remove(&eid) {
            if let Some(vehicle) = self.vehicles.get_mut(&vehicle_eid) {
                if vehicle.passenger_eid == Some(eid) {
                    vehicle.passenger_eid = None;
                }
            }
        }

        dismounted_rider
    }

    /// Returns `true` if `eid` is currently registered as a live Boat or Minecart vehicle.
    #[inline]
    pub fn is_vehicle(&self, eid: i32) -> bool {
        self.vehicles.contains_key(&eid)
    }

    /// O(1) lookup of the vehicle `entity_id` currently ridden by `rider_eid`.
    #[inline]
    pub fn vehicle_of_rider(&self, rider_eid: i32) -> Option<i32> {
        self.rider_to_vehicle.get(&rider_eid).copied()
    }

    /// Read `(profession, level, xp)` for a tracked villager entity, if initialized.
    #[inline]
    pub fn villager_info(&self, eid: i32) -> Option<(VillagerProfession, i32, i32)> {
        self.villager_professions.get(&eid).copied()
    }

    /// Award trading XP to a villager and advance its tier (`1..=5`) per vanilla XP thresholds.
    pub fn add_villager_xp(
        &mut self,
        eid: i32,
        xp_gain: i32,
    ) -> Option<(VillagerProfession, i32, i32)> {
        let entry = self.villager_professions.get_mut(&eid)?;
        if xp_gain > 0 {
            entry.2 = entry.2.saturating_add(xp_gain);
            let mut new_level = entry.1.clamp(1, 5);
            for (idx, &threshold) in VILLAGER_LEVEL_XP.iter().enumerate() {
                let tier = (idx + 1) as i32;
                if entry.2 >= threshold && tier > new_level {
                    new_level = tier;
                }
            }
            entry.1 = new_level;
        }
        Some(*entry)
    }

    /// Advance AI and pairwise breeding for every live non-vehicle mob in `entities` by 1 tick.
    pub fn step_all_mobs(
        &mut self,
        entities: &mut [WorldEntity],
        players: &[(i32, Vec3)],
        is_daytime: bool,
        sky_exposed_fn: impl Fn(Vec3) -> bool,
        tick_seed: u64,
    ) -> MobStepOutput {
        let mut out = MobStepOutput::default();
        let mut love_candidates: Vec<(usize, i32, String, Vec3)> = Vec::new();

        for (idx, entity) in entities.iter_mut().enumerate() {
            if self.vehicles.contains_key(&entity.entity_id) {
                continue;
            }
            let (mob_type, alive) = match &entity.kind {
                EntityKind::Mob {
                    type_name, health, ..
                } => (type_name.clone(), *health > 0.0),
                _ => continue,
            };
            if !alive {
                continue;
            }

            let eid = entity.entity_id;
            let mob_pos = entity.pos;
            let nearest_player = find_nearest_player(mob_pos, players);
            let sky_exposed = sky_exposed_fn(mob_pos);
            // Offset by multiples of 20 so `mob_seed % 20 == tick_seed % 20` (sun-burn cadence)
            // while giving each entity a distinct Enderman teleport hash.
            let mob_seed = tick_seed.wrapping_add((eid as u32 as u64).wrapping_mul(20));

            let (desired_vel, action, in_love_adult) = {
                let ai = self.ensure_mob_ai(eid, &mob_type, tick_seed);
                let (vel, act) = step_mob_ai(
                    &mob_type,
                    mob_pos,
                    ai,
                    nearest_player,
                    is_daytime,
                    sky_exposed,
                    mob_seed,
                );
                let ready_to_love = !ai.is_baby && ai.love_ticks > 0 && ai.breed_cooldown == 0;
                (vel, act, ready_to_love)
            };

            if let Some((vx, vz)) = desired_vel {
                apply_mob_horizontal_velocity(entity, vx, vz);
                if vx == 0.0 && vz == 0.0 {
                    if let Some((_, ppos)) = nearest_player {
                        let dx = ppos.x - mob_pos.x;
                        let dz = ppos.z - mob_pos.z;
                        if dx * dx + dz * dz <= 16.0 * 16.0 && dx * dx + dz * dz > 1e-8 {
                            entity.yaw = yaw_from_horizontal_delta(dx, dz);
                        }
                    }
                }
            }

            match action {
                MobAiAction::None => {}
                MobAiAction::MeleeAttack { target_eid, damage } => {
                    out.melee_attacks.push((target_eid, damage));
                }
                MobAiAction::ShootArrow { origin, target } => {
                    out.arrows_to_spawn.push((eid, origin, target));
                }
                MobAiAction::CreeperExplode { pos, power } => {
                    out.creeper_explosions.push((eid, pos, power));
                }
                MobAiAction::EndermanTeleport { new_pos } => {
                    entity.pos = new_pos;
                    out.teleported_mobs.push((eid, new_pos));
                }
                MobAiAction::BurnInSun { damage } => {
                    out.sun_burn_mobs.push((eid, damage));
                }
                MobAiAction::SpawnBaby { entity_type, pos } => {
                    out.babies_to_spawn.push((entity_type, pos));
                }
            }

            if in_love_adult {
                love_candidates.push((idx, eid, canonical_id(&mob_type), entity.pos));
            }
        }

        // Pairwise breeding for adult mobs currently in love within 8 blocks of a same-species partner.
        for i in 0..love_candidates.len() {
            let (idx_a, eid_a, ref type_a, pos_a) = love_candidates[i];
            let still_ready_a = self
                .ai
                .get(&eid_a)
                .is_some_and(|a| !a.is_baby && a.love_ticks > 0 && a.breed_cooldown == 0);
            if !still_ready_a {
                continue;
            }

            let mut bred = false;
            let mut closest_attract: Option<(usize, Vec3, f64)> = None;

            for &(idx_b, eid_b, ref type_b, pos_b) in &love_candidates[(i + 1)..] {
                if type_a != type_b {
                    continue;
                }
                let still_ready_b = self
                    .ai
                    .get(&eid_b)
                    .is_some_and(|b| !b.is_baby && b.love_ticks > 0 && b.breed_cooldown == 0);
                if !still_ready_b {
                    continue;
                }

                let dist_sq = pos_a.distance_to_sqr(pos_b);
                if dist_sq > BREED_ATTRACT_RADIUS_SQ {
                    continue;
                }

                if dist_sq <= BREED_PAIR_RADIUS_SQ {
                    if let Some(mut ai_a) = self.ai.remove(&eid_a) {
                        let baby_action = self.ai.get_mut(&eid_b).and_then(|ai_b| {
                            try_breed_pair(type_a, pos_a, &mut ai_a, pos_b, ai_b)
                        });
                        self.ai.insert(eid_a, ai_a);
                        if let Some(MobAiAction::SpawnBaby { entity_type, pos }) = baby_action {
                            out.babies_to_spawn.push((entity_type, pos));
                            bred = true;
                            break;
                        }
                    }
                } else if closest_attract.is_none_or(|(_, _, best_d2)| dist_sq < best_d2) {
                    closest_attract = Some((idx_b, pos_b, dist_sq));
                }
            }

            // If a same-species love partner is within 3.5..=8.0 blocks, steer both mobs together.
            if !bred {
                if let Some((idx_b, pos_b, _)) = closest_attract {
                    let speed = (mob_profile(type_a).move_speed as f64) * 0.65;
                    let dx = pos_b.x - pos_a.x;
                    let dz = pos_b.z - pos_a.z;
                    let len = (dx * dx + dz * dz).sqrt();
                    if len > 1e-6 {
                        let vx = (dx / len) * speed;
                        let vz = (dz / len) * speed;
                        apply_mob_horizontal_velocity(&mut entities[idx_a], vx, vz);
                        apply_mob_horizontal_velocity(&mut entities[idx_b], -vx, -vz);
                    }
                }
            }
        }

        out
    }

    /// Handle a player right-clicking `entity` (mounting vehicles, villager trading, feeding/breeding/taming,
    /// or toggling sit on an owned tamed pet when empty-handed).
    pub fn interact_entity(
        &mut self,
        entity: &WorldEntity,
        player_eid: i32,
        player_uuid: Uuid,
        held_item_name: Option<&str>,
        seed: u64,
    ) -> MobInteractOutcome {
        // 1. Rideable vehicle mounting
        if self.vehicles.contains_key(&entity.entity_id) {
            let seat_available = self
                .vehicles
                .get(&entity.entity_id)
                .is_some_and(|v| v.passenger_eid.is_none() || v.passenger_eid == Some(player_eid));
            if !seat_available {
                return MobInteractOutcome::None;
            }

            // Detach player from any previous vehicle first
            if let Some(old_vehicle_eid) = self.rider_to_vehicle.get(&player_eid).copied() {
                if old_vehicle_eid != entity.entity_id {
                    if let Some(old_v) = self.vehicles.get_mut(&old_vehicle_eid) {
                        if old_v.passenger_eid == Some(player_eid) {
                            old_v.passenger_eid = None;
                        }
                    }
                }
            }

            if let Some(vehicle) = self.vehicles.get_mut(&entity.entity_id) {
                vehicle.passenger_eid = Some(player_eid);
                self.rider_to_vehicle.insert(player_eid, entity.entity_id);
                return MobInteractOutcome::MountedVehicle {
                    vehicle_eid: entity.entity_id,
                    passengers: vec![player_eid],
                };
            }
            return MobInteractOutcome::None;
        }

        // 2. Mob interactions
        let EntityKind::Mob {
            ref type_name,
            health,
            ..
        } = entity.kind
        else {
            return MobInteractOutcome::None;
        };
        if health <= 0.0 {
            return MobInteractOutcome::None;
        }

        // Villager trading opens MerchantMenu with its deterministic profession/level/xp
        if short_id(type_name) == "villager" {
            self.ensure_mob_ai(entity.entity_id, type_name, seed);
            let (profession, level, xp) = self
                .villager_professions
                .get(&entity.entity_id)
                .copied()
                .unwrap_or((VillagerProfession::Farmer, 1, 0));
            return MobInteractOutcome::OpenVillagerTrade {
                villager_eid: entity.entity_id,
                profession,
                level,
                xp,
            };
        }

        let ai = self.ensure_mob_ai(entity.entity_id, type_name, seed);
        let held_non_empty = held_item_name.filter(|s| !s.is_empty() && short_id(s) != "air");

        if let Some(item_name) = held_non_empty {
            match feed_mob_for_breeding_or_taming(type_name, item_name, player_uuid, ai) {
                Some("tamed") => MobInteractOutcome::ConsumedItem {
                    hearts: true,
                    tamed: true,
                    toggled_sit: false,
                },
                Some("love") => MobInteractOutcome::ConsumedItem {
                    hearts: true,
                    tamed: false,
                    toggled_sit: false,
                },
                _ => MobInteractOutcome::None,
            }
        } else if ai.tamed_owner == Some(player_uuid)
            && matches!(short_id(type_name), "wolf" | "cat" | "parrot")
        {
            ai.sitting = !ai.sitting;
            MobInteractOutcome::ConsumedItem {
                hearts: false,
                tamed: false,
                toggled_sit: true,
            }
        } else {
            MobInteractOutcome::None
        }
    }

    /// Spawn a Boat or Minecart [`WorldEntity`] (resolving its Protocol 777 `minecraft:entity_type` ID)
    /// and register its [`VehicleState`] in [`Self::vehicles`].
    pub fn spawn_vehicle(
        &mut self,
        entity_id: i32,
        kind: VehicleKind,
        entity_type_name: &str,
        pos: Vec3,
        yaw: f32,
    ) -> Option<WorldEntity> {
        let canonical = canonical_id(entity_type_name);
        let entity_type_id = static_registry_protocol_id("minecraft:entity_type", &canonical)?;
        let (half_width, height) = match kind {
            VehicleKind::Boat => (0.6875, 0.5625),
            VehicleKind::Minecart => (0.49, 0.7),
        };

        let entity = WorldEntity {
            entity_id,
            uuid: Uuid::new_v4(),
            entity_type_id,
            pos,
            vel: Vec3::ZERO,
            yaw,
            pitch: 0.0,
            on_ground: false,
            half_width,
            height,
            kind: EntityKind::Mob {
                type_name: canonical,
                health: 6.0,
                max_health: 6.0,
                ai_cooldown: u16::MAX,
                hurt_ticks: 0,
                wander_dir: (0.0, 0.0),
            },
        };

        self.vehicles.insert(
            entity_id,
            VehicleState {
                kind,
                pos,
                vel: Vec3::ZERO,
                yaw,
                passenger_eid: None,
            },
        );

        Some(entity)
    }

    /// Apply a clientbound-originated `ServerboundMoveVehiclePacket` update from `rider_eid`
    /// onto its mounted vehicle [`WorldEntity`] and [`VehicleState`].
    ///
    /// Returns `Some(vehicle_eid)` when the vehicle was updated so the server can broadcast
    /// `ClientboundEntityPositionSyncPacket`.
    pub fn handle_move_vehicle(
        &mut self,
        rider_eid: i32,
        new_pos: Vec3,
        new_yaw: f32,
        new_pitch: f32,
        entities: &mut [WorldEntity],
    ) -> Option<i32> {
        let vehicle_eid = self.rider_to_vehicle.get(&rider_eid).copied()?;
        let ent = entities.iter_mut().find(|e| e.entity_id == vehicle_eid)?;

        let vel = Vec3::new(
            new_pos.x - ent.pos.x,
            new_pos.y - ent.pos.y,
            new_pos.z - ent.pos.z,
        );
        ent.vel = vel;
        ent.pos = new_pos;
        ent.yaw = new_yaw;
        ent.pitch = new_pitch;

        if let Some(v) = self.vehicles.get_mut(&vehicle_eid) {
            v.vel = vel;
            v.pos = new_pos;
            v.yaw = new_yaw;
        }

        Some(vehicle_eid)
    }

    /// Advance a server-simulated vehicle by 1 tick via [`mc_world::step_vehicle`] and mirror
    /// its updated transform back to the matching [`WorldEntity`]. Returns `true` if moved.
    pub fn step_vehicle_entity(
        &mut self,
        vehicle_eid: i32,
        forward: f32,
        steer: f32,
        on_water: bool,
        on_rail: bool,
        entities: &mut [WorldEntity],
    ) -> bool {
        let Some(v) = self.vehicles.get_mut(&vehicle_eid) else {
            return false;
        };
        let old_pos = v.pos;
        let old_yaw = v.yaw;
        step_vehicle(v, forward, steer, on_water, on_rail);
        let new_pos = v.pos;
        let new_vel = v.vel;
        let new_yaw = v.yaw;

        if let Some(ent) = entities.iter_mut().find(|e| e.entity_id == vehicle_eid) {
            ent.pos = new_pos;
            ent.vel = new_vel;
            ent.yaw = new_yaw;
        }

        (new_pos.x - old_pos.x).abs() > 1e-4
            || (new_pos.y - old_pos.y).abs() > 1e-4
            || (new_pos.z - old_pos.z).abs() > 1e-4
            || (new_yaw - old_yaw).abs() > 0.5
    }

    /// Detach `rider_eid` from its mounted vehicle, returning `(vehicle_eid, remaining_passengers)`
    /// for broadcasting `ClientboundSetPassengersPacket`.
    pub fn dismount_rider(&mut self, rider_eid: i32) -> Option<(i32, Vec<i32>)> {
        let vehicle_eid = self.rider_to_vehicle.remove(&rider_eid)?;
        let remaining = if let Some(vehicle) = self.vehicles.get_mut(&vehicle_eid) {
            if vehicle.passenger_eid == Some(rider_eid) {
                vehicle.passenger_eid = None;
            }
            vehicle.passenger_eid.into_iter().collect()
        } else {
            Vec::new()
        };
        Some((vehicle_eid, remaining))
    }
}

/// Map a Minecraft 26.3 boat, bamboo raft, or minecart item ID to its [`VehicleKind`] and
/// canonical `minecraft:entity_type` identifier.
pub fn vehicle_kind_from_item(item_name: &str) -> Option<(VehicleKind, &'static str)> {
    match short_id(item_name) {
        "oak_boat" => Some((VehicleKind::Boat, "minecraft:oak_boat")),
        "spruce_boat" => Some((VehicleKind::Boat, "minecraft:spruce_boat")),
        "birch_boat" => Some((VehicleKind::Boat, "minecraft:birch_boat")),
        "jungle_boat" => Some((VehicleKind::Boat, "minecraft:jungle_boat")),
        "acacia_boat" => Some((VehicleKind::Boat, "minecraft:acacia_boat")),
        "dark_oak_boat" => Some((VehicleKind::Boat, "minecraft:dark_oak_boat")),
        "mangrove_boat" => Some((VehicleKind::Boat, "minecraft:mangrove_boat")),
        "cherry_boat" => Some((VehicleKind::Boat, "minecraft:cherry_boat")),
        "pale_oak_boat" => Some((VehicleKind::Boat, "minecraft:pale_oak_boat")),
        "bamboo_raft" => Some((VehicleKind::Boat, "minecraft:bamboo_raft")),
        "minecart" => Some((VehicleKind::Minecart, "minecraft:minecart")),
        _ => None,
    }
}

/// Plan natural overworld mob spawns around online `players`.
///
/// - Counts live monsters and passive creatures using [`mob_profile`] (ignoring non-mob vehicles).
/// - Enforces vanilla caps ([`MONSTER_CAP`] = 70 at night, [`CREATURE_CAP`] = 10 during day).
/// - Skips all hostile monster spawns when `is_peaceful` is `true`.
/// - Selects biome-weighted packs via [`select_natural_spawns`] and resolves safe surface Y
///   coordinates via `surface_y_fn` at horizontal distance `24..=64` blocks from players.
pub fn plan_natural_spawns(
    players: &[Vec3],
    existing_entities: &[WorldEntity],
    is_night: bool,
    is_peaceful: bool,
    biome_fn: impl Fn(i32, i32, i32) -> &'static str,
    surface_y_fn: impl Fn(i32, i32) -> Option<i32>,
    tick_seed: u64,
) -> Vec<(String, Vec3)> {
    if players.is_empty() || (is_night && is_peaceful) {
        return Vec::new();
    }

    let mut current_monsters: usize = 0;
    let mut current_creatures: usize = 0;
    for ent in existing_entities {
        let EntityKind::Mob {
            ref type_name,
            health,
            ..
        } = ent.kind
        else {
            continue;
        };
        if health <= 0.0 {
            continue;
        }
        let profile = mob_profile(type_name);
        if profile.entity_type == "minecraft:unknown" {
            continue;
        }
        if profile.hostile {
            current_monsters += 1;
        } else {
            current_creatures += 1;
        }
    }

    if (is_night && current_monsters >= MONSTER_CAP)
        || (!is_night && current_creatures >= CREATURE_CAP)
    {
        return Vec::new();
    }

    let min_dist_sq = NATURAL_SPAWN_MIN_DIST * NATURAL_SPAWN_MIN_DIST;
    let max_dist_sq = NATURAL_SPAWN_MAX_DIST * NATURAL_SPAWN_MAX_DIST;
    let mut planned: Vec<(String, Vec3)> = Vec::new();

    for (p_idx, &player_pos) in players.iter().enumerate() {
        if (is_night && current_monsters >= MONSTER_CAP)
            || (!is_night && current_creatures >= CREATURE_CAP)
        {
            break;
        }

        let px = player_pos.x.floor() as i32;
        let py = player_pos.y.floor() as i32;
        let pz = player_pos.z.floor() as i32;
        let fallback_y = surface_y_fn(px, pz).unwrap_or(py);
        let biome = biome_fn(px, fallback_y, pz);
        let player_seed = splitmix64(tick_seed ^ ((p_idx as u64).wrapping_mul(0x517C_C1B7_2722_0A95)));

        let candidates = select_natural_spawns(
            biome,
            px,
            pz,
            fallback_y,
            is_night,
            current_monsters,
            current_creatures,
            player_seed,
        );

        for cand in candidates {
            let profile = mob_profile(cand.entity_type);
            if is_peaceful && profile.hostile {
                continue;
            }
            if is_night && current_monsters >= MONSTER_CAP {
                break;
            }
            if !is_night && current_creatures >= CREATURE_CAP {
                break;
            }

            // Ensure horizontal offset stays within the 24..=64 block spawn ring around player_pos
            let mut bx = cand.pos.x.floor() as i32;
            let mut bz = cand.pos.z.floor() as i32;
            let dx = (bx as f64 + 0.5) - player_pos.x;
            let dz = (bz as f64 + 0.5) - player_pos.z;
            let dist_sq = dx * dx + dz * dz;
            if dist_sq < min_dist_sq {
                let len = dist_sq.sqrt();
                let (ux, uz) = if len > 1e-6 {
                    (dx / len, dz / len)
                } else {
                    (1.0, 0.0)
                };
                bx = (player_pos.x + ux * (NATURAL_SPAWN_MIN_DIST + 1.0)).floor() as i32;
                bz = (player_pos.z + uz * (NATURAL_SPAWN_MIN_DIST + 1.0)).floor() as i32;
            } else if dist_sq > max_dist_sq {
                continue;
            }

            let Some(surface_y) = surface_y_fn(bx, bz) else {
                continue;
            };
            let spawn_pos = Vec3::new(bx as f64 + 0.5, surface_y as f64, bz as f64 + 0.5);

            // Never spawn inside any online player's 24-block no-spawn sphere
            if players.iter().any(|&other| horiz_dist_sq(spawn_pos, other) < min_dist_sq) {
                continue;
            }
            if horiz_dist_sq(spawn_pos, player_pos) > max_dist_sq {
                continue;
            }

            planned.push((cand.entity_type.to_string(), spawn_pos));
            if profile.hostile {
                current_monsters += 1;
            } else {
                current_creatures += 1;
            }
        }
    }

    planned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creeper_fuse_swells_and_explodes_via_step_all_mobs() {
        let mut mgr = LiveEntityManager::new();
        let creeper =
            WorldEntity::spawn_mob(100, "minecraft:creeper", Vec3::new(0.0, 64.0, 0.0)).unwrap();
        let mut entities = vec![creeper];
        let players = [(1, Vec3::new(2.0, 64.0, 0.0))];

        for tick in 1..30_u64 {
            let out = mgr.step_all_mobs(&mut entities, &players, false, |_| true, tick);
            assert!(out.creeper_explosions.is_empty());
            assert_eq!(entities[0].vel.x, 0.0);
            assert_eq!(entities[0].vel.z, 0.0);
            assert_eq!(mgr.ai[&100].creeper_fuse, tick as u8);
        }

        let out = mgr.step_all_mobs(&mut entities, &players, false, |_| true, 30);
        assert_eq!(
            out.creeper_explosions,
            vec![(100, Vec3::new(0.0, 64.0, 0.0), 3.0)]
        );
    }

    #[test]
    fn skeleton_chases_and_shoots_arrow_via_step_all_mobs() {
        let mut mgr = LiveEntityManager::new();
        let skel =
            WorldEntity::spawn_mob(200, "minecraft:skeleton", Vec3::new(0.0, 64.0, 0.0)).unwrap();
        let mut entities = vec![skel];

        // At 14 blocks (> 12, <= 16), skeleton moves toward +X and rotates yaw toward -90 deg
        let far_player = [(7, Vec3::new(14.0, 64.0, 0.0))];
        let out_far = mgr.step_all_mobs(&mut entities, &far_player, false, |_| true, 1);
        assert!(out_far.arrows_to_spawn.is_empty());
        assert!(entities[0].vel.x > 0.0);
        assert!((entities[0].yaw - (-90.0)).abs() < 1e-3);

        // Within 12 blocks, skeleton holds ground and fires arrows on ticks 0 and 40
        let close_player = [(7, Vec3::new(10.0, 64.0, 0.0))];
        let mut shot_ticks = Vec::new();
        for tick in 0..=40_u64 {
            let out = mgr.step_all_mobs(&mut entities, &close_player, false, |_| true, tick);
            if !out.arrows_to_spawn.is_empty() {
                assert_eq!(out.arrows_to_spawn.len(), 1);
                let (shooter, origin, target) = out.arrows_to_spawn[0];
                assert_eq!(shooter, 200);
                assert_eq!(origin, Vec3::new(0.0, 65.62, 0.0));
                assert_eq!(target, Vec3::new(10.0, 65.0, 0.0));
                shot_ticks.push(tick);
            }
        }
        assert_eq!(shot_ticks, vec![0, 40]);
    }

    #[test]
    fn feeding_two_cows_wheat_emits_baby_cow() {
        let mut mgr = LiveEntityManager::new();
        let player_uuid = Uuid::from_u128(0xABCD);
        let cow_a = WorldEntity::spawn_mob(301, "minecraft:cow", Vec3::new(0.0, 64.0, 0.0)).unwrap();
        let cow_b = WorldEntity::spawn_mob(302, "minecraft:cow", Vec3::new(2.0, 64.0, 0.0)).unwrap();
        let mut entities = vec![cow_a, cow_b];

        // Wrong food ignored
        assert_eq!(
            mgr.interact_entity(&entities[0], 1, player_uuid, Some("minecraft:carrot"), 1),
            MobInteractOutcome::None
        );

        // Feed wheat to both cows
        assert_eq!(
            mgr.interact_entity(&entities[0], 1, player_uuid, Some("minecraft:wheat"), 1),
            MobInteractOutcome::ConsumedItem {
                hearts: true,
                tamed: false,
                toggled_sit: false,
            }
        );
        assert_eq!(
            mgr.interact_entity(&entities[1], 1, player_uuid, Some("minecraft:wheat"), 1),
            MobInteractOutcome::ConsumedItem {
                hearts: true,
                tamed: false,
                toggled_sit: false,
            }
        );
        assert_eq!(mgr.ai[&301].love_ticks, 600);
        assert_eq!(mgr.ai[&302].love_ticks, 600);

        let out = mgr.step_all_mobs(&mut entities, &[], true, |_| true, 10);
        assert_eq!(
            out.babies_to_spawn,
            vec![("minecraft:cow".to_string(), Vec3::new(1.0, 64.0, 0.0))]
        );
        assert_eq!(mgr.ai[&301].love_ticks, 0);
        assert_eq!(mgr.ai[&302].love_ticks, 0);
        assert_eq!(mgr.ai[&301].breed_cooldown, 6000);
        assert_eq!(mgr.ai[&302].breed_cooldown, 6000);

        // Second pair at 6 blocks (within 8 blocks, > 3.5 blocks) steers partners toward each other
        let cow_c =
            WorldEntity::spawn_mob(303, "minecraft:cow", Vec3::new(20.0, 64.0, 0.0)).unwrap();
        let cow_d =
            WorldEntity::spawn_mob(304, "minecraft:cow", Vec3::new(26.0, 64.0, 0.0)).unwrap();
        let mut far_pair = vec![cow_c, cow_d];
        mgr.interact_entity(&far_pair[0], 1, player_uuid, Some("minecraft:wheat"), 2);
        mgr.interact_entity(&far_pair[1], 1, player_uuid, Some("minecraft:wheat"), 2);
        let attract_out = mgr.step_all_mobs(&mut far_pair, &[], true, |_| true, 11);
        assert!(attract_out.babies_to_spawn.is_empty());
        assert!(far_pair[0].vel.x > 0.0);
        assert!(far_pair[1].vel.x < 0.0);
    }

    #[test]
    fn right_clicking_villager_opens_deterministic_trade() {
        let mut mgr = LiveEntityManager::new();
        let player_uuid = Uuid::from_u128(0x1234);
        let villager =
            WorldEntity::spawn_mob(400, "minecraft:villager", Vec3::new(5.0, 64.0, 5.0)).unwrap();

        let first = mgr.interact_entity(&villager, 1, player_uuid, None, 999);
        let (prof, level, xp) = match first {
            MobInteractOutcome::OpenVillagerTrade {
                villager_eid,
                profession,
                level,
                xp,
            } => {
                assert_eq!(villager_eid, 400);
                assert_eq!(level, 1);
                assert_eq!(xp, 0);
                (profession, level, xp)
            }
            other => panic!("expected OpenVillagerTrade, got {other:?}"),
        };

        // Subsequent interaction preserves the same profession & updated XP/tier
        assert_eq!(mgr.add_villager_xp(400, 15), Some((prof, 2, 15)));
        let second = mgr.interact_entity(&villager, 1, player_uuid, Some("minecraft:emerald"), 123);
        assert_eq!(
            second,
            MobInteractOutcome::OpenVillagerTrade {
                villager_eid: 400,
                profession: prof,
                level: level + 1,
                xp: xp + 15,
            }
        );
    }

    #[test]
    fn boat_and_minecart_spawn_mount_move_and_dismount_lifecycle() {
        let mut mgr = LiveEntityManager::new();
        let player_uuid = Uuid::from_u128(0xBEEF);
        let other_uuid = Uuid::from_u128(0xCAFE);

        // Verify item-to-vehicle mapping for all boats/raft and minecart
        for boat_item in [
            "minecraft:oak_boat",
            "minecraft:spruce_boat",
            "minecraft:birch_boat",
            "minecraft:jungle_boat",
            "minecraft:acacia_boat",
            "minecraft:dark_oak_boat",
            "minecraft:mangrove_boat",
            "minecraft:cherry_boat",
            "minecraft:pale_oak_boat",
            "minecraft:bamboo_raft",
        ] {
            let (kind, etype) = vehicle_kind_from_item(boat_item).expect("recognized boat");
            assert_eq!(kind, VehicleKind::Boat);
            assert_eq!(etype, boat_item);
            assert!(static_registry_protocol_id("minecraft:entity_type", etype).is_some());
        }
        assert_eq!(
            vehicle_kind_from_item("minecraft:minecart"),
            Some((VehicleKind::Minecart, "minecraft:minecart"))
        );
        assert_eq!(vehicle_kind_from_item("minecraft:stick"), None);

        // Spawn oak_boat
        let (kind, etype) = vehicle_kind_from_item("minecraft:oak_boat").unwrap();
        let boat = mgr
            .spawn_vehicle(500, kind, etype, Vec3::new(0.0, 63.0, 0.0), 0.0)
            .expect("spawn oak_boat");
        assert!(mgr.is_vehicle(500));
        let mut entities = vec![boat];

        // Mount player 10 onto boat 500
        let mounted = mgr.interact_entity(&entities[0], 10, player_uuid, None, 1);
        assert_eq!(
            mounted,
            MobInteractOutcome::MountedVehicle {
                vehicle_eid: 500,
                passengers: vec![10],
            }
        );
        assert_eq!(mgr.vehicle_of_rider(10), Some(500));

        // Occupied seat rejects a second rider
        assert_eq!(
            mgr.interact_entity(&entities[0], 11, other_uuid, None, 1),
            MobInteractOutcome::None
        );

        // Handle MoveVehicle from rider 10
        let moved_pos = Vec3::new(4.5, 63.0, -2.25);
        assert_eq!(
            mgr.handle_move_vehicle(10, moved_pos, 45.0, 0.0, &mut entities),
            Some(500)
        );
        assert_eq!(entities[0].pos, moved_pos);
        assert_eq!(entities[0].yaw, 45.0);
        assert_eq!(mgr.vehicles[&500].pos, moved_pos);
        assert_eq!(mgr.vehicles[&500].yaw, 45.0);

        // Vehicles are skipped by mob AI
        let step_out = mgr.step_all_mobs(&mut entities, &[(10, moved_pos)], false, |_| true, 5);
        assert!(step_out.melee_attacks.is_empty());
        assert!(!mgr.ai.contains_key(&500));

        // Dismount rider 10
        assert_eq!(mgr.dismount_rider(10), Some((500, Vec::new())));
        assert_eq!(mgr.vehicle_of_rider(10), None);
        assert_eq!(mgr.vehicles[&500].passenger_eid, None);

        // Re-mount and remove_entity(vehicle) cleanly returns dismounted rider
        mgr.interact_entity(&entities[0], 10, player_uuid, None, 2);
        assert_eq!(mgr.remove_entity(500), Some(10));
        assert!(!mgr.is_vehicle(500));
        assert_eq!(mgr.vehicle_of_rider(10), None);
    }

    #[test]
    fn plan_natural_spawns_respects_peaceful_and_monster_cap() {
        let players = [Vec3::new(0.0, 64.0, 0.0)];

        // Nighttime normal difficulty spawns hostile plains monsters within 24..=64 blocks at Y=65
        let night = plan_natural_spawns(
            &players,
            &[],
            true,
            false,
            |_, _, _| "minecraft:plains",
            |_, _| Some(65),
            777,
        );
        assert!(!night.is_empty());
        for (etype, pos) in &night {
            assert!(
                mob_profile(etype).hostile,
                "expected hostile mob at night, got {etype}"
            );
            assert_eq!(pos.y, 65.0);
            let d = horiz_dist_sq(*pos, players[0]).sqrt();
            assert!(
                (24.0..=64.0).contains(&d),
                "spawn distance {d} out of 24..=64 range"
            );
        }

        // Peaceful difficulty completely blocks nighttime monster spawns
        let peaceful_night = plan_natural_spawns(
            &players,
            &[],
            true,
            true,
            |_, _, _| "minecraft:plains",
            |_, _| Some(65),
            777,
        );
        assert!(peaceful_night.is_empty());

        // Peaceful difficulty still allows daytime passive creatures
        let peaceful_day = plan_natural_spawns(
            &players,
            &[],
            false,
            true,
            |_, _, _| "minecraft:plains",
            |_, _| Some(65),
            888,
        );
        assert!(!peaceful_day.is_empty());
        for (etype, _) in &peaceful_day {
            assert!(!mob_profile(etype).hostile);
        }

        // MONSTER_CAP (70) prevents any additional nighttime spawns
        let full_monsters: Vec<WorldEntity> = (0..MONSTER_CAP)
            .map(|i| {
                WorldEntity::spawn_mob(1000 + i as i32, "minecraft:zombie", Vec3::new(30.0, 64.0, 0.0))
                    .unwrap()
            })
            .collect();
        let capped_night = plan_natural_spawns(
            &players,
            &full_monsters,
            true,
            false,
            |_, _, _| "minecraft:plains",
            |_, _| Some(65),
            777,
        );
        assert!(capped_night.is_empty());

        // CREATURE_CAP (10) prevents additional daytime passive spawns, while boats do not count
        let mut mgr = LiveEntityManager::new();
        let boats: Vec<WorldEntity> = (0..15)
            .map(|i| {
                mgr.spawn_vehicle(
                    2000 + i,
                    VehicleKind::Boat,
                    "minecraft:oak_boat",
                    Vec3::new(10.0, 63.0, 0.0),
                    0.0,
                )
                .unwrap()
            })
            .collect();
        let day_with_boats = plan_natural_spawns(
            &players,
            &boats,
            false,
            false,
            |_, _, _| "minecraft:plains",
            |_, _| Some(65),
            888,
        );
        assert!(!day_with_boats.is_empty());

        let full_creatures: Vec<WorldEntity> = (0..CREATURE_CAP)
            .map(|i| {
                WorldEntity::spawn_mob(3000 + i as i32, "minecraft:cow", Vec3::new(30.0, 64.0, 0.0))
                    .unwrap()
            })
            .collect();
        let capped_day = plan_natural_spawns(
            &players,
            &full_creatures,
            false,
            false,
            |_, _, _| "minecraft:plains",
            |_, _| Some(65),
            888,
        );
        assert!(capped_day.is_empty());
    }

    #[test]
    fn tamed_wolf_sit_toggle_and_zombie_sun_burn_and_enderman_teleport() {
        let mut mgr = LiveEntityManager::new();
        let owner = Uuid::from_u128(0x1111);
        let stranger = Uuid::from_u128(0x2222);

        let wolf = WorldEntity::spawn_mob(601, "minecraft:wolf", Vec3::new(0.0, 64.0, 0.0)).unwrap();
        let zombie =
            WorldEntity::spawn_mob(602, "minecraft:zombie", Vec3::new(50.0, 64.0, 0.0)).unwrap();
        let enderman =
            WorldEntity::spawn_mob(603, "minecraft:enderman", Vec3::new(-50.0, 64.0, 0.0)).unwrap();
        let mut entities = vec![wolf, zombie, enderman];

        // Tame wolf with bone
        assert_eq!(
            mgr.interact_entity(&entities[0], 1, owner, Some("minecraft:bone"), 1),
            MobInteractOutcome::ConsumedItem {
                hearts: true,
                tamed: true,
                toggled_sit: false,
            }
        );
        // Stranger cannot toggle sit
        assert_eq!(
            mgr.interact_entity(&entities[0], 2, stranger, None, 2),
            MobInteractOutcome::None
        );
        assert!(!mgr.ai[&601].sitting);
        // Owner toggles sit empty-handed
        assert_eq!(
            mgr.interact_entity(&entities[0], 1, owner, None, 2),
            MobInteractOutcome::ConsumedItem {
                hearts: false,
                tamed: false,
                toggled_sit: true,
            }
        );
        assert!(mgr.ai[&601].sitting);

        // Anger the enderman so it teleports on tick 20
        mgr.ensure_mob_ai(603, "minecraft:enderman", 1).anger_ticks = 200;

        let out = mgr.step_all_mobs(
            &mut entities,
            &[(1, Vec3::new(10.0, 64.0, 0.0))],
            true,
            |_| true,
            20,
        );
        // Sitting wolf stays still
        assert_eq!(entities[0].vel.x, 0.0);
        assert_eq!(entities[0].vel.z, 0.0);
        // Zombie burns in daytime sun on tick 20
        assert_eq!(out.sun_burn_mobs, vec![(602, 2.0)]);
        // Enderman teleported and updated WorldEntity::pos
        assert_eq!(out.teleported_mobs.len(), 1);
        assert_eq!(out.teleported_mobs[0].0, 603);
        assert_eq!(entities[2].pos, out.teleported_mobs[0].1);
    }
}
