# Real `npx repotracer setup` from the public npm registry, in a throwaway HOME.
# Nothing is mocked; only the idle wait is compressed afterwards.
Output assets/demo/setup.gif

Set Theme { "name": "RepoTracer", "background": "#0a2e33", "foreground": "#cfe0e2", "cursor": "#e4ee4f", "black": "#04252b", "red": "#e8b04b", "green": "#e4ee4f", "yellow": "#e4ee4f", "blue": "#7f9ea2", "magenta": "#cfe0e2", "cyan": "#e4ee4f", "white": "#f5f3eb", "brightBlack": "#5b7276", "brightGreen": "#e4ee4f", "brightWhite": "#f5f3eb" }
Set Shell "bash"
Set FontFamily "CommitMono"
Set FontSize 19
Set Width 1180
Set Height 720
Set Padding 44
Set BorderRadius 8
Set TypingSpeed 55ms
Set PlaybackSpeed 1.6

Hide
Type "export HOME=/private/tmp/claude-502/-Users-mac-Documents-projects-tools-tool/6fd082db-3bf1-4f49-b78b-f935438e0243/scratchpad/demohome" Enter
Type "export PATH=/Users/mac/Library/pnpm:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin" Enter
Type "export PS1='$ '" Enter
Type "cd $HOME/repo" Enter
Type "clear" Enter
Show

Sleep 600ms
Type "npx repotracer setup"
Sleep 500ms
Enter
Sleep 3s
Enter
Sleep 45s
Sleep 3s
