#!/usr/bin/env python3
"""Exercise a real tagged-binary upgrade against the current checkout."""

from __future__ import annotations

import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import platform
import shutil
import socketserver
import subprocess
import tempfile
import threading


ROOT = Path(__file__).resolve().parents[1]


def run(*args: str | Path, cwd: Path = ROOT, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(
        [str(arg) for arg in args],
        cwd=cwd,
        env=env,
        text=True,
        capture_output=True,
        timeout=900,
    )
    if result.returncode:
        raise RuntimeError(result.stderr or result.stdout)
    return result


def binary(root: Path) -> Path:
    name = "repotracer.exe" if os.name == "nt" else "repotracer"
    return root / "target" / "debug" / name


def asset_name() -> str:
    key = (platform.system(), platform.machine().lower())
    assets = {
        ("Darwin", "arm64"): "repotracer-darwin-arm64",
        ("Darwin", "x86_64"): "repotracer-darwin-x64",
        ("Linux", "aarch64"): "repotracer-linux-arm64",
        ("Linux", "x86_64"): "repotracer-linux-x64",
        ("Windows", "amd64"): "repotracer-windows-x64.exe",
        ("Windows", "x86_64"): "repotracer-windows-x64.exe",
    }
    try:
        return assets[key]
    except KeyError as error:
        raise RuntimeError(f"unsupported test platform: {key[0]}-{key[1]}") from error


def version(executable: Path, env: dict[str, str] | None = None) -> str:
    return run(executable, "version", env=env).stdout.strip().split()[-1]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--from-tag", default="v0.1.9")
    parser.add_argument("--to-version", default="1.0.0")
    args = parser.parse_args()

    run("git", "rev-parse", "--verify", args.from_tag)
    run("cargo", "build", "-q", "-p", "repotracer")
    release_binary = binary(ROOT)
    if version(release_binary) != args.to_version:
        raise RuntimeError(f"current binary is {version(release_binary)}, expected {args.to_version}")

    with tempfile.TemporaryDirectory(prefix="repotracer-upgrade-") as directory:
        work = Path(directory)
        old_source = work / "old-source"
        run("git", "clone", "-q", "--shared", str(ROOT), old_source)
        run("git", "checkout", "-q", args.from_tag, cwd=old_source)
        old_target = work / "old-target"
        old_env = {**os.environ, "CARGO_TARGET_DIR": str(old_target)}
        run("cargo", "build", "-q", "-p", "repotracer", cwd=old_source, env=old_env)
        old_binary = old_target / "debug" / release_binary.name
        if version(old_binary) != args.from_tag.removeprefix("v"):
            raise RuntimeError("the historical binary has the wrong version")

        home = work / "home"
        codex_home = home / ".codex"
        extension = ".exe" if os.name == "nt" else ""
        managed = home / ".repotracer" / "bin" / f"repotracer{extension}"
        managed.parent.mkdir(parents=True)
        codex_home.mkdir(parents=True)
        shutil.copy2(old_binary, managed)
        if os.name != "nt":
            managed.chmod(0o755)

        app_config = home / ".repotracer" / "config.toml"
        codex_config = codex_home / "config.toml"
        agents = codex_home / "AGENTS.md"
        codex_config.write_text('model = "user-choice"\n\n[user_settings]\nkeep = true\n')
        agents.write_text("# User rules\nNever remove this line.\n")
        environment = {
            **os.environ,
            "HOME": str(home),
            "USERPROFILE": str(home),
            "CODEX_HOME": str(codex_home),
            "REPOTRACER_CONFIG": str(app_config),
        }
        run(managed, "config", "--init", env=environment)
        run(managed, "__refresh-integration", env=environment)
        old_config_hash = hashlib.sha256(app_config.read_bytes()).digest()

        payload = release_binary.read_bytes()
        digest = hashlib.sha256(payload).hexdigest()
        asset = asset_name()
        routes = {
            "/releases/latest": json.dumps({"tag_name": f"v{args.to_version}"}).encode(),
            "/SHA256SUMS": f"{digest}  ./{asset}/{asset}\n".encode(),
            f"/{asset}": payload,
        }

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:
                body = routes.get(self.path)
                if body is None:
                    self.send_response(404)
                    self.end_headers()
                    return
                self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_: object) -> None:
                pass

        server = socketserver.TCPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        base = f"http://127.0.0.1:{server.server_address[1]}"
        update_environment = {
            **environment,
            "REPOTRACER_RELEASE_API": f"{base}/releases/latest",
            "REPOTRACER_RELEASE_BASE_URL": base,
        }
        try:
            update = run(managed, "update", env=update_environment)
        finally:
            server.shutdown()
            server.server_close()

        if f"Updated to {args.to_version}." not in update.stdout:
            raise RuntimeError(f"unexpected updater output: {update.stdout}")
        if managed.read_bytes() != payload or version(managed, environment) != args.to_version:
            raise RuntimeError("the managed binary was not replaced")
        if hashlib.sha256(app_config.read_bytes()).digest() != old_config_hash:
            raise RuntimeError("the updater changed the existing RepoTracer config")
        run(managed, "--json", "status", env=environment)
        run(managed, "--mock", "--root", ROOT, "scout", "where is routing handled?", env=environment)

        refreshed_config = codex_config.read_text()
        refreshed_agents = agents.read_text()
        checks = {
            "user Codex config": 'model = "user-choice"' in refreshed_config
            and "[user_settings]" in refreshed_config,
            "single MCP entry": refreshed_config.count("[mcp_servers.repotracer]") == 1,
            "user instructions": "Never remove this line." in refreshed_agents,
            "current routing block": refreshed_agents.count("<!-- repotracer:start -->") == 1
            and "Examples that stay local" in refreshed_agents,
        }
        failed = [name for name, passed in checks.items() if not passed]
        if failed:
            raise RuntimeError(f"upgrade compatibility failed: {', '.join(failed)}")

    print(f"ok: {args.from_tag} upgraded to {args.to_version}; config, MCP, routing, and Scout still work")


if __name__ == "__main__":
    main()
