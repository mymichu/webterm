#!/bin/sh
# Runs the tmux server that the terminal sidecar attaches to, and keeps a
# "main" session alive so there is always something to attach to.
#
# Each workspace container has its own socket, /run/tmux/$WORKSPACE_NAME, in
# the volume shared with the sidecar (see docker/webterm.yaml).
set -eu

NAME="${WORKSPACE_NAME:-$(hostname)}"
SOCKET_DIR="${TMUX_SOCKET_DIR:-/run/tmux}"
SOCKET="$SOCKET_DIR/$NAME"
SESSION="${TMUX_SESSION:-main}"
tmux_() { tmux -u -S "$SOCKET" -f /opt/devtools/etc/tmux.conf "$@"; }

# The nvim config ships in the image; the home directory is a volume, so copy
# it in on every start to pick up image updates (lazy-lock.json is kept).
mkdir -p "$HOME/.config/nvim"
rm -f "$HOME/.config/nvim/init.lua"
cp /opt/devtools/etc/nvim/init.lua "$HOME/.config/nvim/init.lua"

# First start: install LazyVim's plugins so nvim is ready when first opened.
# They live in the persistent home volume, so this only happens once.
if [ ! -d "$HOME/.local/share/nvim/lazy/LazyVim" ]; then
    echo "workspace: installing LazyVim plugins (first start only)..."
    timeout 900 nvim --headless "+Lazy! sync" +qa \
        || echo "workspace: plugin install incomplete; nvim will finish it on first launch"
fi

echo "workspace: tmux server on $SOCKET"
while true; do
    tmux_ has-session -t "$SESSION" 2>/dev/null \
        || tmux_ new-session -d -s "$SESSION" -c "$HOME/workspace"
    sleep 2
done
