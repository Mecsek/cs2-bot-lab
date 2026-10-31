use crate::live::Shared;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use lab_core::live::{PluginMessage, UiMessage};
use tokio::sync::broadcast::error::RecvError;
use tracing::{info, warn};
use uuid::Uuid;

/// `/ws/plugin`: one connection per CS2 server.
pub async fn plugin(ws: WebSocketUpgrade, State(state): State<Shared>) -> Response {
    ws.max_message_size(4 << 20).on_upgrade(move |socket| plugin_conn(socket, state))
}

async fn plugin_conn(mut socket: WebSocket, state: Shared) {
    let mut server_id: Option<String> = None;
    while let Some(msg) = socket.recv().await {
        let text = match msg {
            Ok(Message::Text(t)) => t,
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => continue,
        };
        let parsed: PluginMessage = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                warn!(?server_id, "bad plugin message: {e}");
                continue;
            }
        };
        match parsed {
            PluginMessage::Hello { server_id: id, map, plugin_version } => {
                info!(server_id = id, map, plugin_version, "plugin connected");
                state.on_hello(&id).await;
                server_id = Some(id);
            }
            PluginMessage::Frame(frame) => match &server_id {
                Some(id) => state.on_frame(id, frame).await,
                None => warn!("frame before hello"),
            },
            PluginMessage::Event(event) => match &server_id {
                Some(id) => state.on_event(id, event).await,
                None => warn!("event before hello"),
            },
        }
    }
    info!(?server_id, "plugin disconnected");
}

/// `/ws/live/{id}`: UI subscription to one running match.
pub async fn live(ws: WebSocketUpgrade, Path(id): Path<Uuid>, State(state): State<Shared>) -> Response {
    ws.on_upgrade(move |socket| live_conn(socket, state, id))
}

async fn send(socket: &mut WebSocket, msg: &UiMessage) -> bool {
    match serde_json::to_string(msg) {
        Ok(s) => socket.send(Message::Text(s.into())).await.is_ok(),
        Err(_) => true,
    }
}

async fn live_conn(mut socket: WebSocket, state: Shared, id: Uuid) {
    let Some((hello, mut rx)) = state.subscribe(id).await else {
        send(&mut socket, &UiMessage::Status { status: "not_live".into() }).await;
        let _ = socket.send(Message::Close(None)).await;
        return;
    };
    if !send(&mut socket, &hello).await {
        return;
    }
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Ok(m) => {
                    let done = matches!(&m, UiMessage::Status { status } if status == "finished");
                    if !send(&mut socket, &m).await || done {
                        break;
                    }
                }
                // A slow client skips frames rather than stalling everyone else.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                _ => {}
            },
        }
    }
}
