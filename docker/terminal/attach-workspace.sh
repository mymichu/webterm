#!/bin/sh
# Run by term-server for each browser connection: attaches to the tmux server
# in the workspace container through the shared socket volume. Every shell and
# editor runs in the workspace; this process is only the tmux client.
#
# `attach` never starts a server, so nothing can accidentally run in here.
SOCKET="${TMUX_SOCKET:-/run/tmux/default}"
SESSION="${TMUX_SESSION:-main}"

if ! tmux -S "$SOCKET" has-session -t "$SESSION" 2>/dev/null; then
    printf 'Waiting for the workspace tmux session...\r\n'
    until tmux -S "$SOCKET" has-session -t "$SESSION" 2>/dev/null; do sleep 0.5; done
fi
exec tmux -u -S "$SOCKET" attach-session -t "$SESSION"
