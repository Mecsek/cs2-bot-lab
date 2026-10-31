//! Bot team definitions loaded from `configs/teams/*.toml`.

use serde::{Deserialize, Serialize};
use std::path::Path;

pub const TEAM_SIZE: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Difficulty {
    Easy,
    Normal,
    Hard,
    Expert,
}

impl Difficulty {
    /// Argument for `bot_add_ct <difficulty> <name>`.
    pub fn cvar_arg(self) -> &'static str {
        match self {
            Difficulty::Easy => "easy",
            Difficulty::Normal => "normal",
            Difficulty::Hard => "hard",
            Difficulty::Expert => "expert",
        }
    }

    /// Value for the `Difficulty =` attribute in botprofile.db.
    pub fn profile_value(self) -> &'static str {
        match self {
            Difficulty::Easy => "EASY",
            Difficulty::Normal => "NORMAL",
            Difficulty::Hard => "HARD",
            Difficulty::Expert => "EXPERT",
        }
    }

    pub fn as_f32(self) -> f32 {
        match self {
            Difficulty::Easy => 0.0,
            Difficulty::Normal => 1.0,
            Difficulty::Hard => 2.0,
            Difficulty::Expert => 3.0,
        }
    }
}

/// Behaviour knobs that map 1:1 onto botprofile.db attributes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BotStyle {
    pub difficulty: Difficulty,
    /// 0-100
    pub skill: u8,
    /// 0-100
    pub aggression: u8,
    /// seconds
    pub reaction_time: f32,
    /// seconds
    pub attack_delay: f32,
    /// 0-100
    pub teamwork: u8,
    /// Designer names without the `weapon_` prefix, most preferred first.
    #[serde(default)]
    pub weapon_preference: Vec<String>,
}

/// Per-bot override; any field left out falls back to the team style.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BotOverride {
    pub difficulty: Option<Difficulty>,
    pub skill: Option<u8>,
    pub aggression: Option<u8>,
    pub reaction_time: Option<f32>,
    pub attack_delay: Option<f32>,
    pub teamwork: Option<u8>,
    pub weapon_preference: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Team {
    /// Upper-case identifier, used as bot name prefix (`<NAME>_1`..`<NAME>_5`).
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub style: BotStyle,
    /// Up to five overrides, applied to bots 1..=5 in order.
    #[serde(default)]
    pub bots: Vec<BotOverride>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedBot {
    pub name: String,
    pub style: BotStyle,
}

#[derive(Debug, thiserror::Error)]
pub enum TeamError {
    #[error("io error reading {path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("invalid team file {path}: {source}")]
    Parse { path: String, source: toml::de::Error },
    #[error("team {0}: {1}")]
    Invalid(String, String),
}

impl Team {
    pub fn validate(&self) -> Result<(), TeamError> {
        let bad = |m: &str| Err(TeamError::Invalid(self.name.clone(), m.to_string()));
        if self.name.is_empty()
            || !self.name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        {
            return bad("name must be non-empty [A-Z0-9_]");
        }
        if self.name.len() > 24 {
            return bad("name too long (max 24 chars, bot names are truncated in-game)");
        }
        if self.bots.len() > TEAM_SIZE {
            return bad("more than 5 bot overrides");
        }
        for b in self.resolved_bots() {
            let s = &b.style;
            if s.skill > 100 || s.aggression > 100 || s.teamwork > 100 {
                return bad("skill/aggression/teamwork must be 0-100");
            }
            if !(0.0..=2.0).contains(&s.reaction_time) || !(0.0..=2.0).contains(&s.attack_delay) {
                return bad("reaction_time/attack_delay must be within 0-2s");
            }
        }
        Ok(())
    }

    pub fn bot_name(&self, idx: usize) -> String {
        format!("{}_{}", self.name, idx + 1)
    }

    pub fn resolved_bots(&self) -> Vec<ResolvedBot> {
        (0..TEAM_SIZE)
            .map(|i| {
                let o = self.bots.get(i).cloned().unwrap_or_default();
                let s = &self.style;
                ResolvedBot {
                    name: self.bot_name(i),
                    style: BotStyle {
                        difficulty: o.difficulty.unwrap_or(s.difficulty),
                        skill: o.skill.unwrap_or(s.skill),
                        aggression: o.aggression.unwrap_or(s.aggression),
                        reaction_time: o.reaction_time.unwrap_or(s.reaction_time),
                        attack_delay: o.attack_delay.unwrap_or(s.attack_delay),
                        teamwork: o.teamwork.unwrap_or(s.teamwork),
                        weapon_preference: o
                            .weapon_preference
                            .unwrap_or_else(|| s.weapon_preference.clone()),
                    },
                }
            })
            .collect()
    }

    /// Team-level numeric descriptors used as model features.
    pub fn params(&self) -> TeamParams {
        let bots = self.resolved_bots();
        let n = bots.len() as f32;
        let mean = |f: &dyn Fn(&BotStyle) -> f32| bots.iter().map(|b| f(&b.style)).sum::<f32>() / n;
        let prefers = |w: &str| {
            bots.iter()
                .filter(|b| b.style.weapon_preference.first().is_some_and(|p| p == w))
                .count() as f32
        };
        TeamParams {
            difficulty: mean(&|s| s.difficulty.as_f32()),
            skill: mean(&|s| s.skill as f32),
            aggression: mean(&|s| s.aggression as f32),
            reaction_time: mean(&|s| s.reaction_time),
            attack_delay: mean(&|s| s.attack_delay),
            teamwork: mean(&|s| s.teamwork as f32),
            awpers: prefers("awp"),
        }
    }
}

/// Averaged team configuration, stored with every match and fed to the model.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct TeamParams {
    pub difficulty: f32,
    pub skill: f32,
    pub aggression: f32,
    pub reaction_time: f32,
    pub attack_delay: f32,
    pub teamwork: f32,
    /// Number of bots whose first weapon preference is the AWP.
    pub awpers: f32,
}

impl TeamParams {
    pub const NAMES: [&'static str; 7] = [
        "difficulty",
        "skill",
        "aggression",
        "reaction_time",
        "attack_delay",
        "teamwork",
        "awpers",
    ];

    pub fn to_array(&self) -> [f32; 7] {
        [
            self.difficulty,
            self.skill,
            self.aggression,
            self.reaction_time,
            self.attack_delay,
            self.teamwork,
            self.awpers,
        ]
    }
}

pub fn load_team(path: &Path) -> Result<Team, TeamError> {
    let p = path.display().to_string();
    let text = std::fs::read_to_string(path).map_err(|source| TeamError::Io { path: p.clone(), source })?;
    let team: Team = toml::from_str(&text).map_err(|source| TeamError::Parse { path: p, source })?;
    team.validate()?;
    Ok(team)
}

/// Loads every `*.toml` in `dir`, sorted by team name. Fails on duplicate names.
pub fn load_teams(dir: &Path) -> Result<Vec<Team>, TeamError> {
    let entries = std::fs::read_dir(dir).map_err(|source| TeamError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    let mut teams = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "toml") {
            teams.push(load_team(&path)?);
        }
    }
    teams.sort_by(|a, b| a.name.cmp(&b.name));
    for w in teams.windows(2) {
        if w[0].name == w[1].name {
            return Err(TeamError::Invalid(w[0].name.clone(), "duplicate team name".into()));
        }
    }
    Ok(teams)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample() -> Team {
        toml::from_str(
            r#"
            name = "AGGRO"
            [style]
            difficulty = "expert"
            skill = 90
            aggression = 80
            reaction_time = 0.2
            attack_delay = 0.1
            teamwork = 60
            weapon_preference = ["ak47", "m4a1"]

            [[bots]]
            weapon_preference = ["awp"]
            skill = 100
            "#,
        )
        .unwrap()
    }

    #[test]
    fn resolves_overrides() {
        let t = sample();
        t.validate().unwrap();
        let bots = t.resolved_bots();
        assert_eq!(bots.len(), 5);
        assert_eq!(bots[0].name, "AGGRO_1");
        assert_eq!(bots[0].style.skill, 100);
        assert_eq!(bots[0].style.weapon_preference, vec!["awp"]);
        assert_eq!(bots[1].style.skill, 90);
        let p = t.params();
        assert_eq!(p.awpers, 1.0);
        assert!((p.skill - 92.0).abs() < 1e-4);
    }

    #[test]
    fn rejects_bad_names() {
        let mut t = sample();
        t.name = "bad name".into();
        assert!(t.validate().is_err());
    }
}
