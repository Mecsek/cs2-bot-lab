//! Wire types shared by the BotLab plugin (C#), the api and the UI.
//! JSON, internally tagged with `"type"`. Keep `plugin/BotLab/Messages.cs` and
//! `cs2-lab-ui/src/api/types.ts` in sync with this file.

use crate::weapons::Grenade;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    #[serde(rename = "CT")]
    Ct,
    #[serde(rename = "T")]
    T,
}

impl Side {
    pub fn other(self) -> Side {
        match self {
            Side::Ct => Side::T,
            Side::T => Side::Ct,
        }
    }
    /// CS team number: 2 = T, 3 = CT.
    pub fn from_team_num(n: i64) -> Option<Side> {
        match n {
            2 => Some(Side::T),
            3 => Some(Side::Ct),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundPhase {
    Warmup,
    Freezetime,
    Live,
    Over,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BombSite {
    A,
    B,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BombState {
    pub planted: bool,
    pub site: Option<BombSite>,
    /// Seconds until detonation while planted.
    pub time_left: Option<f32>,
    pub defusing: bool,
    /// Planted, dropped or carried position.
    pub pos: Option<[f32; 3]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerState {
    /// Bot name, `<TEAM>_<n>`; unique within a match.
    pub name: String,
    pub side: Side,
    pub alive: bool,
    pub hp: i32,
    pub armor: i32,
    pub helmet: bool,
    pub defuser: bool,
    pub money: i32,
    /// Game-computed value of the current loadout (m_unCurrentEquipmentValue).
    pub equip_value: i32,
    /// World position in Source units (Z up).
    pub pos: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub active_weapon: String,
    /// All carried weapons/grenades, any naming scheme (see `weapons::canonical`).
    pub weapons: Vec<String>,
    /// Remaining flash blindness, seconds.
    #[serde(default)]
    pub flash: f32,
}

impl PlayerState {
    /// Team identifier derived from the bot name prefix.
    pub fn team(&self) -> &str {
        team_of_bot(&self.name)
    }
}

/// `AGGRO_AWP_3` -> `AGGRO_AWP`
pub fn team_of_bot(name: &str) -> &str {
    match name.rsplit_once('_') {
        Some((team, n)) if n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty() => team,
        _ => name,
    }
}

/// One live snapshot of a server, sent ~16 times per second by the plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveFrame {
    pub tick: i32,
    pub map: String,
    /// 1-based round number.
    pub round: u32,
    pub phase: RoundPhase,
    /// Seconds left on the round clock (excluding the bomb timer).
    pub round_time_left: f32,
    pub score_ct: u32,
    pub score_t: u32,
    pub bomb: BombState,
    pub players: Vec<PlayerState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    RoundStart { round: u32 },
    FreezeEnd { round: u32 },
    RoundEnd { round: u32, winner: Side, reason: String },
    Kill {
        attacker: Option<String>,
        victim: String,
        assister: Option<String>,
        weapon: String,
        headshot: bool,
        attacker_pos: Option<[f32; 3]>,
        victim_pos: [f32; 3],
    },
    Hurt {
        attacker: Option<String>,
        victim: String,
        weapon: String,
        damage: i32,
        armor_damage: i32,
        hitgroup: i32,
    },
    BombPlanted {
        player: String,
        site: Option<BombSite>,
        /// Planter position; optional on the wire.
        #[serde(default)]
        pos: Option<[f32; 3]>,
    },
    BombDefused { player: String, site: Option<BombSite> },
    BombExploded { site: Option<BombSite> },
    GrenadeDetonate { thrower: Option<String>, grenade: Grenade, pos: [f32; 3] },
    MatchEnd { score_ct: u32, score_t: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameEvent {
    pub tick: i32,
    #[serde(flatten)]
    pub kind: EventKind,
}

/// Plugin -> api.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginMessage {
    Hello { server_id: String, map: String, plugin_version: String },
    Frame(LiveFrame),
    Event(GameEvent),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchInfo {
    pub id: Uuid,
    pub worker: String,
    pub map: String,
    /// Starts on CT; sides swap at halftime.
    pub team_a: String,
    pub team_b: String,
    pub status: String,
}

/// api -> UI over `/ws/live/<match_id>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiMessage {
    /// Sent once on connect.
    Hello { info: MatchInfo, frame: Option<LiveFrame>, recent_events: Vec<GameEvent> },
    Frame(LiveFrame),
    Event(GameEvent),
    /// Not produced by the api: a slot for your own model. The UI charts it when present.
    WinProb { tick: i32, round: u32, p_ct: f32 },
    Status { status: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_prefix() {
        assert_eq!(team_of_bot("AGGRO_AWP_3"), "AGGRO_AWP");
        assert_eq!(team_of_bot("SOLO"), "SOLO");
        assert_eq!(team_of_bot("X_Y"), "X_Y");
    }

    #[test]
    fn plugin_json_roundtrip() {
        let json = r#"{"type":"event","tick":100,"kind":"kill","attacker":"A_1","victim":"B_2",
            "assister":null,"weapon":"weapon_ak47","headshot":true,
            "attacker_pos":[1,2,3],"victim_pos":[4,5,6]}"#;
        let m: PluginMessage = serde_json::from_str(json).unwrap();
        let PluginMessage::Event(e) = &m else { panic!() };
        assert_eq!(e.tick, 100);
        assert!(matches!(e.kind, EventKind::Kill { headshot: true, .. }));
        let back: PluginMessage = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);

        let frame = r#"{"type":"frame","tick":5,"map":"de_dust2","round":1,"phase":"live",
            "round_time_left":100.0,"score_ct":0,"score_t":0,
            "bomb":{"planted":false,"site":null,"time_left":null,"defusing":false,"pos":null},
            "players":[{"name":"A_1","side":"CT","alive":true,"hp":100,"armor":100,"helmet":true,
            "defuser":false,"money":800,"equip_value":1200,"pos":[0,0,0],"yaw":90,"pitch":0,
            "active_weapon":"weapon_usp_silencer","weapons":["weapon_knife","weapon_usp_silencer"]}]}"#;
        let m: PluginMessage = serde_json::from_str(frame).unwrap();
        assert!(matches!(m, PluginMessage::Frame(f) if f.players[0].side == Side::Ct));
    }
}
