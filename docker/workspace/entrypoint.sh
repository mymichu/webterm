#!/bin/sh
# Serves this workspace's browser terminal on $BIND (:8080): open it directly
# by publishing the port, or through the gateway (see docker/webterm.yaml).
#
# WORKSPACE_TMUX=1 (default): every tab attaches to the tmux session "main",
# which keeps running when the tab closes and is shared by all tabs.
# WORKSPACE_TMUX=0: every tab gets its own fish shell, ended with the tab.
set -eu

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

export TERM_CWD="$HOME/workspace"
if [ "${WORKSPACE_TMUX:-1}" = 1 ]; then
    export TERM_SHELL=tmux TERM_ARGS="-u -f /opt/devtools/etc/tmux.conf new-session -A -s main"
else
    export TERM_SHELL=fish TERM_ARGS=-l
fi
echo "workspace: terminal on $BIND ($TERM_SHELL)"
exec term-server
