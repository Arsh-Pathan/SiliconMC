use crate::{PacketReader, PacketWriter, ProtoError};
use mc_data::packet_ids;
use mc_math::{BlockPos, Vec3};
use mc_nbt::{NbtCompound, NbtTag};
use mc_world::{Chunk, ContainerClickType, ItemStack};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ======================== Common Shared Types ========================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Down = 0,
    Up = 1,
    North = 2,
    South = 3,
    West = 4,
    East = 5,
}

impl Direction {
    pub fn from_id(id: i32) -> Result<Self, ProtoError> {
        match id {
            0 => Ok(Self::Down),
            1 => Ok(Self::Up),
            2 => Ok(Self::North),
            3 => Ok(Self::South),
            4 => Ok(Self::West),
            5 => Ok(Self::East),
            other => Err(ProtoError::InvalidValue(other)),
        }
    }

    #[inline]
    pub fn offset_pos(self, pos: BlockPos) -> BlockPos {
        match self {
            Self::Down => pos.offset(0, -1, 0),
            Self::Up => pos.offset(0, 1, 0),
            Self::North => pos.offset(0, 0, -1),
            Self::South => pos.offset(0, 0, 1),
            Self::West => pos.offset(-1, 0, 0),
            Self::East => pos.offset(1, 0, 0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionHand {
    MainHand = 0,
    OffHand = 1,
}

impl InteractionHand {
    pub fn from_id(id: i32) -> Result<Self, ProtoError> {
        match id {
            0 => Ok(Self::MainHand),
            1 => Ok(Self::OffHand),
            other => Err(ProtoError::InvalidValue(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerActionType {
    StartDestroyBlock = 0,
    ChangeDestroyDirection = 1,
    AbortDestroyBlock = 2,
    StopDestroyBlock = 3,
    DropAllItems = 4,
    DropItem = 5,
    ReleaseUseItem = 6,
    SwapItemWithOffhand = 7,
    Stab = 8,
}

impl PlayerActionType {
    pub fn from_id(id: i32) -> Result<Self, ProtoError> {
        match id {
            0 => Ok(Self::StartDestroyBlock),
            1 => Ok(Self::ChangeDestroyDirection),
            2 => Ok(Self::AbortDestroyBlock),
            3 => Ok(Self::StopDestroyBlock),
            4 => Ok(Self::DropAllItems),
            5 => Ok(Self::DropItem),
            6 => Ok(Self::ReleaseUseItem),
            7 => Ok(Self::SwapItemWithOffhand),
            8 => Ok(Self::Stab),
            other => Err(ProtoError::InvalidValue(other)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockHitResult {
    pub pos: BlockPos,
    pub direction: Direction,
    pub click_x: f32,
    pub click_y: f32,
    pub click_z: f32,
    pub inside: bool,
    pub world_border_hit: bool,
}

/// Encode `ItemStack.OPTIONAL_STREAM_CODEC` (used for clientbound container slots).
pub fn write_item_stack_network(w: &mut PacketWriter, stack: &ItemStack) {
    if stack.is_empty() {
        w.write_varint(0);
        return;
    }
    w.write_varint(stack.count as i32);
    w.write_varint(stack.item_id as i32);
    w.write_varint(stack.added_components.len() as i32);
    w.write_varint(stack.removed_components.len() as i32);
    for (type_id, bytes) in &stack.added_components {
        w.write_varint(*type_id);
        w.write_raw(bytes);
    }
    for type_id in &stack.removed_components {
        w.write_varint(*type_id);
    }
}

/// Encode `ItemStack.OPTIONAL_UNTRUSTED_STREAM_CODEC` (length-prefixed components, used by `SetCreativeModeSlot`).
pub fn write_item_stack_untrusted(w: &mut PacketWriter, stack: &ItemStack) {
    if stack.is_empty() {
        w.write_varint(0);
        return;
    }
    w.write_varint(stack.count as i32);
    w.write_varint(stack.item_id as i32);
    w.write_varint(stack.added_components.len() as i32);
    w.write_varint(stack.removed_components.len() as i32);
    for (type_id, bytes) in &stack.added_components {
        w.write_varint(*type_id);
        w.write_byte_array(bytes);
    }
    for type_id in &stack.removed_components {
        w.write_varint(*type_id);
    }
}

/// Decode `ItemStack.OPTIONAL_UNTRUSTED_STREAM_CODEC` (`DataComponentPatch.DELIMITED_STREAM_CODEC`).
pub fn read_item_stack_untrusted(r: &mut PacketReader<'_>) -> Result<ItemStack, ProtoError> {
    let count = r.read_varint()?;
    if count <= 0 {
        return Ok(ItemStack::EMPTY);
    }
    let item_id = r.read_varint()?;
    let positive_count = r.read_varint()?;
    let negative_count = r.read_varint()?;
    if positive_count < 0 || negative_count < 0 {
        return Err(ProtoError::NegativeLength(positive_count.min(negative_count)));
    }
    let mut added_components = Vec::with_capacity((positive_count as usize).min(256));
    for _ in 0..positive_count {
        let type_id = r.read_varint()?;
        let data = r.read_byte_array(1_048_576)?;
        added_components.push((type_id, data));
    }
    let mut removed_components = Vec::with_capacity((negative_count as usize).min(256));
    for _ in 0..negative_count {
        removed_components.push(r.read_varint()?);
    }
    Ok(ItemStack {
        item_id: item_id.clamp(0, u16::MAX as i32) as u16,
        count: count.clamp(0, 99) as u8,
        added_components,
        removed_components,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandArgType {
    SingleWord,
    QuotablePhrase,
    GreedyPhrase,
    Integer { min: Option<i32>, max: Option<i32> },
    BlockPos,
    Vec3,
    BlockState,
    ItemStack,
    GameMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandNodeKind {
    Root,
    Literal(String),
    Argument {
        name: String,
        arg_type: CommandArgType,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandNodeEntry {
    pub kind: CommandNodeKind,
    pub executable: bool,
    pub children: Vec<i32>,
}

impl CommandNodeEntry {
    pub fn root(children: Vec<i32>) -> Self {
        Self {
            kind: CommandNodeKind::Root,
            executable: false,
            children,
        }
    }

    pub fn literal(name: impl Into<String>, executable: bool, children: Vec<i32>) -> Self {
        Self {
            kind: CommandNodeKind::Literal(name.into()),
            executable,
            children,
        }
    }

    pub fn argument(
        name: impl Into<String>,
        arg_type: CommandArgType,
        executable: bool,
        children: Vec<i32>,
    ) -> Self {
        Self {
            kind: CommandNodeKind::Argument {
                name: name.into(),
                arg_type,
            },
            executable,
            children,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientIntent {
    Status = 1,
    Login = 2,
    Transfer = 3,
}

impl ClientIntent {
    pub fn from_id(id: i32) -> Result<Self, ProtoError> {
        match id {
            1 => Ok(Self::Status),
            2 => Ok(Self::Login),
            3 => Ok(Self::Transfer),
            other => Err(ProtoError::InvalidValue(other)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileProperty {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameProfile {
    pub uuid: Uuid,
    pub name: String,
    pub properties: Vec<ProfileProperty>,
}

impl GameProfile {
    pub fn write(&self, w: &mut PacketWriter) {
        w.write_uuid(self.uuid);
        w.write_string(&self.name);
        w.write_varint(self.properties.len() as i32);
        for p in &self.properties {
            w.write_string(&p.name);
            w.write_string(&p.value);
            match &p.signature {
                Some(sig) => {
                    w.write_bool(true);
                    w.write_string(sig);
                }
                None => w.write_bool(false),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownPack {
    pub namespace: String,
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PackedRegistryEntry {
    pub id: String,
    pub data: Option<NbtTag>,
}

/// Build a canonical plain-text Minecraft `Component` NBT tag (vanilla encodes plain unstyled text as `StringTag`).
pub fn text_component(text: impl Into<String>) -> NbtTag {
    NbtTag::String(text.into())
}

#[inline]
pub fn pack_degrees(deg: f32) -> i8 {
    (deg * 256.0_f32 / 360.0_f32).floor() as i32 as i8
}

// ======================== Handshake ========================

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeIntention {
    pub protocol_version: i32,
    pub host_name: String,
    pub port: u16,
    pub intent: ClientIntent,
}

impl HandshakeIntention {
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtoError> {
        let mut r = PacketReader::new(bytes);
        let id = r.read_varint()?;
        if id != packet_ids::handshake::serverbound::INTENTION {
            return Err(ProtoError::UnknownPacketId(id, "handshake"));
        }
        Ok(Self {
            protocol_version: r.read_varint()?,
            host_name: r.read_string(255)?,
            port: r.read_u16()?,
            intent: ClientIntent::from_id(r.read_varint()?)?,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut w = PacketWriter::new();
        w.write_varint(packet_ids::handshake::serverbound::INTENTION);
        w.write_varint(self.protocol_version);
        w.write_string(&self.host_name);
        w.write_u16(self.port);
        w.write_varint(self.intent as i32);
        w.into_bytes()
    }
}

// ======================== Status ========================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusServerbound {
    StatusRequest,
    PingRequest { time: i64 },
}

impl StatusServerbound {
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtoError> {
        let mut r = PacketReader::new(bytes);
        let id = r.read_varint()?;
        if id == packet_ids::status::serverbound::STATUS_REQUEST {
            Ok(Self::StatusRequest)
        } else if id == packet_ids::status::serverbound::PING_REQUEST {
            Ok(Self::PingRequest {
                time: r.read_i64()?,
            })
        } else {
            Err(ProtoError::UnknownPacketId(id, "status"))
        }
    }
}

pub fn encode_status_response(json_str: &str) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::status::clientbound::STATUS_RESPONSE);
    w.write_string(json_str);
    w.into_bytes()
}

pub fn encode_status_pong(time: i64) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::status::clientbound::PONG_RESPONSE);
    w.write_i64(time);
    w.into_bytes()
}

// ======================== Login ========================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginServerbound {
    Hello {
        name: String,
        profile_id: Uuid,
    },
    Key {
        encrypted_secret: Vec<u8>,
        encrypted_challenge: Vec<u8>,
    },
    LoginAcknowledged,
}

impl LoginServerbound {
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtoError> {
        let mut r = PacketReader::new(bytes);
        let id = r.read_varint()?;
        if id == packet_ids::login::serverbound::HELLO {
            Ok(Self::Hello {
                name: r.read_string(16)?,
                profile_id: r.read_uuid()?,
            })
        } else if id == packet_ids::login::serverbound::KEY {
            Ok(Self::Key {
                encrypted_secret: r.read_byte_array(1024)?,
                encrypted_challenge: r.read_byte_array(1024)?,
            })
        } else if id == packet_ids::login::serverbound::LOGIN_ACKNOWLEDGED {
            Ok(Self::LoginAcknowledged)
        } else {
            Err(ProtoError::UnknownPacketId(id, "login"))
        }
    }
}

pub fn encode_login_disconnect(reason_json: &str) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::login::clientbound::LOGIN_DISCONNECT);
    w.write_string(reason_json);
    w.into_bytes()
}

pub fn encode_login_hello(
    server_id: &str,
    public_key_der: &[u8],
    challenge: &[u8],
    should_authenticate: bool,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::login::clientbound::HELLO);
    w.write_string(server_id);
    w.write_byte_array(public_key_der);
    w.write_byte_array(challenge);
    w.write_bool(should_authenticate);
    w.into_bytes()
}

pub fn encode_login_compression(threshold: i32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::login::clientbound::LOGIN_COMPRESSION);
    w.write_varint(threshold);
    w.into_bytes()
}

/// Minecraft 26.3 `ClientboundLoginFinishedPacket(GameProfile gameProfile, UUID sessionId)`.
pub fn encode_login_finished(profile: &GameProfile, session_id: Uuid) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::login::clientbound::LOGIN_FINISHED);
    profile.write(&mut w);
    w.write_uuid(session_id);
    w.into_bytes()
}

// ======================== Configuration ========================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigServerbound {
    ClientInformation {
        locale: String,
        view_distance: i8,
        chat_mode: i32,
        chat_colors: bool,
        model_parts: u8,
        main_hand: i32,
        text_filtering: bool,
        allow_listing: bool,
        particle_status: i32,
    },
    CustomPayload {
        channel: String,
        data: Vec<u8>,
    },
    SelectKnownPacks {
        packs: Vec<KnownPack>,
    },
    FinishConfiguration,
    KeepAlive {
        id: i64,
    },
    Pong {
        id: i32,
    },
    Other(i32),
}

impl ConfigServerbound {
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtoError> {
        let mut r = PacketReader::new(bytes);
        let id = r.read_varint()?;
        if id == packet_ids::configuration::serverbound::CLIENT_INFORMATION {
            Ok(Self::ClientInformation {
                locale: r.read_string(16)?,
                view_distance: r.read_i8()?,
                chat_mode: r.read_varint()?,
                chat_colors: r.read_bool()?,
                model_parts: r.read_u8()?,
                main_hand: r.read_varint()?,
                text_filtering: r.read_bool()?,
                allow_listing: r.read_bool()?,
                particle_status: r.read_varint()?,
            })
        } else if id == packet_ids::configuration::serverbound::CUSTOM_PAYLOAD {
            let channel = r.read_string(32767)?;
            let data = r.read_bytes(r.remaining())?.to_vec();
            Ok(Self::CustomPayload { channel, data })
        } else if id == packet_ids::configuration::serverbound::SELECT_KNOWN_PACKS {
            let count = r.read_varint()? as usize;
            let mut packs = Vec::with_capacity(count.min(64));
            for _ in 0..count {
                packs.push(KnownPack {
                    namespace: r.read_string(32767)?,
                    id: r.read_string(32767)?,
                    version: r.read_string(32767)?,
                });
            }
            Ok(Self::SelectKnownPacks { packs })
        } else if id == packet_ids::configuration::serverbound::FINISH_CONFIGURATION {
            Ok(Self::FinishConfiguration)
        } else if id == packet_ids::configuration::serverbound::KEEP_ALIVE {
            Ok(Self::KeepAlive { id: r.read_i64()? })
        } else if id == packet_ids::configuration::serverbound::PONG {
            Ok(Self::Pong { id: r.read_i32()? })
        } else {
            Ok(Self::Other(id))
        }
    }
}

pub fn encode_config_custom_payload_brand(brand: &str) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::configuration::clientbound::CUSTOM_PAYLOAD);
    w.write_string("minecraft:brand");
    w.write_string(brand);
    w.into_bytes()
}

pub fn encode_config_select_known_packs(packs: &[KnownPack]) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::configuration::clientbound::SELECT_KNOWN_PACKS);
    w.write_varint(packs.len() as i32);
    for p in packs {
        w.write_string(&p.namespace);
        w.write_string(&p.id);
        w.write_string(&p.version);
    }
    w.into_bytes()
}

pub fn encode_config_registry_data(
    registry_id: &str,
    entries: &[PackedRegistryEntry],
) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::with_capacity(256);
    w.write_varint(packet_ids::configuration::clientbound::REGISTRY_DATA);
    w.write_string(registry_id);
    w.write_varint(entries.len() as i32);
    for e in entries {
        w.write_string(&e.id);
        match &e.data {
            Some(tag) => {
                w.write_bool(true);
                w.write_nbt_network(tag)?;
            }
            None => w.write_bool(false),
        }
    }
    Ok(w.into_bytes())
}

pub fn encode_config_update_tags_raw(tags_payload: &[u8]) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(tags_payload.len() + 4);
    w.write_varint(packet_ids::configuration::clientbound::UPDATE_TAGS);
    w.write_raw(tags_payload);
    w.into_bytes()
}

pub fn encode_config_finish() -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::configuration::clientbound::FINISH_CONFIGURATION);
    w.into_bytes()
}

// ======================== Play (Clientbound) ========================

#[derive(Debug, Clone)]
pub struct PlayLoginPacket {
    pub player_id: i32,
    pub hardcore: bool,
    pub levels: Vec<String>,
    pub max_players: i32,
    pub chunk_radius: i32,
    pub simulation_distance: i32,
    pub reduced_debug_info: bool,
    pub show_death_screen: bool,
    pub do_limited_crafting: bool,
    pub dimension_type_id: i32,
    pub dimension_name: String,
    pub hashed_seed: i64,
    pub game_type: i32,
    pub previous_game_type: Option<i32>,
    pub is_debug: bool,
    pub is_flat: bool,
    pub last_death_location: Option<(String, BlockPos)>,
    pub portal_cooldown: i32,
    pub sea_level: i32,
    pub online_mode: bool,
    pub enforces_secure_chat: bool,
}

impl PlayLoginPacket {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = PacketWriter::with_capacity(128);
        w.write_varint(packet_ids::play::clientbound::LOGIN);
        w.write_i32(self.player_id);
        w.write_bool(self.hardcore);
        w.write_varint(self.levels.len() as i32);
        for lvl in &self.levels {
            w.write_string(lvl);
        }
        w.write_varint(self.max_players);
        w.write_varint(self.chunk_radius);
        w.write_varint(self.simulation_distance);
        w.write_bool(self.reduced_debug_info);
        w.write_bool(self.show_death_screen);
        w.write_bool(self.do_limited_crafting);
        // CommonPlayerSpawnInfo
        w.write_varint(self.dimension_type_id);
        w.write_string(&self.dimension_name);
        w.write_i64(self.hashed_seed);
        w.write_varint(self.game_type);
        w.write_optional_varint(self.previous_game_type);
        w.write_bool(self.is_debug);
        w.write_bool(self.is_flat);
        match &self.last_death_location {
            Some((dim, pos)) => {
                w.write_bool(true);
                w.write_string(dim);
                w.write_block_pos(*pos);
            }
            None => w.write_bool(false),
        }
        w.write_varint(self.portal_cooldown);
        w.write_varint(self.sea_level);
        // 26.3 fields
        w.write_bool(self.online_mode);
        w.write_bool(self.enforces_secure_chat);
        w.into_bytes()
    }
}

pub fn encode_play_game_event(event_id: u8, param: f32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::GAME_EVENT);
    w.write_u8(event_id);
    w.write_f32(param);
    w.into_bytes()
}

pub fn encode_play_set_default_spawn(dimension: &str, pos: BlockPos, yaw: f32, pitch: f32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_DEFAULT_SPAWN_POSITION);
    w.write_string(dimension);
    w.write_block_pos(pos);
    w.write_f32(yaw);
    w.write_f32(pitch);
    w.into_bytes()
}

pub fn encode_play_set_chunk_cache_center(cx: i32, cz: i32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_CHUNK_CACHE_CENTER);
    w.write_varint(cx);
    w.write_varint(cz);
    w.into_bytes()
}

pub fn encode_play_chunk_batch_start() -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::CHUNK_BATCH_START);
    w.into_bytes()
}

pub fn encode_play_chunk_batch_finished(batch_size: i32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::CHUNK_BATCH_FINISHED);
    w.write_varint(batch_size);
    w.into_bytes()
}

pub fn encode_play_level_chunk_with_light(chunk: &Chunk) -> Vec<u8> {
    let section_bytes = chunk.encode_network_sections();
    let (sky_mask, block_mask, empty_sky_mask, empty_block_mask, sky_updates, block_updates) =
        chunk.collect_light_data();

    let mut w = PacketWriter::with_capacity(section_bytes.len() + 4096);
    w.write_varint(packet_ids::play::clientbound::LEVEL_CHUNK_WITH_LIGHT);
    w.write_i32(chunk.pos.x);
    w.write_i32(chunk.pos.z);

    // Heightmaps: Map<Heightmap.Types, long[]>
    w.write_varint(chunk.heightmaps.len() as i32);
    for (&hm_type_id, arr) in &chunk.heightmaps {
        w.write_varint(hm_type_id);
        w.write_long_array(arr);
    }

    // Section buffer
    w.write_byte_array(&section_bytes);

    // Block entities (Minecraft 26.3 ClientboundLevelChunkPacketData.BlockEntityInfo)
    let be_list: Vec<_> = chunk
        .block_entities
        .values()
        .filter_map(|be| Some((be, be.protocol_type_id()?)))
        .collect();
    w.write_varint(be_list.len() as i32);
    for (be, type_id) in be_list {
        let packed_xz = (((be.pos.x & 15) << 4) | (be.pos.z & 15)) as u8;
        w.write_u8(packed_xz);
        w.write_i16(be.pos.y as i16);
        w.write_varint(type_id);
        // Empty update tag -> TAG_END (0) matching ByteBufCodecs.OPTIONAL_COMPOUND_TAG
        w.write_u8(0);
    }

    // ClientboundLightUpdatePacketData
    w.write_bitset(&sky_mask);
    w.write_bitset(&block_mask);
    w.write_bitset(&empty_sky_mask);
    w.write_bitset(&empty_block_mask);

    w.write_varint(sky_updates.len() as i32);
    for layer in &sky_updates {
        w.write_byte_array(layer);
    }
    w.write_varint(block_updates.len() as i32);
    for layer in &block_updates {
        w.write_byte_array(layer);
    }

    w.into_bytes()
}

pub fn encode_play_player_position(
    teleport_id: i32,
    pos: Vec3,
    delta: Vec3,
    yaw: f32,
    pitch: f32,
    relative_flags: i32,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::PLAYER_POSITION);
    w.write_varint(teleport_id);
    w.write_vec3(pos);
    w.write_vec3(delta);
    w.write_f32(yaw);
    w.write_f32(pitch);
    w.write_i32(relative_flags);
    w.into_bytes()
}

pub fn encode_play_keep_alive(id: i64) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::KEEP_ALIVE);
    w.write_i64(id);
    w.into_bytes()
}

pub fn encode_play_system_chat(text: &str, overlay: bool) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SYSTEM_CHAT);
    w.write_nbt_network(&text_component(text))?;
    w.write_bool(overlay);
    Ok(w.into_bytes())
}

pub fn encode_play_disconnect(reason: &str) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::DISCONNECT);
    w.write_nbt_network(&text_component(reason))?;
    Ok(w.into_bytes())
}

/// Encode `ClientboundPlayerInfoUpdatePacket` for adding listed players (`ADD_PLAYER | UPDATE_GAME_MODE | UPDATE_LISTED | UPDATE_LATENCY`).
pub fn encode_play_player_info_add(entries: &[(&GameProfile, i32, i32)]) -> Vec<u8> {
    // Action bitmask: ADD_PLAYER(0)=1, UPDATE_GAME_MODE(2)=4, UPDATE_LISTED(3)=8, UPDATE_LATENCY(4)=16 -> 0x1D
    let action_mask: u8 = (1 << 0) | (1 << 2) | (1 << 3) | (1 << 4);
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::PLAYER_INFO_UPDATE);
    w.write_u8(action_mask);
    w.write_varint(entries.len() as i32);
    for &(profile, game_mode, latency) in entries {
        w.write_uuid(profile.uuid);
        // ADD_PLAYER: name + properties
        w.write_string(&profile.name);
        w.write_varint(profile.properties.len() as i32);
        for p in &profile.properties {
            w.write_string(&p.name);
            w.write_string(&p.value);
            match &p.signature {
                Some(sig) => {
                    w.write_bool(true);
                    w.write_string(sig);
                }
                None => w.write_bool(false),
            }
        }
        // UPDATE_GAME_MODE
        w.write_varint(game_mode);
        // UPDATE_LISTED
        w.write_bool(true);
        // UPDATE_LATENCY
        w.write_varint(latency);
    }
    w.into_bytes()
}

pub fn encode_play_player_info_remove(uuids: &[Uuid]) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::PLAYER_INFO_REMOVE);
    w.write_varint(uuids.len() as i32);
    for &u in uuids {
        w.write_uuid(u);
    }
    w.into_bytes()
}

pub fn encode_play_add_entity(
    entity_id: i32,
    uuid: Uuid,
    entity_type_id: i32,
    pos: Vec3,
    movement: Vec3,
    pitch: f32,
    yaw: f32,
    head_yaw: f32,
    data: i32,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::ADD_ENTITY);
    w.write_varint(entity_id);
    w.write_uuid(uuid);
    w.write_varint(entity_type_id);
    w.write_vec3(pos);
    w.write_lp_vec3(movement);
    w.write_i8(pack_degrees(pitch));
    w.write_i8(pack_degrees(yaw));
    w.write_i8(pack_degrees(head_yaw));
    w.write_varint(data);
    w.into_bytes()
}

/// Minecraft 26.3 `ClientboundEntityPositionSyncPacket(int id, PositionPath.Linear(pos), float yRot, float xRot, boolean onGround)`.
pub fn encode_play_entity_position_sync(
    entity_id: i32,
    pos: Vec3,
    yaw: f32,
    pitch: f32,
    on_ground: bool,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::ENTITY_POSITION_SYNC);
    w.write_varint(entity_id);
    // PositionPath.Type.LINEAR = 0, then Vec3
    w.write_varint(0);
    w.write_vec3(pos);
    w.write_f32(yaw);
    w.write_f32(pitch);
    w.write_bool(on_ground);
    w.into_bytes()
}

pub fn encode_play_remove_entities(entity_ids: &[i32]) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::REMOVE_ENTITIES);
    w.write_varint(entity_ids.len() as i32);
    for &id in entity_ids {
        w.write_varint(id);
    }
    w.into_bytes()
}

/// Synched entity metadata entry matching Minecraft 26.3's `EntityDataSerializers` IDs.
#[derive(Debug, Clone, PartialEq)]
pub enum EntityDataValue {
    Byte(u8, i8),             // serializer 0
    Int(u8, i32),             // serializer 1
    Float(u8, f32),           // serializer 3
    ItemStack(u8, ItemStack), // serializer 7
    Boolean(u8, bool),        // serializer 8
}

pub fn encode_play_set_entity_data(entity_id: i32, entries: &[EntityDataValue]) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_ENTITY_DATA);
    w.write_varint(entity_id);
    for entry in entries {
        match entry {
            EntityDataValue::Byte(idx, v) => {
                w.write_u8(*idx);
                w.write_varint(0);
                w.write_i8(*v);
            }
            EntityDataValue::Int(idx, v) => {
                w.write_u8(*idx);
                w.write_varint(1);
                w.write_varint(*v);
            }
            EntityDataValue::Float(idx, v) => {
                w.write_u8(*idx);
                w.write_varint(3);
                w.write_f32(*v);
            }
            EntityDataValue::ItemStack(idx, stack) => {
                w.write_u8(*idx);
                w.write_varint(7);
                write_item_stack_network(&mut w, stack);
            }
            EntityDataValue::Boolean(idx, v) => {
                w.write_u8(*idx);
                w.write_varint(8);
                w.write_bool(*v);
            }
        }
    }
    w.write_u8(255); // EOF_MARKER
    w.into_bytes()
}

pub fn encode_play_take_item_entity(item_id: i32, player_id: i32, amount: i32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::TAKE_ITEM_ENTITY);
    w.write_varint(item_id);
    w.write_varint(player_id);
    w.write_varint(amount);
    w.into_bytes()
}

pub fn encode_play_set_entity_motion(entity_id: i32, velocity: Vec3) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_ENTITY_MOTION);
    w.write_varint(entity_id);
    w.write_lp_vec3(velocity);
    w.into_bytes()
}

pub fn encode_play_block_update(pos: BlockPos, state_id: u32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::BLOCK_UPDATE);
    w.write_block_pos(pos);
    w.write_varint(state_id as i32);
    w.into_bytes()
}

pub fn encode_play_block_changed_ack(sequence: i32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::BLOCK_CHANGED_ACK);
    w.write_varint(sequence);
    w.into_bytes()
}

pub fn encode_play_light_update(chunk: &Chunk, sky_filter: u64, block_filter: u64) -> Vec<u8> {
    let (sky_mask, block_mask, empty_sky_mask, empty_block_mask, sky_updates, block_updates) =
        chunk.collect_light_data_for_mask(sky_filter, block_filter);
    let mut w = PacketWriter::with_capacity(2048);
    w.write_varint(packet_ids::play::clientbound::LIGHT_UPDATE);
    w.write_varint(chunk.pos.x);
    w.write_varint(chunk.pos.z);
    w.write_bitset(&sky_mask);
    w.write_bitset(&block_mask);
    w.write_bitset(&empty_sky_mask);
    w.write_bitset(&empty_block_mask);
    w.write_varint(sky_updates.len() as i32);
    for layer in &sky_updates {
        w.write_byte_array(layer);
    }
    w.write_varint(block_updates.len() as i32);
    for layer in &block_updates {
        w.write_byte_array(layer);
    }
    w.into_bytes()
}

pub fn encode_play_set_held_slot(slot: i32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_HELD_SLOT);
    w.write_varint(slot);
    w.into_bytes()
}

pub fn encode_play_container_set_slot(
    container_id: i32,
    state_id: i32,
    slot: i16,
    stack: &ItemStack,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::CONTAINER_SET_SLOT);
    w.write_varint(container_id);
    w.write_varint(state_id);
    w.write_i16(slot);
    write_item_stack_network(&mut w, stack);
    w.into_bytes()
}

pub fn encode_play_container_set_content(
    container_id: i32,
    state_id: i32,
    items: &[ItemStack],
    carried: &ItemStack,
) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(items.len() * 8 + 16);
    w.write_varint(packet_ids::play::clientbound::CONTAINER_SET_CONTENT);
    w.write_varint(container_id);
    w.write_varint(state_id);
    w.write_varint(items.len() as i32);
    for item in items {
        write_item_stack_network(&mut w, item);
    }
    write_item_stack_network(&mut w, carried);
    w.into_bytes()
}

pub fn encode_play_open_screen(
    container_id: i32,
    menu_type_id: i32,
    title: &str,
) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::OPEN_SCREEN);
    w.write_varint(container_id);
    w.write_varint(menu_type_id);
    w.write_nbt_network(&text_component(title))?;
    Ok(w.into_bytes())
}

pub fn encode_play_container_set_data(container_id: i32, id: i16, value: i16) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::CONTAINER_SET_DATA);
    w.write_varint(container_id);
    w.write_i16(id);
    w.write_i16(value);
    w.into_bytes()
}

pub fn encode_play_container_close(container_id: i32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::CONTAINER_CLOSE);
    w.write_varint(container_id);
    w.into_bytes()
}

pub fn encode_play_block_entity_data(
    pos: BlockPos,
    block_entity_type_id: i32,
    tag: &NbtCompound,
) -> Result<Vec<u8>, ProtoError> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::BLOCK_ENTITY_DATA);
    w.write_block_pos(pos);
    w.write_varint(block_entity_type_id);
    w.write_nbt_network(&NbtTag::Compound(tag.clone()))?;
    Ok(w.into_bytes())
}

pub fn encode_play_set_health(health: f32, food: i32, saturation: f32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_HEALTH);
    w.write_f32(health);
    w.write_varint(food);
    w.write_f32(saturation);
    w.into_bytes()
}

pub fn encode_play_hurt_animation(entity_id: i32, yaw: f32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::HURT_ANIMATION);
    w.write_varint(entity_id);
    w.write_f32(yaw);
    w.into_bytes()
}

pub fn encode_play_entity_event(entity_id: i32, event_id: i8) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::ENTITY_EVENT);
    w.write_i32(entity_id);
    w.write_i8(event_id);
    w.into_bytes()
}

#[derive(Debug, Clone)]
pub struct PlayRespawnPacket {
    pub dimension_type_id: i32,
    pub dimension_name: String,
    pub hashed_seed: i64,
    pub game_type: i32,
    pub previous_game_type: Option<i32>,
    pub is_debug: bool,
    pub is_flat: bool,
    pub last_death_location: Option<(String, BlockPos)>,
    pub portal_cooldown: i32,
    pub sea_level: i32,
    pub data_to_keep: u8,
}

impl PlayRespawnPacket {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = PacketWriter::with_capacity(64);
        w.write_varint(packet_ids::play::clientbound::RESPAWN);
        w.write_varint(self.dimension_type_id);
        w.write_string(&self.dimension_name);
        w.write_i64(self.hashed_seed);
        w.write_varint(self.game_type);
        w.write_optional_varint(self.previous_game_type);
        w.write_bool(self.is_debug);
        w.write_bool(self.is_flat);
        match &self.last_death_location {
            Some((dim, pos)) => {
                w.write_bool(true);
                w.write_string(dim);
                w.write_block_pos(*pos);
            }
            None => w.write_bool(false),
        }
        w.write_varint(self.portal_cooldown);
        w.write_varint(self.sea_level);
        w.write_u8(self.data_to_keep);
        w.into_bytes()
    }
}

pub fn encode_play_player_abilities(
    invulnerable: bool,
    flying: bool,
    may_fly: bool,
    instabuild: bool,
    flying_speed: f32,
    walking_speed: f32,
) -> Vec<u8> {
    let mut flags = 0u8;
    if invulnerable {
        flags |= 1;
    }
    if flying {
        flags |= 2;
    }
    if may_fly {
        flags |= 4;
    }
    if instabuild {
        flags |= 8;
    }
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::PLAYER_ABILITIES);
    w.write_u8(flags);
    w.write_f32(flying_speed);
    w.write_f32(walking_speed);
    w.into_bytes()
}

/// Minecraft 26.3 `ClientboundSetTimePacket(long gameTime, Map<Holder<WorldClock>, ClockNetworkState>)`.
/// Each clock tuple is `(world_clock_registry_id, total_ticks, partial_tick, rate)`.
pub fn encode_play_set_time(game_time: i64, clocks: &[(i32, i64, f32, f32)]) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::clientbound::SET_TIME);
    w.write_i64(game_time);
    w.write_varint(clocks.len() as i32);
    for &(clock_id, total_ticks, partial_tick, rate) in clocks {
        w.write_varint(clock_id);
        w.write_varlong(total_ticks);
        w.write_f32(partial_tick);
        w.write_f32(rate);
    }
    w.into_bytes()
}

/// Encode `ClientboundCommandsPacket` matching Minecraft 26.3's `BuiltInRegistries.COMMAND_ARGUMENT_TYPE`.
pub fn encode_play_commands(entries: &[CommandNodeEntry], root_index: i32) -> Vec<u8> {
    let mut w = PacketWriter::with_capacity(entries.len() * 16 + 8);
    w.write_varint(packet_ids::play::clientbound::COMMANDS);
    w.write_varint(entries.len() as i32);
    for entry in entries {
        let mut flags: u8 = match entry.kind {
            CommandNodeKind::Root => 0,
            CommandNodeKind::Literal(_) => 1,
            CommandNodeKind::Argument { .. } => 2,
        };
        if entry.executable {
            flags |= 4;
        }
        w.write_u8(flags);
        w.write_varint(entry.children.len() as i32);
        for &child in &entry.children {
            w.write_varint(child);
        }
        match &entry.kind {
            CommandNodeKind::Root => {}
            CommandNodeKind::Literal(name) => {
                w.write_string(name);
            }
            CommandNodeKind::Argument { name, arg_type } => {
                w.write_string(name);
                match arg_type {
                    CommandArgType::SingleWord => {
                        w.write_varint(5); // brigadier:string
                        w.write_varint(0);
                    }
                    CommandArgType::QuotablePhrase => {
                        w.write_varint(5);
                        w.write_varint(1);
                    }
                    CommandArgType::GreedyPhrase => {
                        w.write_varint(5);
                        w.write_varint(2);
                    }
                    CommandArgType::Integer { min, max } => {
                        w.write_varint(3); // brigadier:integer
                        let mut b = 0u8;
                        if min.is_some() {
                            b |= 1;
                        }
                        if max.is_some() {
                            b |= 2;
                        }
                        w.write_u8(b);
                        if let Some(lo) = min {
                            w.write_i32(*lo);
                        }
                        if let Some(hi) = max {
                            w.write_i32(*hi);
                        }
                    }
                    CommandArgType::BlockPos => w.write_varint(8),
                    CommandArgType::Vec3 => w.write_varint(10),
                    CommandArgType::BlockState => w.write_varint(12),
                    CommandArgType::ItemStack => w.write_varint(14),
                    CommandArgType::GameMode => w.write_varint(42),
                }
            }
        }
    }
    w.write_varint(root_index);
    w.into_bytes()
}

/// Minecraft 26.3 `HashedStack` sent in `ServerboundContainerClickPacket`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashedStack {
    Empty,
    Actual {
        item_id: i32,
        count: i32,
        added_hashes: Vec<(i32, i32)>,
        removed_components: Vec<i32>,
    },
}

fn read_hashed_stack(r: &mut PacketReader<'_>) -> Result<HashedStack, ProtoError> {
    if !r.read_bool()? {
        return Ok(HashedStack::Empty);
    }
    let item_id = r.read_varint()?;
    let count = r.read_varint()?;
    let added_len = r.read_varint()? as usize;
    if added_len > 256 {
        return Err(ProtoError::InvalidValue(added_len as i32));
    }
    let mut added_hashes = Vec::with_capacity(added_len);
    for _ in 0..added_len {
        let comp_id = r.read_varint()?;
        let hash = r.read_i32()?;
        added_hashes.push((comp_id, hash));
    }
    let removed_len = r.read_varint()? as usize;
    if removed_len > 256 {
        return Err(ProtoError::InvalidValue(removed_len as i32));
    }
    let mut removed_components = Vec::with_capacity(removed_len);
    for _ in 0..removed_len {
        removed_components.push(r.read_varint()?);
    }
    Ok(HashedStack::Actual {
        item_id,
        count,
        added_hashes,
        removed_components,
    })
}

fn write_hashed_stack(w: &mut PacketWriter, hs: &HashedStack) {
    match hs {
        HashedStack::Empty => w.write_bool(false),
        HashedStack::Actual {
            item_id,
            count,
            added_hashes,
            removed_components,
        } => {
            w.write_bool(true);
            w.write_varint(*item_id);
            w.write_varint(*count);
            w.write_varint(added_hashes.len() as i32);
            for &(cid, h) in added_hashes {
                w.write_varint(cid);
                w.write_i32(h);
            }
            w.write_varint(removed_components.len() as i32);
            for &cid in removed_components {
                w.write_varint(cid);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientCommandAction {
    PerformRespawn = 0,
    RequestStats = 1,
    RequestGameruleValues = 2,
}

impl ClientCommandAction {
    pub fn from_id(id: i32) -> Result<Self, ProtoError> {
        match id {
            0 => Ok(Self::PerformRespawn),
            1 => Ok(Self::RequestStats),
            2 => Ok(Self::RequestGameruleValues),
            other => Err(ProtoError::InvalidValue(other)),
        }
    }
}

// ======================== Play (Serverbound) ========================

#[derive(Debug, Clone, PartialEq)]
pub enum PlayServerbound {
    AcceptTeleportation {
        id: i32,
    },
    KeepAlive {
        id: i64,
    },
    MovePlayerPos {
        x: f64,
        y: f64,
        z: f64,
        on_ground: bool,
        horizontal_collision: bool,
    },
    MovePlayerPosRot {
        x: f64,
        y: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
        on_ground: bool,
        horizontal_collision: bool,
    },
    MovePlayerRot {
        yaw: f32,
        pitch: f32,
        on_ground: bool,
        horizontal_collision: bool,
    },
    MovePlayerStatusOnly {
        on_ground: bool,
        horizontal_collision: bool,
    },
    Chat {
        message: String,
    },
    ChatCommand {
        command: String,
    },
    PlayerAction {
        action: PlayerActionType,
        pos: BlockPos,
        direction: Direction,
        sequence: i32,
    },
    UseItemOn {
        hand: InteractionHand,
        hit: BlockHitResult,
        sequence: i32,
    },
    UseItem {
        hand: InteractionHand,
        sequence: i32,
        yaw: f32,
        pitch: f32,
    },
    SetCarriedItem {
        slot: i16,
    },
    SetCreativeModeSlot {
        slot: i16,
        stack: ItemStack,
    },
    Interact {
        entity_id: i32,
        hand: InteractionHand,
        location: Vec3,
        using_secondary_action: bool,
    },
    Attack {
        entity_id: i32,
    },
    ContainerClick {
        container_id: i32,
        state_id: i32,
        slot_num: i16,
        button_num: i8,
        click_type: ContainerClickType,
        changed_slots: Vec<(i16, HashedStack)>,
        carried_item: HashedStack,
    },
    ContainerClose {
        container_id: i32,
    },
    ClientCommand {
        action: ClientCommandAction,
    },
    ChunkBatchReceived {
        desired_chunks_per_tick: f32,
    },
    PlayerLoaded,
    Other(i32),
}

impl PlayServerbound {
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtoError> {
        let mut r = PacketReader::new(bytes);
        let id = r.read_varint()?;
        if id == packet_ids::play::serverbound::ACCEPT_TELEPORTATION {
            Ok(Self::AcceptTeleportation { id: r.read_varint()? })
        } else if id == packet_ids::play::serverbound::KEEP_ALIVE {
            Ok(Self::KeepAlive { id: r.read_i64()? })
        } else if id == packet_ids::play::serverbound::MOVE_PLAYER_POS {
            let x = r.read_f64()?;
            let y = r.read_f64()?;
            let z = r.read_f64()?;
            let flags = r.read_u8()?;
            Ok(Self::MovePlayerPos {
                x,
                y,
                z,
                on_ground: (flags & 1) != 0,
                horizontal_collision: (flags & 2) != 0,
            })
        } else if id == packet_ids::play::serverbound::MOVE_PLAYER_POS_ROT {
            let x = r.read_f64()?;
            let y = r.read_f64()?;
            let z = r.read_f64()?;
            let yaw = r.read_f32()?;
            let pitch = r.read_f32()?;
            let flags = r.read_u8()?;
            Ok(Self::MovePlayerPosRot {
                x,
                y,
                z,
                yaw,
                pitch,
                on_ground: (flags & 1) != 0,
                horizontal_collision: (flags & 2) != 0,
            })
        } else if id == packet_ids::play::serverbound::MOVE_PLAYER_ROT {
            let yaw = r.read_f32()?;
            let pitch = r.read_f32()?;
            let flags = r.read_u8()?;
            Ok(Self::MovePlayerRot {
                yaw,
                pitch,
                on_ground: (flags & 1) != 0,
                horizontal_collision: (flags & 2) != 0,
            })
        } else if id == packet_ids::play::serverbound::MOVE_PLAYER_STATUS_ONLY {
            let flags = r.read_u8()?;
            Ok(Self::MovePlayerStatusOnly {
                on_ground: (flags & 1) != 0,
                horizontal_collision: (flags & 2) != 0,
            })
        } else if id == packet_ids::play::serverbound::CHAT {
            let message = r.read_string(256)?;
            Ok(Self::Chat { message })
        } else if id == packet_ids::play::serverbound::CHAT_COMMAND
            || id == packet_ids::play::serverbound::CHAT_COMMAND_SIGNED
        {
            let command = r.read_string(32767)?;
            Ok(Self::ChatCommand { command })
        } else if id == packet_ids::play::serverbound::PLAYER_ACTION {
            let action = PlayerActionType::from_id(r.read_varint()?)?;
            let pos = r.read_block_pos()?;
            let direction = Direction::from_id(r.read_u8()? as i32)?;
            let sequence = r.read_varint()?;
            Ok(Self::PlayerAction {
                action,
                pos,
                direction,
                sequence,
            })
        } else if id == packet_ids::play::serverbound::USE_ITEM_ON {
            let hand = InteractionHand::from_id(r.read_varint()?)?;
            let pos = r.read_block_pos()?;
            let direction = Direction::from_id(r.read_varint()?)?;
            let click_x = r.read_f32()?;
            let click_y = r.read_f32()?;
            let click_z = r.read_f32()?;
            let inside = r.read_bool()?;
            let world_border_hit = r.read_bool()?;
            let sequence = r.read_varint()?;
            Ok(Self::UseItemOn {
                hand,
                hit: BlockHitResult {
                    pos,
                    direction,
                    click_x,
                    click_y,
                    click_z,
                    inside,
                    world_border_hit,
                },
                sequence,
            })
        } else if id == packet_ids::play::serverbound::USE_ITEM {
            let hand = InteractionHand::from_id(r.read_varint()?)?;
            let sequence = r.read_varint()?;
            let yaw = r.read_f32()?;
            let pitch = r.read_f32()?;
            Ok(Self::UseItem {
                hand,
                sequence,
                yaw,
                pitch,
            })
        } else if id == packet_ids::play::serverbound::SET_CARRIED_ITEM {
            Ok(Self::SetCarriedItem {
                slot: r.read_i16()?,
            })
        } else if id == packet_ids::play::serverbound::SET_CREATIVE_MODE_SLOT {
            let slot = r.read_i16()?;
            let stack = read_item_stack_untrusted(&mut r)?;
            Ok(Self::SetCreativeModeSlot { slot, stack })
        } else if id == packet_ids::play::serverbound::INTERACT {
            let entity_id = r.read_varint()?;
            let hand = InteractionHand::from_id(r.read_varint()?)?;
            let location = r.read_lp_vec3()?;
            let using_secondary_action = r.read_bool()?;
            Ok(Self::Interact {
                entity_id,
                hand,
                location,
                using_secondary_action,
            })
        } else if id == packet_ids::play::serverbound::ATTACK {
            Ok(Self::Attack {
                entity_id: r.read_varint()?,
            })
        } else if id == packet_ids::play::serverbound::CONTAINER_CLICK {
            let container_id = r.read_varint()?;
            let state_id = r.read_varint()?;
            let slot_num = r.read_i16()?;
            let button_num = r.read_i8()?;
            let click_id = r.read_varint()?;
            let click_type =
                ContainerClickType::from_id(click_id).ok_or(ProtoError::InvalidValue(click_id))?;
            let changed_len = r.read_varint()? as usize;
            if changed_len > 128 {
                return Err(ProtoError::InvalidValue(changed_len as i32));
            }
            let mut changed_slots = Vec::with_capacity(changed_len);
            for _ in 0..changed_len {
                let s = r.read_i16()?;
                let hs = read_hashed_stack(&mut r)?;
                changed_slots.push((s, hs));
            }
            let carried_item = read_hashed_stack(&mut r)?;
            Ok(Self::ContainerClick {
                container_id,
                state_id,
                slot_num,
                button_num,
                click_type,
                changed_slots,
                carried_item,
            })
        } else if id == packet_ids::play::serverbound::CONTAINER_CLOSE {
            Ok(Self::ContainerClose {
                container_id: r.read_varint()?,
            })
        } else if id == packet_ids::play::serverbound::CLIENT_COMMAND {
            Ok(Self::ClientCommand {
                action: ClientCommandAction::from_id(r.read_varint()?)?,
            })
        } else if id == packet_ids::play::serverbound::CHUNK_BATCH_RECEIVED {
            Ok(Self::ChunkBatchReceived {
                desired_chunks_per_tick: r.read_f32()?,
            })
        } else if id == packet_ids::play::serverbound::PLAYER_LOADED {
            Ok(Self::PlayerLoaded)
        } else {
            Ok(Self::Other(id))
        }
    }
}

pub fn encode_serverbound_player_action(
    action: PlayerActionType,
    pos: BlockPos,
    direction: Direction,
    sequence: i32,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::PLAYER_ACTION);
    w.write_varint(action as i32);
    w.write_block_pos(pos);
    w.write_u8(direction as u8);
    w.write_varint(sequence);
    w.into_bytes()
}

pub fn encode_serverbound_use_item_on(
    hand: InteractionHand,
    hit: BlockHitResult,
    sequence: i32,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::USE_ITEM_ON);
    w.write_varint(hand as i32);
    w.write_block_pos(hit.pos);
    w.write_varint(hit.direction as i32);
    w.write_f32(hit.click_x);
    w.write_f32(hit.click_y);
    w.write_f32(hit.click_z);
    w.write_bool(hit.inside);
    w.write_bool(hit.world_border_hit);
    w.write_varint(sequence);
    w.into_bytes()
}

pub fn encode_serverbound_use_item(
    hand: InteractionHand,
    sequence: i32,
    yaw: f32,
    pitch: f32,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::USE_ITEM);
    w.write_varint(hand as i32);
    w.write_varint(sequence);
    w.write_f32(yaw);
    w.write_f32(pitch);
    w.into_bytes()
}

pub fn encode_serverbound_set_carried_item(slot: i16) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::SET_CARRIED_ITEM);
    w.write_i16(slot);
    w.into_bytes()
}

pub fn encode_serverbound_set_creative_mode_slot(slot: i16, stack: &ItemStack) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::SET_CREATIVE_MODE_SLOT);
    w.write_i16(slot);
    write_item_stack_untrusted(&mut w, stack);
    w.into_bytes()
}

pub fn encode_serverbound_chat_command(command: &str) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::CHAT_COMMAND);
    w.write_string(command);
    w.into_bytes()
}

pub fn encode_serverbound_interact(
    entity_id: i32,
    hand: InteractionHand,
    location: Vec3,
    using_secondary_action: bool,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::INTERACT);
    w.write_varint(entity_id);
    w.write_varint(hand as i32);
    w.write_lp_vec3(location);
    w.write_bool(using_secondary_action);
    w.into_bytes()
}

pub fn encode_serverbound_attack(entity_id: i32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::ATTACK);
    w.write_varint(entity_id);
    w.into_bytes()
}

pub fn encode_serverbound_container_click(
    container_id: i32,
    state_id: i32,
    slot_num: i16,
    button_num: i8,
    click_type: ContainerClickType,
    changed_slots: &[(i16, HashedStack)],
    carried_item: &HashedStack,
) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::CONTAINER_CLICK);
    w.write_varint(container_id);
    w.write_varint(state_id);
    w.write_i16(slot_num);
    w.write_i8(button_num);
    w.write_varint(click_type as i32);
    w.write_varint(changed_slots.len() as i32);
    for (s, hs) in changed_slots {
        w.write_i16(*s);
        write_hashed_stack(&mut w, hs);
    }
    write_hashed_stack(&mut w, carried_item);
    w.into_bytes()
}

pub fn encode_serverbound_container_close(container_id: i32) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::CONTAINER_CLOSE);
    w.write_varint(container_id);
    w.into_bytes()
}

pub fn encode_serverbound_client_command(action: ClientCommandAction) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::CLIENT_COMMAND);
    w.write_varint(action as i32);
    w.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_data::{ENTITY_TYPE_PLAYER_ID, PROTOCOL_VERSION};
    use mc_math::ChunkPos;
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
    fn diff_all_mvp_protocol_777_packets_against_java_oracle() {
        // 1. Handshake
        let hs = HandshakeIntention {
            protocol_version: PROTOCOL_VERSION,
            host_name: "localhost".into(),
            port: 25565,
            intent: ClientIntent::Login,
        };
        assert_oracle_roundtrip("handshake", "serverbound", "minecraft:intention", &hs.encode());

        // 2. Status
        let status_json = r#"{"description":{"text":"SiliconMC Rust 26.3"},"players":{"max":20,"online":0},"version":{"name":"26.3","protocol":777}}"#;
        let status_bytes = encode_status_response(status_json);
        let (ptype, _, _) = with_oracle(|o| o.packet_roundtrip("status", "clientbound", &status_bytes)).unwrap();
        assert_eq!(ptype, "minecraft:status_response");
        assert_oracle_roundtrip("status", "clientbound", "minecraft:pong_response", &encode_status_pong(123456789));

        // 3. Login
        assert_oracle_roundtrip(
            "login",
            "clientbound",
            "minecraft:hello",
            &encode_login_hello("", &[1, 2, 3, 4], &[9, 8, 7, 6], true),
        );
        assert_oracle_roundtrip(
            "login",
            "clientbound",
            "minecraft:login_compression",
            &encode_login_compression(256),
        );
        let profile = GameProfile {
            uuid: Uuid::from_u128(0x01234567_89ab_cdef_0123_456789abcdef),
            name: "Steve".into(),
            properties: vec![],
        };
        assert_oracle_roundtrip(
            "login",
            "clientbound",
            "minecraft:login_finished",
            &encode_login_finished(&profile, Uuid::from_u128(0xdeadbeef_0000_4000_8000_112233445566)),
        );

        // 4. Configuration
        assert_oracle_roundtrip(
            "configuration",
            "clientbound",
            "minecraft:select_known_packs",
            &encode_config_select_known_packs(&[KnownPack {
                namespace: "minecraft".into(),
                id: "core".into(),
                version: "26.3".into(),
            }]),
        );
        assert_oracle_roundtrip(
            "configuration",
            "clientbound",
            "minecraft:registry_data",
            &encode_config_registry_data(
                "minecraft:dimension_type",
                &[PackedRegistryEntry {
                    id: "minecraft:overworld".into(),
                    data: None,
                }],
            )
            .unwrap(),
        );
        assert_oracle_roundtrip(
            "configuration",
            "clientbound",
            "minecraft:finish_configuration",
            &encode_config_finish(),
        );

        // 5. Play (Clientbound)
        let login = PlayLoginPacket {
            player_id: 1,
            hardcore: false,
            levels: vec!["minecraft:overworld".into()],
            max_players: 20,
            chunk_radius: 10,
            simulation_distance: 10,
            reduced_debug_info: false,
            show_death_screen: true,
            do_limited_crafting: false,
            dimension_type_id: 0,
            dimension_name: "minecraft:overworld".into(),
            hashed_seed: 0x1234_5678_9abc_def0,
            game_type: 1,
            previous_game_type: None,
            is_debug: false,
            is_flat: true,
            last_death_location: None,
            portal_cooldown: 0,
            sea_level: 63,
            online_mode: false,
            enforces_secure_chat: false,
        };
        assert_oracle_roundtrip("play", "clientbound", "minecraft:login", &login.encode());
        assert_oracle_roundtrip("play", "clientbound", "minecraft:game_event", &encode_play_game_event(13, 0.0));
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_default_spawn_position",
            &encode_play_set_default_spawn("minecraft:overworld", BlockPos::new(0, -60, 0), 0.0, 0.0),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_chunk_cache_center",
            &encode_play_set_chunk_cache_center(0, 0),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:chunk_batch_start",
            &encode_play_chunk_batch_start(),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:chunk_batch_finished",
            &encode_play_chunk_batch_finished(9),
        );
        let flat_chunk = Chunk::new_flat_overworld(ChunkPos::new(0, 0), 0);
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:level_chunk_with_light",
            &encode_play_level_chunk_with_light(&flat_chunk),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:player_position",
            &encode_play_player_position(1, Vec3::new(0.5, -60.0, 0.5), Vec3::ZERO, 90.0, 0.0, 0),
        );
        assert_oracle_roundtrip("play", "clientbound", "minecraft:keep_alive", &encode_play_keep_alive(99999));
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:system_chat",
            &encode_play_system_chat("Welcome to SiliconMC Rust!", false).unwrap(),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:player_info_update",
            &encode_play_player_info_add(&[(&profile, 1, 0)]),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:player_info_remove",
            &encode_play_player_info_remove(&[profile.uuid]),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:add_entity",
            &encode_play_add_entity(
                2,
                profile.uuid,
                ENTITY_TYPE_PLAYER_ID,
                Vec3::new(1.5, -60.0, -2.5),
                Vec3::new(0.1, 0.0, -0.2),
                15.0,
                -45.0,
                -45.0,
                0,
            ),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:entity_position_sync",
            &encode_play_entity_position_sync(2, Vec3::new(2.0, -60.0, -3.0), 45.0, 10.0, true),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:remove_entities",
            &encode_play_remove_entities(&[2, 3]),
        );

        // 6. Phase 3 Play (Clientbound)
        let stone_item = ItemStack::from_name("minecraft:stone", 32).unwrap();
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:block_update",
            &encode_play_block_update(BlockPos::new(4, -60, -3), mc_data::BlockStateId::STONE.0),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:block_changed_ack",
            &encode_play_block_changed_ack(42),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:light_update",
            &encode_play_light_update(&flat_chunk, 0x3, 0x0),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_held_slot",
            &encode_play_set_held_slot(3),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:container_set_slot",
            &encode_play_container_set_slot(0, 7, 36, &stone_item),
        );
        let mut inv_slots = vec![ItemStack::EMPTY; 46];
        inv_slots[36] = stone_item.clone();
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:container_set_content",
            &encode_play_container_set_content(0, 8, &inv_slots, &ItemStack::EMPTY),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:player_abilities",
            &encode_play_player_abilities(true, false, true, true, 0.05, 0.1),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_time",
            &encode_play_set_time(12345, &[(0, 6000, 0.0, 1.0)]),
        );
        let cmd_nodes = vec![
            CommandNodeEntry::root(vec![1, 3]),
            CommandNodeEntry::literal("gamemode", false, vec![2]),
            CommandNodeEntry::argument("gm", CommandArgType::GameMode, true, vec![]),
            CommandNodeEntry::literal("give", false, vec![4]),
            CommandNodeEntry::argument("item", CommandArgType::ItemStack, true, vec![5]),
            CommandNodeEntry::argument(
                "count",
                CommandArgType::Integer {
                    min: Some(1),
                    max: Some(64),
                },
                true,
                vec![],
            ),
        ];
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:commands",
            &encode_play_commands(&cmd_nodes, 0),
        );

        // 7. Phase 3 Play (Serverbound)
        let pa = encode_serverbound_player_action(
            PlayerActionType::StartDestroyBlock,
            BlockPos::new(1, -61, 2),
            Direction::Up,
            9,
        );
        assert_oracle_roundtrip("play", "serverbound", "minecraft:player_action", &pa);
        assert!(matches!(
            PlayServerbound::decode(&pa).unwrap(),
            PlayServerbound::PlayerAction {
                action: PlayerActionType::StartDestroyBlock,
                direction: Direction::Up,
                sequence: 9,
                ..
            }
        ));

        let use_on = encode_serverbound_use_item_on(
            InteractionHand::MainHand,
            BlockHitResult {
                pos: BlockPos::new(1, -61, 2),
                direction: Direction::Up,
                click_x: 0.5,
                click_y: 1.0,
                click_z: 0.5,
                inside: false,
                world_border_hit: false,
            },
            10,
        );
        assert_oracle_roundtrip("play", "serverbound", "minecraft:use_item_on", &use_on);
        assert!(matches!(
            PlayServerbound::decode(&use_on).unwrap(),
            PlayServerbound::UseItemOn {
                hand: InteractionHand::MainHand,
                sequence: 10,
                ..
            }
        ));

        let carried = encode_serverbound_set_carried_item(4);
        assert_oracle_roundtrip("play", "serverbound", "minecraft:set_carried_item", &carried);
        assert_eq!(
            PlayServerbound::decode(&carried).unwrap(),
            PlayServerbound::SetCarriedItem { slot: 4 }
        );

        let creative = encode_serverbound_set_creative_mode_slot(36, &stone_item);
        assert_oracle_roundtrip(
            "play",
            "serverbound",
            "minecraft:set_creative_mode_slot",
            &creative,
        );
        assert_eq!(
            PlayServerbound::decode(&creative).unwrap(),
            PlayServerbound::SetCreativeModeSlot {
                slot: 36,
                stack: stone_item
            }
        );

        let chat_cmd = encode_serverbound_chat_command("tp 0 -60 0");
        assert_oracle_roundtrip("play", "serverbound", "minecraft:chat_command", &chat_cmd);
        assert_eq!(
            PlayServerbound::decode(&chat_cmd).unwrap(),
            PlayServerbound::ChatCommand {
                command: "tp 0 -60 0".into()
            }
        );

        // 8. Phase 5 Play Entity Packets (Clientbound + Serverbound)
        let meta_item = ItemStack::from_name("minecraft:diamond", 3).unwrap();
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_entity_data",
            &encode_play_set_entity_data(
                77,
                &[
                    EntityDataValue::Byte(0, 0),
                    EntityDataValue::ItemStack(8, meta_item),
                ],
            ),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:take_item_entity",
            &encode_play_take_item_entity(77, 1, 3),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_entity_motion",
            &encode_play_set_entity_motion(77, Vec3::new(0.15, 0.25, -0.1)),
        );
        let atk = encode_serverbound_attack(77);
        assert_oracle_roundtrip("play", "serverbound", "minecraft:attack", &atk);
        assert_eq!(
            PlayServerbound::decode(&atk).unwrap(),
            PlayServerbound::Attack { entity_id: 77 }
        );
        let inter = encode_serverbound_interact(
            77,
            InteractionHand::MainHand,
            Vec3::new(0.2, 0.5, -0.1),
            false,
        );
        assert_oracle_roundtrip("play", "serverbound", "minecraft:interact", &inter);
        assert!(matches!(
            PlayServerbound::decode(&inter).unwrap(),
            PlayServerbound::Interact {
                entity_id: 77,
                hand: InteractionHand::MainHand,
                using_secondary_action: false,
                ..
            }
        ));

        // 9. Phase 5c Container + Survival Protocol 777 Packets (Clientbound + Serverbound)
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:open_screen",
            &encode_play_open_screen(1, mc_data::recipes::menu_types::GENERIC_9X3, "Chest").unwrap(),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:container_set_data",
            &encode_play_container_set_data(2, 0, 1600),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:container_close",
            &encode_play_container_close(2),
        );
        let mut be_tag = NbtCompound::new();
        be_tag.insert("id", NbtTag::String("minecraft:chest".into()));
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:block_entity_data",
            &encode_play_block_entity_data(
                BlockPos::new(5, -60, 5),
                mc_data::recipes::block_entity_types::CHEST,
                &be_tag,
            )
            .unwrap(),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:set_health",
            &encode_play_set_health(14.5, 18, 3.25),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:hurt_animation",
            &encode_play_hurt_animation(1, 90.0),
        );
        assert_oracle_roundtrip(
            "play",
            "clientbound",
            "minecraft:entity_event",
            &encode_play_entity_event(1, 3),
        );
        let respawn = PlayRespawnPacket {
            dimension_type_id: 0,
            dimension_name: "minecraft:overworld".into(),
            hashed_seed: 0x1234_5678_9abc_def0,
            game_type: 0,
            previous_game_type: Some(0),
            is_debug: false,
            is_flat: true,
            last_death_location: Some(("minecraft:overworld".into(), BlockPos::new(1, -60, 1))),
            portal_cooldown: 0,
            sea_level: 63,
            data_to_keep: 0,
        };
        assert_oracle_roundtrip("play", "clientbound", "minecraft:respawn", &respawn.encode());

        let click = encode_serverbound_container_click(
            1,
            5,
            0,
            0,
            ContainerClickType::Pickup,
            &[(
                0,
                HashedStack::Actual {
                    item_id: 1,
                    count: 16,
                    added_hashes: vec![],
                    removed_components: vec![],
                },
            )],
            &HashedStack::Empty,
        );
        assert_oracle_roundtrip("play", "serverbound", "minecraft:container_click", &click);
        assert!(matches!(
            PlayServerbound::decode(&click).unwrap(),
            PlayServerbound::ContainerClick {
                container_id: 1,
                state_id: 5,
                slot_num: 0,
                click_type: ContainerClickType::Pickup,
                ..
            }
        ));

        let cclose = encode_serverbound_container_close(1);
        assert_oracle_roundtrip("play", "serverbound", "minecraft:container_close", &cclose);
        assert_eq!(
            PlayServerbound::decode(&cclose).unwrap(),
            PlayServerbound::ContainerClose { container_id: 1 }
        );

        let ccmd = encode_serverbound_client_command(ClientCommandAction::PerformRespawn);
        assert_oracle_roundtrip("play", "serverbound", "minecraft:client_command", &ccmd);
        assert_eq!(
            PlayServerbound::decode(&ccmd).unwrap(),
            PlayServerbound::ClientCommand {
                action: ClientCommandAction::PerformRespawn
            }
        );

        let use_item = encode_serverbound_use_item(InteractionHand::MainHand, 77, 45.0, -10.0);
        assert_oracle_roundtrip("play", "serverbound", "minecraft:use_item", &use_item);
        assert_eq!(
            PlayServerbound::decode(&use_item).unwrap(),
            PlayServerbound::UseItem {
                hand: InteractionHand::MainHand,
                sequence: 77,
                yaw: 45.0,
                pitch: -10.0,
            }
        );
    }
}
