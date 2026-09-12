use super::*;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use windows_sys::Win32::{
    Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE},
};

fn fixture(mode: &str) -> (tempfile::TempDir, RepoTracerConfig, ScoutRequest) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("mode"), mode).unwrap();
    // Use an executable wrapper: std deliberately rejects multiline arguments
    // for .cmd files, and Claude's real system prompt contains newlines.
    std::fs::write(dir.path().join("runner.rs"), r#"
use std::os::windows::process::CommandExt;
fn main() {
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", "provider.ps1"])
        .creation_flags(0x08000000).status().unwrap();
    std::process::exit(status.code().unwrap_or(1));
}
"#).unwrap();
    let compile = std::process::Command::new("rustc")
        .arg(dir.path().join("runner.rs"))
        .arg("-o")
        .arg(dir.path().join("runner.exe"))
        .output()
        .unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    std::fs::write(
        dir.path().join("helper.ps1"),
        r#"
$start = New-Object Diagnostics.ProcessStartInfo
$start.FileName = 'powershell.exe'
$start.Arguments = '-NoProfile -NonInteractive -Command Start-Sleep -Seconds 60'
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$leaf = [Diagnostics.Process]::Start($start)
[IO.File]::WriteAllText((Join-Path (Get-Location) 'leaf-pid'), [string]$leaf.Id)
Start-Sleep -Seconds 60
"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("provider.ps1"), r#"
$ErrorActionPreference = 'Stop'
$start = New-Object Diagnostics.ProcessStartInfo
$start.FileName = 'powershell.exe'
$start.Arguments = '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File helper.ps1'
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$helper = [Diagnostics.Process]::Start($start)
[IO.File]::WriteAllText((Join-Path (Get-Location) 'helper-pid'), [string]$helper.Id)
while (!(Test-Path 'leaf-pid')) { Start-Sleep -Milliseconds 10 }
$mode = [IO.File]::ReadAllText((Join-Path (Get-Location) 'mode'))
if ($mode -eq 'exit') { exit 0 }
while ($line = [Console]::ReadLine()) {
    if ($mode -eq 'hang') { Start-Sleep -Seconds 60 }
    elseif ($mode -eq 'error') { [Console]::WriteLine('{"type":"result","subtype":"error","is_error":true,"result":"fixture failure"}') }
    else { [Console]::WriteLine('{"type":"result","subtype":"success","is_error":false,"num_turns":1,"structured_output":{"summary":"fixture","status":"partial","findings":[],"unresolved":["fixture"],"searched_scope":[],"limitations":[]},"usage":{"input_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":1}}') }
}
"#).unwrap();
    let mut cfg = RepoTracerConfig::default();
    cfg.model.backend = "claude-cli".into();
    cfg.model.model = "haiku".into();
    cfg.model.executable = Some(dir.path().join("runner.exe").display().to_string());
    cfg.model.reasoning_effort = "medium".into();
    cfg.session.idle_secs = 2;
    let request = ScoutRequest {
        investigation: repotracer_core::InvestigationSpec {
            conversation_id: Some("windows".into()),
            ..Default::default()
        },
        query: "fixture".into(),
        root: dir.path().into(),
        focus: None,
        max_turns: Some(2),
        timeout: Some(Duration::from_secs(10)),
    };
    (dir, cfg, request)
}

async fn descendants(root: &std::path::Path) -> Vec<OwnedHandle> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut handles = Vec::new();
    for name in ["helper-pid", "leaf-pid"] {
        let pid = loop {
            if let Ok(text) = std::fs::read_to_string(root.join(name)) {
                if let Ok(pid) = text.parse::<u32>() {
                    break pid;
                }
            }
            assert!(Instant::now() < deadline, "fixture never started {name}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        // Hold handles, not just PIDs, so PID reuse cannot make the test pass.
        let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(!raw.is_null(), "could not observe fixture {name}");
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        assert_eq!(
            unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) },
            WAIT_TIMEOUT
        );
        handles.push(handle);
    }
    handles
}

async fn assert_dead(handles: &[OwnedHandle]) {
    let deadline = Instant::now() + Duration::from_secs(10);
    for handle in handles {
        loop {
            let status = unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) };
            if status == WAIT_OBJECT_0 {
                break;
            }
            assert_eq!(status, WAIT_TIMEOUT);
            assert!(
                Instant::now() < deadline,
                "Claude descendant survived cleanup"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

#[tokio::test]
async fn windows_cleanup_reaches_descendants_after_parent_exit_and_without_runtime() {
    for mode in ["exit", "explicit", "drop-without-runtime", "eviction"] {
        let (dir, cfg, request) = fixture(mode);
        let scout = ClaudeScout::new(&cfg).unwrap();
        let provider_identity =
            claude_provider_identity(cfg.model.executable.as_deref().map(std::path::Path::new))
                .unwrap();
        let mut conversation = scout
            .spawn(
                &request,
                dir.path().canonicalize().unwrap(),
                "windows".into(),
                "medium",
                provider_identity,
            )
            .await
            .unwrap();
        let handles = descendants(dir.path()).await;
        match mode {
            "exit" => {
                tokio::time::timeout(Duration::from_secs(10), conversation.child.wait())
                    .await
                    .unwrap()
                    .unwrap();
                conversation.kill_tree().await;
            }
            "explicit" => {
                conversation.kill_tree().await;
                conversation.kill_tree().await;
            }
            "drop-without-runtime" => {
                std::thread::spawn(move || drop(conversation))
                    .join()
                    .unwrap();
            }
            "eviction" => {
                let mut store = SessionStore::new();
                let evicted = store.insert(
                    SessionKey {
                        provider_identity,
                        id: "windows".into(),
                    },
                    conversation,
                    0,
                );
                ClaudeScout::retire_many(evicted).await;
            }
            _ => unreachable!(),
        }
        assert_dead(&handles).await;
    }
}

#[tokio::test]
async fn windows_idle_reaper_kills_warm_descendants() {
    let (dir, cfg, request) = fixture("success");
    let scout = ClaudeScout::new(&cfg).unwrap();
    let first = scout.scout(request.clone()).await.unwrap();
    let handles = descendants(dir.path()).await;
    let second = scout.scout(request).await.unwrap();
    assert!(!first.stats.warm_process);
    assert!(second.stats.warm_process);
    assert_dead(&handles).await;
    assert!(scout.lock_sessions().sessions.is_empty());
}

#[tokio::test]
async fn windows_cancelled_and_failed_turns_kill_descendants() {
    for mode in ["cancel", "timeout", "error"] {
        let (dir, cfg, mut request) = fixture(if mode == "error" { "error" } else { "hang" });
        if mode == "timeout" {
            request.timeout = Some(Duration::from_secs(3));
        }
        let scout = ClaudeScout::new(&cfg).unwrap();
        let provider_identity =
            claude_provider_identity(cfg.model.executable.as_deref().map(std::path::Path::new))
                .unwrap();
        // Spawn first to observe the descendants before a fast error retires them.
        let conversation = scout
            .spawn(
                &request,
                dir.path().canonicalize().unwrap(),
                "windows".into(),
                "medium",
                provider_identity,
            )
            .await
            .unwrap();
        let handles = descendants(dir.path()).await;
        scout.lock_sessions().sessions.insert(
            SessionKey {
                provider_identity,
                id: "windows".into(),
            },
            conversation,
        );
        let task = tokio::spawn(async move { scout.scout(request).await });
        if mode == "cancel" {
            tokio::time::sleep(Duration::from_millis(100)).await;
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            assert!(task.await.unwrap().is_err());
        }
        assert_dead(&handles).await;
    }
}
