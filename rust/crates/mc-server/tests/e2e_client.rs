use mc_data::{BlockStateId, PROTOCOL_VERSION, VERSION_NAME, default_state_id, packet_ids};
use mc_math::BlockPos;
use mc_oracle::{OracleClient, with_oracle};
use mc_proto::{
    BlockHitResult, ClientCommandAction, ClientIntent, Direction, FramedConnection,
    HandshakeIntention, HashedStack, InteractionHand, PacketReader, PacketWriter, PlayerActionType,
    encode_serverbound_chat_command, encode_serverbound_client_command,
    encode_serverbound_container_click, encode_serverbound_container_close,
    encode_serverbound_player_action, encode_serverbound_set_creative_mode_slot,
    encode_serverbound_use_item, encode_serverbound_use_item_on,
};
use mc_server::{MinecraftServer, ServerConfig};
use mc_world::{ContainerClickType, ItemStack};
use std::collections::HashSet;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use uuid::Uuid;

fn encode_serverbound_login_hello(name: &str, uuid: Uuid) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::login::serverbound::HELLO);
    w.write_string(name);
    w.write_uuid(uuid);
    w.into_bytes()
}

fn encode_serverbound_login_ack() -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::login::serverbound::LOGIN_ACKNOWLEDGED);
    w.into_bytes()
}

fn encode_serverbound_known_packs() -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::configuration::serverbound::SELECT_KNOWN_PACKS);
    w.write_varint(1);
    w.write_string("minecraft");
    w.write_string("core");
    w.write_string(VERSION_NAME);
    w.into_bytes()
}

fn encode_serverbound_finish_config() -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::configuration::serverbound::FINISH_CONFIGURATION);
    w.into_bytes()
}

fn encode_serverbound_move_pos(x: f64, y: f64, z: f64) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::MOVE_PLAYER_POS);
    w.write_f64(x);
    w.write_f64(y);
    w.write_f64(z);
    w.write_u8(1); // on_ground
    w.into_bytes()
}

fn encode_serverbound_chat(msg: &str) -> Vec<u8> {
    let mut w = PacketWriter::new();
    w.write_varint(packet_ids::play::serverbound::CHAT);
    w.write_string(msg);
    w.write_i64(1_700_000_000_000);
    w.write_i64(0);
    w.write_bool(false); // no signature
    w.write_varint(0); // last seen offset
    w.write_raw(&[0u8; 3]); // fixed bitset(20)
    w.write_u8(0); // checksum
    w.into_bytes()
}

async fn connect_and_join_play(
    addr: std::net::SocketAddr,
    username: &str,
    verify_packets_with_oracle: bool,
) -> (FramedConnection, Vec<String>) {
    let stream = TcpStream::connect(addr).await.unwrap();
    let mut conn = FramedConnection::new(stream);

    // Handshake -> Login
    let hs = HandshakeIntention {
        protocol_version: PROTOCOL_VERSION,
        host_name: "127.0.0.1".into(),
        port: addr.port(),
        intent: ClientIntent::Login,
    };
    conn.write_raw_packet(&hs.encode()).await.unwrap();

    // Login Hello
    conn.write_raw_packet(&encode_serverbound_login_hello(username, Uuid::new_v4()))
        .await
        .unwrap();

    // Expect LoginCompression + LoginFinished
    let comp = conn.read_raw_packet().await.unwrap();
    if verify_packets_with_oracle {
        let (pt, _, _) = with_oracle(|o| o.packet_roundtrip("login", "clientbound", &comp)).unwrap();
        assert_eq!(pt, "minecraft:login_compression");
    }
    let mut r = PacketReader::new(&comp);
    assert_eq!(r.read_varint().unwrap(), packet_ids::login::clientbound::LOGIN_COMPRESSION);
    let threshold = r.read_varint().unwrap();
    conn.set_compression_threshold(Some(threshold as usize));

    let fin = conn.read_raw_packet().await.unwrap();
    if verify_packets_with_oracle {
        let (pt, _, _) = with_oracle(|o| o.packet_roundtrip("login", "clientbound", &fin)).unwrap();
        assert_eq!(pt, "minecraft:login_finished");
    }

    // Ack login -> Configuration phase
    conn.write_raw_packet(&encode_serverbound_login_ack()).await.unwrap();

    // Receive Brand + SelectKnownPacks
    let brand = conn.read_raw_packet().await.unwrap();
    if verify_packets_with_oracle {
        let (pt, _, _) =
            with_oracle(|o| o.packet_roundtrip("configuration", "clientbound", &brand)).unwrap();
        assert_eq!(pt, "minecraft:custom_payload");
    }
    let known = conn.read_raw_packet().await.unwrap();
    if verify_packets_with_oracle {
        let (pt, _, _) =
            with_oracle(|o| o.packet_roundtrip("configuration", "clientbound", &known)).unwrap();
        assert_eq!(pt, "minecraft:select_known_packs");
    }

    // Confirm minecraft:core:26.3
    conn.write_raw_packet(&encode_serverbound_known_packs()).await.unwrap();

    // Receive all 32 RegistryData packets + UpdateTags + FinishConfiguration
    let mut reg_count = 0;
    let mut saw_update_tags = false;
    loop {
        let raw = conn.read_raw_packet().await.unwrap();
        let mut r = PacketReader::new(&raw);
        let pid = r.read_varint().unwrap();
        if pid == packet_ids::configuration::clientbound::REGISTRY_DATA {
            if verify_packets_with_oracle && reg_count < 3 {
                let (pt, _, _) =
                    with_oracle(|o| o.packet_roundtrip("configuration", "clientbound", &raw))
                        .unwrap();
                assert_eq!(pt, "minecraft:registry_data");
            }
            reg_count += 1;
        } else if pid == packet_ids::configuration::clientbound::UPDATE_TAGS {
            if verify_packets_with_oracle {
                let (pt, _, _) =
                    with_oracle(|o| o.packet_roundtrip("configuration", "clientbound", &raw))
                        .unwrap();
                assert_eq!(pt, "minecraft:update_tags");
            }
            saw_update_tags = true;
        } else if pid == packet_ids::configuration::clientbound::FINISH_CONFIGURATION {
            if verify_packets_with_oracle {
                let (pt, _, _) =
                    with_oracle(|o| o.packet_roundtrip("configuration", "clientbound", &raw))
                        .unwrap();
                assert_eq!(pt, "minecraft:finish_configuration");
            }
            break;
        }
    }
    assert_eq!(reg_count, 32, "Expected all 32 synced 26.3 registries");
    assert!(saw_update_tags, "Expected minecraft:update_tags during Configuration");

    // Enter Play!
    conn.write_raw_packet(&encode_serverbound_finish_config()).await.unwrap();

    let mut received_play_types = Vec::new();
    let mut seen_types = HashSet::new();
    // Expect PlayLogin, DefaultSpawn, GameEvent, ChunkCacheCenter, ChunkBatchStart,
    // (2*2+1)^2 = 25 chunks, ChunkBatchFinished, PlayerPosition, PlayerInfoUpdate, SystemChat
    loop {
        let raw = conn.read_raw_packet().await.unwrap();
        let mut r = PacketReader::new(&raw);
        let pid = r.read_varint().unwrap();

        if verify_packets_with_oracle && seen_types.insert(pid) {
            let (pt, _, reenc) =
                with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
            if pt == "minecraft:login" {
                // ClientboundLoginPacket stores `levels` in a Java HashSet (unordered on re-encode)
                assert_eq!(reenc.len(), raw.len(), "Live TCP minecraft:login length mismatch");
            } else {
                assert_eq!(reenc, raw, "Live TCP play packet {pt} differed from Java re-encode");
            }
            received_play_types.push(pt.clone());
            if pt == "minecraft:system_chat" {
                break;
            }
        } else {
            if pid == packet_ids::play::clientbound::SYSTEM_CHAT {
                received_play_types.push("minecraft:system_chat".into());
                break;
            }
            received_play_types.push(format!("id:{pid}"));
        }
    }

    (conn, received_play_types)
}

#[tokio::test]
async fn e2e_status_login_config_play_chunks_movement_and_chat() {
    let root = OracleClient::repo_root();
    let mut cfg = ServerConfig::default();
    cfg.bind_addr = "127.0.0.1".into();
    cfg.port = 0; // Ephemeral port
    cfg.view_distance = 2; // 5x5 = 25 chunks
    cfg.online_mode = false;
    cfg.generator_type = "flat".into();
    cfg.enable_rcon = true;
    cfg.rcon_port = 0;
    cfg.rcon_password = "silicon263".into();
    cfg.management_server_enabled = true;
    cfg.management_server_port = 0;
    cfg.management_server_secret = "secret263".into();
    cfg.data_dir = root.join("src").join("main").join("resources").join("data");
    cfg.network_tags_path = Some(root.join("rust").join("fixtures").join("vanilla_network_tags.bin"));
    cfg.world_dir = std::env::temp_dir().join("siliconmc_e2e_world");
    let _ = std::fs::remove_dir_all(&cfg.world_dir);

    let server = MinecraftServer::bind(cfg).await.unwrap();
    let addr = server.local_addr();
    let rcon_addr = server.rcon_addr().unwrap();
    let jsonrpc_addr = server.jsonrpc_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server_task = tokio::spawn(async move {
        let _ = server.run(Some(shutdown_rx)).await;
    });

    // 1. Test Status Ping
    {
        let mut conn = FramedConnection::new(TcpStream::connect(addr).await.unwrap());
        let hs = HandshakeIntention {
            protocol_version: PROTOCOL_VERSION,
            host_name: "127.0.0.1".into(),
            port: addr.port(),
            intent: ClientIntent::Status,
        };
        conn.write_raw_packet(&hs.encode()).await.unwrap();
        let mut req = PacketWriter::new();
        req.write_varint(packet_ids::status::serverbound::STATUS_REQUEST);
        conn.write_raw_packet(&req.into_bytes()).await.unwrap();

        let status_raw = conn.read_raw_packet().await.unwrap();
        let (pt, _, _) =
            with_oracle(|o| o.packet_roundtrip("status", "clientbound", &status_raw)).unwrap();
        assert_eq!(pt, "minecraft:status_response");

        let mut ping = PacketWriter::new();
        ping.write_varint(packet_ids::status::serverbound::PING_REQUEST);
        ping.write_i64(777_000);
        conn.write_raw_packet(&ping.into_bytes()).await.unwrap();
        let pong = conn.read_raw_packet().await.unwrap();
        let (pt2, _, _) =
            with_oracle(|o| o.packet_roundtrip("status", "clientbound", &pong)).unwrap();
        assert_eq!(pt2, "minecraft:pong_response");
    }

    // 2. Client 1 joins Play — EVERY unique Play packet verified byte-for-byte against Java 26.3!
    let (mut c1, types1) = connect_and_join_play(addr, "Alice", true).await;
    for required in [
        "minecraft:login",
        "minecraft:player_abilities",
        "minecraft:set_health",
        "minecraft:set_held_slot",
        "minecraft:commands",
        "minecraft:set_time",
        "minecraft:container_set_content",
        "minecraft:set_default_spawn_position",
        "minecraft:game_event",
        "minecraft:set_chunk_cache_center",
        "minecraft:chunk_batch_start",
        "minecraft:level_chunk_with_light",
        "minecraft:chunk_batch_finished",
        "minecraft:player_position",
        "minecraft:player_info_update",
        "minecraft:system_chat",
    ] {
        assert!(
            types1.iter().any(|t| t == required),
            "Client 1 did not receive {required}; got {types1:?}"
        );
    }

    // 3. Client 2 joins Play — verify multiplayer visibility, movement sync & chat broadcast!
    let (mut c2, _) = connect_and_join_play(addr, "Bob", false).await;

    // Client 1 moves and sends chat
    c1.write_raw_packet(&encode_serverbound_move_pos(5.5, -60.0, -3.25))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_chat("Hello from SiliconMC Rust!"))
        .await
        .unwrap();

    // Client 2 should receive EntityPositionSync + SystemChat (verified with Java oracle!)
    let mut got_pos_sync = false;
    let mut got_chat = false;
    for _ in 0..15 {
        let raw = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            c2.read_raw_packet(),
        )
        .await
        .unwrap()
        .unwrap();
        let mut r = PacketReader::new(&raw);
        let pid = r.read_varint().unwrap();
        if pid == packet_ids::play::clientbound::ENTITY_POSITION_SYNC {
            let (pt, _, reenc) =
                with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
            assert_eq!(pt, "minecraft:entity_position_sync");
            assert_eq!(reenc, raw);
            got_pos_sync = true;
        } else if pid == packet_ids::play::clientbound::SYSTEM_CHAT {
            let (pt, _, reenc) =
                with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
            assert_eq!(pt, "minecraft:system_chat");
            assert_eq!(reenc, raw);
            let tag = r.read_nbt_network().unwrap();
            let text_opt = match &tag {
                mc_nbt::NbtTag::String(s) => Some(s.as_str()),
                mc_nbt::NbtTag::Compound(c) => c.get_string("text"),
                _ => None,
            };
            if text_opt == Some("<Alice> Hello from SiliconMC Rust!") {
                got_chat = true;
            }
        }
        if got_pos_sync && got_chat {
            break;
        }
    }
    assert!(got_pos_sync, "Client 2 did not receive EntityPositionSync");
    assert!(got_chat, "Client 2 did not receive Alice's chat broadcast");

    // 4. Phase 3 Live Gameplay E2E: Creative slot -> Place torch -> Light update -> Break block -> /setblock -> /save-all!
    let torch_item = ItemStack::from_name("minecraft:torch", 16).unwrap();
    let torch_state = default_state_id("minecraft:torch").unwrap();
    c1.write_raw_packet(&encode_serverbound_set_creative_mode_slot(36, &torch_item))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_use_item_on(
        InteractionHand::MainHand,
        BlockHitResult {
            pos: BlockPos::new(2, -61, 2),
            direction: Direction::Up,
            click_x: 0.5,
            click_y: 1.0,
            click_z: 0.5,
            inside: false,
            world_border_hit: false,
        },
        101,
    ))
    .await
    .unwrap();

    // Client 1 must receive BlockUpdate(2, -60, 2, torch) + LightUpdate + BlockChangedAck(101), all verified by Java oracle!
    let mut got_torch_place = false;
    let mut got_light_update = false;
    let mut got_ack = false;
    for _ in 0..25 {
        let raw = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            c1.read_raw_packet(),
        )
        .await
        .unwrap()
        .unwrap();
        let mut r = PacketReader::new(&raw);
        let pid = r.read_varint().unwrap();
        if pid == packet_ids::play::clientbound::BLOCK_UPDATE {
            let (pt, _, reenc) =
                with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
            assert_eq!(pt, "minecraft:block_update");
            assert_eq!(reenc, raw);
            let pos = r.read_block_pos().unwrap();
            let st = r.read_varint().unwrap() as u32;
            if pos == BlockPos::new(2, -60, 2) && st == torch_state.0 {
                got_torch_place = true;
            }
        } else if pid == packet_ids::play::clientbound::LIGHT_UPDATE {
            let (pt, _, reenc) =
                with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
            assert_eq!(pt, "minecraft:light_update");
            assert_eq!(reenc, raw);
            got_light_update = true;
        } else if pid == packet_ids::play::clientbound::BLOCK_CHANGED_ACK {
            let (pt, _, reenc) =
                with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
            assert_eq!(pt, "minecraft:block_changed_ack");
            assert_eq!(reenc, raw);
            if r.read_varint().unwrap() == 101 {
                got_ack = true;
            }
        }
        if got_torch_place && got_light_update && got_ack {
            break;
        }
    }
    assert!(got_torch_place, "Client 1 did not receive torch BlockUpdate");
    assert!(got_light_update, "Client 1 did not receive LightUpdate for torch");
    assert!(got_ack, "Client 1 did not receive BlockChangedAck(101)");

    // Break the torch, run /setblock 3 -60 3 stone, and /save-all
    c1.write_raw_packet(&encode_serverbound_player_action(
        PlayerActionType::StartDestroyBlock,
        BlockPos::new(2, -60, 2),
        Direction::Up,
        102,
    ))
    .await
    .unwrap();
    c1.write_raw_packet(&encode_serverbound_chat_command("setblock 3 -60 3 stone"))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_chat_command("gamemode survival"))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_chat_command("save-all"))
        .await
        .unwrap();

    let mut got_break_air = false;
    let mut got_setblock_stone = false;
    let mut got_saved_msg = false;
    for _ in 0..30 {
        let raw = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            c1.read_raw_packet(),
        )
        .await
        .unwrap()
        .unwrap();
        let mut r = PacketReader::new(&raw);
        let pid = r.read_varint().unwrap();
        if pid == packet_ids::play::clientbound::BLOCK_UPDATE {
            let pos = r.read_block_pos().unwrap();
            let st = r.read_varint().unwrap() as u32;
            if pos == BlockPos::new(2, -60, 2) && st == BlockStateId::AIR.0 {
                got_break_air = true;
            } else if pos == BlockPos::new(3, -60, 3) && st == BlockStateId::STONE.0 {
                got_setblock_stone = true;
            }
        } else if pid == packet_ids::play::clientbound::SYSTEM_CHAT {
            let tag = r.read_nbt_network().unwrap();
            if let mc_nbt::NbtTag::String(s) = tag {
                if s.starts_with("Saved the game") {
                    got_saved_msg = true;
                }
            }
        }
        if got_break_air && got_setblock_stone && got_saved_msg {
            break;
        }
    }
    assert!(got_break_air, "Did not receive AIR BlockUpdate after breaking torch");
    assert!(got_setblock_stone, "Did not receive STONE BlockUpdate from /setblock");
    assert!(got_saved_msg, "Did not receive /save-all confirmation");

    // 5. Phase 5 Live Gameplay E2E: Redstone (lever -> wire -> redstone_lamp) + /summon pig -> AddEntity + SetEntityData!
    c1.write_raw_packet(&encode_serverbound_chat_command("setblock 4 -60 4 lever"))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_chat_command("setblock 5 -60 4 redstone_wire"))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_chat_command("setblock 6 -60 4 redstone_lamp"))
        .await
        .unwrap();
    // Flip lever ON via UseItemOn -> lamp at (6, -60, 4) must light up!
    c1.write_raw_packet(&encode_serverbound_use_item_on(
        InteractionHand::MainHand,
        BlockHitResult {
            pos: BlockPos::new(4, -60, 4),
            direction: Direction::Up,
            click_x: 0.5,
            click_y: 0.5,
            click_z: 0.5,
            inside: false,
            world_border_hit: false,
        },
        103,
    ))
    .await
    .unwrap();
    c1.write_raw_packet(&encode_serverbound_chat_command("summon pig 1 -60 1"))
        .await
        .unwrap();

    let mut got_lit_lamp = false;
    let mut got_pig_spawn = false;
    let mut got_pig_meta = false;
    for _ in 0..80 {
        let raw = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            c1.read_raw_packet(),
        )
        .await
        .unwrap()
        .unwrap();
        let mut r = PacketReader::new(&raw);
        let pid = r.read_varint().unwrap();
        if pid == packet_ids::play::clientbound::BLOCK_UPDATE {
            let pos = r.read_block_pos().unwrap();
            let st = r.read_varint().unwrap() as u32;
            if pos == BlockPos::new(6, -60, 4) && mc_data::block_behaviors::block_behavior(st).is_lit() {
                got_lit_lamp = true;
            }
        } else if pid == packet_ids::play::clientbound::ADD_ENTITY {
            let (pt, _, reenc) =
                with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
            assert_eq!(pt, "minecraft:add_entity");
            assert_eq!(reenc, raw);
            got_pig_spawn = true;
        } else if pid == packet_ids::play::clientbound::SET_ENTITY_DATA {
            let (pt, _, reenc) =
                with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
            assert_eq!(pt, "minecraft:set_entity_data");
            assert_eq!(reenc, raw);
            got_pig_meta = true;
        }
        if got_lit_lamp && got_pig_spawn && got_pig_meta {
            break;
        }
    }
    assert!(got_lit_lamp, "Lever + wire did not light redstone_lamp at (6, -60, 4)");
    assert!(got_pig_spawn, "Did not receive AddEntity after /summon pig");
    assert!(got_pig_meta, "Did not receive SetEntityData after /summon pig");

    // 6. Phase 5c Live Gameplay E2E: Chest + Furnace ignition + Survival fall death + PerformRespawn!
    c1.write_raw_packet(&encode_serverbound_chat_command("gamemode creative"))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_chat_command("setblock 8 -60 4 chest"))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_chat_command("setblock 9 -60 4 furnace"))
        .await
        .unwrap();
    let raw_iron = ItemStack::from_name("minecraft:raw_iron", 1).unwrap();
    let coal = ItemStack::from_name("minecraft:coal", 1).unwrap();
    c1.write_raw_packet(&encode_serverbound_set_creative_mode_slot(37, &raw_iron))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_set_creative_mode_slot(38, &coal))
        .await
        .unwrap();

    // Open Chest at (8, -60, 4) -> expect OpenScreen (container_id = 1)
    c1.write_raw_packet(&encode_serverbound_use_item_on(
        InteractionHand::MainHand,
        BlockHitResult {
            pos: BlockPos::new(8, -60, 4),
            direction: Direction::Up,
            click_x: 0.5,
            click_y: 0.5,
            click_z: 0.5,
            inside: false,
            world_border_hit: false,
        },
        104,
    ))
    .await
    .unwrap();
    // Close Chest (cid=1) and Open Furnace at (9, -60, 4) (cid=2)
    c1.write_raw_packet(&encode_serverbound_container_close(1))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_use_item_on(
        InteractionHand::MainHand,
        BlockHitResult {
            pos: BlockPos::new(9, -60, 4),
            direction: Direction::Up,
            click_x: 0.5,
            click_y: 0.5,
            click_z: 0.5,
            inside: false,
            world_border_hit: false,
        },
        105,
    ))
    .await
    .unwrap();
    // Put raw_iron (furnace menu slot 31 = hotbar slot 1) into furnace input (slot 0),
    // and coal (furnace menu slot 32 = hotbar slot 2) into furnace fuel (slot 1)!
    for (slot_num, btn) in [(31i16, 0i8), (0, 0), (32, 0), (1, 0)] {
        c1.write_raw_packet(&encode_serverbound_container_click(
            2,
            1,
            slot_num,
            btn,
            ContainerClickType::Pickup,
            &[],
            &HashedStack::Empty,
        ))
        .await
        .unwrap();
    }
    c1.write_raw_packet(&encode_serverbound_chat_command("save-all"))
        .await
        .unwrap();

    let mut open_screens_verified = 0usize;
    let mut got_furnace_data = false;
    let mut got_furnace_lit = false;
    for _ in 0..120 {
        let raw = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            c1.read_raw_packet(),
        )
        .await
        .unwrap()
        .unwrap();
        let mut r = PacketReader::new(&raw);
        let pid = r.read_varint().unwrap();
        if pid == packet_ids::play::clientbound::OPEN_SCREEN {
            let (pt, _, reenc) =
                with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
            assert_eq!(pt, "minecraft:open_screen");
            assert_eq!(reenc, raw);
            open_screens_verified += 1;
        } else if pid == packet_ids::play::clientbound::CONTAINER_SET_DATA {
            if !got_furnace_data {
                let (pt, _, reenc) =
                    with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
                assert_eq!(pt, "minecraft:container_set_data");
                assert_eq!(reenc, raw);
                got_furnace_data = true;
            }
        } else if pid == packet_ids::play::clientbound::BLOCK_UPDATE {
            let pos = r.read_block_pos().unwrap();
            let st = r.read_varint().unwrap() as u32;
            if pos == BlockPos::new(9, -60, 4) && mc_data::block_behaviors::block_behavior(st).is_lit() {
                got_furnace_lit = true;
            }
        }
        if open_screens_verified >= 2 && got_furnace_data && got_furnace_lit {
            break;
        }
    }
    assert!(open_screens_verified >= 2, "Expected both Chest and Furnace OpenScreen packets");
    assert!(got_furnace_data, "Expected ContainerSetData for Furnace");
    assert!(got_furnace_lit, "Furnace did not ignite when loaded with raw_iron + coal!");

    // Survival fall damage -> death -> PerformRespawn -> ClientboundRespawnPacket verified vs Java!
    c1.write_raw_packet(&encode_serverbound_chat_command("gamemode survival"))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_chat_command("tp 0 -20 0"))
        .await
        .unwrap();
    // Simulate landing at y = -60 (40-block fall -> 37 damage -> death!)
    c1.write_raw_packet(&encode_serverbound_move_pos(0.0, -60.0, 0.0))
        .await
        .unwrap();

    let mut got_hurt = false;
    let mut got_death_health = false;
    let mut got_death_event = false;
    for _ in 0..80 {
        let raw = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            c1.read_raw_packet(),
        )
        .await
        .unwrap()
        .unwrap();
        let mut r = PacketReader::new(&raw);
        let pid = r.read_varint().unwrap();
        if pid == packet_ids::play::clientbound::HURT_ANIMATION {
            let (pt, _, reenc) =
                with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
            assert_eq!(pt, "minecraft:hurt_animation");
            assert_eq!(reenc, raw);
            got_hurt = true;
        } else if pid == packet_ids::play::clientbound::SET_HEALTH {
            let (pt, _, reenc) =
                with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
            assert_eq!(pt, "minecraft:set_health");
            assert_eq!(reenc, raw);
            let hp = r.read_f32().unwrap();
            if hp <= 0.0 {
                got_death_health = true;
            }
        } else if pid == packet_ids::play::clientbound::ENTITY_EVENT {
            let (pt, _, reenc) =
                with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
            assert_eq!(pt, "minecraft:entity_event");
            assert_eq!(reenc, raw);
            got_death_event = true;
        }
        if got_hurt && got_death_health && got_death_event {
            break;
        }
    }
    assert!(got_hurt, "Expected HurtAnimation on fatal fall");
    assert!(got_death_health, "Expected SetHealth(0.0) on fatal fall");
    assert!(got_death_event, "Expected EntityEvent(3) death animation");

    // Request respawn -> verify ClientboundRespawnPacket byte-for-byte against Java 26.3 oracle!
    c1.write_raw_packet(&encode_serverbound_client_command(
        ClientCommandAction::PerformRespawn,
    ))
    .await
    .unwrap();
    let mut got_respawn = false;
    for _ in 0..40 {
        let raw = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            c1.read_raw_packet(),
        )
        .await
        .unwrap()
        .unwrap();
        let mut r = PacketReader::new(&raw);
        let pid = r.read_varint().unwrap();
        if pid == packet_ids::play::clientbound::RESPAWN {
            let (pt, _, reenc) =
                with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
            assert_eq!(pt, "minecraft:respawn");
            assert_eq!(reenc, raw);
            got_respawn = true;
            break;
        }
    }
    assert!(got_respawn, "Did not receive ClientboundRespawnPacket after PerformRespawn");

    // 7. Phase 4d + Phase 6 Live E2E: /dimension nether & /dimension end + Live RCON + Live 26.3 JSON-RPC!
    for dim_cmd in ["dimension nether", "dimension end"] {
        c1.write_raw_packet(&encode_serverbound_chat_command(dim_cmd))
            .await
            .unwrap();
        let mut saw_dim_respawn = false;
        let mut saw_dim_chunk = false;
        for _ in 0..80 {
            let raw = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                c1.read_raw_packet(),
            )
            .await
            .unwrap()
            .unwrap();
            let mut r = PacketReader::new(&raw);
            let pid = r.read_varint().unwrap();
            if pid == packet_ids::play::clientbound::RESPAWN {
                let (pt, _, reenc) =
                    with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
                assert_eq!(pt, "minecraft:respawn");
                assert_eq!(reenc, raw);
                saw_dim_respawn = true;
            } else if pid == packet_ids::play::clientbound::LEVEL_CHUNK_WITH_LIGHT && saw_dim_respawn {
                let (pt, _, reenc) =
                    with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
                assert_eq!(pt, "minecraft:level_chunk_with_light");
                assert_eq!(reenc, raw);
                saw_dim_chunk = true;
            }
            if saw_dim_respawn && saw_dim_chunk {
                break;
            }
        }
        assert!(saw_dim_respawn, "Did not receive Respawn for /{dim_cmd}");
        assert!(saw_dim_chunk, "Did not receive valid LevelChunkWithLight for /{dim_cmd}");
    }

    // Live Source RCON test: auth -> `list` -> `op Alice`
    {
        async fn rcon_send(s: &mut TcpStream, req_id: i32, ty: i32, payload: &str) {
            let len = (4 + 4 + payload.len() + 2) as i32;
            let mut buf = Vec::new();
            buf.extend_from_slice(&len.to_le_bytes());
            buf.extend_from_slice(&req_id.to_le_bytes());
            buf.extend_from_slice(&ty.to_le_bytes());
            buf.extend_from_slice(payload.as_bytes());
            buf.extend_from_slice(&[0, 0]);
            s.write_all(&buf).await.unwrap();
        }
        async fn rcon_recv(s: &mut TcpStream) -> (i32, i32, String) {
            let mut lb = [0u8; 4];
            s.read_exact(&mut lb).await.unwrap();
            let len = i32::from_le_bytes(lb) as usize;
            let mut body = vec![0u8; len];
            s.read_exact(&mut body).await.unwrap();
            let id = i32::from_le_bytes(body[0..4].try_into().unwrap());
            let ty = i32::from_le_bytes(body[4..8].try_into().unwrap());
            let text = String::from_utf8_lossy(&body[8..len - 2]).to_string();
            (id, ty, text)
        }
        let mut rcon = TcpStream::connect(rcon_addr).await.unwrap();
        rcon_send(&mut rcon, 1, 3, "silicon263").await;
        let (aid, _, _) = rcon_recv(&mut rcon).await;
        assert_eq!(aid, 1, "RCON auth failed");

        rcon_send(&mut rcon, 2, 2, "list").await;
        let (lid, _, lmsg) = rcon_recv(&mut rcon).await;
        assert_eq!(lid, 2);
        assert!(lmsg.contains("Alice") && lmsg.contains("Bob"), "Unexpected RCON list: {lmsg}");

        rcon_send(&mut rcon, 3, 2, "op Alice").await;
        let (_, _, omsg) = rcon_recv(&mut rcon).await;
        assert!(omsg.contains("Made Alice a server operator"), "Unexpected RCON op: {omsg}");
    }

    // Live Minecraft 26.3 JSON-RPC 2.0 Management API test (Bearer auth + server/status + operators)
    {
        async fn rpc_call(addr: std::net::SocketAddr, secret: &str, body: &str) -> String {
            let mut s = TcpStream::connect(addr).await.unwrap();
            let req = format!(
                "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {secret}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            s.write_all(req.as_bytes()).await.unwrap();
            let mut resp = String::new();
            s.read_to_string(&mut resp).await.unwrap();
            resp
        }
        let status_resp = rpc_call(
            jsonrpc_addr,
            "secret263",
            r#"{"jsonrpc":"2.0","id":1,"method":"minecraft:server/status","params":{}}"#,
        )
        .await;
        assert!(status_resp.contains("200 OK"), "JSON-RPC status HTTP failed: {status_resp}");
        assert!(status_resp.contains("\"protocol\":777"), "Missing protocol 777: {status_resp}");
        assert!(status_resp.contains("\"Alice\""), "Missing Alice in status: {status_resp}");

        let ops_resp = rpc_call(
            jsonrpc_addr,
            "secret263",
            r#"{"jsonrpc":"2.0","id":2,"method":"minecraft:operators","params":{}}"#,
        )
        .await;
        assert!(ops_resp.contains("\"Alice\""), "Alice not found in JSON-RPC operators: {ops_resp}");
    }

    // 8. Batch 7 Live E2E: Hopper OpenScreen + Water/Lava -> Obsidian + UseItem Armor Quick-Equip!
    c1.write_raw_packet(&encode_serverbound_chat_command("dimension overworld"))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_chat_command("gamemode creative"))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_chat_command("setblock 10 -60 4 hopper"))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_use_item_on(
        InteractionHand::MainHand,
        BlockHitResult {
            pos: BlockPos::new(10, -60, 4),
            direction: Direction::Up,
            click_x: 0.5,
            click_y: 0.5,
            click_z: 0.5,
            inside: false,
            world_border_hit: false,
        },
        106,
    ))
    .await
    .unwrap();
    c1.write_raw_packet(&encode_serverbound_chat_command("setblock 12 -60 4 lava"))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_chat_command("setblock 13 -60 4 water"))
        .await
        .unwrap();
    let dchest = ItemStack::from_name("minecraft:diamond_chestplate", 1).unwrap();
    c1.write_raw_packet(&encode_serverbound_set_creative_mode_slot(36, &dchest))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_use_item(
        InteractionHand::MainHand,
        107,
        0.0,
        0.0,
    ))
    .await
    .unwrap();

    let obsidian_id = default_state_id("minecraft:obsidian").unwrap().0;
    let mut got_hopper_screen = false;
    let mut got_obsidian = false;
    let mut got_equipped_chest = false;
    for _ in 0..160 {
        let raw = tokio::time::timeout(
            std::time::Duration::from_secs(4),
            c1.read_raw_packet(),
        )
        .await
        .unwrap()
        .unwrap();
        let mut r = PacketReader::new(&raw);
        let pid = r.read_varint().unwrap();
        if pid == packet_ids::play::clientbound::OPEN_SCREEN {
            let (pt, _, reenc) =
                with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
            assert_eq!(pt, "minecraft:open_screen");
            assert_eq!(reenc, raw);
            let _cid = r.read_varint().unwrap();
            let mtype = r.read_varint().unwrap();
            if mtype == mc_data::recipes::menu_types::HOPPER {
                got_hopper_screen = true;
            }
        } else if pid == packet_ids::play::clientbound::BLOCK_UPDATE {
            let pos = r.read_block_pos().unwrap();
            let st = r.read_varint().unwrap() as u32;
            if pos == BlockPos::new(12, -60, 4) && st == obsidian_id {
                got_obsidian = true;
            }
        } else if pid == packet_ids::play::clientbound::CONTAINER_SET_SLOT {
            let cid = r.read_varint().unwrap();
            let _sid = r.read_varint().unwrap();
            let slot = r.read_i16().unwrap();
            if cid == 0 && slot == 6 {
                got_equipped_chest = true;
            }
        }
        if got_hopper_screen && got_obsidian && got_equipped_chest {
            break;
        }
    }
    assert!(got_hopper_screen, "Did not receive OpenScreen(HOPPER)");
    assert!(got_obsidian, "Water next to lava source did not turn into obsidian!");
    assert!(got_equipped_chest, "UseItem did not quick-equip diamond_chestplate to slot 6");

    // 9. Batch 8 Live E2E: /place template igloo/top loading real vanilla 26.3 .nbt structure!
    c1.write_raw_packet(&encode_serverbound_chat_command("place template igloo/top 16 -60 16"))
        .await
        .unwrap();
    let snow_id = default_state_id("minecraft:snow_block").unwrap().0;
    let mut got_structure_msg = false;
    let mut got_igloo_snow = false;
    for _ in 0..400 {
        let raw = tokio::time::timeout(
            std::time::Duration::from_secs(4),
            c1.read_raw_packet(),
        )
        .await
        .unwrap()
        .unwrap();
        let mut r = PacketReader::new(&raw);
        let pid = r.read_varint().unwrap();
        if pid == packet_ids::play::clientbound::BLOCK_UPDATE {
            let _pos = r.read_block_pos().unwrap();
            let st = r.read_varint().unwrap() as u32;
            if st == snow_id {
                got_igloo_snow = true;
            }
        } else if pid == packet_ids::play::clientbound::SYSTEM_CHAT {
            if let Ok(mc_nbt::NbtTag::String(s)) = r.read_nbt_network() {
                if s.starts_with("Placed template igloo/top") {
                    got_structure_msg = true;
                }
            }
        }
        if got_structure_msg && got_igloo_snow {
            break;
        }
    }
    assert!(got_structure_msg, "Did not receive confirmation for /place template igloo/top");
    assert!(got_igloo_snow, "Did not receive minecraft:snow_block BlockUpdate from igloo/top.nbt!");

    // 10. Batch 9 Live E2E: /xp + /weather + 2-block Oak Door placement + level.dat & playerdata/<uuid>.dat persistence!
    c1.write_raw_packet(&encode_serverbound_chat_command("xp add 55 points"))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_chat_command("weather thunder"))
        .await
        .unwrap();
    let door_item = ItemStack::from_name("minecraft:oak_door", 1).unwrap();
    c1.write_raw_packet(&encode_serverbound_set_creative_mode_slot(36, &door_item))
        .await
        .unwrap();
    c1.write_raw_packet(&encode_serverbound_use_item_on(
        InteractionHand::MainHand,
        BlockHitResult {
            pos: BlockPos::new(5, -61, 8),
            direction: Direction::Up,
            click_x: 0.5,
            click_y: 1.0,
            click_z: 0.5,
            inside: false,
            world_border_hit: false,
        },
        108,
    ))
    .await
    .unwrap();
    c1.write_raw_packet(&encode_serverbound_chat_command("save-all"))
        .await
        .unwrap();

    let mut got_xp_lvl5 = false;
    let mut got_thunder_event = false;
    let mut got_door_lower = false;
    let mut got_door_upper = false;
    let mut got_save_confirm = false;
    for _ in 0..600 {
        let raw = tokio::time::timeout(
            std::time::Duration::from_secs(4),
            c1.read_raw_packet(),
        )
        .await
        .unwrap()
        .unwrap();
        let mut r = PacketReader::new(&raw);
        let pid = r.read_varint().unwrap();
        if pid == packet_ids::play::clientbound::SET_EXPERIENCE {
            let (pt, _, reenc) =
                with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
            assert_eq!(pt, "minecraft:set_experience");
            assert_eq!(reenc, raw);
            let _prog = r.read_f32().unwrap();
            let lvl = r.read_varint().unwrap();
            let tot = r.read_varint().unwrap();
            if lvl == 5 && tot == 55 {
                got_xp_lvl5 = true;
            }
        } else if pid == packet_ids::play::clientbound::GAME_EVENT {
            let ev = r.read_u8().unwrap();
            let val = r.read_f32().unwrap();
            if ev == 8 && val >= 0.9 {
                let (pt, _, reenc) =
                    with_oracle(|o| o.packet_roundtrip("play", "clientbound", &raw)).unwrap();
                assert_eq!(pt, "minecraft:game_event");
                assert_eq!(reenc, raw);
                got_thunder_event = true;
            }
        } else if pid == packet_ids::play::clientbound::BLOCK_UPDATE {
            let pos = r.read_block_pos().unwrap();
            let st = mc_data::BlockStateId(r.read_varint().unwrap() as u32);
            if pos == BlockPos::new(5, -60, 8) && st.block_name() == "minecraft:oak_door" {
                got_door_lower = true;
            } else if pos == BlockPos::new(5, -59, 8) && st.block_name() == "minecraft:oak_door" {
                got_door_upper = true;
            }
        } else if pid == packet_ids::play::clientbound::SYSTEM_CHAT {
            if let Ok(mc_nbt::NbtTag::String(s)) = r.read_nbt_network() {
                if s.contains("level.dat") && s.contains("playerdata") {
                    got_save_confirm = true;
                }
            }
        }
        if got_xp_lvl5 && got_thunder_event && got_door_lower && got_door_upper && got_save_confirm {
            break;
        }
    }
    assert!(got_xp_lvl5, "Expected ClientboundSetExperiencePacket(level=5, total=55)");
    assert!(got_thunder_event, "Expected GameEvent(THUNDER_LEVEL_CHANGE=1.0)");
    assert!(got_door_lower && got_door_upper, "Expected both lower and upper halves of oak_door!");
    assert!(got_save_confirm, "Expected /save-all confirmation with level.dat and playerdata");

    let e2e_world = std::env::temp_dir().join("siliconmc_e2e_world");
    assert!(e2e_world.join("level.dat").is_file(), "level.dat was not written!");
    let alice_uuid = mc_proto::offline_player_uuid("Alice");
    let loaded_alice = mc_server::playerdata::load_player_data(&e2e_world, alice_uuid)
        .expect("Alice's playerdata/<uuid>.dat was not written!");
    assert_eq!(loaded_alice.xp_level, 5);
    assert_eq!(loaded_alice.xp_total, 55);

    let _ = shutdown_tx.send(());
    let _ = server_task.await;
    let _ = std::fs::remove_dir_all(e2e_world);
}
