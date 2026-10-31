use crate::sampler::{Counts, Matchup, pair_key};
use anyhow::Result;
use lab_core::team::Team;
use sqlx::PgPool;
use sqlx::Row;
use uuid::Uuid;

pub async fn connect(url: &str) -> Result<PgPool> {
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(8).connect(url).await?;
    sqlx::migrate!("../../migrations").run(&pool).await?;
    Ok(pool)
}

/// Upserts the roster so matches can reference teams and the api can list them.
pub async fn sync_teams(pool: &PgPool, teams: &[Team]) -> Result<()> {
    for t in teams {
        sqlx::query(
            "INSERT INTO teams (name, description, config, params) VALUES ($1, $2, $3, $4)
             ON CONFLICT (name) DO UPDATE
             SET description = EXCLUDED.description, config = EXCLUDED.config,
                 params = EXCLUDED.params, updated_at = now()",
        )
        .bind(&t.name)
        .bind(&t.description)
        .bind(serde_json::to_value(t)?)
        .bind(serde_json::to_value(t.params())?)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Matches played or in progress per (pair, map); failed ones don't count.
pub async fn play_counts(pool: &PgPool) -> Result<Counts> {
    let rows = sqlx::query(
        "SELECT team_a, team_b, map, count(*)::INT AS n FROM matches
         WHERE status <> 'failed' GROUP BY team_a, team_b, map",
    )
    .fetch_all(pool)
    .await?;
    let mut counts = Counts::new();
    for r in rows {
        let key = pair_key(r.get("team_a"), r.get("team_b"), r.get("map"));
        *counts.entry(key).or_default() += r.get::<i32, _>("n") as u32;
    }
    Ok(counts)
}

pub struct NewMatch<'a> {
    pub id: Uuid,
    pub worker: &'a str,
    pub matchup: &'a Matchup,
    pub team_a: &'a Team,
    pub team_b: &'a Team,
    pub max_rounds: u32,
    pub demo_file: &'a str,
}

pub async fn insert_match(pool: &PgPool, m: &NewMatch<'_>) -> Result<()> {
    sqlx::query(
        "INSERT INTO matches (id, worker, map, team_a, team_b, team_a_config, team_b_config,
                              max_rounds, status, demo_file)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'running', $9)",
    )
    .bind(m.id)
    .bind(m.worker)
    .bind(&m.matchup.map)
    .bind(&m.matchup.team_a)
    .bind(&m.matchup.team_b)
    .bind(serde_json::to_value(m.team_a)?)
    .bind(serde_json::to_value(m.team_b)?)
    .bind(m.max_rounds as i32)
    .bind(m.demo_file)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn live_ended(pool: &PgPool, id: Uuid) -> Result<bool> {
    let ended: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT live_ended_at FROM matches WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await?;
    Ok(ended.is_some())
}

pub async fn mark_finished(pool: &PgPool, id: Uuid) -> Result<()> {
    sqlx::query("UPDATE matches SET status = 'finished', finished_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn mark_failed(pool: &PgPool, id: Uuid, error: &str) -> Result<()> {
    sqlx::query("UPDATE matches SET status = 'failed', error = $2, finished_at = now() WHERE id = $1")
        .bind(id)
        .bind(error)
        .execute(pool)
        .await?;
    Ok(())
}

/// Matches left `running` by a previous orchestrator process can never finish cleanly.
pub async fn fail_orphans(pool: &PgPool, worker: &str) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE matches SET status = 'failed', error = 'orchestrator restarted', finished_at = now()
         WHERE worker = $1 AND status = 'running'",
    )
    .bind(worker)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}
