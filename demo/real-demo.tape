# Three real coding agents change one real repository at once through Zit.
# vhs renders its own terminal off-screen: nothing else on the machine is recorded.
#   ZIT_SOURCE=... AUTOHAND_BIN=... vhs demo/real-demo.tape
Output demo/out/real-demo.mp4

Set Shell bash
Set FontSize 13
Set Width 1600
Set Height 1000
Set Padding 16
Set Framerate 10
Set TypingSpeed 20ms
Set Theme { "name": "Zit", "background": "#181818", "foreground": "#e6e6e6", "cursor": "#e6e6e6", "selection": "#3a3a3a", "black": "#181818", "red": "#ff7b72", "green": "#7ee787", "yellow": "#e3b341", "blue": "#79c0ff", "magenta": "#d2a8ff", "cyan": "#a5d6ff", "white": "#e6e6e6", "brightBlack": "#8b8b8b", "brightRed": "#ffa198", "brightGreen": "#aff5b4", "brightYellow": "#f8e3a1", "brightBlue": "#a5d6ff", "brightMagenta": "#e2c5ff", "brightCyan": "#b6e3ff", "brightWhite": "#ffffff" }

Hide
Type `unset MAIL MAILCHECK MAILPATH; export PS1='$ '; clear` Enter
Show
Type "demo/real-demo.sh" Enter
Wait+Screen@20m /end of demo/
Sleep 6s
