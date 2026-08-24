# Temporary-script scout experiment

Decision: **reject for production**. Keep the production `read-only` default.

The current Codex `read-only` sandbox permits command execution but rejects an attempted `/tmp` script write with `EPERM`. The experiment-only `workspace-write` arm can create, execute, and delete the same script without changing the repository, and the live regression check passes. This isolates the initial limitation to sandbox policy, not prompt policy.

The natural benchmark did not use that capability. Luna used command execution in all six scout-only trials, but `temporary_script_used` was false in every variant trial. The variant cost 4.90% more, took 13.64% longer, and scored 7.63 versus 7.87 blind. Because the capability was unused, these deltas do not establish a benefit from temporary scripts.

The end-to-end Sol pair was retained because the variant instructions materially changed the scout handoff. Again, neither scout used a temporary script. The variant was cheaper and slightly faster, but both patches failed blind quality: they changed per-tool timeout semantics and never propagated the actual scout deadline. No win is inferred.

Production behavior remains unchanged unless `REPOTRACER_EXPERIMENTAL_TEMP_SCRIPTS=1` is explicitly set. The opt-in variant uses Codex's existing `workspace-write` sandbox and assigns a private OS-temp subdirectory; it adds no dependency or custom sandbox.

Official background: [OpenAI's CI/CD scan guidance](https://learn.chatgpt.com/docs/security/plugin/fix-findings#scan-and-fix-findings-in-cicd) likewise uses `workspace-write` for temporary artifacts while requiring the checkout to remain unchanged. The local probes, not the documentation, are the deciding evidence here.

Raw trial results, provider request rows, randomized mappings, blind grades, Sol trajectories, patches, focused checks, the excluded invalid Sol attempt, and capability logs are retained below this directory. `manifest.json` records the protocol; `summary.json` contains the aggregate numbers.
