# webterm

**A GPU-accelerated terminal emulator in Rust + WebAssembly, rendered with WebGL2.**
Run your shell, or a full tmux dev container, in a browser tab.

[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Rust 2024](https://img.shields.io/badge/rust-2024_edition-orange.svg)
![WebAssembly](https://img.shields.io/badge/target-wasm32-654ff0.svg)

- **Fast**: WebGL2 renderer with a glyph atlas, one draw call per frame
- **Correct**: xterm-compatible VT core, truecolor, mouse reporting, bracketed paste, OSC 52
- **Crisp**: box drawing, block elements and Powerline glyphs are drawn procedurally, with no seams
- **Batteries included**: Docker workspaces (Go, Python, ...) sharing one tmux + LazyVim setup, switchable from the header

## Quick start

### Docker: browser dev environment

Needs Docker with Compose (Docker Desktop, Colima, OrbStack, ...) and [`just`](https://github.com/casey/just).

```sh
just up      # build and start → http://localhost:8080
just down    # stop (plugins, history and tmux sessions are kept)
```

This starts a `go` and a `python` workspace, as listed in
[`docker/webterm.yaml`](docker/webterm.yaml). The header shows the workspace
you're in; click it to open the side panel and switch. With only one workspace
configured, the page is just the terminal. Each workspace keeps its
own tmux session (optional), and every tab on the same workspace shares it. The first
start installs the Neovim plugins, which takes about a minute
(`docker compose logs -f go python`).

Two ready-made setups in [`examples/`](examples):

- **[Local development](examples/local)**: one workspace per project, your
  project folder mounted in, opened directly on `localhost` (no gateway).
  ```sh
  just local ~/code/myapp                     # Go workspace → http://localhost:8080
  just local ~/code/api python:3-trixie 8081  # another project, base and port
  just local-down ~/code/api
  ```
- **[Server](examples/server)**: several workspaces behind one HTTPS URL, with
  Caddy for TLS, tokens, and CPU/memory limits per workspace.
  ```sh
  cp examples/server/.env.example examples/server/.env   # domain + tokens
  just server                                            # → https://<domain>/?token=...
  ```

**→ [Docker workspaces guide](docs/docker.md)**: both setups explained, getting
started step by step, everyday commands, and scenarios like adding a Rust workspace.

### Native: your own shell

Needs Rust with the `wasm32-unknown-unknown` target and a matching `wasm-bindgen` CLI.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130
just run     # build, then serve http://127.0.0.1:8080
```

This opens your login shell (`$SHELL -l`). To get tmux instead:

```sh
TERM_SHELL=tmux TERM_ARGS="new-session -A -s main" just run
```

No `just`? `./build.sh` builds everything, then run `./target/release/term-server`.

## How it works

```
 Browser                                          Backend (term-server)
┌──────────────────────────────────────┐  ws   ┌─────────────────────────┐
│ web (wasm)                           │ ───── │ axum  /ws               │
│  keyboard/IME/paste/mouse ──encode──▶│ Input │  ├─ PTY (portable-pty)  │
│  term-core: vte parser → grid ◀──────│Output │  │   └─ $SHELL -l       │
│  WebGL2 renderer (1 draw call/frame) │Resize │  └─ static files (web/) │
└──────────────────────────────────────┘       └─────────────────────────┘
```

The whole terminal emulator runs in the browser. The server only moves bytes
between a WebSocket and a PTY, with one shell per connection.

| crate | role |
|---|---|
| [`term-core`](crates/term-core) | Platform-independent VT emulator: parser (`vte`), grid, scrollback, alternate screen, scroll regions, modes, selection, key and mouse encoding. Unit tested natively. |
| [`web`](crates/web) | The wasm front end: WebGL2 renderer, glyph atlas, DOM input, WebSocket client. |
| [`server`](crates/server) | Serves `web/` and spawns a shell in a PTY for each WebSocket. |
| [`protocol`](crates/protocol) | Binary wire format shared by both sides (1-byte tag + payload). |

### The Docker setup

```
 browser ──ws──▶ gateway (term-server) ──ws──▶ go:8080      term-server → tmux or fish
                 relays, runs nothing        python:8080  term-server → tmux or fish
```

`compose.yaml` runs these containers, all built from [`docker/Dockerfile`](docker/Dockerfile):

- **Workspaces** (`go`, `python`): a language base image (`golang:1-trixie`,
  `python:3-trixie`) plus the shared **devtools** layer: a static
  `term-server` with the web files, Neovim + LazyVim, lazygit, fzf, starship,
  the tmux plugins and their config, all under `/opt/devtools`. The devtools
  layer doesn't depend on the base, so BuildKit adds it to every workspace as
  the same layer (`COPY --link`). Each workspace serves its own terminal on
  port 8080: every tab attaches to the tmux session `main`, or with
  `WORKSPACE_TMUX=0` gets a plain fish shell. Each has its own home volume;
  your code lives in `./workspace`, shared by all of them (override with
  `WORKSPACE_DIR`). LazyVim turns on the Go/Python/Rust extras when that
  toolchain is installed.
- **gateway**: the same `term-server`, configured with the workspaces'
  URLs in `docker/webterm.yaml`. It serves the page and relays each tab to the
  chosen workspace, so every process runs in the workspace. The image is
  distroless and static (12 MB). No sshd, Docker socket or extra privileges
  are involved.

The gateway is optional: publish a workspace's port and open it directly.

The tmux (prefix `C-b`), LazyVim, fish and starship config lives in
[`docker/workspace/`](docker/workspace). Edit it there and rerun `just up`.

**Adding a workspace:** copy the `python` service in `compose.yaml`, then change
its name, `WORKSPACE_BASE` and home volume, and add an entry to
`docker/webterm.yaml`. Any apt-based (Debian or Ubuntu) image works,
e.g. `rust:1-trixie` or `node:24-trixie`.

## Configuration

**Server** (environment variables):

| variable | default | meaning |
|---|---|---|
| `BIND` | `127.0.0.1:8080` | listen address |
| `TERM_CONFIG` | unset | workspace config file (see below) |
| `TERM_SHELL` | `$SHELL` | without `TERM_CONFIG`: program to run for each connection |
| `TERM_ARGS` | `-l` | its arguments (whitespace-separated) |
| `TERM_CWD` | `$HOME` | working directory of that program |
| `TERM_TOKEN` | unset | if set, connections need `?token=...` |
| `TERM_UPSTREAM_TOKEN` | unset | token sent to the term-servers behind `url` workspaces |
| `ALLOWED_ORIGINS` | unset | extra comma-separated origins allowed to connect |
| `STATIC_DIR` | `web/` | front-end directory |

**Workspaces** (`TERM_CONFIG`, YAML): what a browser tab can attach to, in the
order the side panel lists them. The first one opens when the URL doesn't name
one (`?workspace=<name>`). With a single workspace there's no header or panel,
just the terminal.

```yaml
workspaces:
  - name: go
    description: Go · golang:1-trixie   # optional, shown in the header
    url: ws://go:8080/ws                # relay to the term-server there

  - name: shell
    command: [fish, -l]                 # or run a command on this host
```

Without `TERM_CONFIG` there's one workspace running `TERM_SHELL` `TERM_ARGS`.
`GET /api/workspaces` lists the workspaces and whether each one is reachable.

Docker also reads `PORT` (default `8080`), `WORKSPACE_DIR`, `WORKSPACE_TMUX`
(default `1`), `TERM_TOKEN` and `WORKSPACE_TOKEN` (shared by the gateway and
the workspaces).

**Front end** (query string): `http://localhost:8080/?workspace=python&fontSize=15&font=Fira+Code&token=...`

> [!WARNING]
> A connection is a shell running as the server's user. By default the server
> listens on localhost only and rejects cross-origin WebSockets, so other
> websites can't open a shell. Before exposing it on a network, set
> `TERM_TOKEN` (e.g. `openssl rand -hex 16`) and put it behind TLS.

## Usage

| action | how |
|---|---|
| Select | drag · double-click for a word · triple-click for a line |
| Copy / paste | <kbd>Cmd</kbd>+<kbd>C</kbd> / <kbd>Cmd</kbd>+<kbd>V</kbd> on macOS, <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>C</kbd> / <kbd>V</kbd> elsewhere |
| Scroll back | mouse wheel, <kbd>Shift</kbd>+<kbd>PgUp</kbd> / <kbd>PgDn</kbd> / <kbd>Home</kbd> / <kbd>End</kbd> |
| Select while an app uses the mouse | hold <kbd>Shift</kbd> |
| Reconnect after the shell exits | <kbd>Enter</kbd> |
| Switch workspace | click the workspace name in the header (middle-click an entry to open it in a new tab) |

## Features

<details>
<summary>Full list of supported terminal features</summary>

- xterm-compatible: cursor movement, erase/insert/delete, scroll regions,
  origin mode, tab stops, DEC line drawing, alternate screen (1049), REP,
  DSR/DA/DECRQM/XTVERSION replies, soft/hard reset
- SGR: bold, dim, italic, underline (single/double/curly), strikethrough,
  inverse, hidden; 16/256/truecolor
- Wide (CJK) characters and color emoji (font fallback is done by the browser)
- Box drawing, block elements and Powerline separators drawn procedurally
- 10k-line scrollback
- Mouse reporting (1000/1002/1003, SGR 1006); the wheel sends arrow keys in
  full-screen apps without mouse support
- Bracketed paste, focus events, synchronized output (2026), OSC 0/2 title,
  OSC 52 clipboard, OSC 4/10/11/12 color queries
- IME and dead keys via a hidden textarea; macOS Option-composed characters pass through
- HiDPI aware

</details>

**Not yet:** text reflow on resize, ligatures, combining characters (currently
dropped), kitty keyboard/graphics protocols, configurable themes.

## Development

```sh
just build                 # release build (web/pkg + target/release/term-server)
just profile=debug build   # debug build
just test                  # unit tests (cargo test --workspace)
just run                   # build, then serve your own shell on http://127.0.0.1:8080
```

`just --list` shows every recipe, including the Docker ones (`up`, `local`,
`server`, ...) described in the [Docker guide](docs/docker.md#commands).

Front-end changes only need `just build` and a page reload, because the server
reads `web/` from disk on every request. If `wasm-opt` is installed, the build
uses it to shrink the wasm.

## Acknowledgements

The design takes its cues from [Ghostty](https://github.com/ghostty-org/ghostty):
a standalone VT core, a GPU renderer with a glyph atlas, and procedurally drawn
box and Powerline glyphs. Parsing is built on [`vte`](https://github.com/alacritty/vte).

## License

[MIT](LICENSE)
