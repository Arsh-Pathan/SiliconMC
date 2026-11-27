//! Additional Minecraft 26.3 (Protocol 777) Play-phase packet encoders and decoders.
//!
//! Every wire layout in this module was verified directly against the decompiled
//! Minecraft 26.3 Java classes in `net.minecraft.network.protocol.game.*` and
//! round-trip verified byte-for-byte against the live Java 26.3 `OracleMain`.

use crate::{
    InteractionHand, PacketReader, PacketWriter, ProtoError, encode_play_add_entity, pack_degrees,
    text_component, write_item_stack_network,
};
use mc_data::packet_ids;
use mc_math::Vec3;
use mc_nbt::{NbtCompound, NbtTag};
use mc_world::ItemStack;
use uuid::Uuid;

// ======================== Shared Protocol 777 Types ========================

/// Weighted block-explosion particle entry (`Weighted<ExplosionParticleInfo>` in 26.3).
///
/// Assumes a `SimpleParticleType` (zero extra payload bytes after the `VarInt` particle type ID).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExplosionParticleEntry {
    /// Registry protocol ID in `minecraft:particle_type`.
    pub particle_id: i32,
    pub scaling: f32,
    pub speed: f32,
    pub weight: i32,
}

/// Action IDs for `ClientboundAnimatePacket` in Minecraft 26.3.
///
/// Note: Hand swing animations moved to `ClientboundSwingAnimationPacket` (`minecraft:swing_animation`) in 26.3.
pub mod animate_action {
    pub const WAKE_UP: u8 = 0;
    pub const CRITICAL_HIT: u8 = 1;
    pub const MAGIC_CRITICAL_HIT: u8 = 2;
}

/// `net.minecraft.world.item.SwingAnimationType` (26.3 `ClientboundSwingAnimationPacket`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum SwingAnimationType {
    None = 0,
    Whack = 1,
    Stab = 2,
}

impl SwingAnimationType {
    pub fn from_id(id: i32) -> Result<Self, ProtoError> {
        match id {
            0 => Ok(Self::None),
            1 => Ok(Self::Whack),
            2 => Ok(Self::Stab),
            other => Err(ProtoError::InvalidValue(other)),
        }
    }
}

/// `net.minecraft.world.Difficulty` (`Difficulty.STREAM_CODEC` uses `VarInt` IDs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Difficulty {
    Peaceful = 0,
    Easy = 1,
    Normal = 2,
    Hard = 3,
}

impl Difficulty {
    pub fn from_id(id: i32) -> Result<Self, ProtoError> {
        match id {
            0 => Ok(Self::Peaceful),
            1 => Ok(Self::Easy),
            2 => Ok(Self::Normal),
            3 => Ok(Self::Hard),
            other => Err(ProtoError::InvalidValue(other)),
        }
    }
}

/// Single step in a 26.3 `VecDelta.Stepped` (`ClientboundMoveEntityPacket`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VecDeltaStep {
    pub dx: i16,
    pub dy: i16,
    pub dz: i16,
    pub ticks: i32,
}

/// `net.minecraft.world.entity.EquipmentSlot` using exact Java enum **ordinals**,
/// which `ClientboundSetEquipmentPacket` writes on the wire (`slotType.ordinal()`,
/// NOT `slotType.getId()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EquipmentSlot {
    MainHand = 0,
    OffHand = 1,
    Feet = 2,
    Legs = 3,
    Chest = 4,
    Head = 5,
    Body = 6,
    Saddle = 7,
}

impl EquipmentSlot {
    pub fn from_ordinal(ordinal: u8) -> Result<Self, ProtoError> {
        match ordinal {
            0 => Ok(Self::MainHand),
            1 => Ok(Self::OffHand),
            2 => Ok(Self::Feet),
            3 => Ok(Self::Legs),
            4 => Ok(Self::Chest),
            5 => Ok(Self::Head),
            6 => Ok(Self::Body),
            7 => Ok(Self::Saddle),
            other => Err(ProtoError::InvalidValue(other as i32)),
        }
    }
}

/// Method discriminator for `ClientboundSetObjectivePacket`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i8)]
pub enum ObjectiveMethod {
    Add = 0,
    Remove = 1,
    Change = 2,
}

/// `net.minecraft.world.scores.criteria.ObjectiveCriteria.RenderType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ObjectiveRenderType {
    Integer = 0,
    Hearts = 1,
}

/// `net.minecraft.network.chat.numbers.NumberFormat` (`NumberFormatTypes.STREAM_CODEC`).
#[derive(Debug, Clone, PartialEq)]
pub enum ScoreNumberFormat {
    /// `minecraft:blank` (registry protocol ID 0).
    Blank,
    /// `minecraft:styled` (registry protocol ID 1): network NBT `Style` compound.
    Styled(NbtCompound),
    /// `minecraft:fixed` (registry protocol ID 2): network NBT `Component` tag.
    Fixed(NbtTag),
}

fn write_optional_number_format(
    w: &mut PacketWriter,
    fmt: Option<&ScoreNumberFormat>,
) -> Result<(), ProtoError> {
    match fmt {
        None => {
            w.write_bool(false);
        }
        Some(ScoreNumberFormat::Blank) => {
            w.write_bool(true);
            w.write_varint(0);
        }
        Some(ScoreNumberFormat::Styled(style)) => {
            w.write_bool(true);
            w.write_varint(1);
            w.write_nbt_network(&NbtTag::Compound(style.clone()))?;
        }
        Some(ScoreNumberFormat::Fixed(component)) => {
            w.write_bool(true);
            w.write_varint(2);
            w.write_nbt_network(component)?;
        }
    }
    Ok(())
}

/// `net.minecraft.world.scores.DisplaySlot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum DisplaySlot {
    List = 0,
    Sidebar = 1,
    BelowName = 2,
    TeamBlack = 3,
    TeamDarkBlue = 4,
    TeamDarkGreen = 5,
    TeamDarkAqua = 6,
    TeamDarkRed = 7,
    TeamDarkPurple = 8,
    TeamGold = 9,
    TeamGray = 10,
    TeamDarkGray = 11,
    TeamBlue = 12,
    TeamGreen = 13,
    TeamAqua = 14,
    TeamRed = 15,
    TeamLightPurple = 16,
    TeamYellow = 17,
    TeamWhite = 18,
}

/// `net.minecraft.world.BossEvent.BossBarColor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum BossBarColor {
    Pink = 0,
    Blue = 1,
    Red = 2,
    Green = 3,
    Yellow = 4,
    Purple = 5,
    White = 6,
}

/// `net.minecraft.world.BossEvent.BossBarOverlay`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum BossBarOverlay {
    Progress = 0,
    Notched6 = 1,
    Notched10 = 2,
    Notched12 = 3,
    Notched20 = 4,
}

/// `net.minecraft.sounds.SoundSource` (note: includes `Ui = 10` in 26.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum SoundSource {
    Master = 0,
    Music = 1,
    Records = 2,
    Weather = 3,
    Blocks = 4,
    Hostile = 5,
    Neutral = 6,
    Players = 7,
    Ambient = 8,
    Voice = 9,
    Ui = 10,
}

/// `net.minecraft.world.item.trading.ItemCost` (`ItemCost.STREAM_CODEC`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemCost {
    /// 0-based protocol ID in `minecraft:item`.
    pub item_id: i32,
    pub count: i32,
    /// `DataComponentExactPredicate`: list of `(component_type_id, encoded_value_bytes)`.
    pub components: Vec<(i32, Vec<u8>)>,
}

impl ItemCost {
    pub fn simple(item_id: i32, count: i32) -> Self {
        Self {
            item_id,
            count,
            components: Vec::new(),
        }
    }

    fn write(&self, w: &mut PacketWriter) {
        // Item.STREAM_CODEC = ByteBufCodecs.holderRegistry(Registries.ITEM) -> raw VarInt ID
        w.write_varint(self.item_id);
        w.write_varint(self.count);
        // DataComponentExactPredicate.STREAM_CODEC = list of TypedDataComponent
        w.write_varint(self.components.len() as i32);
        for (type_id, raw_bytes) in &self.components {
            w.write_varint(*type_id);
            w.write_raw(raw_bytes);
        }
    }
}

/// `net.minecraft.world.item.trading.MerchantOffer` (`MerchantOffer.STREAM_CODEC`).
#[derive(Debug, Clone, PartialEq)]
pub struct MerchantOffer {
    pub base_cost_a: ItemCost,
    /// Must be non-empty (`ItemStack.STREAM_CODEC` rejects empty stacks).
    pub result: ItemStack,
    pub cost_b: Option<ItemCost>,
    pub uses: i32,
    pub max_uses: i32,
    pub xp: i32,
    pub special_price_diff: i32,
    pub price_multiplier: f32,
    pub demand: i32,
}

impl MerchantOffer {
    #[inline]
    pub fn is_out_of_stock(&self) -> bool {
        self.uses >= self.max_uses
    }
}

/// `net.minecraft.world.scores.Team.Visibility` (`VarInt` ID via `ByteBufCodecs.idMapper` in 26.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum TeamVisibility {
    Always = 0,
    Never = 1,
    HideForOtherTeams = 2,
    HideForOwnTeam = 3,
}

/// `net.minecraft.world.scores.Team.CollisionRule` (`VarInt` ID via `ByteBufCodecs.idMapper` in 26.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum TeamCollisionRule {
    Always = 0,
    Never = 1,
    PushOtherTeams = 2,
    PushOwnTeam = 3,
}

/// `net.minecraft.world.scores.TeamColor` (`Optional<TeamColor>` replaced `ChatFormatting` in 26.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum TeamColor {
    Black = 0,
    DarkBlue = 1,
    DarkGreen = 2,
    DarkAqua = 3,
    DarkRed = 4,
    DarkPurple = 5,
    Gold = 6,
    Gray = 7,
    DarkGray = 8,
    Blue = 9,
    Green = 10,
    Aqua = 11,
    Red = 12,
    LightPurple = 13,
    Yellow = 14,
    White = 15,
}

/// `ClientboundSetPlayerTeamPacket.Parameters` (Minecraft 26.3 wire layout).
///
/// Option bit flags: bit 0 (`0x01`) = allow friendly fire, bit 1 (`0x02`) = see friendly invisibles.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerTeamParams {
    pub display_name: NbtTag,
    pub player_prefix: NbtTag,
    pub player_suffix: NbtTag,
    pub name_tag_visibility: TeamVisibility,
    pub collision_rule: TeamCollisionRule,
    pub color: Option<TeamColor>,
    pub options: u8,
}

impl PlayerTeamParams {
    fn write(&self, w: &mut PacketWriter) -> Result<(), ProtoError> {
        // Exact 26.3 order: displayName, playerPrefix, playerSuffix,
        // nameTagVisibility, collisionRule, Optional<TeamColor>, options(byte).
        w.write_nbt_network(&self.display_name)?;
        w.write_nbt_network(&self.player_prefix)?;
        w.write_nbt_network(&self.player_suffix)?;
        w.write_varint(self.name_tag_visibility as i32);
        w.write_varint(self.collision_rule as i32);
        match self.color {
            Some(c) => {
                w.write_bool(true);
                w.write_varint(c as i32);
            }
            None => w.write_bool(false),
        }
        w.write_u8(self.options);
        Ok(())
    }
}

/// `ServerboundPlayerCommandPacket.Action` ordinals in Minecraft 26.3.
///
/// Note: Sneak actions were removed; sneaking is sent via `ServerboundPlayerInputPacket`.
pub mod player_command_action {
    pub const STOP_SLEEPING: i32 = 0;
    pub const START_SPRINTING: i32 = 1;
    pub const STOP_SPRINTING: i32 = 2;
    pub const START_RIDING_JUMP: i32 = 3;
    pub const STOP_RIDING_JUMP: i32 = 4;
    pub const OPEN_INVENTORY: i32 = 5;
    pub const START_FALL_FLYING: i32 = 6;
}

/// `net.minecraft.world.entity.player.Input` flag bits for `ServerboundPlayerInputPacket`.
pub mod player_input_flags {
    pub const FORWARD: u8 = 1 << 0;
    pub const BACKWARD: u8 = 1 << 1;
    pub const LEFT: u8 = 1 << 2;
    pub const RIGHT: u8 = 1 << 3;
    pub const JUMP: u8 = 1 << 4;
    pub const SHIFT: u8 = 1 << 5;
    pub const SPRINT: u8 = 1 << 6;
}

// ======================== Play Clientbound Encoders ========================

/// 1. `ClientboundExplodePacket` (Minecraft 26.3 Protocol 777).
///
/// Wire format:
/// - `Vec3 center` (`3 × f64`)
/// - `float radius` (`f32`)
/// - `int blockCount` (4-byte BE `i32`, NOT VarInt)
/// - `Optional<Vec3> playerKnockback` (`bool` + optional `3 × f64`)
/// - `ParticleOptions explosionParticle` (`VarInt` simple particle type ID)
/// - `Holder<SoundEvent> explosionSound` (`VarInt(sound_id + 1)` registry holder)
/// - `WeightedList<ExplosionParticleInfo> blockParticles` (`VarInt len`, each: `VarInt particle_id`, `f32 scaling`, `f32 speed`, `VarInt weight`)
/// - `boolean playSound` (`bool` — new in 26.3)
#[allow(clippy::too_many_arguments)]
pub fn encode_play_explode(
    center: Vec3,
    radius: f32,
    block_count: i32,
    player_knockback: Option<Vec3>,
    explosion_particle_id: i32,
    explosion_sound_id: i32,
    block_particles: &[ExplosionParticleEntry],
    play_sound: bool,
) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(64 + block_particles.len() * 12);
    w.write_varint(packet_ids::play::clientbound::EXPLODE);
    w.write_vec3(center);
    w.write_f32(radius);
    w.write_i32(block_count);
    match player_knockback {
        Some(kb) => {
            w.write_bool(true);
            w.write_vec3(kb);
        }
        None => w.write_bool(false),
    }
    w.write_varint(explosion_particle_id);
    // SoundEvent.STREAM_CODEC = ByteBufCodecs.holder -> registry ID + 1 (0 is reserved for direct SoundEvent)
    w.write_varint(explosion_sound_id + 1);
    w.write_varint(block_particles.len() as i32);
    for entry in block_particles {
        w.write_varint(entry.particle_id);
        w.write_f32(entry.scaling);
        w.write_f32(entry.speed);
        w.write_varint(entry.weight);
    }
    w.write_bool(play_sound);
    w.into_bytes()
}

/// 2. `ClientboundAnimatePacket` (Protocol 777: `WAKE_UP=0`, `CRITICAL_HIT=1`, `MAGIC_CRITICAL_HIT=2`).
pub fn encode_play_animate(entity_id: i32, action: u8) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::ANIMATE);
    w.write_varint(entity_id);
    w.write_u8(action);
    w.into_bytes()
}

/// Companion 26.3 packet: `ClientboundSwingAnimationPacket` (`minecraft:swing_animation`),
/// which replaced the old swing action IDs from `ClientboundAnimatePacket`.
pub fn encode_play_swing_animation(
    entity_id: i32,
    hand: InteractionHand,
    animation_type: SwingAnimationType,
    duration: i32,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SWING_ANIMATION);
    w.write_varint(entity_id);
    w.write_varint(hand as i32);
    w.write_varint(animation_type as i32);
    w.write_varint(duration);
    w.into_bytes()
}

/// 3. `ClientboundChangeDifficultyPacket` (`Difficulty.STREAM_CODEC` `VarInt` + `bool locked`).
pub fn encode_play_change_difficulty(difficulty: Difficulty, locked: bool) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::CHANGE_DIFFICULTY);
    w.write_varint(difficulty as i32);
    w.write_bool(locked);
    w.into_bytes()
}

/// 4. `ClientboundRotateHeadPacket` (`entity_id: VarInt`, `yHeadRot: i8`).
pub fn encode_play_rotate_head(entity_id: i32, head_yaw: f32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::ROTATE_HEAD);
    w.write_varint(entity_id);
    w.write_i8(pack_degrees(head_yaw));
    w.into_bytes()
}

/// 5. `ClientboundMoveEntityPacket.Pos` (Minecraft 26.3 `VecDelta.Linear` wire layout).
///
/// 26.3 wire format:
/// - `entity_id: VarInt`
/// - `properties: VarInt` = `(on_ground ? 1 : 0) | (step_count << 1)` (`step_count = 0` for linear)
/// - `dx: i16`, `dy: i16`, `dz: i16`
pub fn encode_play_move_entity_pos(
    entity_id: i32,
    dx: i16,
    dy: i16,
    dz: i16,
    on_ground: bool,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::MOVE_ENTITY_POS);
    w.write_varint(entity_id);
    w.write_varint(if on_ground { 1 } else { 0 });
    w.write_i16(dx);
    w.write_i16(dy);
    w.write_i16(dz);
    w.into_bytes()
}

/// `ClientboundMoveEntityPacket.Pos` using 26.3 `VecDelta.Stepped` (`step_count = steps.len()`).
///
/// Note: `steps` must be non-empty for stepped encoding; if empty, encodes a zero `VecDelta.Linear`.
pub fn encode_play_move_entity_pos_stepped(
    entity_id: i32,
    steps: &[VecDeltaStep],
    on_ground: bool,
) -> Vec<u8> {
    if steps.is_empty() {
        return encode_play_move_entity_pos(entity_id, 0, 0, 0, on_ground);
    }
    let mut w = PacketWriter::with_capacity(8 + steps.len() * 8);
    w.write_varint(packet_ids::play::clientbound::MOVE_ENTITY_POS);
    w.write_varint(entity_id);
    w.write_varint((if on_ground { 1 } else { 0 }) | ((steps.len() as i32) << 1));
    for s in steps {
        w.write_varint(s.ticks);
        w.write_i16(s.dx);
        w.write_i16(s.dy);
        w.write_i16(s.dz);
    }
    w.into_bytes()
}

/// 6. `ClientboundMoveEntityPacket.PosRot` (Minecraft 26.3 `VecDelta.Linear` wire layout).
///
/// 26.3 wire format:
/// - `entity_id: VarInt`
/// - `properties: VarInt` = `(on_ground ? 1 : 0) | (0 << 1)`
/// - `dx: i16`, `dy: i16`, `dz: i16`
/// - `y_rot: i8`, `x_rot: i8`
pub fn encode_play_move_entity_pos_rot(
    entity_id: i32,
    dx: i16,
    dy: i16,
    dz: i16,
    yaw: f32,
    pitch: f32,
    on_ground: bool,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::MOVE_ENTITY_POS_ROT);
    w.write_varint(entity_id);
    w.write_varint(if on_ground { 1 } else { 0 });
    w.write_i16(dx);
    w.write_i16(dy);
    w.write_i16(dz);
    w.write_i8(pack_degrees(yaw));
    w.write_i8(pack_degrees(pitch));
    w.into_bytes()
}

/// 7. `ClientboundMoveEntityPacket.Rot` (Minecraft 26.3 wire layout).
///
/// **26.3 order change**: `on_ground: bool` is written **before** `y_rot: i8` and `x_rot: i8`!
pub fn encode_play_move_entity_rot(
    entity_id: i32,
    yaw: f32,
    pitch: f32,
    on_ground: bool,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::MOVE_ENTITY_ROT);
    w.write_varint(entity_id);
    w.write_bool(on_ground);
    w.write_i8(pack_degrees(yaw));
    w.write_i8(pack_degrees(pitch));
    w.into_bytes()
}

/// 8. `ClientboundSetEquipmentPacket` (`slots` must be non-empty).
///
/// Each entry writes `(slot.ordinal() as u8) | (if more { 0x80 } else { 0 })` followed by
/// `ItemStack.OPTIONAL_STREAM_CODEC`.
pub fn encode_play_set_equipment(
    entity_id: i32,
    slots: &[(EquipmentSlot, ItemStack)],
) -> Result<Vec<u8>, ProtoError> {
    if slots.is_empty() {
        return Err(ProtoError::InvalidValue(0));
    }
    let mut w = PacketWriter::with_capacity(8 + slots.len() * 8);
    w.write_varint(packet_ids::play::clientbound::SET_EQUIPMENT);
    w.write_varint(entity_id);
    let last = slots.len() - 1;
    for (i, (slot, stack)) in slots.iter().enumerate() {
        let raw = (*slot as u8) | if i < last { 0x80 } else { 0 };
        w.write_u8(raw);
        write_item_stack_network(&mut w, stack);
    }
    Ok(w.into_bytes())
}

/// 9. `ClientboundSetObjectivePacket`.
///
/// For `ObjectiveMethod::Add` (0) and `ObjectiveMethod::Change` (2), writes `display_name`
/// (defaults to empty text component if `None`), `render_type`, and optional `number_format`.
/// For `ObjectiveMethod::Remove` (1), writes only `objective_name` and method byte.
pub fn encode_play_set_objective(
    objective_name: &str,
    method: ObjectiveMethod,
    display_name: Option<&NbtTag>,
    render_type: ObjectiveRenderType,
    number_format: Option<&ScoreNumberFormat>,
) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_OBJECTIVE);
    w.write_string(objective_name);
    w.write_i8(method as i8);
    if matches!(method, ObjectiveMethod::Add | ObjectiveMethod::Change) {
        let empty = text_component("");
        w.write_nbt_network(display_name.unwrap_or(&empty))?;
        w.write_varint(render_type as i32);
        write_optional_number_format(&mut w, number_format)?;
    }
    Ok(w.into_bytes())
}

/// 10. `ClientboundSetDisplayObjectivePacket` (`DisplaySlot` VarInt + `objective_name`, `""` to clear).
pub fn encode_play_set_display_objective(slot: DisplaySlot, objective_name: &str) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_DISPLAY_OBJECTIVE);
    w.write_varint(slot as i32);
    w.write_string(objective_name);
    w.into_bytes()
}

/// 11. `ClientboundSetScorePacket`.
pub fn encode_play_set_score(
    owner: &str,
    objective_name: &str,
    score: i32,
    display: Option<&NbtTag>,
    number_format: Option<&ScoreNumberFormat>,
) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_SCORE);
    w.write_string(owner);
    w.write_string(objective_name);
    w.write_varint(score);
    match display {
        Some(tag) => {
            w.write_bool(true);
            w.write_nbt_network(tag)?;
        }
        None => w.write_bool(false),
    }
    write_optional_number_format(&mut w, number_format)?;
    Ok(w.into_bytes())
}

/// 12. `ClientboundResetScorePacket` (`owner` + nullable `objective_name`).
pub fn encode_play_reset_score(owner: &str, objective_name: Option<&str>) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::RESET_SCORE);
    w.write_string(owner);
    match objective_name {
        Some(obj) => {
            w.write_bool(true);
            w.write_string(obj);
        }
        None => w.write_bool(false),
    }
    w.into_bytes()
}

/// 13a. `ClientboundBossEventPacket` — `OperationType::ADD` (0).
#[allow(clippy::too_many_arguments)]
pub fn encode_play_boss_event_add(
    id: Uuid,
    name: &NbtTag,
    progress: f32,
    color: BossBarColor,
    overlay: BossBarOverlay,
    darken_screen: bool,
    play_music: bool,
    create_world_fog: bool,
) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::BOSS_EVENT);
    w.write_uuid(id);
    w.write_varint(0); // ADD
    w.write_nbt_network(name)?;
    w.write_f32(progress);
    w.write_varint(color as i32);
    w.write_varint(overlay as i32);
    let mut flags = 0u8;
    if darken_screen {
        flags |= 1;
    }
    if play_music {
        flags |= 2;
    }
    if create_world_fog {
        flags |= 4;
    }
    w.write_u8(flags);
    Ok(w.into_bytes())
}

/// 13b. `ClientboundBossEventPacket` — `OperationType::REMOVE` (1).
pub fn encode_play_boss_event_remove(id: Uuid) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::BOSS_EVENT);
    w.write_uuid(id);
    w.write_varint(1); // REMOVE
    w.into_bytes()
}

/// 13c. `ClientboundBossEventPacket` — `OperationType::UPDATE_PROGRESS` (2).
pub fn encode_play_boss_event_update_progress(id: Uuid, progress: f32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::BOSS_EVENT);
    w.write_uuid(id);
    w.write_varint(2); // UPDATE_PROGRESS
    w.write_f32(progress);
    w.into_bytes()
}

/// 13d. `ClientboundBossEventPacket` — `OperationType::UPDATE_NAME` (3).
pub fn encode_play_boss_event_update_name(id: Uuid, name: &NbtTag) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::BOSS_EVENT);
    w.write_uuid(id);
    w.write_varint(3); // UPDATE_NAME
    w.write_nbt_network(name)?;
    Ok(w.into_bytes())
}

/// 13e. `ClientboundBossEventPacket` — `OperationType::UPDATE_STYLE` (4).
pub fn encode_play_boss_event_update_style(
    id: Uuid,
    color: BossBarColor,
    overlay: BossBarOverlay,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::BOSS_EVENT);
    w.write_uuid(id);
    w.write_varint(4); // UPDATE_STYLE
    w.write_varint(color as i32);
    w.write_varint(overlay as i32);
    w.into_bytes()
}

/// 13f. `ClientboundBossEventPacket` — `OperationType::UPDATE_PROPERTIES` (5).
pub fn encode_play_boss_event_update_properties(
    id: Uuid,
    darken_screen: bool,
    play_music: bool,
    create_world_fog: bool,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::BOSS_EVENT);
    w.write_uuid(id);
    w.write_varint(5); // UPDATE_PROPERTIES
    let mut flags = 0u8;
    if darken_screen {
        flags |= 1;
    }
    if play_music {
        flags |= 2;
    }
    if create_world_fog {
        flags |= 4;
    }
    w.write_u8(flags);
    w.into_bytes()
}

/// 14a. `ClientboundSetTitleTextPacket`.
pub fn encode_play_set_title_text(text: &str) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_TITLE_TEXT);
    w.write_nbt_network(&text_component(text))?;
    Ok(w.into_bytes())
}

/// 14b. `ClientboundSetSubtitleTextPacket`.
pub fn encode_play_set_subtitle_text(text: &str) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_SUBTITLE_TEXT);
    w.write_nbt_network(&text_component(text))?;
    Ok(w.into_bytes())
}

/// 14c. `ClientboundSetActionBarTextPacket`.
pub fn encode_play_set_action_bar_text(text: &str) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_ACTION_BAR_TEXT);
    w.write_nbt_network(&text_component(text))?;
    Ok(w.into_bytes())
}

/// 14d. `ClientboundSetTitlesAnimationPacket` (3 × 4-byte BE `i32`).
pub fn encode_play_set_titles_animation(fade_in: i32, stay: i32, fade_out: i32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_TITLES_ANIMATION);
    w.write_i32(fade_in);
    w.write_i32(stay);
    w.write_i32(fade_out);
    w.into_bytes()
}

/// 14e. `ClientboundClearTitlesPacket`.
pub fn encode_play_clear_titles(reset_times: bool) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::CLEAR_TITLES);
    w.write_bool(reset_times);
    w.into_bytes()
}

/// 15. `ClientboundTabListPacket`.
pub fn encode_play_tab_list(header: &str, footer: &str) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::TAB_LIST);
    w.write_nbt_network(&text_component(header))?;
    w.write_nbt_network(&text_component(footer))?;
    Ok(w.into_bytes())
}

/// 16. `ClientboundSetPassengersPacket` (`vehicle: VarInt`, `passengers: VarInt[]`).
pub fn encode_play_set_passengers(vehicle_entity_id: i32, passenger_entity_ids: &[i32]) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(8 + passenger_entity_ids.len() * 4);
    w.write_varint(packet_ids::play::clientbound::SET_PASSENGERS);
    w.write_varint(vehicle_entity_id);
    w.write_varint(passenger_entity_ids.len() as i32);
    for &pid in passenger_entity_ids {
        w.write_varint(pid);
    }
    w.into_bytes()
}

/// 17. `ClientboundSoundPacket` using a 0-based `minecraft:sound_event` registry protocol ID.
///
/// Writes `VarInt(sound_id + 1)` (`SoundEvent.STREAM_CODEC` registry holder encoding),
/// `SoundSource` enum ordinal (`VarInt`), fixed-point `(int)(coord * 8.0)` × 3, `volume`,
/// `pitch`, and `seed`.
#[allow(clippy::too_many_arguments)]
pub fn encode_play_sound(
    sound_id: i32,
    source: SoundSource,
    x: f64,
    y: f64,
    z: f64,
    volume: f32,
    pitch: f32,
    seed: i64,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SOUND);
    w.write_varint(sound_id + 1);
    w.write_varint(source as i32);
    w.write_i32((x * 8.0) as i32);
    w.write_i32((y * 8.0) as i32);
    w.write_i32((z * 8.0) as i32);
    w.write_f32(volume);
    w.write_f32(pitch);
    w.write_i64(seed);
    w.into_bytes()
}

/// 18. Spawn an Experience Orb in Minecraft 26.3.
///
/// **26.3 protocol note**: `ClientboundAddExperienceOrbPacket` (`minecraft:add_experience_orb`)
/// was removed in Minecraft 26.3 and merged into `ClientboundAddEntityPacket` using entity type
/// `minecraft:experience_orb`. Sync the orb's XP amount afterwards with `encode_play_set_entity_data`
/// using `EntityDataValue::Int(8, xp_value)` (`ExperienceOrb.DATA_VALUE` at synched data index 8).
pub fn encode_play_add_experience_orb(
    entity_id: i32,
    uuid: Uuid,
    pos: Vec3,
    velocity: Vec3,
) -> Vec<u8> {
    let exp_orb_type_id =
        mc_data::static_registry_protocol_id("minecraft:entity_type", "minecraft:experience_orb")
            .expect("minecraft:experience_orb missing from entity_type registry");
    encode_play_add_entity(entity_id, uuid, exp_orb_type_id, pos, velocity, 0.0, 0.0, 0.0, 0)
}

/// 19a. `ClientboundSetBorderCenterPacket`.
pub fn encode_play_set_border_center(center_x: f64, center_z: f64) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_BORDER_CENTER);
    w.write_f64(center_x);
    w.write_f64(center_z);
    w.into_bytes()
}

/// 19b. `ClientboundSetBorderSizePacket`.
pub fn encode_play_set_border_size(size: f64) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_BORDER_SIZE);
    w.write_f64(size);
    w.into_bytes()
}

/// 19c. `ClientboundInitializeBorderPacket`.
///
/// Wire order: `center_x(f64)`, `center_z(f64)`, `old_size(f64)`, `new_size(f64)`,
/// `lerp_time(VarLong)`, `new_absolute_max_size(VarInt)`, `warning_blocks(VarInt)`,
/// `warning_time(VarInt)`.
#[allow(clippy::too_many_arguments)]
pub fn encode_play_initialize_border(
    center_x: f64,
    center_z: f64,
    old_size: f64,
    new_size: f64,
    lerp_time: i64,
    new_absolute_max_size: i32,
    warning_blocks: i32,
    warning_time: i32,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::INITIALIZE_BORDER);
    w.write_f64(center_x);
    w.write_f64(center_z);
    w.write_f64(old_size);
    w.write_f64(new_size);
    w.write_varlong(lerp_time);
    w.write_varint(new_absolute_max_size);
    w.write_varint(warning_blocks);
    w.write_varint(warning_time);
    w.into_bytes()
}

/// 20. `ClientboundMerchantOffersPacket` (Minecraft 26.3 `MerchantOffer.STREAM_CODEC`).
///
/// Returns `Err(ProtoError::InvalidValue(0))` if any offer has an empty `result` stack
/// (`ItemStack.STREAM_CODEC` forbids empty stacks).
pub fn encode_play_merchant_offers(
    container_id: i32,
    offers: &[MerchantOffer],
    villager_level: i32,
    villager_xp: i32,
    show_progress: bool,
    can_restock: bool,
) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::with_capacity(32 + offers.len() * 48);
    w.write_varint(packet_ids::play::clientbound::MERCHANT_OFFERS);
    w.write_varint(container_id);
    w.write_varint(offers.len() as i32);
    for offer in offers {
        if offer.result.is_empty() {
            return Err(ProtoError::InvalidValue(0));
        }
        offer.base_cost_a.write(&mut w);
        write_item_stack_network(&mut w, &offer.result);
        match &offer.cost_b {
            Some(cost_b) => {
                w.write_bool(true);
                cost_b.write(&mut w);
            }
            None => w.write_bool(false),
        }
        w.write_bool(offer.is_out_of_stock());
        w.write_i32(offer.uses);
        w.write_i32(offer.max_uses);
        w.write_i32(offer.xp);
        w.write_i32(offer.special_price_diff);
        w.write_f32(offer.price_multiplier);
        w.write_i32(offer.demand);
    }
    w.write_varint(villager_level);
    w.write_varint(villager_xp);
    w.write_bool(show_progress);
    w.write_bool(can_restock);
    Ok(w.into_bytes())
}

/// 21a. `ClientboundSetPlayerTeamPacket` — `METHOD_ADD` (0: create team with parameters + initial players).
pub fn encode_play_set_player_team_create(
    team_name: &str,
    params: &PlayerTeamParams,
    players: &[&str],
) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::with_capacity(64 + players.len() * 16);
    w.write_varint(packet_ids::play::clientbound::SET_PLAYER_TEAM);
    w.write_string(team_name);
    w.write_i8(0); // METHOD_ADD
    params.write(&mut w)?;
    w.write_varint(players.len() as i32);
    for p in players {
        w.write_string(p);
    }
    Ok(w.into_bytes())
}

/// 21b. `ClientboundSetPlayerTeamPacket` — `METHOD_REMOVE` (1: remove team).
pub fn encode_play_set_player_team_remove(team_name: &str) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_PLAYER_TEAM);
    w.write_string(team_name);
    w.write_i8(1); // METHOD_REMOVE
    w.into_bytes()
}

/// 21c. `ClientboundSetPlayerTeamPacket` — `METHOD_CHANGE` (2: update parameters only).
pub fn encode_play_set_player_team_modify(
    team_name: &str,
    params: &PlayerTeamParams,
) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::with_capacity(64);
    w.write_varint(packet_ids::play::clientbound::SET_PLAYER_TEAM);
    w.write_string(team_name);
    w.write_i8(2); // METHOD_CHANGE
    params.write(&mut w)?;
    Ok(w.into_bytes())
}

/// 21d. `ClientboundSetPlayerTeamPacket` — `METHOD_JOIN` (3: add entities/players to team).
pub fn encode_play_set_player_team_join(team_name: &str, players: &[&str]) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(32 + players.len() * 16);
    w.write_varint(packet_ids::play::clientbound::SET_PLAYER_TEAM);
    w.write_string(team_name);
    w.write_i8(3); // METHOD_JOIN
    w.write_varint(players.len() as i32);
    for p in players {
        w.write_string(p);
    }
    w.into_bytes()
}

/// 21e. `ClientboundSetPlayerTeamPacket` — `METHOD_LEAVE` (4: remove entities/players from team).
pub fn encode_play_set_player_team_leave(team_name: &str, players: &[&str]) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(32 + players.len() * 16);
    w.write_varint(packet_ids::play::clientbound::SET_PLAYER_TEAM);
    w.write_string(team_name);
    w.write_i8(4); // METHOD_LEAVE
    w.write_varint(players.len() as i32);
    for p in players {
        w.write_string(p);
    }
    w.into_bytes()
}

// ======================== Play Serverbound Decoders & Encoders ========================

/// Additional Minecraft 26.3 (Protocol 777) Play-phase serverbound packets.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayExtraServerbound {
    /// Decoded from `ServerboundPunchPacket` (`minecraft:punch`, ID 46, 0-byte payload).
    ///
    /// **26.3 protocol note**: `ServerboundSwingPacket` (`minecraft:swing`) was replaced in 26.3
    /// by the unit packet `ServerboundPunchPacket`, which always swings `InteractionHand::MainHand`
    /// in `ServerGamePacketListenerImpl.handlePunch`.
    Swing { hand: InteractionHand },
    /// `ServerboundPlayerCommandPacket` (`minecraft:player_command`).
    PlayerCommand {
        entity_id: i32,
        action_id: i32,
        data: i32,
    },
    /// `ServerboundRenameItemPacket` (`minecraft:rename_item`).
    RenameItem { name: String },
    /// `ServerboundContainerButtonClickPacket` (`minecraft:container_button_click`).
    ContainerButtonClick { container_id: i32, button_id: i32 },
    /// `ServerboundSelectTradePacket` (`minecraft:select_trade`).
    SelectTrade { item: i32 },
    /// `ServerboundPlayerInputPacket` (`minecraft:player_input`).
    PlayerInput { flags: u8 },
    /// `ServerboundPlayerAbilitiesPacket` (`minecraft:player_abilities`).
    PlayerAbilities { flags: u8 },
    /// `ServerboundMoveVehiclePacket` (`minecraft:move_vehicle`).
    MoveVehicle {
        x: f64,
        y: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    },
}

impl PlayExtraServerbound {
    /// Decode one of the extra Play serverbound packets from a raw decompressed packet frame
    /// (starting with the `VarInt` packet ID). Returns `Ok(None)` if the packet ID is not
    /// handled by `PlayExtraServerbound`.
    pub fn decode(bytes: &[u8]) -> Result<Option<Self>, ProtoError> {
        let mut r = PacketReader::new(bytes);
        let id = r.read_varint()?;
        if id == packet_ids::play::serverbound::PUNCH {
            Ok(Some(Self::Swing {
                hand: InteractionHand::MainHand,
            }))
        } else if id == packet_ids::play::serverbound::PLAYER_COMMAND {
            let entity_id = r.read_varint()?;
            let action_id = r.read_varint()?;
            if !(0..=6).contains(&action_id) {
                return Err(ProtoError::InvalidValue(action_id));
            }
            let data = r.read_varint()?;
            Ok(Some(Self::PlayerCommand {
                entity_id,
                action_id,
                data,
            }))
        } else if id == packet_ids::play::serverbound::RENAME_ITEM {
            let name = r.read_string(32767)?;
            Ok(Some(Self::RenameItem { name }))
        } else if id == packet_ids::play::serverbound::CONTAINER_BUTTON_CLICK {
            let container_id = r.read_varint()?;
            let button_id = r.read_varint()?;
            Ok(Some(Self::ContainerButtonClick {
                container_id,
                button_id,
            }))
        } else if id == packet_ids::play::serverbound::SELECT_TRADE {
            let item = r.read_varint()?;
            Ok(Some(Self::SelectTrade { item }))
        } else if id == packet_ids::play::serverbound::PLAYER_INPUT {
            let flags = r.read_u8()?;
            Ok(Some(Self::PlayerInput { flags }))
        } else if id == packet_ids::play::serverbound::PLAYER_ABILITIES {
            let flags = r.read_u8()?;
            Ok(Some(Self::PlayerAbilities { flags }))
        } else if id == packet_ids::play::serverbound::MOVE_VEHICLE {
            let x = r.read_f64()?;
            let y = r.read_f64()?;
            let z = r.read_f64()?;
            let yaw = r.read_f32()?;
            let pitch = r.read_f32()?;
            let on_ground = r.read_bool()?;
            Ok(Some(Self::MoveVehicle {
                x,
                y,
                z,
                yaw,
                pitch,
                on_ground,
            }))
        } else {
            Ok(None)
        }
    }
}

/// 22. Encode `ServerboundPunchPacket` (`minecraft:punch`, 26.3 replacement for `ServerboundSwingPacket`).
pub fn encode_serverbound_swing() -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::PUNCH);
    w.into_bytes()
}

/// Alias for `encode_serverbound_swing` matching the exact 26.3 packet name (`minecraft:punch`).
pub fn encode_serverbound_punch() -> Vec<u8> {
    encode_serverbound_swing()
}

/// 23. Encode `ServerboundPlayerCommandPacket`.
pub fn encode_serverbound_player_command(entity_id: i32, action_id: i32, data: i32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::PLAYER_COMMAND);
    w.write_varint(entity_id);
    w.write_varint(action_id);
    w.write_varint(data);
    w.into_bytes()
}

/// 24. Encode `ServerboundRenameItemPacket`.
pub fn encode_serverbound_rename_item(name: &str) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::RENAME_ITEM);
    w.write_string(name);
    w.into_bytes()
}

/// 25. Encode `ServerboundContainerButtonClickPacket`.
pub fn encode_serverbound_container_button_click(container_id: i32, button_id: i32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::CONTAINER_BUTTON_CLICK);
    w.write_varint(container_id);
    w.write_varint(button_id);
    w.into_bytes()
}

/// 26. Encode `ServerboundSelectTradePacket`.
pub fn encode_serverbound_select_trade(item: i32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::SELECT_TRADE);
    w.write_varint(item);
    w.into_bytes()
}

/// 27. Encode `ServerboundPlayerInputPacket`.
pub fn encode_serverbound_player_input(flags: u8) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::PLAYER_INPUT);
    w.write_u8(flags);
    w.into_bytes()
}

/// 28. Encode `ServerboundPlayerAbilitiesPacket`.
pub fn encode_serverbound_player_abilities(flags: u8) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::PLAYER_ABILITIES);
    w.write_u8(flags);
    w.into_bytes()
}

/// 29. Encode `ServerboundMoveVehiclePacket`.
pub fn encode_serverbound_move_vehicle(
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
    on_ground: bool,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::MOVE_VEHICLE);
    w.write_f64(x);
    w.write_f64(y);
    w.write_f64(z);
    w.write_f32(yaw);
    w.write_f32(pitch);
    w.write_bool(on_ground);
    w.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_oracle::with_oracle;

    fn assert_oracle_roundtrip(phase: &str, dir: &str, expected_type: &str, bytes: &[u8]) {
        let (ptype, _jclass, reencoded) =
            with_oracle(|o| o.packet_roundtrip(phase, dir, bytes)).unwrap_or_else(|e| {
                panic!("Oracle failed decoding {phase}/{dir} {expected_type}: {e}")
            });
        assert_eq!(ptype, expected_type, "Packet type mismatch");
        assert_eq!(
            reencoded, bytes,
            "Wire bytes mismatch after Java roundtrip for {phase}/{dir} {expected_type}"
        );
    }

    #[test]
    fn diff_all_extra_protocol_777_packets_against_java_oracle() {
        // 1. ClientboundExplodePacket (with knockback, block particles, and 26.3 playSound=true)
        let explosion_emitter_id = mc_data::static_registry_protocol_id(
            "minecraft:particle_type",
            "minecraft:explosion_emitter",
        )
        .unwrap();
        let poof_id =
            mc_data::static_registry_protocol_id("minecraft:particle_type", "minecraft:poof")
                .unwrap();
        let explode_sound_id = mc_data::static_registry_protocol_id(
            "minecraft:sound_event",
            "minecraft:entity.generic.explode",
        )
        .unwrap();
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:explode",
            &encode_play_explode(
                Vec3::new(10.5, 64.0, -22.25),
                4.0,
                19,
                Some(Vec3::new(0.25, 0.75, -0.5)),
                explosion_emitter_id,
                explode_sound_id,
                &[ExplosionParticleEntry {
                    particle_id: poof_id,
                    scaling: 1.0,
                    speed: 0.5,
                    weight: 3,
                }],
                true,
            ),
        );
        // Also verify None knockback, empty blockParticles, playSound=false
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:explode",
            &encode_play_explode(
                Vec3::new(0.0, 65.0, 0.0),
                2.5,
                0,
                None,
                explosion_emitter_id,
                explode_sound_id,
                &[],
                false,
            ),
        );

        // 2. ClientboundAnimatePacket (+ companion 26.3 ClientboundSwingAnimationPacket)
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:animate",
            &encode_play_animate(42, animate_action::CRITICAL_HIT),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:swing_animation",
            &encode_play_swing_animation(42, InteractionHand::MainHand, SwingAnimationType::Whack, 6),
        );

        // 3. ClientboundChangeDifficultyPacket
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:change_difficulty",
            &encode_play_change_difficulty(Difficulty::Hard, true),
        );

        // 4. ClientboundRotateHeadPacket
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:rotate_head",
            &encode_play_rotate_head(77, -135.0),
        );

        // 5, 6, 7. ClientboundMoveEntityPacket (.Pos, .Pos stepped, .PosRot, .Rot)
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:move_entity_pos",
            &encode_play_move_entity_pos(77, 128, -64, 256, true),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:move_entity_pos",
            &encode_play_move_entity_pos_stepped(
                77,
                &[
                    VecDeltaStep {
                        dx: 32,
                        dy: 0,
                        dz: -16,
                        ticks: 1,
                    },
                    VecDeltaStep {
                        dx: 48,
                        dy: 16,
                        dz: -32,
                        ticks: 2,
                    },
                ],
                false,
            ),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:move_entity_pos_rot",
            &encode_play_move_entity_pos_rot(77, 512, 0, -256, 90.0, -15.0, true),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:move_entity_rot",
            &encode_play_move_entity_rot(77, 180.0, 30.0, false),
        );

        // 8. ClientboundSetEquipmentPacket (multi-slot + offhand/armor/body/saddle ordinal check)
        let sword = ItemStack::from_name("minecraft:diamond_sword", 1).unwrap();
        let shield = ItemStack::from_name("minecraft:shield", 1).unwrap();
        let helm = ItemStack::from_name("minecraft:iron_helmet", 1).unwrap();
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_equipment",
            &encode_play_set_equipment(
                77,
                &[
                    (EquipmentSlot::MainHand, sword),
                    (EquipmentSlot::OffHand, shield),
                    (EquipmentSlot::Head, helm),
                    (EquipmentSlot::Chest, ItemStack::EMPTY),
                ],
            )
            .unwrap(),
        );

        // 9. ClientboundSetObjectivePacket (ADD with Fixed format, CHANGE with Styled, REMOVE)
        let obj_title = text_component("SiliconMC Kills");
        let fixed_fmt = ScoreNumberFormat::Fixed(text_component("99+"));
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_objective",
            &encode_play_set_objective(
                "kills",
                ObjectiveMethod::Add,
                Some(&obj_title),
                ObjectiveRenderType::Integer,
                Some(&fixed_fmt),
            )
            .unwrap(),
        );
        let mut style_nbt = NbtCompound::new();
        style_nbt.insert("color", NbtTag::String("gold".into()));
        let styled_fmt = ScoreNumberFormat::Styled(style_nbt);
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_objective",
            &encode_play_set_objective(
                "kills",
                ObjectiveMethod::Change,
                Some(&obj_title),
                ObjectiveRenderType::Hearts,
                Some(&styled_fmt),
            )
            .unwrap(),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_objective",
            &encode_play_set_objective(
                "kills",
                ObjectiveMethod::Remove,
                None,
                ObjectiveRenderType::Integer,
                None,
            )
            .unwrap(),
        );

        // 10. ClientboundSetDisplayObjectivePacket
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_display_objective",
            &encode_play_set_display_objective(DisplaySlot::Sidebar, "kills"),
        );

        // 11. ClientboundSetScorePacket (with display + Blank number format, and with None/None)
        let custom_display = text_component("Steve [VIP]");
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_score",
            &encode_play_set_score(
                "Steve",
                "kills",
                42,
                Some(&custom_display),
                Some(&ScoreNumberFormat::Blank),
            )
            .unwrap(),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_score",
            &encode_play_set_score("Alex", "kills", 7, None, None).unwrap(),
        );

        // 12. ClientboundResetScorePacket (with and without objective)
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:reset_score",
            &encode_play_reset_score("Steve", Some("kills")),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:reset_score",
            &encode_play_reset_score("Steve", None),
        );

        // 13. ClientboundBossEventPacket (ADD, UPDATE_PROGRESS, UPDATE_NAME, UPDATE_STYLE, UPDATE_PROPERTIES, REMOVE)
        let boss_id = Uuid::from_u128(0x11223344_5566_4777_8899_aabbccddeeff);
        let boss_name = text_component("Ender Dragon");
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:boss_event",
            &encode_play_boss_event_add(
                boss_id,
                &boss_name,
                0.75,
                BossBarColor::Purple,
                BossBarOverlay::Notched10,
                true,
                true,
                false,
            )
            .unwrap(),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:boss_event",
            &encode_play_boss_event_update_progress(boss_id, 0.5),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:boss_event",
            &encode_play_boss_event_update_name(boss_id, &text_component("Wither")).unwrap(),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:boss_event",
            &encode_play_boss_event_update_style(
                boss_id,
                BossBarColor::Red,
                BossBarOverlay::Notched20,
            ),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:boss_event",
            &encode_play_boss_event_update_properties(boss_id, false, false, true),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:boss_event",
            &encode_play_boss_event_remove(boss_id),
        );

        // 14. Titles (title, subtitle, action bar, animation, clear)
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_title_text",
            &encode_play_set_title_text("Victory!").unwrap(),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_subtitle_text",
            &encode_play_set_subtitle_text("SiliconMC 26.3").unwrap(),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_action_bar_text",
            &encode_play_set_action_bar_text("XP +15").unwrap(),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_titles_animation",
            &encode_play_set_titles_animation(10, 70, 20),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:clear_titles",
            &encode_play_clear_titles(true),
        );

        // 15. ClientboundTabListPacket
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:tab_list",
            &encode_play_tab_list("Welcome to SiliconMC", "TPS: 20.0").unwrap(),
        );

        // 16. ClientboundSetPassengersPacket
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_passengers",
            &encode_play_set_passengers(100, &[1, 2, 3]),
        );

        // 17. ClientboundSoundPacket (using registry sound ID)
        let click_sound_id = mc_data::static_registry_protocol_id(
            "minecraft:sound_event",
            "minecraft:ui.button.click",
        )
        .unwrap();
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:sound",
            &encode_play_sound(
                click_sound_id,
                SoundSource::Ui,
                12.5,
                64.0,
                -8.25,
                1.0,
                1.2,
                0x0123_4567_89ab_cdef,
            ),
        );

        // 18. Experience Orb (merged into ClientboundAddEntityPacket in 26.3)
        let orb_uuid = Uuid::from_u128(0xabcdef01_2345_4678_9abc_def012345678);
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:add_entity",
            &encode_play_add_experience_orb(
                505,
                orb_uuid,
                Vec3::new(3.5, 65.0, -4.5),
                Vec3::new(0.05, 0.1, -0.05),
            ),
        );

        // 19. World Border packets
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_border_center",
            &encode_play_set_border_center(100.5, -250.25),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_border_size",
            &encode_play_set_border_size(1000.0),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:initialize_border",
            &encode_play_initialize_border(0.0, 0.0, 59999968.0, 10000.0, 60000, 29999984, 5, 15),
        );

        // 20. ClientboundMerchantOffersPacket
        let emerald_id =
            mc_data::static_registry_protocol_id("minecraft:item", "minecraft:emerald").unwrap();
        let paper_id =
            mc_data::static_registry_protocol_id("minecraft:item", "minecraft:paper").unwrap();
        let bread_stack = ItemStack::from_name("minecraft:bread", 6).unwrap();
        let book_stack = ItemStack::from_name("minecraft:book", 1).unwrap();
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:merchant_offers",
            &encode_play_merchant_offers(
                3,
                &[
                    MerchantOffer {
                        base_cost_a: ItemCost::simple(emerald_id, 1),
                        result: bread_stack,
                        cost_b: None,
                        uses: 2,
                        max_uses: 16,
                        xp: 5,
                        special_price_diff: -1,
                        price_multiplier: 0.05,
                        demand: 0,
                    },
                    MerchantOffer {
                        base_cost_a: ItemCost::simple(emerald_id, 4),
                        result: book_stack,
                        cost_b: Some(ItemCost::simple(paper_id, 24)),
                        uses: 12,
                        max_uses: 12,
                        xp: 10,
                        special_price_diff: 0,
                        price_multiplier: 0.2,
                        demand: 4,
                    },
                ],
                2,
                35,
                true,
                true,
            )
            .unwrap(),
        );

        // 21. ClientboundSetPlayerTeamPacket (create, modify, join, leave, remove)
        let team_params = PlayerTeamParams {
            display_name: text_component("Red Team"),
            player_prefix: text_component("[RED] "),
            player_suffix: text_component(""),
            name_tag_visibility: TeamVisibility::HideForOtherTeams,
            collision_rule: TeamCollisionRule::PushOwnTeam,
            color: Some(TeamColor::Red),
            options: 0x03, // friendly fire + see friendly invisibles
        };
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_player_team",
            &encode_play_set_player_team_create("red", &team_params, &["Steve", "Alex"]).unwrap(),
        );
        let team_params_no_color = PlayerTeamParams {
            color: None,
            options: 0x00,
            ..team_params
        };
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_player_team",
            &encode_play_set_player_team_modify("red", &team_params_no_color).unwrap(),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_player_team",
            &encode_play_set_player_team_join("red", &["Notch"]),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_player_team",
            &encode_play_set_player_team_leave("red", &["Alex"]),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_player_team",
            &encode_play_set_player_team_remove("red"),
        );

        // ======================== Serverbound Packets (22–29) ========================

        // 22. Swing / Punch (ServerboundPunchPacket in 26.3)
        let punch = encode_serverbound_swing();
        assert_oracle_roundtrip("play", "serverbound", "minecraft:punch", &punch);
        assert_eq!(
            PlayExtraServerbound::decode(&punch).unwrap(),
            Some(PlayExtraServerbound::Swing {
                hand: InteractionHand::MainHand
            })
        );

        // 23. ServerboundPlayerCommandPacket
        let pcmd = encode_serverbound_player_command(1, player_command_action::START_SPRINTING, 0);
        assert_oracle_roundtrip("play", "serverbound", "minecraft:player_command", &pcmd);
        assert_eq!(
            PlayExtraServerbound::decode(&pcmd).unwrap(),
            Some(PlayExtraServerbound::PlayerCommand {
                entity_id: 1,
                action_id: player_command_action::START_SPRINTING,
                data: 0,
            })
        );

        // 24. ServerboundRenameItemPacket
        let rename = encode_serverbound_rename_item("Excalibur");
        assert_oracle_roundtrip("play", "serverbound", "minecraft:rename_item", &rename);
        assert_eq!(
            PlayExtraServerbound::decode(&rename).unwrap(),
            Some(PlayExtraServerbound::RenameItem {
                name: "Excalibur".into()
            })
        );

        // 25. ServerboundContainerButtonClickPacket
        let btn = encode_serverbound_container_button_click(2, 1);
        assert_oracle_roundtrip(
            "play",
            "serverbound",
            "minecraft:container_button_click",
            &btn,
        );
        assert_eq!(
            PlayExtraServerbound::decode(&btn).unwrap(),
            Some(PlayExtraServerbound::ContainerButtonClick {
                container_id: 2,
                button_id: 1,
            })
        );

        // 26. ServerboundSelectTradePacket
        let trade = encode_serverbound_select_trade(3);
        assert_oracle_roundtrip("play", "serverbound", "minecraft:select_trade", &trade);
        assert_eq!(
            PlayExtraServerbound::decode(&trade).unwrap(),
            Some(PlayExtraServerbound::SelectTrade { item: 3 })
        );

        // 27. ServerboundPlayerInputPacket
        let input_flags =
            player_input_flags::FORWARD | player_input_flags::JUMP | player_input_flags::SPRINT;
        let pinput = encode_serverbound_player_input(input_flags);
        assert_oracle_roundtrip("play", "serverbound", "minecraft:player_input", &pinput);
        assert_eq!(
            PlayExtraServerbound::decode(&pinput).unwrap(),
            Some(PlayExtraServerbound::PlayerInput { flags: input_flags })
        );

        // 28. ServerboundPlayerAbilitiesPacket (bit 1 = isFlying)
        let pabil = encode_serverbound_player_abilities(0x02);
        assert_oracle_roundtrip("play", "serverbound", "minecraft:player_abilities", &pabil);
        assert_eq!(
            PlayExtraServerbound::decode(&pabil).unwrap(),
            Some(PlayExtraServerbound::PlayerAbilities { flags: 0x02 })
        );

        // 29. ServerboundMoveVehiclePacket
        let mveh = encode_serverbound_move_vehicle(12.5, 64.0, -30.75, 45.0, -5.5, true);
        assert_oracle_roundtrip("play", "serverbound", "minecraft:move_vehicle", &mveh);
        assert_eq!(
            PlayExtraServerbound::decode(&mveh).unwrap(),
            Some(PlayExtraServerbound::MoveVehicle {
                x: 12.5,
                y: 64.0,
                z: -30.75,
                yaw: 45.0,
                pitch: -5.5,
                on_ground: true,
            })
        );

        // Unhandled packet ID should cleanly return Ok(None)
        let mut unknown = PacketWriter::new();
        unknown.write_varint(packet_ids::play::serverbound::KEEP_ALIVE);
        unknown.write_i64(123);
        assert_eq!(
            PlayExtraServerbound::decode(unknown.as_slice()).unwrap(),
            None
        );
    }
}
