//! Per-map metadata from `configs/maps.toml`.
//!
//! Bombsite centres are not hard-coded: `demo-pipeline calibrate-sites` averages observed
//! plant positions and writes them here. Until then site-distance features are NaN.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MapInfo {
    pub site_a: Option<[f32; 3]>,
    pub site_b: Option<[f32; 3]>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MapsConfig(pub BTreeMap<String, MapInfo>);

impl MapsConfig {
    pub fn load(path: &Path) -> std::io::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    pub fn get(&self, map: &str) -> MapInfo {
        self.0.get(map).cloned().unwrap_or_default()
    }
}
