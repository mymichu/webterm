# Build profile: `just profile=debug build`
profile := "release"

# The Nix toolchain on macOS links against the SDK's libiconv and its std is
# built for macOS 14; these keep native builds linking without warnings.
export LIBRARY_PATH := if os() == "macos" { `xcrun --show-sdk-path 2>/dev/null || true` + "/usr/lib:" + env("LIBRARY_PATH", "") } else { env("LIBRARY_PATH", "") }
export MACOSX_DEPLOYMENT_TARGET := if os() == "macos" { env("MACOSX_DEPLOYMENT_TARGET", "14.0") } else { "" }
export PATH := env("HOME") + "/.cargo/bin:" + env("PATH")
export PROFILE := profile

[private]
default:
    @just --list

# Build the wasm front end (web/pkg) and the backend server
build:
    #!/usr/bin/env bash
    set -euo pipefail
    # rustc needs lld to link wasm; fetch it from nixpkgs if it isn't installed.
    if ! command -v lld >/dev/null && ! command -v wasm-ld >/dev/null && command -v nix >/dev/null; then
        export PATH="$(nix build nixpkgs#lld --print-out-paths --no-link)/bin:$PATH"
    fi
    ./build.sh

# Build, then start the server on $BIND (default http://127.0.0.1:8080)
run: build
    ./target/{{profile}}/term-server

# Run all unit tests
test:
    cargo test --workspace

# ---- Docker: demo (compose.yaml) ---------------------------------------------

# Start the demo: gateway + Go and Python workspaces on http://localhost:8080
up:
    docker compose up -d --build --remove-orphans
    @echo "http://localhost:${PORT:-8080}  (first start installs LazyVim plugins: docker compose logs -f go python)"

# Stop the demo (volumes with plugins and history are kept)
down:
    docker compose down

# ---- Docker: local development (examples/local) ------------------------------
# One workspace per project, opened directly; the compose project is named
# after the project folder, so each project keeps its own home volume.

# Start a workspace for a project folder: just local ~/code/myapp [base] [port]
local project base="golang:1-trixie" port="8080":
    PROJECT_DIR="{{join(invocation_directory(), project)}}" WORKSPACE_BASE="{{base}}" PORT="{{port}}" \
      docker compose -f examples/local/compose.yaml -p "webterm-local-{{lowercase(replace_regex(file_name(join(invocation_directory(), project)), '[^A-Za-z0-9_-]', '-'))}}" up -d --build
    @echo "http://localhost:{{port}}  ({{base}}, {{join(invocation_directory(), project)}} → ~/workspace)"

# Stop a project's workspace (its home volume is kept)
local-down project:
    docker compose -f examples/local/compose.yaml -p "webterm-local-{{lowercase(replace_regex(file_name(join(invocation_directory(), project)), '[^A-Za-z0-9_-]', '-'))}}" down

# List the running project workspaces
local-ls:
    docker compose ls --filter name=webterm-local-

# ---- Docker: server (examples/server) -----------------------------------------
# Caddy (HTTPS) → gateway → workspaces; configured in examples/server/.env.

# Start the server setup (needs examples/server/.env, see .env.example)
server:
    @test -f examples/server/.env || { echo "create examples/server/.env first: cp examples/server/.env.example examples/server/.env"; exit 1; }
    docker compose -f examples/server/compose.yaml up -d --build --remove-orphans

# Stop the server setup (volumes with code, homes and certificates are kept)
server-down:
    docker compose -f examples/server/compose.yaml down

# Follow the server setup's logs, or one service's: just server-logs gateway
server-logs *service:
    docker compose -f examples/server/compose.yaml logs -f {{service}}
