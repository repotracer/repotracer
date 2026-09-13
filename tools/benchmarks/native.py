#!/usr/bin/env python3
"""Native Codex and Claude Code adapters for local benchmark workers.

The adapters keep the user's configured provider and authentication. They only
add per-run model, effort, RepoTracer, and output settings.
"""
from __future__ import annotations

import hashlib
import contextlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
from typing import Callable, Iterable

TOKEN_FIELDS = ("uncached_input", "cache_read", "cache_write", "output")


def _json_lines(path: Path) -> Iterable[dict]:
    if not path.exists():
        return
    with path.open(encoding="utf-8", errors="replace") as stream:
        for line in stream:
            try:
                value = json.loads(line)
                if isinstance(value, dict):
                    yield value
            except ValueError:
                continue


def _integer(value) -> int | None:
    return value if type(value) is int and value >= 0 else None


def normalize_tokens(usage: dict, provider: str) -> dict | None:
    """Return the four disjoint bench.py buckets, or None for bad usage."""
    if not isinstance(usage, dict):
        return None
    if provider == "claude":
        ordinary = _integer(usage.get("input_tokens"))
        read = _integer(usage.get("cache_read_input_tokens", 0))
        write = _integer(usage.get("cache_creation_input_tokens", 0))
    else:
        total = _integer(usage.get("input_tokens"))
        read = _integer(usage.get("cached_input_tokens", usage.get("cache_read_input_tokens", 0)))
        write = _integer(usage.get("cache_write_input_tokens", usage.get("cache_creation_input_tokens", 0)))
        if total is None or read is None or write is None or total < read + write:
            return None
        ordinary = total - read - write
    output = _integer(usage.get("output_tokens"))
    if None in (ordinary, read, write, output):
        return None
    return {"uncached_input": ordinary, "cache_read": read,
            "cache_write": write, "output": output}


def _request(identity: str, role: str, model, effort, tokens, rate_cards: dict) -> dict:
    matching = [key for key, card in rate_cards.items()
                if isinstance(card, dict) and card.get("model") == model]
    return {
        "id": identity,
        "role": role,
        "model": model or "unknown",
        "effort": effort or "unknown",
        "rate_card": matching[0] if len(matching) == 1 else None,
        "tokens": tokens,
    }


def parse_codex_rollouts(paths: Iterable[Path], worktree: Path, rate_cards: dict,
                          role: str = "parent") -> tuple[list[dict], list[str]]:
    """Collect task-local request usage, deduplicating replayed response ids."""
    requests: dict[str, dict] = {}
    problems: list[str] = []
    wanted = str(worktree.resolve())
    for path in paths:
        events = list(_json_lines(path))
        contexts = [event.get("payload") or {} for event in events
                    if event.get("type") == "turn_context"]
        if contexts and not any(str(row.get("cwd", "")) == wanted for row in contexts):
            continue
        if not contexts and wanted not in path.read_text(encoding="utf-8", errors="ignore"):
            continue
        model = effort = session = None
        for position, event in enumerate(events):
            payload = event.get("payload") or {}
            if event.get("type") == "turn_context":
                model = payload.get("model") or model
                effort = (payload.get("effort") or payload.get("reasoning_effort")
                          or payload.get("model_reasoning_effort") or effort)
                session = payload.get("session_id") or session
            if event.get("type") != "token_usage_record":
                continue
            tokens = normalize_tokens(payload.get("usage") or {}, "codex")
            if tokens is None:
                problems.append(f"invalid Codex usage in {path.name}")
                continue
            raw_id = payload.get("response_id")
            if raw_id:
                identity = f"codex:{raw_id}"
            else:
                stable = json.dumps({"session": session, "position": position,
                                     "payload": payload}, sort_keys=True).encode()
                identity = "codex-missing-id:" + hashlib.sha256(stable).hexdigest()
                problems.append(f"Codex request in {path.name} had no response id")
            row = _request(identity, role, payload.get("model") or model,
                           payload.get("effort") or effort, tokens, rate_cards)
            previous = requests.get(identity)
            if previous is not None and previous != row:
                problems.append(f"conflicting replay for {identity}")
            else:
                requests[identity] = row
    return list(requests.values()), problems


def parse_claude_events(path: Path, rate_cards: dict, configured_effort: str | None,
                        role: str = "parent") -> tuple[list[dict], list[str], float | None]:
    """Normalize Claude request events without double-counting streamed chunks."""
    requests: dict[str, dict] = {}
    problems: list[str] = []
    reported_cost = 0.0
    reported_cost_complete = True
    saw_result = False
    segment: set[str] = set()
    for event in _json_lines(path):
        if event.get("type") == "result":
            saw_result = True
            value = event.get("total_cost_usd")
            if isinstance(value, (int, float)) and not isinstance(value, bool):
                reported_cost += float(value)
            else:
                reported_cost_complete = False
            terminal_tokens = normalize_tokens(event.get("usage") or {}, "claude")
            summed = {key: sum(requests[identity]["tokens"][key]
                               for identity in segment)
                      for key in TOKEN_FIELDS}
            if terminal_tokens is None:
                problems.append("Claude terminal usage was unavailable")
            elif summed != terminal_tokens:
                problems.append(
                    "Claude request ledger does not reconcile with terminal usage")
            segment.clear()
            continue
        message = event.get("message") or {}
        if event.get("type") != "assistant" or not isinstance(message, dict):
            continue
        usage = message.get("usage")
        tokens = normalize_tokens(usage, "claude")
        if tokens is None:
            continue
        raw_id = event.get("request_id") or message.get("id")
        if not raw_id:
            problems.append("Claude assistant usage had no request id")
            continue
        identity = f"claude:{raw_id}"
        effort = (event.get("effort") or message.get("effort")
                  or configured_effort or "native-default")
        row = _request(identity, role, message.get("model"), effort, tokens, rate_cards)
        segment.add(identity)
        previous = requests.get(identity)
        if previous is None:
            requests[identity] = row
        else:
            # Claude repeats one request while streaming content. Keep the
            # largest observed count in each bucket, never sum the repeats.
            if previous["model"] != row["model"] or previous["effort"] != row["effort"]:
                problems.append(f"conflicting replay for {identity}")
            previous["tokens"] = {key: max(previous["tokens"][key], row["tokens"][key])
                                  for key in TOKEN_FIELDS}
    if segment:
        problems.append("Claude terminal usage was unavailable")
        reported_cost_complete = False
    native_cost = reported_cost if saw_result and reported_cost_complete else None
    return list(requests.values()), problems, native_cost


def parse_scout_log(path: Path, rate_cards: dict) -> tuple[list[dict], list[str], float | None]:
    requests: dict[str, dict] = {}
    pending: set[str] = set()
    problems: list[str] = []
    operation_costs: dict[str, float | None] = {}
    saw_request = False
    all_usage_complete = True
    for row in _json_lines(path):
        identity = str(row.get("id"))
        if row.get("event") == "request":
            saw_request = True
            pending.add(identity)
            continue
        if row.get("event") == "missing_response":
            pending.add(identity)
            continue
        if row.get("event") != "response":
            continue
        pending.discard(identity)
        if row.get("failed"):
            problems.append(f"RepoTracer request {identity} failed")
        stats = row.get("stats") or {}
        usage_status = str(stats.get("usage_status") or "unknown").lower()
        if usage_status != "complete":
            all_usage_complete = False
            problems.append(
                f"RepoTracer request {identity} reported {usage_status} usage")
        arguments = (row.get("request") or {}).get("arguments") or {}
        investigation = arguments.get("investigation") or {}
        model = stats.get("model") or "unknown"
        rid = stats.get("request_id") or stats.get("response_id") or identity
        attempts = stats.get("attempts")
        attempts = attempts if isinstance(attempts, list) and attempts else [stats]
        attempt_costs = []
        for index, attempt in enumerate(attempts, 1):
            attempt_status = str(attempt.get("usage_status") or usage_status).lower()
            if attempt_status != "complete":
                all_usage_complete = False
                problems.append(
                    f"RepoTracer request {identity} attempt {index} reported "
                    f"{attempt_status} usage")
            tokens = normalize_tokens(attempt.get("usage") or {}, "codex")
            if tokens is None:
                problems.append(
                    f"RepoTracer request {identity} attempt {index} has missing or invalid usage")
                continue
            actual_effort = (attempt.get("reasoning_effort")
                             or stats.get("reasoning_effort") or stats.get("effort")
                             or investigation.get("reasoning_effort") or "unknown")
            request_id = (f"scout:{rid}:attempt:{index}"
                          if attempts != [stats] else f"scout:{rid}")
            normalized = _request(
                request_id, "scout", attempt.get("model") or model,
                actual_effort, tokens, rate_cards)
            old = requests.get(normalized["id"])
            if old is not None and old != normalized:
                problems.append(f"conflicting replay for {normalized['id']}")
            else:
                requests[normalized["id"]] = normalized
            attempt_cost = attempt.get("reported_cost_usd")
            attempt_costs.append(
                float(attempt_cost) if isinstance(attempt_cost, (int, float))
                and not isinstance(attempt_cost, bool) else None)
        cost = stats.get("reported_cost_usd")
        native_cost = (float(cost) if isinstance(cost, (int, float))
                       and not isinstance(cost, bool) else
                       sum(attempt_costs) if attempt_costs
                       and all(value is not None for value in attempt_costs) else None)
        previous_cost = operation_costs.get(identity)
        if identity in operation_costs and previous_cost != native_cost:
            problems.append(f"conflicting reported cost for RepoTracer request {identity}")
        operation_costs[identity] = native_cost
    if pending:
        problems.append(f"{len(pending)} RepoTracer request(s) have no response")
    cost = (0.0 if not saw_request else
            sum(operation_costs.values()) if operation_costs and not pending
            and all_usage_complete
            and all(value is not None for value in operation_costs.values()) else None)
    return list(requests.values()), problems, cost


def toml_inline(values: dict[str, str]) -> str:
    return "{" + ",".join(f"{key}={json.dumps(value)}" for key, value in values.items()) + "}"


def scout_config(provider: str, executable: str, model: str,
                 requested_effort: str, path: Path) -> None:
    automatic = requested_effort in {"auto", "auto-low-medium", "natural"}
    effort = ("medium" if provider == "codex" else "low") if automatic else requested_effort
    path.write_text(
        "[model]\n"
        f"backend = {json.dumps(provider + '-cli')}\n"
        f"executable = {json.dumps(executable)}\n"
        f"model = {json.dumps(model)}\n"
        f"reasoning_effort = {json.dumps(effort)}\n"
        f"adaptive_reasoning = {'true' if automatic else 'false'}\n"
        "service_tier = \"default\"\ntimeout_ms = 0\n\n"
        "[explorer]\nmax_turns = 0\ntimeout_seconds = 0\nmax_tool_calls = 40\n"
        "tool_timeout_seconds = 0\nconcurrency = 1\n\n"
        "[session]\nwarm = true\nidle_secs = 300\nmax_warm = 1\nmax_process_threads = 8\n"
        "max_thread_turns = 4\nmax_thread_input_tokens = 0\n\n"
        "[updates]\nautomatic = false\n",
        encoding="utf-8",
    )


def _mcp(repotracer: Path, repo: Path, scout_path: Path, usage_path: Path) -> dict:
    tap = Path(__file__).with_name("usage_tap.py").resolve()
    return {
        "command": sys.executable,
        "args": [str(tap), str(repotracer), "--root", str(repo), "--config", str(scout_path), "serve"],
        "env": {"REPOTRACER_USAGE_LOG": str(usage_path), "REPOTRACER_NO_UPDATE": "1"},
    }


def codex_command(executable: str, repo: Path, prompt: str, model: str, effort: str,
                  final_path: Path, assisted: bool, repotracer: Path | None = None,
                  scout_path: Path | None = None, usage_path: Path | None = None,
                  routing: str | None = None, session_id: str | None = None) -> list[str]:
    if session_id:
        command = [executable, "exec", "resume", "--all", "--json",
                   "--skip-git-repo-check", "--dangerously-bypass-approvals-and-sandbox",
                   "--model", model, "--output-last-message", str(final_path),
                   "--config", f"model_reasoning_effort={json.dumps(effort)}"]
    else:
        command = [executable, "exec", "--json", "--skip-git-repo-check",
                   "--dangerously-bypass-approvals-and-sandbox", "--model", model,
                   "--cd", str(repo), "--output-last-message", str(final_path),
                   "--config", f"model_reasoning_effort={json.dumps(effort)}"]
    if assisted:
        if None in (repotracer, scout_path, usage_path, routing):
            raise ValueError("assisted Codex run is missing RepoTracer settings")
        server = _mcp(repotracer, repo, scout_path, usage_path)
        command += [
            "--config", f"mcp_servers.repotracer.command={json.dumps(server['command'])}",
            "--config", f"mcp_servers.repotracer.args={json.dumps(server['args'])}",
            "--config", f"mcp_servers.repotracer.env={toml_inline(server['env'])}",
            "--config", "mcp_servers.repotracer.enabled=true",
            "--config", "mcp_servers.repotracer.required=true",
            "--config", "mcp_servers.repotracer.tool_timeout_sec=2147483647",
            "--config", "features.code_mode.direct_only_tool_namespaces=[\"mcp__repotracer\"]",
            "--config", f"developer_instructions={json.dumps(routing)}",
        ]
    else:
        command += ["--config", "mcp_servers.repotracer.enabled=false"]
    command += [session_id, "Continue the original task."] if session_id else [prompt]
    return command


def claude_command(executable: str, repo: Path, prompt: str, model: str,
                   effort: str | None, final_path: Path, assisted: bool,
                   repotracer: Path | None = None, scout_path: Path | None = None,
                   usage_path: Path | None = None, routing: str | None = None,
                   session_id: str | None = None) -> list[str]:
    server = _mcp(repotracer, repo, scout_path, usage_path) if assisted else None
    mcp = {"mcpServers": {"repotracer": server} if server else {}}
    command = [executable, "--print", "--verbose", "--output-format", "stream-json",
               "--include-partial-messages", "--forward-subagent-text",
               "--strict-mcp-config", "--mcp-config", json.dumps(mcp),
               "--model", model, "--dangerously-skip-permissions", "--disable-slash-commands",
               "--no-chrome"]
    if effort and effort not in {"natural", "auto", "native-default"}:
        command += ["--effort", effort]
    if assisted:
        if routing is None:
            raise ValueError("assisted Claude run is missing routing instructions")
        command += ["--append-system-prompt", routing]
    command += ["--resume", session_id, "-p", "Continue the original task."] if session_id else ["-p", prompt]
    return command


class NativeAdapter:
    """Process adapter kept injectable so unit tests never call a model."""

    def __init__(self, popen: Callable = subprocess.Popen):
        self.popen = popen

    def run(self, command: list[str], cwd: Path, trace: Path, stderr: Path,
            on_event: Callable[[dict], None] | None = None,
            on_start: Callable[[int], None] | None = None,
            stdin_path: Path | None = None) -> int:
        trace.parent.mkdir(parents=True, exist_ok=True)
        with trace.open("a", encoding="utf-8", buffering=1) as output, \
                stderr.open("a", encoding="utf-8", buffering=1) as errors, \
                (stdin_path.open("r", encoding="utf-8") if stdin_path else
                 contextlib.nullcontext(subprocess.DEVNULL)) as input_stream:
            position = output.tell()
            process = self.popen(command, cwd=cwd, stdout=output,
                                 stderr=errors, stdin=input_stream, text=True, bufsize=1,
                                 start_new_session=True)
            if on_start:
                on_start(process.pid)
            with trace.open(encoding="utf-8", errors="replace") as observer:
                observer.seek(position)
                while process.poll() is None:
                    _observe_available(observer, on_event)
                    time.sleep(0.1)
                _observe_available(observer, on_event)
            return process.wait()


def _observe_available(stream, callback: Callable[[dict], None] | None) -> None:
    if callback is None:
        return
    while True:
        line = stream.readline()
        if not line:
            return
        try:
            event = json.loads(line)
            if isinstance(event, dict):
                callback(event)
        except ValueError:
            pass


def _pid_alive(pid) -> bool:
    if type(pid) is not int or pid <= 0:
        return False
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def _atomic_json(path: Path, value: dict) -> None:
    temporary = path.with_suffix(".tmp")
    with temporary.open("w", encoding="utf-8") as stream:
        stream.write(json.dumps(value, indent=2) + "\n")
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)


def _wait_for_detached(pid: int, trace: Path,
                       on_event: Callable[[dict], None] | None) -> None:
    position = trace.stat().st_size if trace.exists() else 0
    with trace.open(encoding="utf-8", errors="replace") as observer:
        observer.seek(position)
        while _pid_alive(pid):
            _observe_available(observer, on_event)
            time.sleep(0.2)
        _observe_available(observer, on_event)


def session_from_trace(path: Path, provider: str) -> str | None:
    for event in _json_lines(path):
        if provider == "codex" and event.get("type") == "thread.started":
            return event.get("thread_id") or (event.get("thread") or {}).get("id")
        if provider == "claude" and event.get("session_id"):
            return event["session_id"]
    return None


def terminal_success(path: Path, provider: str, exit_code: int) -> bool:
    events = list(_json_lines(path))
    if provider == "codex":
        return exit_code == 0 and any(event.get("type") == "turn.completed" for event in events)
    terminal = next((event for event in reversed(events) if event.get("type") == "result"), {})
    answer = str(terminal.get("result") or "")
    return (exit_code == 0 and terminal.get("subtype") == "success"
            and not terminal.get("is_error", False)
            and not answer.lstrip().startswith("API Error:"))


def terminal_completion(path: Path, provider: str) -> bool:
    """Return whether a native turn wrote a terminal success or failure event."""
    events = list(_json_lines(path))
    if provider == "codex":
        return any(event.get("type") in {"turn.completed", "turn.failed"}
                   for event in events)
    return any(event.get("type") == "result" for event in events)


def find_codex_rollouts(started_unix: float, worktree: Path) -> list[Path]:
    root = Path(os.environ.get("CODEX_HOME", str(Path.home() / ".codex")))
    found = []
    for path in root.glob("**/rollout-*.jsonl"):
        try:
            if path.stat().st_mtime >= started_unix - 60:
                found.append(path)
        except OSError:
            pass
    return found


def run_arm(provider: str, repo: Path, prompt: str, settings: dict,
            trial: Path, assisted: bool, repotracer: Path | None,
            routing: str | None, rate_cards: dict,
            adapter: NativeAdapter | None = None, resume: bool = False,
            status_callback: Callable[[dict], None] | None = None) -> dict:
    """Run or resume one native arm and return normalized evidence."""
    adapter = adapter or NativeAdapter()
    trial.mkdir(parents=True, exist_ok=True)
    saved_result = trial / "native-result.json"
    if saved_result.exists():
        return json.loads(saved_result.read_text(encoding="utf-8"))
    process_path = trial / "native-process.json"
    trace, errors = trial / "trajectory.jsonl", trial / "stderr.log"
    final, usage_log = trial / "final.md", trial / "scout-usage.jsonl"
    previous_process = (json.loads(process_path.read_text(encoding="utf-8"))
                        if process_path.exists() else None)
    was_resumed = bool(previous_process)
    started_unix = (previous_process or {}).get("started_unix", time.time())
    started = time.monotonic()

    def observed(event: dict) -> None:
        session = (event.get("thread_id") if event.get("type") == "thread.started"
                   else event.get("session_id"))
        if status_callback and session:
            status_callback({"session_id": session})

    if previous_process and _pid_alive(previous_process.get("pid")):
        _wait_for_detached(previous_process["pid"], trace, observed)
        exit_code = 0 if terminal_success(trace, provider, 0) else 1
    elif previous_process and terminal_completion(trace, provider):
        # The native process finished after its controller disappeared. Its
        # terminal event is the commit marker, including terminal failures.
        exit_code = 0 if terminal_success(trace, provider, 0) else 1
    else:
        existing_session = session_from_trace(trace, provider) if resume else None
        executable = shutil.which(provider)
        if not executable:
            raise ValueError(f"native executable not found: {provider}")
        scout_path = trial / "scout.toml"
        if assisted:
            scout_config(provider, executable, settings["scout_model"],
                         settings["scout_effort"], scout_path)
        if provider == "codex":
            command = codex_command(executable, repo, prompt, settings["parent_model"],
                                    settings["parent_effort"], final, assisted,
                                    repotracer, scout_path, usage_log, routing, existing_session)
        elif provider == "claude":
            command = claude_command(executable, repo, prompt, settings["parent_model"],
                                     settings.get("parent_effort"), final, assisted,
                                     repotracer, scout_path, usage_log, routing, existing_session)
        else:
            raise ValueError(f"unsupported native provider: {provider}")
        stdin_path = None
        if not existing_session:
            # Packets and task prompts can exceed the operating system's argv
            # limit. Both native print modes accept the prompt through stdin.
            stdin_path = trial / "input.txt"
            stdin_path.write_text(prompt, encoding="utf-8")
            if provider == "codex":
                command[-1] = "-"
            else:
                command.pop()
        (trial / "command.json").write_text(
            json.dumps(command, indent=2) + "\n", encoding="utf-8")
        with (trial / "commands.jsonl").open("a", encoding="utf-8") as stream:
            stream.write(json.dumps({"recorded_unix": time.time(),
                                     "command": command}) + "\n")

        def started_process(pid: int) -> None:
            _atomic_json(process_path, {"pid": pid, "started_unix": started_unix,
                                        "provider": provider})
            if status_callback:
                status_callback({"native_pid": pid})

        exit_code = adapter.run(command, repo, trace, errors, observed, started_process,
                                stdin_path=stdin_path)
    seconds = round(time.monotonic() - started, 3)
    session = session_from_trace(trace, provider)
    if provider == "codex":
        requests, problems = parse_codex_rollouts(
            find_codex_rollouts(started_unix, repo), repo, rate_cards)
        reported = None
    else:
        configured = settings.get("parent_effort")
        configured = None if configured in {"natural", "auto"} else configured
        requests, problems, reported = parse_claude_events(trace, rate_cards, configured)
    parent_usage_incomplete = bool(problems)
    scout_requests, scout_problems, scout_cost = parse_scout_log(usage_log, rate_cards)
    if not assisted:
        scout_requests, scout_problems, scout_cost = [], [], 0.0
    requests.extend(scout_requests)
    problems.extend(scout_problems)
    complete = terminal_success(trace, provider, exit_code)
    if not complete:
        problems.append("Native run ended without a successful terminal event; usage may be partial")
    if not requests:
        problems.append("native request usage was not found")
    def priced(rows: list[dict], *, allow_empty: bool = False) -> float | None:
        if not rows:
            return 0.0 if allow_empty else None
        total = 0.0
        for row in rows:
            card = rate_cards.get(row.get("rate_card"))
            if not isinstance(card, dict):
                return None
            for field in TOKEN_FIELDS:
                rate = card.get(field)
                if not isinstance(rate, (int, float)) or isinstance(rate, bool):
                    return None
                total += row["tokens"][field] * rate / 1_000_000
        return total

    parent_rows = [row for row in requests if row["role"] == "parent"]
    scout_rows = [row for row in requests if row["role"] == "scout"]
    parent_cost = (reported if reported is not None else
                   None if parent_usage_incomplete else priced(parent_rows))
    effective_scout_cost = (None if scout_problems else
                            scout_cost if scout_cost is not None
                            else priced(scout_rows, allow_empty=True))
    reported_total = (parent_cost + effective_scout_cost
                      if parent_cost is not None and effective_scout_cost is not None else None)
    reported_by_role = {}
    reported_sources = {}
    if reported is not None:
        reported_by_role["parent"] = reported
        reported_sources["parent"] = "Claude Code total_cost_usd"
    if assisted and scout_cost is not None:
        reported_by_role["scout"] = scout_cost
        reported_sources["scout"] = "RepoTracer reported_cost_usd"
    result = {
        "exit_code": exit_code, "terminal_success": complete,
        "session_id": session, "seconds": seconds, "requests": requests,
        "usage_complete": complete and not problems,
        "usage_missing_reason": "; ".join(dict.fromkeys(problems)) or None,
        "reported_cost_usd": reported_total,
        "reported_cost_by_role_usd": reported_by_role,
        "reported_cost_sources": reported_sources,
        "cost_missing_reason": (None if reported_total is not None else
                                "Native requests lack a reported cost or matching rate card."),
        "final": str(final), "trace": str(trace), "stderr": str(errors),
        "resumed": was_resumed,
    }
    _atomic_json(saved_result, result)
    process_path.unlink(missing_ok=True)
    return result
