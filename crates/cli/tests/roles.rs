#![cfg(unix)]
use assert_cmd::Command;
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

fn cli(root: &Path) -> Command {
    let mut command = Command::cargo_bin("repotracer").unwrap();
    command.args(["--config", root.join("config.toml").to_str().unwrap()]);
    command.env("CODEX_HOME", root.join("codex"));
    command.env("REPOTRACER_CONFIG", root.join("config.toml"));
    command.env("CLAUDE_CONFIG_DIR", root.join("claude-home"));
    command.env(
        "PATH",
        format!("{}:{}", root.display(), std::env::var("PATH").unwrap()),
    );
    command.env("REPOTRACER_NO_UPDATE", "1");
    command.env("CLAUDE_CALL_LOG", root.join("claude-calls.jsonl"));
    command
}

fn fake_claude(root: &Path) {
    let fake = root.join("claude");
    fs::write(
        &fake,
        r#"#!/usr/bin/env python3
import os,sys,json,pathlib
root=pathlib.Path(os.environ['CLAUDE_CALL_LOG']).parent
with open(os.environ['CLAUDE_CALL_LOG'],'a') as f: f.write(json.dumps(sys.argv[1:])+'\n')
marker=root/'registered.json'
if sys.argv[1:3]==['mcp','add-json']:
    if marker.exists():
        print('MCP server repotracer already exists in user config',file=sys.stderr)
        sys.exit(1)
    if (root/'fail-add').exists(): sys.exit(1)
    marker.write_text(sys.argv[-1])
elif sys.argv[1:3]==['mcp','remove']:
    if (root/'fail-remove').exists():
        print('permission denied while removing MCP entry',file=sys.stderr)
        sys.exit(7)
    if not marker.exists(): sys.exit(1)
    marker.unlink()
else: sys.exit(2)
"#,
    )
    .unwrap();
    fs::set_permissions(fake, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn doctor_requires_verified_claude_authentication() {
    for (body, ready) in [
        ("printf '%s\\n' '{\"loggedIn\":false}'", false),
        ("printf '%s\\n' '{\"loggedIn\":true}'", true),
        ("printf '%s\\n' 'invalid-json'", false),
        ("if [ -n \"$ANTHROPIC_API_KEY\" ]; then printf '%s\\n' '{\"loggedIn\":true}'; else printf '%s\\n' '{\"loggedIn\":false}'; fi", false),
        ("exit 1", false),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let fake = root.join("claude");
        fs::write(&fake, format!(
            "#!/bin/sh\n[ \"$*\" = \"auth status --json\" ] || exit 9\n{body}\n"
        )).unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let mut config = repotracer_core::RepoTracerConfig::default();
        config.model.backend = "Claude-cli".into();
        config.model.model = "sonnet".into();
        config.model.executable = Some(fake.display().to_string());
        config.save_to(&root.join("config.toml")).unwrap();
        let output = cli(root).env("ANTHROPIC_API_KEY", "test-api-key")
            .args(["--root", root.to_str().unwrap(), "--json", "doctor"])
            .output().unwrap();
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["ready"], ready, "{result}");
        assert_eq!(output.status.success(), ready);
    }
}

#[test]
fn installs_both_and_changes_one_mapping_without_overwriting_the_other() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fake_claude(root);
    fs::create_dir(root.join("codex")).unwrap();
    fs::write(
        root.join("codex/config.toml"),
        "# keep my preferences\nmodel = \"custom\"\n",
    )
    .unwrap();
    cli(root)
        .args(["settings", "--agents", "both"])
        .assert()
        .success();
    let codex = fs::read_to_string(root.join("config.codex.toml")).unwrap();
    let claude = fs::read_to_string(root.join("config.claude.toml")).unwrap();
    assert!(codex.contains("codex-cli"));
    assert!(claude.contains("claude-cli"));
    let integration = fs::read_to_string(root.join("codex/config.toml")).unwrap();
    assert!(integration.contains("# keep my preferences"));
    assert!(integration.contains("config.codex.toml"));
    let calls = fs::read_to_string(root.join("claude-calls.jsonl")).unwrap();
    assert!(calls.contains("config.claude.toml"));
    cli(root)
        .args([
            "settings",
            "--claude-scout",
            "codex",
            "--claude-model",
            "gpt-5.6-luna",
        ])
        .assert()
        .success();
    assert_eq!(
        fs::read_to_string(root.join("config.codex.toml")).unwrap(),
        codex
    );
    assert!(fs::read_to_string(root.join("config.claude.toml"))
        .unwrap()
        .contains("codex-cli"));
    cli(root)
        .args([
            "settings",
            "--codex-scout",
            "claude",
            "--codex-model",
            "sonnet",
        ])
        .assert()
        .success();
    assert!(fs::read_to_string(root.join("config.codex.toml"))
        .unwrap()
        .contains("sonnet"));
    cli(root).arg("__refresh-integration").assert().success();
    cli(root).arg("setup").assert().success();
    assert!(fs::read_to_string(root.join("codex/config.toml"))
        .unwrap()
        .contains("config.codex.toml"));
    cli(root).args(["uninstall", "--yes"]).assert().success();
    assert!(!root.join("config.codex.toml").exists());
    assert!(!root.join("config.claude.toml").exists());
}

#[test]
fn failed_second_parent_retains_first_and_can_be_retried() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fake_claude(root);
    fs::write(root.join("fail-add"), "fixture").unwrap();
    cli(root)
        .args(["settings", "--agents", "both"])
        .assert()
        .failure();
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("config.integrations.json")).unwrap()).unwrap();
    assert_eq!(saved["parents"], serde_json::json!(["codex"]));
    assert!(root.join("config.codex.toml").exists());
    assert!(!root.join("config.claude.toml").exists());
    fs::remove_file(root.join("fail-add")).unwrap();
    cli(root)
        .args(["settings", "--agents", "both"])
        .assert()
        .success();
    assert!(root.join("config.claude.toml").exists());
}

#[test]
fn untracked_claude_registration_is_never_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fake_claude(root);
    fs::write(root.join("registered.json"), "user-owned registration").unwrap();
    cli(root)
        .args(["settings", "--agents", "claude"])
        .assert()
        .failure();
    assert_eq!(
        fs::read_to_string(root.join("registered.json")).unwrap(),
        "user-owned registration"
    );
    assert!(!root.join("config.claude.toml").exists());
    assert!(!fs::read_to_string(root.join("claude-calls.jsonl"))
        .unwrap()
        .contains("remove"));
}

#[test]
fn uninstall_persists_successful_parent_removal_before_a_later_failure() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fake_claude(root);
    cli(root)
        .args(["settings", "--agents", "both"])
        .assert()
        .success();
    fs::write(root.join("fail-remove"), "fixture").unwrap();

    cli(root)
        .args(["uninstall", "--yes"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "permission denied while removing MCP entry",
        ));

    let state: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("config.integrations.json")).unwrap()).unwrap();
    assert_eq!(state["parents"], serde_json::json!(["claude"]));
    assert!(!root.join("config.codex.toml").exists());
    assert!(root.join("config.claude.toml").exists());
}

#[test]
fn uninstall_without_claude_cli_still_removes_managed_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let claude_home = root.join("claude-home");
    fs::create_dir_all(&claude_home).unwrap();
    fs::write(
        claude_home.join("CLAUDE.md"),
        "user instructions\n\n<!-- repotracer:start -->\nrouting\n<!-- repotracer:end -->\n",
    )
    .unwrap();
    fs::write(
        root.join("config.integrations.json"),
        r#"{"parents":["claude"]}"#,
    )
    .unwrap();
    fs::write(root.join("config.claude.toml"), "profile").unwrap();

    cli(root)
        .env("PATH", root)
        .args(["uninstall", "--yes"])
        .assert()
        .success();

    assert!(!root.join("config.integrations.json").exists());
    assert!(!root.join("config.claude.toml").exists());
    assert_eq!(
        fs::read_to_string(claude_home.join("CLAUDE.md")).unwrap(),
        "user instructions\n"
    );
}

#[test]
fn shared_tracer_choice_applies_to_both_selected_harnesses() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fake_claude(root);
    cli(root)
        .args([
            "settings",
            "--agents",
            "both",
            "--tracer-model",
            "claude:sonnet",
        ])
        .assert()
        .success();
    for parent in ["codex", "claude"] {
        let text = fs::read_to_string(root.join(format!("config.{parent}.toml"))).unwrap();
        assert!(text.contains("model = \"sonnet\""));
        assert!(text.contains("backend = \"claude-cli\""));
    }
    let claude_calls = fs::read(root.join("claude-calls.jsonl")).unwrap();
    cli(root)
        .args([
            "settings",
            "--agents",
            "codex",
            "--tracer-model",
            "codex:o3",
        ])
        .assert()
        .success();
    assert_eq!(
        fs::read(root.join("claude-calls.jsonl")).unwrap(),
        claude_calls
    );
    assert!(fs::read_to_string(root.join("config.claude.toml"))
        .unwrap()
        .contains("sonnet"));
    assert!(fs::read_to_string(root.join("config.codex.toml"))
        .unwrap()
        .contains("model = \"o3\""));
}

#[test]
fn settings_without_a_terminal_is_read_only() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fake_claude(root);
    cli(root)
        .args(["settings", "--agents", "both"])
        .assert()
        .success();
    let before = fs::read(root.join("claude-calls.jsonl")).unwrap();
    let codex = fs::read(root.join("config.codex.toml")).unwrap();
    cli(root).arg("settings").assert().success();
    cli(root).arg("reconfigure").assert().success();
    assert_eq!(fs::read(root.join("claude-calls.jsonl")).unwrap(), before);
    assert_eq!(fs::read(root.join("config.codex.toml")).unwrap(), codex);
}

#[test]
fn npm_settings_preview_does_not_install_or_launch_claude() {
    let temp = tempfile::tempdir().unwrap();
    let launcher =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/npm/bin/repotracer.js");
    let status = std::process::Command::new("node")
        .arg(launcher)
        .env("REPOTRACER_BIN", assert_cmd::cargo::cargo_bin("repotracer"))
        .args(["settings", "--agents", "both", "--dry-run", "--config"])
        .arg(temp.path().join("config.toml"))
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert!(!temp.path().join("config.codex.toml").exists());
}
