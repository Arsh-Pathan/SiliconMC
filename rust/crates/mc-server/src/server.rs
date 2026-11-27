use crate::admin::AdminStore;
use crate::advancements::PlayerAdvancements;
use crate::auth::{ServerKeypair, verify_mojang_session};
use crate::commands::{self, CommandAction};
use crate::config::ServerConfig;
use crate::extra_commands::{BossBarProperty, ExtraCommandAction, FillMode, TitleSlot, parse_extra_command};
use crate::jsonrpc::JsonRpcServer;
use crate::playerdata::{SavedPlayerData, add_experience_points, load_player_data, save_player_data};
use crate::query::{QueryServer, QuerySnapshot};
use crate::rcon::RconServer;
use crate::scoreboard::ScoreboardStore;
use crate::world_loader::{DimensionId, WorldStore};
use anyhow::Result;
use mc_data::block_behaviors::{
    KIND_BARREL, KIND_BLAST_FURNACE, KIND_CHEST, KIND_CRAFTING_TABLE, KIND_END_PORTAL,
    KIND_FURNACE, KIND_HOPPER, KIND_NETHER_PORTAL, KIND_SIGN, KIND_SMOKER, block_behavior,
    resolve_placement,
};
use mc_data::block_extra::{FIRE_STATE_ID, is_tnt};
use mc_data::gameplay_tables::enchantment_by_id;
use mc_data::items::item_by_id;
use mc_data::loot_and_tools::{block_loot, entity_loot, item_combat_stats};
use mc_data::recipes::{block_entity_types, food_info, menu_types};
use mc_data::{
    BlockStateId, ENTITY_TYPE_PLAYER_ID, LoadedDatapacks, PROTOCOL_VERSION, VERSION_NAME,
    static_registry_protocol_id,
};
use mc_math::{BlockPos, ChunkPos, Vec3};
use mc_proto::{
    BossBarColor, BossBarOverlay, ClientCommandAction, ClientIntent, ConfigServerbound, Difficulty,
    DisplaySlot, EntityDataValue, EquipmentSlot, FramedConnection, GameProfile, HandshakeIntention,
    InteractionHand, KnownPack, LoginServerbound, ObjectiveMethod, ObjectiveRenderType,
    PackedRegistryEntry, PlayExtraServerbound, PlayLoginPacket, PlayRespawnPacket, PlayServerbound,
    PlayerActionType, PlayerTeamParams, StatusServerbound, SwingAnimationType, TeamCollisionRule,
    TeamColor, TeamVisibility, encode_config_custom_payload_brand, encode_config_finish,
    encode_config_registry_data, encode_config_select_known_packs, encode_config_update_tags_raw,
    encode_login_compression, encode_login_disconnect, encode_login_finished, encode_login_hello,
    encode_play_add_entity, encode_play_block_changed_ack, encode_play_block_entity_data,
    encode_play_block_update, encode_play_boss_event_add, encode_play_boss_event_remove,
    encode_play_boss_event_update_name, encode_play_boss_event_update_progress,
    encode_play_boss_event_update_style, encode_play_change_difficulty,
    encode_play_chunk_batch_finished, encode_play_chunk_batch_start, encode_play_clear_titles,
    encode_play_container_close, encode_play_container_set_content,
    encode_play_container_set_data, encode_play_container_set_slot, encode_play_disconnect,
    encode_play_entity_event, encode_play_entity_position_sync, encode_play_explode,
    encode_play_game_event, encode_play_hurt_animation, encode_play_keep_alive,
    encode_play_level_chunk_with_light, encode_play_level_event, encode_play_light_update,
    encode_play_open_screen, encode_play_open_sign_editor, encode_play_player_abilities,
    encode_play_player_info_add, encode_play_player_info_remove, encode_play_player_position,
    encode_play_remove_entities, encode_play_remove_mob_effect, encode_play_reset_score,
    encode_play_set_action_bar_text, encode_play_set_chunk_cache_center,
    encode_play_set_default_spawn, encode_play_set_display_objective, encode_play_set_entity_data,
    encode_play_set_entity_motion, encode_play_set_equipment, encode_play_set_experience,
    encode_play_set_health, encode_play_set_held_slot, encode_play_set_objective,
    encode_play_set_player_team_create, encode_play_set_player_team_join,
    encode_play_set_player_team_leave, encode_play_set_player_team_remove, encode_play_set_score,
    encode_play_set_subtitle_text, encode_play_set_time, encode_play_set_title_text,
    encode_play_set_titles_animation, encode_play_swing_animation, encode_play_system_chat,
    encode_play_take_item_entity, encode_play_update_mob_effect, encode_status_pong,
    encode_status_response, offline_player_uuid, text_component,
};
use mc_world::{
    Arena, BlockEntityKind, ContainerClickType, DirtyLightSections, EntityKind, EntityTickOutcome,
    Handle, ItemStack, PlayerInventory, StructureRotation, StructureTemplate, TickScheduler,
    WorldEntity, apply_container_click, compute_and_apply_explosion,
    compute_entity_explosion_knockback, execute_scheduled_tick, interact_block,
    propagate_neighbor_updates, run_chunk_random_ticks, trigger_pressure_plate,
    try_ignite_nether_portal,
};
use rand::RngCore;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, warn};
use uuid::Uuid;

#[derive(Debug, Clone)]
enum OpenMenu {
    Chest { container_id: i32, pos: BlockPos },
    Barrel { container_id: i32, pos: BlockPos },
    Furnace { container_id: i32, pos: BlockPos },
    Hopper { container_id: i32, pos: BlockPos },
    CraftingTable { container_id: i32, slots: Vec<ItemStack> },
}

impl OpenMenu {
    fn container_id(&self) -> i32 {
        match self {
            Self::Chest { container_id, .. }
            | Self::Barrel { container_id, .. }
            | Self::Furnace { container_id, .. }
            | Self::Hopper { container_id, .. }
            | Self::CraftingTable { container_id, .. } => *container_id,
        }
    }

    fn block_pos(&self) -> Option<BlockPos> {
        match self {
            Self::Chest { pos, .. }
            | Self::Barrel { pos, .. }
            | Self::Furnace { pos, .. }
            | Self::Hopper { pos, .. } => Some(*pos),
            Self::CraftingTable { .. } => None,
        }
    }
}

#[derive(Debug, Clone)]
struct ActiveEffect {
    effect_id: i32,
    name: String,
    amplifier: i32,
    duration: i32,
}

struct PlayerEntity {
    entity_id: i32,
    profile: GameProfile,
    gamemode: i32,
    dimension: DimensionId,
    pos: Vec3,
    yaw: f32,
    pitch: f32,
    on_ground: bool,
    health: f32,
    food: i32,
    saturation: f32,
    food_exhaustion: f32,
    food_tick_timer: u32,
    xp_level: i32,
    xp_progress: f32,
    xp_total: i32,
    portal_cooldown: u32,
    effects: Vec<ActiveEffect>,
    advancements: PlayerAdvancements,
    spawnpoint: Option<BlockPos>,
    fall_distance: f64,
    dead: bool,
    last_death: Option<(String, BlockPos)>,
    center_chunk: ChunkPos,
    loaded_chunks: HashSet<ChunkPos>,
    view_distance: i32,
    inventory: PlayerInventory,
    next_teleport_id: i32,
    next_container_id: i32,
    open_menu: Option<OpenMenu>,
    tx: mpsc::UnboundedSender<Vec<u8>>,
}

impl PlayerEntity {
    fn to_saved_data(&self) -> SavedPlayerData {
        SavedPlayerData {
            dimension: self.dimension,
            pos: self.pos,
            yaw: self.yaw,
            pitch: self.pitch,
            gamemode: self.gamemode,
            health: self.health,
            food: self.food,
            saturation: self.saturation,
            food_exhaustion: self.food_exhaustion,
            xp_level: self.xp_level,
            xp_progress: self.xp_progress,
            xp_total: self.xp_total,
            selected_hotbar: self.inventory.selected_hotbar,
            slots: self.inventory.slots.clone(),
        }
    }
}

type PlayerHandle = Handle<PlayerEntity>;

enum TickEvent {
    CheckLogin {
        name: String,
        reply: oneshot::Sender<Option<String>>,
    },
    Join {
        profile: GameProfile,
        view_distance: i32,
        tx: mpsc::UnboundedSender<Vec<u8>>,
        reply: oneshot::Sender<PlayerHandle>,
    },
    Packet {
        handle: PlayerHandle,
        packet: PlayServerbound,
    },
    ExtraPacket {
        handle: PlayerHandle,
        packet: PlayExtraServerbound,
    },
    Disconnect {
        handle: PlayerHandle,
    },
    ConsoleCommand {
        command: String,
        reply: oneshot::Sender<String>,
    },
    JsonRpc {
        method: String,
        params: serde_json::Value,
        reply: oneshot::Sender<Result<serde_json::Value, (i32, String)>>,
    },
    QueryStatus {
        reply: oneshot::Sender<QuerySnapshot>,
    },
    Shutdown {
        done: oneshot::Sender<()>,
    },
}

/// Handle for sending synchronous console commands from stdin or external threads.
#[derive(Clone)]
pub struct ConsoleHandle {
    tick_tx: mpsc::UnboundedSender<TickEvent>,
}

impl ConsoleHandle {
    pub fn exec_blocking(&self, command: String) -> String {
        let (tx, rx) = oneshot::channel();
        if self
            .tick_tx
            .send(TickEvent::ConsoleCommand { command, reply: tx })
            .is_err()
        {
            return "Server is shutting down.".into();
        }
        rx.blocking_recv()
            .unwrap_or_else(|_| "Server closed.".into())
    }
}

struct SharedContext {
    config: ServerConfig,
    keypair: ServerKeypair,
    datapacks: Arc<LoadedDatapacks>,
    network_tags_payload: Option<Vec<u8>>,
    commands_packet: Vec<u8>,
    online_count: AtomicI32,
    tick_tx: mpsc::UnboundedSender<TickEvent>,
}

pub struct MinecraftServer {
    listener: TcpListener,
    local_addr: SocketAddr,
    rcon_server: Option<RconServer>,
    rcon_addr: Option<SocketAddr>,
    query_server: Option<QueryServer>,
    query_addr: Option<SocketAddr>,
    jsonrpc_server: Option<JsonRpcServer>,
    jsonrpc_addr: Option<SocketAddr>,
    ctx: Arc<SharedContext>,
    tick_rx: Option<mpsc::UnboundedReceiver<TickEvent>>,
    world_store: Option<WorldStore>,
}

impl MinecraftServer {
    pub async fn bind(config: ServerConfig) -> Result<Self> {
        let datapacks = Arc::new(LoadedDatapacks::load_from_data_dir(&config.data_dir)?);
        info!(
            "Loaded {} synchronized dynamic registries from {}",
            datapacks.registries.len(),
            config.data_dir.display()
        );
        let keypair = ServerKeypair::generate()?;
        let world_store = WorldStore::open(
            &config.world_dir,
            Arc::clone(&datapacks),
            config.level_seed,
            &config.generator_type,
        );
        let network_tags_payload = config
            .network_tags_path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok());
        if let Some(ref bytes) = network_tags_payload {
            info!("Loaded vanilla network tags payload ({} bytes)", bytes.len());
        }

        let commands_packet = commands::build_commands_packet();

        let addr = format!("{}:{}", config.bind_addr, config.port);
        let listener = TcpListener::bind(&addr).await?;
        let local_addr = listener.local_addr()?;
        let (tick_tx, tick_rx) = mpsc::unbounded_channel();

        let (rcon_server, rcon_addr) = if config.enable_rcon {
            let tx_clone = tick_tx.clone();
            let handler = Arc::new(move |cmd: String, reply: oneshot::Sender<String>| {
                let _ = tx_clone.send(TickEvent::ConsoleCommand {
                    command: cmd,
                    reply,
                });
            });
            let srv =
                RconServer::bind(&config.bind_addr, config.rcon_port, config.rcon_password.clone(), handler)
                    .await?;
            let raddr = srv.local_addr();
            (Some(srv), Some(raddr))
        } else {
            (None, None)
        };

        let (jsonrpc_server, jsonrpc_addr) = if config.management_server_enabled {
            let tx_clone = tick_tx.clone();
            let disp = Arc::new(
                move |method: String,
                      params: serde_json::Value,
                      reply: oneshot::Sender<Result<serde_json::Value, (i32, String)>>| {
                    let _ = tx_clone.send(TickEvent::JsonRpc {
                        method,
                        params,
                        reply,
                    });
                },
            );
            let srv = JsonRpcServer::bind(
                &config.bind_addr,
                config.management_server_port,
                config.management_server_secret.clone(),
                disp,
            )
            .await?;
            let jaddr = srv.local_addr();
            (Some(srv), Some(jaddr))
        } else {
            (None, None)
        };

        let (query_server, query_addr) = if config.enable_query {
            let tx_clone = tick_tx.clone();
            let cb = Arc::new(move |reply: oneshot::Sender<QuerySnapshot>| {
                let _ = tx_clone.send(TickEvent::QueryStatus { reply });
            });
            let srv = QueryServer::bind(&config.bind_addr, config.query_port, cb).await?;
            let qaddr = srv.local_addr();
            (Some(srv), Some(qaddr))
        } else {
            (None, None)
        };

        let ctx = Arc::new(SharedContext {
            config,
            keypair,
            datapacks,
            network_tags_payload,
            commands_packet,
            online_count: AtomicI32::new(0),
            tick_tx,
        });

        Ok(Self {
            listener,
            local_addr,
            rcon_server,
            rcon_addr,
            query_server,
            query_addr,
            jsonrpc_server,
            jsonrpc_addr,
            ctx,
            tick_rx: Some(tick_rx),
            world_store: Some(world_store),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn rcon_addr(&self) -> Option<SocketAddr> {
        self.rcon_addr
    }

    pub fn query_addr(&self) -> Option<SocketAddr> {
        self.query_addr
    }

    pub fn jsonrpc_addr(&self) -> Option<SocketAddr> {
        self.jsonrpc_addr
    }

    pub fn console_handle(&self) -> ConsoleHandle {
        ConsoleHandle {
            tick_tx: self.ctx.tick_tx.clone(),
        }
    }

    /// Run the server until the shutdown signal completes (or `/stop` is issued).
    pub async fn run(mut self, shutdown: Option<oneshot::Receiver<()>>) -> Result<()> {
        let tick_rx = self.tick_rx.take().unwrap();
        let world_store = self.world_store.take().unwrap();
        let tick_ctx = Arc::clone(&self.ctx);
        let (stop_tx, stop_rx) = oneshot::channel();

        let tick_handle = tokio::spawn(async move {
            run_tick_loop(tick_ctx, world_store, tick_rx, Some(stop_tx)).await;
        });
        let rcon_handle = self
            .rcon_server
            .take()
            .map(|srv| tokio::spawn(async move { srv.run().await }));
        let query_handle = self
            .query_server
            .take()
            .map(|srv| tokio::spawn(async move { srv.run().await }));
        let jsonrpc_handle = self
            .jsonrpc_server
            .take()
            .map(|srv| tokio::spawn(async move { srv.run().await }));

        info!(
            "SiliconMC Rust server ({VERSION_NAME} / protocol {PROTOCOL_VERSION}) listening on {}",
            self.local_addr
        );

        let accept_loop = async {
            loop {
                match self.listener.accept().await {
                    Ok((stream, peer)) => {
                        let ctx = Arc::clone(&self.ctx);
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, peer, ctx).await {
                                warn!("Connection {peer} closed: {e}");
                            }
                        });
                    }
                    Err(e) => error!("TCP accept error: {e}"),
                }
            }
        };

        if let Some(rx) = shutdown {
            tokio::select! {
                _ = accept_loop => {}
                _ = rx => {
                    info!("Shutting down SiliconMC Rust server");
                }
                _ = stop_rx => {
                    info!("Server stopped via /stop command");
                }
            }
        } else {
            tokio::select! {
                _ = accept_loop => {}
                _ = stop_rx => {
                    info!("Server stopped via /stop command");
                }
            }
        }

        let (done_tx, done_rx) = oneshot::channel();
        if self.ctx.tick_tx.send(TickEvent::Shutdown { done: done_tx }).is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(5), done_rx).await;
        }
        if let Some(h) = rcon_handle {
            h.abort();
        }
        if let Some(h) = query_handle {
            h.abort();
        }
        if let Some(h) = jsonrpc_handle {
            h.abort();
        }
        tick_handle.abort();
        Ok(())
    }
}

struct SimState {
    world: WorldStore,
    admin: AdminStore,
    scoreboard: ScoreboardStore,
    players: Arena<PlayerEntity>,
    entities: Arena<WorldEntity>,
    scheduler: TickScheduler,
    next_entity_id: i32,
    overworld_dim_id: i32,
    nether_dim_id: i32,
    end_dim_id: i32,
    overworld_clock_id: i32,
}

impl SimState {
    fn save_all_players(&self) {
        let dir = self.world.world_dir();
        for (_, p) in self.players.iter() {
            if let Err(e) = save_player_data(dir, p.profile.uuid, &p.to_saved_data()) {
                warn!("Failed saving playerdata for {}: {e}", p.profile.name);
            }
            if let Err(e) = p.advancements.save(dir) {
                warn!("Failed saving advancements for {}: {e}", p.profile.name);
            }
        }
    }

    fn announce_advancements(&self, player_name: &str, awarded: Vec<(&'static str, &'static str)>) {
        if awarded.is_empty() || !self.world.gamerules.show_advancement_messages() {
            return;
        }
        for (_, title) in awarded {
            let msg = format!("{player_name} has made the advancement [{title}]");
            if let Ok(pkt) = encode_play_system_chat(&msg, false) {
                for (_, other) in self.players.iter() {
                    let _ = other.tx.send(pkt.clone());
                }
            }
        }
    }

    fn trigger_explosion(&mut self, center: Vec3, power: f32, fire: bool) {
        let break_blocks = self.world.gamerules.tnt_explodes();
        let seed = (self.world.game_time as u64)
            .wrapping_mul(6364136223846793005)
            .wrapping_add(0x9E37_79B9_7F4A_7C15);
        let res = compute_and_apply_explosion(&mut self.world, center, power, fire, break_blocks, seed);
        let destroyed_count = res.destroyed_blocks.len() as i32;

        let mut changed: Vec<(BlockPos, BlockStateId)> = res
            .destroyed_blocks
            .iter()
            .map(|(bp, _)| (*bp, BlockStateId::AIR))
            .collect();
        changed.extend(res.fire_blocks.iter().copied());
        self.broadcast_block_changes(&changed);

        let particle_id =
            static_registry_protocol_id("minecraft:particle_type", "minecraft:explosion_emitter")
                .unwrap_or(0);
        let sound_id =
            static_registry_protocol_id("minecraft:sound_event", "minecraft:entity.generic.explode")
                .unwrap_or(0);

        let phandles: Vec<PlayerHandle> = self.players.iter().map(|(h, _)| h).collect();
        let mut dmg_targets = Vec::new();
        for ph in phandles {
            if let Some(p) = self.players.get(ph) {
                let kb_opt = compute_entity_explosion_knockback(center, power, p.pos);
                let kb_vec = kb_opt.map(|(_, v)| v);
                if let Some((dmg, _)) = kb_opt {
                    if dmg > 0.0 {
                        dmg_targets.push((ph, dmg));
                    }
                }
                let pkt = encode_play_explode(
                    center,
                    power,
                    destroyed_count,
                    kb_vec,
                    particle_id,
                    sound_id,
                    &[],
                    true,
                );
                let _ = p.tx.send(pkt);
            }
        }
        for (ph, dmg) in dmg_targets {
            self.damage_player(ph, dmg, "blew up");
        }
    }

    fn execute_extra_command(&mut self, handle: PlayerHandle, action: ExtraCommandAction) -> String {
        let mut block_changes: Vec<(BlockPos, BlockStateId)> = Vec::new();
        let msg = match action {
            ExtraCommandAction::Reply(s) => s,
            ExtraCommandAction::Fill {
                min,
                max,
                state,
                mode,
            } => {
                let mut count = 0usize;
                for y in min.y..=max.y {
                    for z in min.z..=max.z {
                        for x in min.x..=max.x {
                            let is_shell = x == min.x
                                || x == max.x
                                || y == min.y
                                || y == max.y
                                || z == min.z
                                || z == max.z;
                            let bp = BlockPos::new(x, y, z);
                            let place = match mode {
                                FillMode::Replace { filter } => {
                                    if let Some(f) = filter {
                                        if self.world.get_block(bp).block_name() != f.block_name() {
                                            continue;
                                        }
                                    }
                                    state
                                }
                                FillMode::Keep => {
                                    if !self.world.get_block(bp).is_air() {
                                        continue;
                                    }
                                    state
                                }
                                FillMode::Hollow => {
                                    if is_shell { state } else { BlockStateId::AIR }
                                }
                                FillMode::Outline => {
                                    if !is_shell {
                                        continue;
                                    }
                                    state
                                }
                                FillMode::Destroy => state,
                            };
                            if self.world.set_block_and_relight(bp, place).is_some() {
                                block_changes.push((bp, place));
                                count += 1;
                            }
                        }
                    }
                }
                format!("Successfully filled {count} block(s)")
            }
            ExtraCommandAction::Clone {
                src_min,
                src_max,
                dst,
                masked,
            } => {
                let mut buf = Vec::new();
                for y in src_min.y..=src_max.y {
                    for z in src_min.z..=src_max.z {
                        for x in src_min.x..=src_max.x {
                            let st = self.world.get_block(BlockPos::new(x, y, z));
                            if masked && st.is_air() {
                                continue;
                            }
                            buf.push((
                                BlockPos::new(
                                    dst.x + (x - src_min.x),
                                    dst.y + (y - src_min.y),
                                    dst.z + (z - src_min.z),
                                ),
                                st,
                            ));
                        }
                    }
                }
                let count = buf.len();
                for (bp, st) in buf {
                    if self.world.set_block_and_relight(bp, st).is_some() {
                        block_changes.push((bp, st));
                    }
                }
                format!("Successfully cloned {count} block(s)")
            }
            ExtraCommandAction::GameRuleList => {
                let names = self.world.gamerules.list_names();
                format!("Game rules ({}): {}", names.len(), names.join(", "))
            }
            ExtraCommandAction::GameRule { rule, value } => match value {
                None => match self.world.gamerules.get_str(&rule) {
                    Some(v) => format!("Gamerule {rule} is currently set to: {v}"),
                    None => format!("Unknown gamerule: {rule}"),
                },
                Some(v) => match self.world.gamerules.set_from_str(&rule, &v) {
                    Ok(nv) => format!("Gamerule {rule} is now set to: {nv}"),
                    Err(e) => e,
                },
            },
            ExtraCommandAction::ScoreboardObjectivesList => {
                let names: Vec<&str> = self
                    .scoreboard
                    .list_objectives()
                    .iter()
                    .map(|o| o.name.as_str())
                    .collect();
                format!("There are {} objective(s): {}", names.len(), names.join(", "))
            }
            ExtraCommandAction::ScoreboardObjectivesAdd {
                name,
                criteria,
                display_name,
            } => {
                let disp = display_name.unwrap_or_else(|| name.clone());
                match self
                    .scoreboard
                    .add_objective(&name, &criteria, Some(&disp))
                {
                    Ok(_) => {
                        if let Ok(pkt) = encode_play_set_objective(
                            &name,
                            ObjectiveMethod::Add,
                            Some(&text_component(&disp)),
                            ObjectiveRenderType::Integer,
                            None,
                        ) {
                            for (_, p) in self.players.iter() {
                                let _ = p.tx.send(pkt.clone());
                            }
                        }
                        format!("Created new objective [{disp}]")
                    }
                    Err(e) => e,
                }
            }
            ExtraCommandAction::ScoreboardObjectivesRemove { name } => {
                match self.scoreboard.remove_objective(&name) {
                    Ok(_) => {
                        if let Ok(pkt) = encode_play_set_objective(
                            &name,
                            ObjectiveMethod::Remove,
                            None,
                            ObjectiveRenderType::Integer,
                            None,
                        ) {
                            for (_, p) in self.players.iter() {
                                let _ = p.tx.send(pkt.clone());
                            }
                        }
                        format!("Removed objective {name}")
                    }
                    Err(e) => e,
                }
            }
            ExtraCommandAction::ScoreboardObjectivesSetDisplay { slot, objective } => {
                match self
                    .scoreboard
                    .set_display_slot(slot, objective.as_deref())
                {
                    Ok(()) => {
                        let dslot = match slot {
                            0 => DisplaySlot::List,
                            2 => DisplaySlot::BelowName,
                            _ => DisplaySlot::Sidebar,
                        };
                        let pkt = encode_play_set_display_objective(
                            dslot,
                            objective.as_deref().unwrap_or(""),
                        );
                        for (_, p) in self.players.iter() {
                            let _ = p.tx.send(pkt.clone());
                        }
                        format!(
                            "Set display slot {slot} to {}",
                            objective.as_deref().unwrap_or("(cleared)")
                        )
                    }
                    Err(e) => e,
                }
            }
            ExtraCommandAction::ScoreboardPlayersSet {
                target,
                objective,
                score,
            } => match self.scoreboard.set_score(&target, &objective, score) {
                Ok(val) => {
                    if let Ok(pkt) = encode_play_set_score(&target, &objective, val, None, None) {
                        for (_, p) in self.players.iter() {
                            let _ = p.tx.send(pkt.clone());
                        }
                    }
                    format!("Set [{objective}] for {target} to {val}")
                }
                Err(e) => e,
            },
            ExtraCommandAction::ScoreboardPlayersAdd {
                target,
                objective,
                amount,
            } => match self.scoreboard.add_score(&target, &objective, amount) {
                Ok(ns) => {
                    if let Ok(pkt) = encode_play_set_score(&target, &objective, ns, None, None) {
                        for (_, p) in self.players.iter() {
                            let _ = p.tx.send(pkt.clone());
                        }
                    }
                    format!("Added {amount} to [{objective}] for {target} (now {ns})")
                }
                Err(e) => e,
            },
            ExtraCommandAction::ScoreboardPlayersRemove {
                target,
                objective,
                amount,
            } => match self.scoreboard.remove_score(&target, &objective, amount) {
                Ok(ns) => {
                    if let Ok(pkt) = encode_play_set_score(&target, &objective, ns, None, None) {
                        for (_, p) in self.players.iter() {
                            let _ = p.tx.send(pkt.clone());
                        }
                    }
                    format!("Removed {amount} from [{objective}] for {target} (now {ns})")
                }
                Err(e) => e,
            },
            ExtraCommandAction::ScoreboardPlayersReset { target, objective } => {
                let _ = self
                    .scoreboard
                    .reset_score(&target, objective.as_deref());
                let pkt = encode_play_reset_score(&target, objective.as_deref());
                for (_, p) in self.players.iter() {
                    let _ = p.tx.send(pkt.clone());
                }
                format!("Reset score(s) for {target}")
            }
            ExtraCommandAction::ScoreboardPlayersGet { target, objective } => {
                let val = self.scoreboard.get_score(&target, &objective).unwrap_or(0);
                format!("{target} has {val} [{objective}]")
            }
            ExtraCommandAction::TeamList => {
                let names: Vec<&str> = self
                    .scoreboard
                    .list_teams()
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect();
                format!("There are {} team(s): {}", names.len(), names.join(", "))
            }
            ExtraCommandAction::TeamAdd { name, display_name } => {
                let disp = display_name.unwrap_or_else(|| name.clone());
                match self.scoreboard.add_team(&name, Some(&disp)) {
                    Ok(_) => {
                        let params = PlayerTeamParams {
                            display_name: text_component(&disp),
                            player_prefix: text_component(""),
                            player_suffix: text_component(""),
                            name_tag_visibility: TeamVisibility::Always,
                            collision_rule: TeamCollisionRule::Always,
                            color: Some(TeamColor::White),
                            options: 0x03,
                        };
                        if let Ok(pkt) = encode_play_set_player_team_create(&name, &params, &[]) {
                            for (_, p) in self.players.iter() {
                                let _ = p.tx.send(pkt.clone());
                            }
                        }
                        format!("Created team [{disp}]")
                    }
                    Err(e) => e,
                }
            }
            ExtraCommandAction::TeamRemove { name } => {
                match self.scoreboard.remove_team(&name) {
                    Ok(_) => {
                        let pkt = encode_play_set_player_team_remove(&name);
                        for (_, p) in self.players.iter() {
                            let _ = p.tx.send(pkt.clone());
                        }
                        format!("Removed team {name}")
                    }
                    Err(e) => e,
                }
            }
            ExtraCommandAction::TeamJoin { team, member } => {
                match self.scoreboard.join_team(&team, &member) {
                    Ok(()) => {
                        let pkt = encode_play_set_player_team_join(&team, &[member.as_str()]);
                        for (_, p) in self.players.iter() {
                            let _ = p.tx.send(pkt.clone());
                        }
                        format!("Added {member} to team {team}")
                    }
                    Err(e) => e,
                }
            }
            ExtraCommandAction::TeamLeave { member } => {
                if let Some(old_team) = self
                    .scoreboard
                    .team_of_member(&member)
                    .map(|t| t.name.clone())
                {
                    self.scoreboard.leave_team(&member);
                    let pkt = encode_play_set_player_team_leave(&old_team, &[member.as_str()]);
                    for (_, p) in self.players.iter() {
                        let _ = p.tx.send(pkt.clone());
                    }
                    format!("Removed {member} from team {old_team}")
                } else {
                    format!("{member} is not on any team")
                }
            }
            ExtraCommandAction::TeamModifyColor { team, color } => {
                if let Some(t) = self.scoreboard.get_team_mut(&team) {
                    t.color = color;
                    format!("Updated team {team} color")
                } else {
                    format!("Unknown team '{team}'")
                }
            }
            ExtraCommandAction::BossBarList => {
                let ids: Vec<&str> = self
                    .scoreboard
                    .list_bossbars()
                    .iter()
                    .map(|b| b.id.as_str())
                    .collect();
                format!("There are {} bossbar(s): {}", ids.len(), ids.join(", "))
            }
            ExtraCommandAction::BossBarAdd { id, name } => {
                match self.scoreboard.add_bossbar(&id, &name) {
                    Ok(bb) => {
                        let uuid = bb.uuid;
                        if let Ok(pkt) = encode_play_boss_event_add(
                            uuid,
                            &text_component(&name),
                            0.0,
                            BossBarColor::White,
                            BossBarOverlay::Progress,
                            false,
                            false,
                            false,
                        ) {
                            for (_, p) in self.players.iter() {
                                let _ = p.tx.send(pkt.clone());
                            }
                        }
                        format!("Created custom bossbar [{name}]")
                    }
                    Err(e) => e,
                }
            }
            ExtraCommandAction::BossBarRemove { id } => {
                match self.scoreboard.remove_bossbar(&id) {
                    Ok(removed) => {
                        let pkt = encode_play_boss_event_remove(removed.uuid);
                        for (_, p) in self.players.iter() {
                            let _ = p.tx.send(pkt.clone());
                        }
                        format!("Removed custom bossbar [{id}]")
                    }
                    Err(e) => e,
                }
            }
            ExtraCommandAction::BossBarSet { id, property } => {
                let res = match &property {
                    BossBarProperty::Name(n) => self.scoreboard.set_bossbar_name(&id, n),
                    BossBarProperty::Value(v) => self.scoreboard.set_bossbar_value(&id, *v),
                    BossBarProperty::Max(m) => self.scoreboard.set_bossbar_max(&id, *m),
                    BossBarProperty::Visible(vis) => self.scoreboard.set_bossbar_visible(&id, *vis),
                    BossBarProperty::Color(c) => self.scoreboard.set_bossbar_color(&id, *c),
                    BossBarProperty::Overlay(ov) => self.scoreboard.set_bossbar_overlay(&id, *ov),
                };
                match res {
                    Ok(bb) => {
                        let pkt_opt = match property {
                            BossBarProperty::Name(_) => {
                                encode_play_boss_event_update_name(bb.uuid, &text_component(&bb.name)).ok()
                            }
                            BossBarProperty::Value(_) | BossBarProperty::Max(_) => {
                                Some(encode_play_boss_event_update_progress(bb.uuid, bb.progress))
                            }
                            BossBarProperty::Visible(true) => encode_play_boss_event_add(
                                bb.uuid,
                                &text_component(&bb.name),
                                bb.progress,
                                int_to_bossbar_color(bb.color),
                                int_to_bossbar_overlay(bb.overlay),
                                false,
                                false,
                                false,
                            )
                            .ok(),
                            BossBarProperty::Visible(false) => {
                                Some(encode_play_boss_event_remove(bb.uuid))
                            }
                            BossBarProperty::Color(_) | BossBarProperty::Overlay(_) => {
                                Some(encode_play_boss_event_update_style(
                                    bb.uuid,
                                    int_to_bossbar_color(bb.color),
                                    int_to_bossbar_overlay(bb.overlay),
                                ))
                            }
                        };
                        if let Some(pkt) = pkt_opt {
                            for (_, p) in self.players.iter() {
                                let _ = p.tx.send(pkt.clone());
                            }
                        }
                        format!("Updated bossbar {id}")
                    }
                    Err(e) => e,
                }
            }
            ExtraCommandAction::TitleText { slot, text, .. } => {
                let pkt = match slot {
                    TitleSlot::Title => encode_play_set_title_text(&text),
                    TitleSlot::Subtitle => encode_play_set_subtitle_text(&text),
                    TitleSlot::ActionBar => encode_play_set_action_bar_text(&text),
                };
                if let Ok(bytes) = pkt {
                    for (_, p) in self.players.iter() {
                        let _ = p.tx.send(bytes.clone());
                    }
                }
                format!("Showing title '{text}'")
            }
            ExtraCommandAction::TitleClear { reset, .. } => {
                let pkt = encode_play_clear_titles(reset);
                for (_, p) in self.players.iter() {
                    let _ = p.tx.send(pkt.clone());
                }
                "Cleared titles".into()
            }
            ExtraCommandAction::TitleTimes {
                fade_in,
                stay,
                fade_out,
                ..
            } => {
                let pkt = encode_play_set_titles_animation(fade_in, stay, fade_out);
                for (_, p) in self.players.iter() {
                    let _ = p.tx.send(pkt.clone());
                }
                format!("Updated title display times ({fade_in}, {stay}, {fade_out})")
            }
            ExtraCommandAction::ClearInventory {
                item_id, max_count, ..
            } => {
                let mut cleared = 0u32;
                if let Some(p) = self.players.get_mut(handle) {
                    let mut rem_cap = max_count.unwrap_or(u32::MAX);
                    for s in p.inventory.slots.iter_mut() {
                        if rem_cap == 0 || s.is_empty() {
                            continue;
                        }
                        if item_id.map_or(true, |id| s.item_id == id) {
                            let take = (s.count as u32).min(rem_cap);
                            s.count = (s.count as u32 - take) as u8;
                            if s.count == 0 {
                                *s = ItemStack::EMPTY;
                            }
                            cleared += take;
                            rem_cap -= take;
                        }
                    }
                    let sid = p.inventory.next_state_id();
                    let _ = p.tx.send(encode_play_container_set_content(
                        0,
                        sid,
                        &p.inventory.slots,
                        &p.inventory.carried,
                    ));
                }
                format!("Removed {cleared} item(s) from inventory")
            }
            ExtraCommandAction::SetSpawnpoint { pos, .. } => {
                if let Some(p) = self.players.get_mut(handle) {
                    p.spawnpoint = Some(pos);
                }
                format!("Set spawn point to {}, {}, {}", pos.x, pos.y, pos.z)
            }
            ExtraCommandAction::SetWorldSpawn { pos } => {
                self.world.spawn_pos = pos;
                let pkt = encode_play_set_default_spawn("minecraft:overworld", pos, 0.0, 0.0);
                for (_, p) in self.players.iter() {
                    let _ = p.tx.send(pkt.clone());
                }
                format!("Set the world spawn point to {}, {}, {}", pos.x, pos.y, pos.z)
            }
            ExtraCommandAction::QueryDifficulty => {
                let label = match self.world.difficulty {
                    0 => "Peaceful",
                    1 => "Easy",
                    3 => "Hard",
                    _ => "Normal",
                };
                format!("The difficulty is {label}")
            }
            ExtraCommandAction::SetDifficulty(d) => {
                self.world.difficulty = d.clamp(0, 3);
                let diff = Difficulty::from_id(self.world.difficulty).unwrap_or(Difficulty::Normal);
                let pkt = encode_play_change_difficulty(diff, false);
                for (_, p) in self.players.iter() {
                    let _ = p.tx.send(pkt.clone());
                }
                format!("The difficulty has been set to {diff:?}")
            }
            ExtraCommandAction::Enchant {
                enchantment, level, ..
            } => {
                if let Some(spec) = enchantment_by_id(&enchantment) {
                    let clamped = (level.max(1) as u8).min(spec.max_level);
                    let awarded = self.players.get_mut(handle).and_then(|p| {
                        let name = p.profile.name.clone();
                        p.advancements.check_enchanted_item().map(|a| (name, vec![a]))
                    });
                    if let Some((pname, list)) = awarded {
                        self.announce_advancements(&pname, list);
                    }
                    format!("Applied enchantment {} level {clamped}", spec.id)
                } else {
                    format!("Unknown enchantment: {enchantment}")
                }
            }
            ExtraCommandAction::Advancement {
                grant,
                advancement_id,
                ..
            } => {
                let mut msg_out = String::new();
                if let Some(p) = self.players.get_mut(handle) {
                    if grant {
                        match advancement_id {
                            None => {
                                let n = p.advancements.grant_all();
                                msg_out = format!("Granted {n} advancement(s) to {}", p.profile.name);
                            }
                            Some(id) => {
                                p.advancements.grant_advancement(&id);
                                msg_out = format!("Granted advancement {id} to {}", p.profile.name);
                            }
                        }
                    } else {
                        match advancement_id {
                            None => {
                                let n = p.advancements.revoke_all();
                                msg_out = format!("Revoked {n} advancement(s) from {}", p.profile.name);
                            }
                            Some(id) => {
                                p.advancements.revoke(&id);
                                msg_out = format!("Revoked advancement {id} from {}", p.profile.name);
                            }
                        }
                    }
                }
                msg_out
            }
        };
        self.broadcast_block_changes(&block_changes);
        msg
    }

    fn apply_weather(&mut self, raining: bool, thundering: bool, duration_ticks: i32) {
        self.world.raining = raining;
        self.world.rain_time = duration_ticks;
        self.world.thundering = raining && thundering;
        self.world.thunder_time = duration_ticks;

        let ev_pkt = encode_play_game_event(if raining { 1 } else { 2 }, 0.0);
        let rain_pkt = encode_play_game_event(7, if raining { 1.0 } else { 0.0 });
        let thunder_pkt = encode_play_game_event(8, if raining && thundering { 1.0 } else { 0.0 });
        for (_, p) in self.players.iter() {
            let _ = p.tx.send(ev_pkt.clone());
            let _ = p.tx.send(rain_pkt.clone());
            let _ = p.tx.send(thunder_pkt.clone());
        }
    }

    fn alloc_entity_id(&mut self) -> i32 {
        let id = self.next_entity_id;
        self.next_entity_id += 1;
        id
    }

    fn spawn_world_entity(&mut self, ent: WorldEntity) -> Handle<WorldEntity> {
        let add_pkt = encode_play_add_entity(
            ent.entity_id,
            ent.uuid,
            ent.entity_type_id,
            ent.pos,
            ent.vel,
            ent.pitch,
            ent.yaw,
            ent.yaw,
            ent.add_entity_data_field(),
        );
        let meta_pkt = match &ent.kind {
            EntityKind::Item { stack, .. } => Some(encode_play_set_entity_data(
                ent.entity_id,
                &[EntityDataValue::ItemStack(8, stack.clone())],
            )),
            EntityKind::Mob { health, .. } => Some(encode_play_set_entity_data(
                ent.entity_id,
                &[EntityDataValue::Float(9, *health)],
            )),
            EntityKind::FallingBlock { .. } | EntityKind::Projectile { .. } => None,
        };
        for (_, p) in self.players.iter() {
            let _ = p.tx.send(add_pkt.clone());
            if let Some(ref mp) = meta_pkt {
                let _ = p.tx.send(mp.clone());
            }
        }
        self.entities.insert(ent)
    }

    fn broadcast_block_changes(&mut self, changes: &[(BlockPos, BlockStateId)]) {
        if changes.is_empty() {
            return;
        }
        let mut combined_light = DirtyLightSections::default();
        for &(pos, state) in changes {
            let dirty = self.world.relight_block(pos);
            for (cp, (sm, bm)) in dirty.chunks {
                let entry = combined_light.chunks.entry(cp).or_insert((0, 0));
                entry.0 |= sm;
                entry.1 |= bm;
            }
            let block_pkt = encode_play_block_update(pos, state.0);
            let bcp = pos.chunk_pos();
            for (_, p) in self.players.iter() {
                if p.loaded_chunks.contains(&bcp) {
                    let _ = p.tx.send(block_pkt.clone());
                }
            }
        }
        for (&cp, &(sky_mask, block_mask)) in &combined_light.chunks {
            if sky_mask == 0 && block_mask == 0 {
                continue;
            }
            if let Some(chunk) = self.world.get_chunk(cp) {
                let lpkt = encode_play_light_update(chunk, sky_mask, block_mask);
                for (_, p) in self.players.iter() {
                    if p.loaded_chunks.contains(&cp) {
                        let _ = p.tx.send(lpkt.clone());
                    }
                }
            }
        }
    }

    fn give_player_effect(
        &mut self,
        handle: PlayerHandle,
        effect_name: &str,
        duration_ticks: i32,
        amplifier: i32,
    ) -> Result<(), String> {
        let Some(effect_id) = static_registry_protocol_id("minecraft:mob_effect", effect_name)
        else {
            return Err(format!("Unknown mob effect: {effect_name}"));
        };
        if effect_name == "minecraft:instant_health" {
            if let Some(p) = self.players.get_mut(handle) {
                let heal = (4 << amplifier.clamp(0, 5)) as f32;
                p.health = (p.health + heal).min(20.0);
                let _ = p.tx.send(encode_play_set_health(p.health, p.food, p.saturation));
            }
            return Ok(());
        }
        if effect_name == "minecraft:instant_damage" {
            let dmg = (6 << amplifier.clamp(0, 5)) as f32;
            self.damage_player(handle, dmg, "was killed by magic");
            return Ok(());
        }
        if let Some(p) = self.players.get_mut(handle) {
            if let Some(existing) = p.effects.iter_mut().find(|e| e.effect_id == effect_id) {
                existing.amplifier = existing.amplifier.max(amplifier);
                existing.duration = existing.duration.max(duration_ticks);
            } else {
                p.effects.push(ActiveEffect {
                    effect_id,
                    name: effect_name.to_string(),
                    amplifier,
                    duration: duration_ticks,
                });
            }
            let _ = p.tx.send(encode_play_update_mob_effect(
                p.entity_id,
                effect_id,
                amplifier,
                duration_ticks,
                0x06, // visible | show_icon
            ));
        }
        Ok(())
    }

    fn clear_player_effects(&mut self, handle: PlayerHandle) -> usize {
        let Some(p) = self.players.get_mut(handle) else {
            return 0;
        };
        let removed: Vec<ActiveEffect> = std::mem::take(&mut p.effects);
        let n = removed.len();
        for ef in removed {
            let _ = p
                .tx
                .send(encode_play_remove_mob_effect(p.entity_id, ef.effect_id));
        }
        n
    }

    fn tick_scheduled_and_entities(&mut self) {
        let now = self.world.game_time as u64;

        // 1. Deterministic scheduled block ticks (buttons, torches, repeaters, falling sand/gravel)
        let due = self.scheduler.drain_due(now);
        let mut all_block_changes = Vec::new();
        let mut falling_spawns = Vec::new();
        for tick in due {
            let (changes, falling) =
                execute_scheduled_tick(&mut self.world, &mut self.scheduler, now, tick);
            all_block_changes.extend(changes);
            if let Some(fb) = falling {
                falling_spawns.push(fb);
            }
        }
        self.broadcast_block_changes(&all_block_changes);
        for (pos, state) in falling_spawns {
            let eid = self.alloc_entity_id();
            let ent = WorldEntity::spawn_falling_block(eid, pos, state);
            self.spawn_world_entity(ent);
        }

        // 2. Step physics & AI for all live world entities (Items, FallingBlocks, Mobs)
        let handles: Vec<Handle<WorldEntity>> = self.entities.iter().map(|(h, _)| h).collect();
        let mut to_remove: Vec<(Handle<WorldEntity>, i32)> = Vec::new();
        let mut landed_blocks: Vec<(BlockPos, BlockStateId)> = Vec::new();
        let mut projectile_hits: Vec<(Vec3, String, i32, f32)> = Vec::new();
        let mut moved_pkts: Vec<Vec<u8>> = Vec::new();

        for h in handles {
            let Some(ent) = self.entities.get_mut(h) else {
                continue;
            };
            match ent.tick(&mut self.world, now) {
                EntityTickOutcome::Despawn => {
                    to_remove.push((h, ent.entity_id));
                }
                EntityTickOutcome::LandedFallingBlock { pos, state } => {
                    to_remove.push((h, ent.entity_id));
                    landed_blocks.push((pos, state));
                }
                EntityTickOutcome::ProjectileHit {
                    hit_pos,
                    kind_name,
                    owner_eid,
                    damage,
                } => {
                    to_remove.push((h, ent.entity_id));
                    projectile_hits.push((hit_pos, kind_name, owner_eid, damage));
                }
                EntityTickOutcome::Alive { moved } => {
                    if moved {
                        moved_pkts.push(encode_play_entity_position_sync(
                            ent.entity_id,
                            ent.pos,
                            ent.yaw,
                            ent.pitch,
                            ent.on_ground,
                        ));
                    }
                }
            }
        }

        // Broadcast moved entities
        if !moved_pkts.is_empty() {
            for (_, p) in self.players.iter() {
                for pkt in &moved_pkts {
                    let _ = p.tx.send(pkt.clone());
                }
            }
        }

        // 3. Automatic ItemEntity pickup by nearby players
        let item_handles: Vec<Handle<WorldEntity>> = self
            .entities
            .iter()
            .filter_map(|(h, e)| match &e.kind {
                EntityKind::Item { pickup_delay: 0, .. } => Some(h),
                _ => None,
            })
            .collect();

        for ih in item_handles {
            if to_remove.iter().any(|(rh, _)| *rh == ih) {
                continue;
            }
            let Some(ent) = self.entities.get(ih) else {
                continue;
            };
            let item_pos = ent.pos;
            let item_eid = ent.entity_id;
            let EntityKind::Item { ref stack, .. } = ent.kind else {
                continue;
            };
            let stack_clone = stack.clone();

            // Find closest player within 1.5 blocks
            let mut picker: Option<PlayerHandle> = None;
            for (ph, p) in self.players.iter() {
                let dx = p.pos.x - item_pos.x;
                let dy = (p.pos.y + 0.5) - item_pos.y;
                let dz = p.pos.z - item_pos.z;
                if dx * dx + dy * dy + dz * dz <= 2.25 {
                    picker = Some(ph);
                    break;
                }
            }
            if let Some(ph) = picker {
                if let Some(p) = self.players.get_mut(ph) {
                    let changed = p
                        .inventory
                        .give_item(stack_clone.item_id, stack_clone.count as u32);
                    if !changed.is_empty() {
                        let p_eid = p.entity_id;
                        let pname = p.profile.name.clone();
                        let awarded = item_by_id(stack_clone.item_id)
                            .map(|i| p.advancements.check_item_obtained(i.name))
                            .unwrap_or_default();
                        for (slot_idx, st) in changed {
                            let sid = p.inventory.next_state_id();
                            let _ = p
                                .tx
                                .send(encode_play_container_set_slot(0, sid, slot_idx, &st));
                        }
                        let take_pkt =
                            encode_play_take_item_entity(item_eid, p_eid, stack_clone.count as i32);
                        for (_, other) in self.players.iter() {
                            let _ = other.tx.send(take_pkt.clone());
                        }
                        to_remove.push((ih, item_eid));
                        if !awarded.is_empty() {
                            self.announce_advancements(&pname, awarded);
                        }
                    }
                }
            }
        }

        // Remove despawned / picked-up / landed entities
        if !to_remove.is_empty() {
            let mut rm_ids = Vec::with_capacity(to_remove.len());
            for (h, eid) in to_remove {
                if self.entities.remove(h).is_some() {
                    rm_ids.push(eid);
                }
            }
            if !rm_ids.is_empty() {
                let rm_pkt = encode_play_remove_entities(&rm_ids);
                for (_, p) in self.players.iter() {
                    let _ = p.tx.send(rm_pkt.clone());
                }
            }
        }

        // Solidify landed FallingBlocks and propagate neighbor updates
        if !landed_blocks.is_empty() {
            let mut placed_changes = Vec::new();
            let mut seeds = Vec::new();
            for (pos, state) in landed_blocks {
                if self.world.set_block_and_relight(pos, state).is_some() {
                    placed_changes.push((pos, state));
                    seeds.push(pos);
                }
            }
            placed_changes.extend(propagate_neighbor_updates(
                &mut self.world,
                &mut self.scheduler,
                now,
                &seeds,
            ));
            self.broadcast_block_changes(&placed_changes);
        }

        // Resolve projectile impacts (Ender Pearl teleport + Arrow/Snowball mob hits)
        for (hit_pos, kind_name, owner_eid, damage) in projectile_hits {
            if kind_name == "minecraft:ender_pearl" {
                let owner_h = self
                    .players
                    .iter()
                    .find_map(|(ph, p)| if p.entity_id == owner_eid { Some(ph) } else { None });
                if let Some(oh) = owner_h {
                    let tp_pos = Vec3::new(hit_pos.x, hit_pos.y + 0.1, hit_pos.z);
                    let mut sync_pkt = None;
                    if let Some(p) = self.players.get_mut(oh) {
                        p.pos = tp_pos;
                        p.fall_distance = 0.0;
                        let tp_id = p.next_teleport_id;
                        p.next_teleport_id += 1;
                        let _ = p.tx.send(encode_play_player_position(
                            tp_id,
                            tp_pos,
                            Vec3::ZERO,
                            p.yaw,
                            p.pitch,
                            0,
                        ));
                        let bp = BlockPos::new(
                            tp_pos.x.floor() as i32,
                            tp_pos.y.floor() as i32,
                            tp_pos.z.floor() as i32,
                        );
                        let _ = p.tx.send(encode_play_level_event(2003, bp, 0, false));
                        stream_chunks_for_player(p, &mut self.world);
                        sync_pkt = Some(encode_play_entity_position_sync(
                            p.entity_id,
                            p.pos,
                            p.yaw,
                            p.pitch,
                            true,
                        ));
                    }
                    if let Some(sp) = sync_pkt {
                        for (ph, other) in self.players.iter() {
                            if ph != oh {
                                let _ = other.tx.send(sp.clone());
                            }
                        }
                    }
                    self.damage_player(oh, 5.0, "hit the ground too hard");
                }
            } else if damage > 0.0 {
                // Check mobs within 2.5 blocks of arrow impact
                let hit_mob = self.entities.iter().find_map(|(eh, e)| {
                    if matches!(e.kind, EntityKind::Mob { .. }) {
                        let d2 = (e.pos.x - hit_pos.x).powi(2)
                            + (e.pos.y + 0.9 - hit_pos.y).powi(2)
                            + (e.pos.z - hit_pos.z).powi(2);
                        if d2 <= 6.25 { Some(eh) } else { None }
                    } else {
                        None
                    }
                });
                if let Some(mh) = hit_mob {
                    let mut died_eid = None;
                    if let Some(ent) = self.entities.get_mut(mh) {
                        if let EntityKind::Mob { ref mut health, .. } = ent.kind {
                            *health = (*health - damage).max(0.0);
                            if *health <= 0.0 {
                                died_eid = Some(ent.entity_id);
                            }
                        }
                    }
                    if let Some(eid) = died_eid {
                        self.entities.remove(mh);
                        let rm = encode_play_remove_entities(&[eid]);
                        for (_, p) in self.players.iter() {
                            let _ = p.tx.send(rm.clone());
                        }
                    }
                }
            }
        }

        // 4. Tick all loaded furnace block entities (smelting, fuel, lit state, viewer sync)
        let furnace_updates = self.world.tick_furnaces();
        let mut lit_changes = Vec::new();
        for (pos, res, data) in furnace_updates {
            if let Some(now_lit) = res.lit_changed_to {
                let st = self.world.get_block(pos);
                let beh = block_behavior(st.0);
                if beh.is_lit() != now_lit {
                    if let Some(toggled) = beh.toggled_lit() {
                        let ns = BlockStateId(toggled);
                        if self.world.set_block_and_relight(pos, ns).is_some() {
                            lit_changes.push((pos, ns));
                        }
                    }
                }
            }
            if res.data_changed {
                for (_, p) in self.players.iter() {
                    if let Some(OpenMenu::Furnace {
                        container_id,
                        pos: fpos,
                    }) = &p.open_menu
                    {
                        if *fpos == pos {
                            for (idx, &val) in data.iter().enumerate() {
                                let _ = p.tx.send(encode_play_container_set_data(
                                    *container_id,
                                    idx as i16,
                                    val,
                                ));
                            }
                        }
                    }
                }
            }
            if res.slots_changed {
                self.sync_open_block_container(pos);
            }
        }
        self.broadcast_block_changes(&lit_changes);

        // 5. Tick all loaded hoppers and sync modified container viewers
        let hopper_modified = self.world.tick_hoppers();
        for pos in hopper_modified {
            self.sync_open_block_container(pos);
        }

        // 6. Tick vanilla FoodData (exhaustion, natural regen, starvation) for survival players
        let nat_regen = self.world.gamerules.natural_regeneration();
        let player_handles: Vec<PlayerHandle> = self.players.iter().map(|(h, _)| h).collect();
        let mut starve_targets = Vec::new();
        for ph in player_handles {
            if let Some(p) = self.players.get_mut(ph) {
                if p.dead || p.gamemode != 0 {
                    continue;
                }
                let mut health_dirty = false;
                if p.food_exhaustion > 4.0 {
                    p.food_exhaustion -= 4.0;
                    if p.saturation > 0.0 {
                        p.saturation = (p.saturation - 1.0).max(0.0);
                    } else {
                        p.food = (p.food - 1).max(0);
                    }
                    health_dirty = true;
                }
                if nat_regen && p.saturation > 0.0 && p.food >= 20 && p.health < 20.0 {
                    p.food_tick_timer += 1;
                    if p.food_tick_timer >= 10 {
                        let spent = p.saturation.min(6.0);
                        p.health = (p.health + spent / 6.0).min(20.0);
                        p.food_exhaustion = (p.food_exhaustion + spent).min(40.0);
                        p.food_tick_timer = 0;
                        health_dirty = true;
                    }
                } else if nat_regen && p.food >= 18 && p.health < 20.0 {
                    p.food_tick_timer += 1;
                    if p.food_tick_timer >= 80 {
                        p.health = (p.health + 1.0).min(20.0);
                        p.food_exhaustion = (p.food_exhaustion + 6.0).min(40.0);
                        p.food_tick_timer = 0;
                        health_dirty = true;
                    }
                } else if p.food <= 0 {
                    p.food_tick_timer += 1;
                    if p.food_tick_timer >= 80 {
                        p.food_tick_timer = 0;
                        if p.health > 1.0 {
                            starve_targets.push(ph);
                        }
                    }
                } else {
                    p.food_tick_timer = 0;
                }
                if health_dirty {
                    let _ = p
                        .tx
                        .send(encode_play_set_health(p.health, p.food, p.saturation));
                }
            }
        }
        for ph in starve_targets {
            self.damage_player(ph, 1.0, "starved to death");
        }

        // 6b. Tick active player MobEffects (Regeneration, Poison, Wither, Hunger, Saturation, Expiration)
        let eff_handles: Vec<PlayerHandle> = self.players.iter().map(|(h, _)| h).collect();
        let mut effect_damages: Vec<(PlayerHandle, f32, &'static str)> = Vec::new();
        for ph in eff_handles {
            if let Some(p) = self.players.get_mut(ph) {
                if p.dead || p.effects.is_empty() {
                    continue;
                }
                let mut health_dirty = false;
                let mut expired = Vec::new();
                for ef in p.effects.iter_mut() {
                    let amp = ef.amplifier.clamp(0, 5);
                    match ef.name.as_str() {
                        "minecraft:regeneration" => {
                            let interval = (50 >> amp).max(1);
                            if ef.duration % interval == 0 && p.health < 20.0 {
                                p.health = (p.health + 1.0).min(20.0);
                                health_dirty = true;
                            }
                        }
                        "minecraft:poison" => {
                            let interval = (25 >> amp).max(1);
                            if ef.duration % interval == 0 && p.health > 1.0 {
                                effect_damages.push((ph, 1.0, "died of poison"));
                            }
                        }
                        "minecraft:wither" => {
                            let interval = (40 >> amp).max(1);
                            if ef.duration % interval == 0 {
                                effect_damages.push((ph, 1.0, "withered away"));
                            }
                        }
                        "minecraft:hunger" => {
                            p.food_exhaustion += 0.005 * (amp + 1) as f32;
                        }
                        "minecraft:saturation" => {
                            p.food = (p.food + amp + 1).min(20);
                            p.saturation = (p.saturation + 2.0 * (amp + 1) as f32).min(p.food as f32);
                            health_dirty = true;
                        }
                        _ => {}
                    }
                    ef.duration -= 1;
                    if ef.duration <= 0 {
                        expired.push(ef.effect_id);
                    }
                }
                if !expired.is_empty() {
                    p.effects.retain(|e| e.duration > 0);
                    for eid in expired {
                        let _ = p.tx.send(encode_play_remove_mob_effect(p.entity_id, eid));
                    }
                }
                if health_dirty {
                    let _ = p.tx.send(encode_play_set_health(p.health, p.food, p.saturation));
                }
            }
        }
        for (ph, dmg, reason) in effect_damages {
            self.damage_player(ph, dmg, reason);
        }

        // 7. Hostile mob chasing & melee combat (every 10 ticks)
        if now % 10 == 0 {
            let survival_players: Vec<(PlayerHandle, Vec3)> = self
                .players
                .iter()
                .filter_map(|(ph, p)| {
                    if !p.dead && p.gamemode == 0 {
                        Some((ph, p.pos))
                    } else {
                        None
                    }
                })
                .collect();
            if !survival_players.is_empty() {
                let mut mob_hits: Vec<(PlayerHandle, String)> = Vec::new();
                for (_, ent) in self.entities.iter_mut() {
                    if let EntityKind::Mob {
                        ref type_name,
                        ref mut wander_dir,
                        ..
                    } = ent.kind
                    {
                        let hostile = matches!(
                            type_name.as_str(),
                            "minecraft:zombie"
                                | "minecraft:skeleton"
                                | "minecraft:spider"
                                | "minecraft:creeper"
                                | "minecraft:husk"
                                | "minecraft:drowned"
                                | "minecraft:stray"
                                | "minecraft:wither_skeleton"
                        );
                        if !hostile {
                            continue;
                        }
                        let mut closest: Option<(PlayerHandle, Vec3, f64)> = None;
                        for &(ph, ppos) in &survival_players {
                            let d2 = (ppos.x - ent.pos.x).powi(2)
                                + (ppos.y - ent.pos.y).powi(2)
                                + (ppos.z - ent.pos.z).powi(2);
                            if d2 < 256.0 && closest.map(|c| d2 < c.2).unwrap_or(true) {
                                closest = Some((ph, ppos, d2));
                            }
                        }
                        if let Some((ph, ppos, d2)) = closest {
                            let dx = ppos.x - ent.pos.x;
                            let dz = ppos.z - ent.pos.z;
                            let hdist = (dx * dx + dz * dz).sqrt().max(0.001);
                            *wander_dir = (dx / hdist, dz / hdist);
                            if d2 <= 3.24 && now % 20 == 0 {
                                mob_hits.push((ph, type_name.clone()));
                            }
                        }
                    }
                }
                for (ph, mname) in mob_hits {
                    let short = mname.strip_prefix("minecraft:").unwrap_or(&mname);
                    self.damage_player(ph, 3.0, &format!("was slain by {short}"));
                }
            }
        }

        // 8. Weather countdown, random block ticks & Nether/End portal entry teleportation
        if self.world.gamerules.do_weather_cycle() && self.world.raining && self.world.rain_time > 0 {
            self.world.rain_time -= 1;
            if self.world.rain_time == 0 {
                self.apply_weather(false, false, 24000);
            }
        }

        let rts = self.world.gamerules.random_tick_speed().max(0) as u32;
        if rts > 0 && now % 20 == 0 {
            let center_chunks: Vec<ChunkPos> = self
                .players
                .iter()
                .filter_map(|(_, p)| {
                    if p.dimension == DimensionId::Overworld {
                        Some(p.center_chunk)
                    } else {
                        None
                    }
                })
                .collect();
            let mut rt_changes = Vec::new();
            for cp in center_chunks {
                rt_changes.extend(run_chunk_random_ticks(
                    &mut self.world,
                    cp.x,
                    cp.z,
                    -64,
                    320,
                    rts,
                    now.wrapping_mul(1_000_003),
                ));
            }
            self.broadcast_block_changes(&rt_changes);
        }

        let portal_candidates: Vec<(PlayerHandle, DimensionId, BlockPos)> = self
            .players
            .iter_mut()
            .filter_map(|(ph, p)| {
                if p.portal_cooldown > 0 {
                    p.portal_cooldown -= 1;
                    return None;
                }
                if p.dead {
                    return None;
                }
                Some((
                    ph,
                    p.dimension,
                    BlockPos::new(
                        p.pos.x.floor() as i32,
                        p.pos.y.floor() as i32,
                        p.pos.z.floor() as i32,
                    ),
                ))
            })
            .collect();
        for (ph, dim, bp) in portal_candidates {
            let chunk = self.world.get_or_load_chunk_in(dim, bp.chunk_pos());
            let st = chunk.get_block((bp.x & 15) as usize, bp.y, (bp.z & 15) as usize);
            let kind = block_behavior(st.0).kind;
            let target_dim = if kind == KIND_NETHER_PORTAL {
                Some(if dim == DimensionId::Nether {
                    DimensionId::Overworld
                } else {
                    DimensionId::Nether
                })
            } else if kind == KIND_END_PORTAL {
                Some(if dim == DimensionId::End {
                    DimensionId::Overworld
                } else {
                    DimensionId::End
                })
            } else {
                None
            };
            if let Some(tdim) = target_dim {
                if let Some(p) = self.players.get_mut(ph) {
                    p.portal_cooldown = 80;
                }
                self.switch_dimension(ph, tdim);
            }
        }
    }

    fn build_menu_slots(&self, p: &PlayerEntity, menu: &OpenMenu) -> Option<Vec<ItemStack>> {
        let mut out = match menu {
            OpenMenu::Chest { pos, .. }
            | OpenMenu::Barrel { pos, .. }
            | OpenMenu::Furnace { pos, .. }
            | OpenMenu::Hopper { pos, .. } => {
                let be = self.world.get_block_entity(*pos)?;
                be.items_slice()?.to_vec()
            }
            OpenMenu::CraftingTable { slots, .. } => slots.clone(),
        };
        // Append player main inventory (9..36) + hotbar (36..45)
        out.extend_from_slice(&p.inventory.slots[9..45]);
        Some(out)
    }

    fn sync_open_block_container(&mut self, pos: BlockPos) {
        let viewers: Vec<(PlayerHandle, i32)> = self
            .players
            .iter()
            .filter_map(|(h, p)| {
                let m = p.open_menu.as_ref()?;
                if m.block_pos() == Some(pos) {
                    Some((h, m.container_id()))
                } else {
                    None
                }
            })
            .collect();
        for (vh, cid) in viewers {
            let menu_opt = self.players.get(vh).and_then(|p| p.open_menu.clone());
            if let Some(menu) = menu_opt {
                if let Some(slots) = self
                    .players
                    .get(vh)
                    .and_then(|p| self.build_menu_slots(p, &menu))
                {
                    if let Some(p) = self.players.get_mut(vh) {
                        let sid = p.inventory.next_state_id();
                        let _ = p.tx.send(encode_play_container_set_content(
                            cid,
                            sid,
                            &slots,
                            &p.inventory.carried,
                        ));
                    }
                }
            }
        }
    }

    fn open_block_menu(&mut self, handle: PlayerHandle, pos: BlockPos) -> bool {
        let st = self.world.get_block(pos);
        let kind = block_behavior(st.0).kind;
        let (menu_type, title, is_be) = match kind {
            KIND_CHEST => (menu_types::GENERIC_9X3, "Chest", true),
            KIND_BARREL => (menu_types::GENERIC_9X3, "Barrel", true),
            KIND_FURNACE => (menu_types::FURNACE, "Furnace", true),
            KIND_BLAST_FURNACE => (menu_types::BLAST_FURNACE, "Blast Furnace", true),
            KIND_SMOKER => (menu_types::SMOKER, "Smoker", true),
            KIND_HOPPER => (menu_types::HOPPER, "Item Hopper", true),
            KIND_CRAFTING_TABLE => (menu_types::CRAFTING, "Crafting", false),
            _ => return false,
        };

        let mut furnace_data: Option<[i16; 4]> = None;
        if is_be {
            let Some(be) = self.world.ensure_block_entity(pos) else {
                return false;
            };
            if let BlockEntityKind::Furnace {
                lit_time,
                lit_duration,
                cook_progress,
                cook_total,
                ..
            } = &be.kind
            {
                furnace_data = Some([*lit_time, *lit_duration, *cook_progress, *cook_total]);
            }
        }

        let cid = {
            let Some(p) = self.players.get_mut(handle) else {
                return false;
            };
            let id = p.next_container_id;
            p.next_container_id = (p.next_container_id % 100) + 1;
            let menu = match kind {
                KIND_CHEST => OpenMenu::Chest {
                    container_id: id,
                    pos,
                },
                KIND_BARREL => OpenMenu::Barrel {
                    container_id: id,
                    pos,
                },
                KIND_FURNACE | KIND_BLAST_FURNACE | KIND_SMOKER => OpenMenu::Furnace {
                    container_id: id,
                    pos,
                },
                KIND_HOPPER => OpenMenu::Hopper {
                    container_id: id,
                    pos,
                },
                _ => OpenMenu::CraftingTable {
                    container_id: id,
                    slots: vec![ItemStack::EMPTY; 10],
                },
            };
            p.open_menu = Some(menu);
            id
        };

        let menu_clone = self
            .players
            .get(handle)
            .and_then(|p| p.open_menu.clone())
            .unwrap();
        let slots = self
            .players
            .get(handle)
            .and_then(|p| self.build_menu_slots(p, &menu_clone))
            .unwrap_or_default();

        if let Some(p) = self.players.get_mut(handle) {
            if let Ok(open_pkt) = encode_play_open_screen(cid, menu_type, title) {
                let _ = p.tx.send(open_pkt);
            }
            let sid = p.inventory.next_state_id();
            let _ = p.tx.send(encode_play_container_set_content(
                cid,
                sid,
                &slots,
                &p.inventory.carried,
            ));
            if let Some(data) = furnace_data {
                for (i, &v) in data.iter().enumerate() {
                    let _ = p.tx.send(encode_play_container_set_data(cid, i as i16, v));
                }
            }
        }
        true
    }

    fn handle_container_click(
        &mut self,
        handle: PlayerHandle,
        container_id: i32,
        slot_num: i16,
        button_num: i8,
        click_type: ContainerClickType,
    ) {
        let mut dropped_item: Option<(Vec3, Vec3, ItemStack)> = None;

        if container_id == 0 {
            if let Some(p) = self.players.get_mut(handle) {
                let is_creative = p.gamemode == 1;
                let res = apply_container_click(
                    &mut p.inventory.slots,
                    &mut p.inventory.carried,
                    slot_num,
                    button_num,
                    click_type,
                    9,
                    Some((2, 2)),
                    36,
                    is_creative,
                );
                if let Some(item) = res.dropped {
                    let yaw_rad = (p.yaw as f64).to_radians();
                    dropped_item = Some((
                        Vec3::new(p.pos.x, p.pos.y + 1.3, p.pos.z),
                        Vec3::new(-yaw_rad.sin() * 0.3, 0.2, yaw_rad.cos() * 0.3),
                        item,
                    ));
                }
                let sid = p.inventory.next_state_id();
                let _ = p.tx.send(encode_play_container_set_content(
                    0,
                    sid,
                    &p.inventory.slots,
                    &p.inventory.carried,
                ));
            }
        } else {
            let menu_opt = self.players.get(handle).and_then(|p| p.open_menu.clone());
            if let Some(mut menu) = menu_opt {
                if menu.container_id() == container_id {
                    let (top_count, crafting_grid, hotbar_start, block_pos) = match &menu {
                        OpenMenu::Chest { pos, .. } | OpenMenu::Barrel { pos, .. } => {
                            (27, None, 54, Some(*pos))
                        }
                        OpenMenu::Furnace { pos, .. } => (3, None, 30, Some(*pos)),
                        OpenMenu::Hopper { pos, .. } => (5, None, 32, Some(*pos)),
                        OpenMenu::CraftingTable { .. } => (10, Some((3, 3)), 37, None),
                    };
                    if let Some(mut flat) = self
                        .players
                        .get(handle)
                        .and_then(|p| self.build_menu_slots(p, &menu))
                    {
                        let mut carried = self
                            .players
                            .get(handle)
                            .map(|p| p.inventory.carried.clone())
                            .unwrap_or(ItemStack::EMPTY);
                        let is_creative = self
                            .players
                            .get(handle)
                            .map(|p| p.gamemode == 1)
                            .unwrap_or(false);

                        let res = apply_container_click(
                            &mut flat,
                            &mut carried,
                            slot_num,
                            button_num,
                            click_type,
                            top_count,
                            crafting_grid,
                            hotbar_start,
                            is_creative,
                        );

                        // Write back top slots to BlockEntity or CraftingTable
                        if let Some(bp) = block_pos {
                            if let Some(be) = self.world.get_block_entity_mut(bp) {
                                if let Some(be_slots) = be.items_slice_mut() {
                                    be_slots.clone_from_slice(&flat[..top_count]);
                                }
                            }
                        } else if let OpenMenu::CraftingTable { ref mut slots, .. } = menu {
                            slots.clone_from_slice(&flat[..top_count]);
                        }

                        // Write back player inventory slots (9..45) + carried
                        if let Some(p) = self.players.get_mut(handle) {
                            p.inventory.slots[9..45]
                                .clone_from_slice(&flat[top_count..top_count + 36]);
                            p.inventory.carried = carried;
                            p.open_menu = Some(menu);
                            if let Some(item) = res.dropped {
                                let yaw_rad = (p.yaw as f64).to_radians();
                                dropped_item = Some((
                                    Vec3::new(p.pos.x, p.pos.y + 1.3, p.pos.z),
                                    Vec3::new(-yaw_rad.sin() * 0.3, 0.2, yaw_rad.cos() * 0.3),
                                    item,
                                ));
                            }
                            let sid = p.inventory.next_state_id();
                            let _ = p.tx.send(encode_play_container_set_content(
                                container_id,
                                sid,
                                &flat,
                                &p.inventory.carried,
                            ));
                        }

                        if let Some(bp) = block_pos {
                            self.sync_open_block_container(bp);
                        }
                    }
                }
            }
        }

        if let Some((pos, vel, stack)) = dropped_item {
            let eid = self.alloc_entity_id();
            self.spawn_world_entity(WorldEntity::spawn_item(eid, pos, vel, stack, 40));
        }
    }

    fn handle_container_close(&mut self, handle: PlayerHandle, container_id: i32) {
        let mut to_return: Vec<ItemStack> = Vec::new();
        if let Some(p) = self.players.get_mut(handle) {
            if container_id == 0 {
                // Return any items in 2x2 crafting grid (slots 1..=4)
                for i in 1..=4 {
                    if !p.inventory.slots[i].is_empty() {
                        to_return.push(std::mem::take(&mut p.inventory.slots[i]));
                    }
                }
                p.inventory.slots[0] = ItemStack::EMPTY;
            } else if let Some(menu) = p.open_menu.take() {
                if let OpenMenu::CraftingTable { slots, .. } = menu {
                    for item in slots.into_iter().skip(1) {
                        if !item.is_empty() {
                            to_return.push(item);
                        }
                    }
                }
            }
            if !p.inventory.carried.is_empty() {
                to_return.push(std::mem::take(&mut p.inventory.carried));
            }
            for stack in &to_return {
                p.inventory.give_item(stack.item_id, stack.count as u32);
            }
            if !to_return.is_empty() {
                let sid = p.inventory.next_state_id();
                let _ = p.tx.send(encode_play_container_set_content(
                    0,
                    sid,
                    &p.inventory.slots,
                    &p.inventory.carried,
                ));
            }
        }
    }

    fn damage_player(&mut self, handle: PlayerHandle, amount: f32, reason: &str) {
        if amount <= 0.0 {
            return;
        }
        if reason.contains("fell") && !self.world.gamerules.fall_damage() {
            return;
        }
        let keep_inv = self.world.gamerules.keep_inventory();
        let mut hurt_pkt = None;
        let mut death_info: Option<(i32, String, Vec3, Vec<ItemStack>)> = None;

        if let Some(p) = self.players.get_mut(handle) {
            if p.dead || p.gamemode != 0 {
                return;
            }
            // Exact vanilla armor mitigation (CombatRules.getDamageAfterAbsorb) unless fall/starvation
            let bypasses_armor = reason.contains("fell") || reason.contains("starved");
            let effective = if bypasses_armor {
                amount
            } else {
                let mut total_armor = 0.0_f32;
                let mut total_tough = 0.0_f32;
                for slot in 5..=8 {
                    let st = &p.inventory.slots[slot];
                    if !st.is_empty() {
                        let cs = item_combat_stats(st.item_id);
                        total_armor += cs.armor;
                        total_tough += cs.armor_toughness;
                    }
                }
                let f = (total_armor / 5.0)
                    .max(total_armor - 4.0 * amount / (total_tough + 8.0))
                    .clamp(0.0, 20.0);
                amount * (1.0 - f / 25.0)
            };
            p.health = (p.health - effective).max(0.0);
            let _ = p
                .tx
                .send(encode_play_set_health(p.health, p.food, p.saturation));
            hurt_pkt = Some(encode_play_hurt_animation(p.entity_id, 0.0));
            if p.health <= 0.0 {
                p.dead = true;
                let death_bp = BlockPos::new(
                    p.pos.x.floor() as i32,
                    p.pos.y.floor() as i32,
                    p.pos.z.floor() as i32,
                );
                p.last_death = Some(("minecraft:overworld".into(), death_bp));
                if let Some(menu) = p.open_menu.take() {
                    let _ = p.tx.send(encode_play_container_close(menu.container_id()));
                }
                let mut drops = Vec::new();
                if !keep_inv {
                    for s in p.inventory.slots.iter_mut() {
                        if !s.is_empty() {
                            drops.push(std::mem::take(s));
                        }
                    }
                }
                death_info = Some((
                    p.entity_id,
                    format!("{} {reason}", p.profile.name),
                    p.pos,
                    drops,
                ));
            }
        }

        if let Some(hp) = hurt_pkt {
            for (_, other) in self.players.iter() {
                let _ = other.tx.send(hp.clone());
            }
        }
        if let Some((eid, msg, pos, drops)) = death_info {
            let death_ev = encode_play_entity_event(eid, 3);
            let chat_pkt = encode_play_system_chat(&msg, false).unwrap_or_default();
            for (_, other) in self.players.iter() {
                let _ = other.tx.send(death_ev.clone());
                let _ = other.tx.send(chat_pkt.clone());
            }
            for stack in drops {
                let ieid = self.alloc_entity_id();
                self.spawn_world_entity(WorldEntity::spawn_item(
                    ieid,
                    Vec3::new(pos.x, pos.y + 0.5, pos.z),
                    Vec3::new(0.0, 0.2, 0.0),
                    stack,
                    10,
                ));
            }
        }
    }

    fn handle_respawn(&mut self, handle: PlayerHandle) {
        let spawn_pos = self
            .players
            .get(handle)
            .and_then(|p| p.spawnpoint)
            .unwrap_or(self.world.spawn_pos);
        let spawn_vec = Vec3::new(
            spawn_pos.x as f64 + 0.5,
            spawn_pos.y as f64,
            spawn_pos.z as f64 + 0.5,
        );
        let hashed_seed = self.world.hashed_seed;
        let dim_type = self.overworld_dim_id;

        if let Some(p) = self.players.get_mut(handle) {
            p.health = 20.0;
            p.food = 20;
            p.saturation = 5.0;
            p.fall_distance = 0.0;
            p.dead = false;
            p.pos = spawn_vec;
            p.on_ground = true;

            let respawn = PlayRespawnPacket {
                dimension_type_id: dim_type,
                dimension_name: "minecraft:overworld".into(),
                hashed_seed,
                game_type: p.gamemode,
                previous_game_type: Some(p.gamemode),
                is_debug: false,
                is_flat: false,
                last_death_location: p.last_death.clone(),
                portal_cooldown: 0,
                sea_level: 63,
                data_to_keep: 0,
            };
            let _ = p.tx.send(respawn.encode());
            let _ = p
                .tx
                .send(encode_play_set_health(p.health, p.food, p.saturation));
            let _ = p.tx.send(abilities_packet_for_gamemode(p.gamemode));
            let tp_id = p.next_teleport_id;
            p.next_teleport_id += 1;
            let _ = p.tx.send(encode_play_player_position(
                tp_id,
                spawn_vec,
                Vec3::ZERO,
                0.0,
                0.0,
                0,
            ));
            let sid = p.inventory.next_state_id();
            let _ = p.tx.send(encode_play_container_set_content(
                0,
                sid,
                &p.inventory.slots,
                &p.inventory.carried,
            ));
            stream_chunks_for_player(p, &mut self.world);
        }
    }

    fn dim_type_id(&self, dim: DimensionId) -> i32 {
        match dim {
            DimensionId::Overworld => self.overworld_dim_id,
            DimensionId::Nether => self.nether_dim_id,
            DimensionId::End => self.end_dim_id,
        }
    }

    fn switch_dimension(&mut self, handle: PlayerHandle, target: DimensionId) {
        let dest_bp = match target {
            DimensionId::Overworld => self.world.spawn_pos,
            DimensionId::Nether => BlockPos::new(0, 64, 0),
            DimensionId::End => BlockPos::new(0, 66, 0),
        };
        let dest_vec = Vec3::new(
            dest_bp.x as f64 + 0.5,
            dest_bp.y as f64,
            dest_bp.z as f64 + 0.5,
        );
        let center = dest_bp.chunk_pos();
        let dim_type_id = self.dim_type_id(target);
        let hashed_seed = self.world.hashed_seed;

        if let Some(p) = self.players.get_mut(handle) {
            p.dimension = target;
            p.pos = dest_vec;
            p.fall_distance = 0.0;
            p.center_chunk = center;
            p.loaded_chunks.clear();
            p.open_menu = None;

            let respawn = PlayRespawnPacket {
                dimension_type_id: dim_type_id,
                dimension_name: target.name().to_string(),
                hashed_seed,
                game_type: p.gamemode,
                previous_game_type: Some(p.gamemode),
                is_debug: false,
                is_flat: false,
                last_death_location: p.last_death.clone(),
                portal_cooldown: 0,
                sea_level: target.sea_level(),
                data_to_keep: 3, // KEEP_ALL_DATA
            };
            let _ = p.tx.send(respawn.encode());
            let _ = p.tx.send(abilities_packet_for_gamemode(p.gamemode));
            let _ = p
                .tx
                .send(encode_play_set_health(p.health, p.food, p.saturation));
            let _ = p
                .tx
                .send(encode_play_set_chunk_cache_center(center.x, center.z));

            let r = p.view_distance.min(2);
            let _ = p.tx.send(encode_play_chunk_batch_start());
            let mut sent = 0;
            for dz in -r..=r {
                for dx in -r..=r {
                    let cp = ChunkPos::new(center.x + dx, center.z + dz);
                    let chunk = self.world.get_or_load_chunk_in(target, cp);
                    let _ = p.tx.send(encode_play_level_chunk_with_light(chunk));
                    p.loaded_chunks.insert(cp);
                    sent += 1;
                }
            }
            let _ = p.tx.send(encode_play_chunk_batch_finished(sent));

            let tp_id = p.next_teleport_id;
            p.next_teleport_id += 1;
            let _ = p.tx.send(encode_play_player_position(
                tp_id,
                dest_vec,
                Vec3::ZERO,
                p.yaw,
                p.pitch,
                0,
            ));
            let pname = p.profile.name.clone();
            let awarded = p.advancements.check_dimension_entered(target.name());
            if !awarded.is_empty() {
                self.announce_advancements(&pname, awarded);
            }
        }
    }

    fn kick_player_by_name(&mut self, name: &str, reason: &str) -> bool {
        let target = self
            .players
            .iter()
            .find_map(|(h, p)| {
                if p.profile.name.eq_ignore_ascii_case(name) {
                    Some(h)
                } else {
                    None
                }
            });
        if let Some(h) = target {
            if let Some(p) = self.players.get(h) {
                if let Ok(pkt) = encode_play_disconnect(reason) {
                    let _ = p.tx.send(pkt);
                }
            }
            true
        } else {
            false
        }
    }

    fn execute_admin_or_console(
        &mut self,
        raw: &str,
        ctx: &SharedContext,
        stop_signal: &mut Option<oneshot::Sender<()>>,
    ) -> (String, bool) {
        let action = commands::parse_command(raw, Vec3::ZERO);
        match action {
            CommandAction::Reply(msg) => (msg, false),
            CommandAction::ListPlayers => {
                let names: Vec<String> = self
                    .players
                    .iter()
                    .map(|(_, pl)| pl.profile.name.clone())
                    .collect();
                (
                    format!(
                        "There are {} of a max of {} players online: {}",
                        names.len(),
                        ctx.config.max_players,
                        names.join(", ")
                    ),
                    false,
                )
            }
            CommandAction::Seed => (format!("Seed: [{}]", self.world.seed), false),
            CommandAction::SaveAll => {
                self.save_all_players();
                match self.world.save_dirty_chunks() {
                    Ok(n) => (
                        format!("Saved the game ({n} dirty chunk(s), level.dat, and playerdata)"),
                        false,
                    ),
                    Err(e) => (format!("Save failed: {e}"), false),
                }
            }
            CommandAction::SetWeather {
                raining,
                thundering,
                duration_ticks,
            } => {
                self.apply_weather(raining, thundering, duration_ticks);
                let label = if !raining {
                    "clear"
                } else if thundering {
                    "rain & thunder"
                } else {
                    "rain"
                };
                (format!("Set the weather to {label}"), false)
            }
            CommandAction::SetTime(ticks) => {
                self.world.day_time = ticks;
                let tp = encode_play_set_time(
                    self.world.game_time,
                    &[(self.overworld_clock_id, ticks, 0.0, 1.0)],
                );
                for (_, other) in self.players.iter() {
                    let _ = other.tx.send(tp.clone());
                }
                (format!("Set the time to {ticks}"), false)
            }
            CommandAction::QueryTime => (
                format!(
                    "The time is {} (game_time={})",
                    self.world.day_time % 24000,
                    self.world.game_time
                ),
                false,
            ),
            CommandAction::Op(name) => {
                let uuid = offline_player_uuid(&name);
                if self.admin.add_op(uuid, &name) {
                    (format!("Made {name} a server operator"), false)
                } else {
                    (format!("{name} is already an operator"), false)
                }
            }
            CommandAction::Deop(name) => {
                if self.admin.remove_op(&name) {
                    (format!("Made {name} no longer a server operator"), false)
                } else {
                    (format!("{name} is not an operator"), false)
                }
            }
            CommandAction::Ban { player, reason } => {
                let uuid = offline_player_uuid(&player);
                self.admin.add_ban(uuid, &player, &reason, "Server");
                self.kick_player_by_name(&player, &format!("You are banned: {reason}"));
                (format!("Banned {player}: {reason}"), false)
            }
            CommandAction::Pardon(player) => {
                if self.admin.remove_ban(&player) {
                    (format!("Unbanned {player}"), false)
                } else {
                    (format!("{player} is not banned"), false)
                }
            }
            CommandAction::Kick { player, reason } => {
                if self.kick_player_by_name(&player, &reason) {
                    (format!("Kicked {player}: {reason}"), false)
                } else {
                    (format!("Player {player} is not online"), false)
                }
            }
            CommandAction::Whitelist { action, player } => match action.as_str() {
                "on" => {
                    self.admin.whitelist_enabled = true;
                    ("Whitelist is now turned on".into(), false)
                }
                "off" => {
                    self.admin.whitelist_enabled = false;
                    ("Whitelist is now turned off".into(), false)
                }
                "list" => {
                    let names: Vec<&str> = self
                        .admin
                        .whitelist
                        .iter()
                        .map(|w| w.name.as_str())
                        .collect();
                    (
                        format!(
                            "There are {} whitelisted player(s): {}",
                            names.len(),
                            names.join(", ")
                        ),
                        false,
                    )
                }
                "add" => {
                    if let Some(name) = player {
                        let uuid = offline_player_uuid(&name);
                        self.admin.add_whitelist(uuid, &name);
                        (format!("Added {name} to the whitelist"), false)
                    } else {
                        ("Usage: /whitelist add <player>".into(), false)
                    }
                }
                "remove" => {
                    if let Some(name) = player {
                        self.admin.remove_whitelist(&name);
                        (format!("Removed {name} from the whitelist"), false)
                    } else {
                        ("Usage: /whitelist remove <player>".into(), false)
                    }
                }
                _ => ("Usage: /whitelist <on|off|list|add|remove> [player]".into(), false),
            },
            CommandAction::StopServer => {
                self.save_all_players();
                let _ = self.world.save_dirty_chunks();
                if let Ok(pkt) = encode_play_system_chat("Stopping the server", false) {
                    for (_, other) in self.players.iter() {
                        let _ = other.tx.send(pkt.clone());
                    }
                }
                if let Some(sig) = stop_signal.take() {
                    let _ = sig.send(());
                }
                ("Stopping the server".into(), true)
            }
            _ => ("That command requires an in-game player context.".into(), false),
        }
    }

    fn handle_jsonrpc(
        &mut self,
        method: &str,
        params: serde_json::Value,
        stop_signal: &mut Option<oneshot::Sender<()>>,
    ) -> (Result<serde_json::Value, (i32, String)>, bool) {
        use serde_json::json;
        match method {
            "minecraft:server/status" => {
                let players: Vec<serde_json::Value> = self
                    .players
                    .iter()
                    .map(|(_, p)| json!({ "id": p.profile.uuid.to_string(), "name": p.profile.name }))
                    .collect();
                (
                    Ok(json!({
                        "started": true,
                        "players": players,
                        "version": {
                            "name": VERSION_NAME,
                            "protocol": PROTOCOL_VERSION
                        }
                    })),
                    false,
                )
            }
            "minecraft:players" => {
                let players: Vec<serde_json::Value> = self
                    .players
                    .iter()
                    .map(|(_, p)| json!({ "id": p.profile.uuid.to_string(), "name": p.profile.name }))
                    .collect();
                (Ok(json!(players)), false)
            }
            "minecraft:server/save" => {
                self.save_all_players();
                match self.world.save_dirty_chunks() {
                    Ok(_) => (Ok(json!(true)), false),
                    Err(e) => (Err((-32000, e)), false),
                }
            }
            "minecraft:server/stop" => {
                self.save_all_players();
                let _ = self.world.save_dirty_chunks();
                if let Some(sig) = stop_signal.take() {
                    let _ = sig.send(());
                }
                (Ok(json!(true)), true)
            }
            "minecraft:server/system_message" => {
                let msg = params
                    .pointer("/message/literal")
                    .or_else(|| params.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Ok(pkt) = encode_play_system_chat(msg, false) {
                    for (_, other) in self.players.iter() {
                        let _ = other.tx.send(pkt.clone());
                    }
                }
                (Ok(json!(true)), false)
            }
            "minecraft:operators" => {
                let ops: Vec<serde_json::Value> = self
                    .admin
                    .ops
                    .iter()
                    .map(|o| {
                        json!({
                            "player": { "id": o.uuid, "name": o.name },
                            "permissionLevel": o.level,
                            "bypassesPlayerLimit": o.bypasses_player_limit
                        })
                    })
                    .collect();
                (Ok(json!(ops)), false)
            }
            "minecraft:operators/add" => {
                if let Some(name) = params.pointer("/add/0/player/name").or_else(|| params.get("name")).and_then(|v| v.as_str()) {
                    self.admin.add_op(offline_player_uuid(name), name);
                }
                (Ok(json!(true)), false)
            }
            "minecraft:operators/remove" => {
                if let Some(name) = params.pointer("/remove/0/name").or_else(|| params.get("name")).and_then(|v| v.as_str()) {
                    self.admin.remove_op(name);
                }
                (Ok(json!(true)), false)
            }
            "minecraft:allowlist" => {
                let list: Vec<serde_json::Value> = self
                    .admin
                    .whitelist
                    .iter()
                    .map(|w| json!({ "id": w.uuid, "name": w.name }))
                    .collect();
                (Ok(json!(list)), false)
            }
            "minecraft:bans" => {
                let list: Vec<serde_json::Value> = self
                    .admin
                    .bans
                    .iter()
                    .map(|b| {
                        json!({
                            "player": { "id": b.uuid, "name": b.name },
                            "reason": b.reason,
                            "source": b.source,
                            "expires": b.expires
                        })
                    })
                    .collect();
                (Ok(json!(list)), false)
            }
            other => (Err((-32601, format!("Method not found: {other}"))), false),
        }
    }
}

fn int_to_bossbar_color(c: i32) -> BossBarColor {
    match c {
        0 => BossBarColor::Pink,
        1 => BossBarColor::Blue,
        2 => BossBarColor::Red,
        3 => BossBarColor::Green,
        4 => BossBarColor::Yellow,
        5 => BossBarColor::Purple,
        _ => BossBarColor::White,
    }
}

fn int_to_bossbar_overlay(o: i32) -> BossBarOverlay {
    match o {
        1 => BossBarOverlay::Notched6,
        2 => BossBarOverlay::Notched10,
        3 => BossBarOverlay::Notched12,
        4 => BossBarOverlay::Notched20,
        _ => BossBarOverlay::Progress,
    }
}

fn abilities_packet_for_gamemode(gamemode: i32) -> Vec<u8> {
    let (invulnerable, flying, may_fly, instabuild) = match gamemode {
        1 => (true, false, true, true),
        3 => (true, true, true, false),
        _ => (false, false, false, false),
    };
    encode_play_player_abilities(invulnerable, flying, may_fly, instabuild, 0.05, 0.1)
}

fn stream_chunks_for_player(p: &mut PlayerEntity, world: &mut WorldStore) {
    let new_center = ChunkPos::new(
        (p.pos.x.floor() as i32) >> 4,
        (p.pos.z.floor() as i32) >> 4,
    );
    if new_center == p.center_chunk {
        return;
    }
    p.center_chunk = new_center;
    let _ = p
        .tx
        .send(encode_play_set_chunk_cache_center(new_center.x, new_center.z));
    let r = p.view_distance;
    let mut to_send = Vec::new();
    for dz in -r..=r {
        for dx in -r..=r {
            let cp = ChunkPos::new(new_center.x + dx, new_center.z + dz);
            if p.loaded_chunks.insert(cp) {
                to_send.push(cp);
            }
        }
    }
    if !to_send.is_empty() {
        let _ = p.tx.send(encode_play_chunk_batch_start());
        for cp in &to_send {
            let chunk = world.get_or_load_chunk_in(p.dimension, *cp);
            let _ = p.tx.send(encode_play_level_chunk_with_light(chunk));
        }
        let _ = p
            .tx
            .send(encode_play_chunk_batch_finished(to_send.len() as i32));
    }
}

async fn run_tick_loop(
    ctx: Arc<SharedContext>,
    world: WorldStore,
    mut rx: mpsc::UnboundedReceiver<TickEvent>,
    mut stop_signal: Option<oneshot::Sender<()>>,
) {
    let overworld_dim_id = ctx.datapacks.dimension_type_id("minecraft:overworld") as i32;
    let nether_dim_id = ctx.datapacks.dimension_type_id("minecraft:the_nether") as i32;
    let end_dim_id = ctx.datapacks.dimension_type_id("minecraft:the_end") as i32;
    let overworld_clock_id = ctx.datapacks.world_clock_id("minecraft:overworld") as i32;
    let admin = AdminStore::load(&ctx.config.world_dir, ctx.config.white_list);
    let mut sim = SimState {
        world,
        admin,
        scoreboard: ScoreboardStore::new(),
        players: Arena::new(),
        entities: Arena::new(),
        scheduler: TickScheduler::new(),
        next_entity_id: 1,
        overworld_dim_id,
        nether_dim_id,
        end_dim_id,
        overworld_clock_id,
    };
    let mut ticker = tokio::time::interval(Duration::from_millis(50)); // 20 TPS

    loop {
        tokio::select! {
            Some(ev) = rx.recv() => {
                if process_event(ev, &ctx, &mut sim, &mut stop_signal) {
                    break;
                }
            }
            _ = ticker.tick() => {
                sim.world.game_time += 1;
                if sim.world.gamerules.do_daylight_cycle() {
                    sim.world.day_time += 1;
                }

                let mut should_exit = false;
                while let Ok(ev) = rx.try_recv() {
                    if process_event(ev, &ctx, &mut sim, &mut stop_signal) {
                        should_exit = true;
                        break;
                    }
                }
                if should_exit {
                    break;
                }

                // Run deterministic scheduled ticks + entity physics/AI/pickups
                sim.tick_scheduled_and_entities();

                if sim.world.game_time % 20 == 0 && !sim.players.is_empty() {
                    let time_pkt = encode_play_set_time(
                        sim.world.game_time,
                        &[(sim.overworld_clock_id, sim.world.day_time, 0.0, 1.0)],
                    );
                    for (_, p) in sim.players.iter() {
                        let _ = p.tx.send(time_pkt.clone());
                    }
                }

                if sim.world.game_time % 300 == 0 && !sim.players.is_empty() {
                    let now_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(sim.world.game_time);
                    let ka = encode_play_keep_alive(now_ms);
                    for (_, p) in sim.players.iter() {
                        let _ = p.tx.send(ka.clone());
                    }
                }
            }
        }
    }
}

fn process_event(
    ev: TickEvent,
    ctx: &SharedContext,
    sim: &mut SimState,
    stop_signal: &mut Option<oneshot::Sender<()>>,
) -> bool {
    match ev {
        TickEvent::CheckLogin { name, reply } => {
            let deny = if let Some(reason) = sim.admin.ban_reason(&name) {
                Some(format!("You are banned from this server: {reason}"))
            } else if !sim.admin.is_whitelisted(&name) {
                Some("You are not white-listed on this server!".into())
            } else {
                None
            };
            let _ = reply.send(deny);
        }

        TickEvent::ConsoleCommand { command, reply } => {
            let (msg, should_stop) = sim.execute_admin_or_console(&command, ctx, stop_signal);
            let _ = reply.send(msg);
            if should_stop {
                return true;
            }
        }

        TickEvent::JsonRpc {
            method,
            params,
            reply,
        } => {
            let (res, should_stop) = sim.handle_jsonrpc(&method, params, stop_signal);
            let _ = reply.send(res);
            if should_stop {
                return true;
            }
        }

        TickEvent::QueryStatus { reply } => {
            let players: Vec<String> = sim
                .players
                .iter()
                .map(|(_, p)| p.profile.name.clone())
                .collect();
            let world_name = sim
                .world
                .world_dir()
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("world")
                .to_string();
            let _ = reply.send(QuerySnapshot {
                motd: ctx.config.motd.clone(),
                world_name,
                max_players: ctx.config.max_players,
                server_port: ctx.config.port,
                host_ip: ctx.config.bind_addr.clone(),
                players,
            });
        }

        TickEvent::Shutdown { done } => {
            sim.save_all_players();
            if let Err(e) = sim.world.save_dirty_chunks() {
                error!("Failed saving dirty chunks on shutdown: {e}");
            }
            let _ = done.send(());
            return true;
        }

        TickEvent::Join {
            profile,
            view_distance,
            tx,
            reply,
        } => {
            let entity_id = sim.alloc_entity_id();
            let saved_opt = load_player_data(sim.world.world_dir(), profile.uuid);

            let spawn_pos = sim.world.spawn_pos;
            let default_spawn_vec = Vec3::new(
                spawn_pos.x as f64 + 0.5,
                spawn_pos.y as f64,
                spawn_pos.z as f64 + 0.5,
            );
            let (
                dimension,
                spawn_vec,
                yaw,
                pitch,
                gamemode,
                health,
                food,
                saturation,
                food_exhaustion,
                xp_level,
                xp_progress,
                xp_total,
                inventory,
            ) = if let Some(s) = saved_opt {
                let mut inv = PlayerInventory::new();
                inv.slots = s.slots;
                inv.selected_hotbar = s.selected_hotbar;
                (
                    s.dimension,
                    s.pos,
                    s.yaw,
                    s.pitch,
                    s.gamemode,
                    s.health,
                    s.food,
                    s.saturation,
                    s.food_exhaustion,
                    s.xp_level,
                    s.xp_progress,
                    s.xp_total,
                    inv,
                )
            } else {
                (
                    DimensionId::Overworld,
                    default_spawn_vec,
                    0.0_f32,
                    0.0_f32,
                    ctx.config.gamemode,
                    20.0_f32,
                    20_i32,
                    5.0_f32,
                    0.0_f32,
                    0_i32,
                    0.0_f32,
                    0_i32,
                    PlayerInventory::new(),
                )
            };

            let center_chunk = ChunkPos::new(
                (spawn_vec.x.floor() as i32) >> 4,
                (spawn_vec.z.floor() as i32) >> 4,
            );
            let radius = view_distance.min(ctx.config.view_distance).clamp(1, 16);

            let login_pkt = PlayLoginPacket {
                player_id: entity_id,
                hardcore: false,
                levels: vec![
                    "minecraft:overworld".into(),
                    "minecraft:the_nether".into(),
                    "minecraft:the_end".into(),
                ],
                max_players: ctx.config.max_players,
                chunk_radius: radius,
                simulation_distance: ctx.config.simulation_distance,
                reduced_debug_info: false,
                show_death_screen: true,
                do_limited_crafting: false,
                dimension_type_id: sim.dim_type_id(dimension),
                dimension_name: dimension.name().to_string(),
                hashed_seed: sim.world.hashed_seed,
                game_type: gamemode,
                previous_game_type: None,
                is_debug: false,
                is_flat: false,
                last_death_location: None,
                portal_cooldown: 0,
                sea_level: dimension.sea_level(),
                online_mode: ctx.config.online_mode,
                enforces_secure_chat: false,
            };
            let _ = tx.send(login_pkt.encode());

            let _ = tx.send(abilities_packet_for_gamemode(gamemode));
            let _ = tx.send(encode_play_set_health(health, food, saturation));
            let _ = tx.send(encode_play_set_experience(xp_progress, xp_level, xp_total));
            let _ = tx.send(encode_play_set_held_slot(inventory.selected_hotbar as i32));
            let _ = tx.send(ctx.commands_packet.clone());
            let _ = tx.send(encode_play_set_time(
                sim.world.game_time,
                &[(sim.overworld_clock_id, sim.world.day_time, 0.0, 1.0)],
            ));
            let _ = tx.send(encode_play_container_set_content(
                0,
                inventory.state_id,
                &inventory.slots,
                &inventory.carried,
            ));

            let _ = tx.send(encode_play_set_default_spawn(
                "minecraft:overworld",
                spawn_pos,
                0.0,
                0.0,
            ));
            let _ = tx.send(encode_play_game_event(13, 0.0));
            if sim.world.raining {
                let _ = tx.send(encode_play_game_event(1, 0.0));
                let _ = tx.send(encode_play_game_event(7, 1.0));
                if sim.world.thundering {
                    let _ = tx.send(encode_play_game_event(8, 1.0));
                }
            }
            let _ = tx.send(encode_play_set_chunk_cache_center(
                center_chunk.x,
                center_chunk.z,
            ));

            let mut loaded_chunks = HashSet::new();
            let _ = tx.send(encode_play_chunk_batch_start());
            let mut sent_chunks = 0;
            for dz in -radius..=radius {
                for dx in -radius..=radius {
                    let cp = ChunkPos::new(center_chunk.x + dx, center_chunk.z + dz);
                    let chunk = sim.world.get_or_load_chunk_in(dimension, cp);
                    let _ = tx.send(encode_play_level_chunk_with_light(chunk));
                    loaded_chunks.insert(cp);
                    sent_chunks += 1;
                }
            }
            let _ = tx.send(encode_play_chunk_batch_finished(sent_chunks));

            let _ = tx.send(encode_play_player_position(
                1,
                spawn_vec,
                Vec3::ZERO,
                yaw,
                pitch,
                0,
            ));

            let mut existing_profiles: Vec<(GameProfile, i32, i32, Vec3, f32, f32)> = Vec::new();
            for (_, other) in sim.players.iter() {
                existing_profiles.push((
                    other.profile.clone(),
                    other.entity_id,
                    other.gamemode,
                    other.pos,
                    other.yaw,
                    other.pitch,
                ));
            }

            let new_info_pkt = encode_play_player_info_add(&[(&profile, gamemode, 0)]);
            let spawn_new_entity_pkt = encode_play_add_entity(
                entity_id,
                profile.uuid,
                ENTITY_TYPE_PLAYER_ID,
                spawn_vec,
                Vec3::ZERO,
                0.0,
                0.0,
                0.0,
                0,
            );
            for (_, other) in sim.players.iter() {
                let _ = other.tx.send(new_info_pkt.clone());
                let _ = other.tx.send(spawn_new_entity_pkt.clone());
            }

            let mut all_refs: Vec<(&GameProfile, i32, i32)> = existing_profiles
                .iter()
                .map(|(p, _, gm, _, _, _)| (p, *gm, 0))
                .collect();
            all_refs.push((&profile, gamemode, 0));
            let _ = tx.send(encode_play_player_info_add(&all_refs));

            for (other_profile, other_eid, _, other_pos, other_yaw, other_pitch) in
                &existing_profiles
            {
                let _ = tx.send(encode_play_add_entity(
                    *other_eid,
                    other_profile.uuid,
                    ENTITY_TYPE_PLAYER_ID,
                    *other_pos,
                    Vec3::ZERO,
                    *other_pitch,
                    *other_yaw,
                    *other_yaw,
                    0,
                ));
            }

            // Spawn existing world entities (items, mobs, falling blocks) for the joining player
            for (_, ent) in sim.entities.iter() {
                let _ = tx.send(encode_play_add_entity(
                    ent.entity_id,
                    ent.uuid,
                    ent.entity_type_id,
                    ent.pos,
                    ent.vel,
                    ent.pitch,
                    ent.yaw,
                    ent.yaw,
                    ent.add_entity_data_field(),
                ));
                if let EntityKind::Item { ref stack, .. } = ent.kind {
                    let _ = tx.send(encode_play_set_entity_data(
                        ent.entity_id,
                        &[EntityDataValue::ItemStack(8, stack.clone())],
                    ));
                }
            }

            let join_msg = format!("{} joined the game", profile.name);
            let _ = tx.send(encode_play_system_chat(&join_msg, false).unwrap_or_default());
            for (_, other) in sim.players.iter() {
                let _ = other
                    .tx
                    .send(encode_play_system_chat(&join_msg, false).unwrap_or_default());
            }

            let name = profile.name.clone();
            let advancements = PlayerAdvancements::load(sim.world.world_dir(), profile.uuid);
            let handle = sim.players.insert(PlayerEntity {
                entity_id,
                profile,
                gamemode,
                dimension,
                pos: spawn_vec,
                yaw,
                pitch,
                on_ground: true,
                health,
                food,
                saturation,
                food_exhaustion,
                food_tick_timer: 0,
                xp_level,
                xp_progress,
                xp_total,
                portal_cooldown: 0,
                effects: Vec::new(),
                advancements,
                spawnpoint: None,
                fall_distance: 0.0,
                dead: false,
                last_death: None,
                center_chunk,
                loaded_chunks,
                view_distance: radius,
                inventory,
                next_teleport_id: 2,
                next_container_id: 1,
                open_menu: None,
                tx,
            });
            ctx.online_count
                .store(sim.players.len() as i32, Ordering::Relaxed);
            info!("Player {name} (entity #{entity_id}) entered Play");
            let _ = reply.send(handle);
        }

        TickEvent::Packet { handle, packet } => {
            let now = sim.world.game_time as u64;
            let mut moved: Option<(i32, Vec3, f32, f32, bool)> = None;
            let mut chat_broadcast: Option<String> = None;
            let mut block_changes: Vec<(BlockPos, BlockStateId)> = Vec::new();
            let mut drop_spawns: Vec<(Vec3, Vec3, ItemStack, u16)> = Vec::new();
            let mut time_broadcast: Option<i64> = None;
            let mut pending_command: Option<(String, Vec3)> = None;
            let mut pending_attack: Option<i32> = None;
            let mut pending_fall_damage: f32 = 0.0;
            let mut pending_open_menu: Option<BlockPos> = None;
            let mut pending_container_click: Option<(i32, i16, i8, ContainerClickType)> = None;
            let mut pending_container_close: Option<i32> = None;
            let mut pending_respawn = false;
            let mut pending_effects: Vec<(&'static str, i32, i32)> = Vec::new();
            let mut pending_projectiles: Vec<(&'static str, i32, Vec3, f32, f32, f64, f32)> =
                Vec::new();
            let mut pending_level_events: Vec<(i32, BlockPos, i32)> = Vec::new();
            let mut pending_sign_update: Option<(BlockPos, [String; 4], bool)> = None;
            let mut pending_explosion: Option<(Vec3, f32, bool)> = None;
            let mut pending_equip_sync: Option<(i32, EquipmentSlot, ItemStack)> = None;

            if let Some(p) = sim.players.get_mut(handle) {
                match packet {
                    PlayServerbound::MovePlayerPos {
                        x,
                        y,
                        z,
                        on_ground,
                        ..
                    } => {
                        let prev_y = p.pos.y;
                        p.pos = Vec3::new(x, y, z);
                        p.on_ground = on_ground;
                        if p.gamemode == 0 && !p.dead {
                            if y < prev_y {
                                p.fall_distance += prev_y - y;
                            }
                            if on_ground {
                                if p.fall_distance > 3.0 {
                                    pending_fall_damage = (p.fall_distance - 3.0).floor() as f32;
                                }
                                p.fall_distance = 0.0;
                            } else if y > prev_y {
                                p.fall_distance = 0.0;
                            }
                        } else {
                            p.fall_distance = 0.0;
                        }
                        moved = Some((p.entity_id, p.pos, p.yaw, p.pitch, p.on_ground));
                        let feet = BlockPos::new(x.floor() as i32, y.floor() as i32, z.floor() as i32);
                        block_changes.extend(trigger_pressure_plate(
                            &mut sim.world,
                            &mut sim.scheduler,
                            now,
                            feet,
                        ));
                    }
                    PlayServerbound::MovePlayerPosRot {
                        x,
                        y,
                        z,
                        yaw,
                        pitch,
                        on_ground,
                        ..
                    } => {
                        let prev_y = p.pos.y;
                        p.pos = Vec3::new(x, y, z);
                        p.yaw = yaw;
                        p.pitch = pitch;
                        p.on_ground = on_ground;
                        if p.gamemode == 0 && !p.dead {
                            if y < prev_y {
                                p.fall_distance += prev_y - y;
                            }
                            if on_ground {
                                if p.fall_distance > 3.0 {
                                    pending_fall_damage = (p.fall_distance - 3.0).floor() as f32;
                                }
                                p.fall_distance = 0.0;
                            } else if y > prev_y {
                                p.fall_distance = 0.0;
                            }
                        } else {
                            p.fall_distance = 0.0;
                        }
                        moved = Some((p.entity_id, p.pos, p.yaw, p.pitch, p.on_ground));
                        let feet = BlockPos::new(x.floor() as i32, y.floor() as i32, z.floor() as i32);
                        block_changes.extend(trigger_pressure_plate(
                            &mut sim.world,
                            &mut sim.scheduler,
                            now,
                            feet,
                        ));
                    }
                    PlayServerbound::MovePlayerRot {
                        yaw,
                        pitch,
                        on_ground,
                        ..
                    } => {
                        p.yaw = yaw;
                        p.pitch = pitch;
                        p.on_ground = on_ground;
                        moved = Some((p.entity_id, p.pos, p.yaw, p.pitch, p.on_ground));
                    }
                    PlayServerbound::MovePlayerStatusOnly { on_ground, .. } => {
                        p.on_ground = on_ground;
                    }
                    PlayServerbound::Chat { message } => {
                        chat_broadcast = Some(format!("<{}> {message}", p.profile.name));
                    }
                    PlayServerbound::ChatCommand { command } => {
                        pending_command = Some((command, p.pos));
                    }
                    PlayServerbound::Attack { entity_id } => {
                        pending_attack = Some(entity_id);
                    }
                    PlayServerbound::ContainerClick {
                        container_id,
                        slot_num,
                        button_num,
                        click_type,
                        ..
                    } => {
                        pending_container_click =
                            Some((container_id, slot_num, button_num, click_type));
                    }
                    PlayServerbound::ContainerClose { container_id } => {
                        pending_container_close = Some(container_id);
                    }
                    PlayServerbound::ClientCommand {
                        action: ClientCommandAction::PerformRespawn,
                    } => {
                        pending_respawn = true;
                    }
                    PlayServerbound::SignUpdate {
                        pos,
                        lines,
                        is_front,
                    } => {
                        pending_sign_update = Some((pos, lines, is_front));
                    }
                    PlayServerbound::SetCarriedItem { slot } => {
                        p.inventory.set_selected_hotbar(slot);
                        pending_equip_sync = Some((
                            p.entity_id,
                            EquipmentSlot::MainHand,
                            p.inventory.held_item().clone(),
                        ));
                    }
                    PlayServerbound::SetCreativeModeSlot { slot, stack } => {
                        if p.gamemode == 1 && slot >= 0 {
                            if p.inventory.set_slot(slot as usize, stack.clone()) {
                                let sid = p.inventory.next_state_id();
                                let _ = p.tx.send(encode_play_container_set_slot(
                                    0,
                                    sid,
                                    slot,
                                    &p.inventory.slots[slot as usize],
                                ));
                            }
                        }
                    }
                    PlayServerbound::PlayerAction {
                        action,
                        pos,
                        sequence,
                        ..
                    } => {
                        match action {
                            PlayerActionType::StartDestroyBlock if p.gamemode == 1 => {
                                if let Some((old, _)) =
                                    sim.world.set_block_and_relight(pos, BlockStateId::AIR)
                                {
                                    if old != BlockStateId::AIR {
                                        pending_level_events.push((2001, pos, old.0 as i32));
                                        block_changes.push((pos, BlockStateId::AIR));
                                        block_changes.extend(propagate_neighbor_updates(
                                            &mut sim.world,
                                            &mut sim.scheduler,
                                            now,
                                            &[pos],
                                        ));
                                    }
                                }
                            }
                            PlayerActionType::StopDestroyBlock if p.gamemode == 0 => {
                                let cur = sim.world.get_block(pos);
                                if !cur.is_air() && cur.props().destroy_speed >= 0.0 {
                                    let removed_be = sim.world.take_block_entity(pos);
                                    if let Some((old, _)) =
                                        sim.world.set_block_and_relight(pos, BlockStateId::AIR)
                                    {
                                        pending_level_events.push((2001, pos, old.0 as i32));
                                        block_changes.push((pos, BlockStateId::AIR));
                                        block_changes.extend(propagate_neighbor_updates(
                                            &mut sim.world,
                                            &mut sim.scheduler,
                                            now,
                                            &[pos],
                                        ));
                                        let drop_pos = Vec3::new(
                                            pos.x as f64 + 0.5,
                                            pos.y as f64 + 0.5,
                                            pos.z as f64 + 0.5,
                                        );
                                        if let Some(loot) = block_loot(old.block_name()) {
                                            let cnt = loot.min_count.max(1);
                                            drop_spawns.push((
                                                drop_pos,
                                                Vec3::new(0.0, 0.2, 0.0),
                                                ItemStack::new(loot.item_id, cnt),
                                                10,
                                            ));
                                        } else if old.props().item_id > 0 {
                                            drop_spawns.push((
                                                drop_pos,
                                                Vec3::new(0.0, 0.2, 0.0),
                                                ItemStack::new(old.props().item_id, 1),
                                                10,
                                            ));
                                        }
                                        if let Some(be) = removed_be {
                                            if let Some(items) = be.items_slice() {
                                                for st in items {
                                                    if !st.is_empty() {
                                                        drop_spawns.push((
                                                            drop_pos,
                                                            Vec3::new(0.0, 0.2, 0.0),
                                                            st.clone(),
                                                            10,
                                                        ));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            PlayerActionType::DropItem | PlayerActionType::DropAllItems => {
                                let drop_all = action == PlayerActionType::DropAllItems;
                                let held_idx = p.inventory.held_slot_index();
                                let cur = p.inventory.slots[held_idx].clone();
                                if !cur.is_empty() {
                                    let drop_cnt = if drop_all { cur.count } else { 1 };
                                    let mut dropped = cur.clone();
                                    dropped.count = drop_cnt;
                                    if cur.count <= drop_cnt {
                                        p.inventory.slots[held_idx] = ItemStack::EMPTY;
                                    } else {
                                        p.inventory.slots[held_idx].count -= drop_cnt;
                                    }
                                    let sid = p.inventory.next_state_id();
                                    let _ = p.tx.send(encode_play_container_set_slot(
                                        0,
                                        sid,
                                        held_idx as i16,
                                        &p.inventory.slots[held_idx],
                                    ));
                                    let yaw_rad = (p.yaw as f64).to_radians();
                                    let vel = Vec3::new(-yaw_rad.sin() * 0.3, 0.2, yaw_rad.cos() * 0.3);
                                    drop_spawns.push((
                                        Vec3::new(p.pos.x, p.pos.y + 1.3, p.pos.z),
                                        vel,
                                        dropped,
                                        40,
                                    ));
                                }
                            }
                            PlayerActionType::SwapItemWithOffhand => {
                                let held_idx = p.inventory.held_slot_index();
                                p.inventory
                                    .slots
                                    .swap(held_idx, PlayerInventory::OFFHAND_SLOT);
                                let sid1 = p.inventory.next_state_id();
                                let _ = p.tx.send(encode_play_container_set_slot(
                                    0,
                                    sid1,
                                    held_idx as i16,
                                    &p.inventory.slots[held_idx],
                                ));
                                let sid2 = p.inventory.next_state_id();
                                let _ = p.tx.send(encode_play_container_set_slot(
                                    0,
                                    sid2,
                                    PlayerInventory::OFFHAND_SLOT as i16,
                                    &p.inventory.slots[PlayerInventory::OFFHAND_SLOT],
                                ));
                            }
                            _ => {}
                        }
                        if sequence > 0 {
                            let _ = p.tx.send(encode_play_block_changed_ack(sequence));
                        }
                    }
                    PlayServerbound::UseItemOn {
                        hand,
                        hit,
                        sequence,
                    } => {
                        let clicked_kind = block_behavior(sim.world.get_block(hit.pos).0).kind;
                        let opens_screen = matches!(
                            clicked_kind,
                            KIND_CHEST
                                | KIND_BARREL
                                | KIND_FURNACE
                                | KIND_BLAST_FURNACE
                                | KIND_SMOKER
                                | KIND_HOPPER
                                | KIND_CRAFTING_TABLE
                        );
                        if opens_screen {
                            pending_open_menu = Some(hit.pos);
                        } else if clicked_kind == KIND_SIGN {
                            let _ = p.tx.send(encode_play_open_sign_editor(hit.pos, true));
                        } else if let Some(interacted) =
                            interact_block(&mut sim.world, &mut sim.scheduler, now, hit.pos)
                        {
                            block_changes.extend(interacted);
                        } else {
                            let offhand = hand == InteractionHand::OffHand;
                            let item = if offhand {
                                p.inventory.offhand_item()
                            } else {
                                p.inventory.held_item()
                            };
                            let held_name = item_by_id(item.item_id).map(|i| i.name).unwrap_or("");
                            if held_name == "minecraft:flint_and_steel" {
                                let clicked_state = sim.world.get_block(hit.pos);
                                if is_tnt(clicked_state.0) {
                                    let _ = sim.world.set_block_and_relight(hit.pos, BlockStateId::AIR);
                                    block_changes.push((hit.pos, BlockStateId::AIR));
                                    pending_explosion = Some((
                                        Vec3::new(
                                            hit.pos.x as f64 + 0.5,
                                            hit.pos.y as f64 + 0.5,
                                            hit.pos.z as f64 + 0.5,
                                        ),
                                        4.0,
                                        false,
                                    ));
                                } else {
                                    let fire_pos = hit.direction.offset_pos(hit.pos);
                                    if let Some(portal_blocks) =
                                        try_ignite_nether_portal(&mut sim.world, fire_pos)
                                    {
                                        block_changes.extend(portal_blocks);
                                    } else if sim.world.get_block(fire_pos).is_air() {
                                        let fire_st = BlockStateId(FIRE_STATE_ID);
                                        if sim.world.set_block_and_relight(fire_pos, fire_st).is_some() {
                                            block_changes.push((fire_pos, fire_st));
                                        }
                                    }
                                }
                            } else if let Some(default_place) = item.placed_block_state() {
                                let clicked_state = sim.world.get_block(hit.pos);
                                let outcome = resolve_placement(
                                    default_place.0,
                                    hit.direction as u8,
                                    hit.click_y,
                                    p.yaw,
                                    p.pitch,
                                    clicked_state.0,
                                );
                                let place_state = BlockStateId(outcome.primary_state);
                                let target_pos = if outcome.replace_clicked
                                    || clicked_state.props().can_be_replaced()
                                {
                                    hit.pos
                                } else {
                                    hit.direction.offset_pos(hit.pos)
                                };
                                let existing = sim.world.get_block(target_pos);
                                let sec_ok = outcome.secondary.map_or(true, |(dx, dy, dz, _)| {
                                    sim.world
                                        .get_block(target_pos.offset(dx, dy, dz))
                                        .props()
                                        .can_be_replaced()
                                });
                                if (outcome.replace_clicked || existing.props().can_be_replaced())
                                    && sec_ok
                                {
                                    if sim.world.set_block_and_relight(target_pos, place_state).is_some()
                                    {
                                        let mut seeds = vec![target_pos];
                                        block_changes.push((target_pos, place_state));
                                        if block_behavior(place_state.0).kind == KIND_SIGN {
                                            let _ = p
                                                .tx
                                                .send(encode_play_open_sign_editor(target_pos, true));
                                        }
                                        if let Some((dx, dy, dz, sec_id)) = outcome.secondary {
                                            let sec_pos = target_pos.offset(dx, dy, dz);
                                            let sec_st = BlockStateId(sec_id);
                                            if sim.world.set_block_and_relight(sec_pos, sec_st).is_some() {
                                                block_changes.push((sec_pos, sec_st));
                                                seeds.push(sec_pos);
                                            }
                                        }
                                        block_changes.extend(propagate_neighbor_updates(
                                            &mut sim.world,
                                            &mut sim.scheduler,
                                            now,
                                            &seeds,
                                        ));
                                        if p.gamemode == 0 {
                                            if let Some((slot_idx, updated)) =
                                                p.inventory.consume_one(offhand)
                                            {
                                                let sid = p.inventory.next_state_id();
                                                let _ = p.tx.send(encode_play_container_set_slot(
                                                    0, sid, slot_idx, &updated,
                                                ));
                                            }
                                        }
                                    }
                                } else {
                                    let _ = p
                                        .tx
                                        .send(encode_play_block_update(target_pos, existing.0));
                                }
                            }
                        }
                        if sequence > 0 {
                            let _ = p.tx.send(encode_play_block_changed_ack(sequence));
                        }
                    }
                    PlayServerbound::UseItem {
                        hand,
                        sequence,
                        yaw,
                        pitch,
                    } => {
                        let offhand = hand == InteractionHand::OffHand;
                        let slot_idx = if offhand {
                            PlayerInventory::OFFHAND_SLOT
                        } else {
                            p.inventory.held_slot_index()
                        };
                        let held = p.inventory.slots[slot_idx].clone();
                        if !held.is_empty() {
                            let item_name = item_by_id(held.item_id).map(|i| i.name).unwrap_or("");
                            let cs = item_combat_stats(held.item_id);
                            let throw_spec = match item_name {
                                "minecraft:snowball" => Some(("minecraft:snowball", 1.5, 0.0, true)),
                                "minecraft:egg" => Some(("minecraft:egg", 1.5, 0.0, true)),
                                "minecraft:ender_pearl" => Some(("minecraft:ender_pearl", 1.5, 0.0, true)),
                                "minecraft:bow" => Some(("minecraft:arrow", 2.2, 6.0, false)),
                                _ => None,
                            };
                            if let Some((proj_kind, speed, dmg, consume_self)) = throw_spec {
                                let eye = Vec3::new(p.pos.x, p.pos.y + 1.52, p.pos.z);
                                pending_projectiles.push((
                                    proj_kind,
                                    p.entity_id,
                                    eye,
                                    yaw,
                                    pitch,
                                    speed,
                                    dmg,
                                ));
                                if consume_self && p.gamemode == 0 {
                                    if let Some((si, updated)) = p.inventory.consume_one(offhand) {
                                        let sid = p.inventory.next_state_id();
                                        let _ = p.tx.send(encode_play_container_set_slot(
                                            0, sid, si, &updated,
                                        ));
                                    }
                                }
                            } else if (5..=8).contains(&cs.armor_slot) {
                                // Quick-equip armor into vanilla armor slot 5..=8!
                                let aslot = cs.armor_slot as usize;
                                p.inventory.slots.swap(slot_idx, aslot);
                                let s1 = p.inventory.next_state_id();
                                let _ = p.tx.send(encode_play_container_set_slot(
                                    0,
                                    s1,
                                    slot_idx as i16,
                                    &p.inventory.slots[slot_idx],
                                ));
                                let s2 = p.inventory.next_state_id();
                                let _ = p.tx.send(encode_play_container_set_slot(
                                    0,
                                    s2,
                                    aslot as i16,
                                    &p.inventory.slots[aslot],
                                ));
                            } else if let Some(fi) = food_info(held.item_id) {
                                let always_edible = matches!(
                                    item_name,
                                    "minecraft:golden_apple" | "minecraft:enchanted_golden_apple"
                                );
                                if p.food < 20 || p.gamemode == 1 || always_edible {
                                    p.food = (p.food + fi.nutrition as i32).min(20);
                                    p.saturation =
                                        (p.saturation + fi.saturation).min(p.food as f32);
                                    let _ = p.tx.send(encode_play_set_health(
                                        p.health,
                                        p.food,
                                        p.saturation,
                                    ));
                                    if item_name == "minecraft:golden_apple" {
                                        pending_effects.push(("minecraft:regeneration", 100, 1));
                                        pending_effects.push(("minecraft:absorption", 2400, 0));
                                    } else if item_name == "minecraft:enchanted_golden_apple" {
                                        pending_effects.push(("minecraft:regeneration", 400, 1));
                                        pending_effects.push(("minecraft:resistance", 6000, 0));
                                        pending_effects.push(("minecraft:fire_resistance", 6000, 0));
                                        pending_effects.push(("minecraft:absorption", 2400, 3));
                                    }
                                    if p.gamemode == 0 {
                                        if let Some((si, updated)) =
                                            p.inventory.consume_one(offhand)
                                        {
                                            let sid = p.inventory.next_state_id();
                                            let _ = p.tx.send(encode_play_container_set_slot(
                                                0, sid, si, &updated,
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                        if sequence > 0 {
                            let _ = p.tx.send(encode_play_block_changed_ack(sequence));
                        }
                    }
                    _ => {}
                }

                stream_chunks_for_player(p, &mut sim.world);
            }

            if let Some(pos) = pending_open_menu {
                sim.open_block_menu(handle, pos);
            }

            if let Some((cid, slot_num, button_num, click_type)) = pending_container_click {
                sim.handle_container_click(handle, cid, slot_num, button_num, click_type);
            }

            if let Some(cid) = pending_container_close {
                sim.handle_container_close(handle, cid);
            }

            if pending_fall_damage > 0.0 {
                sim.damage_player(handle, pending_fall_damage, "fell from a high place");
            }

            if pending_respawn {
                sim.handle_respawn(handle);
            }

            if let Some((spos, lines, is_front)) = pending_sign_update {
                if let Some(be) = sim.world.ensure_block_entity(spos) {
                    if let BlockEntityKind::Sign {
                        ref mut front_lines,
                        ref mut back_lines,
                        is_waxed,
                    } = be.kind
                    {
                        if !is_waxed {
                            if is_front {
                                *front_lines = lines;
                            } else {
                                *back_lines = lines;
                            }
                            let nbt = be.save_to_nbt();
                            if let Ok(pkt) = encode_play_block_entity_data(
                                spos,
                                block_entity_types::SIGN,
                                &nbt,
                            ) {
                                for (_, other) in sim.players.iter() {
                                    let _ = other.tx.send(pkt.clone());
                                }
                            }
                        }
                    }
                }
            }

            for (eff_name, dur, amp) in pending_effects {
                let _ = sim.give_player_effect(handle, eff_name, dur, amp);
            }

            for (pkind, owner_eid, origin, pyaw, ppitch, speed, dmg) in pending_projectiles {
                let eid = sim.alloc_entity_id();
                if let Some(ent) = WorldEntity::spawn_projectile(
                    eid, pkind, owner_eid, origin, pyaw, ppitch, speed, dmg,
                ) {
                    sim.spawn_world_entity(ent);
                }
            }

            for (ev_type, ev_pos, ev_data) in pending_level_events {
                let lpkt = encode_play_level_event(ev_type, ev_pos, ev_data, false);
                for (_, other) in sim.players.iter() {
                    let _ = other.tx.send(lpkt.clone());
                }
            }

            if let Some((center, power, fire)) = pending_explosion {
                sim.trigger_explosion(center, power, fire);
            }

            if let Some((eid, eslot, stack)) = pending_equip_sync {
                if let Ok(epkt) = encode_play_set_equipment(eid, &[(eslot, stack)]) {
                    for (other_h, other) in sim.players.iter() {
                        if other_h != handle {
                            let _ = other.tx.send(epkt.clone());
                        }
                    }
                }
            }

            for (pos, vel, stack, delay) in drop_spawns {
                let eid = sim.alloc_entity_id();
                let ent = WorldEntity::spawn_item(eid, pos, vel, stack, delay);
                sim.spawn_world_entity(ent);
            }

            if let Some(target_eid) = pending_attack {
                let (attacker_yaw, attack_dmg) = sim
                    .players
                    .get(handle)
                    .map(|p| {
                        let held_id = p.inventory.held_item().item_id;
                        let bonus = if held_id > 0 {
                            item_combat_stats(held_id).attack_damage
                        } else {
                            0.0
                        };
                        ((p.yaw as f64).to_radians(), (1.0 + bonus).max(1.0))
                    })
                    .unwrap_or((0.0, 1.0));
                let target_h = sim
                    .entities
                    .iter()
                    .find_map(|(h, e)| if e.entity_id == target_eid { Some(h) } else { None });
                if let Some(th) = target_h {
                    let mut died = false;
                    let mut mob_death_drop: Option<(Vec3, String)> = None;
                    let mut knock_pkt = None;
                    let mut health_pkt = None;
                    if let Some(ent) = sim.entities.get_mut(th) {
                        if let EntityKind::Mob {
                            ref type_name,
                            ref mut health,
                            ref mut hurt_ticks,
                            ..
                        } = ent.kind
                        {
                            if *hurt_ticks == 0 {
                                *health = (*health - attack_dmg).max(0.0);
                                *hurt_ticks = 10;
                                ent.vel.x += -attacker_yaw.sin() * 0.4;
                                ent.vel.y = 0.35;
                                ent.vel.z += attacker_yaw.cos() * 0.4;
                                knock_pkt = Some(encode_play_set_entity_motion(ent.entity_id, ent.vel));
                                health_pkt = Some(encode_play_set_entity_data(
                                    ent.entity_id,
                                    &[EntityDataValue::Float(9, *health)],
                                ));
                                died = *health <= 0.0;
                                if died {
                                    mob_death_drop = Some((ent.pos, type_name.clone()));
                                }
                            }
                        }
                    }
                    for (_, p) in sim.players.iter() {
                        if let Some(ref kp) = knock_pkt {
                            let _ = p.tx.send(kp.clone());
                        }
                        if let Some(ref hp) = health_pkt {
                            let _ = p.tx.send(hp.clone());
                        }
                    }
                    if died {
                        sim.entities.remove(th);
                        let rm = encode_play_remove_entities(&[target_eid]);
                        for (_, p) in sim.players.iter() {
                            let _ = p.tx.send(rm.clone());
                        }
                        if let Some((dpos, mtype)) = mob_death_drop {
                            for drop in entity_loot(&mtype) {
                                let cnt = drop.min_count.max(1);
                                let ieid = sim.alloc_entity_id();
                                sim.spawn_world_entity(WorldEntity::spawn_item(
                                    ieid,
                                    Vec3::new(dpos.x, dpos.y + 0.5, dpos.z),
                                    Vec3::new(0.0, 0.2, 0.0),
                                    ItemStack::new(drop.item_id, cnt),
                                    10,
                                ));
                            }
                            // Grant vanilla mob-kill experience (+5 XP)
                            if let Some(p) = sim.players.get_mut(handle) {
                                add_experience_points(
                                    &mut p.xp_level,
                                    &mut p.xp_progress,
                                    &mut p.xp_total,
                                    5,
                                );
                                let _ = p.tx.send(encode_play_set_experience(
                                    p.xp_progress,
                                    p.xp_level,
                                    p.xp_total,
                                ));
                            }
                        }
                    }
                } else {
                    // Check PvP player target
                    let victim_h = sim.players.iter().find_map(|(ph, pl)| {
                        if pl.entity_id == target_eid && ph != handle {
                            Some(ph)
                        } else {
                            None
                        }
                    });
                    if let Some(vh) = victim_h {
                        if sim.world.gamerules.pvp() {
                            let attacker_name = sim
                                .players
                                .get(handle)
                                .map(|p| p.profile.name.clone())
                                .unwrap_or_else(|| "a player".into());
                            sim.damage_player(
                                vh,
                                attack_dmg,
                                &format!("was slain by {attacker_name}"),
                            );
                        }
                    }
                }
            }

            if let Some((cmd_raw, sender_pos)) = pending_command {
                if let Some(extra_action) = parse_extra_command(&cmd_raw, sender_pos) {
                    let reply_msg = sim.execute_extra_command(handle, extra_action);
                    if let Some(p) = sim.players.get(handle) {
                        if let Ok(pkt) = encode_play_system_chat(&reply_msg, false) {
                            let _ = p.tx.send(pkt);
                        }
                    }
                } else {
                let action = commands::parse_command(&cmd_raw, sender_pos);
                match action {
                    CommandAction::Reply(msg) => {
                        if let Some(p) = sim.players.get(handle) {
                            if let Ok(pkt) = encode_play_system_chat(&msg, false) {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::SetGamemode(gm) => {
                        if let Some(p) = sim.players.get_mut(handle) {
                            p.gamemode = gm;
                            let _ = p.tx.send(encode_play_game_event(3, gm as f32));
                            let _ = p.tx.send(abilities_packet_for_gamemode(gm));
                            let label = match gm {
                                0 => "Survival",
                                1 => "Creative",
                                2 => "Adventure",
                                3 => "Spectator",
                                _ => "Unknown",
                            };
                            if let Ok(pkt) = encode_play_system_chat(
                                &format!("Set own game mode to {label} Mode"),
                                false,
                            ) {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::Teleport(dest) => {
                        if let Some(p) = sim.players.get_mut(handle) {
                            p.pos = dest;
                            let tp_id = p.next_teleport_id;
                            p.next_teleport_id += 1;
                            let _ = p.tx.send(encode_play_player_position(
                                tp_id,
                                dest,
                                Vec3::ZERO,
                                p.yaw,
                                p.pitch,
                                0,
                            ));
                            stream_chunks_for_player(p, &mut sim.world);
                            moved = Some((p.entity_id, p.pos, p.yaw, p.pitch, p.on_ground));
                            if let Ok(pkt) = encode_play_system_chat(
                                &format!("Teleported to {:.2}, {:.2}, {:.2}", dest.x, dest.y, dest.z),
                                false,
                            ) {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::SetBlock { pos, state } => {
                        if sim.world.set_block_and_relight(pos, state).is_some() {
                            block_changes.push((pos, state));
                            block_changes.extend(propagate_neighbor_updates(
                                &mut sim.world,
                                &mut sim.scheduler,
                                now,
                                &[pos],
                            ));
                            if let Some(p) = sim.players.get(handle) {
                                if let Ok(pkt) = encode_play_system_chat(
                                    &format!(
                                        "Changed the block at {}, {}, {} to {}",
                                        pos.x,
                                        pos.y,
                                        pos.z,
                                        state.block_name()
                                    ),
                                    false,
                                ) {
                                    let _ = p.tx.send(pkt);
                                }
                            }
                        }
                    }
                    CommandAction::Give {
                        item_id,
                        item_name,
                        count,
                    } => {
                        let mut adv_out = None;
                        if let Some(p) = sim.players.get_mut(handle) {
                            let changed = p.inventory.give_item(item_id, count);
                            for (slot_idx, st) in changed {
                                let sid = p.inventory.next_state_id();
                                let _ = p.tx.send(encode_play_container_set_slot(
                                    0, sid, slot_idx, &st,
                                ));
                            }
                            let awarded = p.advancements.check_item_obtained(item_name);
                            if !awarded.is_empty() {
                                adv_out = Some((p.profile.name.clone(), awarded));
                            }
                            if let Ok(pkt) = encode_play_system_chat(
                                &format!("Gave {count} [{item_name}] to {}", p.profile.name),
                                false,
                            ) {
                                let _ = p.tx.send(pkt);
                            }
                        }
                        if let Some((pname, awarded)) = adv_out {
                            sim.announce_advancements(&pname, awarded);
                        }
                    }
                    CommandAction::SetTime(ticks) => {
                        sim.world.day_time = ticks;
                        time_broadcast = Some(ticks);
                        if let Some(p) = sim.players.get(handle) {
                            if let Ok(pkt) =
                                encode_play_system_chat(&format!("Set the time to {ticks}"), false)
                            {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::QueryTime => {
                        if let Some(p) = sim.players.get(handle) {
                            if let Ok(pkt) = encode_play_system_chat(
                                &format!(
                                    "The time is {} (game_time={})",
                                    sim.world.day_time % 24000,
                                    sim.world.game_time
                                ),
                                false,
                            ) {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::SaveAll => {
                        sim.save_all_players();
                        let msg = match sim.world.save_dirty_chunks() {
                            Ok(n) => format!(
                                "Saved the game ({n} dirty chunk(s), level.dat, and playerdata)"
                            ),
                            Err(e) => format!("Save failed: {e}"),
                        };
                        if let Some(p) = sim.players.get(handle) {
                            if let Ok(pkt) = encode_play_system_chat(&msg, false) {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::SetWeather {
                        raining,
                        thundering,
                        duration_ticks,
                    } => {
                        sim.apply_weather(raining, thundering, duration_ticks);
                        let label = if !raining {
                            "clear"
                        } else if thundering {
                            "rain & thunder"
                        } else {
                            "rain"
                        };
                        if let Some(p) = sim.players.get(handle) {
                            if let Ok(pkt) =
                                encode_play_system_chat(&format!("Set the weather to {label}"), false)
                            {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::Xp {
                        set_mode,
                        amount,
                        is_levels,
                    } => {
                        if let Some(p) = sim.players.get_mut(handle) {
                            if set_mode {
                                if is_levels {
                                    p.xp_level = amount.max(0);
                                } else {
                                    p.xp_level = 0;
                                    p.xp_progress = 0.0;
                                    p.xp_total = 0;
                                    if amount > 0 {
                                        add_experience_points(
                                            &mut p.xp_level,
                                            &mut p.xp_progress,
                                            &mut p.xp_total,
                                            amount,
                                        );
                                    }
                                }
                            } else if is_levels {
                                p.xp_level = p.xp_level.saturating_add(amount).max(0);
                                if p.xp_level == 0 && amount < 0 {
                                    p.xp_progress = 0.0;
                                    p.xp_total = 0;
                                }
                            } else {
                                add_experience_points(
                                    &mut p.xp_level,
                                    &mut p.xp_progress,
                                    &mut p.xp_total,
                                    amount,
                                );
                            }
                            let _ = p.tx.send(encode_play_set_experience(
                                p.xp_progress,
                                p.xp_level,
                                p.xp_total,
                            ));
                            if let Ok(pkt) = encode_play_system_chat(
                                &format!(
                                    "Experience updated: level {} ({} total XP)",
                                    p.xp_level, p.xp_total
                                ),
                                false,
                            ) {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::EffectGive {
                        effect,
                        seconds,
                        amplifier,
                    } => {
                        let msg = match sim.give_player_effect(handle, &effect, seconds * 20, amplifier) {
                            Ok(()) => format!("Applied effect {effect} ({seconds}s, amp {amplifier})"),
                            Err(e) => e,
                        };
                        if let Some(p) = sim.players.get(handle) {
                            if let Ok(pkt) = encode_play_system_chat(&msg, false) {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::EffectClear => {
                        let n = sim.clear_player_effects(handle);
                        if let Some(p) = sim.players.get(handle) {
                            if let Ok(pkt) = encode_play_system_chat(
                                &format!("Removed {n} active effect(s)"),
                                false,
                            ) {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::ListPlayers => {
                        let names: Vec<String> = sim
                            .players
                            .iter()
                            .map(|(_, pl)| pl.profile.name.clone())
                            .collect();
                        let msg = format!(
                            "There are {} of a max of {} players online: {}",
                            names.len(),
                            ctx.config.max_players,
                            names.join(", ")
                        );
                        if let Some(p) = sim.players.get(handle) {
                            if let Ok(pkt) = encode_play_system_chat(&msg, false) {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::Seed => {
                        if let Some(p) = sim.players.get(handle) {
                            if let Ok(pkt) = encode_play_system_chat(
                                &format!("Seed: [{}]", sim.world.seed),
                                false,
                            ) {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::Biome => {
                        if let Some(p) = sim.players.get(handle) {
                            let bx = p.pos.x.floor() as i32;
                            let by = p.pos.y.floor() as i32;
                            let bz = p.pos.z.floor() as i32;
                            let bname = sim.world.biome_name_in(p.dimension, bx, by, bz);
                            if let Ok(pkt) = encode_play_system_chat(
                                &format!("Biome at ({bx}, {by}, {bz}): {bname}"),
                                false,
                            ) {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::SwitchDimension(dim_name) => {
                        if let Some(target) = DimensionId::from_name(&dim_name) {
                            sim.switch_dimension(handle, target);
                            if let Some(p) = sim.players.get(handle) {
                                if let Ok(pkt) = encode_play_system_chat(
                                    &format!("Switched dimension to {}", target.name()),
                                    false,
                                ) {
                                    let _ = p.tx.send(pkt);
                                }
                            }
                        }
                    }
                    CommandAction::Op(_)
                    | CommandAction::Deop(_)
                    | CommandAction::Ban { .. }
                    | CommandAction::Pardon(_)
                    | CommandAction::Kick { .. }
                    | CommandAction::Whitelist { .. } => {
                        let (msg, _) = sim.execute_admin_or_console(&cmd_raw, ctx, stop_signal);
                        if let Some(p) = sim.players.get(handle) {
                            if let Ok(pkt) = encode_play_system_chat(&msg, false) {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::PlaceTemplate {
                        template_id,
                        pos,
                        rotation_deg,
                    } => {
                        let msg = match StructureTemplate::load_from_data_dir(
                            &ctx.config.data_dir,
                            &template_id,
                        ) {
                            Ok(tpl) => {
                                let rot = StructureRotation::from_degrees(rotation_deg);
                                let (placed, bes) = tpl.place(&mut sim.world, pos, rot);
                                let count = placed.len();
                                for be in bes {
                                    sim.world.insert_block_entity(be);
                                }
                                block_changes.extend(placed);
                                format!(
                                    "Placed template {template_id} ({count} blocks) at {}, {}, {}",
                                    pos.x, pos.y, pos.z
                                )
                            }
                            Err(e) => format!("Failed to load template {template_id}: {e}"),
                        };
                        if let Some(p) = sim.players.get(handle) {
                            if let Ok(pkt) = encode_play_system_chat(&msg, false) {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::Summon { entity_type, pos } => {
                        let eid = sim.alloc_entity_id();
                        let msg = if let Some(ent) = WorldEntity::spawn_mob(eid, &entity_type, pos) {
                            sim.spawn_world_entity(ent);
                            format!("Summoned new {entity_type} (entity #{eid})")
                        } else {
                            format!("Unknown entity type: {entity_type}")
                        };
                        if let Some(p) = sim.players.get(handle) {
                            if let Ok(pkt) = encode_play_system_chat(&msg, false) {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::KillEntities => {
                        let all_handles: Vec<(Handle<WorldEntity>, i32)> = sim
                            .entities
                            .iter()
                            .map(|(h, e)| (h, e.entity_id))
                            .collect();
                        let count = all_handles.len();
                        let mut ids = Vec::with_capacity(count);
                        for (h, eid) in all_handles {
                            sim.entities.remove(h);
                            ids.push(eid);
                        }
                        if !ids.is_empty() {
                            let rm = encode_play_remove_entities(&ids);
                            for (_, other) in sim.players.iter() {
                                let _ = other.tx.send(rm.clone());
                            }
                        }
                        if let Some(p) = sim.players.get(handle) {
                            if let Ok(pkt) =
                                encode_play_system_chat(&format!("Killed {count} entities"), false)
                            {
                                let _ = p.tx.send(pkt);
                            }
                        }
                    }
                    CommandAction::StopServer => {
                        sim.save_all_players();
                        let _ = sim.world.save_dirty_chunks();
                        if let Ok(pkt) = encode_play_system_chat("Stopping the server", false) {
                            for (_, other) in sim.players.iter() {
                                let _ = other.tx.send(pkt.clone());
                            }
                        }
                        if let Some(sig) = stop_signal.take() {
                            let _ = sig.send(());
                        }
                        return true;
                    }
                }
                }
            }

            sim.broadcast_block_changes(&block_changes);

            if let Some(ticks) = time_broadcast {
                let time_pkt = encode_play_set_time(
                    sim.world.game_time,
                    &[(sim.overworld_clock_id, ticks, 0.0, 1.0)],
                );
                for (_, other) in sim.players.iter() {
                    let _ = other.tx.send(time_pkt.clone());
                }
            }

            if let Some((eid, pos, yaw, pitch, on_ground)) = moved {
                let sync_pkt = encode_play_entity_position_sync(eid, pos, yaw, pitch, on_ground);
                for (other_h, other) in sim.players.iter() {
                    if other_h != handle {
                        let _ = other.tx.send(sync_pkt.clone());
                    }
                }
            }

            if let Some(msg) = chat_broadcast {
                info!("[Chat] {msg}");
                if let Ok(pkt) = encode_play_system_chat(&msg, false) {
                    for (_, other) in sim.players.iter() {
                        let _ = other.tx.send(pkt.clone());
                    }
                }
            }
        }

        TickEvent::ExtraPacket { handle, packet } => {
            if let PlayExtraServerbound::Swing { hand } = packet {
                if let Some(eid) = sim.players.get(handle).map(|p| p.entity_id) {
                    let spkt = encode_play_swing_animation(eid, hand, SwingAnimationType::Whack, 6);
                    for (other_h, other) in sim.players.iter() {
                        if other_h != handle {
                            let _ = other.tx.send(spkt.clone());
                        }
                    }
                }
            }
        }

        TickEvent::Disconnect { handle } => {
            if let Some(removed) = sim.players.remove(handle) {
                if let Err(e) = save_player_data(
                    sim.world.world_dir(),
                    removed.profile.uuid,
                    &removed.to_saved_data(),
                ) {
                    warn!("Failed saving playerdata for {}: {e}", removed.profile.name);
                }
                let _ = removed.advancements.save(sim.world.world_dir());
                ctx.online_count
                    .store(sim.players.len() as i32, Ordering::Relaxed);
                let rm_ent = encode_play_remove_entities(&[removed.entity_id]);
                let rm_info = encode_play_player_info_remove(&[removed.profile.uuid]);
                let left_msg = encode_play_system_chat(
                    &format!("{} left the game", removed.profile.name),
                    false,
                )
                .unwrap_or_default();
                for (_, other) in sim.players.iter() {
                    let _ = other.tx.send(rm_ent.clone());
                    let _ = other.tx.send(rm_info.clone());
                    let _ = other.tx.send(left_msg.clone());
                }
                info!("Player {} disconnected", removed.profile.name);
            }
        }
    }
    false
}

async fn handle_connection(
    stream: TcpStream,
    peer: SocketAddr,
    ctx: Arc<SharedContext>,
) -> Result<()> {
    let mut conn = FramedConnection::new(stream);

    // 1. Handshake
    let raw_hs = conn.read_raw_packet().await?;
    let hs = HandshakeIntention::decode(&raw_hs)?;

    match hs.intent {
        ClientIntent::Status => {
            handle_status(&mut conn, &ctx).await?;
            return Ok(());
        }
        ClientIntent::Login | ClientIntent::Transfer => {
            if hs.protocol_version != PROTOCOL_VERSION {
                let msg = serde_json::json!({
                    "text": format!("Incompatible client! Expected Minecraft {VERSION_NAME} (protocol {PROTOCOL_VERSION}), got {}", hs.protocol_version)
                })
                .to_string();
                conn.write_raw_packet(&encode_login_disconnect(&msg)).await?;
                return Ok(());
            }
        }
    }

    // 2. Login
    let raw_hello = conn.read_raw_packet().await?;
    let LoginServerbound::Hello {
        name,
        profile_id: _,
    } = LoginServerbound::decode(&raw_hello)?
    else {
        return Ok(());
    };

    let profile = if ctx.config.online_mode {
        let mut challenge = [0u8; 4];
        rand::thread_rng().fill_bytes(&mut challenge);
        conn.write_raw_packet(&encode_login_hello(
            "",
            ctx.keypair.public_key_der(),
            &challenge,
            true,
        ))
        .await?;

        let raw_key = conn.read_raw_packet().await?;
        let LoginServerbound::Key {
            encrypted_secret,
            encrypted_challenge,
        } = LoginServerbound::decode(&raw_key)?
        else {
            return Ok(());
        };
        let dec_challenge = ctx.keypair.decrypt_pkcs1v15(&encrypted_challenge)?;
        if dec_challenge != challenge {
            conn.write_raw_packet(&encode_login_disconnect(
                r#"{"text":"Invalid encryption challenge"}"#,
            ))
            .await?;
            return Ok(());
        }
        let secret_vec = ctx.keypair.decrypt_pkcs1v15(&encrypted_secret)?;
        let secret: [u8; 16] = secret_vec
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid AES secret length"))?;
        conn.enable_encryption(secret);

        verify_mojang_session(&name, "", &secret, ctx.keypair.public_key_der()).await?
    } else {
        GameProfile {
            uuid: offline_player_uuid(&name),
            name,
            properties: vec![],
        }
    };

    // Enforce banned-players.json & whitelist.json during Login!
    let (chk_tx, chk_rx) = oneshot::channel();
    let _ = ctx.tick_tx.send(TickEvent::CheckLogin {
        name: profile.name.clone(),
        reply: chk_tx,
    });
    if let Ok(Some(deny_reason)) = chk_rx.await {
        let json = serde_json::json!({ "text": deny_reason }).to_string();
        conn.write_raw_packet(&encode_login_disconnect(&json))
            .await?;
        return Ok(());
    }

    if ctx.config.compression_threshold >= 0 {
        let thr = ctx.config.compression_threshold;
        conn.write_raw_packet(&encode_login_compression(thr)).await?;
        conn.set_compression_threshold(Some(thr as usize));
    }

    let session_id = Uuid::new_v4();
    conn.write_raw_packet(&encode_login_finished(&profile, session_id))
        .await?;

    let raw_ack = conn.read_raw_packet().await?;
    if !matches!(
        LoginServerbound::decode(&raw_ack)?,
        LoginServerbound::LoginAcknowledged
    ) {
        return Ok(());
    }

    // 3. Configuration Phase
    conn.write_raw_packet(&encode_config_custom_payload_brand("SiliconMC-Rust"))
        .await?;
    let vanilla_pack = KnownPack {
        namespace: "minecraft".into(),
        id: "core".into(),
        version: VERSION_NAME.into(),
    };
    conn.write_raw_packet(&encode_config_select_known_packs(&[vanilla_pack.clone()]))
        .await?;

    let mut client_view_distance = ctx.config.view_distance;
    let mut client_has_vanilla_pack = false;

    loop {
        let raw = conn.read_raw_packet().await?;
        match ConfigServerbound::decode(&raw)? {
            ConfigServerbound::ClientInformation { view_distance, .. } => {
                client_view_distance = view_distance as i32;
            }
            ConfigServerbound::SelectKnownPacks { packs } => {
                client_has_vanilla_pack = packs.iter().any(|p| *p == vanilla_pack);
                break;
            }
            ConfigServerbound::KeepAlive { .. }
            | ConfigServerbound::Pong { .. }
            | ConfigServerbound::CustomPayload { .. }
            | ConfigServerbound::Other(_) => {}
            ConfigServerbound::FinishConfiguration => break,
        }
    }

    for reg in ctx.datapacks.registries.values() {
        let entries: Vec<PackedRegistryEntry> = reg
            .entries
            .iter()
            .map(|e| PackedRegistryEntry {
                id: e.id.clone(),
                data: if client_has_vanilla_pack {
                    None
                } else {
                    e.data.clone()
                },
            })
            .collect();
        let pkt = encode_config_registry_data(&reg.registry_id, &entries)?;
        conn.write_raw_packet(&pkt).await?;
    }

    if let Some(ref tags_payload) = ctx.network_tags_payload {
        conn.write_raw_packet(&encode_config_update_tags_raw(tags_payload))
            .await?;
    }

    conn.write_raw_packet(&encode_config_finish()).await?;

    loop {
        let raw = conn.read_raw_packet().await?;
        match ConfigServerbound::decode(&raw)? {
            ConfigServerbound::FinishConfiguration => break,
            ConfigServerbound::ClientInformation { view_distance, .. } => {
                client_view_distance = view_distance as i32;
            }
            _ => {}
        }
    }

    // 4. Play Phase
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let (join_tx, join_rx) = oneshot::channel();
    ctx.tick_tx.send(TickEvent::Join {
        profile,
        view_distance: client_view_distance,
        tx: out_tx,
        reply: join_tx,
    })?;
    let player_handle = join_rx.await?;

    info!("Entering Play loop for {peer}");

    let res: Result<()> = loop {
        tokio::select! {
            biased;
            maybe_out = out_rx.recv() => {
                let Some(pkt) = maybe_out else { break Ok(()); };
                if let Err(e) = conn.write_raw_packet(&pkt).await {
                    break Err(e.into());
                }
            }
            maybe_in = conn.read_raw_packet() => {
                match maybe_in {
                    Ok(raw) => {
                        match PlayServerbound::decode(&raw) {
                            Ok(PlayServerbound::Other(_)) => {
                                if let Ok(Some(epkt)) = PlayExtraServerbound::decode(&raw) {
                                    let _ = ctx.tick_tx.send(TickEvent::ExtraPacket {
                                        handle: player_handle,
                                        packet: epkt,
                                    });
                                }
                            }
                            Ok(pkt) => {
                                let _ = ctx.tick_tx.send(TickEvent::Packet {
                                    handle: player_handle,
                                    packet: pkt,
                                });
                            }
                            Err(e) => warn!("Ignoring malformed play packet from {peer}: {e}"),
                        }
                    }
                    Err(mc_proto::ProtoError::Eof) => break Ok(()),
                    Err(e) => break Err(e.into()),
                }
            }
        }
    };

    let _ = ctx.tick_tx.send(TickEvent::Disconnect {
        handle: player_handle,
    });
    res
}

async fn handle_status(conn: &mut FramedConnection, ctx: &SharedContext) -> Result<()> {
    for _ in 0..2 {
        let raw = match conn.read_raw_packet().await {
            Ok(b) => b,
            Err(mc_proto::ProtoError::Eof) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        match StatusServerbound::decode(&raw)? {
            StatusServerbound::StatusRequest => {
                let online = ctx.online_count.load(Ordering::Relaxed);
                let json = serde_json::json!({
                    "version": {
                        "name": VERSION_NAME,
                        "protocol": PROTOCOL_VERSION
                    },
                    "players": {
                        "max": ctx.config.max_players,
                        "online": online,
                        "sample": []
                    },
                    "description": {
                        "text": ctx.config.motd
                    },
                    "enforcesSecureChat": false
                })
                .to_string();
                conn.write_raw_packet(&encode_status_response(&json)).await?;
            }
            StatusServerbound::PingRequest { time } => {
                conn.write_raw_packet(&encode_status_pong(time)).await?;
                return Ok(());
            }
        }
    }
    Ok(())
}
