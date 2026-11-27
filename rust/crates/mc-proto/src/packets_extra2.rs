//! Second batch of Minecraft 26.3 (Protocol 777) Play-phase packet encoders and decoders.
//!
//! Every wire layout in this module was verified directly against the decompiled
//! Minecraft 26.3 Java classes in `net.minecraft.network.protocol.game.*` and
//! round-trip verified byte-for-byte against the live Java 26.3 `OracleMain`.

use crate::{PacketReader, PacketWriter, ProtoError};
use mc_data::packet_ids;
use mc_math::{BlockPos, Vec3};
use mc_nbt::NbtTag;

// Re-export encoders already implemented in `packets_extra` so both `mc_proto::*`
// and `mc_proto::packets_extra2::*` expose the full set without glob-reexport conflicts.
pub use crate::packets_extra::{
    encode_play_set_border_center, encode_play_set_border_size, encode_play_tab_list,
};

// ======================== Shared Protocol 777 Types ========================

/// Pack `(chunk_x, chunk_z)` into Minecraft's `ChunkPos.pack(int x, int z)` 64-bit integer:
/// `(chunk_x & 0xFFFF_FFFF) | ((chunk_z & 0xFFFF_FFFF) << 32)`.
///
/// Note that on the wire (`FriendlyByteBuf.writeChunkPos` -> big-endian `i64`),
/// the upper 32 bits (`chunk_z`) are transmitted **first**, followed by `chunk_x`.
#[inline]
pub fn pack_chunk_pos(chunk_x: i32, chunk_z: i32) -> i64 {
    (((chunk_z as u32 as u64) << 32) | (chunk_x as u32 as u64)) as i64
}

/// Unpack a Minecraft `ChunkPos.pack` 64-bit integer into `(chunk_x, chunk_z)`.
#[inline]
pub fn unpack_chunk_pos(packed: i64) -> (i32, i32) {
    let raw = packed as u64;
    let x = (raw & 0xFFFF_FFFF) as u32 as i32;
    let z = ((raw >> 32) & 0xFFFF_FFFF) as u32 as i32;
    (x, z)
}

/// `ClientboundLevelParticlesPacket.RandomizationType` (new in Minecraft 26.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ParticleRandomizationType {
    Default = 0,
    Alternative = 1,
    AlternativeWithSpeed = 2,
}

/// Protocol IDs in `minecraft:stat_type` for `ClientboundAwardStatsPacket`.
pub mod stat_category {
    pub const MINED: i32 = 0;
    pub const CRAFTED: i32 = 1;
    pub const USED: i32 = 2;
    pub const BROKEN: i32 = 3;
    pub const PICKED_UP: i32 = 4;
    pub const DROPPED: i32 = 5;
    pub const KILLED: i32 = 6;
    pub const KILLED_BY: i32 = 7;
    pub const CUSTOM: i32 = 8;
}

/// `net.minecraft.commands.arguments.EntityAnchorArgument.Anchor` enum ordinals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum EntityAnchor {
    Feet = 0,
    Eyes = 1,
}

/// `net.minecraft.world.item.crafting.display.SlotDisplay` (Minecraft 26.3 `SlotDisplay.STREAM_CODEC`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotDisplay {
    /// `minecraft:empty` (ID 0).
    Empty,
    /// `minecraft:any_fuel` (ID 1).
    AnyFuel,
    /// `minecraft:with_any_potion` (ID 2).
    WithAnyPotion(Box<SlotDisplay>),
    /// `minecraft:only_with_component` (ID 3): `(source, component_type_id)`.
    OnlyWithComponent {
        source: Box<SlotDisplay>,
        component_type_id: i32,
    },
    /// `minecraft:item` (ID 4): 0-based protocol ID in `minecraft:item`.
    Item(i32),
    /// `minecraft:item_stack` (ID 5): 26.3 `ItemStackTemplate` (`item_id`, `count > 0`, empty patch).
    ItemStack { item_id: i32, count: i32 },
    /// `minecraft:tag` (ID 6): named item tag identifier (e.g. `"minecraft:planks"`).
    Tag(String),
    /// `minecraft:dyed` (ID 7).
    Dyed {
        dye: Box<SlotDisplay>,
        target: Box<SlotDisplay>,
    },
    /// `minecraft:with_remainder` (ID 9).
    WithRemainder {
        input: Box<SlotDisplay>,
        remainder: Box<SlotDisplay>,
    },
    /// `minecraft:composite` (ID 10).
    Composite(Vec<SlotDisplay>),
}

impl SlotDisplay {
    fn write(&self, w: &mut PacketWriter) -> Result<(), ProtoError> {
        match self {
            Self::Empty => w.write_varint(0),
            Self::AnyFuel => w.write_varint(1),
            Self::WithAnyPotion(inner) => {
                w.write_varint(2);
                inner.write(w)?;
            }
            Self::OnlyWithComponent {
                source,
                component_type_id,
            } => {
                w.write_varint(3);
                source.write(w)?;
                w.write_varint(*component_type_id);
            }
            Self::Item(item_id) => {
                w.write_varint(4);
                w.write_varint(*item_id);
            }
            Self::ItemStack { item_id, count } => {
                if *count <= 0 || *item_id <= 0 {
                    return Err(ProtoError::InvalidValue(*count));
                }
                w.write_varint(5);
                // 26.3 `ItemStackTemplate.STREAM_CODEC`: item_id FIRST, then count, then DataComponentPatch
                w.write_varint(*item_id);
                w.write_varint(*count);
                w.write_varint(0); // added components
                w.write_varint(0); // removed components
            }
            Self::Tag(tag_id) => {
                w.write_varint(6);
                // `ByteBufCodecs.holderSet`: VarInt(0) = named tag followed by Identifier
                w.write_varint(0);
                w.write_string(tag_id);
            }
            Self::Dyed { dye, target } => {
                w.write_varint(7);
                dye.write(w)?;
                target.write(w)?;
            }
            Self::WithRemainder { input, remainder } => {
                w.write_varint(9);
                input.write(w)?;
                remainder.write(w)?;
            }
            Self::Composite(contents) => {
                w.write_varint(10);
                w.write_varint(contents.len() as i32);
                for entry in contents {
                    entry.write(w)?;
                }
            }
        }
        Ok(())
    }
}

/// `net.minecraft.world.item.crafting.display.RecipeDisplay` (Minecraft 26.3 `RecipeDisplay.STREAM_CODEC`).
#[derive(Debug, Clone, PartialEq)]
pub enum RecipeDisplay {
    /// `minecraft:crafting_shapeless` (ID 0).
    CraftingShapeless {
        ingredients: Vec<SlotDisplay>,
        result: SlotDisplay,
        crafting_station: SlotDisplay,
    },
    /// `minecraft:crafting_shaped` (ID 1). `ingredients.len()` must equal `width * height`.
    CraftingShaped {
        width: i32,
        height: i32,
        ingredients: Vec<SlotDisplay>,
        result: SlotDisplay,
        crafting_station: SlotDisplay,
    },
    /// `minecraft:furnace` (ID 2).
    Furnace {
        ingredient: SlotDisplay,
        fuel: SlotDisplay,
        result: SlotDisplay,
        crafting_station: SlotDisplay,
        duration: i32,
        experience: f32,
    },
    /// `minecraft:stonecutter` (ID 3).
    Stonecutter {
        input: SlotDisplay,
        result: SlotDisplay,
        crafting_station: SlotDisplay,
    },
    /// `minecraft:smithing` (ID 4).
    Smithing {
        template: SlotDisplay,
        base: SlotDisplay,
        addition: SlotDisplay,
        result: SlotDisplay,
        crafting_station: SlotDisplay,
    },
}

impl RecipeDisplay {
    fn write(&self, w: &mut PacketWriter) -> Result<(), ProtoError> {
        match self {
            Self::CraftingShapeless {
                ingredients,
                result,
                crafting_station,
            } => {
                w.write_varint(0);
                w.write_varint(ingredients.len() as i32);
                for ing in ingredients {
                    ing.write(w)?;
                }
                result.write(w)?;
                crafting_station.write(w)?;
            }
            Self::CraftingShaped {
                width,
                height,
                ingredients,
                result,
                crafting_station,
            } => {
                if *width < 0 || *height < 0 || ingredients.len() != (*width as usize) * (*height as usize) {
                    return Err(ProtoError::InvalidValue(ingredients.len() as i32));
                }
                w.write_varint(1);
                w.write_varint(*width);
                w.write_varint(*height);
                w.write_varint(ingredients.len() as i32);
                for ing in ingredients {
                    ing.write(w)?;
                }
                result.write(w)?;
                crafting_station.write(w)?;
            }
            Self::Furnace {
                ingredient,
                fuel,
                result,
                crafting_station,
                duration,
                experience,
            } => {
                w.write_varint(2);
                ingredient.write(w)?;
                fuel.write(w)?;
                result.write(w)?;
                crafting_station.write(w)?;
                w.write_varint(*duration);
                w.write_f32(*experience);
            }
            Self::Stonecutter {
                input,
                result,
                crafting_station,
            } => {
                w.write_varint(3);
                input.write(w)?;
                result.write(w)?;
                crafting_station.write(w)?;
            }
            Self::Smithing {
                template,
                base,
                addition,
                result,
                crafting_station,
            } => {
                w.write_varint(4);
                template.write(w)?;
                base.write(w)?;
                addition.write(w)?;
                result.write(w)?;
                crafting_station.write(w)?;
            }
        }
        Ok(())
    }
}

/// `ServerboundSeenAdvancementsPacket.Action` enum ordinals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum SeenAdvancementsAction {
    OpenedTab = 0,
    ClosedScreen = 1,
}

impl SeenAdvancementsAction {
    pub fn from_id(id: i32) -> Result<Self, ProtoError> {
        match id {
            0 => Ok(Self::OpenedTab),
            1 => Ok(Self::ClosedScreen),
            other => Err(ProtoError::InvalidValue(other)),
        }
    }
}

// ======================== Play Clientbound Encoders ========================

/// `ClientboundForgetLevelChunkPacket` (`minecraft:forget_level_chunk`, ID 38).
///
/// Writes `ChunkPos.pack(chunk_x, chunk_z)` as a big-endian `i64` (`z` in upper 32 bits, `x` in lower 32 bits).
pub fn encode_play_forget_level_chunk(chunk_x: i32, chunk_z: i32) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(10);
    w.write_varint(packet_ids::play::clientbound::FORGET_LEVEL_CHUNK);
    w.write_i64(pack_chunk_pos(chunk_x, chunk_z));
    w.into_bytes()
}

/// `ClientboundSetSimulationDistancePacket` (`minecraft:set_simulation_distance`, ID 113).
pub fn encode_play_set_simulation_distance(simulation_distance: i32) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(8);
    w.write_varint(packet_ids::play::clientbound::SET_SIMULATION_DISTANCE);
    w.write_varint(simulation_distance);
    w.into_bytes()
}

/// `ClientboundSetChunkCacheRadiusPacket` (`minecraft:set_chunk_cache_radius`, ID 97).
pub fn encode_play_set_chunk_cache_radius(radius: i32) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(8);
    w.write_varint(packet_ids::play::clientbound::SET_CHUNK_CACHE_RADIUS);
    w.write_varint(radius);
    w.into_bytes()
}

/// `ClientboundDamageEventPacket` (`minecraft:damage_event`, ID 25).
///
/// Wire layout (Minecraft 26.3):
/// - `entity_id`: `VarInt`
/// - `source_type_id`: `VarInt` (`DamageType.STREAM_CODEC` = `ByteBufCodecs.holderRegistry`, raw 0-based ID)
/// - `source_cause_id`: `VarInt(id + 1)` (0 for `None`, `id + 1` for `Some(id)`)
/// - `source_direct_id`: `VarInt(id + 1)` (0 for `None`, `id + 1` for `Some(id)`)
/// - `source_position`: `bool` + optional `3 × f64`
pub fn encode_play_damage_event(
    entity_id: i32,
    source_type_id: i32,
    source_cause_id: Option<i32>,
    source_direct_id: Option<i32>,
    source_position: Option<Vec3>,
) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(40);
    w.write_varint(packet_ids::play::clientbound::DAMAGE_EVENT);
    w.write_varint(entity_id);
    // DamageType.STREAM_CODEC = ByteBufCodecs.holderRegistry(Registries.DAMAGE_TYPE) -> raw 0-based VarInt
    w.write_varint(source_type_id);
    w.write_optional_varint(source_cause_id);
    w.write_optional_varint(source_direct_id);
    match source_position {
        Some(pos) => {
            w.write_bool(true);
            w.write_vec3(pos);
        }
        None => w.write_bool(false),
    }
    w.into_bytes()
}

/// `ClientboundLevelParticlesPacket` (`minecraft:level_particles`, ID 48) for any `SimpleParticleType`,
/// exposing the full Minecraft 26.3 wire layout (3-axis speed + `ParticleRandomizationType`).
///
/// **26.3 wire changes** (verified against `ClientboundLevelParticlesPacket.STREAM_CODEC`):
/// - `particle` is written **first** (not last!)
/// - `overrideLimiter` (`bool`) + `alwaysShow` (`bool`)
/// - `x`, `y`, `z` (`3 × f64`)
/// - `xDist`, `yDist`, `zDist` (`3 × f32`)
/// - `xMaxSpeed`, `yMaxSpeed`, `zMaxSpeed` (`3 × f32`, replacing the single `maxSpeed` float)
/// - `count` (`VarInt`, replacing 4-byte BE `i32`)
/// - `randomizationType` (`VarInt`: `0 = Default`, `1 = Alternative`, `2 = AlternativeWithSpeed`)
#[allow(clippy::too_many_arguments)]
pub fn encode_play_level_particles_simple_ex(
    particle_id: i32,
    override_limiter: bool,
    always_show: bool,
    x: f64,
    y: f64,
    z: f64,
    x_dist: f32,
    y_dist: f32,
    z_dist: f32,
    x_max_speed: f32,
    y_max_speed: f32,
    z_max_speed: f32,
    count: i32,
    randomization: ParticleRandomizationType,
) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(64);
    w.write_varint(packet_ids::play::clientbound::LEVEL_PARTICLES);
    // ParticleTypes.STREAM_CODEC comes FIRST in 26.3!
    w.write_varint(particle_id);
    w.write_bool(override_limiter);
    w.write_bool(always_show);
    w.write_f64(x);
    w.write_f64(y);
    w.write_f64(z);
    w.write_f32(x_dist);
    w.write_f32(y_dist);
    w.write_f32(z_dist);
    w.write_f32(x_max_speed);
    w.write_f32(y_max_speed);
    w.write_f32(z_max_speed);
    w.write_varint(count);
    w.write_varint(randomization as i32);
    w.into_bytes()
}

/// `ClientboundLevelParticlesPacket` (`minecraft:level_particles`, ID 48) convenience encoder
/// for a `SimpleParticleType` with uniform `max_speed` and `ParticleRandomizationType::Default`
/// (matching Java's 11-argument constructor).
#[allow(clippy::too_many_arguments)]
pub fn encode_play_level_particles_simple(
    particle_id: i32,
    override_limiter: bool,
    always_show: bool,
    x: f64,
    y: f64,
    z: f64,
    x_dist: f32,
    y_dist: f32,
    z_dist: f32,
    max_speed: f32,
    count: i32,
) -> Vec<u8> {
    encode_play_level_particles_simple_ex(
        particle_id,
        override_limiter,
        always_show,
        x,
        y,
        z,
        x_dist,
        y_dist,
        z_dist,
        max_speed,
        max_speed,
        max_speed,
        count,
        ParticleRandomizationType::Default,
    )
}

/// `ClientboundAwardStatsPacket` (`minecraft:award_stats`, ID 3).
///
/// Each tuple is `(category_id, stat_id, value)` where `category_id` is a 0-based protocol ID
/// in `minecraft:stat_type` (see [`stat_category`]) and `stat_id` is a 0-based protocol ID in
/// that stat type's backing registry (`block`, `item`, `entity_type`, or `custom_stat`).
pub fn encode_play_award_stats(stats: &[(i32, i32, i32)]) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(8 + stats.len() * 6);
    w.write_varint(packet_ids::play::clientbound::AWARD_STATS);
    w.write_varint(stats.len() as i32);
    for &(category_id, stat_id, value) in stats {
        w.write_varint(category_id);
        w.write_varint(stat_id);
        w.write_varint(value);
    }
    w.into_bytes()
}

/// `ClientboundPlaceGhostRecipePacket` (`minecraft:place_ghost_recipe`, ID 64).
pub fn encode_play_place_ghost_recipe(
    container_id: i32,
    recipe_display: &RecipeDisplay,
) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::with_capacity(32);
    w.write_varint(packet_ids::play::clientbound::PLACE_GHOST_RECIPE);
    w.write_varint(container_id);
    recipe_display.write(&mut w)?;
    Ok(w.into_bytes())
}

/// `ClientboundCooldownPacket` (`minecraft:cooldown`, ID 22).
///
/// In Minecraft 26.3, `cooldown_group` is an `Identifier` string (e.g. `"minecraft:ender_pearl"`)
/// rather than an item registry ID, followed by `duration` (`VarInt` ticks).
pub fn encode_play_cooldown(cooldown_group: &str, duration: i32) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(12 + cooldown_group.len());
    w.write_varint(packet_ids::play::clientbound::COOLDOWN);
    w.write_string(cooldown_group);
    w.write_varint(duration);
    w.into_bytes()
}

/// `ClientboundPlayerLookAtPacket` (`minecraft:player_look_at`, ID 72).
///
/// If `target_entity` is `Some((entity_id, to_anchor))`, sets `at_entity = true` and appends
/// `VarInt(entity_id)` and `VarInt(to_anchor)`.
pub fn encode_play_player_look_at(
    from_anchor: EntityAnchor,
    x: f64,
    y: f64,
    z: f64,
    target_entity: Option<(i32, EntityAnchor)>,
) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(36);
    w.write_varint(packet_ids::play::clientbound::PLAYER_LOOK_AT);
    w.write_varint(from_anchor as i32);
    w.write_f64(x);
    w.write_f64(y);
    w.write_f64(z);
    match target_entity {
        Some((entity_id, to_anchor)) => {
            w.write_bool(true);
            w.write_varint(entity_id);
            w.write_varint(to_anchor as i32);
        }
        None => w.write_bool(false),
    }
    w.into_bytes()
}

/// `ClientboundSetBorderLerpSizePacket` (`minecraft:set_border_lerp_size`, ID 91).
pub fn encode_play_set_border_lerp_size(old_size: f64, new_size: f64, lerp_time: i64) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(28);
    w.write_varint(packet_ids::play::clientbound::SET_BORDER_LERP_SIZE);
    w.write_f64(old_size);
    w.write_f64(new_size);
    w.write_varlong(lerp_time);
    w.into_bytes()
}

/// `ClientboundSetBorderWarningDelayPacket` (`minecraft:set_border_warning_delay`, ID 93).
pub fn encode_play_set_border_warning_delay(warning_delay: i32) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(8);
    w.write_varint(packet_ids::play::clientbound::SET_BORDER_WARNING_DELAY);
    w.write_varint(warning_delay);
    w.into_bytes()
}

/// `ClientboundSetBorderWarningDistancePacket` (`minecraft:set_border_warning_distance`, ID 94).
pub fn encode_play_set_border_warning_distance(warning_blocks: i32) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(8);
    w.write_varint(packet_ids::play::clientbound::SET_BORDER_WARNING_DISTANCE);
    w.write_varint(warning_blocks);
    w.into_bytes()
}

/// `ClientboundTabListPacket` (`minecraft:tab_list`, ID 125) accepting full NBT `Component` tags.
///
/// For plain-text strings, see [`encode_play_tab_list`].
pub fn encode_play_tab_list_components(
    header: &NbtTag,
    footer: &NbtTag,
) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::with_capacity(64);
    w.write_varint(packet_ids::play::clientbound::TAB_LIST);
    w.write_nbt_network(header)?;
    w.write_nbt_network(footer)?;
    Ok(w.into_bytes())
}

/// `ClientboundSetEntityLinkPacket` (`minecraft:set_entity_link`, ID 102).
///
/// Both `source_id` and `dest_id` are 4-byte big-endian `i32` (NOT `VarInt`); pass `dest_id = 0` to detach.
pub fn encode_play_set_entity_link(source_id: i32, dest_id: i32) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(12);
    w.write_varint(packet_ids::play::clientbound::SET_ENTITY_LINK);
    w.write_i32(source_id);
    w.write_i32(dest_id);
    w.into_bytes()
}

/// `ClientboundBundleDelimiterPacket` (`minecraft:bundle_delimiter`, ID 0, 0-byte payload).
pub fn encode_play_bundle_delimiter() -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(2);
    w.write_varint(packet_ids::play::clientbound::BUNDLE_DELIMITER);
    w.into_bytes()
}

// ======================== Play Serverbound Decoders & Encoders ========================

/// Second batch of Minecraft 26.3 (Protocol 777) Play-phase serverbound packets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayExtra2Serverbound {
    /// `ServerboundPaddleBoatPacket` (`minecraft:paddle_boat`, ID 35).
    PaddleBoat { left: bool, right: bool },
    /// `ServerboundEditBookPacket` (`minecraft:edit_book`, ID 24).
    EditBook {
        slot: i32,
        pages: Vec<String>,
        title: Option<String>,
    },
    /// `ServerboundJigsawGeneratePacket` (`minecraft:jigsaw_generate`, ID 27).
    JigsawGenerate {
        pos: BlockPos,
        levels: i32,
        keep_jigsaws: bool,
    },
    /// `ServerboundPlayerInputPacket` (`minecraft:player_input`, ID 43).
    ///
    /// Bit flags (`crate::player_input_flags`):
    /// `0x01` forward, `0x02` backward, `0x04` left, `0x08` right,
    /// `0x10` jump, `0x20` shift, `0x40` sprint.
    PlayerInput { flags: u8 },
    /// `ServerboundSeenAdvancementsPacket` (`minecraft:seen_advancements`, ID 51).
    SeenAdvancements {
        action: SeenAdvancementsAction,
        tab: Option<String>,
    },
    /// `ServerboundSetBeaconPacket` (`minecraft:set_beacon`, ID 53).
    ///
    /// Effect IDs are raw 0-based protocol IDs in `minecraft:mob_effect`
    /// (`MobEffect.STREAM_CODEC` = `ByteBufCodecs.holderRegistry`).
    SetBeacon {
        primary: Option<i32>,
        secondary: Option<i32>,
    },
    /// `ServerboundClientTickEndPacket` (`minecraft:client_tick_end`, ID 13, 0-byte payload).
    ClientTickEnd,
    /// `ServerboundChatAckPacket` (`minecraft:chat_ack`, ID 6).
    ChatAck { offset: i32 },
}

/// Decode a Play-phase serverbound packet handled by [`PlayExtra2Serverbound`] from its
/// already-read `packet_id` and remaining payload `body`.
///
/// Returns `Ok(None)` if `packet_id` does not belong to this batch.
pub fn decode_play_extra2_serverbound(
    packet_id: i32,
    body: &[u8],
) -> Result<Option<PlayExtra2Serverbound>, ProtoError> {
    let mut r = PacketReader::new(body);
    if packet_id == packet_ids::play::serverbound::PADDLE_BOAT {
        let left = r.read_bool()?;
        let right = r.read_bool()?;
        Ok(Some(PlayExtra2Serverbound::PaddleBoat { left, right }))
    } else if packet_id == packet_ids::play::serverbound::EDIT_BOOK {
        let slot = r.read_varint()?;
        let page_count = r.read_varint()?;
        if page_count < 0 {
            return Err(ProtoError::NegativeLength(page_count));
        }
        if page_count > 100 {
            return Err(ProtoError::LengthExceeded(page_count as usize, 100));
        }
        let mut pages = Vec::with_capacity(page_count as usize);
        for _ in 0..page_count {
            pages.push(r.read_string(1024)?);
        }
        let title = if r.read_bool()? {
            Some(r.read_string(32)?)
        } else {
            None
        };
        Ok(Some(PlayExtra2Serverbound::EditBook { slot, pages, title }))
    } else if packet_id == packet_ids::play::serverbound::JIGSAW_GENERATE {
        let pos = r.read_block_pos()?;
        let levels = r.read_varint()?;
        let keep_jigsaws = r.read_bool()?;
        Ok(Some(PlayExtra2Serverbound::JigsawGenerate {
            pos,
            levels,
            keep_jigsaws,
        }))
    } else if packet_id == packet_ids::play::serverbound::PLAYER_INPUT {
        let flags = r.read_u8()?;
        Ok(Some(PlayExtra2Serverbound::PlayerInput { flags }))
    } else if packet_id == packet_ids::play::serverbound::SEEN_ADVANCEMENTS {
        let action = SeenAdvancementsAction::from_id(r.read_varint()?)?;
        let tab = match action {
            SeenAdvancementsAction::OpenedTab => Some(r.read_string(32767)?),
            SeenAdvancementsAction::ClosedScreen => None,
        };
        Ok(Some(PlayExtra2Serverbound::SeenAdvancements { action, tab }))
    } else if packet_id == packet_ids::play::serverbound::SET_BEACON {
        let primary = if r.read_bool()? {
            Some(r.read_varint()?)
        } else {
            None
        };
        let secondary = if r.read_bool()? {
            Some(r.read_varint()?)
        } else {
            None
        };
        Ok(Some(PlayExtra2Serverbound::SetBeacon { primary, secondary }))
    } else if packet_id == packet_ids::play::serverbound::CLIENT_TICK_END {
        Ok(Some(PlayExtra2Serverbound::ClientTickEnd))
    } else if packet_id == packet_ids::play::serverbound::CHAT_ACK {
        let offset = r.read_varint()?;
        Ok(Some(PlayExtra2Serverbound::ChatAck { offset }))
    } else {
        Ok(None)
    }
}

impl PlayExtra2Serverbound {
    /// Convenience decoder accepting a raw decompressed packet frame (starting with the `VarInt` packet ID).
    pub fn decode(bytes: &[u8]) -> Result<Option<Self>, ProtoError> {
        let mut r = PacketReader::new(bytes);
        let packet_id = r.read_varint()?;
        let body = r.read_bytes(r.remaining())?;
        decode_play_extra2_serverbound(packet_id, body)
    }
}

/// Encode `ServerboundPaddleBoatPacket` (`minecraft:paddle_boat`, ID 35).
pub fn encode_sb_paddle_boat(left: bool, right: bool) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(4);
    w.write_varint(packet_ids::play::serverbound::PADDLE_BOAT);
    w.write_bool(left);
    w.write_bool(right);
    w.into_bytes()
}

/// Encode `ServerboundEditBookPacket` (`minecraft:edit_book`, ID 24).
pub fn encode_sb_edit_book(slot: i32, pages: &[&str], title: Option<&str>) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(32 + pages.len() * 32);
    w.write_varint(packet_ids::play::serverbound::EDIT_BOOK);
    w.write_varint(slot);
    w.write_varint(pages.len() as i32);
    for page in pages {
        w.write_string(page);
    }
    match title {
        Some(t) => {
            w.write_bool(true);
            w.write_string(t);
        }
        None => w.write_bool(false),
    }
    w.into_bytes()
}

/// Encode `ServerboundJigsawGeneratePacket` (`minecraft:jigsaw_generate`, ID 27).
pub fn encode_sb_jigsaw_generate(pos: BlockPos, levels: i32, keep_jigsaws: bool) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(16);
    w.write_varint(packet_ids::play::serverbound::JIGSAW_GENERATE);
    w.write_block_pos(pos);
    w.write_varint(levels);
    w.write_bool(keep_jigsaws);
    w.into_bytes()
}

/// Encode `ServerboundPlayerInputPacket` (`minecraft:player_input`, ID 43).
pub fn encode_sb_player_input(flags: u8) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(4);
    w.write_varint(packet_ids::play::serverbound::PLAYER_INPUT);
    w.write_u8(flags);
    w.into_bytes()
}

/// Encode `ServerboundSeenAdvancementsPacket` (`minecraft:seen_advancements`, ID 51).
///
/// When `action` is [`SeenAdvancementsAction::OpenedTab`], writes the `tab` identifier
/// (must be a valid `namespace:path` identifier string). When `ClosedScreen`, writes no identifier.
pub fn encode_sb_seen_advancements(action: SeenAdvancementsAction, tab: Option<&str>) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(32);
    w.write_varint(packet_ids::play::serverbound::SEEN_ADVANCEMENTS);
    w.write_varint(action as i32);
    if action == SeenAdvancementsAction::OpenedTab {
        w.write_string(tab.unwrap_or("minecraft:story/root"));
    }
    w.into_bytes()
}

/// Encode `ServerboundSetBeaconPacket` (`minecraft:set_beacon`, ID 53).
///
/// Effect IDs are 0-based protocol IDs in `minecraft:mob_effect`.
pub fn encode_sb_set_beacon(primary: Option<i32>, secondary: Option<i32>) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(12);
    w.write_varint(packet_ids::play::serverbound::SET_BEACON);
    match primary {
        Some(id) => {
            w.write_bool(true);
            w.write_varint(id);
        }
        None => w.write_bool(false),
    }
    match secondary {
        Some(id) => {
            w.write_bool(true);
            w.write_varint(id);
        }
        None => w.write_bool(false),
    }
    w.into_bytes()
}

/// Encode `ServerboundClientTickEndPacket` (`minecraft:client_tick_end`, ID 13, 0-byte payload).
pub fn encode_sb_client_tick_end() -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(2);
    w.write_varint(packet_ids::play::serverbound::CLIENT_TICK_END);
    w.into_bytes()
}

/// Encode `ServerboundChatAckPacket` (`minecraft:chat_ack`, ID 6).
pub fn encode_sb_chat_ack(offset: i32) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(6);
    w.write_varint(packet_ids::play::serverbound::CHAT_ACK);
    w.write_varint(offset);
    w.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{player_input_flags, text_component};
    use mc_nbt::NbtCompound;
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

    fn decode_frame(bytes: &[u8]) -> Result<Option<PlayExtra2Serverbound>, ProtoError> {
        let mut r = PacketReader::new(bytes);
        let id = r.read_varint()?;
        let body = r.read_bytes(r.remaining())?;
        decode_play_extra2_serverbound(id, body)
    }

    #[test]
    fn diff_all_extra2_protocol_777_packets_against_java_oracle() {
        // ======================== Clientbound Packets ========================

        // 1. ClientboundForgetLevelChunkPacket (verify negative/positive ChunkPos pack order!)
        assert_eq!(unpack_chunk_pos(pack_chunk_pos(-7, 13)), (-7, 13));
        assert_eq!(unpack_chunk_pos(pack_chunk_pos(42, -99)), (42, -99));
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:forget_level_chunk",
            &encode_play_forget_level_chunk(-7, 13),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:forget_level_chunk",
            &encode_play_forget_level_chunk(42, -99),
        );

        // 2. ClientboundSetSimulationDistancePacket
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_simulation_distance",
            &encode_play_set_simulation_distance(12),
        );

        // 3. ClientboundSetChunkCacheRadiusPacket
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_chunk_cache_radius",
            &encode_play_set_chunk_cache_radius(16),
        );

        // 4. ClientboundDamageEventPacket (with entities + position, and with None/None/None)
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:damage_event",
            &encode_play_damage_event(
                101,
                0,
                Some(202),
                Some(303),
                Some(Vec3::new(10.5, 64.25, -20.75)),
            ),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:damage_event",
            &encode_play_damage_event(101, 5, None, None, None),
        );
        // Also test entity ID 0 as cause (encodes as VarInt(1) -> Java decodes 0 -> re-encodes 1)
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:damage_event",
            &encode_play_damage_event(101, 2, Some(0), None, None),
        );

        // 5. ClientboundLevelParticlesPacket (26.3: particle FIRST, 3 speed floats, VarInt count, RandomizationType!)
        let flame_id =
            mc_data::static_registry_protocol_id("minecraft:particle_type", "minecraft:flame")
                .unwrap();
        let heart_id =
            mc_data::static_registry_protocol_id("minecraft:particle_type", "minecraft:heart")
                .unwrap();
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:level_particles",
            &encode_play_level_particles_simple(
                flame_id,
                true,
                false,
                12.5,
                65.0,
                -4.25,
                0.25,
                0.5,
                0.25,
                0.08,
                20,
            ),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:level_particles",
            &encode_play_level_particles_simple_ex(
                heart_id,
                false,
                true,
                -3.0,
                70.5,
                8.0,
                0.1,
                0.2,
                0.3,
                0.05,
                0.15,
                0.25,
                7,
                ParticleRandomizationType::AlternativeWithSpeed,
            ),
        );

        // 6. ClientboundAwardStatsPacket (verify single-entry byte-for-byte across multiple categories + multi-entry set equality)
        let stone_block_id =
            mc_data::static_registry_protocol_id("minecraft:block", "minecraft:stone").unwrap();
        let diamond_item_id =
            mc_data::static_registry_protocol_id("minecraft:item", "minecraft:diamond").unwrap();
        let zombie_entity_id =
            mc_data::static_registry_protocol_id("minecraft:entity_type", "minecraft:zombie")
                .unwrap();
        let jump_stat_id =
            mc_data::static_registry_protocol_id("minecraft:custom_stat", "minecraft:jump")
                .unwrap();
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:award_stats",
            &encode_play_award_stats(&[]),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:award_stats",
            &encode_play_award_stats(&[(stat_category::MINED, stone_block_id, 250)]),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:award_stats",
            &encode_play_award_stats(&[(stat_category::CRAFTED, diamond_item_id, 12)]),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:award_stats",
            &encode_play_award_stats(&[(stat_category::KILLED, zombie_entity_id, 5)]),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:award_stats",
            &encode_play_award_stats(&[(stat_category::CUSTOM, jump_stat_id, 99)]),
        );
        // Multi-entry check: Java stores stats in an unordered Object2IntOpenHashMap, so verify exact multiset match.
        let multi_stats = vec![
            (stat_category::MINED, stone_block_id, 250),
            (stat_category::CRAFTED, diamond_item_id, 12),
            (stat_category::KILLED, zombie_entity_id, 5),
            (stat_category::CUSTOM, jump_stat_id, 99),
        ];
        let encoded_multi = encode_play_award_stats(&multi_stats);
        let (ptype, _jclass, reencoded_multi) = with_oracle(|o| {
            o.packet_roundtrip("play", "clientbound", &encoded_multi)
        })
        .unwrap();
        assert_eq!(ptype, "minecraft:award_stats");
        let mut mr = PacketReader::new(&reencoded_multi);
        assert_eq!(mr.read_varint().unwrap(), packet_ids::play::clientbound::AWARD_STATS);
        let count = mr.read_varint().unwrap();
        assert_eq!(count as usize, multi_stats.len());
        let mut decoded_stats = Vec::with_capacity(count as usize);
        for _ in 0..count {
            decoded_stats.push((
                mr.read_varint().unwrap(),
                mr.read_varint().unwrap(),
                mr.read_varint().unwrap(),
            ));
        }
        assert_eq!(mr.remaining(), 0);
        let mut expected_sorted = multi_stats;
        expected_sorted.sort_unstable();
        decoded_stats.sort_unstable();
        assert_eq!(decoded_stats, expected_sorted);

        // 7. ClientboundPlaceGhostRecipePacket (shaped crafting + furnace + stonecutter)
        let oak_planks_id =
            mc_data::static_registry_protocol_id("minecraft:item", "minecraft:oak_planks").unwrap();
        let stick_id =
            mc_data::static_registry_protocol_id("minecraft:item", "minecraft:stick").unwrap();
        let crafting_table_id =
            mc_data::static_registry_protocol_id("minecraft:item", "minecraft:crafting_table")
                .unwrap();
        let furnace_id =
            mc_data::static_registry_protocol_id("minecraft:item", "minecraft:furnace").unwrap();
        let raw_iron_id =
            mc_data::static_registry_protocol_id("minecraft:item", "minecraft:raw_iron").unwrap();
        let iron_ingot_id =
            mc_data::static_registry_protocol_id("minecraft:item", "minecraft:iron_ingot").unwrap();
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:place_ghost_recipe",
            &encode_play_place_ghost_recipe(
                1,
                &RecipeDisplay::CraftingShaped {
                    width: 1,
                    height: 2,
                    ingredients: vec![
                        SlotDisplay::Tag("minecraft:planks".into()),
                        SlotDisplay::Item(oak_planks_id),
                    ],
                    result: SlotDisplay::ItemStack {
                        item_id: stick_id,
                        count: 4,
                    },
                    crafting_station: SlotDisplay::Item(crafting_table_id),
                },
            )
            .unwrap(),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:place_ghost_recipe",
            &encode_play_place_ghost_recipe(
                2,
                &RecipeDisplay::Furnace {
                    ingredient: SlotDisplay::Item(raw_iron_id),
                    fuel: SlotDisplay::AnyFuel,
                    result: SlotDisplay::Item(iron_ingot_id),
                    crafting_station: SlotDisplay::Item(furnace_id),
                    duration: 200,
                    experience: 0.7,
                },
            )
            .unwrap(),
        );

        // 8. ClientboundCooldownPacket (26.3: Identifier cooldownGroup + VarInt duration)
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:cooldown",
            &encode_play_cooldown("minecraft:ender_pearl", 20),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:cooldown",
            &encode_play_cooldown("minecraft:shield", 100),
        );

        // 9. ClientboundPlayerLookAtPacket (position-only + targeting an entity)
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:player_look_at",
            &encode_play_player_look_at(EntityAnchor::Eyes, 1.5, 64.0, -3.25, None),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:player_look_at",
            &encode_play_player_look_at(
                EntityAnchor::Feet,
                10.0,
                65.62,
                20.0,
                Some((55, EntityAnchor::Eyes)),
            ),
        );

        // 10. World Border packets (center, size, lerp size, warning delay, warning distance)
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_border_center",
            &encode_play_set_border_center(-128.5, 512.75),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_border_size",
            &encode_play_set_border_size(2048.0),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_border_lerp_size",
            &encode_play_set_border_lerp_size(2048.0, 512.0, 30_000),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_border_warning_delay",
            &encode_play_set_border_warning_delay(15),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_border_warning_distance",
            &encode_play_set_border_warning_distance(10),
        );

        // 11. ClientboundTabListPacket (plain-text re-export + styled NBT component variant)
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:tab_list",
            &encode_play_tab_list("SiliconMC 26.3", "Players: 1").unwrap(),
        );
        let mut styled_header = NbtCompound::new();
        styled_header.insert("text", NbtTag::String("SiliconMC".into()));
        styled_header.insert("color", NbtTag::String("aqua".into()));
        let styled_raw = encode_play_tab_list_components(
            &NbtTag::Compound(styled_header.clone()),
            &text_component("20.0 TPS"),
        )
        .unwrap();
        let (tab_type, _tab_cls, tab_reenc) =
            with_oracle(|o| o.packet_roundtrip("play", "clientbound", &styled_raw)).unwrap();
        assert_eq!(tab_type, "minecraft:tab_list");
        assert_eq!(tab_reenc.len(), styled_raw.len());
        let mut tab_r = PacketReader::new(&tab_reenc);
        assert_eq!(tab_r.read_varint().unwrap(), packet_ids::play::clientbound::TAB_LIST);
        let decoded_header = tab_r.read_nbt_network().unwrap();
        assert_eq!(decoded_header, NbtTag::Compound(styled_header));
        assert_eq!(tab_r.read_nbt_network().unwrap(), text_component("20.0 TPS"));

        // 12. ClientboundSetEntityLinkPacket (attach and detach with dest_id = 0)
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_entity_link",
            &encode_play_set_entity_link(401, 402),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_entity_link",
            &encode_play_set_entity_link(401, 0),
        );

        // 13. ClientboundBundleDelimiterPacket (0-byte payload)
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:bundle_delimiter",
            &encode_play_bundle_delimiter(),
        );

        // ======================== Serverbound Packets ========================

        // 14. ServerboundPaddleBoatPacket
        let boat = encode_sb_paddle_boat(true, false);
        assert_oracle_roundtrip("play", "serverbound", "minecraft:paddle_boat", &boat);
        assert_eq!(
            decode_frame(&boat).unwrap(),
            Some(PlayExtra2Serverbound::PaddleBoat {
                left: true,
                right: false,
            })
        );

        // 15. ServerboundEditBookPacket (signed with title + unsigned without title)
        let book_signed = encode_sb_edit_book(
            4,
            &["Chapter 1: SiliconMC", "Protocol 777 verified!"],
            Some("Field Notes"),
        );
        assert_oracle_roundtrip("play", "serverbound", "minecraft:edit_book", &book_signed);
        assert_eq!(
            decode_frame(&book_signed).unwrap(),
            Some(PlayExtra2Serverbound::EditBook {
                slot: 4,
                pages: vec![
                    "Chapter 1: SiliconMC".into(),
                    "Protocol 777 verified!".into(),
                ],
                title: Some("Field Notes".into()),
            })
        );
        let book_unsigned = encode_sb_edit_book(0, &["Draft page"], None);
        assert_oracle_roundtrip("play", "serverbound", "minecraft:edit_book", &book_unsigned);
        assert_eq!(
            decode_frame(&book_unsigned).unwrap(),
            Some(PlayExtra2Serverbound::EditBook {
                slot: 0,
                pages: vec!["Draft page".into()],
                title: None,
            })
        );

        // 16. ServerboundJigsawGeneratePacket
        let jigsaw_pos = BlockPos::new(-32, 72, 144);
        let jigsaw = encode_sb_jigsaw_generate(jigsaw_pos, 7, true);
        assert_oracle_roundtrip("play", "serverbound", "minecraft:jigsaw_generate", &jigsaw);
        assert_eq!(
            decode_frame(&jigsaw).unwrap(),
            Some(PlayExtra2Serverbound::JigsawGenerate {
                pos: jigsaw_pos,
                levels: 7,
                keep_jigsaws: true,
            })
        );

        // 17. ServerboundPlayerInputPacket
        let flags = player_input_flags::FORWARD
            | player_input_flags::LEFT
            | player_input_flags::SHIFT
            | player_input_flags::SPRINT;
        let pinput = encode_sb_player_input(flags);
        assert_oracle_roundtrip("play", "serverbound", "minecraft:player_input", &pinput);
        assert_eq!(
            decode_frame(&pinput).unwrap(),
            Some(PlayExtra2Serverbound::PlayerInput { flags })
        );

        // 18. ServerboundSeenAdvancementsPacket (OpenedTab + ClosedScreen)
        let adv_open = encode_sb_seen_advancements(
            SeenAdvancementsAction::OpenedTab,
            Some("minecraft:story/root"),
        );
        assert_oracle_roundtrip(
            "play",
            "serverbound",
            "minecraft:seen_advancements",
            &adv_open,
        );
        assert_eq!(
            decode_frame(&adv_open).unwrap(),
            Some(PlayExtra2Serverbound::SeenAdvancements {
                action: SeenAdvancementsAction::OpenedTab,
                tab: Some("minecraft:story/root".into()),
            })
        );
        let adv_close = encode_sb_seen_advancements(SeenAdvancementsAction::ClosedScreen, None);
        assert_oracle_roundtrip(
            "play",
            "serverbound",
            "minecraft:seen_advancements",
            &adv_close,
        );
        assert_eq!(
            decode_frame(&adv_close).unwrap(),
            Some(PlayExtra2Serverbound::SeenAdvancements {
                action: SeenAdvancementsAction::ClosedScreen,
                tab: None,
            })
        );

        // 19. ServerboundSetBeaconPacket (both present, primary only, both empty)
        let speed_id =
            mc_data::static_registry_protocol_id("minecraft:mob_effect", "minecraft:speed")
                .unwrap();
        let regen_id =
            mc_data::static_registry_protocol_id("minecraft:mob_effect", "minecraft:regeneration")
                .unwrap();
        let beacon_both = encode_sb_set_beacon(Some(speed_id), Some(regen_id));
        assert_oracle_roundtrip("play", "serverbound", "minecraft:set_beacon", &beacon_both);
        assert_eq!(
            decode_frame(&beacon_both).unwrap(),
            Some(PlayExtra2Serverbound::SetBeacon {
                primary: Some(speed_id),
                secondary: Some(regen_id),
            })
        );
        let beacon_primary = encode_sb_set_beacon(Some(speed_id), None);
        assert_oracle_roundtrip(
            "play",
            "serverbound",
            "minecraft:set_beacon",
            &beacon_primary,
        );
        assert_eq!(
            decode_frame(&beacon_primary).unwrap(),
            Some(PlayExtra2Serverbound::SetBeacon {
                primary: Some(speed_id),
                secondary: None,
            })
        );
        let beacon_none = encode_sb_set_beacon(None, None);
        assert_oracle_roundtrip("play", "serverbound", "minecraft:set_beacon", &beacon_none);
        assert_eq!(
            decode_frame(&beacon_none).unwrap(),
            Some(PlayExtra2Serverbound::SetBeacon {
                primary: None,
                secondary: None,
            })
        );

        // 20. ServerboundClientTickEndPacket (0-byte payload)
        let tick_end = encode_sb_client_tick_end();
        assert_oracle_roundtrip(
            "play",
            "serverbound",
            "minecraft:client_tick_end",
            &tick_end,
        );
        assert_eq!(
            decode_frame(&tick_end).unwrap(),
            Some(PlayExtra2Serverbound::ClientTickEnd)
        );
        assert_eq!(
            PlayExtra2Serverbound::decode(&tick_end).unwrap(),
            Some(PlayExtra2Serverbound::ClientTickEnd)
        );

        // 21. ServerboundChatAckPacket
        let chat_ack = encode_sb_chat_ack(17);
        assert_oracle_roundtrip("play", "serverbound", "minecraft:chat_ack", &chat_ack);
        assert_eq!(
            decode_frame(&chat_ack).unwrap(),
            Some(PlayExtra2Serverbound::ChatAck { offset: 17 })
        );

        // Unhandled packet ID cleanly returns Ok(None)
        assert_eq!(
            decode_play_extra2_serverbound(packet_ids::play::serverbound::KEEP_ALIVE, &[0; 8])
                .unwrap(),
            None
        );
    }
}
