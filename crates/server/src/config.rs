//! The workspaces a browser tab can attach to, read from the YAML file named by
//! `TERM_CONFIG`:
//!
//! ```yaml
//! workspaces:
//!   - name: go
//!     description: Go · golang:1-trixie
//!     url: ws://go:8080/ws        # relay to the term-server running there
//!   - name: shell
//!     command: [fish, -l]         # or run a command here
//! ```
//!
//! Without `TERM_CONFIG` there is a single workspace running `TERM_SHELL`
//! with `TERM_ARGS`. The front end only offers a workspace switcher when more
//! than one is configured.

use std::collections::HashSet;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

use axum::http::Uri;
use serde::Deserialize;

/// How long the liveness check waits for an upstream term-server.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, PartialEq)]
pub struct Workspace {
    pub name: String,
    pub description: String,
    pub target: Target,
}

#[derive(Debug, PartialEq)]
pub enum Target {
    /// Run a program in a PTY on this host.
    Command { program: String, args: Vec<String> },
    /// Relay the connection to another term-server (`ws://host:port/path`).
    Url { url: String, host: String, port: u16 },
}

impl Workspace {
    /// Whether the workspace can be connected to right now: its upstream
    /// term-server accepts TCP connections. Command workspaces always can.
    pub fn running(&self) -> bool {
        match &self.target {
            Target::Command { .. } => true,
            Target::Url { host, port, .. } => (host.as_str(), *port)
                .to_socket_addrs()
                .is_ok_and(|mut addrs| addrs.any(|addr| TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).is_ok())),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    workspaces: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    #[serde(default)]
    description: String,
    url: Option<String>,
    command: Option<Vec<String>>,
}

/// Loads the workspaces from `TERM_CONFIG`, or the single `TERM_SHELL` one.
pub fn load() -> Result<Vec<Workspace>, String> {
    match std::env::var_os("TERM_CONFIG").filter(|v| !v.is_empty()) {
        Some(path) => {
            let path = PathBuf::from(path);
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            parse(&text).map_err(|e| format!("{}: {e}", path.display()))
        }
        None => {
            let program = std::env::var("TERM_SHELL").or_else(|_| std::env::var("SHELL")).unwrap_or_else(|_| "/bin/sh".into());
            let args = std::env::var("TERM_ARGS").unwrap_or_else(|_| "-l".into()).split_whitespace().map(String::from).collect();
            Ok(vec![Workspace { name: "local".into(), description: String::new(), target: Target::Command { program, args } }])
        }
    }
}

pub fn parse(text: &str) -> Result<Vec<Workspace>, String> {
    let file: File = serde_yaml_ng::from_str(text).map_err(|e| e.to_string())?;
    if file.workspaces.is_empty() {
        return Err("no workspaces configured".into());
    }
    let mut seen = HashSet::new();
    file.workspaces
        .into_iter()
        .map(|e| {
            // Names end up in URLs (?workspace=<name>), so keep them simple.
            if e.name.is_empty() || !e.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                return Err(format!("workspace name {:?} may only contain letters, digits, '-' and '_'", e.name));
            }
            if !seen.insert(e.name.clone()) {
                return Err(format!("workspace {:?} is configured twice", e.name));
            }
            let target = match (e.url, e.command) {
                (Some(url), None) => parse_url(url).map_err(|err| format!("workspace {:?}: {err}", e.name))?,
                (None, Some(command)) => {
                    let mut command = command.into_iter();
                    let program = command.next().ok_or_else(|| format!("workspace {:?}: command is empty", e.name))?;
                    Target::Command { program, args: command.collect() }
                }
                _ => return Err(format!("workspace {:?}: set either url or command", e.name)),
            };
            Ok(Workspace { name: e.name, description: e.description, target })
        })
        .collect()
}

fn parse_url(url: String) -> Result<Target, String> {
    let uri: Uri = url.parse().map_err(|_| format!("invalid url {url:?}"))?;
    if uri.scheme_str() != Some("ws") {
        return Err(format!("url {url:?} must start with ws://"));
    }
    let host = uri.host().filter(|h| !h.is_empty()).ok_or_else(|| format!("url {url:?} has no host"))?;
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    let port = uri.port_u16().unwrap_or(80);
    Ok(Target::Url { url, host, port })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_url_and_command_workspaces() {
        let ws = parse(
            "
workspaces:
  - name: go
    description: Go
    url: ws://go:8080/ws
  - name: shell
    command: [fish, -l]
",
        )
        .unwrap();
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].name, "go");
        assert_eq!(ws[0].description, "Go");
        assert_eq!(ws[0].target, Target::Url { url: "ws://go:8080/ws".into(), host: "go".into(), port: 8080 });
        assert_eq!(ws[1].target, Target::Command { program: "fish".into(), args: vec!["-l".into()] });
        assert!(ws[1].running());
    }

    #[test]
    fn rejects_invalid_configs() {
        let err = |text: &str| parse(text).unwrap_err();
        assert!(err("workspaces: []").contains("no workspaces"));
        assert!(err("workspaces: [{name: ../x, command: [sh]}]").contains("may only contain"));
        assert!(err("workspaces: [{name: a, command: [sh]}, {name: a, command: [sh]}]").contains("twice"));
        assert!(err("workspaces: [{name: a}]").contains("either url or command"));
        assert!(err("workspaces: [{name: a, url: 'ws://x/ws', command: [sh]}]").contains("either url or command"));
        assert!(err("workspaces: [{name: a, command: []}]").contains("empty"));
        assert!(err("workspaces: [{name: a, url: 'http://x/ws'}]").contains("must start with ws://"));
        assert!(err("workspaces: [{name: a, url: 'wss://x/ws'}]").contains("must start with ws://"));
        assert!(err("workspaces: [{name: a, command: [sh], colour: 1}]").contains("unknown field"));
        assert!(err("workspaces: [{name: a, tmux_socket: /run/tmux/a}]").contains("unknown field"));
        assert!(err("workspace: []").contains("unknown field"));
    }

    #[test]
    fn unreachable_url_workspace_is_not_running() {
        // Port 9 (discard) on localhost: nothing listens there.
        let ws = parse("workspaces: [{name: gone, url: 'ws://127.0.0.1:9/ws'}]").unwrap();
        assert!(!ws[0].running());
    }
}
