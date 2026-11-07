//! Backend for the browser terminal.
//!
//! Serves the static front end (HTML + WebAssembly) and exposes `/ws`: each
//! WebSocket connection gets its own shell running in a pseudo-terminal.
//! Keystrokes from the browser are written to the PTY, PTY output is streamed
//! back, and resize events are applied to the PTY window size.
//!
//! What a connection runs comes from the configured workspaces (see
//! [`config`]): `/api/workspaces` lists them and `/ws?workspace=<name>` picks
//! one, defaulting to the first. A workspace either runs a command here or
//! relays the connection to the term-server inside another container, which
//! makes this server a gateway in front of them.

mod config;

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade, close_code};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use protocol::{ClientMsg, ServerMsg};
use serde::{Deserialize, Serialize};

use crate::config::{Target, Workspace};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as Upstream;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::{info, warn};

type Error = Box<dyn std::error::Error + Send + Sync>;

/// Upper bound on PTY output merged into a single WebSocket frame.
const MAX_FRAME: usize = 256 * 1024;

struct Config {
    workspaces: Vec<Workspace>,
    token: Option<String>,
    /// Sent as `?token=` to the term-servers behind `url` workspaces.
    upstream_token: Option<String>,
    allowed_origins: Vec<String>,
}

#[derive(Deserialize)]
struct WsQuery {
    token: Option<String>,
    workspace: Option<String>,
}

#[derive(Serialize)]
struct WorkspaceList {
    workspaces: Vec<WorkspaceInfo>,
}

#[derive(Serialize)]
struct WorkspaceInfo {
    name: String,
    description: String,
    running: bool,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,tower_http=warn".into()))
        .init();

    let addr: SocketAddr = env_or("BIND", "127.0.0.1:8080").parse()?;
    let static_dir = PathBuf::from(env_or("STATIC_DIR", concat!(env!("CARGO_MANIFEST_DIR"), "/../../web")));
    let config = Arc::new(Config {
        workspaces: config::load()?,
        token: std::env::var("TERM_TOKEN").ok().filter(|t| !t.is_empty()),
        upstream_token: std::env::var("TERM_UPSTREAM_TOKEN").ok().filter(|t| !t.is_empty()),
        allowed_origins: std::env::var("ALLOWED_ORIGINS")
            .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default(),
    });

    if !addr.ip().is_loopback() && config.token.is_none() {
        warn!("listening on {addr} without TERM_TOKEN: anyone who can reach this port gets a shell");
    }

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .route("/api/workspaces", get(workspaces_handler))
        .fallback_service(ServeDir::new(&static_dir))
        .layer(TraceLayer::new_for_http())
        .with_state(config.clone());

    let names: Vec<&str> = config.workspaces.iter().map(|w| w.name.as_str()).collect();
    info!("serving {} on http://{addr} (workspaces: {})", static_dir.display(), names.join(", "));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

async fn ws_handler(ws: WebSocketUpgrade, headers: HeaderMap, Query(query): Query<WsQuery>, State(config): State<Arc<Config>>) -> Response {
    // Browsers let any web page open a WebSocket to localhost, so reject
    // cross-origin connections; otherwise a malicious site could get a shell.
    if !origin_allowed(&headers, &config.allowed_origins) {
        warn!("rejected WebSocket from origin {:?}", headers.get(header::ORIGIN));
        return (StatusCode::FORBIDDEN, "origin not allowed").into_response();
    }
    if !token_ok(&config, query.token.as_deref()) {
        return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
    }
    let workspace = match &query.workspace {
        Some(name) => config.workspaces.iter().find(|w| w.name == *name),
        None => config.workspaces.first(),
    };
    let Some(workspace) = workspace else {
        return (StatusCode::NOT_FOUND, "no such workspace").into_response();
    };
    match &workspace.target {
        Target::Command { program, args } => {
            let (program, args) = (program.clone(), args.clone());
            ws.on_upgrade(move |socket| async move {
                if let Err(err) = run_session(socket, &program, &args).await {
                    warn!("session ended with error: {err}");
                }
            })
        }
        Target::Url { url, .. } => {
            let url = with_token(url, config.upstream_token.as_deref());
            let name = workspace.name.clone();
            ws.on_upgrade(move |socket| relay_session(socket, name, url))
        }
    }
}

fn with_token(url: &str, token: Option<&str>) -> String {
    match token {
        Some(token) => {
            let sep = if url.contains('?') { '&' } else { '?' };
            format!("{url}{sep}token={token}")
        }
        None => url.to_string(),
    }
}

async fn workspaces_handler(Query(query): Query<WsQuery>, State(config): State<Arc<Config>>) -> Response {
    if !token_ok(&config, query.token.as_deref()) {
        return (StatusCode::UNAUTHORIZED, "invalid token").into_response();
    }
    let config = config.clone();
    // Probing upstream term-servers is blocking I/O.
    let list = tokio::task::spawn_blocking(move || {
        let workspaces = config
            .workspaces
            .iter()
            .map(|w| WorkspaceInfo { name: w.name.clone(), description: w.description.clone(), running: w.running() })
            .collect();
        WorkspaceList { workspaces }
    })
    .await;
    match list {
        Ok(list) => Json(list).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn token_ok(config: &Config, token: Option<&str>) -> bool {
    config.token.as_deref().is_none_or(|expected| token == Some(expected))
}

/// Accepts same-origin requests, requests without an Origin header (non-browser
/// clients), and origins listed in `ALLOWED_ORIGINS`.
fn origin_allowed(headers: &HeaderMap, allowed: &[String]) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true;
    };
    if allowed.iter().any(|a| a == origin) {
        return true;
    }
    let origin_host = origin.split_once("://").map_or(origin, |(_, rest)| rest);
    headers.get(header::HOST).and_then(|v| v.to_str().ok()) == Some(origin_host)
}

async fn run_session(mut socket: WebSocket, program: &str, args: &[String]) -> Result<(), Error> {
    let pair = native_pty_system().openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })?;

    let mut cmd = CommandBuilder::new(program);
    cmd.args(args);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "webterm");
    if let Ok(dir) = std::env::var("TERM_CWD").or_else(|_| std::env::var("HOME")) {
        cmd.cwd(dir);
    }
    let mut child = pair.slave.spawn_command(cmd)?;
    let mut killer = child.clone_killer();
    drop(pair.slave);
    info!("spawned {program} {} (pid {:?})", args.join(" "), child.process_id());

    let master = pair.master;
    let mut reader = master.try_clone_reader()?;
    let mut writer = master.take_writer()?;

    // PTY I/O is blocking, so it runs on dedicated threads bridged by channels.
    let (out_tx, mut out_rx) = mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out_tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let (in_tx, in_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        for data in in_rx {
            if writer.write_all(&data).and_then(|_| writer.flush()).is_err() {
                break;
            }
        }
    });

    let client_gone = loop {
        tokio::select! {
            out = out_rx.recv() => {
                let Some(mut data) = out else { break false };
                // Merge whatever else is already queued into one frame.
                while data.len() < MAX_FRAME {
                    match out_rx.try_recv() {
                        Ok(more) => data.extend_from_slice(&more),
                        Err(_) => break,
                    }
                }
                if socket.send(Message::Binary(ServerMsg::Output(data).encode().into())).await.is_err() {
                    break true;
                }
            }
            msg = socket.recv() => match msg {
                Some(Ok(Message::Binary(frame))) => match ClientMsg::decode(&frame) {
                    Ok(ClientMsg::Input(data)) => {
                        let _ = in_tx.send(data);
                    }
                    Ok(ClientMsg::Resize { cols, rows }) => {
                        master.resize(PtySize { rows: rows.max(1), cols: cols.max(1), pixel_width: 0, pixel_height: 0 })?;
                    }
                    Err(err) => warn!("bad frame from client: {err}"),
                },
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break true,
                Some(Ok(_)) => {}
            },
        }
    };

    if client_gone {
        info!("client disconnected, terminating shell");
        let _ = killer.kill();
    }
    let status = tokio::task::spawn_blocking(move || child.wait()).await??;
    info!("shell exited with {}", status.exit_code());
    if !client_gone {
        let _ = socket.send(Message::Binary(ServerMsg::Exit(status.exit_code() as i32).encode().into())).await;
        let _ = socket.send(Message::Close(None)).await;
    }
    Ok(())
}

/// Forwards frames unchanged between the browser and the term-server of a
/// `url` workspace; the session (shell, tmux, exit code) lives over there.
async fn relay_session(mut socket: WebSocket, name: String, url: String) {
    let mut upstream = match tokio_tungstenite::connect_async(url.as_str()).await {
        Ok((upstream, _)) => upstream,
        Err(err) => {
            warn!("workspace {name}: cannot connect upstream: {err}");
            let mut reason = format!("workspace {name} is unreachable ({err})");
            // A close reason is limited to 123 bytes.
            while reason.len() > 123 {
                reason.pop();
            }
            let frame = CloseFrame { code: close_code::ERROR, reason: reason.into() };
            let _ = socket.send(Message::Close(Some(frame))).await;
            return;
        }
    };
    info!("workspace {name}: relaying");
    loop {
        tokio::select! {
            msg = socket.recv() => match msg {
                Some(Ok(Message::Binary(frame))) => {
                    if upstream.send(Upstream::Binary(frame)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            msg = upstream.next() => match msg {
                Some(Ok(Upstream::Binary(frame))) => {
                    if socket.send(Message::Binary(frame)).await.is_err() {
                        break;
                    }
                }
                Some(Ok(Upstream::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
    let _ = upstream.close(None).await;
    let _ = socket.send(Message::Close(None)).await;
    info!("workspace {name}: relay closed");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(origin: Option<&str>, host: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, host.parse().unwrap());
        if let Some(o) = origin {
            h.insert(header::ORIGIN, o.parse().unwrap());
        }
        h
    }

    #[test]
    fn origin_check() {
        assert!(origin_allowed(&headers(None, "localhost:8080"), &[]));
        assert!(origin_allowed(&headers(Some("http://localhost:8080"), "localhost:8080"), &[]));
        assert!(!origin_allowed(&headers(Some("https://evil.example"), "localhost:8080"), &[]));
        assert!(origin_allowed(&headers(Some("https://ok.example"), "localhost:8080"), &["https://ok.example".into()]));
    }

    #[test]
    fn upstream_token_is_appended() {
        assert_eq!(with_token("ws://go:8080/ws", None), "ws://go:8080/ws");
        assert_eq!(with_token("ws://go:8080/ws", Some("s3cret")), "ws://go:8080/ws?token=s3cret");
        assert_eq!(with_token("ws://go:8080/ws?x=1", Some("s3cret")), "ws://go:8080/ws?x=1&token=s3cret");
    }
}
