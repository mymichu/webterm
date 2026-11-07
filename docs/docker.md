# Docker workspaces

webterm can run your development environments in Docker and open them in a
browser tab. Each **workspace** is a container with a language toolchain (Go,
Python, Rust, ...) plus the same editor setup: tmux, fish, starship, Neovim with
LazyVim, lazygit, fzf, ripgrep and fd.

- [Two ways to run it](#two-ways-to-run-it)
- [Getting started](#getting-started)
- [Everyday use](#everyday-use)
- [Workspace scenarios](#workspace-scenarios)
- [How it fits together](#how-it-fits-together)
- [Troubleshooting](#troubleshooting)

## Two ways to run it

Every workspace serves its own browser terminal. The **gateway** is an
optional extra that puts several workspaces behind one URL. Which setup fits
depends on where it runs:

| | **Local development** | **Server** |
|---|---|---|
| Example | [`examples/local`](../examples/local) | [`examples/server`](../examples/server) |
| Gateway | no: open the workspace directly | yes, behind Caddy for HTTPS |
| Workspaces | one per project, each its own compose project and port | as many as you like, one URL, switched in the header |
| Code | your project folder, bind-mounted from your machine | a volume on the server, shared by the workspaces |
| Reachable from | `127.0.0.1` only | the internet (or your VPN), over HTTPS |
| Auth | none needed | `TERM_TOKEN` for browsers, `WORKSPACE_TOKEN` between gateway and workspaces |
| Resources | whatever Docker has | CPU and memory limits per workspace |

The [`compose.yaml`](../compose.yaml) at the top of the repo sits in between:
it runs the server layout (gateway with Go and Python) on localhost without
TLS, so you can try everything with one `just up`. The
[getting started](#getting-started) steps below use it.

### Local development: one workspace per project

You work on a project on your laptop and want its toolchain and editor setup in
a container, without installing anything. Start a workspace for it and open it
directly; there's no gateway to configure.

```sh
just local ~/code/myapp                    # → http://localhost:8080
```

The project folder is mounted as `~/workspace`, so your files stay on your
machine and your host editor and git see the same files. The optional second
and third arguments pick the toolchain (any apt-based image) and the port, so
several projects can run side by side:

```sh
just local ~/code/api python:3-trixie 8081   # → http://localhost:8081
just local-ls                                # list running project workspaces
just local-down ~/code/api                   # stop it (its home volume is kept)
```

Each project gets its own compose project, named after its folder
(`webterm-local-api`), and so its own home volume with plugins, history and
the tmux session. Starting it again later picks all of that up.

<details>
<summary>Without just</summary>

```sh
PROJECT_DIR=~/code/api WORKSPACE_BASE=python:3-trixie PORT=8081 \
  docker compose -f examples/local/compose.yaml -p webterm-local-api up -d --build
docker compose -f examples/local/compose.yaml -p webterm-local-api down
```

</details>

### Server: many workspaces behind one URL

You run webterm on a server, a cloud VM or a home lab box, for yourself from
anywhere or for a team. Only HTTPS is exposed; the workspaces are reachable
only through the gateway.

```
internet ──https──▶ caddy ──▶ gateway ──▶ go:8080, python:8080, ...
                    (TLS)     (token)     (WORKSPACE_TOKEN, no published ports)
```

```sh
cp examples/server/.env.example examples/server/.env
$EDITOR examples/server/.env     # DOMAIN, TERM_TOKEN, WORKSPACE_TOKEN (openssl rand -hex 32)
just server                      # → https://$DOMAIN/?token=$TERM_TOKEN
just server-logs gateway         # follow the logs (all services without an argument)
just server-down                 # stop (code, homes and certificates are kept)
```

Without just, run `docker compose up -d --build` (or `logs -f`, `down`) in
`examples/server`.

- The DNS record for `DOMAIN` must point at the server, with ports 80 and 443
  open, so Caddy can get a Let's Encrypt certificate.
- Caddy only reaches the gateway, and only the gateway reaches the workspaces
  (two Docker networks). Every workspace has CPU and memory limits, so one
  busy build doesn't slow down the others.
- The code lives in the `code` volume on the server; clone your repositories
  into `~/workspace` from any workspace.
- Workspaces are added the same way as below: a service in `compose.yaml` and
  an entry in `webterm.yaml`.
- The token in the URL is the only login. For a team, put real
  authentication in front, e.g. Caddy's `forward_auth` with an identity
  provider, or keep the server on a VPN like Tailscale or WireGuard.

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

The first run builds three images (`go`, `python` and the `gateway`), which
takes a few minutes. Each workspace then installs its Neovim plugins once,
which takes about a minute. Follow along with:

```sh
docker compose logs -f go python
```

A workspace is ready when its log shows `workspace: terminal on 0.0.0.0:8080`
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

The [`justfile`](../justfile) has a recipe for each setup (`just --list` shows
them all):

| recipe | setup | what it does |
|---|---|---|
| `just up` | demo | build if needed, then start the gateway with Go and Python on :8080 |
| `just down` | demo | stop it (data is kept) |
| `just local <dir> [base] [port]` | local | start a workspace for a project folder, default `golang:1-trixie` on :8080 |
| `just local-down <dir>` | local | stop that project's workspace (its home volume is kept) |
| `just local-ls` | local | list the running project workspaces |
| `just server` | server | start Caddy, the gateway and the workspaces (needs `examples/server/.env`) |
| `just server-logs [service]` | server | follow the logs, of all services or one |
| `just server-down` | server | stop it (code, homes and certificates are kept) |

For the demo, plain `docker compose` works from the repo folder:

| command | what it does |
|---|---|
| `docker compose ps` | status of each container |
| `docker compose logs -f go` | follow a workspace's log |
| `docker compose restart gateway` | reload `docker/webterm.yaml` |
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
the session keeps running in the container. (Without tmux, see
[below](#without-tmux), each tab is its own shell.)

### Settings

Set these in your shell or in a `.env` file next to `compose.yaml` (add `.env`
to `.gitignore` first if it holds a token):

| variable | default | meaning |
|---|---|---|
| `PORT` | `8080` | port on `127.0.0.1` the gateway is served on |
| `WORKSPACE_DIR` | `./workspace` | host folder mounted as `~/workspace` in every workspace |
| `WORKSPACE_TMUX` | `1` | `1`: tabs attach to a persistent tmux session; `0`: a plain fish shell per tab |
| `TERM_TOKEN` | unset | require `http://localhost:8080/?token=<value>` |
| `WORKSPACE_TOKEN` | unset | shared secret between the gateway and the workspaces |

The port is only reachable from your own machine. If you expose it on a network,
set `TERM_TOKEN` (e.g. `TERM_TOKEN=$(openssl rand -hex 16) just up`) and put it
behind TLS: anyone who can connect gets a shell in your workspaces.

The workspaces' own port 8080 is only reachable on the compose network, so by
default any container on it could open a shell in them. To lock that down, set
`WORKSPACE_TOKEN` as well: the gateway sends it, and the workspaces require it.

## Workspace scenarios

Two files describe the workspaces, and they have to agree:

- **[`compose.yaml`](../compose.yaml)** has one service per workspace: which base
  image it's built from and where its data lives.
- **[`docker/webterm.yaml`](../docker/webterm.yaml)** lists the workspaces the
  gateway offers, in the order the side panel shows them. The first one is the
  default.

### Go and Python (the default)

This is what `just up` starts. Both workspaces share `./workspace`, so you can
open the same project in either one. Each has its own home volume, so the Go
workspace's Mason tools (gopls, ...) and the Python workspace's (pyright, ruff,
...) don't mix. LazyVim turns on its Go, Python or Rust support when that
toolchain is installed.

### A single workspace, without the gateway

Every workspace serves its own terminal, so with just one you don't need the
gateway: publish the workspace's port and open it directly. The page is just
the terminal, full screen, with no header.

To keep only Go:

1. In `compose.yaml`, delete the `python` and `gateway` services and the
   `home-python` volume, and publish Go's port:

   ```yaml
     go:
       # ...
       ports:
         - "127.0.0.1:8080:8080"
   ```

2. Run `just up` and open <http://localhost:8080>. The `--remove-orphans` flag
   removes the old containers.

The `home-python` volume stays on disk until you delete it
(`docker volume rm webterm_home-python`).

You can also keep the gateway and publish a workspace next to it, e.g.
`127.0.0.1:8081:8080` (commented out in `compose.yaml`), to open that workspace
both ways.

### Without tmux

tmux is optional. With `WORKSPACE_TMUX=0`, each tab gets its own fish shell in
`~/workspace`, which ends when the tab closes:

```sh
WORKSPACE_TMUX=0 just up
```

To change it for one workspace only, set `WORKSPACE_TMUX: 0` in that service's
`environment` in `compose.yaml`.

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
       volumes:
         - home-rust:/home/dev
         - ${WORKSPACE_DIR:-./workspace}:/home/dev/workspace
   ```

   and add the volume at the bottom:

   ```yaml
   volumes:
     home-rust:
   ```

2. In `docker/webterm.yaml`, add the entry. The URL uses the service name:

   ```yaml
     - name: rust
       description: Rust · rust:1-trixie
       url: ws://rust:8080/ws
   ```

3. Run `just up`. The new workspace appears in the side panel once its first start
   is done.

**Which base images work:** any image with `apt`, i.e. based on Debian or
Ubuntu, e.g. `golang:1-trixie`, `python:3-trixie`, `rust:1-trixie`,
`node:24-trixie` or `buildpack-deps:trixie` for a general-purpose one. The
workspace's term-server is a static binary and doesn't care about the distro;
only the package installation in `docker/Dockerfile` uses `apt`.

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
| `entrypoint.sh` | what a tab runs (tmux or fish) |

The tool versions (Neovim, lazygit, starship, fzf) are set as `ARG`s at the top
of the `devtools` stage in [`docker/Dockerfile`](../docker/Dockerfile).

## How it fits together

```
 browser ──▶ gateway :8080 ──▶ go:8080      term-server → tmux session "main" (or fish)
             term-server       python:8080  term-server → tmux session "main" (or fish)
             docker/webterm.yaml
```

- Every **workspace** runs `term-server` on port 8080. A connection gets a
  terminal running `tmux new-session -A -s main` (or `fish -l`), so your
  shells, editors and programs all run in the workspace.
- The **gateway** is the same `term-server`, configured with the workspaces'
  URLs. It serves the page and the workspace list, and relays each tab's
  WebSocket to the chosen workspace without looking at the bytes. It's a 12 MB
  distroless image with no shell. No SSH, Docker socket or extra privileges are
  involved.
- The workspace images are built in layers: the language base image, then one
  shared **devtools** layer (term-server, Neovim, LazyVim config, lazygit, fzf,
  starship, tmux plugins) that doesn't depend on the base. Docker stores it
  once for all workspaces.

## Troubleshooting

**`Cannot connect to the Docker daemon` with Colima, although `colima start` says
it's running.** The socket forwarding is stuck. Run `colima restart`.

**A workspace is marked as stopped, or the page says
`workspace <name> is unreachable`.** Check `docker compose ps` and
`docker compose logs <name>`. On the first start, the workspace only starts its
terminal after its Neovim plugins are installed.

**The page says `invalid token` or the workspace is unreachable with a 401.**
`WORKSPACE_TOKEN` has to be the same for the gateway and the workspaces; run
`just up` after changing it so all containers pick it up.

**The side panel doesn't show a workspace I added.** It has to be in
`docker/webterm.yaml` as well as in `compose.yaml`. Run
`docker compose restart gateway` after editing `docker/webterm.yaml`.

**The `gateway` container keeps restarting.** `docker/webterm.yaml` is probably
invalid; `docker compose logs gateway` names the problem, such as a duplicate
name, an unknown key, or an entry with neither `url` nor `command`.

**Neovim shows errors on first open.** The plugin install may still be running in
the background, or have been cut short. Run `:Lazy sync` in Neovim.

**<kbd>Ctrl</kbd>+<kbd>T</kbd> or <kbd>Ctrl</kbd>+<kbd>W</kbd> act on the browser
instead of the terminal.** On Windows and Linux the browser keeps these keys for
itself, and web pages can't receive them. On macOS they reach the terminal.
