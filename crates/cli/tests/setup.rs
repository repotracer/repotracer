use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn dry_run_plans_zero_question_gpt_setup_without_writes() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let config = home.path().join(".grephound").join("config.toml");

    let mut command = Command::cargo_bin("grephound").unwrap();
    command
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CODEX_HOME", home.path().join(".codex"))
        .env("GREPHOUND_CONFIG", &config)
        .args([
            "--root",
            root.path().to_str().unwrap(),
            "setup",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("2 / 4  GPT scout"))
        .stdout(predicate::str::contains("gpt-5.6-luna"))
        .stdout(predicate::str::contains("Codex MCP + automatic routing"))
        .stdout(predicate::str::contains("DRY RUN COMPLETE"));

    assert!(!config.exists());
    assert!(!home.path().join(".codex").exists());
}

#[test]
fn custom_setup_accepts_only_gpt_models() {
    let root = tempfile::tempdir().unwrap();
    let mut command = Command::cargo_bin("grephound").unwrap();
    command
        .args([
            "--root",
            root.path().to_str().unwrap(),
            "--base-url",
            "https://models.example.com/v1",
            "--model",
            "gpt-5.6-mini",
            "setup",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("gpt-5.6-mini"))
        .stdout(predicate::str::contains("https://models.example.com/v1"));

    let mut command = Command::cargo_bin("grephound").unwrap();
    command
        .args([
            "--root",
            root.path().to_str().unwrap(),
            "--base-url",
            "https://models.example.com/v1",
            "--model",
            "claude",
            "setup",
            "--dry-run",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("currently supports GPT models"));
}

#[cfg(unix)]
#[test]
fn setup_uses_existing_codex_login_without_prompts() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let codex = bin.join("codex");
    std::fs::write(&codex, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = std::fs::metadata(&codex).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&codex, permissions).unwrap();

    let config = home.path().join(".grephound/config.toml");
    let codex_home = home.path().join(".codex");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut command = Command::cargo_bin("grephound").unwrap();
    command
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("PATH", path)
        .env("CODEX_HOME", &codex_home)
        .env("GREPHOUND_CONFIG", &config)
        .args(["--root", root.path().to_str().unwrap(), "setup"])
        .assert()
        .success()
        .stdout(predicate::str::contains("GPT scout via Codex CLI ready"))
        .stdout(predicate::str::contains("READY"));

    assert!(std::fs::read_to_string(&config)
        .unwrap()
        .contains("model = \"gpt-5.6-luna\""));
    assert!(std::fs::read_to_string(&config)
        .unwrap()
        .contains("reasoning_effort = \"medium\""));
    assert!(std::fs::read_to_string(codex_home.join("config.toml"))
        .unwrap()
        .contains("mcp_servers.grephound"));
    assert!(std::fs::read_to_string(codex_home.join("AGENTS.md"))
        .unwrap()
        .contains("Validated citations complete broad exploration"));
}

#[test]
fn status_json_redacts_api_key() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config.toml");
    std::fs::write(
        &config,
        r#"[model]
backend = "openai-compatible"
model = "scout"
base_url = "https://models.example.com/v1"
api_key = "secret-provider-token"
"#,
    )
    .unwrap();

    let mut command = Command::cargo_bin("grephound").unwrap();
    command
        .env("GREPHOUND_CONFIG", &config)
        .args(["--json", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<redacted>"))
        .stdout(predicate::str::contains("secret-provider-token").not());
}
