//! Drives one CS2 server: set up a match over RCON, wait for it to end, repeat.

use crate::config::{LabConfig, WorkerConfig};
use crate::{db, sampler};
use anyhow::{Context, Result, anyhow, bail};
use lab_core::team::Team;
use rcon::Connection;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::time::sleep;
use tracing::{error, info, warn};
use uuid::Uuid;

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const PING_INTERVAL: Duration = Duration::from_secs(30);
const MAP_LOAD_TIMEOUT: Duration = Duration::from_secs(120);
const BOOT_GRACE: Duration = Duration::from_secs(180);

pub struct Shared {
    pub cfg: LabConfig,
    pub teams: HashMap<String, Team>,
    pub pool: PgPool,
    pub rcon_password: String,
    /// Serialises sampling + insert so parallel workers see each other's picks.
    pub sample_lock: tokio::sync::Mutex<()>,
}

/// RCON connection that reconnects lazily after any error.
struct Rcon {
    addr: String,
    password: String,
    conn: Option<Connection<TcpStream>>,
}

impl Rcon {
    async fn cmd(&mut self, cmd: &str) -> Result<String> {
        if self.conn.is_none() {
            let conn = tokio::time::timeout(
                Duration::from_secs(10),
                Connection::<TcpStream>::builder().connect(&self.addr, &self.password),
            )
            .await
            .map_err(|_| anyhow!("rcon connect timeout"))?
            .with_context(|| format!("rcon connect {}", self.addr))?;
            self.conn = Some(conn);
        }
        let conn = self.conn.as_mut().expect("connected above");
        match tokio::time::timeout(Duration::from_secs(15), conn.cmd(cmd)).await {
            Ok(Ok(out)) => Ok(out),
            Ok(Err(e)) => {
                self.conn = None;
                Err(anyhow!("rcon `{cmd}`: {e}"))
            }
            Err(_) => {
                self.conn = None;
                Err(anyhow!("rcon `{cmd}`: timeout"))
            }
        }
    }

    async fn cmds(&mut self, cmds: &[String]) -> Result<()> {
        for c in cmds {
            self.cmd(c).await?;
        }
        Ok(())
    }
}

/// RCON commands that set up a fresh match. `demo_name` is relative to game/csgo, no extension.
pub fn setup_commands(team_a: &Team, team_b: &Team, max_rounds: u32, demo_name: &str) -> Vec<String> {
    let mut cmds = vec![
        "exec botlab".to_string(),
        format!("mp_maxrounds {max_rounds}"),
        "bot_kick".to_string(),
    ];
    for (team, side) in [(team_a, "ct"), (team_b, "t")] {
        for bot in team.resolved_bots() {
            cmds.push(format!("bot_add_{side} {} \"{}\"", bot.style.difficulty.cvar_arg(), bot.name));
        }
    }
    cmds.push("mp_warmup_end".to_string());
    cmds.push("mp_restartgame 1".to_string());
    cmds.push(format!("tv_record \"{demo_name}\""));
    cmds
}

pub async fn run(shared: Arc<Shared>, wc: WorkerConfig) {
    let mut rcon = Rcon { addr: wc.rcon.clone(), password: shared.rcon_password.clone(), conn: None };
    match db::fail_orphans(&shared.pool, &wc.id).await {
        Ok(n) if n > 0 => warn!(worker = wc.id, n, "marked orphaned running matches as failed"),
        Ok(_) => {}
        Err(e) => error!(worker = wc.id, "fail_orphans: {e:#}"),
    }
    loop {
        if let Err(e) = wait_ready(&mut rcon, &wc).await {
            warn!(worker = wc.id, "server not ready: {e:#}");
            continue;
        }
        if let Err(e) = play_one(&shared, &wc, &mut rcon).await {
            error!(worker = wc.id, "match loop error: {e:#}");
            sleep(Duration::from_secs(10)).await;
        }
    }
}

async fn wait_ready(rcon: &mut Rcon, wc: &WorkerConfig) -> Result<()> {
    let start = Instant::now();
    loop {
        match rcon.cmd("status").await {
            Ok(_) => return Ok(()),
            Err(e) if start.elapsed() > BOOT_GRACE => bail!("{e:#}"),
            Err(_) => {
                info!(worker = wc.id, "waiting for rcon");
                sleep(POLL_INTERVAL).await;
            }
        }
    }
}

async fn play_one(shared: &Shared, wc: &WorkerConfig, rcon: &mut Rcon) -> Result<()> {
    let cfg = &shared.cfg;
    let id = Uuid::new_v4();
    let demo_name = format!("{}/match_{}", cfg.demo_subdir, id.simple());
    let (matchup, team_a, team_b) = {
        let _guard = shared.sample_lock.lock().await;
        let counts = db::play_counts(&shared.pool).await?;
        let mut names: Vec<String> = shared.teams.keys().cloned().collect();
        names.sort();
        let matchup = sampler::sample(&mut rand::rng(), &names, &cfg.maps, &counts, cfg.sampler.balance_exponent)
            .ok_or_else(|| anyhow!("need at least two teams"))?;
        let team_a = shared.teams[&matchup.team_a].clone();
        let team_b = shared.teams[&matchup.team_b].clone();
        db::insert_match(
            &shared.pool,
            &db::NewMatch {
                id,
                worker: &wc.id,
                matchup: &matchup,
                team_a: &team_a,
                team_b: &team_b,
                max_rounds: cfg.max_rounds,
                demo_file: &format!("{demo_name}.dem"),
            },
        )
        .await?;
        (matchup, team_a, team_b)
    };
    info!(worker = wc.id, %id, map = matchup.map, a = matchup.team_a, b = matchup.team_b, "starting match");

    let result = async {
        start_match(rcon, &matchup.map, &team_a, &team_b, cfg.max_rounds, &demo_name).await?;
        wait_for_end(shared, wc, rcon, id).await
    }
    .await;

    // Always try to close the demo so the file is finalised.
    let _ = rcon.cmd("tv_stoprecord").await;
    match result {
        Ok(()) => {
            // Give the server a moment to flush the demo before the pipeline picks it up.
            sleep(Duration::from_secs(5)).await;
            db::mark_finished(&shared.pool, id).await?;
            info!(worker = wc.id, %id, "match finished");
        }
        Err(e) => {
            db::mark_failed(&shared.pool, id, &format!("{e:#}")).await?;
            warn!(worker = wc.id, %id, "match failed: {e:#}");
        }
    }
    Ok(())
}

async fn start_match(
    rcon: &mut Rcon,
    map: &str,
    team_a: &Team,
    team_b: &Team,
    max_rounds: u32,
    demo_name: &str,
) -> Result<()> {
    rcon.cmd("tv_stoprecord").await?;
    rcon.cmd(&format!("changelevel {map}")).await?;
    // The connection usually survives a level change, but the server is unresponsive while
    // loading; poll until `status` reports the new map.
    let start = Instant::now();
    loop {
        sleep(POLL_INTERVAL).await;
        if let Ok(out) = rcon.cmd("status").await
            && out.contains(map)
        {
            break;
        }
        if start.elapsed() > MAP_LOAD_TIMEOUT {
            bail!("map {map} did not load within {MAP_LOAD_TIMEOUT:?}");
        }
    }
    sleep(Duration::from_secs(5)).await;
    rcon.cmds(&setup_commands(team_a, team_b, max_rounds, demo_name)).await
}

async fn wait_for_end(shared: &Shared, wc: &WorkerConfig, rcon: &mut Rcon, id: Uuid) -> Result<()> {
    let timeout = Duration::from_secs(shared.cfg.match_timeout_minutes * 60);
    let start = Instant::now();
    let mut last_ping = Instant::now();
    let mut failed_pings = 0;
    loop {
        sleep(POLL_INTERVAL).await;
        if db::live_ended(&shared.pool, id).await? {
            return Ok(());
        }
        if start.elapsed() > timeout {
            bail!("match timed out after {timeout:?}");
        }
        if last_ping.elapsed() >= PING_INTERVAL {
            last_ping = Instant::now();
            match rcon.cmd("status").await {
                Ok(_) => failed_pings = 0,
                Err(e) => {
                    failed_pings += 1;
                    warn!(worker = wc.id, failed_pings, "rcon ping failed: {e:#}");
                }
            }
            if failed_pings >= shared.cfg.watchdog.restart_after_failed_pings {
                if shared.cfg.watchdog.restart_via_docker {
                    restart_container(&wc.container).await?;
                }
                bail!("server unresponsive ({failed_pings} failed pings)");
            }
        }
    }
}

async fn restart_container(name: &str) -> Result<()> {
    warn!(container = name, "restarting worker container");
    let docker = bollard::Docker::connect_with_local_defaults()?;
    docker
        .restart_container(name, None::<bollard::query_parameters::RestartContainerOptions>)
        .await
        .with_context(|| format!("restarting container {name}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn team(name: &str) -> Team {
        toml::from_str(&format!(
            r#"name = "{name}"
            [style]
            difficulty = "hard"
            skill = 50
            aggression = 50
            reaction_time = 0.3
            attack_delay = 0.0
            teamwork = 50"#
        ))
        .unwrap()
    }

    #[test]
    fn setup_adds_ten_named_bots_and_records() {
        let cmds = setup_commands(&team("RED"), &team("BLUE"), 24, "botlab_demos/match_x");
        assert_eq!(cmds.iter().filter(|c| c.starts_with("bot_add_ct hard \"RED_")).count(), 5);
        assert_eq!(cmds.iter().filter(|c| c.starts_with("bot_add_t hard \"BLUE_")).count(), 5);
        assert!(cmds.contains(&"mp_maxrounds 24".to_string()));
        // bots must be kicked before adding and recording must come last
        let kick = cmds.iter().position(|c| c == "bot_kick").unwrap();
        let first_add = cmds.iter().position(|c| c.starts_with("bot_add")).unwrap();
        assert!(kick < first_add);
        assert_eq!(cmds.last().unwrap(), "tv_record \"botlab_demos/match_x\"");
    }
}
