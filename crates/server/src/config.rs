//! The workspaces a browser tab can attach to, read from the YAML file named by
//! `TERM_CONFIG`:
//!
//! ```yaml
//! workspaces:
//!   - name: go
//!     description: Go · golang:1-trixie
//!     tmux_socket: /run/tmux/go   # attach to the tmux server on this socket
//!   - name: shell
//!     command: [fish, -l]         # or run any command
//! ```
//!
//! Without `TERM_CONFIG` there is a single workspace running `TERM_SHELL`
//! with `TERM_ARGS`. The front end only offers a workspace switcher when more
//! than one is configured.

use std::collections::HashSet;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, PartialEq)]
pub struct Workspace {
    pub name: String,
    pub description: String,
    target: Target,
}

#[derive(Debug, PartialEq)]
enum Target {
    Tmux { socket: PathBuf, session: String },
    Command { program: String, args: Vec<String> },
}

impl Workspace {
    /// The program and arguments to run in a new PTY for this workspace.
    pub fn command(&self) -> (String, Vec<String>) {
        match &self.target {
            Target::Tmux { socket, session } => (
                "tmux".into(),
                vec!["-u".into(), "-S".into(), socket.display().to_string(), "attach-session".into(), "-t".into(), session.clone()],
            ),
            Target::Command { program, args } => (program.clone(), args.clone()),
        }
    }

    /// Whether the workspace can be attached to right now: its tmux server
    /// accepts connections. Command workspaces are always available.
    pub fn running(&self) -> bool {
        match &self.target {
            Target::Tmux { socket, .. } => UnixStream::connect(socket).is_ok(),
            Target::Command { .. } => true,
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
    tmux_socket: Option<PathBuf>,
    tmux_session: Option<String>,
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
            let target = match (e.tmux_socket, e.command) {
                (Some(socket), None) => Target::Tmux { socket, session: e.tmux_session.unwrap_or_else(|| "main".into()) },
                (None, Some(command)) if e.tmux_session.is_none() => {
                    let mut command = command.into_iter();
                    let program = command.next().ok_or_else(|| format!("workspace {:?}: command is empty", e.name))?;
                    Target::Command { program, args: command.collect() }
                }
                (None, Some(_)) => return Err(format!("workspace {:?}: tmux_session needs tmux_socket", e.name)),
                _ => return Err(format!("workspace {:?}: set either tmux_socket or command", e.name)),
            };
            Ok(Workspace { name: e.name, description: e.description, target })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tmux_and_command_workspaces() {
        let ws = parse(
            "
workspaces:
  - name: go
    description: Go
    tmux_socket: /run/tmux/go
  - name: shell
    command: [fish, -l]
",
        )
        .unwrap();
        assert_eq!(ws.len(), 2);
        assert_eq!(ws[0].name, "go");
        assert_eq!(ws[0].description, "Go");
        assert_eq!(ws[0].command(), ("tmux".into(), ["-u", "-S", "/run/tmux/go", "attach-session", "-t", "main"].map(String::from).to_vec()));
        assert_eq!(ws[1].command(), ("fish".into(), vec!["-l".into()]));
        assert!(ws[1].running());
    }

    #[test]
    fn rejects_invalid_configs() {
        let err = |text: &str| parse(text).unwrap_err();
        assert!(err("workspaces: []").contains("no workspaces"));
        assert!(err("workspaces: [{name: ../x, command: [sh]}]").contains("may only contain"));
        assert!(err("workspaces: [{name: a, command: [sh]}, {name: a, command: [sh]}]").contains("twice"));
        assert!(err("workspaces: [{name: a}]").contains("either tmux_socket or command"));
        assert!(err("workspaces: [{name: a, command: []}]").contains("empty"));
        assert!(err("workspaces: [{name: a, command: [sh], tmux_session: x}]").contains("needs tmux_socket"));
        assert!(err("workspaces: [{name: a, command: [sh], colour: 1}]").contains("unknown field"));
        assert!(err("workspace: []").contains("unknown field"));
    }

    #[test]
    fn stopped_tmux_workspace_is_not_running() {
        let ws = parse("workspaces: [{name: gone, tmux_socket: /nonexistent/webterm.sock}]").unwrap();
        assert!(!ws[0].running());
    }
}
