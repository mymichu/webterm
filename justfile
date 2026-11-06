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

# Start the dockerized workspaces + terminal sidecar on http://localhost:8080
up:
    docker compose up -d --build --remove-orphans
    @echo "http://localhost:${PORT:-8080}  (first start installs LazyVim plugins: docker compose logs -f go python)"

# Stop the containers (volumes with plugins and history are kept)
down:
    docker compose down
