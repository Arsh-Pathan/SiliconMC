use crate::scoreboard::{
    parse_bossbar_color, parse_bossbar_overlay, parse_display_slot, parse_team_color,
};
use mc_data::items::item_by_name;
use mc_data::{BlockStateId, default_state_id};
use mc_math::{BlockPos, Vec3};

/// Vanilla default `max_block_modifications` limit for `/fill` and `/clone`.
pub const MAX_FILL_CLONE_BLOCKS: usize = 32_768;

/// Names of all extended vanilla commands parsed by `parse_extra_command`.
pub const EXTRA_COMMAND_NAMES: &[&str] = &[
    "fill",
    "clone",
    "gamerule",
    "scoreboard",
    "team",
    "bossbar",
    "title",
    "clear",
    "spawnpoint",
    "setworldspawn",
    "difficulty",
    "enchant",
    "advancement",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillMode {
    Replace { filter: Option<BlockStateId> },
    Hollow,
    Outline,
    Keep,
    Destroy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleSlot {
    Title,
    Subtitle,
    ActionBar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BossBarProperty {
    Name(String),
    Value(i32),
    Max(i32),
    Visible(bool),
    Color(i32),
    Overlay(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtraCommandAction {
    Reply(String),
    Fill {
        min: BlockPos,
        max: BlockPos,
        state: BlockStateId,
        mode: FillMode,
    },
    Clone {
        src_min: BlockPos,
        src_max: BlockPos,
        dst: BlockPos,
        masked: bool,
    },
    GameRuleList,
    GameRule {
        rule: String,
        value: Option<String>,
    },
    ScoreboardObjectivesList,
    ScoreboardObjectivesAdd {
        name: String,
        criteria: String,
        display_name: Option<String>,
    },
    ScoreboardObjectivesRemove {
        name: String,
    },
    ScoreboardObjectivesSetDisplay {
        slot: i32,
        objective: Option<String>,
    },
    ScoreboardPlayersSet {
        target: String,
        objective: String,
        score: i32,
    },
    ScoreboardPlayersAdd {
        target: String,
        objective: String,
        amount: i32,
    },
    ScoreboardPlayersRemove {
        target: String,
        objective: String,
        amount: i32,
    },
    ScoreboardPlayersReset {
        target: String,
        objective: Option<String>,
    },
    ScoreboardPlayersGet {
        target: String,
        objective: String,
    },
    TeamList,
    TeamAdd {
        name: String,
        display_name: Option<String>,
    },
    TeamRemove {
        name: String,
    },
    TeamJoin {
        team: String,
        member: String,
    },
    TeamLeave {
        member: String,
    },
    TeamModifyColor {
        team: String,
        color: i32,
    },
    BossBarList,
    BossBarAdd {
        id: String,
        name: String,
    },
    BossBarRemove {
        id: String,
    },
    BossBarSet {
        id: String,
        property: BossBarProperty,
    },
    TitleText {
        target: Option<String>,
        slot: TitleSlot,
        text: String,
    },
    TitleClear {
        target: Option<String>,
        reset: bool,
    },
    TitleTimes {
        target: Option<String>,
        fade_in: i32,
        stay: i32,
        fade_out: i32,
    },
    ClearInventory {
        target: Option<String>,
        item_id: Option<u16>,
        max_count: Option<u32>,
    },
    SetSpawnpoint {
        target: Option<String>,
        pos: BlockPos,
    },
    SetWorldSpawn {
        pos: BlockPos,
    },
    QueryDifficulty,
    SetDifficulty(i32),
    Enchant {
        target: Option<String>,
        enchantment: String,
        level: i32,
    },
    Advancement {
        grant: bool,
        target: Option<String>,
        /// `None` represents `everything`.
        advancement_id: Option<String>,
    },
}

fn normalize_mc_id(raw: &str) -> String {
    if raw.contains(':') {
        raw.to_string()
    } else {
        format!("minecraft:{raw}")
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
            Ok(origin.saturating_add(off))
        }
    } else {
        token
            .parse()
            .map_err(|_| format!("Invalid block coordinate '{token}'"))
    }
}

fn parse_block_pos(tokens: &[&str], ox: i32, oy: i32, oz: i32) -> Result<BlockPos, String> {
    if tokens.len() < 3 {
        return Err("Expected 3 block coordinates <x> <y> <z>".into());
    }
    let x = parse_block_coord(tokens[0], ox)?;
    let y = parse_block_coord(tokens[1], oy)?;
    let z = parse_block_coord(tokens[2], oz)?;
    Ok(BlockPos::new(x, y, z))
}

fn normalize_box(a: BlockPos, b: BlockPos) -> (BlockPos, BlockPos) {
    (
        BlockPos::new(a.x.min(b.x), a.y.min(b.y), a.z.min(b.z)),
        BlockPos::new(a.x.max(b.x), a.y.max(b.y), a.z.max(b.z)),
    )
}

fn box_volume(min: BlockPos, max: BlockPos) -> u128 {
    let dx = (max.x as i64 - min.x as i64 + 1).max(0) as u128;
    let dy = (max.y as i64 - min.y as i64 + 1).max(0) as u128;
    let dz = (max.z as i64 - min.z as i64 + 1).max(0) as u128;
    dx.saturating_mul(dy).saturating_mul(dz)
}

/// Extract plain display text from a command text argument, unwrapping either JSON
/// `{"text":"..."}` or surrounding `"..."` quotes when present.
fn unwrap_text_arg(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
            if let Some(text) = val.get("text").and_then(|v| v.as_str()) {
                return text.to_string();
            }
        }
    }
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        return trimmed[1..trimmed.len() - 1].to_string();
    }
    trimmed.to_string()
}

/// Parse an extended vanilla slash-command relative to `sender_pos`.
/// Returns `None` if the command verb is not handled by this module.
pub fn parse_extra_command(raw: &str, sender_pos: Vec3) -> Option<ExtraCommandAction> {
    let trimmed = raw.trim().trim_start_matches('/');
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    let ox = sender_pos.x.floor() as i32;
    let oy = sender_pos.y.floor() as i32;
    let oz = sender_pos.z.floor() as i32;

    match parts[0].to_ascii_lowercase().as_str() {
        "fill" => Some(parse_fill(&parts[1..], ox, oy, oz)),
        "clone" => Some(parse_clone(&parts[1..], ox, oy, oz)),
        "gamerule" => Some(parse_gamerule(&parts[1..])),
        "scoreboard" => Some(parse_scoreboard(&parts[1..])),
        "team" => Some(parse_team(&parts[1..])),
        "bossbar" => Some(parse_bossbar(&parts[1..])),
        "title" => Some(parse_title(&parts[1..])),
        "clear" => Some(parse_clear(&parts[1..])),
        "spawnpoint" => Some(parse_spawnpoint(&parts[1..], ox, oy, oz)),
        "setworldspawn" => Some(parse_setworldspawn(&parts[1..], ox, oy, oz)),
        "difficulty" => Some(parse_difficulty(&parts[1..])),
        "enchant" => Some(parse_enchant(&parts[1..])),
        "advancement" => Some(parse_advancement(&parts[1..])),
        _ => None,
    }
}

fn parse_fill(args: &[&str], ox: i32, oy: i32, oz: i32) -> ExtraCommandAction {
    if args.len() < 7 {
        return ExtraCommandAction::Reply(
            "Usage: /fill <x1> <y1> <z1> <x2> <y2> <z2> <block> [replace|hollow|outline|keep]"
                .into(),
        );
    }
    let p1 = match parse_block_pos(&args[0..3], ox, oy, oz) {
        Ok(p) => p,
        Err(e) => return ExtraCommandAction::Reply(e),
    };
    let p2 = match parse_block_pos(&args[3..6], ox, oy, oz) {
        Ok(p) => p,
        Err(e) => return ExtraCommandAction::Reply(e),
    };
    let (min, max) = normalize_box(p1, p2);
    let volume = box_volume(min, max);
    if volume > MAX_FILL_CLONE_BLOCKS as u128 {
        return ExtraCommandAction::Reply(format!(
            "Too many blocks in the specified area (maximum {MAX_FILL_CLONE_BLOCKS}, specified {volume})"
        ));
    }

    let block_id = normalize_mc_id(args[6]);
    let Some(state) = default_state_id(&block_id) else {
        return ExtraCommandAction::Reply(format!("Unknown block: {block_id}"));
    };

    let mode = match args.get(7).map(|s| s.to_ascii_lowercase()).as_deref() {
        None => FillMode::Replace { filter: None },
        Some("replace") => {
            if let Some(&filter_tok) = args.get(8) {
                let filter_id = normalize_mc_id(filter_tok);
                match default_state_id(&filter_id) {
                    Some(f_state) => FillMode::Replace {
                        filter: Some(f_state),
                    },
                    None => {
                        return ExtraCommandAction::Reply(format!(
                            "Unknown filter block: {filter_id}"
                        ));
                    }
                }
            } else {
                FillMode::Replace { filter: None }
            }
        }
        Some("hollow") => FillMode::Hollow,
        Some("outline") => FillMode::Outline,
        Some("keep") => FillMode::Keep,
        Some("destroy") => FillMode::Destroy,
        Some(other) => {
            return ExtraCommandAction::Reply(format!(
                "Unknown /fill mode '{other}' (expected replace, hollow, outline, keep, or destroy)"
            ));
        }
    };

    ExtraCommandAction::Fill {
        min,
        max,
        state,
        mode,
    }
}

fn parse_clone(args: &[&str], ox: i32, oy: i32, oz: i32) -> ExtraCommandAction {
    if args.len() < 9 {
        return ExtraCommandAction::Reply(
            "Usage: /clone <x1> <y1> <z1> <x2> <y2> <z2> <dx> <dy> <dz> [replace|masked]".into(),
        );
    }
    let p1 = match parse_block_pos(&args[0..3], ox, oy, oz) {
        Ok(p) => p,
        Err(e) => return ExtraCommandAction::Reply(e),
    };
    let p2 = match parse_block_pos(&args[3..6], ox, oy, oz) {
        Ok(p) => p,
        Err(e) => return ExtraCommandAction::Reply(e),
    };
    let dst = match parse_block_pos(&args[6..9], ox, oy, oz) {
        Ok(p) => p,
        Err(e) => return ExtraCommandAction::Reply(e),
    };
    let (src_min, src_max) = normalize_box(p1, p2);
    let volume = box_volume(src_min, src_max);
    if volume > MAX_FILL_CLONE_BLOCKS as u128 {
        return ExtraCommandAction::Reply(format!(
            "Too many blocks in the specified area (maximum {MAX_FILL_CLONE_BLOCKS}, specified {volume})"
        ));
    }

    let masked = match args.get(9).map(|s| s.to_ascii_lowercase()).as_deref() {
        None | Some("replace") => false,
        Some("masked") => true,
        Some(other) => {
            return ExtraCommandAction::Reply(format!(
                "Unknown /clone mode '{other}' (expected replace or masked)"
            ));
        }
    };

    ExtraCommandAction::Clone {
        src_min,
        src_max,
        dst,
        masked,
    }
}

fn parse_gamerule(args: &[&str]) -> ExtraCommandAction {
    if args.is_empty() {
        return ExtraCommandAction::GameRuleList;
    }
    ExtraCommandAction::GameRule {
        rule: args[0].to_string(),
        value: args.get(1).map(|v| (*v).to_string()),
    }
}

fn parse_scoreboard(args: &[&str]) -> ExtraCommandAction {
    if args.is_empty() {
        return ExtraCommandAction::Reply(
            "Usage: /scoreboard <objectives|players> ...".into(),
        );
    }
    match args[0].to_ascii_lowercase().as_str() {
        "objectives" => parse_scoreboard_objectives(&args[1..]),
        "players" => parse_scoreboard_players(&args[1..]),
        other => ExtraCommandAction::Reply(format!("Unknown /scoreboard subcommand '{other}'")),
    }
}

fn parse_scoreboard_objectives(args: &[&str]) -> ExtraCommandAction {
    if args.is_empty() {
        return ExtraCommandAction::Reply(
            "Usage: /scoreboard objectives <add|remove|list|setdisplay> ...".into(),
        );
    }
    match args[0].to_ascii_lowercase().as_str() {
        "list" => ExtraCommandAction::ScoreboardObjectivesList,
        "add" => {
            if args.len() < 3 {
                return ExtraCommandAction::Reply(
                    "Usage: /scoreboard objectives add <name> <criteria> [displayName]".into(),
                );
            }
            let display_name = if args.len() > 3 {
                Some(unwrap_text_arg(&args[3..].join(" ")))
            } else {
                None
            };
            ExtraCommandAction::ScoreboardObjectivesAdd {
                name: args[1].to_string(),
                criteria: args[2].to_string(),
                display_name,
            }
        }
        "remove" => {
            if args.len() < 2 {
                return ExtraCommandAction::Reply(
                    "Usage: /scoreboard objectives remove <name>".into(),
                );
            }
            ExtraCommandAction::ScoreboardObjectivesRemove {
                name: args[1].to_string(),
            }
        }
        "setdisplay" => {
            if args.len() < 2 {
                return ExtraCommandAction::Reply(
                    "Usage: /scoreboard objectives setdisplay <list|sidebar|below_name> [objective]"
                        .into(),
                );
            }
            let Some(slot) = parse_display_slot(args[1]) else {
                return ExtraCommandAction::Reply(format!(
                    "Unknown display slot '{}'",
                    args[1]
                ));
            };
            ExtraCommandAction::ScoreboardObjectivesSetDisplay {
                slot,
                objective: args.get(2).map(|s| (*s).to_string()),
            }
        }
        other => ExtraCommandAction::Reply(format!(
            "Unknown /scoreboard objectives action '{other}'"
        )),
    }
}

fn parse_scoreboard_players(args: &[&str]) -> ExtraCommandAction {
    if args.is_empty() {
        return ExtraCommandAction::Reply(
            "Usage: /scoreboard players <set|add|remove|reset|get> ...".into(),
        );
    }
    match args[0].to_ascii_lowercase().as_str() {
        "set" => {
            if args.len() < 4 {
                return ExtraCommandAction::Reply(
                    "Usage: /scoreboard players set <target> <objective> <score>".into(),
                );
            }
            let Ok(score) = args[3].parse::<i32>() else {
                return ExtraCommandAction::Reply(format!("Invalid score: {}", args[3]));
            };
            ExtraCommandAction::ScoreboardPlayersSet {
                target: args[1].to_string(),
                objective: args[2].to_string(),
                score,
            }
        }
        "add" => {
            if args.len() < 4 {
                return ExtraCommandAction::Reply(
                    "Usage: /scoreboard players add <target> <objective> <amount>".into(),
                );
            }
            let Ok(amount) = args[3].parse::<i32>() else {
                return ExtraCommandAction::Reply(format!("Invalid score amount: {}", args[3]));
            };
            ExtraCommandAction::ScoreboardPlayersAdd {
                target: args[1].to_string(),
                objective: args[2].to_string(),
                amount,
            }
        }
        "remove" => {
            if args.len() < 4 {
                return ExtraCommandAction::Reply(
                    "Usage: /scoreboard players remove <target> <objective> <amount>".into(),
                );
            }
            let Ok(amount) = args[3].parse::<i32>() else {
                return ExtraCommandAction::Reply(format!("Invalid score amount: {}", args[3]));
            };
            ExtraCommandAction::ScoreboardPlayersRemove {
                target: args[1].to_string(),
                objective: args[2].to_string(),
                amount,
            }
        }
        "reset" => {
            if args.len() < 2 {
                return ExtraCommandAction::Reply(
                    "Usage: /scoreboard players reset <target> [objective]".into(),
                );
            }
            ExtraCommandAction::ScoreboardPlayersReset {
                target: args[1].to_string(),
                objective: args.get(2).map(|s| (*s).to_string()),
            }
        }
        "get" => {
            if args.len() < 3 {
                return ExtraCommandAction::Reply(
                    "Usage: /scoreboard players get <target> <objective>".into(),
                );
            }
            ExtraCommandAction::ScoreboardPlayersGet {
                target: args[1].to_string(),
                objective: args[2].to_string(),
            }
        }
        other => ExtraCommandAction::Reply(format!(
            "Unknown /scoreboard players action '{other}'"
        )),
    }
}

fn parse_team(args: &[&str]) -> ExtraCommandAction {
    if args.is_empty() {
        return ExtraCommandAction::Reply(
            "Usage: /team <list|add|remove|join|leave|modify> ...".into(),
        );
    }
    match args[0].to_ascii_lowercase().as_str() {
        "list" => ExtraCommandAction::TeamList,
        "add" => {
            if args.len() < 2 {
                return ExtraCommandAction::Reply("Usage: /team add <name> [displayName]".into());
            }
            let display_name = if args.len() > 2 {
                Some(unwrap_text_arg(&args[2..].join(" ")))
            } else {
                None
            };
            ExtraCommandAction::TeamAdd {
                name: args[1].to_string(),
                display_name,
            }
        }
        "remove" => {
            if args.len() < 2 {
                return ExtraCommandAction::Reply("Usage: /team remove <name>".into());
            }
            ExtraCommandAction::TeamRemove {
                name: args[1].to_string(),
            }
        }
        "join" => {
            if args.len() < 3 {
                return ExtraCommandAction::Reply("Usage: /team join <team> <member>".into());
            }
            ExtraCommandAction::TeamJoin {
                team: args[1].to_string(),
                member: args[2].to_string(),
            }
        }
        "leave" => {
            if args.len() < 2 {
                return ExtraCommandAction::Reply("Usage: /team leave <member>".into());
            }
            ExtraCommandAction::TeamLeave {
                member: args[1].to_string(),
            }
        }
        "modify" => {
            if args.len() < 4 || !args[2].eq_ignore_ascii_case("color") {
                return ExtraCommandAction::Reply("Usage: /team modify <team> color <color>".into());
            }
            let Some(color) = parse_team_color(args[3]) else {
                return ExtraCommandAction::Reply(format!("Unknown team color: {}", args[3]));
            };
            ExtraCommandAction::TeamModifyColor {
                team: args[1].to_string(),
                color,
            }
        }
        other => ExtraCommandAction::Reply(format!("Unknown /team action '{other}'")),
    }
}

fn parse_bossbar(args: &[&str]) -> ExtraCommandAction {
    if args.is_empty() {
        return ExtraCommandAction::Reply("Usage: /bossbar <add|remove|set|list> ...".into());
    }
    match args[0].to_ascii_lowercase().as_str() {
        "list" => ExtraCommandAction::BossBarList,
        "add" => {
            if args.len() < 3 {
                return ExtraCommandAction::Reply("Usage: /bossbar add <id> <name>".into());
            }
            ExtraCommandAction::BossBarAdd {
                id: normalize_mc_id(args[1]),
                name: unwrap_text_arg(&args[2..].join(" ")),
            }
        }
        "remove" => {
            if args.len() < 2 {
                return ExtraCommandAction::Reply("Usage: /bossbar remove <id>".into());
            }
            ExtraCommandAction::BossBarRemove {
                id: normalize_mc_id(args[1]),
            }
        }
        "set" => {
            if args.len() < 4 {
                return ExtraCommandAction::Reply(
                    "Usage: /bossbar set <id> <name|value|max|visible|color|style> <value>".into(),
                );
            }
            let id = normalize_mc_id(args[1]);
            let property = match args[2].to_ascii_lowercase().as_str() {
                "name" => BossBarProperty::Name(unwrap_text_arg(&args[3..].join(" "))),
                "value" => match args[3].parse::<i32>() {
                    Ok(v) if v >= 0 => BossBarProperty::Value(v),
                    _ => return ExtraCommandAction::Reply(format!("Invalid bossbar value: {}", args[3])),
                },
                "max" => match args[3].parse::<i32>() {
                    Ok(m) if m >= 1 => BossBarProperty::Max(m),
                    _ => return ExtraCommandAction::Reply(format!("Invalid bossbar max: {}", args[3])),
                },
                "visible" => match args[3].to_ascii_lowercase().as_str() {
                    "true" => BossBarProperty::Visible(true),
                    "false" => BossBarProperty::Visible(false),
                    _ => {
                        return ExtraCommandAction::Reply(format!(
                            "Invalid boolean for visible: {}",
                            args[3]
                        ));
                    }
                },
                "color" => match parse_bossbar_color(args[3]) {
                    Some(c) => BossBarProperty::Color(c),
                    None => {
                        return ExtraCommandAction::Reply(format!(
                            "Unknown bossbar color: {}",
                            args[3]
                        ));
                    }
                },
                "style" | "overlay" => match parse_bossbar_overlay(args[3]) {
                    Some(o) => BossBarProperty::Overlay(o),
                    None => {
                        return ExtraCommandAction::Reply(format!(
                            "Unknown bossbar style: {}",
                            args[3]
                        ));
                    }
                },
                other => {
                    return ExtraCommandAction::Reply(format!(
                        "Unknown /bossbar set property '{other}'"
                    ));
                }
            };
            ExtraCommandAction::BossBarSet { id, property }
        }
        other => ExtraCommandAction::Reply(format!("Unknown /bossbar action '{other}'")),
    }
}

fn parse_title(args: &[&str]) -> ExtraCommandAction {
    if args.is_empty() {
        return ExtraCommandAction::Reply(
            "Usage: /title [target] <title|subtitle|actionbar|clear|reset|times> ...".into(),
        );
    }

    let first_lower = args[0].to_ascii_lowercase();
    let (target, sub_idx) = match first_lower.as_str() {
        "title" | "subtitle" | "actionbar" | "clear" | "reset" | "times" => (None, 0usize),
        _ => (Some(args[0].to_string()), 1usize),
    };

    let Some(&action) = args.get(sub_idx) else {
        return ExtraCommandAction::Reply(
            "Usage: /title [target] <title|subtitle|actionbar|clear|reset|times> ...".into(),
        );
    };

    match action.to_ascii_lowercase().as_str() {
        "clear" => ExtraCommandAction::TitleClear {
            target,
            reset: false,
        },
        "reset" => ExtraCommandAction::TitleClear {
            target,
            reset: true,
        },
        "title" | "subtitle" | "actionbar" => {
            if args.len() <= sub_idx + 1 {
                return ExtraCommandAction::Reply(format!(
                    "Usage: /title [target] {action} <text>"
                ));
            }
            let slot = match action.to_ascii_lowercase().as_str() {
                "title" => TitleSlot::Title,
                "subtitle" => TitleSlot::Subtitle,
                _ => TitleSlot::ActionBar,
            };
            let text = unwrap_text_arg(&args[sub_idx + 1..].join(" "));
            ExtraCommandAction::TitleText { target, slot, text }
        }
        "times" => {
            if args.len() < sub_idx + 4 {
                return ExtraCommandAction::Reply(
                    "Usage: /title [target] times <fadeIn> <stay> <fadeOut>".into(),
                );
            }
            let (Ok(fade_in), Ok(stay), Ok(fade_out)) = (
                args[sub_idx + 1].parse::<i32>(),
                args[sub_idx + 2].parse::<i32>(),
                args[sub_idx + 3].parse::<i32>(),
            ) else {
                return ExtraCommandAction::Reply(
                    "Invalid /title times arguments (expected three integers)".into(),
                );
            };
            ExtraCommandAction::TitleTimes {
                target,
                fade_in: fade_in.max(0),
                stay: stay.max(0),
                fade_out: fade_out.max(0),
            }
        }
        other => ExtraCommandAction::Reply(format!("Unknown /title action '{other}'")),
    }
}

fn parse_clear(args: &[&str]) -> ExtraCommandAction {
    if args.is_empty() {
        return ExtraCommandAction::ClearInventory {
            target: None,
            item_id: None,
            max_count: None,
        };
    }

    // Support both `/clear [item] [maxCount]` and vanilla `/clear <target> [item] [maxCount]`.
    let has_target = args[0].starts_with('@')
        || (args.len() >= 2
            && item_by_name(&normalize_mc_id(args[0])).is_none()
            && args[1].parse::<u32>().is_err());

    let (target, item_idx) = if has_target {
        (Some(args[0].to_string()), 1usize)
    } else {
        (None, 0usize)
    };

    let item_id = if let Some(&item_tok) = args.get(item_idx) {
        let full = normalize_mc_id(item_tok);
        let Some(info) = item_by_name(&full) else {
            return ExtraCommandAction::Reply(format!("Unknown item: {full}"));
        };
        Some(info.id)
    } else {
        None
    };

    let max_count = if let Some(&cnt_tok) = args.get(item_idx + 1) {
        match cnt_tok.parse::<u32>() {
            Ok(c) => Some(c),
            Err(_) => {
                return ExtraCommandAction::Reply(format!("Invalid max count: {cnt_tok}"));
            }
        }
    } else {
        None
    };

    ExtraCommandAction::ClearInventory {
        target,
        item_id,
        max_count,
    }
}

fn parse_spawnpoint(args: &[&str], ox: i32, oy: i32, oz: i32) -> ExtraCommandAction {
    match args.len() {
        0 => ExtraCommandAction::SetSpawnpoint {
            target: None,
            pos: BlockPos::new(ox, oy, oz),
        },
        1 => ExtraCommandAction::SetSpawnpoint {
            target: Some(args[0].to_string()),
            pos: BlockPos::new(ox, oy, oz),
        },
        3 => match parse_block_pos(&args[0..3], ox, oy, oz) {
            Ok(pos) => ExtraCommandAction::SetSpawnpoint { target: None, pos },
            Err(e) => ExtraCommandAction::Reply(e),
        },
        len if len >= 4 => match parse_block_pos(&args[1..4], ox, oy, oz) {
            Ok(pos) => ExtraCommandAction::SetSpawnpoint {
                target: Some(args[0].to_string()),
                pos,
            },
            Err(e) => ExtraCommandAction::Reply(e),
        },
        _ => ExtraCommandAction::Reply("Usage: /spawnpoint [target] [x y z]".into()),
    }
}

fn parse_setworldspawn(args: &[&str], ox: i32, oy: i32, oz: i32) -> ExtraCommandAction {
    if args.is_empty() {
        return ExtraCommandAction::SetWorldSpawn {
            pos: BlockPos::new(ox, oy, oz),
        };
    }
    if args.len() < 3 {
        return ExtraCommandAction::Reply("Usage: /setworldspawn [x y z]".into());
    }
    match parse_block_pos(&args[0..3], ox, oy, oz) {
        Ok(pos) => ExtraCommandAction::SetWorldSpawn { pos },
        Err(e) => ExtraCommandAction::Reply(e),
    }
}

fn parse_difficulty(args: &[&str]) -> ExtraCommandAction {
    if args.is_empty() {
        return ExtraCommandAction::QueryDifficulty;
    }
    match args[0].to_ascii_lowercase().as_str() {
        "peaceful" | "p" | "0" => ExtraCommandAction::SetDifficulty(0),
        "easy" | "e" | "1" => ExtraCommandAction::SetDifficulty(1),
        "normal" | "n" | "2" => ExtraCommandAction::SetDifficulty(2),
        "hard" | "h" | "3" => ExtraCommandAction::SetDifficulty(3),
        other => ExtraCommandAction::Reply(format!(
            "Unknown difficulty '{other}' (expected peaceful, easy, normal, or hard)"
        )),
    }
}

fn parse_enchant(args: &[&str]) -> ExtraCommandAction {
    if args.is_empty() {
        return ExtraCommandAction::Reply(
            "Usage: /enchant [target] <enchantment> [level]".into(),
        );
    }

    // Support both `/enchant <enchantment> [level]` and vanilla `/enchant <target> <enchantment> [level]`.
    let has_target = args[0].starts_with('@')
        || (args.len() >= 2 && args[1].parse::<i32>().is_err());

    let (target, ench_idx, level_idx) = if has_target {
        if args.len() < 2 {
            return ExtraCommandAction::Reply(
                "Usage: /enchant [target] <enchantment> [level]".into(),
            );
        }
        (Some(args[0].to_string()), 1usize, 2usize)
    } else {
        (None, 0usize, 1usize)
    };

    let enchantment = normalize_mc_id(args[ench_idx]);
    let level = match args.get(level_idx) {
        None => 1,
        Some(&tok) => match tok.parse::<i32>() {
            Ok(lvl) if (1..=255).contains(&lvl) => lvl,
            _ => {
                return ExtraCommandAction::Reply(format!(
                    "Invalid enchantment level '{tok}' (expected 1..=255)"
                ));
            }
        },
    };

    ExtraCommandAction::Enchant {
        target,
        enchantment,
        level,
    }
}

fn parse_advancement(args: &[&str]) -> ExtraCommandAction {
    if args.len() < 2 {
        return ExtraCommandAction::Reply(
            "Usage: /advancement <grant|revoke> [target] <id|everything>".into(),
        );
    }
    let grant = match args[0].to_ascii_lowercase().as_str() {
        "grant" => true,
        "revoke" => false,
        other => {
            return ExtraCommandAction::Reply(format!(
                "Unknown /advancement action '{other}' (expected grant or revoke)"
            ));
        }
    };

    // Short form: `/advancement <grant|revoke> <everything|id>`
    if args.len() == 2 {
        let advancement_id = if args[1].eq_ignore_ascii_case("everything") {
            None
        } else {
            Some(normalize_mc_id(args[1]))
        };
        return ExtraCommandAction::Advancement {
            grant,
            target: None,
            advancement_id,
        };
    }

    // Vanilla form: `/advancement <grant|revoke> <target> <everything | only|from|until|through <id>>`
    let target = Some(args[1].to_string());
    match args[2].to_ascii_lowercase().as_str() {
        "everything" => ExtraCommandAction::Advancement {
            grant,
            target,
            advancement_id: None,
        },
        "only" | "from" | "until" | "through" => {
            let Some(&id_tok) = args.get(3) else {
                return ExtraCommandAction::Reply(
                    "Usage: /advancement <grant|revoke> <target> only <advancement_id>".into(),
                );
            };
            ExtraCommandAction::Advancement {
                grant,
                target,
                advancement_id: Some(normalize_mc_id(id_tok)),
            }
        }
        other => ExtraCommandAction::Advancement {
            grant,
            target,
            advancement_id: Some(normalize_mc_id(other)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoreboard::{
        BOSSBAR_COLOR_RED, BOSSBAR_OVERLAY_NOTCHED_10, DISPLAY_SLOT_SIDEBAR,
    };

    fn origin() -> Vec3 {
        Vec3::new(10.5, 64.0, -5.2)
    }

    #[test]
    fn unrecognized_command_returns_none() {
        assert_eq!(parse_extra_command("/summon zombie", origin()), None);
        assert_eq!(parse_extra_command("/gamemode creative", origin()), None);
        assert_eq!(parse_extra_command("   ", origin()), None);
    }

    #[test]
    fn parses_fill_and_enforces_32768_limit() {
        let act = parse_extra_command("/fill ~ ~ ~ ~3 ~2 ~1 stone hollow", origin()).unwrap();
        assert_eq!(
            act,
            ExtraCommandAction::Fill {
                min: BlockPos::new(10, 64, -6),
                max: BlockPos::new(13, 66, -5),
                state: default_state_id("minecraft:stone").unwrap(),
                mode: FillMode::Hollow,
            }
        );

        let keep = parse_extra_command("/fill 0 0 0 1 1 1 minecraft:dirt keep", origin()).unwrap();
        assert!(matches!(keep, ExtraCommandAction::Fill { mode: FillMode::Keep, .. }));

        let outline =
            parse_extra_command("/fill 0 0 0 2 2 2 oak_planks outline", origin()).unwrap();
        assert!(matches!(outline, ExtraCommandAction::Fill { mode: FillMode::Outline, .. }));

        // 33 * 33 * 33 = 35937 > 32768 -> Reply error
        let too_big = parse_extra_command("/fill 0 0 0 32 32 32 stone", origin()).unwrap();
        assert!(matches!(too_big, ExtraCommandAction::Reply(msg) if msg.contains("32768")));
    }

    #[test]
    fn parses_clone_with_relative_coords_and_limit() {
        let act = parse_extra_command("/clone 0 60 0 2 62 2 ~5 ~ ~-1 masked", origin()).unwrap();
        assert_eq!(
            act,
            ExtraCommandAction::Clone {
                src_min: BlockPos::new(0, 60, 0),
                src_max: BlockPos::new(2, 62, 2),
                dst: BlockPos::new(15, 64, -7),
                masked: true,
            }
        );

        let too_big = parse_extra_command("/clone 0 0 0 40 40 40 100 0 0", origin()).unwrap();
        assert!(matches!(too_big, ExtraCommandAction::Reply(msg) if msg.contains("32768")));
    }

    #[test]
    fn parses_gamerule_and_scoreboard_commands() {
        assert_eq!(
            parse_extra_command("/gamerule keepInventory true", origin()),
            Some(ExtraCommandAction::GameRule {
                rule: "keepInventory".into(),
                value: Some("true".into()),
            })
        );
        assert_eq!(
            parse_extra_command("/gamerule random_tick_speed", origin()),
            Some(ExtraCommandAction::GameRule {
                rule: "random_tick_speed".into(),
                value: None,
            })
        );
        assert_eq!(
            parse_extra_command("/gamerule", origin()),
            Some(ExtraCommandAction::GameRuleList)
        );

        assert_eq!(
            parse_extra_command("/scoreboard objectives add deaths deathCount \"Total Deaths\"", origin()),
            Some(ExtraCommandAction::ScoreboardObjectivesAdd {
                name: "deaths".into(),
                criteria: "deathCount".into(),
                display_name: Some("Total Deaths".into()),
            })
        );
        assert_eq!(
            parse_extra_command("/scoreboard objectives setdisplay sidebar deaths", origin()),
            Some(ExtraCommandAction::ScoreboardObjectivesSetDisplay {
                slot: DISPLAY_SLOT_SIDEBAR,
                objective: Some("deaths".into()),
            })
        );
        assert_eq!(
            parse_extra_command("/scoreboard objectives remove deaths", origin()),
            Some(ExtraCommandAction::ScoreboardObjectivesRemove {
                name: "deaths".into(),
            })
        );
        assert_eq!(
            parse_extra_command("/scoreboard objectives list", origin()),
            Some(ExtraCommandAction::ScoreboardObjectivesList)
        );
        assert_eq!(
            parse_extra_command("/scoreboard players set Steve deaths 5", origin()),
            Some(ExtraCommandAction::ScoreboardPlayersSet {
                target: "Steve".into(),
                objective: "deaths".into(),
                score: 5,
            })
        );
        assert_eq!(
            parse_extra_command("/scoreboard players add Steve deaths 2", origin()),
            Some(ExtraCommandAction::ScoreboardPlayersAdd {
                target: "Steve".into(),
                objective: "deaths".into(),
                amount: 2,
            })
        );
        assert_eq!(
            parse_extra_command("/scoreboard players remove Steve deaths 1", origin()),
            Some(ExtraCommandAction::ScoreboardPlayersRemove {
                target: "Steve".into(),
                objective: "deaths".into(),
                amount: 1,
            })
        );
        assert_eq!(
            parse_extra_command("/scoreboard players reset Steve", origin()),
            Some(ExtraCommandAction::ScoreboardPlayersReset {
                target: "Steve".into(),
                objective: None,
            })
        );
        assert_eq!(
            parse_extra_command("/scoreboard players get Steve deaths", origin()),
            Some(ExtraCommandAction::ScoreboardPlayersGet {
                target: "Steve".into(),
                objective: "deaths".into(),
            })
        );
    }

    #[test]
    fn parses_bossbar_and_title_commands() {
        assert_eq!(
            parse_extra_command("/bossbar add boss1 \"Final Boss\"", origin()),
            Some(ExtraCommandAction::BossBarAdd {
                id: "minecraft:boss1".into(),
                name: "Final Boss".into(),
            })
        );
        assert_eq!(
            parse_extra_command("/bossbar set boss1 color red", origin()),
            Some(ExtraCommandAction::BossBarSet {
                id: "minecraft:boss1".into(),
                property: BossBarProperty::Color(BOSSBAR_COLOR_RED),
            })
        );
        assert_eq!(
            parse_extra_command("/bossbar set boss1 style notched_10", origin()),
            Some(ExtraCommandAction::BossBarSet {
                id: "minecraft:boss1".into(),
                property: BossBarProperty::Overlay(BOSSBAR_OVERLAY_NOTCHED_10),
            })
        );
        assert_eq!(
            parse_extra_command("/bossbar set boss1 value 42", origin()),
            Some(ExtraCommandAction::BossBarSet {
                id: "minecraft:boss1".into(),
                property: BossBarProperty::Value(42),
            })
        );
        assert_eq!(
            parse_extra_command("/bossbar set boss1 visible false", origin()),
            Some(ExtraCommandAction::BossBarSet {
                id: "minecraft:boss1".into(),
                property: BossBarProperty::Visible(false),
            })
        );
        assert_eq!(
            parse_extra_command("/bossbar remove boss1", origin()),
            Some(ExtraCommandAction::BossBarRemove {
                id: "minecraft:boss1".into(),
            })
        );
        assert_eq!(
            parse_extra_command("/bossbar list", origin()),
            Some(ExtraCommandAction::BossBarList)
        );

        assert_eq!(
            parse_extra_command("/title title Welcome Home", origin()),
            Some(ExtraCommandAction::TitleText {
                target: None,
                slot: TitleSlot::Title,
                text: "Welcome Home".into(),
            })
        );
        assert_eq!(
            parse_extra_command("/title @a actionbar {\"text\":\"Low HP!\"}", origin()),
            Some(ExtraCommandAction::TitleText {
                target: Some("@a".into()),
                slot: TitleSlot::ActionBar,
                text: "Low HP!".into(),
            })
        );
        assert_eq!(
            parse_extra_command("/title times 10 70 20", origin()),
            Some(ExtraCommandAction::TitleTimes {
                target: None,
                fade_in: 10,
                stay: 70,
                fade_out: 20,
            })
        );
        assert_eq!(
            parse_extra_command("/title clear", origin()),
            Some(ExtraCommandAction::TitleClear {
                target: None,
                reset: false,
            })
        );
    }

    #[test]
    fn parses_clear_spawn_difficulty_enchant_and_advancement() {
        assert_eq!(
            parse_extra_command("/clear", origin()),
            Some(ExtraCommandAction::ClearInventory {
                target: None,
                item_id: None,
                max_count: None,
            })
        );
        let stone_id = item_by_name("minecraft:stone").unwrap().id;
        assert_eq!(
            parse_extra_command("/clear stone", origin()),
            Some(ExtraCommandAction::ClearInventory {
                target: None,
                item_id: Some(stone_id),
                max_count: None,
            })
        );
        assert_eq!(
            parse_extra_command("/clear @s minecraft:stone 16", origin()),
            Some(ExtraCommandAction::ClearInventory {
                target: Some("@s".into()),
                item_id: Some(stone_id),
                max_count: Some(16),
            })
        );

        assert_eq!(
            parse_extra_command("/spawnpoint", origin()),
            Some(ExtraCommandAction::SetSpawnpoint {
                target: None,
                pos: BlockPos::new(10, 64, -6),
            })
        );
        assert_eq!(
            parse_extra_command("/spawnpoint ~1 ~2 ~3", origin()),
            Some(ExtraCommandAction::SetSpawnpoint {
                target: None,
                pos: BlockPos::new(11, 66, -3),
            })
        );
        assert_eq!(
            parse_extra_command("/setworldspawn 0 80 0", origin()),
            Some(ExtraCommandAction::SetWorldSpawn {
                pos: BlockPos::new(0, 80, 0),
            })
        );

        assert_eq!(
            parse_extra_command("/difficulty", origin()),
            Some(ExtraCommandAction::QueryDifficulty)
        );
        assert_eq!(
            parse_extra_command("/difficulty hard", origin()),
            Some(ExtraCommandAction::SetDifficulty(3))
        );

        assert_eq!(
            parse_extra_command("/enchant sharpness 5", origin()),
            Some(ExtraCommandAction::Enchant {
                target: None,
                enchantment: "minecraft:sharpness".into(),
                level: 5,
            })
        );
        assert_eq!(
            parse_extra_command("/enchant @s mending", origin()),
            Some(ExtraCommandAction::Enchant {
                target: Some("@s".into()),
                enchantment: "minecraft:mending".into(),
                level: 1,
            })
        );

        assert_eq!(
            parse_extra_command("/advancement grant everything", origin()),
            Some(ExtraCommandAction::Advancement {
                grant: true,
                target: None,
                advancement_id: None,
            })
        );
        assert_eq!(
            parse_extra_command("/advancement revoke story/mine_stone", origin()),
            Some(ExtraCommandAction::Advancement {
                grant: false,
                target: None,
                advancement_id: Some("minecraft:story/mine_stone".into()),
            })
        );
        assert_eq!(
            parse_extra_command("/advancement grant @s only minecraft:story/mine_diamond", origin()),
            Some(ExtraCommandAction::Advancement {
                grant: true,
                target: Some("@s".into()),
                advancement_id: Some("minecraft:story/mine_diamond".into()),
            })
        );
    }
}
