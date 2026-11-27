//! Minecraft 26.3 Entity `.mca` region persistence (`world/entities/r.<rx>.<rz>.mca`)
//! and per-dimension chunk ticket / grace-period unloading manager.

use crate::{EntityKind, ItemStack, RegionError, RegionFile, WorldEntity};
use mc_data::items::{item_by_id, item_by_name};
use mc_data::{
    BlockStateId, WORLD_VERSION, default_state_id, lookup_block_state_from_nbt,
    static_registry_protocol_id,
};
use mc_math::{ChunkPos, Vec3};
use mc_nbt::{NbtCompound, NbtTag};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Default grace period (200 ticks = 10 seconds) after the last player leaves an
/// unpinned chunk before it is returned for saving and unloading from memory.
pub const DEFAULT_UNLOAD_GRACE_TICKS: u32 = 200;

/// Convert a [`Uuid`] into Minecraft's 4-int big-endian `UUID` NBT array (`UUIDUtil.uuidToIntArray`).
#[inline]
pub fn uuid_to_int_array(uuid: Uuid) -> [i32; 4] {
    let b = uuid.as_bytes();
    [
        i32::from_be_bytes([b[0], b[1], b[2], b[3]]),
        i32::from_be_bytes([b[4], b[5], b[6], b[7]]),
        i32::from_be_bytes([b[8], b[9], b[10], b[11]]),
        i32::from_be_bytes([b[12], b[13], b[14], b[15]]),
    ]
}

/// Reconstruct a [`Uuid`] from Minecraft's 4-int big-endian `UUID` NBT array (`UUIDUtil.uuidFromIntArray`).
#[inline]
pub fn uuid_from_int_array(arr: &[i32]) -> Option<Uuid> {
    if arr.len() != 4 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (i, &word) in arr.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    Some(Uuid::from_bytes(bytes))
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
fn tag_as_f64(tag: &NbtTag) -> Option<f64> {
    match tag {
        NbtTag::Double(v) => Some(*v),
        NbtTag::Float(v) => Some(*v as f64),
        NbtTag::Int(v) => Some(*v as f64),
        NbtTag::Short(v) => Some(*v as f64),
        NbtTag::Byte(v) => Some(*v as f64),
        NbtTag::Long(v) => Some(*v as f64),
        _ => None,
    }
}

#[inline]
fn tag_as_f32(tag: &NbtTag) -> Option<f32> {
    match tag {
        NbtTag::Float(v) => Some(*v),
        NbtTag::Double(v) => Some(*v as f32),
        NbtTag::Int(v) => Some(*v as f32),
        NbtTag::Short(v) => Some(*v as f32),
        NbtTag::Byte(v) => Some(*v as f32),
        _ => None,
    }
}

fn read_vec3_nbt(nbt: &NbtCompound, key: &str) -> Option<Vec3> {
    let list = nbt.get_list(key)?;
    if list.len() != 3 {
        return None;
    }
    let x = tag_as_f64(&list[0])?;
    let y = tag_as_f64(&list[1])?;
    let z = tag_as_f64(&list[2])?;
    if !x.is_finite() || !y.is_finite() || !z.is_finite() {
        return None;
    }
    Some(Vec3::new(x, y, z))
}

fn read_rotation_nbt(nbt: &NbtCompound, key: &str) -> Option<(f32, f32)> {
    let list = nbt.get_list(key)?;
    if list.len() != 2 {
        return None;
    }
    let yaw = tag_as_f32(&list[0])?;
    let pitch = tag_as_f32(&list[1])?;
    if !yaw.is_finite() || !pitch.is_finite() {
        return None;
    }
    Some((yaw, pitch))
}

fn is_projectile_entity_id(id: &str) -> bool {
    matches!(
        id,
        "minecraft:arrow"
            | "minecraft:spectral_arrow"
            | "minecraft:trident"
            | "minecraft:snowball"
            | "minecraft:egg"
            | "minecraft:ender_pearl"
            | "minecraft:experience_bottle"
            | "minecraft:potion"
            | "minecraft:splash_potion"
            | "minecraft:lingering_potion"
            | "minecraft:fireball"
            | "minecraft:small_fireball"
            | "minecraft:dragon_fireball"
            | "minecraft:wither_skull"
            | "minecraft:shulker_bullet"
            | "minecraft:llama_spit"
            | "minecraft:firework_rocket"
            | "minecraft:wind_charge"
            | "minecraft:breeze_wind_charge"
            | "minecraft:fishing_bobber"
    )
}

/// Serialize a [`WorldEntity`] into Minecraft 26.3 vanilla entity NBT format.
pub fn entity_to_nbt(entity: &WorldEntity) -> NbtCompound {
    let mut c = NbtCompound::new();

    let id = match &entity.kind {
        EntityKind::Item { .. } => "minecraft:item".to_string(),
        EntityKind::FallingBlock { .. } => "minecraft:falling_block".to_string(),
        EntityKind::Mob { type_name, .. } => canonical_id(type_name),
        EntityKind::Projectile { kind_name, .. } => canonical_id(kind_name),
    };

    c.insert("id", NbtTag::String(id));
    c.insert("UUID", NbtTag::IntArray(uuid_to_int_array(entity.uuid).to_vec()));
    c.insert(
        "Pos",
        NbtTag::List(vec![
            NbtTag::Double(entity.pos.x),
            NbtTag::Double(entity.pos.y),
            NbtTag::Double(entity.pos.z),
        ]),
    );
    c.insert(
        "Motion",
        NbtTag::List(vec![
            NbtTag::Double(entity.vel.x),
            NbtTag::Double(entity.vel.y),
            NbtTag::Double(entity.vel.z),
        ]),
    );
    c.insert(
        "Rotation",
        NbtTag::List(vec![NbtTag::Float(entity.yaw), NbtTag::Float(entity.pitch)]),
    );
    c.insert("OnGround", NbtTag::Byte(if entity.on_ground { 1 } else { 0 }));

    match &entity.kind {
        EntityKind::Mob { health, .. } => {
            c.insert("Health", NbtTag::Float(*health));
        }
        EntityKind::Item {
            stack,
            pickup_delay,
            age,
        } => {
            c.insert("Age", NbtTag::Short((*age).min(i16::MAX as u16) as i16));
            c.insert(
                "PickupDelay",
                NbtTag::Short((*pickup_delay).min(i16::MAX as u16) as i16),
            );
            if !stack.is_empty() {
                if let Some(info) = item_by_id(stack.item_id) {
                    let mut item_nbt = NbtCompound::new();
                    item_nbt.insert("id", NbtTag::String(info.name.to_string()));
                    item_nbt.insert("count", NbtTag::Int(stack.count as i32));
                    c.insert("Item", NbtTag::Compound(item_nbt));
                }
            }
        }
        EntityKind::FallingBlock { block_state, age } => {
            c.insert("Time", NbtTag::Int(*age as i32));
            c.insert("BlockState", NbtTag::Compound(block_state.to_nbt()));
        }
        EntityKind::Projectile { age, damage, .. } => {
            c.insert("damage", NbtTag::Double(*damage as f64));
            c.insert("life", NbtTag::Short((*age).min(i16::MAX as u16) as i16));
        }
    }

    c
}

/// Deserialize a single Minecraft 26.3 entity NBT compound into a [`WorldEntity`].
///
/// Returns `None` if required fields are missing/invalid or the entity type is unknown.
pub fn entity_from_nbt(nbt: &NbtCompound, entity_id: i32) -> Option<WorldEntity> {
    let id = canonical_id(nbt.get_string("id")?);
    if id == "minecraft:player" {
        return None;
    }

    let pos = read_vec3_nbt(nbt, "Pos").unwrap_or(Vec3::ZERO);
    let raw_vel = read_vec3_nbt(nbt, "Motion").unwrap_or(Vec3::ZERO);
    let vel = Vec3::new(
        if raw_vel.x.abs() > 10.0 { 0.0 } else { raw_vel.x },
        if raw_vel.y.abs() > 10.0 { 0.0 } else { raw_vel.y },
        if raw_vel.z.abs() > 10.0 { 0.0 } else { raw_vel.z },
    );
    let (yaw, pitch) = read_rotation_nbt(nbt, "Rotation").unwrap_or((0.0, 0.0));
    let on_ground = nbt.get_byte("OnGround").unwrap_or(0) != 0;
    let uuid = nbt
        .get_int_array("UUID")
        .and_then(uuid_from_int_array)
        .unwrap_or_else(Uuid::new_v4);

    match id.as_str() {
        "minecraft:item" => {
            let item_c = nbt.get_compound("Item")?;
            let item_name = canonical_id(item_c.get_string("id")?);
            let info = item_by_name(&item_name)?;
            let count = item_c
                .get_int("count")
                .or_else(|| item_c.get_byte("Count").map(|b| b as i32))
                .unwrap_or(1)
                .clamp(0, 99) as u8;
            if count == 0 {
                return None;
            }
            let stack = ItemStack::new(info.id, count);
            let pickup_delay = nbt
                .get_short("PickupDelay")
                .map(|v| v.max(0) as u16)
                .or_else(|| nbt.get_int("PickupDelay").map(|v| v.clamp(0, u16::MAX as i32) as u16))
                .unwrap_or(0);
            let age = nbt
                .get_short("Age")
                .map(|v| v.max(0) as u16)
                .or_else(|| nbt.get_int("Age").map(|v| v.clamp(0, u16::MAX as i32) as u16))
                .unwrap_or(0);

            let mut ent = WorldEntity::spawn_item(entity_id, pos, vel, stack, pickup_delay);
            ent.uuid = uuid;
            ent.yaw = yaw;
            ent.pitch = pitch;
            ent.on_ground = on_ground;
            if let EntityKind::Item { age: ref mut a, .. } = ent.kind {
                *a = age;
            }
            Some(ent)
        }
        "minecraft:falling_block" => {
            let block_state = nbt
                .get_compound("BlockState")
                .and_then(lookup_block_state_from_nbt)
                .or_else(|| {
                    nbt.get_string("BlockState")
                        .and_then(|s| default_state_id(&canonical_id(s)))
                })
                .unwrap_or_else(|| {
                    default_state_id("minecraft:sand").unwrap_or(BlockStateId::STONE)
                });
            let age = nbt.get_int("Time").unwrap_or(0).clamp(0, u16::MAX as i32) as u16;
            let entity_type_id =
                static_registry_protocol_id("minecraft:entity_type", "minecraft:falling_block")
                    .unwrap_or(45);
            Some(WorldEntity {
                entity_id,
                uuid,
                entity_type_id,
                pos,
                vel,
                yaw,
                pitch,
                on_ground,
                half_width: 0.49,
                height: 0.98,
                kind: EntityKind::FallingBlock { block_state, age },
            })
        }
        other if is_projectile_entity_id(other) => {
            let entity_type_id = static_registry_protocol_id("minecraft:entity_type", other)?;
            let damage = nbt
                .get_double("damage")
                .map(|d| d as f32)
                .or_else(|| nbt.get_float("damage"))
                .filter(|d| d.is_finite())
                .unwrap_or(2.0);
            let age = nbt
                .get_short("life")
                .map(|v| v.max(0) as u16)
                .or_else(|| nbt.get_int("life").map(|v| v.clamp(0, u16::MAX as i32) as u16))
                .unwrap_or(0);
            Some(WorldEntity {
                entity_id,
                uuid,
                entity_type_id,
                pos,
                vel,
                yaw,
                pitch,
                on_ground,
                half_width: 0.125,
                height: 0.25,
                kind: EntityKind::Projectile {
                    kind_name: id,
                    owner_eid: 0,
                    age,
                    damage,
                },
            })
        }
        _ => {
            let mut ent = WorldEntity::spawn_mob(entity_id, &id, pos)?;
            ent.uuid = uuid;
            ent.vel = vel;
            ent.yaw = yaw;
            ent.pitch = pitch;
            ent.on_ground = on_ground;
            if let EntityKind::Mob { ref mut health, .. } = ent.kind {
                if let Some(h) = nbt
                    .get_float("Health")
                    .or_else(|| nbt.get_double("Health").map(|d| d as f32))
                    .filter(|h| h.is_finite())
                {
                    *health = h;
                }
            }
            Some(ent)
        }
    }
}

/// Serialize all entities belonging to a chunk into Minecraft 26.3's `entities/` `.mca`
/// chunk NBT format (`DataVersion: 5023`, `Position: [cx, cz]`, `Entities: [...]`).
pub fn entities_chunk_to_nbt(pos: ChunkPos, entities: &[&WorldEntity]) -> NbtCompound {
    let mut root = NbtCompound::new();
    root.insert("DataVersion", NbtTag::Int(WORLD_VERSION));
    root.insert("Position", NbtTag::IntArray(vec![pos.x, pos.z]));
    let list: Vec<NbtTag> = entities
        .iter()
        .map(|entity| NbtTag::Compound(entity_to_nbt(entity)))
        .collect();
    root.insert("Entities", NbtTag::List(list));
    root
}

/// Deserialize all entities from a Minecraft 26.3 `entities/` `.mca` chunk NBT compound,
/// assigning fresh runtime entity IDs via `next_eid`.
pub fn entities_chunk_from_nbt<F: FnMut() -> i32>(
    nbt: &NbtCompound,
    next_eid: &mut F,
) -> Vec<WorldEntity> {
    let Some(list) = nbt.get_list("Entities") else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(list.len());
    for tag in list {
        let Some(comp) = tag.as_compound() else {
            continue;
        };
        if let Some(mut ent) = entity_from_nbt(comp, 0) {
            ent.entity_id = next_eid();
            out.push(ent);
        }
    }
    out
}

/// Per-dimension Anvil `.mca` region store for `<dim_dir>/entities/r.<rx>.<rz>.mca`.
#[derive(Debug, Clone)]
pub struct EntityRegionStore {
    entities_dir: PathBuf,
}

impl EntityRegionStore {
    pub fn new(entities_dir: impl Into<PathBuf>) -> Self {
        Self {
            entities_dir: entities_dir.into(),
        }
    }

    pub fn entities_dir(&self) -> &Path {
        &self.entities_dir
    }

    /// Resolve `<entities_dir>/r.<rx>.<rz>.mca` for `pos`.
    pub fn region_path(&self, pos: ChunkPos) -> PathBuf {
        self.entities_dir
            .join(format!("r.{}.{}.mca", pos.region_x(), pos.region_z()))
    }

    /// Save all entities in chunk `pos` to `<entities_dir>/r.<rx>.<rz>.mca`.
    pub fn save_chunk_entities(
        &self,
        pos: ChunkPos,
        entities: &[&WorldEntity],
    ) -> Result<(), RegionError> {
        std::fs::create_dir_all(&self.entities_dir)?;
        let path = self.region_path(pos);
        let mut rf = RegionFile::open(&path)?;
        let nbt = entities_chunk_to_nbt(pos, entities);
        rf.write_chunk_nbt(pos, &nbt)
    }

    /// Load all entities for chunk `pos` from `<entities_dir>/r.<rx>.<rz>.mca`.
    /// Returns `Ok(Vec::new())` if the region file or chunk slot does not exist.
    pub fn load_chunk_entities<F: FnMut() -> i32>(
        &self,
        pos: ChunkPos,
        next_eid: &mut F,
    ) -> Result<Vec<WorldEntity>, RegionError> {
        let path = self.region_path(pos);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let mut rf = RegionFile::open_readonly(&path)?;
        let Some(nbt) = rf.read_chunk_nbt(pos)? else {
            return Ok(Vec::new());
        };
        Ok(entities_chunk_from_nbt(&nbt, next_eid))
    }
}

/// Per-dimension chunk ticket reference counter, spawn-chunk pinner, and grace-period
/// unloading scheduler.
#[derive(Debug, Clone)]
pub struct ChunkTicketManager {
    ref_counts: HashMap<ChunkPos, usize>,
    pinned: HashSet<ChunkPos>,
    pending_unload: HashMap<ChunkPos, u32>,
    grace_ticks: u32,
}

impl Default for ChunkTicketManager {
    fn default() -> Self {
        Self::with_grace_ticks(DEFAULT_UNLOAD_GRACE_TICKS)
    }
}

impl ChunkTicketManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_grace_ticks(grace_ticks: u32) -> Self {
        Self {
            ref_counts: HashMap::new(),
            pinned: HashSet::new(),
            pending_unload: HashMap::new(),
            grace_ticks,
        }
    }

    #[inline]
    pub fn grace_ticks(&self) -> u32 {
        self.grace_ticks
    }

    /// Pin a chunk (e.g. spawn chunks / force-loaded chunks) so it never unloads
    /// while pinned, cancelling any pending unload timer.
    pub fn pin_chunk(&mut self, pos: ChunkPos) {
        self.pinned.insert(pos);
        self.pending_unload.remove(&pos);
    }

    /// Unpin a previously pinned chunk. If no player tickets remain on it, schedules
    /// unloading after `grace_ticks`.
    pub fn unpin_chunk(&mut self, pos: ChunkPos) {
        if self.pinned.remove(&pos) && !self.ref_counts.contains_key(&pos) {
            self.pending_unload.insert(pos, self.grace_ticks);
        }
    }

    /// Increment the player ticket refcount for `pos` and cancel any pending unload.
    pub fn add_player_ticket(&mut self, pos: ChunkPos) {
        *self.ref_counts.entry(pos).or_insert(0) += 1;
        self.pending_unload.remove(&pos);
    }

    /// Decrement the player ticket refcount for `pos`. When the count reaches zero
    /// and the chunk is not pinned, schedules unloading after `grace_ticks`.
    pub fn remove_player_ticket(&mut self, pos: ChunkPos) {
        let reached_zero = match self.ref_counts.get_mut(&pos) {
            Some(count) => {
                *count -= 1;
                *count == 0
            }
            None => false,
        };
        if reached_zero {
            self.ref_counts.remove(&pos);
            if !self.pinned.contains(&pos) {
                self.pending_unload.insert(pos, self.grace_ticks);
            }
        }
    }

    /// Returns `true` if `pos` currently has at least one player ticket or is pinned.
    #[inline]
    pub fn is_active(&self, pos: ChunkPos) -> bool {
        self.pinned.contains(&pos) || self.ref_counts.contains_key(&pos)
    }

    #[inline]
    pub fn is_pinned(&self, pos: ChunkPos) -> bool {
        self.pinned.contains(&pos)
    }

    #[inline]
    pub fn is_pending_unload(&self, pos: ChunkPos) -> bool {
        self.pending_unload.contains_key(&pos)
    }

    #[inline]
    pub fn ticket_count(&self, pos: ChunkPos) -> usize {
        self.ref_counts.get(&pos).copied().unwrap_or(0)
    }

    /// Number of distinct chunks currently active (pinned and/or holding player tickets).
    pub fn active_count(&self) -> usize {
        let pinned_only = self
            .pinned
            .iter()
            .filter(|pos| !self.ref_counts.contains_key(*pos))
            .count();
        self.ref_counts.len() + pinned_only
    }

    /// Number of chunks currently waiting out their unload grace period.
    #[inline]
    pub fn pending_unload_count(&self) -> usize {
        self.pending_unload.len()
    }

    /// Advance all pending-unload timers by 1 tick and return every chunk whose grace
    /// period expired (ready to save dirty blocks + entities and drop from memory).
    pub fn tick(&mut self) -> Vec<ChunkPos> {
        let mut expired = Vec::new();
        self.pending_unload.retain(|&pos, remaining| {
            if *remaining <= 1 {
                expired.push(pos);
                false
            } else {
                *remaining -= 1;
                true
            }
        });
        expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD as B64;
    use mc_math::BlockPos;
    use mc_oracle::with_oracle;
    use serde_json::json;

    #[test]
    fn uuid_int_array_matches_vanilla_big_endian_layout() {
        let uuid = Uuid::from_bytes([
            0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ]);
        let ints = uuid_to_int_array(uuid);
        assert_eq!(
            ints,
            [
                0x0123_4567_u32 as i32,
                0x89AB_CDEF_u32 as i32,
                0xFEDC_BA98_u32 as i32,
                0x7654_3210_u32 as i32,
            ]
        );
        assert_eq!(uuid_from_int_array(&ints), Some(uuid));
        assert_eq!(uuid_from_int_array(&[1, 2, 3]), None);
    }

    #[test]
    fn entity_nbt_roundtrip_all_kinds() {
        // 1. Mob (zombie with partial health and custom kinematics)
        let mut zombie =
            WorldEntity::spawn_mob(10, "minecraft:zombie", Vec3::new(12.25, 64.5, -8.75)).unwrap();
        zombie.uuid = Uuid::from_u128(0x11223344_5566_7788_99aa_bbccddeeff00);
        zombie.vel = Vec3::new(0.15, -0.08, -0.25);
        zombie.yaw = 135.5;
        zombie.pitch = -12.25;
        zombie.on_ground = true;
        if let EntityKind::Mob { ref mut health, .. } = zombie.kind {
            *health = 13.5;
        }

        let z_nbt = entity_to_nbt(&zombie);
        assert_eq!(z_nbt.get_string("id"), Some("minecraft:zombie"));
        assert_eq!(
            z_nbt.get_int_array("UUID"),
            Some(&uuid_to_int_array(zombie.uuid)[..])
        );
        assert_eq!(z_nbt.get_float("Health"), Some(13.5));
        assert_eq!(z_nbt.get_byte("OnGround"), Some(1));

        let z_back = entity_from_nbt(&z_nbt, 101).unwrap();
        assert_eq!(z_back.entity_id, 101);
        assert_eq!(z_back.uuid, zombie.uuid);
        assert_eq!(z_back.pos, zombie.pos);
        assert_eq!(z_back.vel, zombie.vel);
        assert_eq!(z_back.yaw, zombie.yaw);
        assert_eq!(z_back.pitch, zombie.pitch);
        assert!(z_back.on_ground);
        match z_back.kind {
            EntityKind::Mob {
                ref type_name,
                health,
                ..
            } => {
                assert_eq!(type_name, "minecraft:zombie");
                assert_eq!(health, 13.5);
            }
            other => panic!("expected Mob, got {other:?}"),
        }

        // 2. Item entity
        let stack = ItemStack::from_name("minecraft:cobblestone", 42).unwrap();
        let mut item = WorldEntity::spawn_item(
            11,
            Vec3::new(3.5, 70.125, 9.25),
            Vec3::new(-0.05, 0.2, 0.03),
            stack.clone(),
            17,
        );
        item.uuid = Uuid::from_u128(0xaabbccdd_eeff_0011_2233_445566778899);
        item.yaw = 45.0;
        item.pitch = 5.0;
        item.on_ground = false;
        if let EntityKind::Item { ref mut age, .. } = item.kind {
            *age = 410;
        }

        let i_nbt = entity_to_nbt(&item);
        assert_eq!(i_nbt.get_string("id"), Some("minecraft:item"));
        assert_eq!(i_nbt.get_short("Age"), Some(410));
        assert_eq!(i_nbt.get_short("PickupDelay"), Some(17));
        let item_tag = i_nbt.get_compound("Item").unwrap();
        assert_eq!(item_tag.get_string("id"), Some("minecraft:cobblestone"));
        assert_eq!(item_tag.get_int("count"), Some(42));

        let i_back = entity_from_nbt(&i_nbt, 102).unwrap();
        assert_eq!(i_back.uuid, item.uuid);
        assert_eq!(i_back.pos, item.pos);
        assert_eq!(i_back.vel, item.vel);
        assert_eq!(i_back.yaw, 45.0);
        assert_eq!(i_back.pitch, 5.0);
        match i_back.kind {
            EntityKind::Item {
                stack: s,
                pickup_delay,
                age,
            } => {
                assert_eq!(s, stack);
                assert_eq!(pickup_delay, 17);
                assert_eq!(age, 410);
            }
            other => panic!("expected Item, got {other:?}"),
        }

        // 3. FallingBlock (preserving exact BlockStateId including properties)
        let button_state = BlockStateId(12514); // minecraft:acacia_button[face=floor,facing=north,powered=true]
        let mut fb = WorldEntity::spawn_falling_block(12, BlockPos::new(5, 80, -3), button_state);
        fb.uuid = Uuid::from_u128(0xdeadbeef_cafe_babe_0102_030405060708);
        fb.pos = Vec3::new(5.5, 79.75, -2.5);
        fb.vel = Vec3::new(0.0, -0.32, 0.0);
        if let EntityKind::FallingBlock { ref mut age, .. } = fb.kind {
            *age = 23;
        }

        let fb_nbt = entity_to_nbt(&fb);
        assert_eq!(fb_nbt.get_string("id"), Some("minecraft:falling_block"));
        assert_eq!(fb_nbt.get_int("Time"), Some(23));
        assert_eq!(
            fb_nbt.get_compound("BlockState").unwrap().get_string("Name"),
            Some("minecraft:acacia_button")
        );

        let fb_back = entity_from_nbt(&fb_nbt, 103).unwrap();
        assert_eq!(fb_back.uuid, fb.uuid);
        assert_eq!(fb_back.pos, fb.pos);
        assert_eq!(fb_back.vel, fb.vel);
        match fb_back.kind {
            EntityKind::FallingBlock { block_state, age } => {
                assert_eq!(block_state, button_state);
                assert_eq!(age, 23);
            }
            other => panic!("expected FallingBlock, got {other:?}"),
        }

        // 4. Projectile (arrow)
        let mut arrow = WorldEntity::spawn_projectile(
            13,
            "minecraft:arrow",
            99,
            Vec3::new(-4.0, 68.25, 16.5),
            -60.0,
            15.0,
            2.5,
            4.5,
        )
        .unwrap();
        arrow.uuid = Uuid::from_u128(0x0f1e2d3c_4b5a_6978_8796_a5b4c3d2e1f0);
        if let EntityKind::Projectile { ref mut age, .. } = arrow.kind {
            *age = 19;
        }

        let a_nbt = entity_to_nbt(&arrow);
        assert_eq!(a_nbt.get_string("id"), Some("minecraft:arrow"));
        assert_eq!(a_nbt.get_double("damage"), Some(4.5));

        let a_back = entity_from_nbt(&a_nbt, 104).unwrap();
        assert_eq!(a_back.uuid, arrow.uuid);
        assert_eq!(a_back.pos, arrow.pos);
        assert_eq!(a_back.vel, arrow.vel);
        assert_eq!(a_back.yaw, -60.0);
        assert_eq!(a_back.pitch, 15.0);
        match a_back.kind {
            EntityKind::Projectile {
                ref kind_name,
                age,
                damage,
                ..
            } => {
                assert_eq!(kind_name, "minecraft:arrow");
                assert_eq!(age, 19);
                assert_eq!(damage, 4.5);
            }
            other => panic!("expected Projectile, got {other:?}"),
        }

        // 5. Chunk-level wrapper compound
        let chunk_pos = ChunkPos::new(3, -2);
        let chunk_nbt = entities_chunk_to_nbt(chunk_pos, &[&zombie, &item, &fb, &arrow]);
        assert_eq!(chunk_nbt.get_int("DataVersion"), Some(WORLD_VERSION));
        assert_eq!(chunk_nbt.get_int_array("Position"), Some(&[3, -2][..]));
        assert_eq!(chunk_nbt.get_list("Entities").map(|l| l.len()), Some(4));

        let mut next_id = 500;
        let parsed = entities_chunk_from_nbt(&chunk_nbt, &mut || {
            let id = next_id;
            next_id += 1;
            id
        });
        assert_eq!(parsed.len(), 4);
        assert_eq!(parsed[0].entity_id, 500);
        assert_eq!(parsed[1].entity_id, 501);
        assert_eq!(parsed[2].entity_id, 502);
        assert_eq!(parsed[3].entity_id, 503);
        assert_eq!(parsed[0].uuid, zombie.uuid);
        assert_eq!(parsed[1].uuid, item.uuid);
        assert_eq!(parsed[2].uuid, fb.uuid);
        assert_eq!(parsed[3].uuid, arrow.uuid);
    }

    #[test]
    fn entity_region_store_disk_mca_roundtrip_and_java_oracle() {
        let tmp_dir = std::env::temp_dir().join(format!(
            "siliconmc_entity_mca_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        let entities_dir = tmp_dir.join("entities");
        let store = EntityRegionStore::new(&entities_dir);

        let chunk_a = ChunkPos::new(3, -2); // region (0, -1)
        let chunk_b = ChunkPos::new(-35, 1); // region (-2, 0)

        let mut eid_counter = 1000;
        let mut alloc_eid = || {
            let id = eid_counter;
            eid_counter += 1;
            id
        };

        // Missing region file loads cleanly as empty
        assert!(store.load_chunk_entities(chunk_a, &mut alloc_eid).unwrap().is_empty());

        let mut cow =
            WorldEntity::spawn_mob(1, "minecraft:cow", Vec3::new(52.5, 65.0, -24.25)).unwrap();
        cow.yaw = 90.0;
        cow.on_ground = true;
        if let EntityKind::Mob { ref mut health, .. } = cow.kind {
            *health = 7.5;
        }
        let dropped = WorldEntity::spawn_item(
            2,
            Vec3::new(50.25, 65.5, -20.5),
            Vec3::new(0.05, 0.1, -0.02),
            ItemStack::from_name("minecraft:diamond", 5).unwrap(),
            10,
        );
        let sand_fb = WorldEntity::spawn_falling_block(
            3,
            BlockPos::new(51, 72, -22),
            default_state_id("minecraft:sand").unwrap(),
        );
        let arrow = WorldEntity::spawn_projectile(
            4,
            "minecraft:arrow",
            7,
            Vec3::new(-550.5, 70.0, 20.5),
            15.0,
            -5.0,
            1.8,
            3.0,
        )
        .unwrap();

        store
            .save_chunk_entities(chunk_a, &[&cow, &dropped, &sand_fb])
            .unwrap();
        store.save_chunk_entities(chunk_b, &[&arrow]).unwrap();

        assert!(entities_dir.join("r.0.-1.mca").is_file());
        assert!(entities_dir.join("r.-2.0.mca").is_file());

        // Rust disk reload
        let loaded_a = store.load_chunk_entities(chunk_a, &mut alloc_eid).unwrap();
        assert_eq!(loaded_a.len(), 3);
        assert_eq!(loaded_a[0].uuid, cow.uuid);
        assert_eq!(loaded_a[0].pos, cow.pos);
        assert_eq!(loaded_a[1].uuid, dropped.uuid);
        assert_eq!(loaded_a[2].uuid, sand_fb.uuid);

        let loaded_b = store.load_chunk_entities(chunk_b, &mut alloc_eid).unwrap();
        assert_eq!(loaded_b.len(), 1);
        assert_eq!(loaded_b[0].uuid, arrow.uuid);
        assert_eq!(loaded_b[0].pos, arrow.pos);

        // Verify live Minecraft 26.3 Java RegionFile + NbtIo reads our entities `.mca`!
        let mca_path = store.region_path(chunk_a);
        let resp = with_oracle(|o| {
            o.call(json!({
                "op": "mca_read_chunk",
                "path": mca_path.to_string_lossy(),
                "cx": chunk_a.x,
                "cz": chunk_a.z
            }))
        })
        .unwrap();
        assert_eq!(resp["present"].as_bool(), Some(true));
        assert_eq!(resp["data_version"].as_i64(), Some(WORLD_VERSION as i64));

        let java_nbt_bytes = B64.decode(resp["bytes_b64"].as_str().unwrap()).unwrap();
        let (_, java_compound) = mc_nbt::read_file_bytes(&java_nbt_bytes).unwrap();
        assert_eq!(
            java_compound.get_int_array("Position"),
            Some(&[chunk_a.x, chunk_a.z][..])
        );
        let from_java = entities_chunk_from_nbt(&java_compound, &mut alloc_eid);
        assert_eq!(from_java.len(), 3);
        assert_eq!(from_java[0].uuid, cow.uuid);
        assert_eq!(from_java[1].uuid, dropped.uuid);
        assert_eq!(from_java[2].uuid, sand_fb.uuid);

        // Overwrite chunk_a with empty entities list and verify reload is empty
        store.save_chunk_entities(chunk_a, &[]).unwrap();
        assert!(store.load_chunk_entities(chunk_a, &mut alloc_eid).unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn chunk_ticket_manager_refcount_pinning_grace_and_cancel() {
        // Default grace ticks check
        let default_mgr = ChunkTicketManager::default();
        assert_eq!(default_mgr.grace_ticks(), DEFAULT_UNLOAD_GRACE_TICKS);
        assert_eq!(DEFAULT_UNLOAD_GRACE_TICKS, 200);

        let mut mgr = ChunkTicketManager::with_grace_ticks(5);
        let c1 = ChunkPos::new(1, 2);
        let spawn = ChunkPos::new(0, 0);

        // Pin spawn chunk
        mgr.pin_chunk(spawn);
        assert!(mgr.is_active(spawn));
        assert!(mgr.is_pinned(spawn));
        assert_eq!(mgr.active_count(), 1);
        assert_eq!(mgr.pending_unload_count(), 0);

        // Two players enter c1
        mgr.add_player_ticket(c1);
        mgr.add_player_ticket(c1);
        assert!(mgr.is_active(c1));
        assert_eq!(mgr.ticket_count(c1), 2);
        assert_eq!(mgr.active_count(), 2);

        // First player leaves c1 -> still active, not scheduled for unload
        mgr.remove_player_ticket(c1);
        assert!(mgr.is_active(c1));
        assert_eq!(mgr.ticket_count(c1), 1);
        assert_eq!(mgr.pending_unload_count(), 0);

        // Second player leaves c1 -> scheduled for unload after 5 ticks
        mgr.remove_player_ticket(c1);
        assert!(!mgr.is_active(c1));
        assert_eq!(mgr.ticket_count(c1), 0);
        assert_eq!(mgr.active_count(), 1); // only spawn remains active
        assert_eq!(mgr.pending_unload_count(), 1);
        assert!(mgr.is_pending_unload(c1));

        // Advance 3 ticks (still within grace period)
        for _ in 0..3 {
            assert!(mgr.tick().is_empty());
        }
        assert_eq!(mgr.pending_unload_count(), 1);

        // Player re-enters c1 mid-grace -> cancels pending unload!
        mgr.add_player_ticket(c1);
        assert!(mgr.is_active(c1));
        assert!(!mgr.is_pending_unload(c1));
        assert_eq!(mgr.pending_unload_count(), 0);
        for _ in 0..10 {
            assert!(mgr.tick().is_empty());
        }

        // Player leaves again -> full 5-tick grace period counts down and expires on tick 5
        mgr.remove_player_ticket(c1);
        assert_eq!(mgr.pending_unload_count(), 1);
        for _ in 0..4 {
            assert!(mgr.tick().is_empty());
        }
        let expired = mgr.tick();
        assert_eq!(expired, vec![c1]);
        assert_eq!(mgr.pending_unload_count(), 0);
        assert!(!mgr.is_active(c1));

        // Player entering & leaving a pinned spawn chunk never schedules unload
        mgr.add_player_ticket(spawn);
        assert_eq!(mgr.active_count(), 1); // distinct count stays 1!
        mgr.remove_player_ticket(spawn);
        assert!(mgr.is_active(spawn));
        assert_eq!(mgr.pending_unload_count(), 0);
        for _ in 0..10 {
            assert!(mgr.tick().is_empty());
        }

        // Unpinning spawn chunk with 0 player tickets schedules grace-period unload
        mgr.unpin_chunk(spawn);
        assert!(!mgr.is_active(spawn));
        assert_eq!(mgr.active_count(), 0);
        assert_eq!(mgr.pending_unload_count(), 1);

        // Re-pinning mid-grace cancels unload
        assert!(mgr.tick().is_empty());
        mgr.pin_chunk(spawn);
        assert!(mgr.is_active(spawn));
        assert_eq!(mgr.pending_unload_count(), 0);

        // Unpin again and let it fully expire
        mgr.unpin_chunk(spawn);
        for _ in 0..4 {
            assert!(mgr.tick().is_empty());
        }
        assert_eq!(mgr.tick(), vec![spawn]);
        assert_eq!(mgr.pending_unload_count(), 0);
    }
}
