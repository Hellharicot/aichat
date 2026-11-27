_penpen_zsh() {
    if [[ -n "$BUFFER" ]]; then
        local _old=$BUFFER
        BUFFER+="⌛"
        zle -I && zle redisplay
        BUFFER=$(penpen -e "$_old")
        zle end-of-line
    fi
}
zle -N _penpen_zsh
bindkey '\ee' _penpen_zsh