# syntax=docker/dockerfile:1.7
#
# Images built from this file:
#   workspace  a dev environment: any Debian trixie base image (golang, python,
#              ...) plus the shared devtools layer (tmux, fish, LazyVim, ...)
#   terminal   the sidecar serving the browser terminal (distroless)
#
# Every image takes tmux from Debian trixie, so the sidecar's tmux client and
# each workspace's tmux server are the exact same version (the tmux
# client/server protocol is not stable across versions).

# Base image of a workspace; compose builds one workspace per base.
ARG WORKSPACE_BASE=buildpack-deps:trixie

# ---- build: Rust backend + WebAssembly front end ---------------------------
FROM rust:1-trixie AS build
RUN rustup target add wasm32-unknown-unknown \
 && cargo install wasm-bindgen-cli --version 0.2.130 --locked
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY web/index.html web/index.html
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p server \
 && cargo build --release -p web --target wasm32-unknown-unknown \
 && mkdir -p /out/web \
 && cp target/release/term-server /out/ \
 && cp web/index.html /out/web/ \
 && wasm-bindgen --target web --no-typescript --out-dir /out/web/pkg \
      target/wasm32-unknown-unknown/release/web.wasm

# ---- base: Debian's tmux, for the sidecar ----------------------------------
FROM debian:trixie-slim AS base
# tmux replaces non-ASCII characters with '_' unless the locale is UTF-8.
ENV LANG=C.UTF-8 LC_ALL=C.UTF-8
RUN apt-get update \
 && apt-get install -y --no-install-recommends tmux ca-certificates \
 && rm -rf /var/lib/apt/lists/*
# All containers run as the same uid: a tmux server only accepts clients
# running as its own user.
RUN useradd --create-home --uid 1000 --shell /bin/bash dev

# ---- devtools: the editor setup shared by every workspace -------------------
# Self-contained under /opt/devtools and independent of the workspace base, so
# `COPY --link` adds it to each workspace as the same, shared layer.
FROM debian:trixie-slim AS devtools
ARG TARGETARCH
ARG NVIM_VERSION=v0.11.5
ARG LAZYGIT_VERSION=0.55.1
ARG STARSHIP_VERSION=v1.23.0
RUN apt-get update \
 && apt-get install -y --no-install-recommends curl ca-certificates git \
 && rm -rf /var/lib/apt/lists/*
# Debian's neovim is too old for LazyVim (needs >= 0.11.2).
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) arch=x86_64; triple=x86_64 ;; \
      arm64) arch=arm64;  triple=aarch64 ;; \
      *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    mkdir -p /opt/devtools/bin; \
    curl -fsSL "https://github.com/neovim/neovim/releases/download/${NVIM_VERSION}/nvim-linux-${arch}.tar.gz" \
      | tar -xz -C /opt/devtools; \
    ln -s "../nvim-linux-${arch}/bin/nvim" /opt/devtools/bin/nvim; \
    curl -fsSL "https://github.com/jesseduffield/lazygit/releases/download/v${LAZYGIT_VERSION}/lazygit_${LAZYGIT_VERSION}_Linux_${arch}.tar.gz" \
      | tar -xz -C /opt/devtools/bin lazygit; \
    curl -fsSL "https://github.com/starship/starship/releases/download/${STARSHIP_VERSION}/starship-${triple}-unknown-linux-musl.tar.gz" \
      | tar -xz -C /opt/devtools/bin starship
# tmux session persistence, loaded at the end of tmux.conf.
RUN git clone --depth 1 https://github.com/tmux-plugins/tmux-resurrect /opt/devtools/tmux-plugins/tmux-resurrect \
 && git clone --depth 1 https://github.com/tmux-plugins/tmux-continuum /opt/devtools/tmux-plugins/tmux-continuum
COPY docker/workspace/tmux.conf docker/workspace/starship.toml /opt/devtools/etc/
COPY docker/workspace/config.fish /opt/devtools/etc/fish/workspace.fish
COPY docker/workspace/nvim/init.lua /opt/devtools/etc/nvim/init.lua
COPY --chmod=755 docker/workspace/entrypoint.sh /opt/devtools/bin/workspace-entrypoint

# ---- workspace: a dev environment on any Debian trixie base -----------------
FROM ${WORKSPACE_BASE} AS workspace
ARG WORKSPACE_BASE
# tmux replaces non-ASCII characters with '_' unless the locale is UTF-8.
ENV LANG=C.UTF-8 LC_ALL=C.UTF-8 WORKSPACE_BASE=${WORKSPACE_BASE}

# LazyVim needs git, a C compiler + tree-sitter CLI (parsers), ripgrep/fd/fzf
# (pickers), unzip/curl/node/python (Mason) and yarn (markdown-preview). The
# buildpack-deps based images (golang, python, ...) already have most of it;
# Node and yarn come from Debian only when the base doesn't bring its own.
RUN set -eux; \
    node_pkgs="$(command -v node >/dev/null || echo nodejs npm)"; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
      tmux fish git curl ca-certificates build-essential unzip ripgrep fd-find fzf \
      procps less openssh-client python3 python3-venv $node_pkgs; \
    rm -rf /var/lib/apt/lists/*; \
    ln -sf /usr/bin/fdfind /usr/local/bin/fd; \
    npm install -g --no-fund --no-audit tree-sitter-cli $(command -v yarn >/dev/null || echo yarn)

# The home layout is created here, owned by dev, so a fresh volume starts out writable.
# dev must be uid 1000 to match the sidecar; some bases (node) already use it.
RUN existing="$(getent passwd 1000 | cut -d: -f1)" \
 && if [ -n "$existing" ]; then userdel --remove "$existing"; fi \
 && useradd --create-home --uid 1000 --shell /usr/bin/fish dev \
 && install -d -o dev -g dev /run/tmux \
 && mkdir -p /home/dev/.config/nvim /home/dev/.config/fish /home/dev/.local/share \
      /home/dev/.local/state /home/dev/.cache /home/dev/workspace \
 && chown -R dev:dev /home/dev

COPY --link --from=devtools /opt/devtools /opt/devtools
RUN ln -s /opt/devtools/etc/fish/workspace.fish /etc/fish/conf.d/workspace.fish

USER dev
WORKDIR /home/dev
ENV PATH=/opt/devtools/bin:$PATH \
    EDITOR=nvim VISUAL=nvim \
    STARSHIP_CONFIG=/opt/devtools/etc/starship.toml
ENTRYPOINT ["workspace-entrypoint"]

# ---- tmux-dist: Debian's tmux client and its shared libraries ---------------
# Taken from `base` so the sidecar's tmux matches the workspace's exactly.
# glibc itself is left out: the distroless image ships the same trixie build.
FROM base AS tmux-dist
RUN set -eu; \
    mkdir -p /out/usr/bin /out/etc; \
    install -d -o dev -g dev /out/run/tmux; \
    cp /usr/bin/tmux /out/usr/bin/; \
    ldd /usr/bin/tmux | awk '/=> \// { print $3 }' \
      | grep -vE '/(libc|libm|libresolv)\.so' \
      | while read -r lib; do \
          dir="/out$(dirname "$(readlink -f "$lib")")"; \
          mkdir -p "$dir"; cp -L "$lib" "$dir/"; \
        done; \
    mkdir -p /out/usr/share/terminfo/x; \
    cp -L "$(find /usr/lib/terminfo /usr/share/terminfo -name xterm-256color | head -n1)" /out/usr/share/terminfo/x/; \
    mkdir -p /out/usr/lib/locale; \
    cp -r /usr/lib/locale/C.utf8 /out/usr/lib/locale/; \
    grep -E '^(root|nobody|dev):' /etc/passwd > /out/etc/passwd; \
    grep -E '^(root|nogroup|dev):' /etc/group > /out/etc/group

# ---- terminal: the sidecar serving the browser terminal ---------------------
# No shell or package manager: just term-server, the web files and the tmux
# client. Each connection runs `tmux attach` against the chosen workspace, as
# configured in docker/webterm.yaml.
FROM gcr.io/distroless/cc-debian13 AS terminal
COPY --from=tmux-dist /out/ /
COPY --from=build /out/term-server /usr/local/bin/term-server
COPY --from=build /out/web /srv/web
COPY docker/webterm.yaml /etc/webterm/webterm.yaml
USER 1000:1000
ENV HOME=/ \
    LANG=C.UTF-8 \
    BIND=0.0.0.0:8080 \
    STATIC_DIR=/srv/web \
    TERM_CONFIG=/etc/webterm/webterm.yaml
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/term-server"]
