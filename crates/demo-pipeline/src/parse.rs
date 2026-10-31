//! Thin adapter over demoparser's Rust core: turns its column store into plain rows.

use ahash::AHashMap;
use anyhow::{Result, anyhow};
use demoparser::first_pass::parser_settings::ParserInputs;
use demoparser::maps::FRIENDLY_NAMES_MAPPING;
use demoparser::parse_demo::{DemoOutput, Parser, ParsingMode};
use demoparser::second_pass::parser_settings::create_huffman_lookup_table;
use demoparser::second_pass::variants::{VarVec, Variant};
use std::collections::HashMap;

/// Player state is sampled every `TICK_STRIDE` ticks (8 = 8 Hz at 64 tick).
pub const TICK_STRIDE: i32 = 8;
const MAX_TICK: i32 = 64 * 60 * 60 * 4;

const TICK_PROPS: &[&str] = &[
    "X", "Y", "Z", "pitch", "yaw", "health", "armor_value", "has_helmet", "has_defuser", "balance",
    "current_equip_value", "inventory", "active_weapon_name", "is_alive", "team_num", "flash_duration",
];

pub const EVENTS: &[&str] = &[
    "round_start", "round_freeze_end", "round_end", "player_death", "player_hurt", "bomb_planted",
    "bomb_begindefuse", "bomb_abortdefuse", "bomb_defused", "bomb_exploded", "hegrenade_detonate", "flashbang_detonate", "smokegrenade_detonate",
    "inferno_startburn", "decoy_started", "cs_win_panel_match",
];
const EVENT_PLAYER_PROPS: &[&str] = &["X", "Y", "Z", "which_bomb_zone"];

#[derive(Debug, Clone, Default)]
pub struct TickRow {
    pub tick: i32,
    pub name: String,
    pub team_num: i64,
    pub pos: [f32; 3],
    pub pitch: f32,
    pub yaw: f32,
    pub hp: i32,
    pub armor: i32,
    pub helmet: bool,
    pub defuser: bool,
    pub money: i32,
    pub equip_value: i32,
    pub inventory: Vec<String>,
    pub active_weapon: String,
    pub alive: bool,
    pub flash: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Str(String),
    Num(f64),
    Bool(bool),
}

#[derive(Debug, Clone)]
pub struct RawEvent {
    pub name: String,
    pub tick: i32,
    pub fields: HashMap<String, Val>,
}

impl RawEvent {
    pub fn str(&self, k: &str) -> Option<&str> {
        match self.fields.get(k)? {
            Val::Str(s) if !s.is_empty() => Some(s),
            _ => None,
        }
    }
    pub fn num(&self, k: &str) -> Option<f64> {
        match self.fields.get(k)? {
            Val::Num(n) => Some(*n),
            Val::Bool(b) => Some(*b as u8 as f64),
            Val::Str(s) => s.parse().ok(),
        }
    }
    pub fn bool(&self, k: &str) -> bool {
        match self.fields.get(k) {
            Some(Val::Bool(b)) => *b,
            Some(Val::Num(n)) => *n != 0.0,
            _ => false,
        }
    }
    pub fn pos(&self, prefix: &str) -> Option<[f32; 3]> {
        let p = |a: &str| self.num(&format!("{prefix}{a}")).map(|v| v as f32);
        Some([p("X")?, p("Y")?, p("Z")?])
    }
}

pub struct ParsedDemo {
    pub map: String,
    pub convars: HashMap<String, String>,
    pub ticks: Vec<TickRow>,
    pub events: Vec<RawEvent>,
}

fn real_names(friendly: &[&str]) -> Result<(Vec<String>, AHashMap<String, String>)> {
    let mut real = Vec::new();
    let mut back = AHashMap::default();
    for f in friendly {
        let r = FRIENDLY_NAMES_MAPPING.get(f).ok_or_else(|| anyhow!("unknown demoparser prop `{f}`"))?;
        real.push(r.to_string());
        back.insert(r.to_string(), f.to_string());
    }
    Ok((real, back))
}

fn run(bytes: &[u8], huf: &Vec<(u8, u8)>, events: bool) -> Result<DemoOutput> {
    let (props, back) = real_names(if events { EVENT_PLAYER_PROPS } else { TICK_PROPS })?;
    let inputs = ParserInputs {
        real_name_to_og_name: back,
        wanted_players: vec![],
        wanted_player_props: props,
        wanted_other_props: vec![],
        wanted_prop_states: AHashMap::default(),
        // demoparser only collects tick rows when no events are requested, so this is two passes.
        wanted_ticks: if events { vec![] } else { (0..MAX_TICK).step_by(TICK_STRIDE as usize).collect() },
        wanted_events: if events { EVENTS.iter().map(|s| s.to_string()).collect() } else { vec![] },
        parse_ents: true,
        parse_projectiles: false,
        parse_grenades: false,
        only_header: false,
        only_convars: false,
        huffman_lookup_table: huf,
        order_by_steamid: false,
        list_props: false,
        fallback_bytes: None,
    };
    Parser::new(inputs, ParsingMode::Normal).parse_demo(bytes).map_err(|e| anyhow!("demoparser: {e:?}"))
}

pub fn parse(bytes: &[u8]) -> Result<ParsedDemo> {
    let huf = create_huffman_lookup_table();
    let tick_out = run(bytes, &huf, false)?;
    let event_out = run(bytes, &huf, true)?;
    let map = event_out
        .header
        .as_ref()
        .and_then(|h| h.get("map_name").cloned())
        .unwrap_or_default();
    Ok(ParsedDemo {
        map,
        convars: event_out.convars.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        ticks: tick_rows(&tick_out)?,
        events: event_out.game_events.iter().map(raw_event).collect(),
    })
}

fn raw_event(e: &demoparser::second_pass::game_events::GameEvent) -> RawEvent {
    let fields = e
        .fields
        .iter()
        .filter_map(|f| {
            let v = match f.data.as_ref()? {
                Variant::String(s) => Val::Str(s.clone()),
                Variant::Bool(b) => Val::Bool(*b),
                Variant::F32(x) => Val::Num(*x as f64),
                Variant::I32(x) => Val::Num(*x as f64),
                Variant::U32(x) => Val::Num(*x as f64),
                Variant::U64(x) => Val::Num(*x as f64),
                _ => return None,
            };
            Some((f.name.clone(), v))
        })
        .collect();
    RawEvent { name: e.name.clone(), tick: e.tick, fields }
}

struct Columns<'a>(HashMap<&'a str, &'a VarVec>);

impl<'a> Columns<'a> {
    fn get(&self, name: &str) -> Result<&'a VarVec> {
        self.0.get(name).copied().ok_or_else(|| anyhow!("demo has no `{name}` column"))
    }
    fn num(&self, name: &str, i: usize) -> Option<f64> {
        match self.0.get(name)? {
            VarVec::F32(v) => v.get(i).copied().flatten().map(|x| x as f64),
            VarVec::I32(v) => v.get(i).copied().flatten().map(|x| x as f64),
            VarVec::U32(v) => v.get(i).copied().flatten().map(|x| x as f64),
            VarVec::U64(v) => v.get(i).copied().flatten().map(|x| x as f64),
            VarVec::Bool(v) => v.get(i).copied().flatten().map(|x| x as u8 as f64),
            _ => None,
        }
    }
    fn bool(&self, name: &str, i: usize) -> bool {
        self.num(name, i).is_some_and(|x| x != 0.0)
    }
    fn string(&self, name: &str, i: usize) -> String {
        match self.0.get(name) {
            Some(VarVec::String(v)) => v.get(i).cloned().flatten().unwrap_or_default(),
            _ => String::new(),
        }
    }
    fn strings(&self, name: &str, i: usize) -> Vec<String> {
        match self.0.get(name) {
            Some(VarVec::StringVec(v)) => v.get(i).cloned().unwrap_or_default(),
            _ => Vec::new(),
        }
    }
}

fn tick_rows(out: &DemoOutput) -> Result<Vec<TickRow>> {
    let mut cols = HashMap::new();
    for info in &out.prop_controller.prop_infos {
        if let Some(col) = out.df.get(&info.id).and_then(|c| c.data.as_ref()) {
            cols.insert(info.prop_friendly_name.as_str(), col);
        }
    }
    let cols = Columns(cols);
    let rows = match cols.get("tick")? {
        VarVec::I32(v) => v.len(),
        other => return Err(anyhow!("unexpected tick column type {other:?}")),
    };
    let f = |n: &str, i| cols.num(n, i).unwrap_or(0.0) as f32;
    let n = |n: &str, i| cols.num(n, i).unwrap_or(0.0) as i32;
    Ok((0..rows)
        .map(|i| TickRow {
            tick: n("tick", i),
            name: cols.string("name", i),
            team_num: n("team_num", i) as i64,
            pos: [f("X", i), f("Y", i), f("Z", i)],
            pitch: f("pitch", i),
            yaw: f("yaw", i),
            hp: n("health", i),
            armor: n("armor_value", i),
            helmet: cols.bool("has_helmet", i),
            defuser: cols.bool("has_defuser", i),
            money: n("balance", i),
            equip_value: n("current_equip_value", i),
            inventory: cols.strings("inventory", i),
            active_weapon: cols.string("active_weapon_name", i),
            alive: cols.bool("is_alive", i),
            flash: f("flash_duration", i),
        })
        .filter(|r| !r.name.is_empty())
        .collect())
}
