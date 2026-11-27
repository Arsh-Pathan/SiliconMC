use mc_nbt::{NbtCompound, NbtTag};
use std::collections::BTreeMap;

/// Typed value of a Minecraft 26.3 game rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameRuleValue {
    Bool(bool),
    Int(i32),
}

impl GameRuleValue {
    pub fn as_bool(self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(b),
            Self::Int(_) => None,
        }
    }

    pub fn as_int(self) -> Option<i32> {
        match self {
            Self::Int(v) => Some(v),
            Self::Bool(_) => None,
        }
    }

    pub fn format_value(self) -> String {
        match self {
            Self::Bool(b) => b.to_string(),
            Self::Int(v) => v.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GameRuleSpec {
    id: &'static str,
    default: GameRuleValue,
    min: i32,
    max: i32,
}

const fn bool_rule(id: &'static str, default: bool) -> GameRuleSpec {
    GameRuleSpec {
        id,
        default: GameRuleValue::Bool(default),
        min: 0,
        max: 1,
    }
}

const fn int_rule(id: &'static str, default: i32, min: i32, max: i32) -> GameRuleSpec {
    GameRuleSpec {
        id,
        default: GameRuleValue::Int(default),
        min,
        max,
    }
}

/// Complete table of all 59 Minecraft 26.3 game rules from
/// `net.minecraft.world.level.gamerules.GameRules`.
const GAME_RULE_SPECS: &[GameRuleSpec] = &[
    bool_rule("advance_time", true),
    bool_rule("advance_weather", true),
    bool_rule("allow_entering_nether_using_portals", true),
    bool_rule("block_drops", true),
    bool_rule("block_explosion_drop_decay", true),
    bool_rule("command_block_output", true),
    bool_rule("command_blocks_work", true),
    bool_rule("drowning_damage", true),
    bool_rule("elytra_movement_check", true),
    bool_rule("ender_pearls_vanish_on_death", true),
    bool_rule("entity_drops", true),
    bool_rule("fall_damage", true),
    bool_rule("fire_damage", true),
    int_rule("fire_spread_radius_around_player", 128, -1, i32::MAX),
    bool_rule("forgive_dead_players", true),
    bool_rule("freeze_damage", true),
    bool_rule("global_sound_events", true),
    bool_rule("immediate_respawn", false),
    bool_rule("keep_inventory", false),
    bool_rule("lava_source_conversion", false),
    bool_rule("limited_crafting", false),
    bool_rule("locator_bar", true),
    bool_rule("log_admin_commands", true),
    int_rule("max_block_modifications", 32768, 1, i32::MAX),
    int_rule("max_command_forks", 65536, 0, i32::MAX),
    int_rule("max_command_sequence_length", 65536, 0, i32::MAX),
    int_rule("max_entity_cramming", 24, 0, i32::MAX),
    int_rule("max_minecart_speed", 8, 1, 1000),
    int_rule("max_snow_accumulation_height", 1, 0, 8),
    bool_rule("mob_drops", true),
    bool_rule("mob_explosion_drop_decay", true),
    bool_rule("mob_griefing", true),
    bool_rule("natural_health_regeneration", true),
    bool_rule("player_movement_check", true),
    int_rule("players_nether_portal_creative_delay", 0, 0, i32::MAX),
    int_rule("players_nether_portal_default_delay", 80, 0, i32::MAX),
    int_rule("players_sleeping_percentage", 100, 0, i32::MAX),
    bool_rule("projectiles_can_break_blocks", true),
    bool_rule("pvp", true),
    bool_rule("raids", true),
    int_rule("random_tick_speed", 3, 0, i32::MAX),
    bool_rule("reduced_debug_info", false),
    int_rule("respawn_radius", 10, 0, i32::MAX),
    bool_rule("send_command_feedback", true),
    bool_rule("show_advancement_messages", true),
    bool_rule("show_death_messages", true),
    bool_rule("spawn_mobs", true),
    bool_rule("spawn_monsters", true),
    bool_rule("spawn_patrols", true),
    bool_rule("spawn_phantoms", true),
    bool_rule("spawn_wandering_traders", true),
    bool_rule("spawn_wardens", true),
    bool_rule("spawner_blocks_work", true),
    bool_rule("spectators_generate_chunks", true),
    bool_rule("spread_vines", true),
    bool_rule("tnt_explodes", true),
    bool_rule("tnt_explosion_drop_decay", false),
    bool_rule("universal_anger", false),
    bool_rule("water_source_conversion", true),
];

/// Resolution target for a canonical 26.3 rule name or legacy camelCase alias.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedRule {
    Direct(&'static GameRuleSpec),
    /// Legacy inverted boolean alias (e.g. `disableRaids` -> `!raids`).
    InvertedBool(&'static GameRuleSpec),
    /// Legacy `doFireTick` boolean alias mapped onto `fire_spread_radius_around_player`.
    DoFireTick,
    /// Legacy `allowFireTicksAwayFromPlayer` boolean alias mapped onto `fire_spread_radius_around_player`.
    AllowFireTicksAwayFromPlayer,
}

fn find_spec(canonical_id: &str) -> Option<&'static GameRuleSpec> {
    GAME_RULE_SPECS.iter().find(|s| s.id == canonical_id)
}

fn resolve_rule(raw_name: &str) -> Option<ResolvedRule> {
    let trimmed = raw_name.trim();
    let stripped = trimmed
        .strip_prefix("minecraft:")
        .or_else(|| trimmed.strip_prefix("MINECRAFT:"))
        .unwrap_or(trimmed);

    // Exact canonical 26.3 snake_case match first.
    if let Some(spec) = find_spec(stripped) {
        return Some(ResolvedRule::Direct(spec));
    }

    // Case-insensitive lookup for canonical names and all legacy camelCase aliases
    // from `net.minecraft.util.datafix.fixes.GameRuleRegistryFix`.
    let lower = stripped.to_ascii_lowercase();
    if let Some(spec) = find_spec(&lower) {
        return Some(ResolvedRule::Direct(spec));
    }

    match lower.as_str() {
        "dofiretick" => Some(ResolvedRule::DoFireTick),
        "allowfireticksawayfromplayer" => Some(ResolvedRule::AllowFireTicksAwayFromPlayer),
        "disableelytramovementcheck" => {
            find_spec("elytra_movement_check").map(ResolvedRule::InvertedBool)
        }
        "disableplayermovementcheck" => {
            find_spec("player_movement_check").map(ResolvedRule::InvertedBool)
        }
        "disableraids" => find_spec("raids").map(ResolvedRule::InvertedBool),
        _ => {
            let mapped = match lower.as_str() {
                "allowenteringnetherusingportals" => "allow_entering_nether_using_portals",
                "announceadvancements" => "show_advancement_messages",
                "blockexplosiondropdecay" => "block_explosion_drop_decay",
                "commandblockoutput" => "command_block_output",
                "enablecommandblocks" | "commandblocksenabled" => "command_blocks_work",
                "commandmodificationblocklimit" => "max_block_modifications",
                "dodaylightcycle" => "advance_time",
                "doentitydrops" => "entity_drops",
                "doimmediaterespawn" => "immediate_respawn",
                "doinsomnia" => "spawn_phantoms",
                "dolimitedcrafting" => "limited_crafting",
                "domobloot" => "mob_drops",
                "domobspawning" => "spawn_mobs",
                "dopatrolspawning" => "spawn_patrols",
                "dotiledrops" => "block_drops",
                "dotraderspawning" => "spawn_wandering_traders",
                "dovinesspread" => "spread_vines",
                "dowardenspawning" => "spawn_wardens",
                "doweathercycle" => "advance_weather",
                "drowningdamage" => "drowning_damage",
                "enderpearlsvanishondeath" => "ender_pearls_vanish_on_death",
                "falldamage" => "fall_damage",
                "firedamage" => "fire_damage",
                "forgivedeadplayers" => "forgive_dead_players",
                "freezedamage" => "freeze_damage",
                "globalsoundevents" => "global_sound_events",
                "keepinventory" => "keep_inventory",
                "lavasourceconversion" => "lava_source_conversion",
                "locatorbar" => "locator_bar",
                "logadmincommands" => "log_admin_commands",
                "maxcommandchainlength" => "max_command_sequence_length",
                "maxcommandforkcount" => "max_command_forks",
                "maxentitycramming" => "max_entity_cramming",
                "minecartmaxspeed" => "max_minecart_speed",
                "mobexplosiondropdecay" => "mob_explosion_drop_decay",
                "mobgriefing" => "mob_griefing",
                "naturalregeneration" => "natural_health_regeneration",
                "playersnetherportalcreativedelay" => "players_nether_portal_creative_delay",
                "playersnetherportaldefaultdelay" => "players_nether_portal_default_delay",
                "playerssleepingpercentage" => "players_sleeping_percentage",
                "projectilescanbreakblocks" => "projectiles_can_break_blocks",
                "randomtickspeed" => "random_tick_speed",
                "reduceddebuginfo" => "reduced_debug_info",
                "sendcommandfeedback" => "send_command_feedback",
                "showdeathmessages" => "show_death_messages",
                "snowaccumulationheight" => "max_snow_accumulation_height",
                "spawnmonsters" => "spawn_monsters",
                "spawnradius" => "respawn_radius",
                "spawnerblocksenabled" => "spawner_blocks_work",
                "spectatorsgeneratechunks" => "spectators_generate_chunks",
                "tntexplodes" => "tnt_explodes",
                "tntexplosiondropdecay" => "tnt_explosion_drop_decay",
                "universalanger" => "universal_anger",
                "watersourceconversion" => "water_source_conversion",
                _ => return None,
            };
            find_spec(mapped).map(ResolvedRule::Direct)
        }
    }
}

fn parse_bool_strict(val: &str) -> Result<bool, String> {
    match val.trim().to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("Invalid boolean value '{other}' (expected 'true' or 'false')")),
    }
}

/// Persistent Minecraft 26.3 game rule store supporting all 59 canonical snake_case
/// game rule IDs plus classic camelCase aliases for `/gamerule` and legacy NBT loading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameRuleStore {
    values: BTreeMap<&'static str, GameRuleValue>,
}

impl Default for GameRuleStore {
    fn default() -> Self {
        Self::new()
    }
}

impl GameRuleStore {
    /// Create a new store populated with exact Minecraft 26.3 default values.
    pub fn new() -> Self {
        let mut values = BTreeMap::new();
        for spec in GAME_RULE_SPECS {
            values.insert(spec.id, spec.default);
        }
        Self { values }
    }

    /// Resolve a rule name or legacy alias to its canonical 26.3 snake_case ID.
    pub fn canonical_name(name: &str) -> Option<&'static str> {
        match resolve_rule(name)? {
            ResolvedRule::Direct(spec) | ResolvedRule::InvertedBool(spec) => Some(spec.id),
            ResolvedRule::DoFireTick | ResolvedRule::AllowFireTicksAwayFromPlayer => {
                Some("fire_spread_radius_around_player")
            }
        }
    }

    /// Return all 59 canonical 26.3 game rule names in sorted order.
    pub fn list_names(&self) -> Vec<&'static str> {
        self.values.keys().copied().collect()
    }

    fn bool_val(&self, canonical_id: &str) -> bool {
        self.values
            .get(canonical_id)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    fn int_val(&self, canonical_id: &str) -> i32 {
        self.values
            .get(canonical_id)
            .and_then(|v| v.as_int())
            .unwrap_or(0)
    }

    // ---- Typed accessors (classic + canonical 26.3 names) ----

    pub fn keep_inventory(&self) -> bool {
        self.bool_val("keep_inventory")
    }

    pub fn advance_time(&self) -> bool {
        self.bool_val("advance_time")
    }

    pub fn do_daylight_cycle(&self) -> bool {
        self.advance_time()
    }

    pub fn advance_weather(&self) -> bool {
        self.bool_val("advance_weather")
    }

    pub fn do_weather_cycle(&self) -> bool {
        self.advance_weather()
    }

    pub fn spawn_mobs(&self) -> bool {
        self.bool_val("spawn_mobs")
    }

    pub fn do_mob_spawning(&self) -> bool {
        self.spawn_mobs()
    }

    pub fn spawn_monsters(&self) -> bool {
        self.bool_val("spawn_monsters")
    }

    pub fn mob_griefing(&self) -> bool {
        self.bool_val("mob_griefing")
    }

    pub fn fire_spread_radius_around_player(&self) -> i32 {
        self.int_val("fire_spread_radius_around_player")
    }

    pub fn do_fire_tick(&self) -> bool {
        self.fire_spread_radius_around_player() != 0
    }

    pub fn fall_damage(&self) -> bool {
        self.bool_val("fall_damage")
    }

    pub fn drowning_damage(&self) -> bool {
        self.bool_val("drowning_damage")
    }

    pub fn fire_damage(&self) -> bool {
        self.bool_val("fire_damage")
    }

    pub fn freeze_damage(&self) -> bool {
        self.bool_val("freeze_damage")
    }

    pub fn natural_health_regeneration(&self) -> bool {
        self.bool_val("natural_health_regeneration")
    }

    pub fn natural_regeneration(&self) -> bool {
        self.natural_health_regeneration()
    }

    pub fn immediate_respawn(&self) -> bool {
        self.bool_val("immediate_respawn")
    }

    pub fn do_immediate_respawn(&self) -> bool {
        self.immediate_respawn()
    }

    pub fn pvp(&self) -> bool {
        self.bool_val("pvp")
    }

    pub fn random_tick_speed(&self) -> i32 {
        self.int_val("random_tick_speed")
    }

    pub fn players_sleeping_percentage(&self) -> i32 {
        self.int_val("players_sleeping_percentage")
    }

    pub fn respawn_radius(&self) -> i32 {
        self.int_val("respawn_radius")
    }

    pub fn spawn_radius(&self) -> i32 {
        self.respawn_radius()
    }

    pub fn max_block_modifications(&self) -> i32 {
        self.int_val("max_block_modifications")
    }

    pub fn max_entity_cramming(&self) -> i32 {
        self.int_val("max_entity_cramming")
    }

    pub fn show_death_messages(&self) -> bool {
        self.bool_val("show_death_messages")
    }

    pub fn show_advancement_messages(&self) -> bool {
        self.bool_val("show_advancement_messages")
    }

    pub fn send_command_feedback(&self) -> bool {
        self.bool_val("send_command_feedback")
    }

    pub fn block_drops(&self) -> bool {
        self.bool_val("block_drops")
    }

    pub fn mob_drops(&self) -> bool {
        self.bool_val("mob_drops")
    }

    pub fn entity_drops(&self) -> bool {
        self.bool_val("entity_drops")
    }

    pub fn tnt_explodes(&self) -> bool {
        self.bool_val("tnt_explodes")
    }

    pub fn reduced_debug_info(&self) -> bool {
        self.bool_val("reduced_debug_info")
    }

    /// Look up any game rule (canonical 26.3 name or legacy alias) as a formatted string.
    pub fn get_str(&self, name: &str) -> Option<String> {
        match resolve_rule(name)? {
            ResolvedRule::Direct(spec) => self.values.get(spec.id).map(|v| v.format_value()),
            ResolvedRule::InvertedBool(spec) => self
                .values
                .get(spec.id)
                .and_then(|v| v.as_bool())
                .map(|b| (!b).to_string()),
            ResolvedRule::DoFireTick => Some(self.do_fire_tick().to_string()),
            ResolvedRule::AllowFireTicksAwayFromPlayer => {
                Some((self.fire_spread_radius_around_player() < 0).to_string())
            }
        }
    }

    /// Parse and apply a string value for any canonical 26.3 game rule or legacy alias.
    /// Returns the formatted canonical value on success.
    pub fn set_from_str(&mut self, name: &str, val: &str) -> Result<String, String> {
        let Some(resolved) = resolve_rule(name) else {
            return Err(format!("Unknown gamerule: {name}"));
        };
        match resolved {
            ResolvedRule::Direct(spec) => match spec.default {
                GameRuleValue::Bool(_) => {
                    let b = parse_bool_strict(val)?;
                    self.values.insert(spec.id, GameRuleValue::Bool(b));
                    Ok(b.to_string())
                }
                GameRuleValue::Int(_) => {
                    let parsed: i32 = val
                        .trim()
                        .parse()
                        .map_err(|_| format!("Invalid integer value '{val}' for gamerule '{}'", spec.id))?;
                    if parsed < spec.min || parsed > spec.max {
                        return Err(format!(
                            "Value {parsed} out of range [{}, {}] for gamerule '{}'",
                            spec.min, spec.max, spec.id
                        ));
                    }
                    self.values.insert(spec.id, GameRuleValue::Int(parsed));
                    Ok(parsed.to_string())
                }
            },
            ResolvedRule::InvertedBool(spec) => {
                let legacy_val = parse_bool_strict(val)?;
                let canonical_val = !legacy_val;
                self.values
                    .insert(spec.id, GameRuleValue::Bool(canonical_val));
                Ok(legacy_val.to_string())
            }
            ResolvedRule::DoFireTick => {
                let enabled = parse_bool_strict(val)?;
                let new_radius = if enabled {
                    let cur = self.fire_spread_radius_around_player();
                    if cur == 0 { 128 } else { cur }
                } else {
                    0
                };
                self.values.insert(
                    "fire_spread_radius_around_player",
                    GameRuleValue::Int(new_radius),
                );
                Ok(enabled.to_string())
            }
            ResolvedRule::AllowFireTicksAwayFromPlayer => {
                let allow_away = parse_bool_strict(val)?;
                let new_radius = if allow_away { -1 } else { 128 };
                self.values.insert(
                    "fire_spread_radius_around_player",
                    GameRuleValue::Int(new_radius),
                );
                Ok(allow_away.to_string())
            }
        }
    }

    /// Serialize all game rules to a Minecraft 26.3 `game_rules` NBT compound
    /// (`"minecraft:<id>": Byte(0|1) | Int(v)`), matching `GameRuleMap.CODEC`.
    pub fn save_to_nbt(&self) -> NbtCompound {
        let mut out = NbtCompound::new();
        for (&id, &val) in &self.values {
            let key = format!("minecraft:{id}");
            let tag = match val {
                GameRuleValue::Bool(b) => NbtTag::Byte(if b { 1 } else { 0 }),
                GameRuleValue::Int(n) => NbtTag::Int(n),
            };
            out.insert(key, tag);
        }
        out
    }

    /// Load game rules from either a Minecraft 26.3 typed `game_rules` NBT compound
    /// or a legacy string-encoded `GameRules` NBT compound.
    pub fn load_from_nbt(&mut self, c: &NbtCompound) {
        // Handle legacy fire-tick pair together if canonical key is absent.
        let has_canonical_fire = c.get("minecraft:fire_spread_radius_around_player").is_some()
            || c.get("fire_spread_radius_around_player").is_some();
        if !has_canonical_fire
            && (c.get("doFireTick").is_some() || c.get("allowFireTicksAwayFromPlayer").is_some())
        {
            let do_fire_tick = c
                .get("doFireTick")
                .and_then(tag_to_bool)
                .unwrap_or(true);
            let allow_away = c
                .get("allowFireTicksAwayFromPlayer")
                .and_then(tag_to_bool)
                .unwrap_or(false);
            let radius = if !do_fire_tick {
                0
            } else if allow_away {
                -1
            } else {
                128
            };
            self.values
                .insert("fire_spread_radius_around_player", GameRuleValue::Int(radius));
        }

        for (key, tag) in c.iter() {
            if key.eq_ignore_ascii_case("doFireTick")
                || key.eq_ignore_ascii_case("allowFireTicksAwayFromPlayer")
            {
                continue;
            }
            let Some(resolved) = resolve_rule(key) else {
                continue;
            };
            match resolved {
                ResolvedRule::Direct(spec) => match spec.default {
                    GameRuleValue::Bool(_) => {
                        if let Some(b) = tag_to_bool(tag) {
                            self.values.insert(spec.id, GameRuleValue::Bool(b));
                        }
                    }
                    GameRuleValue::Int(_) => {
                        if let Some(n) = tag_to_int(tag) {
                            let clamped = n.clamp(spec.min, spec.max);
                            self.values.insert(spec.id, GameRuleValue::Int(clamped));
                        }
                    }
                },
                ResolvedRule::InvertedBool(spec) => {
                    if let Some(b) = tag_to_bool(tag) {
                        self.values.insert(spec.id, GameRuleValue::Bool(!b));
                    }
                }
                ResolvedRule::DoFireTick | ResolvedRule::AllowFireTicksAwayFromPlayer => {}
            }
        }
    }
}

fn tag_to_bool(tag: &NbtTag) -> Option<bool> {
    match tag {
        NbtTag::Byte(b) => Some(*b != 0),
        NbtTag::Int(n) => Some(*n != 0),
        NbtTag::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn tag_to_int(tag: &NbtTag) -> Option<i32> {
    match tag {
        NbtTag::Int(n) => Some(*n),
        NbtTag::Short(n) => Some(*n as i32),
        NbtTag::Byte(n) => Some(*n as i32),
        NbtTag::Long(n) => i32::try_from(*n).ok(),
        NbtTag::String(s) => s.trim().parse::<i32>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_minecraft_26_3() {
        let store = GameRuleStore::new();
        assert_eq!(store.list_names().len(), 59);
        assert!(!store.keep_inventory());
        assert!(store.do_daylight_cycle());
        assert!(store.do_weather_cycle());
        assert!(store.do_mob_spawning());
        assert!(store.mob_griefing());
        assert!(store.do_fire_tick());
        assert_eq!(store.fire_spread_radius_around_player(), 128);
        assert!(store.fall_damage());
        assert!(store.drowning_damage());
        assert!(store.fire_damage());
        assert!(store.natural_regeneration());
        assert!(!store.do_immediate_respawn());
        assert!(store.pvp());
        assert_eq!(store.random_tick_speed(), 3);
        assert_eq!(store.players_sleeping_percentage(), 100);
        assert_eq!(store.spawn_radius(), 10);
        assert_eq!(store.max_block_modifications(), 32768);
    }

    #[test]
    fn supports_both_canonical_26_3_and_classic_aliases() {
        let mut store = GameRuleStore::new();

        // Classic camelCase aliases
        assert_eq!(store.set_from_str("keepInventory", "true").unwrap(), "true");
        assert!(store.keep_inventory());
        assert_eq!(store.get_str("keep_inventory").as_deref(), Some("true"));
        assert_eq!(
            store.get_str("minecraft:keep_inventory").as_deref(),
            Some("true")
        );

        assert_eq!(store.set_from_str("doDaylightCycle", "false").unwrap(), "false");
        assert!(!store.do_daylight_cycle());
        assert_eq!(store.get_str("advance_time").as_deref(), Some("false"));

        // Inverted legacy alias: disableRaids=true => raids=false
        assert_eq!(store.set_from_str("disableRaids", "true").unwrap(), "true");
        assert_eq!(store.get_str("raids").as_deref(), Some("false"));
        assert_eq!(store.get_str("disableRaids").as_deref(), Some("true"));

        // Legacy doFireTick mapped to fire_spread_radius_around_player
        assert_eq!(store.set_from_str("doFireTick", "false").unwrap(), "false");
        assert!(!store.do_fire_tick());
        assert_eq!(store.fire_spread_radius_around_player(), 0);
        assert_eq!(store.set_from_str("doFireTick", "true").unwrap(), "true");
        assert!(store.do_fire_tick());
        assert_eq!(store.fire_spread_radius_around_player(), 128);

        // Bounds enforcement
        assert!(store.set_from_str("random_tick_speed", "-1").is_err());
        assert!(store.set_from_str("max_snow_accumulation_height", "9").is_err());
        assert_eq!(store.set_from_str("max_snow_accumulation_height", "8").unwrap(), "8");
        assert!(store.set_from_str("unknownRule", "true").is_err());
        assert!(store.set_from_str("pvp", "maybe").is_err());
    }

    #[test]
    fn nbt_roundtrip_and_legacy_load() {
        let mut store = GameRuleStore::new();
        store.set_from_str("keep_inventory", "true").unwrap();
        store.set_from_str("random_tick_speed", "12").unwrap();
        store.set_from_str("pvp", "false").unwrap();
        store.set_from_str("respawn_radius", "4").unwrap();

        let nbt = store.save_to_nbt();
        assert_eq!(nbt.get_byte("minecraft:keep_inventory"), Some(1));
        assert_eq!(nbt.get_int("minecraft:random_tick_speed"), Some(12));
        assert_eq!(nbt.get_byte("minecraft:pvp"), Some(0));

        let mut loaded = GameRuleStore::new();
        loaded.load_from_nbt(&nbt);
        assert_eq!(loaded, store);

        // Legacy string-typed GameRules compound
        let mut legacy = NbtCompound::new();
        legacy.insert("keepInventory", NbtTag::String("true".into()));
        legacy.insert("doDaylightCycle", NbtTag::String("false".into()));
        legacy.insert("doFireTick", NbtTag::String("false".into()));
        legacy.insert("disableRaids", NbtTag::String("true".into()));
        legacy.insert("randomTickSpeed", NbtTag::String("7".into()));
        legacy.insert("spawnRadius", NbtTag::String("25".into()));

        let mut from_legacy = GameRuleStore::new();
        from_legacy.load_from_nbt(&legacy);
        assert!(from_legacy.keep_inventory());
        assert!(!from_legacy.do_daylight_cycle());
        assert!(!from_legacy.do_fire_tick());
        assert_eq!(from_legacy.get_str("raids").as_deref(), Some("false"));
        assert_eq!(from_legacy.random_tick_speed(), 7);
        assert_eq!(from_legacy.spawn_radius(), 25);
    }
}
