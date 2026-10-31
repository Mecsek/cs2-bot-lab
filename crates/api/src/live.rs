//! In-memory live state per worker, fed by the plugin and fanned out to UI clients.

use lab_core::live::{EventKind, GameEvent, LiveFrame, MatchInfo, RoundPhase, Side, UiMessage};
use sqlx::{PgPool, Row};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, broadcast};
use tracing::{info, warn};
use uuid::Uuid;

const RECENT_EVENTS: usize = 64;
const RESOLVE_EVERY: Duration = Duration::from_secs(2);

pub struct LiveMatch {
    pub info: MatchInfo,
    pub max_rounds: u32,
    pub frame: Option<LiveFrame>,
    pub recent: VecDeque<GameEvent>,
    pub tx: broadcast::Sender<UiMessage>,
    last_score: (u32, u32),
}

#[derive(Default)]
pub struct Worker {
    pub current: Option<LiveMatch>,
    last_resolve: Option<Instant>,
}

pub struct AppState {
    pub pool: PgPool,
    pub replay_dir: std::path::PathBuf,
    pub workers: RwLock<HashMap<String, Worker>>,
}

pub type Shared = Arc<AppState>;

impl AppState {
    /// Sender for a running match, if any worker is currently serving it.
    pub async fn subscribe(&self, id: Uuid) -> Option<(UiMessage, broadcast::Receiver<UiMessage>)> {
        let workers = self.workers.read().await;
        workers.values().filter_map(|w| w.current.as_ref()).find(|m| m.info.id == id).map(|m| {
            let hello = UiMessage::Hello {
                info: m.info.clone(),
                frame: m.frame.clone(),
                recent_events: m.recent.iter().cloned().collect(),
            };
            (hello, m.tx.subscribe())
        })
    }

    /// Looks up the worker's running match in the database, at most every RESOLVE_EVERY.
    async fn resolve(&self, server_id: &str, force: bool) {
        {
            let workers = self.workers.read().await;
            if let Some(w) = workers.get(server_id)
                && !force
                && w.last_resolve.is_some_and(|t| t.elapsed() < RESOLVE_EVERY)
            {
                return;
            }
        }
        let found = match load_running(&self.pool, server_id).await {
            Ok(m) => m,
            Err(e) => {
                warn!(server_id, "resolving running match: {e:#}");
                None
            }
        };
        let mut workers = self.workers.write().await;
        let w = workers.entry(server_id.to_string()).or_default();
        w.last_resolve = Some(Instant::now());
        let same = match (&w.current, &found) {
            (Some(cur), Some((info, ..))) => cur.info.id == info.id,
            (None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        if let Some(old) = w.current.take() {
            let _ = old.tx.send(UiMessage::Status { status: "finished".into() });
        }
        w.current = found.map(|(info, max_rounds)| {
            info!(server_id, match_id = %info.id, "tracking match");
            LiveMatch {
                info,
                max_rounds,
                frame: None,
                recent: VecDeque::new(),
                tx: broadcast::channel(256).0,
                last_score: (0, 0),
            }
        });
    }

    pub async fn on_hello(&self, server_id: &str) {
        self.resolve(server_id, true).await;
    }

    pub async fn on_frame(&self, server_id: &str, frame: LiveFrame) {
        self.resolve(server_id, false).await;
        let mut score_update = None;
        {
            let mut workers = self.workers.write().await;
            let Some(m) = workers.get_mut(server_id).and_then(|w| w.current.as_mut()) else { return };
            if frame.phase != RoundPhase::Warmup {
                let score = (frame.score_ct, frame.score_t);
                if score != m.last_score {
                    m.last_score = score;
                    score_update = Some((m.info.id, scores_ab(m, &frame)));
                }
            }
            let _ = m.tx.send(UiMessage::Frame(frame.clone()));
            m.frame = Some(frame);
        }
        if let Some((id, (a, b))) = score_update
            && let Err(e) = sqlx::query("UPDATE matches SET score_a = $2, score_b = $3 WHERE id = $1")
                .bind(id)
                .bind(a as i32)
                .bind(b as i32)
                .execute(&self.pool)
                .await
        {
            warn!("updating score: {e:#}");
        }
    }

    pub async fn on_event(&self, server_id: &str, event: GameEvent) {
        self.resolve(server_id, false).await;
        let mut ended = None;
        {
            let mut workers = self.workers.write().await;
            let Some(m) = workers.get_mut(server_id).and_then(|w| w.current.as_mut()) else { return };
            if let EventKind::MatchEnd { score_ct, score_t } = event.kind {
                let mut frame = m.frame.clone();
                if let Some(f) = frame.as_mut() {
                    f.score_ct = score_ct;
                    f.score_t = score_t;
                }
                let (a, b) = frame.map(|f| scores_ab(m, &f)).unwrap_or((0, 0));
                ended = Some((m.info.id, a, b));
            }
            if m.recent.len() == RECENT_EVENTS {
                m.recent.pop_front();
            }
            m.recent.push_back(event.clone());
            let _ = m.tx.send(UiMessage::Event(event));
        }
        if let Some((id, a, b)) = ended {
            info!(server_id, match_id = %id, a, b, "match ended");
            // The orchestrator polls live_ended_at to know when to stop the demo.
            if let Err(e) = sqlx::query(
                "UPDATE matches SET score_a = $2, score_b = $3, live_ended_at = now()
                 WHERE id = $1 AND live_ended_at IS NULL",
            )
            .bind(id)
            .bind(a as i32)
            .bind(b as i32)
            .execute(&self.pool)
            .await
            {
                warn!("marking match ended: {e:#}");
            }
            let mut workers = self.workers.write().await;
            if let Some(m) = workers.get_mut(server_id).and_then(|w| w.current.as_mut()) {
                m.info.status = "finished".into();
                let _ = m.tx.send(UiMessage::Status { status: "finished".into() });
            }
        }
    }
}

/// Which side team A is on, preferring the bot names in the frame over the halftime rule.
fn side_of_a(m: &LiveMatch, frame: &LiveFrame) -> Side {
    frame
        .players
        .iter()
        .find(|p| p.team() == m.info.team_a)
        .map(|p| p.side)
        .unwrap_or(if frame.round <= m.max_rounds / 2 { Side::Ct } else { Side::T })
}

fn scores_ab(m: &LiveMatch, frame: &LiveFrame) -> (u32, u32) {
    match side_of_a(m, frame) {
        Side::Ct => (frame.score_ct, frame.score_t),
        Side::T => (frame.score_t, frame.score_ct),
    }
}


async fn load_running(
    pool: &PgPool,
    worker: &str,
) -> anyhow::Result<Option<(MatchInfo, u32)>> {
    let row = sqlx::query(
        "SELECT id, worker, map, team_a, team_b, status, max_rounds
         FROM matches WHERE worker = $1 AND status = 'running'
         ORDER BY started_at DESC LIMIT 1",
    )
    .bind(worker)
    .fetch_optional(pool)
    .await?;
    let Some(r) = row else { return Ok(None) };
    let info = MatchInfo {
        id: r.get("id"),
        worker: r.get("worker"),
        map: r.get("map"),
        team_a: r.get("team_a"),
        team_b: r.get("team_b"),
        status: r.get("status"),
    };
    Ok(Some((info, r.get::<i32, _>("max_rounds") as u32)))
}
