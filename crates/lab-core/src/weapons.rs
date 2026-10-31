//! Weapon name normalisation. The demo parser reports display names ("AK-47") while the
//! live plugin reports designer names ("weapon_ak47"); both map to one canonical key.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeaponClass {
    Pistol,
    Smg,
    Rifle,
    Sniper,
    Heavy,
    Grenade,
    Taser,
    Bomb,
    Knife,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grenade {
    He,
    Flash,
    Smoke,
    Molotov,
    Decoy,
}

pub struct WeaponInfo {
    /// Designer name without `weapon_`.
    pub key: &'static str,
    pub display: &'static str,
    pub class: WeaponClass,
}

macro_rules! w {
    ($k:literal, $d:literal, $c:ident) => {
        WeaponInfo { key: $k, display: $d, class: WeaponClass::$c }
    };
}

pub static WEAPONS: &[WeaponInfo] = &[
    w!("glock", "Glock-18", Pistol),
    w!("usp_silencer", "USP-S", Pistol),
    w!("hkp2000", "P2000", Pistol),
    w!("p250", "P250", Pistol),
    w!("elite", "Dual Berettas", Pistol),
    w!("fiveseven", "Five-SeveN", Pistol),
    w!("tec9", "Tec-9", Pistol),
    w!("cz75a", "CZ75-Auto", Pistol),
    w!("deagle", "Desert Eagle", Pistol),
    w!("revolver", "R8 Revolver", Pistol),
    w!("mac10", "MAC-10", Smg),
    w!("mp9", "MP9", Smg),
    w!("mp7", "MP7", Smg),
    w!("mp5sd", "MP5-SD", Smg),
    w!("ump45", "UMP-45", Smg),
    w!("p90", "P90", Smg),
    w!("bizon", "PP-Bizon", Smg),
    w!("galilar", "Galil AR", Rifle),
    w!("famas", "FAMAS", Rifle),
    w!("ak47", "AK-47", Rifle),
    w!("m4a1", "M4A4", Rifle),
    w!("m4a1_silencer", "M4A1-S", Rifle),
    w!("sg556", "SG 553", Rifle),
    w!("aug", "AUG", Rifle),
    w!("ssg08", "SSG 08", Sniper),
    w!("awp", "AWP", Sniper),
    w!("g3sg1", "G3SG1", Sniper),
    w!("scar20", "SCAR-20", Sniper),
    w!("nova", "Nova", Heavy),
    w!("xm1014", "XM1014", Heavy),
    w!("mag7", "MAG-7", Heavy),
    w!("sawedoff", "Sawed-Off", Heavy),
    w!("m249", "M249", Heavy),
    w!("negev", "Negev", Heavy),
    w!("hegrenade", "High Explosive Grenade", Grenade),
    w!("flashbang", "Flashbang", Grenade),
    w!("smokegrenade", "Smoke Grenade", Grenade),
    w!("molotov", "Molotov", Grenade),
    w!("incgrenade", "Incendiary Grenade", Grenade),
    w!("decoy", "Decoy Grenade", Grenade),
    w!("taser", "Zeus x27", Taser),
    w!("c4", "C4 Explosive", Bomb),
];

/// Looks up a weapon by designer name (with or without `weapon_`) or display name.
pub fn lookup(name: &str) -> Option<&'static WeaponInfo> {
    let n = name.trim();
    let key = n.strip_prefix("weapon_").unwrap_or(n);
    WEAPONS
        .iter()
        .find(|w| w.key.eq_ignore_ascii_case(key) || w.display.eq_ignore_ascii_case(n))
}

/// Canonical key; knives collapse to "knife", unknown names pass through lower-cased.
pub fn canonical(name: &str) -> String {
    if let Some(w) = lookup(name) {
        return w.key.to_string();
    }
    if is_knife(name) {
        return "knife".to_string();
    }
    name.trim().trim_start_matches("weapon_").to_ascii_lowercase()
}

fn is_knife(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.contains("knife") || l.contains("bayonet") || l.contains("karambit") || l.contains("dagger")
}

pub fn class_of(name: &str) -> WeaponClass {
    match lookup(name) {
        Some(w) => w.class,
        None if is_knife(name) => WeaponClass::Knife,
        None => WeaponClass::Other,
    }
}

pub fn grenade_of(name: &str) -> Option<Grenade> {
    match lookup(name)?.key {
        "hegrenade" => Some(Grenade::He),
        "flashbang" => Some(Grenade::Flash),
        "smokegrenade" => Some(Grenade::Smoke),
        "molotov" | "incgrenade" => Some(Grenade::Molotov),
        "decoy" => Some(Grenade::Decoy),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_naming_schemes_agree() {
        assert_eq!(canonical("AK-47"), "ak47");
        assert_eq!(canonical("weapon_ak47"), "ak47");
        assert_eq!(canonical("M4A1-S"), "m4a1_silencer");
        assert_eq!(canonical("Bowie Knife"), "knife");
        assert_eq!(canonical("knife_t"), "knife");
        assert_eq!(class_of("weapon_awp"), WeaponClass::Sniper);
        assert_eq!(class_of("SSG 08"), WeaponClass::Sniper);
        assert_eq!(grenade_of("Incendiary Grenade"), Some(Grenade::Molotov));
        assert_eq!(grenade_of("weapon_smokegrenade"), Some(Grenade::Smoke));
        assert_eq!(class_of("C4 Explosive"), WeaponClass::Bomb);
    }
}
