#!/usr/bin/env python3
"""Opt-in real native-CLI checks, not a prompt-quality benchmark.

Uses the selected native CLI's existing login by default. An explicit native
wrapper may select API-only Claude settings. This runner never reads provider
credentials or calls a provider API. Results contain fixture evidence only.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import queue
import shutil
import subprocess
import sys
import tempfile
import threading
import time


def referenced_text(result, reference):
    """Version-2 references address decoded text in UTF-8 bytes, end-exclusive."""
    content = result["content"][reference["content_index"]]["text"].encode("utf-8")
    start, end = reference["start_byte"], reference["end_byte"]
    assert 0 <= start <= end <= len(content), "Invalid handoff reference"
    return content[start:end].decode("utf-8")


def handoff_report(result):
    """Read the supported report rendering without requiring legacy metadata."""
    structured = result["structuredContent"]
    version = structured.get("handoff_version", 1)
    if version == 3:
        report = structured["report"]
    elif version == 2:
        report = referenced_text(result, structured["report_ref"])
    else:
        report = " ".join(
            finding["answer"]
            for finding in structured.get("investigation", {}).get("findings", [])
        )
    assert isinstance(report, str) and report.strip(), "No investigation report returned"
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--backend", choices=["codex-cli", "claude-cli"], default="codex-cli")
    parser.add_argument("--model", help="Native provider model; defaults to Luna or Sonnet")
    parser.add_argument("--native-executable", type=Path,
                        help="Explicit native CLI or wrapper, for example Claude Code with API-only settings")
    parser.add_argument("--impact-effort", choices=["low", "medium", "high", "xhigh", "max"], default="high",
                        help="Exercise per-request reasoning on the independent change-impact investigation")
    args = parser.parse_args()
    project = Path(__file__).resolve().parents[1]
    spec = importlib.util.spec_from_file_location("protocol", project / "scripts/codex-app-server-smoke.py")
    sys.dont_write_bytecode = True
    protocol = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(protocol)
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    # Never overwrite earlier runs.
    run_output = Path(tempfile.mkdtemp(prefix="subscription-", dir=output))
    with tempfile.TemporaryDirectory(prefix="repotracer-subscription-fixture-") as temporary:
        temporary = Path(temporary)
        root = temporary / "repo"
        shutil.copytree(project / "fixtures/investigation", root)
        config = temporary / "repotracer.toml"
        model = args.model or ("sonnet" if args.backend == "claude-cli" else "gpt-5.6-luna")
        executable = (f'executable = {json.dumps(str(args.native_executable.resolve()))}\n'
                      if args.native_executable else '')
        config.write_text(f'[model]\nbackend = {json.dumps(args.backend)}\n{executable}model = {json.dumps(model)}\nreasoning_effort = "medium"\nadaptive_reasoning = false\ntimeout_ms = 180000\n[updates]\nautomatic = false\n')
        process = subprocess.Popen(
            [str(Path(args.binary).resolve()), "--root", str(root), "--config", str(config), "serve"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            text=True, bufsize=1, env=os.environ.copy(),
        )
        lines = queue.Queue()
        threading.Thread(target=lambda: [lines.put(line) for line in process.stdout], daemon=True).start()
        # Drain diagnostics without exporting possible account/provider metadata.
        threading.Thread(target=lambda: [None for _ in process.stderr], daemon=True).start()
        results = []
        try:
            protocol.send(process, {"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{}})
            initialized = protocol.read_response(lines, 1)
            assert "error" not in initialized, initialized
            protocol.send(process, {"jsonrpc":"2.0", "method":"notifications/initialized"})
            cases = [
                ("locate", "Where is the request timeout default defined, and what is its value?", "timeout-followup"),
                ("explain", "Re-read current config.py. What is the request timeout default now, including the environment fallback?", "timeout-followup"),
                ("change_impact", "Which implementation files and existing tests should be reviewed if request_timeout is renamed? Trace its consumers.", None),
            ]
            for number, (intent, query, conversation) in enumerate(cases, 2):
                if number == 3:
                    path = root / "config.py"
                    path.write_text(path.read_text().replace("5.0", "9.0").replace('"5"', '"9"'))
                investigation = {"intent":intent}
                if number == 4:
                    investigation["reasoning_effort"] = args.impact_effort
                if conversation:
                    investigation["conversation_id"] = conversation
                started = time.monotonic()
                protocol.send(process, {"jsonrpc":"2.0", "id":number, "method":"tools/call", "params":{
                    "name":"repo_scout", "arguments":{"query":query, "investigation":investigation}}})
                response = protocol.read_response(lines, number, timeout=240)
                (run_output / f"{number}-{intent}.json").write_text(json.dumps(response, indent=2))
                assert "error" not in response, "Subscription request failed; inspect the saved response."
                assert not response["result"].get("isError"), \
                    "Native scout reported a failure; inspect the saved response and usage."
                result = response["result"]["structuredContent"]
                report = handoff_report(response["result"])
                # These fixture questions ask where source behavior lives. A
                # different investigation may validly return experiment evidence
                # in the report with no source citations.
                assert result["citations"], "No source evidence returned"
                source_spans = result.get("evidence", [])
                source_texts = [span.get("text") if "text" in span else referenced_text(response["result"], span["text_ref"])
                                for span in source_spans]
                assert source_texts and all(source_texts), "No source text in handoff"
                text_response = "\n".join(block.get("text", "") for block in response["result"]["content"])
                assert all(source in text_response for source in source_texts), \
                    "Text and structured responses disagree on source context"
                stats = result["stats"]
                expected_turn = 2 if number == 3 else 1
                assert stats["thread_turn"] == expected_turn, stats
                # Independent MCP calls now receive separate conversation slots.
                expected_warm = number == 3
                assert stats["warm_process"] == expected_warm, stats
                assert result["conversation"]["status"] == ("resumed" if number == 3 else "fresh")
                assert result["repository"] == str(root.resolve())
                if number == 3:
                    assert "9" in report, "Continuation did not report the changed default"
                    assert any("request_timeout: float = 9.0" in text for text in source_texts), \
                        "Continuation did not supply the current dataclass default"
                    assert any('env.get("REQUEST_TIMEOUT", "9")' in text for text in source_texts), \
                        "Continuation did not supply the current environment fallback"
                if number == 4:
                    cited = {c["path"] for c in result["citations"]}
                    assert {"config.py", "client.py", "test_client.py"} <= cited, cited
                results.append({"intent":intent, "requested_effort":investigation.get("reasoning_effort"),
                                "seconds":round(time.monotonic()-started, 2), "stats":stats})
                print(f"PASS {intent}: warm={stats['warm_process']}, thread_turn={stats['thread_turn']}", flush=True)
            (run_output / "summary.json").write_text(json.dumps({"type":"functional smoke, not comparative evidence", "results":results}, indent=2))
            print(f"Artifacts: {run_output}", flush=True)
        finally:
            if process.poll() is None:
                process.stdin.close()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


if __name__ == "__main__":
    main()
