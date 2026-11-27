def _penpen_nushell [] {
    let _prev = (commandline)
    if ($_prev != "") {
        print '⌛'
        commandline edit -r (penpen -e $_prev)
    }
}

$env.config.keybindings = ($env.config.keybindings | append {
        name: penpen_integration
        modifier: alt
        keycode: char_e
        mode: [emacs, vi_insert]
        event:[
            {
                send: executehostcommand,
                cmd: "_penpen_nushell"
            }
        ]
    }
)