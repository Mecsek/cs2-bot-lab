use crate::live::Shared;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, Row};
use uuid::Uuid;

pub struct ApiError(anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        tracing::error!("request failed: {:#}", self.0);
        (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

#[derive(Serialize, FromRow)]
pub struct MatchSummary {
    id: Uuid,
    worker: String,
    map: String,
    team_a: String,
    team_b: String,
    status: String,
    score_a: i32,
    score_b: i32,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
}

macro_rules! summary_select {
    ($rest:literal) => {
        concat!(
            "SELECT id, worker, map, team_a, team_b, status, score_a, score_b, started_at, finished_at ",
            "FROM matches ",
            $rest
        )
    };
}

#[derive(Deserialize)]
pub struct ListQuery {
    status: Option<String>,
    limit: Option<i64>,
}

pub async fn list_matches(State(s): State<Shared>, Query(q): Query<ListQuery>) -> ApiResult<Json<Vec<MatchSummary>>> {
    let rows = sqlx::query_as::<_, MatchSummary>(summary_select!(
        "WHERE ($1::TEXT IS NULL OR status = $1) ORDER BY started_at DESC LIMIT $2"
    ))
    .bind(q.status)
    .bind(q.limit.unwrap_or(50).clamp(1, 500))
    .fetch_all(&s.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Serialize, FromRow)]
pub struct RoundRow {
    round: i32,
    winner_side: String,
    winner_team: String,
    reason: String,
    end_tick: i32,
}

#[derive(Serialize)]
pub struct MatchDetail {
    #[serde(rename = "match")]
    summary: MatchSummary,
    rounds: Vec<RoundRow>,
}

pub async fn get_match(State(s): State<Shared>, Path(id): Path<Uuid>) -> ApiResult<Response> {
    let summary = sqlx::query_as::<_, MatchSummary>(summary_select!("WHERE id = $1"))
        .bind(id)
        .fetch_optional(&s.pool)
        .await?;
    let Some(summary) = summary else { return Ok(StatusCode::NOT_FOUND.into_response()) };
    let rounds = sqlx::query_as::<_, RoundRow>(
        "SELECT round, winner_side, winner_team, reason, end_tick FROM rounds
         WHERE match_id = $1 ORDER BY round",
    )
    .bind(id)
    .fetch_all(&s.pool)
    .await?;
    Ok(Json(MatchDetail { summary, rounds }).into_response())
}

#[derive(Serialize)]
pub struct TeamRow {
    name: String,
    description: String,
    params: Value,
}

pub async fn list_teams(State(s): State<Shared>) -> ApiResult<Json<Vec<TeamRow>>> {
    let rows = sqlx::query("SELECT name, description, params FROM teams ORDER BY name")
        .fetch_all(&s.pool)
        .await?;
    Ok(Json(
        rows.into_iter()
            .map(|r| TeamRow { name: r.get("name"), description: r.get("description"), params: r.get("params") })
            .collect(),
    ))
}

#[derive(Serialize, FromRow)]
pub struct PairStats {
    team_x: String,
    team_y: String,
    map: String,
    matches: i64,
    rounds_x: i64,
    rounds_y: i64,
}

#[derive(Serialize)]
pub struct Stats {
    pairs: Vec<PairStats>,
}

pub async fn stats(State(s): State<Shared>) -> ApiResult<Json<Stats>> {
    let pairs = sqlx::query_as::<_, PairStats>(
        "SELECT LEAST(m.team_a, m.team_b) AS team_x, GREATEST(m.team_a, m.team_b) AS team_y, m.map,
                count(DISTINCT m.id) AS matches,
                count(*) FILTER (WHERE r.winner_team = LEAST(m.team_a, m.team_b)) AS rounds_x,
                count(*) FILTER (WHERE r.winner_team = GREATEST(m.team_a, m.team_b)) AS rounds_y
         FROM matches m JOIN rounds r ON r.match_id = m.id
         WHERE m.status = 'finished'
         GROUP BY 1, 2, 3 ORDER BY 1, 2, 3",
    )
    .fetch_all(&s.pool)
    .await?;
    Ok(Json(Stats { pairs }))
}

/// Pre-built by the demo pipeline as gzip'd JSON; served as-is.
pub async fn replay(State(s): State<Shared>, Path(id): Path<Uuid>) -> ApiResult<Response> {
    let path = s.replay_dir.join(format!("{id}.json.gz"));
    match tokio::fs::read(&path).await {
        Ok(bytes) => Ok((
            [(header::CONTENT_TYPE, "application/json"), (header::CONTENT_ENCODING, "gzip")],
            bytes,
        )
            .into_response()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(StatusCode::NOT_FOUND.into_response()),
        Err(e) => Err(e.into()),
    }
}

pub async fn health() -> &'static str {
    "ok"
}
