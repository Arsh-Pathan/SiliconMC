use crate::{BlockWorld, ItemStack};
use mc_data::{BlockStateId, static_registry_protocol_id};
use mc_math::{BlockPos, Vec3};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum EntityKind {
    Item {
        stack: ItemStack,
        pickup_delay: u16,
        age: u16,
    },
    FallingBlock {
        block_state: BlockStateId,
        age: u16,
    },
    Mob {
        type_name: String,
        health: f32,
        max_health: f32,
        ai_cooldown: u16,
        hurt_ticks: u8,
        wander_dir: (f64, f64),
    },
    Projectile {
        kind_name: String,
        owner_eid: i32,
        age: u16,
        damage: f32,
    },
}

#[derive(Debug, Clone)]
pub struct WorldEntity {
    pub entity_id: i32,
    pub uuid: Uuid,
    pub entity_type_id: i32,
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub half_width: f64,
    pub height: f64,
    pub kind: EntityKind,
}

#[derive(Debug, Clone)]
pub enum EntityTickOutcome {
    Alive { moved: bool },
    Despawn,
    LandedFallingBlock { pos: BlockPos, state: BlockStateId },
    ProjectileHit {
        hit_pos: Vec3,
        kind_name: String,
        owner_eid: i32,
        damage: f32,
    },
}

impl WorldEntity {
    pub fn spawn_item(entity_id: i32, pos: Vec3, vel: Vec3, stack: ItemStack, pickup_delay: u16) -> Self {
        let item_type_id =
            static_registry_protocol_id("minecraft:entity_type", "minecraft:item").unwrap_or(67);
        Self {
            entity_id,
            uuid: Uuid::new_v4(),
            entity_type_id: item_type_id,
            pos,
            vel,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            half_width: 0.125,
            height: 0.25,
            kind: EntityKind::Item {
                stack,
                pickup_delay,
                age: 0,
            },
        }
    }

    pub fn spawn_falling_block(entity_id: i32, pos: BlockPos, block_state: BlockStateId) -> Self {
        let fb_type_id =
            static_registry_protocol_id("minecraft:entity_type", "minecraft:falling_block")
                .unwrap_or(45);
        Self {
            entity_id,
            uuid: Uuid::new_v4(),
            entity_type_id: fb_type_id,
            pos: Vec3::new(pos.x as f64 + 0.5, pos.y as f64, pos.z as f64 + 0.5),
            vel: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            half_width: 0.49,
            height: 0.98,
            kind: EntityKind::FallingBlock {
                block_state,
                age: 0,
            },
        }
    }

    pub fn spawn_mob(entity_id: i32, type_name: &str, pos: Vec3) -> Option<Self> {
        let full = if type_name.contains(':') {
            type_name.to_string()
        } else {
            format!("minecraft:{type_name}")
        };
        let entity_type_id = static_registry_protocol_id("minecraft:entity_type", &full)?;
        let (max_health, half_width, height) = match full.as_str() {
            "minecraft:chicken" | "minecraft:rabbit" => (4.0, 0.2, 0.7),
            "minecraft:pig" | "minecraft:sheep" | "minecraft:cow" => (10.0, 0.45, 1.3),
            "minecraft:iron_golem" => (100.0, 0.7, 2.7),
            _ => (20.0, 0.3, 1.8),
        };
        Some(Self {
            entity_id,
            uuid: Uuid::new_v4(),
            entity_type_id,
            pos,
            vel: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            half_width,
            height,
            kind: EntityKind::Mob {
                type_name: full,
                health: max_health,
                max_health,
                ai_cooldown: 20,
                hurt_ticks: 0,
                wander_dir: (0.0, 0.0),
            },
        })
    }

    pub fn spawn_projectile(
        entity_id: i32,
        kind_name: &str,
        owner_eid: i32,
        pos: Vec3,
        yaw: f32,
        pitch: f32,
        speed: f64,
        damage: f32,
    ) -> Option<Self> {
        let entity_type_id = static_registry_protocol_id("minecraft:entity_type", kind_name)?;
        let yr = (yaw as f64).to_radians();
        let pr = (pitch as f64).to_radians();
        let vx = -yr.sin() * pr.cos() * speed;
        let vy = -pr.sin() * speed;
        let vz = yr.cos() * pr.cos() * speed;
        Some(Self {
            entity_id,
            uuid: Uuid::new_v4(),
            entity_type_id,
            pos,
            vel: Vec3::new(vx, vy, vz),
            yaw,
            pitch,
            on_ground: false,
            half_width: 0.125,
            height: 0.25,
            kind: EntityKind::Projectile {
                kind_name: kind_name.to_string(),
                owner_eid,
                age: 0,
                damage,
            },
        })
    }

    pub fn add_entity_data_field(&self) -> i32 {
        match &self.kind {
            EntityKind::FallingBlock { block_state, .. } => block_state.0 as i32,
            EntityKind::Projectile { owner_eid, .. } => *owner_eid,
            _ => 0,
        }
    }

    /// Advance 1 server tick (1/20s) with exact vanilla-style axis-separated AABB block collision & gravity.
    pub fn tick<W: BlockWorld>(&mut self, world: &mut W, tick_seed: u64) -> EntityTickOutcome {
        let old_pos = self.pos;
        let old_yaw = self.yaw;

        let gravity = match &mut self.kind {
            EntityKind::Item {
                pickup_delay, age, ..
            } => {
                if *pickup_delay > 0 {
                    *pickup_delay -= 1;
                }
                *age += 1;
                if *age >= 6000 {
                    return EntityTickOutcome::Despawn;
                }
                -0.04
            }
            EntityKind::FallingBlock { age, .. } => {
                *age += 1;
                if *age > 600 {
                    return EntityTickOutcome::Despawn;
                }
                -0.04
            }
            EntityKind::Mob {
                health,
                ai_cooldown,
                hurt_ticks,
                wander_dir,
                ..
            } => {
                if *health <= 0.0 {
                    return EntityTickOutcome::Despawn;
                }
                if *hurt_ticks > 0 {
                    *hurt_ticks -= 1;
                }
                if *ai_cooldown == 0 {
                    // Deterministic pseudo-random wandering decision
                    let h = tick_seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(self.entity_id as u64 * 1442695040888963407);
                    if (h & 3) == 0 {
                        *wander_dir = (0.0, 0.0);
                        *ai_cooldown = 30 + ((h >> 8) % 40) as u16;
                    } else {
                        let angle = (((h >> 16) & 0xFFFF) as f64 / 65536.0) * std::f64::consts::TAU;
                        let speed = 0.09;
                        *wander_dir = (angle.sin() * speed, angle.cos() * speed);
                        self.yaw = (-(angle.sin().atan2(angle.cos()))).to_degrees() as f32;
                        *ai_cooldown = 25 + ((h >> 32) % 35) as u16;
                    }
                } else {
                    *ai_cooldown -= 1;
                }
                if self.on_ground && (wander_dir.0 != 0.0 || wander_dir.1 != 0.0) {
                    self.vel.x = wander_dir.0;
                    self.vel.z = wander_dir.1;
                }
                -0.08
            }
            EntityKind::Projectile { age, .. } => {
                *age += 1;
                if *age > 400 || self.pos.y < -128.0 {
                    return EntityTickOutcome::Despawn;
                }
                -0.03
            }
        };

        self.vel.y += gravity;

        // Axis-separated AABB collision sweep: Y -> X -> Z
        let hit_y = self.move_axis(world, 1, self.vel.y);
        if hit_y {
            if self.vel.y < 0.0 {
                self.on_ground = true;
            }
            self.vel.y = 0.0;
        } else {
            self.on_ground = false;
        }

        let hit_x = self.move_axis(world, 0, self.vel.x);
        let hit_z = self.move_axis(world, 2, self.vel.z);
        if let EntityKind::Projectile {
            ref kind_name,
            owner_eid,
            damage,
            ..
        } = self.kind
        {
            if hit_x || hit_y || hit_z {
                return EntityTickOutcome::ProjectileHit {
                    hit_pos: self.pos,
                    kind_name: kind_name.clone(),
                    owner_eid,
                    damage,
                };
            }
        }
        if hit_x {
            self.vel.x = 0.0;
        }
        if hit_z {
            self.vel.z = 0.0;
        }

        // Auto-jump for wandering mobs hitting a 1-block ledge
        if (hit_x || hit_z) && self.on_ground && matches!(self.kind, EntityKind::Mob { .. }) {
            self.vel.y = 0.42;
            self.on_ground = false;
        }

        // Drag & ground friction
        let horiz_drag = if self.on_ground { 0.588 } else { 0.98 };
        self.vel.x *= horiz_drag;
        self.vel.z *= horiz_drag;
        self.vel.y *= 0.98;
        if self.vel.x.abs() < 1e-4 {
            self.vel.x = 0.0;
        }
        if self.vel.z.abs() < 1e-4 {
            self.vel.z = 0.0;
        }

        if let EntityKind::FallingBlock { block_state, .. } = self.kind {
            if self.on_ground || self.pos.y <= -64.0 {
                let land_pos = BlockPos::new(
                    self.pos.x.floor() as i32,
                    self.pos.y.floor() as i32,
                    self.pos.z.floor() as i32,
                );
                return EntityTickOutcome::LandedFallingBlock {
                    pos: land_pos,
                    state: block_state,
                };
            }
        }

        let moved = (self.pos.x - old_pos.x).abs() > 1e-4
            || (self.pos.y - old_pos.y).abs() > 1e-4
            || (self.pos.z - old_pos.z).abs() > 1e-4
            || (self.yaw - old_yaw).abs() > 0.5;
        EntityTickOutcome::Alive { moved }
    }

    fn move_axis<W: BlockWorld>(&mut self, world: &mut W, axis: u8, delta: f64) -> bool {
        if delta.abs() < 1e-7 {
            return false;
        }
        let mut next = self.pos;
        match axis {
            0 => next.x += delta,
            1 => next.y += delta,
            _ => next.z += delta,
        }
        let min_x = (next.x - self.half_width).floor() as i32;
        let max_x = (next.x + self.half_width - 1e-6).floor() as i32;
        let min_y = next.y.floor() as i32;
        let max_y = (next.y + self.height - 1e-6).floor() as i32;
        let min_z = (next.z - self.half_width).floor() as i32;
        let max_z = (next.z + self.half_width - 1e-6).floor() as i32;

        for by in min_y..=max_y {
            for bz in min_z..=max_z {
                for bx in min_x..=max_x {
                    let st = world.get_block_state(BlockPos::new(bx, by, bz));
                    if st.props().is_solid() {
                        // Snap cleanly against the collided block face
                        match axis {
                            1 => {
                                if delta < 0.0 {
                                    self.pos.y = (by + 1) as f64;
                                } else {
                                    self.pos.y = (by as f64) - self.height;
                                }
                            }
                            0 => {
                                if delta < 0.0 {
                                    self.pos.x = (bx + 1) as f64 + self.half_width;
                                } else {
                                    self.pos.x = (bx as f64) - self.half_width;
                                }
                            }
                            _ => {
                                if delta < 0.0 {
                                    self.pos.z = (bz + 1) as f64 + self.half_width;
                                } else {
                                    self.pos.z = (bz as f64) - self.half_width;
                                }
                            }
                        }
                        return true;
                    }
                }
            }
        }
        self.pos = next;
        false
    }
}
