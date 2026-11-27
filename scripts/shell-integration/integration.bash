_penpen_bash() {
    if [[ -n "$READLINE_LINE" ]]; then
        READLINE_LINE=$(penpen -e "$READLINE_LINE")
        READLINE_POINT=${#READLINE_LINE}
    fi
}
bind -x '"\ee": _penpen_bash'