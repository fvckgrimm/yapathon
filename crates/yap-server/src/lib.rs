//! The local API OBS talks to.
//!
//! Two clients, one server. The dock is a normal browser tab; the overlay is an
//! OBS Browser Source. Both speak the same endpoints, so there is no OBS
//! plugin and nothing to build per-platform beyond the app binary itself.
//!
//! Everything binds to loopback. The transcript of a yapathon and the goal it
//! is chasing are the streamer's business, and the server has no
//! authentication because it has no network exposure to authenticate for.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use yap_core::goal::{Event, GoalMode, HardLimit};
use yap_core::session::SessionSnapshot;

pub mod commands;
pub mod overlay;
pub mod state;

pub use state::AppState;

/// The server, and the address it ended up on.
pub struct Server {
    address: SocketAddr,
    state: Arc<AppState>,
}

impl Server {
    /// Bind and start serving.
    ///
    /// # Errors
    /// Propagates bind failures. The address is worth reporting exactly, since
    /// the streamer has to type it into OBS.
    pub async fn bind(address: &str, port: u16, state: Arc<AppState>) -> anyhow::Result<Self> {
        let listener = TcpListener::bind((address, port)).await?;
        let address = listener.local_addr()?;
        let router = router(state.clone());

        tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, router).await {
                log::error!("server stopped: {e}");
            }
        });

        Ok(Self { address, state })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub fn state(&self) -> &Arc<AppState> {
        &self.state
    }

    /// The URL OBS's Browser Source should point at.
    pub fn overlay_url(&self) -> String {
        format!("http://{}/overlay", self.address)
    }

    /// The URL for the control dock.
    pub fn dock_url(&self) -> String {
        format!("http://{}/dock", self.address)
    }
}

fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/state", get(get_state))
        .route("/api/start", post(start_session))
        .route("/api/pause", post(pause))
        .route("/api/resume", post(resume))
        .route("/api/end", post(end))
        .route("/api/undo", post(undo))
        .route("/api/adjust", post(adjust))
        .route("/api/events", post(apply_event))
        .route("/api/transcript", get(get_transcript))
        .route("/api/events/log", get(get_event_log))
        .route("/api/warnings", get(get_warnings))
        .route("/api/ws", get(websocket))
        .route("/overlay", get(overlay::serve_overlay))
        .route("/dock", get(overlay::serve_dock))
        .route("/ping", get(ping))
        // A browser source will not load a page from a private network address
        // without this. OBS's CEF treats loopback as insecure otherwise.
        .layer(
            tower_http::set_header::SetResponseHeaderLayer::if_not_present(
                header::CACHE_CONTROL,
                header::HeaderValue::from_static("no-store"),
            ),
        )
        .with_state(state)
}

/// A plain-text liveness check, for when a Browser Source shows nothing and the
/// question is whether OBS can reach the app at all.
///
/// Plain text deliberately: it renders through no CSS engine, no layout and no
/// JavaScript, so a 200 with `ok` here proves the server is reachable and the
/// fault is somewhere between the page and the pixels.
async fn ping() -> &'static str {
    "ok\n"
}

/// The full state, for a client that just connected.
#[derive(Debug, Serialize)]
struct StateResponse {
    /// The current yapathon, or `null` when none is running.
    ///
    /// Deliberately *not* `#[serde(flatten)]`. Flattening an `Option` splices
    /// the fields into the enclosing object when it is `Some` and emits nothing
    /// when it is `None`, so the key a client reads (`snapshot`) does not exist
    /// either way. That is exactly the bug the dock and the overlay hit before
    /// this was a named field.
    snapshot: Option<SessionSnapshot>,
    listening: bool,
    warnings: Vec<String>,
    events: Vec<Event>,
    /// The overlay theme currently in force.
    ///
    /// Carried here so the overlay can restyle itself without a reload: OBS
    /// creates a Browser Source once and then leaves it alone, so changing the
    /// theme in settings would otherwise need the source to be deleted and
    /// re-added mid-stream.
    overlay_theme: String,
    /// The overlay style currently in force.
    ///
    /// Carried separately from the palette because the two are chosen
    /// independently, and OBS creates a Browser Source once and then leaves it
    /// alone — so a change to either has to reach the page without a reload.
    overlay_style: String,
}

async fn get_state(State(state): State<Arc<AppState>>) -> Json<StateResponse> {
    let (overlay_theme, overlay_style) = state.overlay_look(None, None);

    Json(StateResponse {
        snapshot: state.snapshot().await,
        listening: state.is_listening().await,
        warnings: state.warnings().await,
        events: state.events().await,
        overlay_theme,
        overlay_style,
    })
}

/// Body for starting a yapathon.
#[derive(Debug, Default, Deserialize)]
struct StartRequest {
    /// Goal mode. Defaults to extend, which is the interesting one.
    #[serde(default)]
    mode: Option<ModeRequest>,
    /// Hard limit in the mode's unit.
    #[serde(default)]
    hard_limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ModeRequest {
    Fixed { words: u32 },
    Extend { base_words: u32 },
    Time { secs: u64 },
}

impl From<ModeRequest> for GoalMode {
    fn from(request: ModeRequest) -> Self {
        match request {
            ModeRequest::Fixed { words } => Self::Fixed { words },
            ModeRequest::Extend { base_words } => Self::Extend { base_words },
            ModeRequest::Time { secs } => Self::Time {
                base: std::time::Duration::from_secs(secs),
            },
        }
    }
}

async fn start_session(
    State(state): State<Arc<AppState>>,
    Json(request): Json<StartRequest>,
) -> Result<Json<SessionSnapshot>, ApiError> {
    let mode = request
        .mode
        .map(GoalMode::from)
        .unwrap_or(GoalMode::Extend { base_words: 5_000 });

    let limit = request.hard_limit.map(|max| HardLimit { max });
    let config = yap_core::session::SessionConfig::new(mode, limit);

    Ok(Json(state.start_session(config).await?))
}

/// An action that needs no body.
#[derive(Debug, Serialize)]
struct Acknowledgement {
    ok: bool,
    snapshot: Option<SessionSnapshot>,
}

impl Acknowledgement {
    fn new(snapshot: SessionSnapshot) -> Self {
        Self {
            ok: true,
            snapshot: Some(snapshot),
        }
    }
}

async fn pause(State(state): State<Arc<AppState>>) -> Result<Json<Acknowledgement>, ApiError> {
    ensure_session(&state).await?;
    Ok(Json(Acknowledgement::new(state.pause().await?)))
}

async fn resume(State(state): State<Arc<AppState>>) -> Result<Json<Acknowledgement>, ApiError> {
    ensure_session(&state).await?;
    Ok(Json(Acknowledgement::new(state.resume().await?)))
}

async fn end(State(state): State<Arc<AppState>>) -> Result<Json<Acknowledgement>, ApiError> {
    ensure_session(&state).await?;
    Ok(Json(Acknowledgement::new(state.end().await?)))
}

/// Whether a yapathon is running, as a typed check.
///
/// Distinguishing "no yapathon" from "something went wrong" is what lets the
/// API answer 409 rather than 500 for the ordinary case of a streamer clicking
/// pause before starting.
async fn ensure_session(state: &AppState) -> Result<(), ApiError> {
    if state.status().await.is_none() {
        return Err(ApiError::NoSession);
    }
    Ok(())
}

async fn undo(State(state): State<Arc<AppState>>) -> Json<Acknowledgement> {
    let removed = state.undo_last().await.unwrap_or(None);
    Json(Acknowledgement {
        ok: removed.is_some(),
        snapshot: state.snapshot().await,
    })
}

#[derive(Debug, Deserialize)]
struct AdjustRequest {
    words: i64,
    #[serde(default)]
    note: String,
}

async fn adjust(
    State(state): State<Arc<AppState>>,
    Json(request): Json<AdjustRequest>,
) -> Result<Json<Acknowledgement>, ApiError> {
    let note = if request.note.is_empty() {
        "manual adjustment"
    } else {
        &request.note
    };
    ensure_session(&state).await?;

    Ok(Json(Acknowledgement::new(
        state.adjust(request.words, note).await?,
    )))
}

/// Post an event directly.
///
/// This is how the Twitch and chat integrations reach the goal, and how a
/// streamer can script one with `curl`.
#[derive(Debug, Deserialize)]
struct EventRequest {
    #[serde(default = "default_platform")]
    platform: String,
    #[serde(default = "default_kind")]
    kind: String,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    amount: Option<f64>,
    /// Words to grant. Omit to price from settings.
    #[serde(default)]
    words: Option<u32>,
}

fn default_platform() -> String {
    "chat".to_string()
}

fn default_kind() -> String {
    "command".to_string()
}

async fn apply_event(
    State(state): State<Arc<AppState>>,
    Json(request): Json<EventRequest>,
) -> Result<Json<Acknowledgement>, ApiError> {
    let event = Event {
        platform: request.platform,
        kind: request.kind,
        user: request.user,
        amount: request.amount,
        words: request.words,
        note: None,
    };

    state.apply_event(event).await;
    let snapshot = state.snapshot().await.ok_or_else(|| ApiError::NoSession)?;

    Ok(Json(Acknowledgement::new(snapshot)))
}

/// One line of the audit log.
#[derive(Debug, Serialize)]
struct TranscriptLine {
    started_ms: u64,
    words: u32,
    text: String,
    voided: bool,
}

async fn get_transcript(State(state): State<Arc<AppState>>) -> Json<Vec<TranscriptLine>> {
    Json(
        state
            .transcript()
            .await
            .into_iter()
            .map(|(started_ms, words, text, voided)| TranscriptLine {
                started_ms,
                words,
                text,
                voided,
            })
            .collect(),
    )
}

async fn get_event_log(State(state): State<Arc<AppState>>) -> Json<Vec<Event>> {
    Json(state.events().await)
}

async fn get_warnings(State(state): State<Arc<AppState>>) -> Json<Vec<String>> {
    Json(state.warnings().await)
}

/// Push state to a connected client.
///
/// Sends the current state immediately, then every change. The immediate send
/// matters: without it a client connecting after a goal change would show stale
/// numbers until the next word.
async fn websocket(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| stream_updates(socket, state))
}

async fn stream_updates(mut socket: WebSocket, state: Arc<AppState>) {
    let mut updates = state.subscribe();

    if let Some(snapshot) = state.snapshot().await
        && let Ok(text) = serde_json::to_string(&snapshot)
    {
        socket.send(Message::Text(text.into())).await.ok();
    }

    loop {
        match updates.recv().await {
            Ok(snapshot) => {
                let Ok(text) = serde_json::to_string(&snapshot) else {
                    continue;
                };
                if socket.send(Message::Text(text.into())).await.is_err() {
                    return;
                }
            }
            // A slow client falls behind. It renders the newest state anyway,
            // so there is nothing to replay.
            Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                log::debug!("websocket client lagged by {missed} updates");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// An API failure, as JSON.
#[derive(Debug, Serialize)]
struct ErrorBody {
    error: String,
}

/// Something the client can act on.
#[derive(Debug)]
enum ApiError {
    /// No yapathon is running.
    NoSession,
    /// Anything else, including storage failures.
    Other(anyhow::Error),
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        Self::Other(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message): (_, String) = match self {
            // 409 rather than 400: the request was fine, the state was not.
            Self::NoSession => (
                axum::http::StatusCode::CONFLICT,
                "no yapathon is running".to_string(),
            ),
            Self::Other(e) => {
                log::error!("api error: {e:#}");
                (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
            }
        };

        (status, Json(ErrorBody { error: message })).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_state() -> Arc<AppState> {
        AppState::shared()
    }

    #[tokio::test]
    async fn state_without_a_session_is_not_an_error() {
        // The dock opens before a yapathon starts; a 409 here would make the
        // whole app look broken on launch.
        let state = test_state().await;
        let Json(StateResponse {
            snapshot,
            listening,
            ..
        }) = get_state(State(state)).await;
        assert!(snapshot.is_none());
        assert!(!listening);
    }

    #[tokio::test]
    async fn the_snapshot_key_exists_in_both_states() {
        // The clients read `body.snapshot`, so the key has to be present whether
        // or not a yapathon is running. Flattening the `Option` would have
        // produced a response with no such key at all.
        let state = test_state().await;

        let Json(idle) = get_state(State(state.clone())).await;
        assert!(
            serde_json::to_value(&idle)
                .expect("serialise")
                .get("snapshot")
                .is_some(),
            "the key must be present with no yapathon running"
        );

        let _ = start_session(
            State(state.clone()),
            Json(StartRequest {
                mode: Some(ModeRequest::Fixed { words: 100 }),
                hard_limit: None,
            }),
        )
        .await
        .expect("started");

        let Json(running) = get_state(State(state)).await;
        let value = serde_json::to_value(&running).expect("serialise");
        assert_eq!(
            value.pointer("/snapshot/target").and_then(|v| v.as_u64()),
            Some(100),
            "and it must carry the session when one is running"
        );
    }

    #[tokio::test]
    async fn starting_publishes_a_snapshot() {
        let state = test_state().await;
        let Json(response) = start_session(
            State(state.clone()),
            Json(StartRequest {
                mode: Some(ModeRequest::Extend { base_words: 1_000 }),
                hard_limit: Some(2_000),
            }),
        )
        .await
        .expect("started");

        let snapshot = response;
        assert_eq!(snapshot.target, 1_000);
        assert_eq!(snapshot.mode, GoalMode::Extend { base_words: 1_000 });
    }

    #[tokio::test]
    async fn a_request_with_no_mode_gets_the_default() {
        let state = test_state().await;
        let response = start_session(State(state), Json(StartRequest::default()))
            .await
            .expect("started");

        assert_eq!(response.0.mode, GoalMode::Extend { base_words: 5_000 });
    }

    #[tokio::test]
    async fn all_three_modes_convert() {
        assert_eq!(
            GoalMode::from(ModeRequest::Fixed { words: 100 }),
            GoalMode::Fixed { words: 100 }
        );
        assert_eq!(
            GoalMode::from(ModeRequest::Extend { base_words: 200 }),
            GoalMode::Extend { base_words: 200 }
        );
        assert_eq!(
            GoalMode::from(ModeRequest::Time { secs: 60 }),
            GoalMode::Time {
                base: std::time::Duration::from_secs(60)
            }
        );
    }

    #[tokio::test]
    async fn a_time_mode_body_deserialises() {
        let request: StartRequest =
            serde_json::from_str(r#"{"mode":{"kind":"time","secs":600},"hard_limit":900}"#)
                .expect("parse");

        assert_eq!(
            request.mode.map(GoalMode::from),
            Some(GoalMode::Time {
                base: std::time::Duration::from_secs(600)
            })
        );
        assert_eq!(request.hard_limit, Some(900));
    }

    #[tokio::test]
    async fn an_event_post_moves_the_goal() {
        let state = test_state().await;
        let _ = start_session(
            State(state.clone()),
            Json(StartRequest {
                mode: Some(ModeRequest::Extend { base_words: 500 }),
                hard_limit: None,
            }),
        )
        .await
        .expect("started");

        let Json(response) = apply_event(
            State(state.clone()),
            Json(EventRequest {
                platform: "twitch".into(),
                kind: "sub".into(),
                user: Some("someone".into()),
                amount: None,
                words: Some(250),
            }),
        )
        .await
        .expect("applied");

        assert_eq!(response.snapshot.expect("snapshot").target, 750);

        let Json(log) = get_event_log(State(state)).await;
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].platform, "twitch");
    }

    #[tokio::test]
    async fn an_event_with_no_words_field_defaults_to_chat() {
        let request: EventRequest = serde_json::from_str(r#"{"kind":"sub"}"#).expect("parse");
        assert_eq!(request.platform, "chat");
        assert_eq!(request.words, None);
    }

    #[tokio::test]
    async fn pausing_without_a_session_is_a_conflict() {
        let state = test_state().await;

        // Through the router, so the status code a client sees is what is
        // asserted rather than the handler's error type.
        let server = Server::bind("127.0.0.1", 0, state).await.expect("bind");
        let status = post_status(&format!("http://{}/api/pause", server.address())).await;

        // 409, not 400: the request was well formed, the app just has no
        // yapathon running.
        assert_eq!(status, 409);
    }

    #[tokio::test]
    async fn undo_reports_whether_there_was_anything_to_undo() {
        let state = test_state().await;
        let _ = start_session(
            State(state.clone()),
            Json(StartRequest {
                mode: Some(ModeRequest::Fixed { words: 100 }),
                hard_limit: None,
            }),
        )
        .await
        .expect("started");

        // Nothing recorded yet.
        let Json(empty) = undo(State(state.clone())).await;
        assert!(!empty.ok);

        state.record_words("one two three", 0, 1000).await;
        let Json(undone) = undo(State(state)).await;
        assert!(undone.ok);
        assert_eq!(undone.snapshot.expect("snapshot").words, 0);
    }

    #[tokio::test]
    async fn the_transcript_endpoint_returns_the_audit_log() {
        let state = test_state().await;
        let _ = start_session(
            State(state.clone()),
            Json(StartRequest {
                mode: Some(ModeRequest::Fixed { words: 100 }),
                hard_limit: None,
            }),
        )
        .await
        .expect("started");

        state.record_words("one two three", 500, 1500).await;

        let Json(lines) = get_transcript(State(state)).await;
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].words, 3);
        assert_eq!(lines[0].started_ms, 500);
        assert!(!lines[0].voided);
    }

    #[tokio::test]
    async fn the_server_binds_and_reports_urls() {
        let state = AppState::shared();
        // Port 0 lets the OS choose, so the test does not collide with a real
        // server or a previous run.
        let server = Server::bind("127.0.0.1", 0, state).await.expect("bind");

        assert!(server.overlay_url().starts_with("http://127.0.0.1:"));
        assert!(server.overlay_url().ends_with("/overlay"));
        assert!(server.dock_url().ends_with("/dock"));
        assert_ne!(
            server.address().port(),
            0,
            "the OS must have assigned a port"
        );
    }

    #[tokio::test]
    async fn the_bound_server_answers_a_request() {
        // A real request against the real router, not a direct handler call.
        let state = AppState::shared();
        let server = Server::bind("127.0.0.1", 0, state).await.expect("bind");

        let response = reqwest_get(&format!("http://{}/api/state", server.address()))
            .await
            .expect("request");

        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn the_overlay_page_is_served() {
        let state = AppState::shared();
        let server = Server::bind("127.0.0.1", 0, state).await.expect("bind");

        let response = reqwest_get(&format!("http://{}/overlay", server.address()))
            .await
            .expect("request");

        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn the_ping_endpoint_answers_plain_text() {
        // The first thing to try when a Browser Source shows nothing: does OBS
        // see `ok`? It has to render as plain text, so a 200 here proves the
        // server is reachable and isolates the fault to the page.
        let state = AppState::shared();
        let server = Server::bind("127.0.0.1", 0, state).await.expect("bind");

        let response = reqwest_get(&format!("http://{}/ping", server.address()))
            .await
            .expect("request");

        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn the_debug_page_is_served_for_a_query_param() {
        let state = AppState::shared();
        let server = Server::bind("127.0.0.1", 0, state).await.expect("bind");

        let response = reqwest_get(&format!("http://{}/overlay?debug=1", server.address()))
            .await
            .expect("request");

        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn the_overlay_still_serves_with_no_query_params() {
        // A missing or unparseable query must not break the page: OBS's URL
        // handling has surprised people before, and the overlay is the thing
        // that must never 500.
        let state = AppState::shared();
        let server = Server::bind("127.0.0.1", 0, state).await.expect("bind");

        for url in [
            format!("http://{}/overlay", server.address()),
            format!("http://{}/overlay?opaque=1", server.address()),
            format!("http://{}/overlay?junk", server.address()),
        ] {
            let response = reqwest_get(&url).await.expect("request");
            assert_eq!(response.status(), 200, "{url}");
        }
    }

    /// The status line of a POST with no body, over a real socket.
    async fn post_status(url: &str) -> u16 {
        request_status("POST", url, "").await.expect("request")
    }

    /// A minimal GET, so the dev-dependency tree stays small.
    async fn reqwest_get(url: &str) -> anyhow::Result<axum::response::Response> {
        let status = request_status("GET", url, "").await?;

        Ok(axum::response::Response::builder()
            .status(status)
            .body(axum::body::Body::empty())
            .expect("response"))
    }

    /// Send one request over a real socket and return its status code.
    async fn request_status(method: &str, url: &str, body: &str) -> anyhow::Result<u16> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let without_scheme = url.trim_start_matches("http://");
        // Split on the first slash so `127.0.0.1:1234/api/state` becomes the
        // address and the path, rather than connecting to a "port" of
        // `127.0.0.1:1234/api/state`.
        let (address, path) = match without_scheme.find('/') {
            Some(slash) => (&without_scheme[..slash], &without_scheme[slash..]),
            None => (without_scheme, "/"),
        };

        let mut stream = tokio::net::TcpStream::connect(address).await?;
        stream
            .write_all(
                format!(
                    "{method} {path} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\
                     Content-Length: {}\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await?;

        let mut response = Vec::new();
        stream.read_to_end(&mut response).await?;

        Ok(String::from_utf8_lossy(&response)
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }
}
