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


COMMAND = 'rg -n "^\\[workspace\\]$" Cargo.toml'
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


class FakeResponses(BaseHTTPRequestHandler):
    calls = 0
    failure = None

    def log_message(self, _format, *_args):
        pass

    def do_POST(self):
        try:
            length = int(self.headers.get("Content-Length", "0"))
            request = json.loads(self.rfile.read(length))
            type(self).calls += 1
            if type(self).calls == 1:
                tool_names = [tool.get("name") for tool in request.get("tools", [])]
                if "exec_command" not in tool_names:
                    raise AssertionError(f"Codex did not offer exec_command: {tool_names}")
                body = event_stream(
                    [
                        {"type": "response.created", "response": {"id": "resp-1"}},
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
                            "response": {"id": "resp-1", "usage": USAGE},
                        },
                    ]
                )
            elif type(self).calls == 2:
                tool_result = json.dumps(request, separators=(",", ":"))
                if "1:[workspace]" not in tool_result:
                    raise AssertionError(
                        "Codex did not return the successful rg output: " + tool_result
                    )
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
            else:
                raise AssertionError("Codex made more than two model requests")
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
    binary = root / "target" / "debug" / ("repotracer.exe" if os.name == "nt" else "repotracer")
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

        process = subprocess.Popen(
            [str(binary), "--root", str(root), "--config", str(config), "serve"],
            cwd=root,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
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
            send(
                process,
                {
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/call",
                    "params": {
                        "name": "repo_scout",
                        "arguments": {"query": "Find the Cargo workspace declaration."},
                    },
                },
            )
            response = read_response(lines, 2)
            if "error" in response:
                raise AssertionError(response)
            structured = response["result"]["structuredContent"]
            if structured["citations"][0]["path"] != "Cargo.toml":
                raise AssertionError(response)
            if structured["stats"]["tool_calls"] < 1:
                raise AssertionError("RepoTracer recorded no completed command")
            if FakeResponses.failure:
                raise AssertionError(FakeResponses.failure)
            print("real Codex app-server repository read passed")
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
