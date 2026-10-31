use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct LabConfig {
    pub teams_dir: PathBuf,
    pub max_rounds: u32,
    pub match_timeout_minutes: u64,
    pub demo_subdir: String,
    pub maps: Vec<String>,
    pub sampler: SamplerConfig,
    pub watchdog: WatchdogConfig,
    pub workers: Vec<WorkerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SamplerConfig {
    pub balance_exponent: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WatchdogConfig {
    pub restart_after_failed_pings: u32,
    pub restart_via_docker: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkerConfig {
    /// Must equal the plugin's BOTLAB_SERVER_ID on that server.
    pub id: String,
    pub rcon: String,
    pub container: String,
}

impl LabConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let cfg: LabConfig = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if cfg.maps.is_empty() {
            bail!("lab config: `maps` is empty");
        }
        if cfg.workers.is_empty() {
            bail!("lab config: no workers");
        }
        if cfg.max_rounds == 0 || !cfg.max_rounds.is_multiple_of(2) {
            bail!("lab config: max_rounds must be even and > 0");
        }
        if !cfg.demo_subdir.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            bail!("lab config: demo_subdir must be [A-Za-z0-9_]");
        }
        Ok(cfg)
    }
}
