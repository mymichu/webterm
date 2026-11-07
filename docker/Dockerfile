# syntax=docker/dockerfile:1.7
#
# Images built from this file:
#   workspace  a dev environment: an apt-based base image (golang, python, ...)
#              plus the shared devtools layer (term-server, tmux config, fish,
#              LazyVim, ...). It serves its own browser terminal on :8080.
#   gateway    one URL for several workspaces: relays each browser tab to the
#              chosen workspace's term-server (distroless, static).

# Base image of a workspace; compose builds one workspace per base.
ARG WORKSPACE_BASE=buildpack-deps:trixie

# ---- build: Rust backend + WebAssembly front end ---------------------------
# term-server is linked statically (musl), so it runs on any workspace base.
FROM rust:1-trixie AS build
RUN rustup target add wasm32-unknown-unknown "$(uname -m)-unknown-linux-musl" \
 && cargo install wasm-bindgen-cli --version 0.2.130 --locked
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY web/index.html web/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    target="$(uname -m)-unknown-linux-musl" \
 && cargo build --release -p server --target "$target" \
 && cargo build --release -p web --target wasm32-unknown-unknown \
 && mkdir -p /out/web \
 && cp "target/$target/release/term-server" /out/ \
 && cp web/index.html /out/web/ \
 && wasm-bindgen --target web --no-typescript --out-dir /out/web/pkg \
      target/wasm32-unknown-unknown/release/web.wasm

# ---- devtools: the editor setup shared by every workspace -------------------
# Self-contained under /opt/devtools and independent of the workspace base, so
# `COPY --link` adds it to each workspace as the same, shared layer.
FROM debian:trixie-slim AS devtools
ARG TARGETARCH
ARG NVIM_VERSION=v0.11.5
ARG LAZYGIT_VERSION=0.55.1
ARG STARSHIP_VERSION=v1.23.0
ARG FZF_VERSION=0.62.0
RUN apt-get update \
 && apt-get install -y --no-install-recommends curl ca-certificates git \
 && rm -rf /var/lib/apt/lists/*
# Debian's neovim is too old for LazyVim (needs >= 0.11.2); fzf comes from
# upstream too, as `fzf --fish` needs >= 0.48 and distros ship older ones.
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
      | tar -xz -C /opt/devtools/bin starship; \
    curl -fsSL "https://github.com/junegunn/fzf/releases/download/v${FZF_VERSION}/fzf-${FZF_VERSION}-linux_${TARGETARCH}.tar.gz" \
      | tar -xz -C /opt/devtools/bin fzf
# tmux session persistence, loaded at the end of tmux.conf.
RUN git clone --depth 1 https://github.com/tmux-plugins/tmux-resurrect /opt/devtools/tmux-plugins/tmux-resurrect \
 && git clone --depth 1 https://github.com/tmux-plugins/tmux-continuum /opt/devtools/tmux-plugins/tmux-continuum
COPY docker/workspace/tmux.conf docker/workspace/starship.toml /opt/devtools/etc/
COPY docker/workspace/config.fish /opt/devtools/etc/fish/workspace.fish
COPY docker/workspace/nvim/init.lua /opt/devtools/etc/nvim/init.lua
COPY --chmod=755 docker/workspace/entrypoint.sh /opt/devtools/bin/workspace-entrypoint
COPY --from=build /out/term-server /opt/devtools/bin/term-server
COPY --from=build /out/web /opt/devtools/share/webterm

# ---- workspace: a dev environment on an apt-based base ----------------------
FROM ${WORKSPACE_BASE} AS workspace
ARG WORKSPACE_BASE
# tmux replaces non-ASCII characters with '_' unless the locale is UTF-8.
ENV LANG=C.UTF-8 LC_ALL=C.UTF-8 WORKSPACE_BASE=${WORKSPACE_BASE}

# LazyVim needs git, a C compiler + tree-sitter CLI (parsers), ripgrep/fd
# (pickers), unzip/curl/node/python (Mason) and yarn (markdown-preview). The
# buildpack-deps based images (golang, python, ...) already have most of it;
# Node and yarn come from the distro only when the base doesn't bring its own.
RUN set -eux; \
    node_pkgs="$(command -v node >/dev/null || echo nodejs npm)"; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
      tmux fish git curl ca-certificates build-essential unzip ripgrep fd-find \
      procps less openssh-client python3 python3-venv $node_pkgs; \
    rm -rf /var/lib/apt/lists/*; \
    ln -sf /usr/bin/fdfind /usr/local/bin/fd; \
    npm install -g --no-fund --no-audit tree-sitter-cli $(command -v yarn >/dev/null || echo yarn)

# The home layout is created here, owned by dev, so a fresh volume starts out
# writable. uid 1000 matches the usual host user, for files in ~/workspace;
# some bases (node, ubuntu) already have a user with it.
RUN existing="$(getent passwd 1000 | cut -d: -f1)" \
 && if [ -n "$existing" ]; then userdel --remove "$existing"; fi \
 && useradd --create-home --uid 1000 --shell /usr/bin/fish dev \
 && mkdir -p /home/dev/.config/nvim /home/dev/.config/fish /home/dev/.local/share \
      /home/dev/.local/state /home/dev/.cache /home/dev/workspace \
 && chown -R dev:dev /home/dev

COPY --link --from=devtools /opt/devtools /opt/devtools
RUN ln -s /opt/devtools/etc/fish/workspace.fish /etc/fish/conf.d/workspace.fish

USER dev
WORKDIR /home/dev
ENV PATH=/opt/devtools/bin:$PATH \
    EDITOR=nvim VISUAL=nvim \
    STARSHIP_CONFIG=/opt/devtools/etc/starship.toml \
    BIND=0.0.0.0:8080 \
    STATIC_DIR=/opt/devtools/share/webterm
EXPOSE 8080
ENTRYPOINT ["workspace-entrypoint"]

# ---- gateway: one URL for several workspaces --------------------------------
# No shell, no tmux: just term-server and the web files. Each connection is
# relayed to a workspace's term-server, as configured in docker/webterm.yaml.
FROM gcr.io/distroless/static-debian13:nonroot AS gateway
COPY --from=build /out/term-server /usr/local/bin/term-server
COPY --from=build /out/web /srv/web
COPY docker/webterm.yaml /etc/webterm/webterm.yaml
ENV BIND=0.0.0.0:8080 \
    STATIC_DIR=/srv/web \
    TERM_CONFIG=/etc/webterm/webterm.yaml
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/term-server"]
