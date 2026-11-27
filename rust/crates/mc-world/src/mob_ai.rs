//! Deterministic server-side Mob AI goals, breeding/taming, natural mob spawning,
//! and Boat/Minecart vehicle stepping for Minecraft 26.3.

use mc_data::mob_spawns::{MobSpawnEntry, biome_spawn_table, mob_profile};
use mc_math::Vec3;
use uuid::Uuid;

/// Vanilla overworld mob cap for hostile monsters.
pub const MONSTER_CAP: usize = 70;
/// Vanilla overworld mob cap for passive creatures.
pub const CREATURE_CAP: usize = 10;

/// Per-mob AI, breeding, taming, and villager profession state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MobAiState {
    pub creeper_fuse: u8,
    pub shoot_cooldown: u16,
    pub love_ticks: u16,
    pub breed_cooldown: u16,
    pub tamed_owner: Option<Uuid>,
    pub sitting: bool,
    pub anger_ticks: u16,
    pub is_baby: bool,
    pub profession: u8,
}

/// Discrete server-side gameplay action emitted by [`step_mob_ai`].
#[derive(Debug, Clone, PartialEq)]
pub enum MobAiAction {
    None,
    MeleeAttack { target_eid: i32, damage: f32 },
    ShootArrow { origin: Vec3, target: Vec3 },
    CreeperExplode { pos: Vec3, power: f32 },
    EndermanTeleport { new_pos: Vec3 },
    BurnInSun { damage: f32 },
    SpawnBaby { entity_type: String, pos: Vec3 },
}

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
fn horiz_move_toward(from: Vec3, to: Vec3, base_speed: f32) -> (f64, f64) {
    let dx = to.x - from.x;
    let dz = to.z - from.z;
    let len = (dx * dx + dz * dz).sqrt();
    if len < 1e-6 {
        return (0.0, 0.0);
    }
    let step_speed = (base_speed as f64) * 0.65;
    ((dx / len) * step_speed, (dz / len) * step_speed)
}

/// Advance a single mob's AI by 1 server tick (1/20s).
///
/// Returns `(desired_horizontal_velocity, action)`:
/// - `desired_horizontal_velocity`: `Some((vx, vz))` overrides the mob's wander direction
///   (e.g. chasing a target or holding position); `None` lets default wandering continue.
/// - `action`: combat, ranged attack, creeper detonation, teleport, or sun-burn event.
pub fn step_mob_ai(
    mob_type: &str,
    mob_pos: Vec3,
    ai: &mut MobAiState,
    nearest_player: Option<(i32, Vec3)>,
    is_daytime: bool,
    sky_exposed: bool,
    seed: u64,
) -> (Option<(f64, f64)>, MobAiAction) {
    // Tick passive timers
    ai.love_ticks = ai.love_ticks.saturating_sub(1);
    ai.breed_cooldown = ai.breed_cooldown.saturating_sub(1);

    let short = short_id(mob_type);
    let profile = mob_profile(mob_type);

    // Sitting tamed pets stay stationary
    if ai.tamed_owner.is_some() && ai.sitting {
        ai.anger_ticks = ai.anger_ticks.saturating_sub(1);
        return (Some((0.0, 0.0)), MobAiAction::None);
    }

    let mut move_vel: Option<(f64, f64)> = None;
    let mut action = MobAiAction::None;

    match short {
        "creeper" => {
            if let Some((_eid, ppos)) = nearest_player {
                let dist_sq = mob_pos.distance_to_sqr(ppos);
                if dist_sq <= 3.0 * 3.0 {
                    // Within detonation trigger radius: swell fuse up to 30 ticks
                    move_vel = Some((0.0, 0.0));
                    ai.creeper_fuse = ai.creeper_fuse.saturating_add(1).min(30);
                    if ai.creeper_fuse >= 30 {
                        action = MobAiAction::CreeperExplode {
                            pos: mob_pos,
                            power: 3.0,
                        };
                    }
                } else if dist_sq <= 7.0 * 7.0 {
                    if ai.creeper_fuse > 0 {
                        // Continue swelling while target remains within 7 blocks
                        move_vel = Some((0.0, 0.0));
                        ai.creeper_fuse = ai.creeper_fuse.saturating_add(1).min(30);
                        if ai.creeper_fuse >= 30 {
                            action = MobAiAction::CreeperExplode {
                                pos: mob_pos,
                                power: 3.0,
                            };
                        }
                    } else {
                        move_vel = Some(horiz_move_toward(mob_pos, ppos, profile.move_speed));
                    }
                } else {
                    // Target escaped past 7 blocks: cancel fuse
                    ai.creeper_fuse = 0;
                    if dist_sq <= 16.0 * 16.0 {
                        move_vel = Some(horiz_move_toward(mob_pos, ppos, profile.move_speed));
                    }
                }
            } else {
                ai.creeper_fuse = 0;
            }
        }
        "skeleton" | "stray" | "bogged" | "parched" => {
            if let Some((_eid, ppos)) = nearest_player {
                let dist_sq = mob_pos.distance_to_sqr(ppos);
                if dist_sq <= 12.0 * 12.0 {
                    move_vel = Some((0.0, 0.0));
                    if ai.shoot_cooldown > 0 {
                        ai.shoot_cooldown -= 1;
                    }
                    if ai.shoot_cooldown == 0 {
                        ai.shoot_cooldown = 40;
                        action = MobAiAction::ShootArrow {
                            origin: Vec3::new(mob_pos.x, mob_pos.y + 1.62, mob_pos.z),
                            target: Vec3::new(ppos.x, ppos.y + 1.0, ppos.z),
                        };
                    }
                } else {
                    ai.shoot_cooldown = ai.shoot_cooldown.saturating_sub(1);
                    if dist_sq <= 16.0 * 16.0 {
                        move_vel = Some(horiz_move_toward(mob_pos, ppos, profile.move_speed));
                    }
                }
            } else {
                ai.shoot_cooldown = ai.shoot_cooldown.saturating_sub(1);
            }
        }
        "enderman" => {
            if ai.anger_ticks > 0 {
                let mut did_melee = false;
                if let Some((eid, ppos)) = nearest_player {
                    let dist_sq = mob_pos.distance_to_sqr(ppos);
                    if dist_sq <= 1.9 * 1.9 {
                        move_vel = Some((0.0, 0.0));
                        if ai.shoot_cooldown > 0 {
                            ai.shoot_cooldown -= 1;
                        }
                        if ai.shoot_cooldown == 0 {
                            ai.shoot_cooldown = 20;
                            action = MobAiAction::MeleeAttack {
                                target_eid: eid,
                                damage: profile.attack_damage,
                            };
                            did_melee = true;
                        }
                    } else if dist_sq <= 16.0 * 16.0 {
                        move_vel = Some(horiz_move_toward(mob_pos, ppos, profile.move_speed));
                    }
                }
                if !did_melee {
                    if ai.shoot_cooldown > 0 {
                        ai.shoot_cooldown -= 1;
                    }
                    if ai.shoot_cooldown == 0 {
                        ai.shoot_cooldown = 40;
                        let h1 = splitmix64(seed ^ 0xE1D3_4A11_u64);
                        let h2 = splitmix64(h1);
                        let angle =
                            ((h1 & 0xFFFF) as f64 / 65536.0) * std::f64::consts::TAU;
                        let radius = 4.0 + ((h2 & 0xFFFF) as f64 / 65536.0) * 12.0;
                        let new_pos = Vec3::new(
                            mob_pos.x + angle.cos() * radius,
                            mob_pos.y,
                            mob_pos.z + angle.sin() * radius,
                        );
                        action = MobAiAction::EndermanTeleport { new_pos };
                    }
                }
            } else {
                ai.shoot_cooldown = ai.shoot_cooldown.saturating_sub(1);
            }
        }
        _ => {
            let is_aggressive =
                (profile.hostile && ai.tamed_owner.is_none()) || ai.anger_ticks > 0;
            if is_aggressive {
                if let Some((eid, ppos)) = nearest_player {
                    let dist_sq = mob_pos.distance_to_sqr(ppos);
                    if dist_sq <= 1.9 * 1.9 {
                        move_vel = Some((0.0, 0.0));
                        if ai.shoot_cooldown > 0 {
                            ai.shoot_cooldown -= 1;
                        }
                        if ai.shoot_cooldown == 0 {
                            ai.shoot_cooldown = 20;
                            action = MobAiAction::MeleeAttack {
                                target_eid: eid,
                                damage: profile.attack_damage,
                            };
                        }
                    } else {
                        ai.shoot_cooldown = ai.shoot_cooldown.saturating_sub(1);
                        if dist_sq <= 16.0 * 16.0 {
                            move_vel =
                                Some(horiz_move_toward(mob_pos, ppos, profile.move_speed));
                        }
                    }
                } else {
                    ai.shoot_cooldown = ai.shoot_cooldown.saturating_sub(1);
                }
            } else if ai.tamed_owner.is_some() {
                // Non-sitting tamed pet follows nearby player within 24 blocks
                if let Some((_eid, ppos)) = nearest_player {
                    let dist_sq = mob_pos.distance_to_sqr(ppos);
                    if dist_sq > 3.0 * 3.0 && dist_sq <= 24.0 * 24.0 {
                        move_vel = Some(horiz_move_toward(mob_pos, ppos, profile.move_speed));
                    } else {
                        move_vel = Some((0.0, 0.0));
                    }
                }
            }
        }
    }

    ai.anger_ticks = ai.anger_ticks.saturating_sub(1);

    // Undead & Phantom sun burning: 2.0 damage every 20 ticks when exposed to daytime sky
    if matches!(action, MobAiAction::None)
        && is_daytime
        && sky_exposed
        && profile.burns_in_sun
        && seed % 20 == 0
    {
        action = MobAiAction::BurnInSun { damage: 2.0 };
    }

    (move_vel, action)
}

/// Feed a mob with `item_name` to trigger taming (e.g. bone for wolf, cod/salmon for cat)
/// or breeding love mode (`love_ticks = 600` for wheat/carrots/seeds/species-specific foods).
///
/// Returns `Some("tamed")` on taming, `Some("love")` when entering breeding mode, or `None`
/// if the item was not applicable.
pub fn feed_mob_for_breeding_or_taming(
    mob_type: &str,
    item_name: &str,
    player_uuid: Uuid,
    ai: &mut MobAiState,
) -> Option<&'static str> {
    let mob = short_id(mob_type);
    let item = short_id(item_name);

    // 1. Taming checks (untamed tameable mobs)
    if ai.tamed_owner.is_none() {
        let can_tame = match mob {
            "wolf" => item == "bone",
            "cat" => matches!(item, "cod" | "salmon"),
            "parrot" => matches!(
                item,
                "wheat_seeds"
                    | "melon_seeds"
                    | "pumpkin_seeds"
                    | "beetroot_seeds"
                    | "torchflower_seeds"
                    | "pitcher_pod"
            ),
            _ => false,
        };
        if can_tame {
            ai.tamed_owner = Some(player_uuid);
            ai.anger_ticks = 0;
            return Some("tamed");
        }
    }

    // 2. Breeding checks (adult, not on cooldown, not already in love)
    if ai.is_baby || ai.breed_cooldown > 0 || ai.love_ticks > 0 {
        return None;
    }

    let can_breed = match mob {
        "cow" | "mooshroom" | "sheep" | "goat" => item == "wheat",
        "pig" => matches!(item, "carrot" | "potato" | "beetroot"),
        "chicken" => matches!(
            item,
            "wheat_seeds"
                | "melon_seeds"
                | "pumpkin_seeds"
                | "beetroot_seeds"
                | "torchflower_seeds"
                | "pitcher_pod"
        ),
        "rabbit" => matches!(item, "carrot" | "golden_carrot" | "dandelion"),
        "wolf" => {
            ai.tamed_owner.is_some()
                && matches!(
                    item,
                    "beef"
                        | "cooked_beef"
                        | "porkchop"
                        | "cooked_porkchop"
                        | "chicken"
                        | "cooked_chicken"
                        | "mutton"
                        | "cooked_mutton"
                        | "rabbit"
                        | "cooked_rabbit"
                        | "rotten_flesh"
                )
        }
        "cat" => ai.tamed_owner.is_some() && matches!(item, "cod" | "salmon"),
        "ocelot" => matches!(item, "cod" | "salmon"),
        "horse" | "donkey" => {
            matches!(item, "golden_carrot" | "golden_apple" | "enchanted_golden_apple")
        }
        "llama" | "trader_llama" => item == "hay_block",
        "fox" => matches!(item, "sweet_berries" | "glow_berries"),
        "bee" => matches!(
            item,
            "dandelion"
                | "poppy"
                | "blue_orchid"
                | "allium"
                | "azure_bluet"
                | "red_tulip"
                | "orange_tulip"
                | "white_tulip"
                | "pink_tulip"
                | "oxeye_daisy"
                | "cornflower"
                | "lily_of_the_valley"
                | "sunflower"
                | "lilac"
                | "rose_bush"
                | "peony"
                | "wither_rose"
                | "torchflower"
                | "pink_petals"
                | "wildflowers"
        ),
        "panda" => item == "bamboo",
        "turtle" => item == "seagrass",
        "frog" => item == "slime_ball",
        "axolotl" => item == "tropical_fish_bucket",
        "camel" => item == "cactus",
        "armadillo" => item == "spider_eye",
        "strider" => item == "warped_fungus",
        "hoglin" => item == "crimson_fungus",
        "sniffer" => item == "torchflower_seeds",
        "happy_ghast" => item == "snowball",
        _ => {
            let prof_item = short_id(mob_profile(mob_type).breed_item);
            !prof_item.is_empty() && prof_item == item
        }
    };

    if can_breed {
        ai.love_ticks = 600;
        Some("love")
    } else {
        None
    }
}

/// Pair two same-species adult mobs in love within 3.5 blocks, resetting both timers
/// (`breed_cooldown = 6000`) and returning [`MobAiAction::SpawnBaby`].
pub fn try_breed_pair(
    mob_type: &str,
    pos_a: Vec3,
    ai_a: &mut MobAiState,
    pos_b: Vec3,
    ai_b: &mut MobAiState,
) -> Option<MobAiAction> {
    if ai_a.is_baby
        || ai_b.is_baby
        || ai_a.love_ticks == 0
        || ai_b.love_ticks == 0
        || ai_a.breed_cooldown > 0
        || ai_b.breed_cooldown > 0
    {
        return None;
    }
    if pos_a.distance_to_sqr(pos_b) > 3.5 * 3.5 {
        return None;
    }
    ai_a.love_ticks = 0;
    ai_b.love_ticks = 0;
    ai_a.breed_cooldown = 6000;
    ai_b.breed_cooldown = 6000;
    let mid = Vec3::new(
        (pos_a.x + pos_b.x) * 0.5,
        pos_a.y.min(pos_b.y),
        (pos_a.z + pos_b.z) * 0.5,
    );
    Some(MobAiAction::SpawnBaby {
        entity_type: canonical_id(mob_type),
        pos: mid,
    })
}

/// Candidate entity produced by [`select_natural_spawns`].
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnCandidate {
    pub entity_type: &'static str,
    pub pos: Vec3,
}

/// Weighted random selection from a slice of [`MobSpawnEntry`].
pub fn pick_weighted_spawn(
    entries: &'static [MobSpawnEntry],
    roll: u64,
) -> Option<&'static MobSpawnEntry> {
    let total_weight: u64 = entries.iter().map(|e| e.weight as u64).sum();
    if total_weight == 0 {
        return None;
    }
    let mut target = roll % total_weight;
    for entry in entries {
        let w = entry.weight as u64;
        if target < w {
            return Some(entry);
        }
        target -= w;
    }
    entries.last()
}

/// Deterministically select a natural mob pack to spawn in `biome` near `(center_x, surface_y, center_z)`.
///
/// Respects vanilla mob caps (`MONSTER_CAP = 70` at night, `CREATURE_CAP = 10` during day),
/// performs weighted selection from `biome_spawn_table(biome)`, and places the pack outside
/// the 24-block player no-spawn sphere (24..=48 blocks from center).
pub fn select_natural_spawns(
    biome: &str,
    center_x: i32,
    center_z: i32,
    surface_y: i32,
    is_night: bool,
    current_monsters: usize,
    current_creatures: usize,
    seed: u64,
) -> Vec<SpawnCandidate> {
    let Some(table) = biome_spawn_table(biome) else {
        return Vec::new();
    };

    let (pool, remaining_cap) = if is_night {
        if current_monsters >= MONSTER_CAP {
            return Vec::new();
        }
        (table.monsters, MONSTER_CAP - current_monsters)
    } else {
        if current_creatures >= CREATURE_CAP {
            return Vec::new();
        }
        (table.creatures, CREATURE_CAP - current_creatures)
    };

    if pool.is_empty() || remaining_cap == 0 {
        return Vec::new();
    }

    let mut rng = splitmix64(
        seed ^ ((center_x as u64).wrapping_mul(0x517C_C1B7_2722_0A95))
            ^ ((center_z as u64).wrapping_mul(0x94D0_49BB_1331_11EB)),
    );
    let Some(entry) = pick_weighted_spawn(pool, rng) else {
        return Vec::new();
    };

    rng = splitmix64(rng);
    let min_c = entry.min_count.max(1) as usize;
    let max_c = (entry.max_count as usize).max(min_c);
    let span = (max_c - min_c + 1) as u64;
    let pack_size = (min_c + (rng % span) as usize).min(remaining_cap);

    rng = splitmix64(rng);
    let angle = ((rng & 0xFFFF) as f64 / 65536.0) * std::f64::consts::TAU;
    let dist = 24.0 + (((rng >> 16) & 0xFFFF) as f64 / 65536.0) * 24.0;
    let pack_x = center_x as f64 + angle.cos() * dist;
    let pack_z = center_z as f64 + angle.sin() * dist;

    let mut out = Vec::with_capacity(pack_size);
    for _ in 0..pack_size {
        rng = splitmix64(rng);
        let jx = ((rng & 0xFF) as f64 / 255.0) * 6.0 - 3.0;
        let jz = (((rng >> 8) & 0xFF) as f64 / 255.0) * 6.0 - 3.0;
        out.push(SpawnCandidate {
            entity_type: entry.entity_type,
            pos: Vec3::new((pack_x + jx).floor() + 0.5, surface_y as f64, (pack_z + jz).floor() + 0.5),
        });
    }
    out
}

/// Supported rideable vehicle categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VehicleKind {
    Boat,
    Minecart,
}

/// Simulation state for a Boat or Minecart entity.
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleState {
    pub kind: VehicleKind,
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    pub passenger_eid: Option<i32>,
}

/// Advance a Boat or Minecart by 1 server tick (1/20s).
///
/// - **Boat**: water buoyancy (zero sink on water), responsive yaw steering (`steer`),
///   water thrust with `0.9` drag, and heavy `0.45` drag + gravity on land.
/// - **Minecart**: smooth rail glide (`0.96` drag, zero gravity drop, `0.4` blocks/tick cap)
///   when `on_rail`, and heavy `0.5` ground friction + gravity when derailed.
pub fn step_vehicle(
    v: &mut VehicleState,
    forward: f32,
    steer: f32,
    on_water: bool,
    on_rail: bool,
) {
    let fwd = forward.clamp(-1.0, 1.0) as f64;
    let st = steer.clamp(-1.0, 1.0);

    match v.kind {
        VehicleKind::Boat => {
            v.yaw = (v.yaw + st * 4.5).rem_euclid(360.0);
            let yaw_rad = (v.yaw as f64).to_radians();
            let dir_x = -yaw_rad.sin();
            let dir_z = yaw_rad.cos();

            let (accel, drag, max_speed) = if on_water {
                let a = if fwd >= 0.0 { 0.04 * fwd } else { 0.02 * fwd };
                (a, 0.90_f64, 0.40_f64)
            } else {
                (0.015 * fwd, 0.45_f64, 0.15_f64)
            };

            v.vel.x = (v.vel.x + dir_x * accel) * drag;
            v.vel.z = (v.vel.z + dir_z * accel) * drag;

            let horiz_speed = (v.vel.x * v.vel.x + v.vel.z * v.vel.z).sqrt();
            if horiz_speed > max_speed {
                let scale = max_speed / horiz_speed;
                v.vel.x *= scale;
                v.vel.z *= scale;
            }

            if on_water {
                // Water buoyancy lifts submerged boat back to surface without sinking
                if v.vel.y < 0.0 {
                    v.vel.y = (v.vel.y + 0.06).min(0.0);
                } else {
                    v.vel.y *= 0.8;
                }
            } else {
                v.vel.y = (v.vel.y - 0.04).max(-0.8);
            }
        }
        VehicleKind::Minecart => {
            if on_rail {
                if st.abs() > 1e-4 {
                    v.yaw = (v.yaw + st * 2.0).rem_euclid(360.0);
                }
                let yaw_rad = (v.yaw as f64).to_radians();
                let dir_x = -yaw_rad.sin();
                let dir_z = yaw_rad.cos();

                v.vel.x = (v.vel.x + dir_x * (0.03 * fwd)) * 0.96;
                v.vel.z = (v.vel.z + dir_z * (0.03 * fwd)) * 0.96;
                v.vel.y = 0.0;

                let horiz_speed = (v.vel.x * v.vel.x + v.vel.z * v.vel.z).sqrt();
                const MAX_RAIL_SPEED: f64 = 0.40;
                if horiz_speed > MAX_RAIL_SPEED {
                    let scale = MAX_RAIL_SPEED / horiz_speed;
                    v.vel.x *= scale;
                    v.vel.z *= scale;
                }
            } else {
                let yaw_rad = (v.yaw as f64).to_radians();
                let dir_x = -yaw_rad.sin();
                let dir_z = yaw_rad.cos();

                v.vel.x = (v.vel.x + dir_x * (0.005 * fwd)) * 0.50;
                v.vel.z = (v.vel.z + dir_z * (0.005 * fwd)) * 0.50;
                v.vel.y = (v.vel.y - 0.04).max(-0.8);
            }
        }
    }

    if v.vel.x.abs() < 1e-5 {
        v.vel.x = 0.0;
    }
    if v.vel.y.abs() < 1e-5 {
        v.vel.y = 0.0;
    }
    if v.vel.z.abs() < 1e-5 {
        v.vel.z = 0.0;
    }

    v.pos.x += v.vel.x;
    v.pos.y += v.vel.y;
    v.pos.z += v.vel.z;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creeper_30_tick_fuse_explosion_and_escape_reset() {
        let mut ai = MobAiState::default();
        let creeper_pos = Vec3::new(0.0, 64.0, 0.0);
        let close_player = Some((1, Vec3::new(2.0, 64.0, 0.0)));

        // Ticks 1..=29 swell fuse without exploding yet
        for tick in 1..30_u64 {
            let (vel, action) = step_mob_ai(
                "minecraft:creeper",
                creeper_pos,
                &mut ai,
                close_player,
                false,
                true,
                tick,
            );
            assert_eq!(vel, Some((0.0, 0.0)));
            assert_eq!(action, MobAiAction::None);
            assert_eq!(ai.creeper_fuse, tick as u8);
        }

        // Tick 30 detonates with power 3.0
        let (_vel, action) = step_mob_ai(
            "minecraft:creeper",
            creeper_pos,
            &mut ai,
            close_player,
            false,
            true,
            30,
        );
        assert_eq!(
            action,
            MobAiAction::CreeperExplode {
                pos: creeper_pos,
                power: 3.0,
            }
        );

        // Escape past 7 blocks resets fuse cleanly
        let mut ai2 = MobAiState::default();
        for tick in 0..15_u64 {
            step_mob_ai("minecraft:creeper", creeper_pos, &mut ai2, close_player, false, true, tick);
        }
        assert_eq!(ai2.creeper_fuse, 15);
        let far_player = Some((1, Vec3::new(9.0, 64.0, 0.0)));
        let (chase_vel, action) =
            step_mob_ai("minecraft:creeper", creeper_pos, &mut ai2, far_player, false, true, 16);
        assert_eq!(ai2.creeper_fuse, 0);
        assert_eq!(action, MobAiAction::None);
        assert!(chase_vel.unwrap().0 > 0.0);
    }

    #[test]
    fn skeleton_approaches_and_shoots_every_40_ticks() {
        let mut ai = MobAiState::default();
        let skel_pos = Vec3::new(0.0, 64.0, 0.0);

        // At 14 blocks (> 12, <= 16), skeleton approaches without shooting
        let (vel, action) = step_mob_ai(
            "minecraft:skeleton",
            skel_pos,
            &mut ai,
            Some((1, Vec3::new(14.0, 64.0, 0.0))),
            false,
            true,
            1,
        );
        assert!(vel.unwrap().0 > 0.0);
        assert_eq!(action, MobAiAction::None);

        // Within 12 blocks: fires on tick 0 and tick 40 (exact 40-tick period)
        let target_player = Some((1, Vec3::new(10.0, 64.0, 0.0)));
        let mut shots = Vec::new();
        for tick in 0..81_u64 {
            let (_vel, act) = step_mob_ai(
                "minecraft:skeleton",
                skel_pos,
                &mut ai,
                target_player,
                false,
                true,
                tick,
            );
            if matches!(act, MobAiAction::ShootArrow { .. }) {
                shots.push(tick);
            }
        }
        assert_eq!(shots, vec![0, 40, 80]);
    }

    #[test]
    fn zombie_burns_in_daylight_sun_and_melee_attacks() {
        let mut ai = MobAiState::default();
        let z_pos = Vec3::new(0.0, 64.0, 0.0);

        // Daytime + sky exposed -> burns every 20 ticks
        let mut burn_ticks = Vec::new();
        for tick in 0..41_u64 {
            let (_vel, act) =
                step_mob_ai("minecraft:zombie", z_pos, &mut ai, None, true, true, tick);
            if act == (MobAiAction::BurnInSun { damage: 2.0 }) {
                burn_ticks.push(tick);
            }
        }
        assert_eq!(burn_ticks, vec![0, 20, 40]);

        // Under roof (sky_exposed = false) or night -> does not burn
        let (_vel, shade_act) =
            step_mob_ai("minecraft:zombie", z_pos, &mut ai, None, true, false, 0);
        assert_eq!(shade_act, MobAiAction::None);

        // Husk does not burn in sun
        let (_vel, husk_act) =
            step_mob_ai("minecraft:husk", z_pos, &mut ai, None, true, true, 0);
        assert_eq!(husk_act, MobAiAction::None);

        // Zombie chases at 8 blocks and melee-attacks within 1.9 blocks
        let mut combat_ai = MobAiState::default();
        let (vel, act) = step_mob_ai(
            "minecraft:zombie",
            z_pos,
            &mut combat_ai,
            Some((7, Vec3::new(8.0, 64.0, 0.0))),
            false,
            true,
            1,
        );
        assert!(vel.unwrap().0 > 0.0);
        assert_eq!(act, MobAiAction::None);

        let (_vel, hit) = step_mob_ai(
            "minecraft:zombie",
            z_pos,
            &mut combat_ai,
            Some((7, Vec3::new(1.5, 64.0, 0.0))),
            false,
            true,
            2,
        );
        assert_eq!(
            hit,
            MobAiAction::MeleeAttack {
                target_eid: 7,
                damage: 3.0,
            }
        );
    }

    #[test]
    fn enderman_teleports_within_16_blocks_when_angered() {
        let mut ai = MobAiState {
            anger_ticks: 200,
            ..Default::default()
        };
        let pos = Vec3::new(10.0, 64.0, -5.0);
        let (_vel, act) = step_mob_ai(
            "minecraft:enderman",
            pos,
            &mut ai,
            Some((1, Vec3::new(20.0, 64.0, -5.0))),
            false,
            true,
            12345,
        );
        match act {
            MobAiAction::EndermanTeleport { new_pos } => {
                let d2 = pos.distance_to_sqr(new_pos);
                assert!(d2 >= 16.0 && d2 <= 256.0 + 1e-6, "d2={d2}");
            }
            other => panic!("expected EndermanTeleport, got {other:?}"),
        }
    }

    #[test]
    fn cow_wheat_breeding_and_baby_spawn() {
        let player_id = Uuid::from_u128(42);
        let mut cow_a = MobAiState::default();
        let mut cow_b = MobAiState::default();

        // Wrong food ignored
        assert_eq!(
            feed_mob_for_breeding_or_taming("minecraft:cow", "minecraft:carrot", player_id, &mut cow_a),
            None
        );
        assert_eq!(cow_a.love_ticks, 0);

        // Wheat puts both cows in 600-tick love mode
        assert_eq!(
            feed_mob_for_breeding_or_taming("minecraft:cow", "minecraft:wheat", player_id, &mut cow_a),
            Some("love")
        );
        assert_eq!(cow_a.love_ticks, 600);
        assert_eq!(
            feed_mob_for_breeding_or_taming("minecraft:cow", "wheat", player_id, &mut cow_b),
            Some("love")
        );

        let baby = try_breed_pair(
            "minecraft:cow",
            Vec3::new(0.0, 64.0, 0.0),
            &mut cow_a,
            Vec3::new(2.0, 64.0, 0.0),
            &mut cow_b,
        );
        assert_eq!(
            baby,
            Some(MobAiAction::SpawnBaby {
                entity_type: "minecraft:cow".to_string(),
                pos: Vec3::new(1.0, 64.0, 0.0),
            })
        );
        assert_eq!(cow_a.love_ticks, 0);
        assert_eq!(cow_a.breed_cooldown, 6000);
    }

    #[test]
    fn wolf_bone_taming_and_cat_fish_taming() {
        let owner = Uuid::from_u128(999);
        let mut wolf = MobAiState::default();
        assert_eq!(
            feed_mob_for_breeding_or_taming("minecraft:wolf", "minecraft:bone", owner, &mut wolf),
            Some("tamed")
        );
        assert_eq!(wolf.tamed_owner, Some(owner));

        // Tamed wolf can then be bred with cooked beef
        assert_eq!(
            feed_mob_for_breeding_or_taming("minecraft:wolf", "minecraft:cooked_beef", owner, &mut wolf),
            Some("love")
        );
        assert_eq!(wolf.love_ticks, 600);

        // Cat tamed with salmon
        let mut cat = MobAiState::default();
        assert_eq!(
            feed_mob_for_breeding_or_taming("minecraft:cat", "minecraft:salmon", owner, &mut cat),
            Some("tamed")
        );
        assert_eq!(cat.tamed_owner, Some(owner));
    }

    #[test]
    fn biome_weighted_natural_spawning_and_caps() {
        let night_spawns = select_natural_spawns("minecraft:plains", 0, 0, 64, true, 0, 0, 777);
        assert!(!night_spawns.is_empty());
        let plains = biome_spawn_table("minecraft:plains").unwrap();
        for cand in &night_spawns {
            assert!(
                plains.monsters.iter().any(|e| e.entity_type == cand.entity_type),
                "unexpected night mob {}",
                cand.entity_type
            );
            let d2 = cand.pos.x * cand.pos.x + cand.pos.z * cand.pos.z;
            assert!(d2 >= 18.0 * 18.0, "spawned too close to player: d2={d2}");
        }

        // Monster cap (70) blocks spawning at night
        assert!(select_natural_spawns("minecraft:plains", 0, 0, 64, true, 70, 0, 777).is_empty());

        // Daytime creatures spawn in plains and respect cap (10)
        let day_spawns = select_natural_spawns("plains", 0, 0, 64, false, 0, 0, 888);
        assert!(!day_spawns.is_empty());
        for cand in &day_spawns {
            assert!(
                plains.creatures.iter().any(|e| e.entity_type == cand.entity_type),
                "unexpected day mob {}",
                cand.entity_type
            );
        }
        assert!(select_natural_spawns("plains", 0, 0, 64, false, 0, 10, 888).is_empty());

        // Mushroom fields spawns no monsters at night
        assert!(
            select_natural_spawns("minecraft:mushroom_fields", 0, 0, 64, true, 0, 0, 555)
                .is_empty()
        );
    }

    #[test]
    fn boat_and_minecart_vehicle_stepping() {
        // Boat buoyancy, steering, and forward water propulsion
        let mut boat = VehicleState {
            kind: VehicleKind::Boat,
            pos: Vec3::new(0.0, 62.0, 0.0),
            vel: Vec3::new(0.0, -0.05, 0.0),
            yaw: 0.0,
            passenger_eid: Some(1),
        };
        for _ in 0..10 {
            step_vehicle(&mut boat, 1.0, 1.0, true, false);
        }
        assert!(boat.yaw > 0.0, "boat should steer right");
        assert!(boat.pos.z > 0.5, "boat should move forward on water");
        assert!(boat.vel.y >= 0.0, "boat should float with buoyancy on water");

        // Minecart rail glide vs off-rail friction
        let mut cart = VehicleState {
            kind: VehicleKind::Minecart,
            pos: Vec3::new(0.0, 64.0, 0.0),
            vel: Vec3::new(0.0, 0.0, 0.35),
            yaw: 0.0,
            passenger_eid: Some(1),
        };
        step_vehicle(&mut cart, 0.0, 0.0, false, true);
        let speed_on_rail = cart.vel.z;
        assert!(speed_on_rail > 0.30, "minecart should glide smoothly on rail");
        assert_eq!(cart.vel.y, 0.0);

        step_vehicle(&mut cart, 0.0, 0.0, false, false);
        assert!(
            cart.vel.z < speed_on_rail * 0.6,
            "derailed minecart should brake heavily"
        );
        assert!(cart.vel.y < 0.0, "derailed minecart experiences gravity");
    }
}
