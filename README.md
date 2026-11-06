# webterm

A GPU-accelerated terminal emulator written in Rust, running in the browser as
WebAssembly, with a Rust backend that runs the actual shell. The design takes
its cues from [Ghostty](https://github.com/ghostty-org/ghostty): a standalone
VT core library, a WebGL renderer with a glyph atlas, procedurally drawn box /
block / Powerline glyphs, and correct xterm behavior.

```
 Browser                                          Backend (term-server)
┌──────────────────────────────────────┐  ws   ┌─────────────────────────┐
│ web (wasm)                           │ ───── │ axum  /ws               │
│  keyboard/IME/paste/mouse ──encode──▶│ Input │  ├─ PTY (portable-pty)  │
│  term-core: vte parser → grid ◀──────│Output │  │   └─ $SHELL -l       │
│  WebGL2 renderer (1 draw call/frame) │Resize │  └─ static files (web/) │
└──────────────────────────────────────┘       └─────────────────────────┘
```

## Crates

| crate | what |
|---|---|
| `crates/term-core` | Platform-independent VT emulator: parser (`vte`), screen grid, scrollback, alternate screen, scroll regions, modes, selection, key & mouse encoding. Unit tested natively. |
| `crates/web` | The wasm front end: WebGL2 renderer, glyph atlas, DOM input handling, WebSocket client. |
| `crates/server` | Backend: serves `web/`, spawns one shell in a PTY per WebSocket connection. |
| `crates/protocol` | Binary wire format shared by both sides (1-byte tag + payload). |

## Build & run

Requirements: Rust with the `wasm32-unknown-unknown` target, and a matching
`wasm-bindgen` CLI:

```sh
cargo install wasm-bindgen-cli --version 0.2.130
just build    # builds web/pkg/*.wasm and target/release/term-server
just run      # build, then serve on http://127.0.0.1:8080
just test     # unit tests
```

`just profile=debug build` makes a debug build. Front-end changes only need
`just build` and a page reload; the server reads `web/` from disk on every
request. (`./build.sh` works without `just`.)

## Docker: dev container with a terminal sidecar

```sh
just up      # http://localhost:8080
just down
```

`compose.yaml` runs two containers built from `docker/Dockerfile`:

- **workspace**: the dev environment (Debian trixie, tmux, fish + starship,
  Neovim + LazyVim, git, lazygit, ripgrep/fd/fzf, node, python). It runs the
  tmux server and keeps a `main` session alive. Code lives in `./workspace`
  (override with `WORKSPACE_DIR`). The home directory is a volume, so
  plugins, Mason tools, history and tmux-resurrect saves persist.
- **terminal**: the sidecar. It shares the workspace's network namespace and
  a volume holding the tmux socket, and runs `term-server`. Each browser tab
  starts `tmux attach` against the workspace's server, so every process runs
  in the workspace; the sidecar only holds tmux clients. No sshd, Docker
  socket or extra privileges are involved.

The tmux (prefix `C-b`, with resurrect + continuum), LazyVim, fish and
starship config lives in `docker/workspace/` and is built into the image; edit
it there and rerun `just up`. The first start installs LazyVim's plugins, which
takes about a minute (`docker compose logs -f workspace`).

## Configuration

Server (environment variables):

| variable | default | meaning |
|---|---|---|
| `BIND` | `127.0.0.1:8080` | listen address |
| `TERM_SHELL` | `$SHELL` | program to run for each connection |
| `TERM_ARGS` | `-l` | its arguments (whitespace-separated) |
| `TERM_TOKEN` | unset | if set, `/ws` requires `?token=...` |
| `ALLOWED_ORIGINS` | unset | extra comma-separated origins allowed to connect |
| `STATIC_DIR` | `web/` | front-end directory |

Front end (query string): `?fontSize=15&font=Fira+Code&token=...`

**Security:** a connection to `/ws` is a shell as the user running the server.
By default the server binds to localhost and rejects cross-origin WebSocket
connections (so another website can't open a shell on your machine). Set
`TERM_TOKEN` and put it behind TLS before exposing it on a network.

## Features

- xterm-compatible: cursor movement, erase/insert/delete, scroll regions,
  origin mode, tab stops, DEC line drawing, alternate screen (1049), REP,
  DSR/DA/DECRQM/XTVERSION replies, soft/hard reset
- SGR: bold, dim, italic, underline (single/double/curly), strikethrough,
  inverse, hidden; 16/256/truecolor
- Wide (CJK) characters and color emoji (font fallback is done by the browser)
- Box drawing, block elements and Powerline separators drawn procedurally, so
  they join with no gaps
- 10k-line scrollback (wheel, Shift+PageUp/PageDown/Home/End)
- Mouse reporting (1000/1002/1003, SGR 1006); hold Shift to select anyway.
  Wheel sends arrow keys in full-screen apps without mouse support
- Selection: drag, double-click word, triple-click line. Copy with Cmd+C
  (macOS) or Ctrl+Shift+C
- Bracketed paste, focus events, synchronized output (2026), OSC 0/2 title,
  OSC 52 clipboard, OSC 4/10/11/12 color queries
- IME / dead keys via a hidden textarea; macOS Option-composed characters pass through
- HiDPI aware; reconnect with Enter after the session ends

Not implemented yet: text reflow on resize, ligatures, combining characters
(dropped), kitty keyboard/graphics protocols, configurable themes.

## Tests

```sh
cargo test --workspace
```

## License

[MIT](LICENSE)
