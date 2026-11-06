# syntax=docker/dockerfile:1.7
#
# Three images from one file, so the tmux client in the terminal sidecar and
# the tmux server in the workspace are the exact same version (the tmux
# client/server protocol is not stable across versions).

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

# ---- base: shared by workspace and sidecar ---------------------------------
FROM debian:trixie-slim AS base
# tmux replaces non-ASCII characters with '_' unless the locale is UTF-8.
ENV LANG=C.UTF-8 LC_ALL=C.UTF-8
RUN apt-get update \
 && apt-get install -y --no-install-recommends tmux ca-certificates \
 && rm -rf /var/lib/apt/lists/*
# Both containers run as the same uid: the tmux server only accepts clients
# running as its own user.
RUN useradd --create-home --uid 1000 --shell /bin/bash dev \
 && install -d -o dev -g dev /run/tmux

# ---- workspace: the dev environment ----------------------------------------
FROM base AS workspace
ARG TARGETARCH
ARG NVIM_VERSION=v0.11.5
ARG LAZYGIT_VERSION=0.55.1
ARG STARSHIP_VERSION=v1.23.0

# LazyVim needs git, a C compiler + tree-sitter CLI (parsers), ripgrep/fd/fzf
# (pickers), unzip/curl/node/python (Mason), lazygit and yarn (markdown-preview).
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      git fish curl build-essential unzip ripgrep fd-find fzf procps less \
      openssh-client nodejs npm python3 python3-venv \
 && rm -rf /var/lib/apt/lists/* \
 && ln -s /usr/bin/fdfind /usr/local/bin/fd \
 && npm install -g --no-fund --no-audit tree-sitter-cli yarn

# Debian's neovim is too old for LazyVim (needs >= 0.11.2).
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) arch=x86_64; triple=x86_64 ;; \
      arm64) arch=arm64;  triple=aarch64 ;; \
      *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    curl -fsSL "https://github.com/neovim/neovim/releases/download/${NVIM_VERSION}/nvim-linux-${arch}.tar.gz" | tar -xz -C /opt; \
    ln -s "/opt/nvim-linux-${arch}/bin/nvim" /usr/local/bin/nvim; \
    curl -fsSL "https://github.com/jesseduffield/lazygit/releases/download/v${LAZYGIT_VERSION}/lazygit_${LAZYGIT_VERSION}_Linux_${arch}.tar.gz" \
      | tar -xz -C /usr/local/bin lazygit; \
    curl -fsSL "https://github.com/starship/starship/releases/download/${STARSHIP_VERSION}/starship-${triple}-unknown-linux-musl.tar.gz" \
      | tar -xz -C /usr/local/bin starship

# tmux session persistence, loaded at the end of tmux.conf.
RUN git clone --depth 1 https://github.com/tmux-plugins/tmux-resurrect /usr/share/tmux-plugins/tmux-resurrect \
 && git clone --depth 1 https://github.com/tmux-plugins/tmux-continuum /usr/share/tmux-plugins/tmux-continuum

COPY docker/workspace/tmux.conf /etc/tmux/workspace.conf
COPY docker/workspace/config.fish /etc/fish/conf.d/workspace.fish
COPY docker/workspace/starship.toml /etc/starship.toml
COPY docker/workspace/nvim/init.lua /etc/workspace/nvim/init.lua
COPY --chmod=755 docker/workspace/entrypoint.sh /usr/local/bin/workspace-entrypoint

# Pre-create the home layout owned by dev, so the volume starts out writable.
RUN usermod --shell /usr/bin/fish dev \
 && mkdir -p /home/dev/.config/nvim /home/dev/.local/share \
      /home/dev/.local/state /home/dev/.cache /home/dev/workspace \
 && chown -R dev:dev /home/dev
USER dev
WORKDIR /home/dev
ENV EDITOR=nvim VISUAL=nvim STARSHIP_CONFIG=/etc/starship.toml
ENTRYPOINT ["workspace-entrypoint"]

# ---- terminal: the sidecar serving the browser terminal ---------------------
FROM base AS terminal
COPY --from=build /out/term-server /usr/local/bin/term-server
COPY --from=build /out/web /srv/web
COPY --chmod=755 docker/terminal/attach-workspace.sh /usr/local/bin/attach-workspace
USER dev
WORKDIR /home/dev
ENV BIND=0.0.0.0:8080 \
    STATIC_DIR=/srv/web \
    TERM_SHELL=/usr/local/bin/attach-workspace \
    TERM_ARGS=""
EXPOSE 8080
CMD ["term-server"]
