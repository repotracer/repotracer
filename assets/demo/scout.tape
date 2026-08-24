# Real repo_scout run against this repository. Not mocked, not sped-up output —
# only the playback is compressed. Re-record with: vhs assets/demo/scout.tape
Output assets/demo/scout.gif

Set Theme { "name": "RepoTracer", "background": "#0a2e33", "foreground": "#cfe0e2", "cursor": "#e4ee4f", "black": "#04252b", "red": "#e8b04b", "green": "#e4ee4f", "yellow": "#e4ee4f", "blue": "#7f9ea2", "magenta": "#cfe0e2", "cyan": "#e4ee4f", "white": "#f5f3eb", "brightBlack": "#5b7276", "brightGreen": "#e4ee4f", "brightWhite": "#f5f3eb" }
Set Shell "bash"
Set FontFamily "CommitMono"
Set FontSize 18
Set Width 1240
Set Height 860
Set Padding 42
Set BorderRadius 8
Set TypingSpeed 45ms
Set PlaybackSpeed 2.5

Hide
Type "export PATH=$PWD/target/debug:$PATH" Enter
Type "export PS1='$ '" Enter
Type "clear" Enter
Show

Sleep 500ms
Type `repotracer 'how are citations validated before they reach Codex?'`
Sleep 400ms
Enter
Sleep 50s
Sleep 3s

# NOTE: after recording, the dead wait between the prompt and the result is
# compressed ~10x and the result is held, via scripts/compress-demo.py.
# The scout output itself is never edited.
