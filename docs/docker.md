# Docker workspaces

webterm can run your development environments in Docker and open them in a
browser tab. Each **workspace** is a container with a language toolchain (Go,
Python, Rust, ...) plus the same editor setup: tmux, fish, starship, Neovim with
LazyVim, lazygit, fzf, ripgrep and fd.

- [Getting started](#getting-started)
- [Everyday use](#everyday-use)
- [Workspace scenarios](#workspace-scenarios)
- [How it fits together](#how-it-fits-together)
- [Troubleshooting](#troubleshooting)

## Getting started

### 1. Prerequisites

| tool | why | check |
|---|---|---|
| Docker with Compose v2 | builds and runs the containers | `docker compose version` |
| [`just`](https://github.com/casey/just) (optional) | short commands for the steps below | `just --version` |

Any Docker works: Docker Desktop, OrbStack or Colima. With **Colima**, install the
Docker CLI and the Compose plugin separately:

```sh
brew install colima docker docker-compose
mkdir -p ~/.docker/cli-plugins
ln -sfn "$(brew --prefix)/opt/docker-compose/bin/docker-compose" ~/.docker/cli-plugins/docker-compose
colima start --cpu 4 --memory 8
```

### 2. Start

```sh
git clone https://github.com/mymichu/webterm.git
cd webterm
just up                   # or: docker compose up -d --build --remove-orphans
```

The first run builds three images (`go`, `python` and the `terminal` sidecar),
which takes a few minutes. Each workspace then installs its Neovim plugins once,
which takes about a minute. Follow along with:

```sh
docker compose logs -f go python
```

A workspace is ready when its log shows `workspace: tmux server on /run/tmux/<name>`
and `docker compose ps` lists it as `healthy`.

### 3. Open

Go to **<http://localhost:8080>**. You land in the first workspace from
[`docker/webterm.yaml`](../docker/webterm.yaml) (`go`), inside a tmux session.
Your code lives in `~/workspace`, which is the `./workspace` folder of this repo
on your machine.

The header shows the current workspace. Click it to open the side panel and switch
to `python`.

### 4. Stop

```sh
just down                 # or: docker compose down
```

This stops and removes the containers, but keeps your data: plugins, Mason
tools, shell history and tmux-resurrect saves live in volumes, and your code in
`./workspace`. `just up` brings everything back.

## Everyday use

### Commands

| command | what it does |
|---|---|
| `just up` | build if needed, then start everything |
| `just down` | stop everything (data is kept) |
| `docker compose ps` | status of each container |
| `docker compose logs -f go` | follow a workspace's log |
| `docker compose restart terminal` | reload `docker/webterm.yaml` |
| `docker compose up -d --build go` | rebuild one workspace after changing its config |
| `docker compose exec go fish` | a shell in a workspace without the browser |

### In the browser

| | |
|---|---|
| Switch workspace | click the workspace name in the header |
| Open a workspace in a new tab | middle-click it in the side panel, or open `http://localhost:8080/?workspace=python` |
| tmux prefix | <kbd>Ctrl</kbd>+<kbd>B</kbd> (`\|` and `-` split, `h` `j` `k` `l` move between panes, `c` new window) |
| Save / restore tmux sessions | prefix <kbd>Ctrl</kbd>+<kbd>S</kbd> / <kbd>Ctrl</kbd>+<kbd>R</kbd> (also saved automatically every 5 minutes) |
| Search shell history | <kbd>Ctrl</kbd>+<kbd>R</kbd> |
| Insert a file or directory path | <kbd>Ctrl</kbd>+<kbd>T</kbd> (taken by the browser on Windows/Linux) |
| `cd` into a directory | <kbd>Alt</kbd>+<kbd>C</kbd> |
| Neovim / lazygit | `nvim` (or `vi`), `lazygit`, or <kbd>Space</kbd> <kbd>g</kbd> <kbd>g</kbd> inside Neovim |

All tabs on the same workspace share its tmux session, so a second tab (or a
second computer) shows the same windows. Closing the tab doesn't stop anything;
the session keeps running in the container.

### Settings

Set these in your shell or in a `.env` file next to `compose.yaml` (add `.env`
to `.gitignore` first if it holds a token):

| variable | default | meaning |
|---|---|---|
| `PORT` | `8080` | port on `127.0.0.1` the page is served on |
| `WORKSPACE_DIR` | `./workspace` | host folder mounted as `~/workspace` in every workspace |
| `TERM_TOKEN` | unset | require `http://localhost:8080/?token=<value>` |

The port is only reachable from your own machine. If you expose it on a network,
set `TERM_TOKEN` (e.g. `TERM_TOKEN=$(openssl rand -hex 16) just up`) and put it
behind TLS: anyone who can connect gets a shell in your workspaces.

## Workspace scenarios

Two files describe the workspaces, and they have to agree:

- **[`compose.yaml`](../compose.yaml)** has one service per workspace: which base
  image it's built from and where its data lives.
- **[`docker/webterm.yaml`](../docker/webterm.yaml)** lists the workspaces the
  browser can open, in the order the side panel shows them. The first one is the
  default.

### Go and Python (the default)

This is what `just up` starts. Both workspaces share `./workspace`, so you can
open the same project in either one. Each has its own home volume, so the Go
workspace's Mason tools (gopls, ...) and the Python workspace's (pyright, ruff,
...) don't mix. LazyVim turns on its Go, Python or Rust support when that
toolchain is installed.

### A single workspace: just the terminal

When `docker/webterm.yaml` lists only one workspace, there's nothing to choose:
the page is just the terminal, full screen, with no header.

To keep only Go:

1. In `docker/webterm.yaml`, delete the `python` entry:

   ```yaml
   workspaces:
     - name: go
       description: Go · golang:1-trixie
       tmux_socket: /run/tmux/go
   ```

2. In `compose.yaml`, delete the `python` service and the `home-python` volume.
3. Run `just up`. The `--remove-orphans` flag removes the old `python` container.

The `home-python` volume stays on disk until you delete it
(`docker volume rm webterm_home-python`).

### Adding a workspace (Rust example)

1. In `compose.yaml`, copy the `python` service, then rename it and change its
   base image and volume:

   ```yaml
     rust:
       <<: *workspace
       build:
         context: .
         dockerfile: docker/Dockerfile
         target: workspace
         args:
           WORKSPACE_BASE: rust:1-trixie
       hostname: rust
       environment:
         WORKSPACE_NAME: rust
       volumes:
         - home-rust:/home/dev
         - tmux-socket:/run/tmux
         - ${WORKSPACE_DIR:-./workspace}:/home/dev/workspace
   ```

   and add the volume at the bottom:

   ```yaml
   volumes:
     home-rust:
   ```

2. In `docker/webterm.yaml`, add the entry. `tmux_socket` is always
   `/run/tmux/<WORKSPACE_NAME>`:

   ```yaml
     - name: rust
       description: Rust · rust:1-trixie
       tmux_socket: /run/tmux/rust
   ```

3. Run `just up`. The new workspace appears in the side panel once its first start
   is done.

**Which base images work:** any image based on **Debian trixie**, e.g.
`golang:1-trixie`, `python:3-trixie`, `rust:1-trixie`, `node:24-trixie` or
`buildpack-deps:trixie` for a general-purpose one. Other distributions, and other
Debian releases like bookworm, don't work: the sidecar's tmux client has to be
the exact same version as each workspace's tmux server, and both come from
Debian trixie.

### Removing or resetting a workspace

- **Remove:** delete its service from `compose.yaml` and its entry from
  `docker/webterm.yaml`, then run `just up`.
- **Stop for now:** `docker compose stop python`. It stays in the side panel,
  marked as stopped, until `docker compose start python`.
- **Reset to a clean home** (plugins, history and tmux saves are lost; code in
  `./workspace` is not touched):

  ```sh
  docker compose rm -sf python
  docker volume rm webterm_home-python
  just up
  ```

### Changing the shared editor setup

The tmux, fish, starship and LazyVim configs live in
[`docker/workspace/`](../docker/workspace) and are built into every workspace.
Edit them and run `just up` to rebuild:

| file | what it configures |
|---|---|
| `tmux.conf` | tmux: prefix, key bindings, status line, session saving |
| `config.fish` | fish: aliases, fzf keys, starship prompt |
| `starship.toml` | the prompt |
| `nvim/init.lua` | LazyVim and its language extras |

The tool versions (Neovim, lazygit, starship) are set as `ARG`s at the top of the
`devtools` stage in [`docker/Dockerfile`](../docker/Dockerfile).

## How it fits together

```
 browser ──▶ terminal (sidecar) ── tmux attach ──▶ /run/tmux/go      ◀── go workspace
             term-server                           /run/tmux/python  ◀── python workspace
             reads docker/webterm.yaml                (shared volume)
```

- Every **workspace** runs a tmux server, with its socket in a volume shared with
  the sidecar. Your shells, editors and programs all run in the workspace.
- The **terminal** sidecar serves the page. When you open a workspace, it runs
  `tmux attach` against that workspace's socket and streams the terminal over a
  WebSocket. It's a small distroless image with no shell. No SSH, Docker socket
  or extra privileges are involved.
- The workspace images are built in layers: the language base image, then one
  shared **devtools** layer (Neovim, LazyVim config, lazygit, starship, tmux
  plugins) that doesn't depend on the base. Docker stores it once for all
  workspaces.

## Troubleshooting

**`Cannot connect to the Docker daemon` with Colima, although `colima start` says
it's running.** The socket forwarding is stuck. Run `colima restart`.

**A workspace is marked as stopped, or the page shows a tmux error such as
`no server running` or `Connection refused`.**
Check `docker compose ps` and `docker compose logs <name>`. On the first start,
the workspace only starts tmux after its Neovim plugins are installed.

**The side panel doesn't show a workspace I added.** It has to be in
`docker/webterm.yaml` as well as in `compose.yaml`. Run
`docker compose restart terminal` after editing `docker/webterm.yaml`.

**The `terminal` container keeps restarting.** `docker/webterm.yaml` is probably
invalid; `docker compose logs terminal` names the problem, such as a duplicate
name, an unknown key, or an entry with neither `tmux_socket` nor `command`.

**Neovim shows errors on first open.** The plugin install may still be running in
the background, or have been cut short. Run `:Lazy sync` in Neovim.

**<kbd>Ctrl</kbd>+<kbd>T</kbd> or <kbd>Ctrl</kbd>+<kbd>W</kbd> act on the browser
instead of the terminal.** On Windows and Linux the browser keeps these keys for
itself, and web pages can't receive them. On macOS they reach the terminal.
