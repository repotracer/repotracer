## RepoTracer

Use `repo_scout` when locating or understanding code requires exploration.
Handle a known small lookup directly. Send the objective, constraints,
and context already available; let the scout perform the exploratory work.

Set `repository` to the current target. Reuse `conversation.id` as
`investigation.conversation_id` for related follow-ups. Leave reasoning
effort automatic unless the task warrants an override.

Continue from the returned evidence, checking specific gaps or
contradictions as needed. Use `structuredContent` as the answer;
`content[].text` is the fallback.
