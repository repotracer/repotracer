#!/usr/bin/env python3
"""Run the real RepoTracer -> Codex app-server -> sandbox path with a fake model."""

import json
import os
import queue
import shlex
import shutil
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


COMMAND = "type marker.txt" if os.name == "nt" else "cat marker.txt"
SYMBOLS = os.environ.get("REPOTRACER_SMOKE_SYMBOLS") == "1"
TARGET_MARKERS = {"a": "codex-app-server-target-a", "b": "codex-app-server-target-b"}
USAGE = {
    "input_tokens": 0,
    "input_tokens_details": None,
    "output_tokens": 0,
    "output_tokens_details": None,
    "total_tokens": 0,
}


def event_stream(events):
    return "".join(
        f"event: {event['type']}\ndata: {json.dumps(event, separators=(',', ':'))}\n\n"
        for event in events
    ).encode()


def validate_tool_result(request, expected):
    outputs = [
        item.get("output") for item in request.get("input", [])
        if item.get("type") == "function_call_output"
        and item.get("call_id") == "read-workspace"
    ]
    tool_result = json.dumps(outputs, separators=(",", ":"))
    expected_values = ["ScoutEngine", "crates/core/src/engine.rs"] if SYMBOLS else [expected]
    if not all(value in tool_result for value in expected_values):
        raise AssertionError("Codex did not return the requested tool result: " + tool_result)


class FakeResponses(BaseHTTPRequestHandler):
    calls = 0
    failure = None
    expected_markers = {}

    def log_message(self, _format, *_args):
        pass

    def do_POST(self):
        try:
            length = int(self.headers.get("Content-Length", "0"))
            request = json.loads(self.rfile.read(length))
            type(self).calls += 1
            if type(self).calls == 1:
                tool_names = [tool.get("name") for tool in request.get("tools", [])]
                tool_name = "Symbols" if SYMBOLS else "exec_command"
                if tool_name not in tool_names:
                    raise AssertionError(f"Codex did not offer {tool_name}: {tool_names}")
                body = event_stream(
                    [
                        {"type": "response.created", "response": {"id": "resp-1"}},
                        {
                            "type": "response.output_item.done",
                            "item": {
                                "type": "function_call",
                                "call_id": "read-workspace",
                                "name": tool_name,
                                "arguments": json.dumps({"symbol": "ScoutEngine", "path": "crates/core/src"} if SYMBOLS else {"cmd": COMMAND}),
                            },
                        },
                        {
                            "type": "response.completed",
                            "response": {"id": "resp-1", "usage": USAGE},
                        },
                    ]
                )
            elif type(self).calls in (2, 4):
                if SYMBOLS and type(self).calls == 4:
                    raise AssertionError("Symbols smoke made more than two model requests")
                marker = type(self).expected_markers[type(self).calls]
                validate_tool_result(request, marker)
                answer = json.dumps(
                    {
                        "answer": "Found the workspace manifest.",
                        "citations": [
                            {
                                "path": "Cargo.toml",
                                "start_line": 1,
                                "end_line": 1,
                                "reason": "Declares the Cargo workspace.",
                            }
                        ],
                    },
                    separators=(",", ":"),
                )
                body = event_stream(
                    [
                        {"type": "response.created", "response": {"id": "resp-2"}},
                        {
                            "type": "response.output_item.done",
                            "item": {
                                "type": "message",
                                "role": "assistant",
                                "id": "msg-1",
                                "content": [{"type": "output_text", "text": answer}],
                            },
                        },
                        {
                            "type": "response.completed",
                            "response": {"id": "resp-2", "usage": USAGE},
                        },
                    ]
                )
            elif type(self).calls == 3:
                if SYMBOLS:
                    raise AssertionError("Symbols smoke made more than two model requests")
                tool_names = [tool.get("name") for tool in request.get("tools", [])]
                if "exec_command" not in tool_names:
                    raise AssertionError(f"Codex did not offer exec_command: {tool_names}")
                body = event_stream(
                    [
                        {"type": "response.created", "response": {"id": "resp-3"}},
                        {
                            "type": "response.output_item.done",
                            "item": {
                                "type": "function_call",
                                "call_id": "read-workspace",
                                "name": "exec_command",
                                "arguments": json.dumps({"cmd": COMMAND}),
                            },
                        },
                        {
                            "type": "response.completed",
                            "response": {"id": "resp-3", "usage": USAGE},
                        },
                    ]
                )
            else:
                raise AssertionError("Codex made more than four model requests")
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        except Exception as error:
            type(self).failure = str(error)
            self.send_error(500, str(error))


def write_wrapper(path, codex, base_url):
    overrides = [
        "--config",
        "model_provider=mock_provider",
        "--config",
        "model=mock-model",
        "--config",
        "model_providers.mock_provider.name=mock",
        "--config",
        f"model_providers.mock_provider.base_url={base_url}/v1",
        "--config",
        "model_providers.mock_provider.wire_api=responses",
        "--config",
        "model_providers.mock_provider.request_max_retries=0",
        "--config",
        "model_providers.mock_provider.stream_max_retries=0",
        "--config",
        "model_providers.mock_provider.supports_websockets=false",
        "--config",
        "features.enable_request_compression=false",
    ]
    if os.name == "nt":
        command = subprocess.list2cmdline([codex]) + " %* " + subprocess.list2cmdline(overrides)
        path.write_text("@echo off\r\ncall " + command + "\r\n", encoding="utf-8")
    else:
        command = " ".join([shlex.quote(codex), '"$@"', *map(shlex.quote, overrides)])
        path.write_text("#!/bin/sh\nexec " + command + "\n", encoding="utf-8")
        path.chmod(0o755)


def send(process, message):
    process.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
    process.stdin.flush()


def read_response(lines, request_id, timeout=90):
    while True:
        line = lines.get(timeout=timeout)
        message = json.loads(line)
        if message.get("id") == request_id:
            return message


def main():
    root = Path(__file__).resolve().parents[1]
    binary = Path(os.environ.get(
        "REPOTRACER_TEST_BINARY",
        str(root / "target" / "debug" / ("repotracer.exe" if os.name == "nt" else "repotracer")),
    )).resolve()
    codex = shutil.which("codex")
    if not binary.is_file():
        raise SystemExit(f"build RepoTracer first: {binary}")
    if not codex:
        raise SystemExit("codex is not installed")

    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeResponses)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base_url = f"http://127.0.0.1:{server.server_port}"

    with tempfile.TemporaryDirectory(prefix="repotracer-app-server-") as temporary:
        temporary = Path(temporary)
        targets = {}
        for name, marker in TARGET_MARKERS.items():
            target = temporary / f"target-{name}"
            target.mkdir()
            (target / "marker.txt").write_text(marker + "\n", encoding="utf-8")
            (target / "Cargo.toml").write_text("[workspace]\nmembers = []\n", encoding="utf-8")
            targets[name] = target.resolve()
        FakeResponses.expected_markers = {
            2: TARGET_MARKERS["a"],
            4: TARGET_MARKERS["b"],
        }
        user_codex_home = temporary / "user-codex-home"
        user_codex_home.mkdir()
        codex_config = '[windows]\nsandbox = "unelevated"\n' if os.name == "nt" else ""
        codex_config += "".join(
            f"[projects.{json.dumps(str(target))}]\ntrust_level = \"trusted\"\n\n"
            for target in targets.values()
        )
        (user_codex_home / "config.toml").write_text(codex_config, encoding="utf-8")
        wrapper = temporary / ("codex-wrapper.cmd" if os.name == "nt" else "codex-wrapper")
        write_wrapper(wrapper, codex, base_url)
        config = temporary / "repotracer.toml"
        config.write_text(
            "[model]\n"
            'backend = "codex-cli"\n'
            f"executable = {json.dumps(str(wrapper))}\n"
            'model = "default"\n'
            'reasoning_effort = "medium"\n'
            "timeout_ms = 60000\n\n"
            "[updates]\n"
            "automatic = false\n",
            encoding="utf-8",
        )

        environment = os.environ.copy()
        environment["CODEX_HOME"] = str(user_codex_home)
        process = subprocess.Popen(
            [str(binary), "--root", str(root), "--config", str(config), "serve"],
            cwd=root,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=environment,
            text=True,
            bufsize=1,
        )
        lines = queue.Queue()
        threading.Thread(
            target=lambda: [lines.put(line) for line in process.stdout], daemon=True
        ).start()
        try:
            send(
                process,
                {
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "clientInfo": {"name": "ci", "version": "1"},
                    },
                },
            )
            initialized = read_response(lines, 1)
            if "error" in initialized:
                raise AssertionError(initialized)
            send(process, {"jsonrpc": "2.0", "method": "notifications/initialized"})
            first_arguments = {"query": "Read the target marker and report the workspace declaration."}
            if not SYMBOLS:
                first_arguments.update(
                    {
                        "repository": str(targets["a"]),
                        "investigation": {"conversation_id": "cwd-proof"},
                    }
                )
            send(
                process,
                {
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "repo_scout",
                        "arguments": first_arguments,
                    },
                },
            )
            response = read_response(lines, 2)
            if "error" in response:
                raise AssertionError(FakeResponses.failure or response)
            structured = response["result"]["structuredContent"]
            if structured["citations"][0]["path"] != "Cargo.toml":
                raise AssertionError(response)
            if structured["stats"]["tool_calls"] < 1:
                raise AssertionError("RepoTracer recorded no completed command")
            if not SYMBOLS:
                if not Path(structured["repository"]).samefile(targets["a"]):
                    raise AssertionError(response)
                conversation = structured["conversation"]
                if conversation["id"] != "cwd-proof":
                    raise AssertionError(response)
                if not Path(conversation["repository"]).samefile(targets["a"]):
                    raise AssertionError(response)
                if conversation["status"] != "fresh":
                    raise AssertionError(response)
                if structured["stats"]["thread_turn"] != 1:
                    raise AssertionError(response)
                send(
                    process,
                    {
                        "jsonrpc": "2.0",
                        "id": 3,
                        "method": "tools/call",
                        "params": {
                            "name": "repo_scout",
                            "arguments": {
                                "query": "Read the target marker again on this new checkout.",
                                "repository": str(targets["b"]),
                                "investigation": {"conversation_id": conversation["id"]},
                            },
                        },
                    },
                )
                response = read_response(lines, 3)
                if "error" in response:
                    raise AssertionError(FakeResponses.failure or response)
                structured = response["result"]["structuredContent"]
                if structured["citations"][0]["path"] != "Cargo.toml":
                    raise AssertionError(response)
                if not Path(structured["repository"]).samefile(targets["b"]):
                    raise AssertionError(response)
                conversation = structured["conversation"]
                if conversation["id"] != "cwd-proof":
                    raise AssertionError(response)
                if not Path(conversation["repository"]).samefile(targets["b"]):
                    raise AssertionError(response)
                if conversation["status"] != "resumed":
                    raise AssertionError(response)
                if structured["stats"]["thread_turn"] != 2:
                    raise AssertionError(response)
            if FakeResponses.failure:
                raise AssertionError(FakeResponses.failure)
            print(
                "real Codex app-server "
                + ("dynamic Symbols" if SYMBOLS else "repository cwd reuse")
                + " passed"
            )
        except Exception:
            process.kill()
            process.wait()
            stderr = process.stderr.read()
            if stderr:
                print(stderr, file=sys.stderr)
            raise
        finally:
            if process.poll() is None:
                process.stdin.close()
                process.wait(timeout=10)
            server.shutdown()


if __name__ == "__main__":
    main()
