//! Round-state -> feature vector for the `round_state_samples` training table. Keep inference
//! on this same function so features computed live match the ones trained on.

use lab_core::live::{BombSite, BombState, PlayerState, Side};
use lab_core::maps::MapInfo;
use lab_core::team::TeamParams;
use lab_core::weapons::{self, Grenade, WeaponClass};
use serde::{Deserialize, Serialize};

/// Maps given a one-hot slot. Unknown maps encode as all zeros.
pub const MAP_POOL: [&str; 9] = [
    "de_ancient", "de_anubis", "de_dust2", "de_inferno", "de_mirage", "de_nuke", "de_overpass",
    "de_train", "de_vertigo",
];

/// Everything known about one moment of a round.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoundState {
    pub map: String,
    /// 1-based.
    pub round: u32,
    pub max_rounds: u32,
    pub round_time_left: f32,
    pub score_ct: u32,
    pub score_t: u32,
    pub bomb: BombState,
    pub players: Vec<PlayerState>,
    pub team_ct: TeamParams,
    pub team_t: TeamParams,
}

const SIDE_FEATURES: [&str; 20] = [
    "alive", "hp", "armor", "helmets", "defusers", "money", "equip_value", "rifles", "snipers",
    "smgs", "heavies", "pistol_only", "he", "flash", "smoke", "molotov", "flashed", "dist_a",
    "dist_b", "spread",
];

const GLOBAL_FEATURES: [&str; 13] = [
    "round_num", "is_pistol_round", "score_ct", "score_t", "round_time_left", "bomb_planted",
    "bomb_site_a", "bomb_site_b", "bomb_time_left", "bomb_defusing", "alive_diff", "hp_diff",
    "equip_diff",
];

/// Column names in the exact order produced by [`extract`].
pub fn feature_names() -> Vec<String> {
    let mut names: Vec<String> = GLOBAL_FEATURES.iter().map(|s| s.to_string()).collect();
    for side in ["ct", "t"] {
        names.extend(SIDE_FEATURES.iter().map(|f| format!("{side}_{f}")));
    }
    for side in ["ct", "t"] {
        names.extend(TeamParams::NAMES.iter().map(|f| format!("{side}_team_{f}")));
    }
    names.extend(MAP_POOL.iter().map(|m| format!("map_{m}")));
    names
}

pub fn feature_count() -> usize {
    GLOBAL_FEATURES.len() + 2 * SIDE_FEATURES.len() + 2 * TeamParams::NAMES.len() + MAP_POOL.len()
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn mean(xs: impl Iterator<Item = f32>) -> f32 {
    let (sum, n) = xs.fold((0.0, 0usize), |(s, n), x| (s + x, n + 1));
    if n == 0 { f32::NAN } else { sum / n as f32 }
}

fn side_features(players: &[&PlayerState], map: &MapInfo) -> [f32; 20] {
    let alive: Vec<&PlayerState> = players.iter().copied().filter(|p| p.alive).collect();
    let count = |f: &dyn Fn(&PlayerState) -> bool| alive.iter().filter(|p| f(p)).count() as f32;
    let sum = |f: &dyn Fn(&PlayerState) -> i32| alive.iter().map(|p| f(p) as f32).sum::<f32>();
    let has_class = |p: &PlayerState, c: WeaponClass| p.weapons.iter().any(|w| weapons::class_of(w) == c);
    let primary = |p: &PlayerState| {
        p.weapons.iter().any(|w| {
            matches!(
                weapons::class_of(w),
                WeaponClass::Rifle | WeaponClass::Sniper | WeaponClass::Smg | WeaponClass::Heavy
            )
        })
    };
    let nades = |g: Grenade| {
        alive
            .iter()
            .flat_map(|p| p.weapons.iter())
            .filter(|w| weapons::grenade_of(w) == Some(g))
            .count() as f32
    };
    let site_dist = |site: Option<[f32; 3]>| match site {
        Some(s) => mean(alive.iter().map(|p| dist(p.pos, s))),
        None => f32::NAN,
    };
    let spread = if alive.len() < 2 {
        f32::NAN
    } else {
        let mut ds = Vec::new();
        for i in 0..alive.len() {
            for j in i + 1..alive.len() {
                ds.push(dist(alive[i].pos, alive[j].pos));
            }
        }
        mean(ds.into_iter())
    };
    [
        alive.len() as f32,
        sum(&|p| p.hp),
        sum(&|p| p.armor),
        count(&|p| p.helmet),
        count(&|p| p.defuser),
        // Money and equipment of dead players still matter for the economy, but not for this
        // round; only alive players count.
        sum(&|p| p.money),
        sum(&|p| p.equip_value),
        count(&|p| has_class(p, WeaponClass::Rifle)),
        count(&|p| has_class(p, WeaponClass::Sniper)),
        count(&|p| has_class(p, WeaponClass::Smg)),
        count(&|p| has_class(p, WeaponClass::Heavy)),
        count(&|p| !primary(p)),
        nades(Grenade::He),
        nades(Grenade::Flash),
        nades(Grenade::Smoke),
        nades(Grenade::Molotov),
        count(&|p| p.flash > 0.5),
        site_dist(map.site_a),
        site_dist(map.site_b),
        spread,
    ]
}

/// Builds the feature vector; NaN marks "not applicable / unknown".
pub fn extract(state: &RoundState, map: &MapInfo) -> Vec<f32> {
    let by_side = |s: Side| state.players.iter().filter(|p| p.side == s).collect::<Vec<_>>();
    let ct = side_features(&by_side(Side::Ct), map);
    let t = side_features(&by_side(Side::T), map);
    let half = state.max_rounds / 2;
    let bomb = &state.bomb;
    let flag = |b: bool| if b { 1.0 } else { 0.0 };

    let mut v = Vec::with_capacity(feature_count());
    v.extend([
        state.round as f32,
        flag(state.round == 1 || state.round == half + 1),
        state.score_ct as f32,
        state.score_t as f32,
        state.round_time_left,
        flag(bomb.planted),
        flag(bomb.planted && bomb.site == Some(BombSite::A)),
        flag(bomb.planted && bomb.site == Some(BombSite::B)),
        if bomb.planted { bomb.time_left.unwrap_or(f32::NAN) } else { f32::NAN },
        flag(bomb.planted && bomb.defusing),
        ct[0] - t[0],
        ct[1] - t[1],
        ct[6] - t[6],
    ]);
    v.extend(ct);
    v.extend(t);
    v.extend(state.team_ct.to_array());
    v.extend(state.team_t.to_array());
    v.extend(MAP_POOL.iter().map(|m| flag(*m == state.map)));
    debug_assert_eq!(v.len(), feature_count());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(name: &str, side: Side, alive: bool, pos: [f32; 3], weapons: &[&str]) -> PlayerState {
        PlayerState {
            name: name.into(),
            side,
            alive,
            hp: if alive { 100 } else { 0 },
            armor: 100,
            helmet: true,
            defuser: side == Side::Ct,
            money: 1000,
            equip_value: 4000,
            pos,
            yaw: 0.0,
            pitch: 0.0,
            active_weapon: weapons.first().map(|s| s.to_string()).unwrap_or_default(),
            weapons: weapons.iter().map(|s| s.to_string()).collect(),
            flash: 0.0,
        }
    }

    fn state(players: Vec<PlayerState>) -> RoundState {
        RoundState {
            map: "de_dust2".into(),
            round: 13,
            max_rounds: 24,
            round_time_left: 60.0,
            score_ct: 5,
            score_t: 7,
            bomb: BombState::default(),
            players,
            team_ct: TeamParams::default(),
            team_t: TeamParams::default(),
        }
    }

    fn get(names: &[String], v: &[f32], key: &str) -> f32 {
        v[names.iter().position(|n| n == key).unwrap()]
    }

    #[test]
    fn names_match_vector() {
        let s = state(vec![]);
        assert_eq!(extract(&s, &MapInfo::default()).len(), feature_names().len());
        let names = feature_names();
        // Key columns of round_state_samples must not collide with feature names.
        for key in ["match_id", "round", "tick", "team_ct", "team_t", "ct_won"] {
            assert!(!names.iter().any(|n| n == key), "feature name collides with key column {key}");
        }
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "duplicate feature names");
    }

    #[test]
    fn counts_alive_and_equipment_from_either_naming_scheme() {
        let s = state(vec![
            // live-plugin naming
            player("A_1", Side::Ct, true, [0.0, 0.0, 0.0], &["weapon_m4a1_silencer", "weapon_smokegrenade"]),
            player("A_2", Side::Ct, false, [0.0, 0.0, 0.0], &["weapon_awp"]),
            // demo naming
            player("B_1", Side::T, true, [100.0, 0.0, 0.0], &["AK-47", "Flashbang", "Flashbang"]),
            player("B_2", Side::T, true, [0.0, 0.0, 0.0], &["Glock-18"]),
        ]);
        let map = MapInfo { site_a: Some([0.0, 0.0, 0.0]), site_b: None };
        let v = extract(&s, &map);
        let n = feature_names();
        assert_eq!(get(&n, &v, "ct_alive"), 1.0);
        assert_eq!(get(&n, &v, "ct_snipers"), 0.0, "dead AWPer must not count");
        assert_eq!(get(&n, &v, "ct_smoke"), 1.0);
        assert_eq!(get(&n, &v, "t_rifles"), 1.0);
        assert_eq!(get(&n, &v, "t_flash"), 2.0);
        assert_eq!(get(&n, &v, "t_pistol_only"), 1.0);
        assert_eq!(get(&n, &v, "alive_diff"), -1.0);
        assert_eq!(get(&n, &v, "t_dist_a"), 50.0);
        assert_eq!(get(&n, &v, "t_spread"), 100.0);
        assert!(get(&n, &v, "t_dist_b").is_nan());
        assert!(get(&n, &v, "ct_spread").is_nan());
        assert_eq!(get(&n, &v, "is_pistol_round"), 1.0);
        assert_eq!(get(&n, &v, "map_de_dust2"), 1.0);
        assert!(get(&n, &v, "bomb_time_left").is_nan());
    }

    #[test]
    fn bomb_features() {
        let mut s = state(vec![]);
        s.bomb = BombState { planted: true, site: Some(BombSite::B), time_left: Some(30.0), defusing: true, pos: None };
        let v = extract(&s, &MapInfo::default());
        let n = feature_names();
        assert_eq!(get(&n, &v, "bomb_site_b"), 1.0);
        assert_eq!(get(&n, &v, "bomb_site_a"), 0.0);
        assert_eq!(get(&n, &v, "bomb_time_left"), 30.0);
        assert_eq!(get(&n, &v, "bomb_defusing"), 1.0);
    }
}
