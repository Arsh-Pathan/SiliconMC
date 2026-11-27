use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

pub const RENDER_TYPE_INTEGER: i32 = 0;
pub const RENDER_TYPE_HEARTS: i32 = 1;

pub const DISPLAY_SLOT_LIST: i32 = 0;
pub const DISPLAY_SLOT_SIDEBAR: i32 = 1;
pub const DISPLAY_SLOT_BELOW_NAME: i32 = 2;

pub const BOSSBAR_COLOR_PINK: i32 = 0;
pub const BOSSBAR_COLOR_BLUE: i32 = 1;
pub const BOSSBAR_COLOR_RED: i32 = 2;
pub const BOSSBAR_COLOR_GREEN: i32 = 3;
pub const BOSSBAR_COLOR_YELLOW: i32 = 4;
pub const BOSSBAR_COLOR_PURPLE: i32 = 5;
pub const BOSSBAR_COLOR_WHITE: i32 = 6;

pub const BOSSBAR_OVERLAY_PROGRESS: i32 = 0;
pub const BOSSBAR_OVERLAY_NOTCHED_6: i32 = 1;
pub const BOSSBAR_OVERLAY_NOTCHED_10: i32 = 2;
pub const BOSSBAR_OVERLAY_NOTCHED_12: i32 = 3;
pub const BOSSBAR_OVERLAY_NOTCHED_20: i32 = 4;

/// A scoreboard objective matching `net.minecraft.world.scores.Objective`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Objective {
    pub name: String,
    pub criteria: String,
    pub display_name: String,
    pub render_type: i32,
}

/// A scoreboard team matching `net.minecraft.world.scores.PlayerTeam`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    pub name: String,
    pub display_name: String,
    pub prefix: String,
    pub suffix: String,
    pub color: i32,
    pub friendly_fire: bool,
    pub see_friendly_invisibles: bool,
    pub members: Vec<String>,
}

/// A custom server boss bar matching `net.minecraft.server.bossevents.CustomBossEvent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BossBar {
    pub id: String,
    pub uuid: Uuid,
    pub name: String,
    pub progress: f32,
    pub color: i32,
    pub overlay: i32,
    pub visible: bool,
    pub max: i32,
    pub value: i32,
}

impl BossBar {
    fn recompute_progress(&mut self) {
        self.progress = if self.max <= 0 {
            0.0
        } else {
            (self.value as f32 / self.max as f32).clamp(0.0, 1.0)
        };
    }
}

/// Parse a scoreboard display slot name or numeric ID (`0=list`, `1=sidebar`, `2=below_name`,
/// `3..=18=sidebar.team.<color>`).
pub fn parse_display_slot(raw: &str) -> Option<i32> {
    if let Ok(n) = raw.parse::<i32>() {
        return (0..=18).contains(&n).then_some(n);
    }
    match raw.to_ascii_lowercase().as_str() {
        "list" => Some(DISPLAY_SLOT_LIST),
        "sidebar" => Some(DISPLAY_SLOT_SIDEBAR),
        "below_name" | "belowname" => Some(DISPLAY_SLOT_BELOW_NAME),
        "sidebar.team.black" => Some(3),
        "sidebar.team.dark_blue" => Some(4),
        "sidebar.team.dark_green" => Some(5),
        "sidebar.team.dark_aqua" => Some(6),
        "sidebar.team.dark_red" => Some(7),
        "sidebar.team.dark_purple" => Some(8),
        "sidebar.team.gold" => Some(9),
        "sidebar.team.gray" => Some(10),
        "sidebar.team.dark_gray" => Some(11),
        "sidebar.team.blue" => Some(12),
        "sidebar.team.green" => Some(13),
        "sidebar.team.aqua" => Some(14),
        "sidebar.team.red" => Some(15),
        "sidebar.team.light_purple" => Some(16),
        "sidebar.team.yellow" => Some(17),
        "sidebar.team.white" => Some(18),
        _ => None,
    }
}

/// Parse a `BossEvent.BossBarColor` name or ordinal (`0..=6`).
pub fn parse_bossbar_color(raw: &str) -> Option<i32> {
    if let Ok(n) = raw.parse::<i32>() {
        return (0..=6).contains(&n).then_some(n);
    }
    match raw.to_ascii_lowercase().as_str() {
        "pink" => Some(BOSSBAR_COLOR_PINK),
        "blue" => Some(BOSSBAR_COLOR_BLUE),
        "red" => Some(BOSSBAR_COLOR_RED),
        "green" => Some(BOSSBAR_COLOR_GREEN),
        "yellow" => Some(BOSSBAR_COLOR_YELLOW),
        "purple" => Some(BOSSBAR_COLOR_PURPLE),
        "white" => Some(BOSSBAR_COLOR_WHITE),
        _ => None,
    }
}

/// Parse a `BossEvent.BossBarOverlay` style name or ordinal (`0..=4`).
pub fn parse_bossbar_overlay(raw: &str) -> Option<i32> {
    if let Ok(n) = raw.parse::<i32>() {
        return (0..=4).contains(&n).then_some(n);
    }
    match raw.to_ascii_lowercase().as_str() {
        "progress" => Some(BOSSBAR_OVERLAY_PROGRESS),
        "notched_6" => Some(BOSSBAR_OVERLAY_NOTCHED_6),
        "notched_10" => Some(BOSSBAR_OVERLAY_NOTCHED_10),
        "notched_12" => Some(BOSSBAR_OVERLAY_NOTCHED_12),
        "notched_20" => Some(BOSSBAR_OVERLAY_NOTCHED_20),
        _ => None,
    }
}

/// Parse a `TeamColor` name or ordinal (`0..=15`).
pub fn parse_team_color(raw: &str) -> Option<i32> {
    if let Ok(n) = raw.parse::<i32>() {
        return (0..=15).contains(&n).then_some(n);
    }
    match raw.to_ascii_lowercase().as_str() {
        "black" => Some(0),
        "dark_blue" => Some(1),
        "dark_green" => Some(2),
        "dark_aqua" => Some(3),
        "dark_red" => Some(4),
        "dark_purple" => Some(5),
        "gold" => Some(6),
        "gray" => Some(7),
        "dark_gray" => Some(8),
        "blue" => Some(9),
        "green" => Some(10),
        "aqua" => Some(11),
        "red" => Some(12),
        "light_purple" => Some(13),
        "yellow" => Some(14),
        "white" => Some(15),
        _ => None,
    }
}

fn normalize_bossbar_id(raw: &str) -> String {
    if raw.contains(':') {
        raw.to_string()
    } else {
        format!("minecraft:{raw}")
    }
}

/// Unified in-memory server store for scoreboard objectives, player scores,
/// display slots, teams, and custom boss bars.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScoreboardStore {
    objectives: BTreeMap<String, Objective>,
    /// Map of `objective_name -> (score_holder -> value)`.
    scores: BTreeMap<String, BTreeMap<String, i32>>,
    /// Map of `display_slot_id (0=list, 1=sidebar, 2=below_name, ...) -> objective_name`.
    display_slots: BTreeMap<i32, String>,
    teams: BTreeMap<String, Team>,
    bossbars: BTreeMap<String, BossBar>,
}

impl ScoreboardStore {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- Objectives ----

    pub fn add_objective(
        &mut self,
        name: &str,
        criteria: &str,
        display_name: Option<&str>,
    ) -> Result<&Objective, String> {
        if name.is_empty() {
            return Err("Objective name cannot be empty".into());
        }
        if self.objectives.contains_key(name) {
            return Err(format!("An objective with the name '{name}' already exists"));
        }
        let render_type = if criteria.eq_ignore_ascii_case("health") {
            RENDER_TYPE_HEARTS
        } else {
            RENDER_TYPE_INTEGER
        };
        let obj = Objective {
            name: name.to_string(),
            criteria: criteria.to_string(),
            display_name: display_name.unwrap_or(name).to_string(),
            render_type,
        };
        self.objectives.insert(name.to_string(), obj);
        Ok(self.objectives.get(name).unwrap())
    }

    pub fn remove_objective(&mut self, name: &str) -> Result<Objective, String> {
        let Some(removed) = self.objectives.remove(name) else {
            return Err(format!("Unknown scoreboard objective '{name}'"));
        };
        self.scores.remove(name);
        self.display_slots.retain(|_, obj_name| obj_name != name);
        Ok(removed)
    }

    pub fn get_objective(&self, name: &str) -> Option<&Objective> {
        self.objectives.get(name)
    }

    pub fn list_objectives(&self) -> Vec<&Objective> {
        self.objectives.values().collect()
    }

    pub fn set_objective_display_name(
        &mut self,
        name: &str,
        display_name: &str,
    ) -> Result<(), String> {
        let Some(obj) = self.objectives.get_mut(name) else {
            return Err(format!("Unknown scoreboard objective '{name}'"));
        };
        obj.display_name = display_name.to_string();
        Ok(())
    }

    pub fn set_objective_render_type(&mut self, name: &str, render_type: i32) -> Result<(), String> {
        if render_type != RENDER_TYPE_INTEGER && render_type != RENDER_TYPE_HEARTS {
            return Err(format!("Invalid render type: {render_type}"));
        }
        let Some(obj) = self.objectives.get_mut(name) else {
            return Err(format!("Unknown scoreboard objective '{name}'"));
        };
        obj.render_type = render_type;
        Ok(())
    }

    // ---- Display Slots ----

    /// Assign an objective to a display slot (`0=list`, `1=sidebar`, `2=below_name`),
    /// or clear the slot when `objective` is `None`.
    pub fn set_display_slot(&mut self, slot: i32, objective: Option<&str>) -> Result<(), String> {
        if !(0..=18).contains(&slot) {
            return Err(format!("Invalid display slot: {slot}"));
        }
        match objective {
            Some(name) => {
                if !self.objectives.contains_key(name) {
                    return Err(format!("Unknown scoreboard objective '{name}'"));
                }
                self.display_slots.insert(slot, name.to_string());
            }
            None => {
                self.display_slots.remove(&slot);
            }
        }
        Ok(())
    }

    pub fn get_display_slot(&self, slot: i32) -> Option<&str> {
        self.display_slots.get(&slot).map(String::as_str)
    }

    pub fn display_slots(&self) -> &BTreeMap<i32, String> {
        &self.display_slots
    }

    // ---- Scores ----

    pub fn set_score(&mut self, holder: &str, objective: &str, score: i32) -> Result<i32, String> {
        if !self.objectives.contains_key(objective) {
            return Err(format!("Unknown scoreboard objective '{objective}'"));
        }
        self.scores
            .entry(objective.to_string())
            .or_default()
            .insert(holder.to_string(), score);
        Ok(score)
    }

    pub fn add_score(&mut self, holder: &str, objective: &str, delta: i32) -> Result<i32, String> {
        if !self.objectives.contains_key(objective) {
            return Err(format!("Unknown scoreboard objective '{objective}'"));
        }
        let entry = self
            .scores
            .entry(objective.to_string())
            .or_default()
            .entry(holder.to_string())
            .or_insert(0);
        *entry = entry.saturating_add(delta);
        Ok(*entry)
    }

    pub fn remove_score(
        &mut self,
        holder: &str,
        objective: &str,
        delta: i32,
    ) -> Result<i32, String> {
        if !self.objectives.contains_key(objective) {
            return Err(format!("Unknown scoreboard objective '{objective}'"));
        }
        let entry = self
            .scores
            .entry(objective.to_string())
            .or_default()
            .entry(holder.to_string())
            .or_insert(0);
        *entry = entry.saturating_sub(delta);
        Ok(*entry)
    }

    pub fn reset_score(&mut self, holder: &str, objective: Option<&str>) -> Result<usize, String> {
        match objective {
            Some(obj) => {
                if !self.objectives.contains_key(obj) {
                    return Err(format!("Unknown scoreboard objective '{obj}'"));
                }
                let removed = self
                    .scores
                    .get_mut(obj)
                    .and_then(|m| m.remove(holder))
                    .is_some();
                Ok(usize::from(removed))
            }
            None => {
                let mut count = 0usize;
                for m in self.scores.values_mut() {
                    if m.remove(holder).is_some() {
                        count += 1;
                    }
                }
                Ok(count)
            }
        }
    }

    pub fn get_score(&self, holder: &str, objective: &str) -> Option<i32> {
        self.scores.get(objective)?.get(holder).copied()
    }

    /// Return all scores for an objective sorted descending by score, then ascending by holder.
    pub fn objective_scores(&self, objective: &str) -> Vec<(&str, i32)> {
        let Some(map) = self.scores.get(objective) else {
            return Vec::new();
        };
        let mut list: Vec<(&str, i32)> = map.iter().map(|(k, &v)| (k.as_str(), v)).collect();
        list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        list
    }

    // ---- Teams ----

    pub fn add_team(&mut self, name: &str, display_name: Option<&str>) -> Result<&Team, String> {
        if name.is_empty() {
            return Err("Team name cannot be empty".into());
        }
        if self.teams.contains_key(name) {
            return Err(format!("A team with the name '{name}' already exists"));
        }
        let team = Team {
            name: name.to_string(),
            display_name: display_name.unwrap_or(name).to_string(),
            prefix: String::new(),
            suffix: String::new(),
            color: 15, // white
            friendly_fire: true,
            see_friendly_invisibles: true,
            members: Vec::new(),
        };
        self.teams.insert(name.to_string(), team);
        Ok(self.teams.get(name).unwrap())
    }

    pub fn remove_team(&mut self, name: &str) -> Result<Team, String> {
        self.teams
            .remove(name)
            .ok_or_else(|| format!("Unknown team '{name}'"))
    }

    pub fn get_team(&self, name: &str) -> Option<&Team> {
        self.teams.get(name)
    }

    pub fn get_team_mut(&mut self, name: &str) -> Option<&mut Team> {
        self.teams.get_mut(name)
    }

    pub fn list_teams(&self) -> Vec<&Team> {
        self.teams.values().collect()
    }

    /// Add `member` to `team_name`, removing them from any previous team first.
    pub fn join_team(&mut self, team_name: &str, member: &str) -> Result<(), String> {
        if !self.teams.contains_key(team_name) {
            return Err(format!("Unknown team '{team_name}'"));
        }
        for team in self.teams.values_mut() {
            team.members.retain(|m| m != member);
        }
        let team = self.teams.get_mut(team_name).unwrap();
        team.members.push(member.to_string());
        Ok(())
    }

    /// Remove `member` from whichever team they belong to, returning that team's name.
    pub fn leave_team(&mut self, member: &str) -> Option<String> {
        for team in self.teams.values_mut() {
            let before = team.members.len();
            team.members.retain(|m| m != member);
            if team.members.len() != before {
                return Some(team.name.clone());
            }
        }
        None
    }

    pub fn team_of_member(&self, member: &str) -> Option<&Team> {
        self.teams
            .values()
            .find(|t| t.members.iter().any(|m| m == member))
    }

    // ---- BossBars ----

    pub fn add_bossbar(&mut self, id: &str, name: &str) -> Result<&BossBar, String> {
        let norm_id = normalize_bossbar_id(id);
        if self.bossbars.contains_key(&norm_id) {
            return Err(format!("A bossbar already exists with the ID '{norm_id}'"));
        }
        let bar = BossBar {
            id: norm_id.clone(),
            uuid: Uuid::new_v4(),
            name: name.to_string(),
            progress: 0.0,
            color: BOSSBAR_COLOR_WHITE,
            overlay: BOSSBAR_OVERLAY_PROGRESS,
            visible: true,
            max: 100,
            value: 0,
        };
        self.bossbars.insert(norm_id.clone(), bar);
        Ok(self.bossbars.get(&norm_id).unwrap())
    }

    pub fn remove_bossbar(&mut self, id: &str) -> Result<BossBar, String> {
        let norm_id = normalize_bossbar_id(id);
        self.bossbars
            .remove(&norm_id)
            .ok_or_else(|| format!("No bossbar exists with the ID '{norm_id}'"))
    }

    pub fn get_bossbar(&self, id: &str) -> Option<&BossBar> {
        let norm_id = normalize_bossbar_id(id);
        self.bossbars.get(&norm_id)
    }

    pub fn list_bossbars(&self) -> Vec<&BossBar> {
        self.bossbars.values().collect()
    }

    pub fn set_bossbar_name(&mut self, id: &str, name: &str) -> Result<&BossBar, String> {
        let norm_id = normalize_bossbar_id(id);
        let bar = self
            .bossbars
            .get_mut(&norm_id)
            .ok_or_else(|| format!("No bossbar exists with the ID '{norm_id}'"))?;
        bar.name = name.to_string();
        Ok(bar)
    }

    pub fn set_bossbar_value(&mut self, id: &str, value: i32) -> Result<&BossBar, String> {
        if value < 0 {
            return Err(format!("Bossbar value must be >= 0 (got {value})"));
        }
        let norm_id = normalize_bossbar_id(id);
        let bar = self
            .bossbars
            .get_mut(&norm_id)
            .ok_or_else(|| format!("No bossbar exists with the ID '{norm_id}'"))?;
        bar.value = value;
        bar.recompute_progress();
        Ok(bar)
    }

    pub fn set_bossbar_max(&mut self, id: &str, max: i32) -> Result<&BossBar, String> {
        if max < 1 {
            return Err(format!("Bossbar max must be >= 1 (got {max})"));
        }
        let norm_id = normalize_bossbar_id(id);
        let bar = self
            .bossbars
            .get_mut(&norm_id)
            .ok_or_else(|| format!("No bossbar exists with the ID '{norm_id}'"))?;
        bar.max = max;
        bar.recompute_progress();
        Ok(bar)
    }

    pub fn set_bossbar_visible(&mut self, id: &str, visible: bool) -> Result<&BossBar, String> {
        let norm_id = normalize_bossbar_id(id);
        let bar = self
            .bossbars
            .get_mut(&norm_id)
            .ok_or_else(|| format!("No bossbar exists with the ID '{norm_id}'"))?;
        bar.visible = visible;
        Ok(bar)
    }

    pub fn set_bossbar_color(&mut self, id: &str, color: i32) -> Result<&BossBar, String> {
        if !(0..=6).contains(&color) {
            return Err(format!("Invalid bossbar color ID: {color}"));
        }
        let norm_id = normalize_bossbar_id(id);
        let bar = self
            .bossbars
            .get_mut(&norm_id)
            .ok_or_else(|| format!("No bossbar exists with the ID '{norm_id}'"))?;
        bar.color = color;
        Ok(bar)
    }

    pub fn set_bossbar_overlay(&mut self, id: &str, overlay: i32) -> Result<&BossBar, String> {
        if !(0..=4).contains(&overlay) {
            return Err(format!("Invalid bossbar overlay ID: {overlay}"));
        }
        let norm_id = normalize_bossbar_id(id);
        let bar = self
            .bossbars
            .get_mut(&norm_id)
            .ok_or_else(|| format!("No bossbar exists with the ID '{norm_id}'"))?;
        bar.overlay = overlay;
        Ok(bar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objectives_scores_and_display_slots_crud() {
        let mut sb = ScoreboardStore::new();
        let obj = sb.add_objective("kills", "playerKillCount", Some("Kills")).unwrap();
        assert_eq!(obj.render_type, RENDER_TYPE_INTEGER);
        assert_eq!(obj.display_name, "Kills");
        assert!(sb.add_objective("kills", "dummy", None).is_err());

        let hp = sb.add_objective("hp", "health", None).unwrap();
        assert_eq!(hp.render_type, RENDER_TYPE_HEARTS);

        sb.set_display_slot(DISPLAY_SLOT_SIDEBAR, Some("kills")).unwrap();
        sb.set_display_slot(DISPLAY_SLOT_BELOW_NAME, Some("hp")).unwrap();
        assert_eq!(sb.get_display_slot(DISPLAY_SLOT_SIDEBAR), Some("kills"));
        assert_eq!(sb.get_display_slot(DISPLAY_SLOT_BELOW_NAME), Some("hp"));

        assert_eq!(sb.set_score("Alex", "kills", 5).unwrap(), 5);
        assert_eq!(sb.add_score("Alex", "kills", 3).unwrap(), 8);
        assert_eq!(sb.add_score("Steve", "kills", 10).unwrap(), 10);
        assert_eq!(sb.remove_score("Steve", "kills", 1).unwrap(), 9);
        assert_eq!(sb.get_score("Steve", "kills"), Some(9));
        assert_eq!(
            sb.objective_scores("kills"),
            vec![("Steve", 9), ("Alex", 8)]
        );

        assert_eq!(sb.reset_score("Alex", Some("kills")).unwrap(), 1);
        assert_eq!(sb.get_score("Alex", "kills"), None);

        // Removing an objective clears display slots and scores
        sb.remove_objective("kills").unwrap();
        assert_eq!(sb.get_display_slot(DISPLAY_SLOT_SIDEBAR), None);
        assert_eq!(sb.get_score("Steve", "kills"), None);
    }

    #[test]
    fn teams_crud_and_exclusive_membership() {
        let mut sb = ScoreboardStore::new();
        sb.add_team("red", Some("Red Team")).unwrap();
        sb.add_team("blue", Some("Blue Team")).unwrap();
        assert!(sb.add_team("red", None).is_err());

        sb.join_team("red", "Steve").unwrap();
        assert_eq!(sb.team_of_member("Steve").unwrap().name, "red");

        // Joining another team leaves the previous team automatically
        sb.join_team("blue", "Steve").unwrap();
        assert_eq!(sb.team_of_member("Steve").unwrap().name, "blue");
        assert!(sb.get_team("red").unwrap().members.is_empty());

        assert_eq!(sb.leave_team("Steve").as_deref(), Some("blue"));
        assert!(sb.team_of_member("Steve").is_none());

        sb.remove_team("red").unwrap();
        assert_eq!(sb.list_teams().len(), 1);
    }

    #[test]
    fn bossbar_crud_and_progress_math() {
        let mut sb = ScoreboardStore::new();
        let bar = sb.add_bossbar("dragon", "Ender Dragon").unwrap();
        assert_eq!(bar.id, "minecraft:dragon");
        assert_eq!(bar.max, 100);
        assert_eq!(bar.value, 0);
        assert_eq!(bar.progress, 0.0);
        assert!(bar.visible);

        let bar = sb.set_bossbar_max("dragon", 200).unwrap();
        assert_eq!(bar.max, 200);
        let bar = sb.set_bossbar_value("minecraft:dragon", 50).unwrap();
        assert!((bar.progress - 0.25).abs() < 1e-6);

        let bar = sb.set_bossbar_color("dragon", BOSSBAR_COLOR_PURPLE).unwrap();
        assert_eq!(bar.color, BOSSBAR_COLOR_PURPLE);
        let bar = sb.set_bossbar_visible("dragon", false).unwrap();
        assert!(!bar.visible);

        assert!(sb.set_bossbar_max("dragon", 0).is_err());
        assert!(sb.set_bossbar_color("dragon", 99).is_err());

        sb.remove_bossbar("dragon").unwrap();
        assert!(sb.list_bossbars().is_empty());
    }
}
