//! Parsed demo -> rounds, events, per-tick player states, labelled round-state samples
//! and the UI replay. Everything is derived from one timeline so the tables agree.

use crate::parse::{ParsedDemo, RawEvent, TICK_STRIDE, TickRow};
use features::RoundState;
use lab_core::TICK_RATE;
use lab_core::live::{
    BombSite, BombState, EventKind, GameEvent, LiveFrame, PlayerState, RoundPhase, Side, team_of_bot,
};
use lab_core::maps::MapInfo;
use lab_core::team::TeamParams;
use lab_core::weapons::Grenade;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

/// One labelled sample every 0.5 s of live round time.
pub const SAMPLE_EVERY_TICKS: i32 = 32;

pub struct MatchMeta {
    pub map: String,
    pub team_a: String,
    pub team_b: String,
    pub max_rounds: u32,
    pub params_a: TeamParams,
    pub params_b: TeamParams,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Round {
    pub number: u32,
    pub start_tick: i32,
    pub freeze_end_tick: i32,
    pub end_tick: i32,
    pub winner_side: Side,
    pub reason: String,
    pub team_ct: String,
    pub team_t: String,
    /// Score of the team on CT / T before this round.
    pub score_ct: u32,
    pub score_t: u32,
}

impl Round {
    pub fn winner_team(&self) -> &str {
        match self.winner_side {
            Side::Ct => &self.team_ct,
            Side::T => &self.team_t,
        }
    }
}

pub struct Sample {
    pub round: u32,
    pub tick: i32,
    pub team_ct: String,
    pub team_t: String,
    pub features: Vec<f32>,
    pub ct_won: bool,
}

pub struct TickSnapshot {
    pub round: u32,
    pub tick: i32,
    pub player: PlayerState,
}

#[derive(Serialize)]
pub struct Replay {
    pub tick_rate: f32,
    pub frames: Vec<LiveFrame>,
    pub events: Vec<GameEvent>,
}

pub struct Dataset {
    pub rounds: Vec<Round>,
    /// Events tagged with the round they happened in.
    pub events: Vec<(u32, GameEvent)>,
    pub ticks: Vec<TickSnapshot>,
    pub samples: Vec<Sample>,
    pub replay: Replay,
}

fn convar_secs(d: &ParsedDemo, names: &[&str], scale: f32, default: f32) -> f32 {
    names
        .iter()
        .find_map(|n| d.convars.get(*n).and_then(|v| v.parse::<f32>().ok()))
        .map(|v| v * scale)
        .unwrap_or(default)
}

fn side_of(row: &TickRow) -> Option<Side> {
    Side::from_team_num(row.team_num)
}

fn bomb_site(e: &RawEvent) -> Option<BombSite> {
    // m_nWhichBombZone of the planter: 1 = A, 2 = B (verify in the spike).
    match e.num("user_which_bomb_zone")? as i32 {
        1 => Some(BombSite::A),
        2 => Some(BombSite::B),
        _ => None,
    }
}

fn grenade_of_event(name: &str) -> Option<Grenade> {
    Some(match name {
        "hegrenade_detonate" => Grenade::He,
        "flashbang_detonate" => Grenade::Flash,
        "smokegrenade_detonate" => Grenade::Smoke,
        "inferno_startburn" => Grenade::Molotov,
        "decoy_started" => Grenade::Decoy,
        _ => return None,
    })
}

/// Raw round_end events of the real match: the last run starting at round 1, so rounds
/// played before an `mp_restartgame` are dropped.
fn round_ends(events: &[RawEvent]) -> Vec<(u32, i32, Side, String)> {
    let all: Vec<(u32, i32, Side, String)> = events
        .iter()
        .filter(|e| e.name == "round_end")
        .filter_map(|e| {
            let side = match e.str("winner")? {
                "CT" => Side::Ct,
                "T" => Side::T,
                _ => return None,
            };
            let round = e.num("round")? as u32 + 1;
            Some((round, e.tick, side, e.str("reason").unwrap_or("unknown").to_string()))
        })
        .collect();
    let start = all.iter().rposition(|r| r.0 == 1).unwrap_or(0);
    all[start..].to_vec()
}

fn majority_team<'a>(rows: impl Iterator<Item = &'a TickRow>) -> Option<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for r in rows {
        *counts.entry(team_of_bot(&r.name)).or_default() += 1;
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(t, _)| t.to_string())
}

pub fn build(demo: &ParsedDemo, meta: &MatchMeta, map: &MapInfo) -> anyhow::Result<Dataset> {
    let round_time = convar_secs(demo, &["mp_roundtime_defuse", "mp_roundtime"], 60.0, 115.0);
    let c4_time = convar_secs(demo, &["mp_c4timer"], 1.0, 40.0);

    let mut by_tick: BTreeMap<i32, Vec<&TickRow>> = BTreeMap::new();
    for row in &demo.ticks {
        by_tick.entry(row.tick).or_default().push(row);
    }
    let ticks_of = |name: &'static str| demo.events.iter().filter(move |e| e.name == name).map(|e| e.tick);
    let freeze_ends: Vec<i32> = ticks_of("round_freeze_end").collect();
    let starts: Vec<i32> = ticks_of("round_start").collect();

    // --- rounds
    let mut rounds = Vec::new();
    let mut wins: HashMap<String, u32> = HashMap::new();
    let mut prev_end = 0;
    for (number, end_tick, winner_side, reason) in round_ends(&demo.events) {
        let in_round = |t: &i32| *t > prev_end && *t < end_tick;
        let start_tick = starts.iter().copied().rfind(in_round).unwrap_or(prev_end);
        let freeze_end_tick = freeze_ends.iter().copied().rfind(in_round).unwrap_or(start_tick);
        let rows = || by_tick.range(freeze_end_tick..=end_tick).flat_map(|(_, rs)| rs.iter().copied());
        let team_on = |s: Side| majority_team(rows().filter(move |r| side_of(r) == Some(s)));
        let (Some(team_ct), Some(team_t)) = (team_on(Side::Ct), team_on(Side::T)) else {
            prev_end = end_tick;
            continue;
        };
        let round = Round {
            number,
            start_tick,
            freeze_end_tick,
            end_tick,
            winner_side,
            reason,
            score_ct: wins.get(&team_ct).copied().unwrap_or(0),
            score_t: wins.get(&team_t).copied().unwrap_or(0),
            team_ct,
            team_t,
        };
        *wins.entry(round.winner_team().to_string()).or_default() += 1;
        rounds.push(round);
        prev_end = end_tick;
    }
    anyhow::ensure!(!rounds.is_empty(), "no completed rounds found in demo");

    let round_at = |t: i32| -> Option<&Round> {
        // A tick belongs to the first round whose end is >= t and whose start is <= t.
        rounds.iter().find(|r| t >= r.start_tick && t <= r.end_tick)
    };

    // --- events
    let mut events: Vec<(u32, GameEvent)> = Vec::new();
    let mut plants: Vec<(i32, Option<BombSite>, Option<[f32; 3]>)> = Vec::new();
    let mut defuse_marks: Vec<(i32, bool)> = Vec::new(); // (tick, defusing started?)
    for e in &demo.events {
        let round_no = round_at(e.tick).map(|r| r.number);
        let name = |k: &str| e.str(k).map(str::to_string);
        let kind = match e.name.as_str() {
            "round_start" => round_no.map(|round| EventKind::RoundStart { round }),
            "round_freeze_end" => round_no.map(|round| EventKind::FreezeEnd { round }),
            "round_end" => rounds
                .iter()
                .find(|r| r.end_tick == e.tick)
                .map(|r| EventKind::RoundEnd { round: r.number, winner: r.winner_side, reason: r.reason.clone() }),
            "player_death" => name("user_name").map(|victim| EventKind::Kill {
                attacker: name("attacker_name"),
                victim,
                assister: name("assister_name"),
                weapon: name("weapon").unwrap_or_default(),
                headshot: e.bool("headshot"),
                attacker_pos: e.pos("attacker_"),
                victim_pos: e.pos("user_").unwrap_or_default(),
            }),
            "player_hurt" => name("user_name").map(|victim| EventKind::Hurt {
                attacker: name("attacker_name"),
                victim,
                weapon: name("weapon").unwrap_or_default(),
                damage: e.num("dmg_health").unwrap_or(0.0) as i32,
                armor_damage: e.num("dmg_armor").unwrap_or(0.0) as i32,
                hitgroup: e.num("hitgroup").unwrap_or(0.0) as i32,
            }),
            "bomb_planted" => {
                let (site, pos) = (bomb_site(e), e.pos("user_"));
                plants.push((e.tick, site, pos));
                Some(EventKind::BombPlanted { player: name("user_name").unwrap_or_default(), site, pos })
            }
            "bomb_begindefuse" => {
                defuse_marks.push((e.tick, true));
                None
            }
            "bomb_abortdefuse" => {
                defuse_marks.push((e.tick, false));
                None
            }
            "bomb_defused" => {
                defuse_marks.push((e.tick, false));
                Some(EventKind::BombDefused { player: name("user_name").unwrap_or_default(), site: bomb_site(e) })
            }
            "bomb_exploded" => Some(EventKind::BombExploded { site: bomb_site(e) }),
            "cs_win_panel_match" => rounds.last().map(|r| {
                let (ct, t) = (wins.get(&r.team_ct).copied().unwrap_or(0), wins.get(&r.team_t).copied().unwrap_or(0));
                EventKind::MatchEnd { score_ct: ct, score_t: t }
            }),
            other => grenade_of_event(other).map(|grenade| EventKind::GrenadeDetonate {
                thrower: name("user_name"),
                grenade,
                pos: [
                    e.num("x").unwrap_or(0.0) as f32,
                    e.num("y").unwrap_or(0.0) as f32,
                    e.num("z").unwrap_or(0.0) as f32,
                ],
            }),
        };
        if let (Some(kind), Some(r)) = (kind, round_no.or(rounds.last().map(|r| r.number))) {
            events.push((r, GameEvent { tick: e.tick, kind }));
        }
    }

    // --- per-tick states, samples and replay frames
    let mut ticks = Vec::new();
    let mut samples = Vec::new();
    let mut frames = Vec::new();
    for (&tick, rows) in &by_tick {
        let Some(round) = round_at(tick) else { continue };
        let players: Vec<PlayerState> = rows
            .iter()
            .filter_map(|r| {
                Some(PlayerState {
                    name: r.name.clone(),
                    side: side_of(r)?,
                    alive: r.alive,
                    hp: r.hp,
                    armor: r.armor,
                    helmet: r.helmet,
                    defuser: r.defuser,
                    money: r.money,
                    equip_value: r.equip_value,
                    pos: r.pos,
                    yaw: r.yaw,
                    pitch: r.pitch,
                    active_weapon: r.active_weapon.clone(),
                    weapons: if r.alive { r.inventory.clone() } else { Vec::new() },
                    flash: r.flash,
                })
            })
            .collect();
        let live = tick >= round.freeze_end_tick && tick < round.end_tick;
        let phase = if tick < round.freeze_end_tick {
            RoundPhase::Freezetime
        } else if live {
            RoundPhase::Live
        } else {
            RoundPhase::Over
        };
        let elapsed = (tick - round.freeze_end_tick).max(0) as f32 / TICK_RATE;
        let round_time_left = (round_time - elapsed).clamp(0.0, round_time);
        let plant = plants.iter().rfind(|(t, ..)| *t >= round.start_tick && *t <= tick && *t <= round.end_tick);
        let bomb = match plant {
            Some(&(t, site, pos)) => BombState {
                planted: true,
                site,
                time_left: Some((c4_time - (tick - t) as f32 / TICK_RATE).max(0.0)),
                defusing: defuse_marks.iter().rfind(|(dt, _)| *dt >= t && *dt <= tick).is_some_and(|d| d.1),
                pos,
            },
            None => BombState {
                pos: players
                    .iter()
                    .find(|p| p.alive && p.weapons.iter().any(|w| w == "C4 Explosive"))
                    .map(|p| p.pos),
                ..Default::default()
            },
        };
        let frame = LiveFrame {
            tick,
            map: meta.map.clone(),
            round: round.number,
            phase,
            round_time_left,
            score_ct: round.score_ct,
            score_t: round.score_t,
            bomb,
            players,
        };
        if live && (tick - round.freeze_end_tick) % SAMPLE_EVERY_TICKS < TICK_STRIDE {
            let (team_ct, team_t) = if round.team_ct == meta.team_a {
                (meta.params_a, meta.params_b)
            } else {
                (meta.params_b, meta.params_a)
            };
            let state = RoundState {
                map: meta.map.clone(),
                round: round.number,
                max_rounds: meta.max_rounds,
                round_time_left: frame.round_time_left,
                score_ct: frame.score_ct,
                score_t: frame.score_t,
                bomb: frame.bomb.clone(),
                players: frame.players.clone(),
                team_ct,
                team_t,
            };
            samples.push(Sample {
                round: round.number,
                tick,
                team_ct: round.team_ct.clone(),
                team_t: round.team_t.clone(),
                features: features::extract(&state, map),
                ct_won: round.winner_side == Side::Ct,
            });
        }
        ticks.extend(frame.players.iter().map(|p| TickSnapshot { round: round.number, tick, player: p.clone() }));
        frames.push(frame);
    }

    Ok(Dataset {
        replay: Replay {
            tick_rate: TICK_RATE,
            frames,
            events: events.iter().map(|(_, e)| e.clone()).collect(),
        },
        rounds,
        events,
        ticks,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::Val;

    fn ev(name: &str, tick: i32, fields: &[(&str, Val)]) -> RawEvent {
        RawEvent {
            name: name.into(),
            tick,
            fields: fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        }
    }

    fn row(tick: i32, name: &str, team_num: i64) -> TickRow {
        TickRow {
            tick,
            name: name.into(),
            team_num,
            hp: 100,
            alive: true,
            inventory: vec!["AK-47".into()],
            ..Default::default()
        }
    }

    /// Two rounds; teams swap sides in round 2 (as at halftime with max_rounds = 2).
    fn demo() -> ParsedDemo {
        let mut ticks = Vec::new();
        for t in (0..=2000).step_by(8) {
            let (red, blue) = if t < 1000 { (3, 2) } else { (2, 3) };
            ticks.push(row(t, "RED_1", red));
            ticks.push(row(t, "BLUE_1", blue));
        }
        let s = |x: &str| Val::Str(x.into());
        ParsedDemo {
            map: "de_dust2".into(),
            convars: HashMap::new(),
            ticks,
            events: vec![
                // a warmup round before mp_restartgame, must be ignored
                ev("round_end", 50, &[("round", Val::Num(0.0)), ("winner", s("T")), ("reason", s("x"))]),
                ev("round_start", 100, &[]),
                ev("round_freeze_end", 200, &[]),
                ev("player_death", 600, &[("user_name", s("BLUE_1")), ("attacker_name", s("RED_1")),
                    ("weapon", s("ak47")), ("headshot", Val::Bool(true))]),
                ev("round_end", 900, &[("round", Val::Num(0.0)), ("winner", s("CT")), ("reason", s("t_killed"))]),
                ev("round_start", 1000, &[]),
                ev("round_freeze_end", 1100, &[]),
                ev("bomb_planted", 1500, &[("user_name", s("RED_1")), ("user_which_bomb_zone", Val::Num(2.0))]),
                ev("round_end", 1900, &[("round", Val::Num(1.0)), ("winner", s("T")), ("reason", s("bomb_exploded"))]),
            ],
        }
    }

    fn meta() -> MatchMeta {
        MatchMeta {
            map: "de_dust2".into(),
            team_a: "RED".into(),
            team_b: "BLUE".into(),
            max_rounds: 2,
            params_a: TeamParams { skill: 90.0, ..Default::default() },
            params_b: TeamParams { skill: 10.0, ..Default::default() },
        }
    }

    #[test]
    fn rounds_sides_and_scores() {
        let d = build(&demo(), &meta(), &MapInfo::default()).unwrap();
        assert_eq!(d.rounds.len(), 2, "warmup round must be dropped");
        let (r1, r2) = (&d.rounds[0], &d.rounds[1]);
        assert_eq!((r1.number, r1.team_ct.as_str(), r1.winner_team()), (1, "RED", "RED"));
        assert_eq!((r2.number, r2.team_ct.as_str(), r2.winner_team()), (2, "BLUE", "RED"));
        assert_eq!((r2.score_ct, r2.score_t), (0, 1), "RED (now T) leads 1-0");
        assert_eq!(r1.freeze_end_tick, 200);
    }

    #[test]
    fn samples_are_labelled_and_use_correct_team_params() {
        let d = build(&demo(), &meta(), &MapInfo::default()).unwrap();
        let names = features::feature_names();
        let idx = |k: &str| names.iter().position(|n| n == k).unwrap();
        let r1: Vec<_> = d.samples.iter().filter(|s| s.round == 1).collect();
        let r2: Vec<_> = d.samples.iter().filter(|s| s.round == 2).collect();
        // live from 200 to 900 = 700 ticks -> samples at 200, 232, ..., 872
        assert_eq!(r1.len(), 22);
        assert!(r1.iter().all(|s| s.ct_won && s.tick >= 200 && s.tick < 900));
        assert!(r2.iter().all(|s| !s.ct_won));
        assert_eq!(r1[0].features[idx("ct_team_skill")], 90.0);
        assert_eq!(r2[0].features[idx("ct_team_skill")], 10.0, "BLUE is CT in round 2");
        let planted = r2.iter().find(|s| s.tick >= 1500).unwrap();
        assert_eq!(planted.features[idx("bomb_site_b")], 1.0);
        assert!(r2.iter().find(|s| s.tick < 1500).unwrap().features[idx("bomb_planted")] == 0.0);
    }

    #[test]
    fn events_and_replay() {
        let d = build(&demo(), &meta(), &MapInfo::default()).unwrap();
        let kill = d.events.iter().find(|(_, e)| matches!(e.kind, EventKind::Kill { .. })).unwrap();
        assert_eq!(kill.0, 1);
        assert!(d.replay.frames.iter().all(|f| f.players.len() == 2));
        let f = d.replay.frames.iter().find(|f| f.tick == 1600).unwrap();
        assert!(f.bomb.planted);
        assert!((f.bomb.time_left.unwrap() - (40.0 - 100.0 / 64.0)).abs() < 1e-4);
        assert_eq!(d.replay.frames.iter().find(|f| f.tick == 104).unwrap().phase, RoundPhase::Freezetime);
    }
}
