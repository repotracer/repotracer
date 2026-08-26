use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn dry_run_plans_zero_question_gpt_setup_without_writes() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let config = home.path().join(".repotracer").join("config.toml");

    let mut command = Command::cargo_bin("repotracer").unwrap();
    command
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CODEX_HOME", home.path().join(".codex"))
        .env("REPOTRACER_CONFIG", &config)
        .args([
            "--root",
            root.path().to_str().unwrap(),
            "setup",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("gpt-5.6-luna"))
        .stdout(predicate::str::contains("Codex MCP + automatic routing"))
        .stdout(predicate::str::contains("Dry run. Nothing was changed."));

    assert!(!config.exists());
    assert!(!home.path().join(".codex").exists());
}

#[test]
fn custom_setup_accepts_only_gpt_models() {
    let root = tempfile::tempdir().unwrap();
    let mut command = Command::cargo_bin("repotracer").unwrap();
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

    let mut command = Command::cargo_bin("repotracer").unwrap();
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

#[test]
fn setup_does_not_require_an_existing_codex_login() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let codex = if cfg!(windows) {
        bin.join("codex.cmd")
    } else {
        bin.join("codex")
    };
    #[cfg(windows)]
    std::fs::write(&codex, "@exit /b 1\r\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&codex, "#!/bin/sh\nexit 1\n").unwrap();
        let mut permissions = std::fs::metadata(&codex).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&codex, permissions).unwrap();
    }

    let config = home.path().join(".repotracer/config.toml");
    let codex_home = home.path().join(".codex");
    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(&inherited_path));
    let path = std::env::join_paths(paths).unwrap();
    let mut command = Command::cargo_bin("repotracer").unwrap();
    command
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("PATH", path)
        .env("CODEX_HOME", &codex_home)
        .env("REPOTRACER_CONFIG", &config)
        .args(["--root", root.path().to_str().unwrap(), "setup"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Codex found"))
        .stdout(predicate::str::contains("Ready. Restart Codex"))
        .stdout(predicate::str::contains("will update automatically"))
        .stdout(predicate::str::contains("updates.automatic = false"))
        .stdout(predicate::str::contains("Install RepoTracer updates automatically?").not());

    assert!(std::fs::read_to_string(&config)
        .unwrap()
        .contains("model = \"gpt-5.6-luna\""));
    assert!(std::fs::read_to_string(&config)
        .unwrap()
        .contains("reasoning_effort = \"medium\""));
    assert!(std::fs::read_to_string(codex_home.join("config.toml"))
        .unwrap()
        .contains("mcp_servers.repotracer"));
    assert!(std::fs::read_to_string(codex_home.join("AGENTS.md"))
        .unwrap()
        .contains("Validated citations complete broad exploration"));
}

#[test]
fn updater_refreshes_the_managed_codex_files() {
    let home = tempfile::tempdir().unwrap();
    let codex_home = home.path().join(".codex");

    let mut command = Command::cargo_bin("repotracer").unwrap();
    command
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CODEX_HOME", &codex_home)
        .arg("__refresh-integration")
        .assert()
        .success();

    assert!(std::fs::read_to_string(codex_home.join("config.toml"))
        .unwrap()
        .contains("mcp_servers.repotracer"));
    assert!(std::fs::read_to_string(codex_home.join("AGENTS.md"))
        .unwrap()
        .contains("repotracer:start"));
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

    let mut command = Command::cargo_bin("repotracer").unwrap();
    command
        .env("REPOTRACER_CONFIG", &config)
        .args(["--json", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<redacted>"))
        .stdout(predicate::str::contains("secret-provider-token").not());
}

#[test]
fn doctor_shows_backend_failure_and_exits_nonzero() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.toml");
    std::fs::write(
        &config,
        r#"[model]
backend = "codex-cli"
executable = "/definitely/missing/codex"
model = "gpt-5.6-luna"
reasoning_effort = "medium"
"#,
    )
    .unwrap();

    let mut command = Command::cargo_bin("repotracer").unwrap();
    command
        .env("REPOTRACER_CONFIG", &config)
        .args(["--root", root.path().to_str().unwrap(), "doctor"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("✗ GPT scout CLI"))
        .stdout(predicate::str::contains("NOT READY"));

    let mut command = Command::cargo_bin("repotracer").unwrap();
    command
        .env("REPOTRACER_CONFIG", &config)
        .args(["--json", "--root", root.path().to_str().unwrap(), "doctor"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("\"ready\": false"));
}

/// Setup is a global install and must not touch the working directory.
///
/// A user ran `npx repotracer setup` from their Windows home folder. Setup was
/// running the full doctor, which globbed the whole of `C:\Users\pc` and then
/// made a live model call, so it appeared to hang for minutes. Nothing in setup
/// should depend on, or scan, the directory it happens to be run from.
#[test]
fn setup_is_fast_and_does_not_scan_the_working_directory() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    // A working directory that would be expensive to walk.
    for i in 0..300 {
        let dir = cwd.path().join(format!("dir{i}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file.txt"), "x").unwrap();
    }

    let started = std::time::Instant::now();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_repotracer"))
        .args(["setup", "--dry-run"])
        .current_dir(cwd.path())
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CODEX_HOME", home.path().join(".codex"))
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let elapsed = started.elapsed();

    assert!(output.status.success(), "setup --dry-run failed");
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "setup took {elapsed:?}; it must not scan the working directory or call a model"
    );

    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        !text.contains("Git repository"),
        "setup must not report on the working directory: {text}"
    );
}
