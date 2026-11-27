function _penpen_fish
    set -l _old (commandline)
    if test -n $_old
        echo -n "⌛"
        commandline -f repaint
        commandline (penpen -e $_old)
    end
end
bind \ee _penpen_fish