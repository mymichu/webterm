if status is-interactive
    set -g fish_greeting
    alias vi nvim
    alias vim nvim
    starship init fish | source

    # fzf: Ctrl+R searches history, Ctrl+T inserts a file or directory, Alt+C
    # cds into a directory. fd lists files fast and respects .gitignore.
    set -gx FZF_DEFAULT_COMMAND 'fd --type f --hidden --follow --exclude .git'
    set -gx FZF_CTRL_T_COMMAND 'fd --hidden --follow --exclude .git'
    set -gx FZF_ALT_C_COMMAND 'fd --type d --hidden --follow --exclude .git'
    set -gx FZF_CTRL_T_OPTS "--preview 'test -d {} && ls -la {} || head -n 100 {}'"
    fzf --fish | source
end
