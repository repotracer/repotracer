# Real `repotracer doctor` — read-only, no config is modified by this recording.
Output assets/demo/doctor.gif

Set Theme { "name": "RepoTracer", "background": "#0a2e33", "foreground": "#cfe0e2", "cursor": "#e4ee4f", "black": "#04252b", "red": "#e8b04b", "green": "#e4ee4f", "yellow": "#e4ee4f", "blue": "#7f9ea2", "magenta": "#cfe0e2", "cyan": "#e4ee4f", "white": "#f5f3eb", "brightBlack": "#5b7276", "brightGreen": "#e4ee4f", "brightWhite": "#f5f3eb" }
Set Shell "bash"
Set FontFamily "CommitMono"
Set FontSize 19
Set Width 1180
Set Height 760
Set Padding 44
Set BorderRadius 8
Set TypingSpeed 60ms

Hide
Type "export PATH=$PWD/target/debug:$PATH" Enter
Type "export PS1='$ '" Enter
Type "clear" Enter
Show

Sleep 600ms
Type "repotracer doctor"
Sleep 500ms
Enter
Sleep 6s
Sleep 2500ms
