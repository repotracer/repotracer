use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn dry_run_plans_model_and_routing_without_writes() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let config = home.path().join(".grephound").join("config.toml");

    let mut command = Command::cargo_bin("grephound").unwrap();
    command
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("GREPHOUND_CONFIG", &config)
        .args([
            "--root",
            root.path().to_str().unwrap(),
            "setup",
            "--dry-run",
            "--provider",
            "ollama",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "hf.co/mitkox/FastContext-1.0-4B-RL-Q4_K_M-GGUF:latest",
        ))
        .stdout(predicate::str::contains("Claude Code MCP + skill"))
        .stdout(predicate::str::contains(
            "GitHub Copilot MCP + project instructions",
        ))
        .stdout(predicate::str::contains(
            "would verify a real model tool call",
        ))
        .stdout(predicate::str::contains("DRY RUN COMPLETE"));

    assert!(!config.exists());
    assert!(!home.path().join(".claude").exists());
    assert!(!root.path().join(".github").exists());
}

#[test]
fn dry_run_supports_subscription_and_custom_backends() {
    for (provider, expected) in [
        ("codex", "selected Codex subscription"),
        ("claude", "selected Claude subscription"),
    ] {
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut command = Command::cargo_bin("grephound").unwrap();
        command
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .args([
                "--root",
                root.path().to_str().unwrap(),
                "setup",
                "--dry-run",
                "--provider",
                provider,
            ])
            .assert()
            .success()
            .stdout(predicate::str::contains(expected))
            .stdout(predicate::str::contains("would verify"));
    }

    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut command = Command::cargo_bin("grephound").unwrap();
    command
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .args([
            "--root",
            root.path().to_str().unwrap(),
            "--base-url",
            "https://models.example.com/v1",
            "--model",
            "scout-model",
            "setup",
            "--dry-run",
            "--provider",
            "custom",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "selected custom OpenAI-compatible endpoint",
        ))
        .stdout(predicate::str::contains("https://models.example.com/v1"));
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
