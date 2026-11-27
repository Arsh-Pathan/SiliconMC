use mc_data::BlockStateId;
use mc_data::default_state_id;
use mc_data::items::item_by_name;
use mc_math::{BlockPos, Vec3};
use mc_proto::{CommandArgType, CommandNodeEntry, encode_play_commands};

/// Build the Brigadier-compatible `ClientboundCommandsPacket` tree sent to every player on join.
pub fn build_commands_packet() -> Vec<u8> {
    // Node indices:
    // 0: root
    // 1: literal "gamemode"
    //   2: arg "gamemode" (minecraft:gamemode = 42), executable
    // 3: literal "tp"
    //   4: arg "location" (minecraft:vec3 = 10), executable
    // 5: literal "setblock"
    //   6: arg "pos" (minecraft:block_pos = 8)
    //     7: arg "block" (minecraft:block_state = 12), executable
    // 8: literal "give"
    //   9: arg "item" (minecraft:item_stack = 14), executable
    //     10: arg "count" (brigadier:integer = 3, min=1, max=2560), executable
    // 11: literal "time"
    //   12: literal "set"
    //     13: arg "time" (brigadier:string SINGLE_WORD), executable
    //   14: literal "query", executable
    // 15: literal "save-all", executable
    // 16: literal "list", executable
    // 17: literal "help", executable
    // 18: literal "stop", executable
    // 19: literal "seed", executable
    // 20: literal "biome", executable
    // 21: literal "summon"
    //   22: arg "entity" (brigadier:string SINGLE_WORD), executable
    //     23: arg "pos" (minecraft:vec3 = 10), executable
    // 24: literal "kill", executable
    let nodes = vec![
        CommandNodeEntry::root(vec![1, 3, 5, 8, 11, 15, 16, 17, 18, 19, 20, 21, 24, 25, 27, 29, 31, 33, 35, 37]),
        CommandNodeEntry::literal("gamemode", false, vec![2]),
        CommandNodeEntry::argument("gamemode", CommandArgType::GameMode, true, vec![]),
        CommandNodeEntry::literal("tp", false, vec![4]),
        CommandNodeEntry::argument("location", CommandArgType::Vec3, true, vec![]),
        CommandNodeEntry::literal("setblock", false, vec![6]),
        CommandNodeEntry::argument("pos", CommandArgType::BlockPos, false, vec![7]),
        CommandNodeEntry::argument("block", CommandArgType::BlockState, true, vec![]),
        CommandNodeEntry::literal("give", false, vec![9]),
        CommandNodeEntry::argument("item", CommandArgType::ItemStack, true, vec![10]),
        CommandNodeEntry::argument(
            "count",
            CommandArgType::Integer {
                min: Some(1),
                max: Some(2560),
            },
            true,
            vec![],
        ),
        CommandNodeEntry::literal("time", false, vec![12, 14]),
        CommandNodeEntry::literal("set", false, vec![13]),
        CommandNodeEntry::argument("value", CommandArgType::SingleWord, true, vec![]),
        CommandNodeEntry::literal("query", true, vec![]),
        CommandNodeEntry::literal("save-all", true, vec![]),
        CommandNodeEntry::literal("list", true, vec![]),
        CommandNodeEntry::literal("help", true, vec![]),
        CommandNodeEntry::literal("stop", true, vec![]),
        CommandNodeEntry::literal("seed", true, vec![]),
        CommandNodeEntry::literal("biome", true, vec![]),
        CommandNodeEntry::literal("summon", false, vec![22]),
        CommandNodeEntry::argument("entity", CommandArgType::SingleWord, true, vec![23]),
        CommandNodeEntry::argument("pos", CommandArgType::Vec3, true, vec![]),
        CommandNodeEntry::literal("kill", true, vec![]),
        CommandNodeEntry::literal("dimension", false, vec![26]),
        CommandNodeEntry::argument("dim", CommandArgType::SingleWord, true, vec![]),
        CommandNodeEntry::literal("op", false, vec![28]),
        CommandNodeEntry::argument("player", CommandArgType::SingleWord, true, vec![]),
        CommandNodeEntry::literal("deop", false, vec![30]),
        CommandNodeEntry::argument("player", CommandArgType::SingleWord, true, vec![]),
        CommandNodeEntry::literal("ban", false, vec![32]),
        CommandNodeEntry::argument("player", CommandArgType::GreedyPhrase, true, vec![]),
        CommandNodeEntry::literal("pardon", false, vec![34]),
        CommandNodeEntry::argument("player", CommandArgType::SingleWord, true, vec![]),
        CommandNodeEntry::literal("whitelist", false, vec![36]),
        CommandNodeEntry::argument("action", CommandArgType::GreedyPhrase, true, vec![]),
        CommandNodeEntry::literal("place", false, vec![38]),
        CommandNodeEntry::literal("template", false, vec![39]),
        CommandNodeEntry::argument("args", CommandArgType::GreedyPhrase, true, vec![]),
    ];
    encode_play_commands(&nodes, 0)
}

#[derive(Debug, Clone, PartialEq)]
pub enum CommandAction {
    Reply(String),
    SetGamemode(i32),
    Teleport(Vec3),
    SetBlock {
        pos: BlockPos,
        state: BlockStateId,
    },
    Give {
        item_id: u16,
        item_name: &'static str,
        count: u32,
    },
    SetTime(i64),
    QueryTime,
    SaveAll,
    ListPlayers,
    Seed,
    Biome,
    Summon {
        entity_type: String,
        pos: Vec3,
    },
    KillEntities,
    SwitchDimension(String),
    Op(String),
    Deop(String),
    Ban {
        player: String,
        reason: String,
    },
    Pardon(String),
    Kick {
        player: String,
        reason: String,
    },
    Whitelist {
        action: String,
        player: Option<String>,
    },
    PlaceTemplate {
        template_id: String,
        pos: BlockPos,
        rotation_deg: i32,
    },
    StopServer,
}

fn parse_coord(token: &str, origin: f64) -> Result<f64, String> {
    if let Some(rel) = token.strip_prefix('~') {
        if rel.is_empty() {
            Ok(origin)
        } else {
            let off: f64 = rel
                .parse()
                .map_err(|_| format!("Invalid relative coordinate '{token}'"))?;
            Ok(origin + off)
        }
    } else {
        token
            .parse()
            .map_err(|_| format!("Invalid coordinate '{token}'"))
    }
}

fn parse_block_coord(token: &str, origin: i32) -> Result<i32, String> {
    if let Some(rel) = token.strip_prefix('~') {
        if rel.is_empty() {
            Ok(origin)
        } else {
            let off: i32 = rel
                .parse()
                .map_err(|_| format!("Invalid relative block coordinate '{token}'"))?;
            Ok(origin + off)
        }
    } else {
        token
            .parse()
            .map_err(|_| format!("Invalid block coordinate '{token}'"))
    }
}

fn normalize_mc_id(raw: &str) -> String {
    if raw.contains(':') {
        raw.to_string()
    } else {
        format!("minecraft:{raw}")
    }
}

/// Parse a serverbound slash-command string (without leading `/`) relative to the sender's position.
pub fn parse_command(raw: &str, sender_pos: Vec3) -> CommandAction {
    let trimmed = raw.trim().trim_start_matches('/');
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.is_empty() {
        return CommandAction::Reply("Empty command.".into());
    }

    match parts[0].to_ascii_lowercase().as_str() {
        "help" => CommandAction::Reply(
            "Commands: /gamemode, /tp, /setblock, /give, /summon, /kill, /dimension <overworld|nether|end>, /op, /deop, /ban, /pardon, /kick, /whitelist, /time, /seed, /biome, /save-all, /list, /stop".into()
        ),
        "list" => CommandAction::ListPlayers,
        "seed" => CommandAction::Seed,
        "biome" => CommandAction::Biome,
        "kill" => CommandAction::KillEntities,
        "dimension" | "dim" => {
            if parts.len() < 2 {
                return CommandAction::Reply("Usage: /dimension <overworld|the_nether|the_end>".into());
            }
            let d = match parts[1].to_ascii_lowercase().as_str() {
                "overworld" | "minecraft:overworld" => "minecraft:overworld",
                "nether" | "the_nether" | "minecraft:the_nether" => "minecraft:the_nether",
                "end" | "the_end" | "minecraft:the_end" => "minecraft:the_end",
                other => return CommandAction::Reply(format!("Unknown dimension: {other}")),
            };
            CommandAction::SwitchDimension(d.into())
        }
        "op" => {
            if parts.len() < 2 {
                return CommandAction::Reply("Usage: /op <player>".into());
            }
            CommandAction::Op(parts[1].into())
        }
        "deop" => {
            if parts.len() < 2 {
                return CommandAction::Reply("Usage: /deop <player>".into());
            }
            CommandAction::Deop(parts[1].into())
        }
        "ban" => {
            if parts.len() < 2 {
                return CommandAction::Reply("Usage: /ban <player> [reason]".into());
            }
            let reason = if parts.len() > 2 {
                parts[2..].join(" ")
            } else {
                "Banned by an operator.".into()
            };
            CommandAction::Ban {
                player: parts[1].into(),
                reason,
            }
        }
        "pardon" => {
            if parts.len() < 2 {
                return CommandAction::Reply("Usage: /pardon <player>".into());
            }
            CommandAction::Pardon(parts[1].into())
        }
        "kick" => {
            if parts.len() < 2 {
                return CommandAction::Reply("Usage: /kick <player> [reason]".into());
            }
            let reason = if parts.len() > 2 {
                parts[2..].join(" ")
            } else {
                "Kicked by an operator.".into()
            };
            CommandAction::Kick {
                player: parts[1].into(),
                reason,
            }
        }
        "whitelist" => {
            if parts.len() < 2 {
                return CommandAction::Reply("Usage: /whitelist <on|off|list|add|remove> [player]".into());
            }
            CommandAction::Whitelist {
                action: parts[1].to_ascii_lowercase(),
                player: parts.get(2).map(|s| (*s).to_string()),
            }
        }
        "place" => {
            if parts.len() < 3 || !parts[1].eq_ignore_ascii_case("template") {
                return CommandAction::Reply(
                    "Usage: /place template <id> [x y z] [0|90|180|270]".into(),
                );
            }
            let template_id = parts[2].to_string();
            let ox = sender_pos.x.floor() as i32;
            let oy = sender_pos.y.floor() as i32;
            let oz = sender_pos.z.floor() as i32;
            let pos = if parts.len() >= 6 {
                match (
                    parse_block_coord(parts[3], ox),
                    parse_block_coord(parts[4], oy),
                    parse_block_coord(parts[5], oz),
                ) {
                    (Ok(x), Ok(y), Ok(z)) => BlockPos::new(x, y, z),
                    (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                        return CommandAction::Reply(e);
                    }
                }
            } else {
                BlockPos::new(ox, oy, oz)
            };
            let rotation_deg = parts
                .get(6)
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(0);
            CommandAction::PlaceTemplate {
                template_id,
                pos,
                rotation_deg,
            }
        }
        "summon" => {
            if parts.len() < 2 {
                return CommandAction::Reply("Usage: /summon <entity> [x y z]".into());
            }
            let entity_type = normalize_mc_id(parts[1]);
            let pos = if parts.len() >= 5 {
                match (
                    parse_coord(parts[2], sender_pos.x),
                    parse_coord(parts[3], sender_pos.y),
                    parse_coord(parts[4], sender_pos.z),
                ) {
                    (Ok(x), Ok(y), Ok(z)) => Vec3::new(x, y, z),
                    (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => {
                        return CommandAction::Reply(e);
                    }
                }
            } else {
                sender_pos
            };
            CommandAction::Summon { entity_type, pos }
        }
        "save-all" => CommandAction::SaveAll,
        "stop" => CommandAction::StopServer,
        "gamemode" | "gm" => {
            if parts.len() < 2 {
                return CommandAction::Reply("Usage: /gamemode <survival|creative|adventure|spectator>".into());
            }
            let gm = match parts[1].to_ascii_lowercase().as_str() {
                "survival" | "s" | "0" => 0,
                "creative" | "c" | "1" => 1,
                "adventure" | "a" | "2" => 2,
                "spectator" | "sp" | "3" => 3,
                other => return CommandAction::Reply(format!("Unknown gamemode: {other}")),
            };
            CommandAction::SetGamemode(gm)
        }
        "tp" | "teleport" => {
            if parts.len() < 4 {
                return CommandAction::Reply("Usage: /tp <x> <y> <z>".into());
            }
            match (
                parse_coord(parts[1], sender_pos.x),
                parse_coord(parts[2], sender_pos.y),
                parse_coord(parts[3], sender_pos.z),
            ) {
                (Ok(x), Ok(y), Ok(z)) => CommandAction::Teleport(Vec3::new(x, y, z)),
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => CommandAction::Reply(e),
            }
        }
        "setblock" => {
            if parts.len() < 5 {
                return CommandAction::Reply("Usage: /setblock <x> <y> <z> <block>".into());
            }
            let ox = sender_pos.x.floor() as i32;
            let oy = sender_pos.y.floor() as i32;
            let oz = sender_pos.z.floor() as i32;
            let pos = match (
                parse_block_coord(parts[1], ox),
                parse_block_coord(parts[2], oy),
                parse_block_coord(parts[3], oz),
            ) {
                (Ok(x), Ok(y), Ok(z)) => BlockPos::new(x, y, z),
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return CommandAction::Reply(e),
            };
            let block_id = normalize_mc_id(parts[4]);
            match default_state_id(&block_id) {
                Some(state) => CommandAction::SetBlock { pos, state },
                None => CommandAction::Reply(format!("Unknown block: {block_id}")),
            }
        }
        "give" => {
            if parts.len() < 2 {
                return CommandAction::Reply("Usage: /give <item> [count]".into());
            }
            // Support both `/give <item> [count]` and vanilla `/give <target> <item> [count]`
            let (item_arg, count_arg) = if parts.len() >= 3 && item_by_name(&normalize_mc_id(parts[1])).is_none() {
                (parts[2], parts.get(3).copied())
            } else {
                (parts[1], parts.get(2).copied())
            };
            let full_id = normalize_mc_id(item_arg);
            let Some(info) = item_by_name(&full_id) else {
                return CommandAction::Reply(format!("Unknown item: {full_id}"));
            };
            let count: u32 = match count_arg {
                None => 1,
                Some(s) => match s.parse::<u32>() {
                    Ok(c) if (1..=2560).contains(&c) => c,
                    _ => return CommandAction::Reply(format!("Invalid count: {s}")),
                },
            };
            CommandAction::Give {
                item_id: info.id,
                item_name: info.name,
                count,
            }
        }
        "time" => {
            if parts.len() < 2 {
                return CommandAction::Reply("Usage: /time <set <day|noon|night|midnight|ticks>|query>".into());
            }
            match parts[1].to_ascii_lowercase().as_str() {
                "query" => CommandAction::QueryTime,
                "set" => {
                    if parts.len() < 3 {
                        return CommandAction::Reply("Usage: /time set <day|noon|night|midnight|ticks>".into());
                    }
                    let ticks = match parts[2].to_ascii_lowercase().as_str() {
                        "day" => 1000,
                        "noon" => 6000,
                        "night" => 13000,
                        "midnight" => 18000,
                        other => match other.parse::<i64>() {
                            Ok(v) if v >= 0 => v,
                            _ => return CommandAction::Reply(format!("Invalid time value: {other}")),
                        },
                    };
                    CommandAction::SetTime(ticks)
                }
                other => CommandAction::Reply(format!("Unknown /time subcommand: {other}")),
            }
        }
        other => CommandAction::Reply(format!("Unknown command '/{other}'. Try /help")),
    }
}
