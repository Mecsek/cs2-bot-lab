mod config;
mod db;
mod sampler;
mod worker;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(about = "Runs bot-vs-bot CS2 matches across a pool of servers")]
struct Cli {
    #[arg(long, env = "LAB_CONFIG", default_value = "configs/lab.toml")]
    config: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Sync teams to the database and run matches on every worker until stopped.
    Run {
        #[arg(long, env = "DATABASE_URL")]
        database_url: String,
        #[arg(long, env = "RCON_PASSWORD", hide_env_values = true)]
        rcon_password: String,
    },
    /// Render configs/teams into a botprofile.db for the worker image.
    GenBotprofile {
        #[arg(long, default_value = "docker/worker/botprofile.db")]
        out: PathBuf,
    },
    /// Validate team files and print each team's averaged parameters.
    CheckTeams,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    let cfg = config::LabConfig::load(&cli.config)?;
    let teams = lab_core::team::load_teams(&cfg.teams_dir)?;

    match cli.cmd {
        Cmd::GenBotprofile { out } => {
            std::fs::write(&out, lab_core::botprofile::render(&teams))
                .with_context(|| format!("writing {}", out.display()))?;
            println!("wrote {} teams to {}", teams.len(), out.display());
        }
        Cmd::CheckTeams => {
            for t in &teams {
                let p = t.params();
                println!(
                    "{:<20} skill {:>5.1}  aggr {:>5.1}  react {:.2}s  teamwork {:>5.1}  awpers {}",
                    t.name, p.skill, p.aggression, p.reaction_time, p.teamwork, p.awpers
                );
            }
            println!("{} teams OK", teams.len());
        }
        Cmd::Run { database_url, rcon_password } => {
            anyhow::ensure!(teams.len() >= 2, "need at least two teams in {}", cfg.teams_dir.display());
            let pool = db::connect(&database_url).await?;
            db::sync_teams(&pool, &teams).await?;
            tracing::info!(teams = teams.len(), workers = cfg.workers.len(), "orchestrator starting");
            let shared = Arc::new(worker::Shared {
                teams: teams.into_iter().map(|t| (t.name.clone(), t)).collect(),
                cfg: cfg.clone(),
                pool,
                rcon_password,
                sample_lock: Default::default(),
            });
            let mut tasks = tokio::task::JoinSet::new();
            for wc in cfg.workers {
                tasks.spawn(worker::run(shared.clone(), wc));
            }
            tokio::select! {
                _ = tokio::signal::ctrl_c() => tracing::info!("shutting down"),
                _ = tasks.join_next() => anyhow::bail!("a worker task exited unexpectedly"),
            }
        }
    }
    Ok(())
}
