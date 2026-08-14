Reads a repository-relative text file.
If the user provides a file path, use it directly; nonexistent files return an error.

Usage:
- You can optionally specify a line offset and limit. Omit them for small files; bounded results include the exact next offset when another read is needed.
- Lines in the output are numbered starting at 1, using following format: LINE_NUMBER|LINE_CONTENT
- You have the capability to call multiple tools in a single response. It is always better to speculatively read multiple files as a batch that are potentially useful.
- If you read a file that exists but has empty contents you will receive 'File is empty.'
- Any line longer than 2000 bytes is truncated with '...' appended.
- Output is capped at 2000 source lines and 32 KiB. Truncated results explicitly report the next offset to read.