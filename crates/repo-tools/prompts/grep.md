A powerful search tool built on ripgrep
Usage:
- Prefer using Grep for search tasks when you know the exact symbols or strings to search for. Whenever possible, use this tool instead of invoking grep or rg as a terminal command.
- Supports full regex syntax (e.g., "log.*Error", "function\s+\w+")
- Filter files with glob parameter (e.g., ".js", "**/.tsx") or type parameter (e.g., "js", "py", "rust")
- Output modes: "content" shows matching lines, "files_with_matches" shows only file paths (default), "count" shows match counts
- Pattern syntax: Uses ripgrep (not grep) - literal braces need escaping (use interface\{\} to find interface{} in Go code)
- Multiline matching: By default patterns match within single lines only. For cross-line patterns like struct \{[\s\S]*?field, use multiline: true
- Results are capped at 100 output lines and 32 KiB for responsiveness. Truncated results always say to narrow the pattern or path to continue; shown matches remain exact.
- Content output formatting closely follows ripgrep output format: '-' for context lines, ':' for match lines, and all context/match lines below each file group.