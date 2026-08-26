//! Keeping the installed binary current, without asking anyone to run anything.
//!
//! Codex launches RepoTracer from a fixed path (`~/.repotracer/bin/repotracer`),
//! so an update is a file swap: fetch the newest release asset, verify its
//! SHA-256 against the published `SHA256SUMS`, and put it in place. npm is only
//! ever the installer; it plays no part at runtime.
//!
//! Replacing a *running* executable is the one genuinely hard part, and it is
//! not ours to solve. Unix can rename over a mapped image; Windows can rename
//! the image aside but cannot unlink it, so the old file has to be cleaned up
//! after the process exits. `self-replace` is the crate that encapsulates
//! exactly that, and it is the same one `self_update` and the uv/rye lineage
//! use. We call it and stay out of the way.
//!
//! The swap cannot affect the process doing it, since that code is already
//! mapped, so a new version takes effect the next time Codex starts. That is
//! deliberate: an update applying mid-session would change tool behaviour
//! underneath a running conversation.
//!
//! Three rules keep this from being something the user resents:
//!
//! - It only ever touches the binary under `~/.repotracer/bin`. A `cargo install`
//!   build, a source checkout, or an npx vendor copy belongs to whatever put it
//!   there.
//! - It never runs on the tool-call path. It is a background task at startup,
//!   throttled to one check a day, the same cadence Deno and rustup use.
//! - Every failure is silent. An unreachable release feed is not the user's
//!   problem and must not surface during a coding session.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::debug;

const RELEASES_LATEST: &str = "https://api.github.com/repos/repotracer/repotracer/releases/latest";
const DOWNLOAD_BASE: &str = "https://github.com/repotracer/repotracer/releases/download";
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(60);
/// A wrong URL that returns a huge body must not fill the user's disk.
const MAX_BINARY_BYTES: u64 = 128 * 1024 * 1024;

/// A verified binary sitting next to the one it will replace.
///
/// Downloading and verifying is separated from the swap so the whole pipeline
/// can be tested without a test having to replace its own executable.
pub struct Staged {
    pub version: String,
    pub path: PathBuf,
}

impl Drop for Staged {
    fn drop(&mut self) {
        // A staged file that never got committed is debris in the user's bin
        // directory. `self_replace` consumes the file by copying, not moving, so
        // this runs after a successful commit too.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Kick off an update check without delaying whatever comes next.
///
/// Called once when the MCP server starts.
pub fn spawn(enabled: bool) {
    if !enabled || disabled_by_env() {
        return;
    }
    tokio::spawn(async {
        match update(false).await {
            Ok(Some(version)) => debug!(%version, "self-update applied"),
            Ok(None) => debug!("already current"),
            Err(error) => debug!(%error, "self-update skipped"),
        }
    });
}

/// The foreground path behind `repotracer update`, which reports what happened.
pub async fn run_now() -> Result<()> {
    managed_binary().context(
        "self-update manages only the binary installed under ~/.repotracer/bin; \
         this one came from cargo, a source checkout, or npx",
    )?;
    match update(true).await? {
        Some(version) => {
            println!("Updated to {version}.");
            println!("Restart Codex to pick it up.");
        }
        None => println!("Already on {}, the newest release.", current_version()),
    }
    Ok(())
}

/// Check, download, verify, and swap. `force` ignores the once-a-day throttle.
/// Returns the version installed, or `None` if there was nothing to do.
async fn update(force: bool) -> Result<Option<String>> {
    let Some(staged) = stage(force).await? else {
        return Ok(None);
    };
    let version = staged.version.clone();
    commit(staged)?;
    Ok(Some(version))
}

/// Everything up to the swap: throttle, version check, download, verify.
async fn stage(force: bool) -> Result<Option<Staged>> {
    let Some(target) = managed_binary() else {
        return Ok(None);
    };
    let stamp = check_stamp(&target);
    if !force && !check_is_due(&stamp, CHECK_INTERVAL) {
        return Ok(None);
    }
    // Record the attempt, not the success: a release feed that is down must not
    // cause a fresh retry on every server start.
    record_check(&stamp);

    let latest = fetch_latest_version(&releases_api()).await?;
    if !is_newer(&latest, current_version()) {
        return Ok(None);
    }

    let asset = asset_name(std::env::consts::OS, std::env::consts::ARCH).with_context(|| {
        format!(
            "no release asset for {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let bytes = download_verified(&download_base(&latest), &asset).await?;

    // Stage beside the target so `self_replace` never crosses a filesystem.
    let path = staged_path(&target);
    write_executable(&path, &bytes)
        .with_context(|| format!("could not stage {}", path.display()))?;

    Ok(Some(Staged {
        version: latest,
        path,
    }))
}

/// Put the verified binary in place of the running one.
fn commit(staged: Staged) -> Result<()> {
    self_replace::self_replace(&staged.path).with_context(|| {
        format!(
            "could not replace the running binary with {}",
            staged.path.display()
        )
    })?;
    Ok(())
}

fn staged_path(target: &Path) -> PathBuf {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repotracer".into());
    parent.join(format!("{name}.new-{}", std::process::id()))
}

/// Write a downloaded binary somewhere it can be executed from.
pub fn write_executable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    set_executable_bit(path)
}

#[cfg(unix)]
fn set_executable_bit(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn set_executable_bit(_path: &Path) -> std::io::Result<()> {
    // Windows decides by extension, not by a mode bit.
    Ok(())
}

// ---------------------------------------------------------------------------
// What to download
// ---------------------------------------------------------------------------

/// The release asset for a target, named exactly as the release workflow
/// uploads it. `None` means we publish nothing for this platform.
pub fn asset_name(os: &str, arch: &str) -> Option<String> {
    let name = match (os, arch) {
        ("macos", "aarch64") => "repotracer-darwin-arm64",
        ("macos", "x86_64") => "repotracer-darwin-x64",
        ("linux", "aarch64") => "repotracer-linux-arm64",
        ("linux", "x86_64") => "repotracer-linux-x64",
        ("windows", "x86_64") => "repotracer-windows-x64.exe",
        _ => return None,
    };
    Some(name.to_string())
}

/// Pull one asset's digest out of a `sha256sum` listing.
///
/// The release job hashes files inside per-artifact directories, so the recorded
/// name is a path. Match on the basename, exactly as the npm installer does.
pub fn expected_checksum(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let digest = parts.next()?;
        let name = parts.next()?;
        let well_formed = digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit());
        // `*name` is sha256sum's binary-mode marker.
        let same_asset = Path::new(name.trim_start_matches('*')).file_name()? == asset;
        (well_formed && same_asset).then(|| digest.to_ascii_lowercase())
    })
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Compare dotted numeric versions. Anything unparsable sorts as older, so a
/// malformed tag can never trigger a download.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        v.trim()
            .trim_start_matches('v')
            .split('.')
            .map(|p| {
                p.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            })
            .collect()
    }
    if candidate.trim().is_empty() {
        return false;
    }
    let (a, b) = (parts(candidate), parts(current));
    for index in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(index).copied().unwrap_or(0),
            b.get(index).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Where we are allowed to write
// ---------------------------------------------------------------------------

/// The binary this process is allowed to replace, if any.
///
/// Only the copy setup installed under `~/.repotracer/bin` qualifies. Silently
/// overwriting a `cargo install` build or a developer's `target/release` output
/// would be an unpleasant surprise and would fight whatever manages them.
fn managed_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let expected = managed_path(&home()?);
    same_file(&exe, &expected).then_some(expected)
}

pub fn managed_path(home: &Path) -> PathBuf {
    let name = if cfg!(windows) {
        "repotracer.exe"
    } else {
        "repotracer"
    };
    home.join(".repotracer").join("bin").join(name)
}

fn check_stamp(target: &Path) -> PathBuf {
    // ~/.repotracer/bin/repotracer -> ~/.repotracer/update-check
    target
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("."))
        .join("update-check")
}

/// Compare through symlinks where possible; a `~/.repotracer/bin` entry can be a
/// link on a hand-managed install.
fn same_file(a: &Path, b: &Path) -> bool {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canonical(a) == canonical(b)
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Named after the convention every tool in this space follows: Deno has
/// `DENO_NO_UPDATE_CHECK`, rustup has `RUSTUP_UPDATE_ROOT`.
fn disabled_by_env() -> bool {
    matches!(
        std::env::var("REPOTRACER_NO_UPDATE").as_deref(),
        Ok("1") | Ok("true")
    )
}

// ---------------------------------------------------------------------------
// Throttling
// ---------------------------------------------------------------------------

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether enough time has passed since the last check. A missing or unreadable
/// stamp counts as due, so a fresh install always checks.
pub fn check_is_due(stamp: &Path, interval: Duration) -> bool {
    let last = std::fs::read_to_string(stamp)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .unwrap_or(0);
    now_unix().saturating_sub(last) >= interval.as_secs()
}

pub fn record_check(stamp: &Path) {
    if let Some(parent) = stamp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(stamp, now_unix().to_string());
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(concat!("repotracer/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// Both endpoints are overridable so the integration test can point at a local
/// server. `REPOTRACER_RELEASE_BASE_URL` is the same variable the npm installer
/// already honours.
fn releases_api() -> String {
    std::env::var("REPOTRACER_RELEASE_API").unwrap_or_else(|_| RELEASES_LATEST.to_string())
}

fn download_base(version: &str) -> String {
    std::env::var("REPOTRACER_RELEASE_BASE_URL")
        .unwrap_or_else(|_| format!("{DOWNLOAD_BASE}/v{version}"))
}

/// The newest published version, without its tag prefix.
async fn fetch_latest_version(api: &str) -> Result<String> {
    let body: serde_json::Value = http_client()?
        .get(api)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let tag = body
        .get("tag_name")
        .and_then(|t| t.as_str())
        .context("release feed had no tag_name")?;
    Ok(tag.trim_start_matches('v').to_string())
}

/// Download one release asset and refuse to return it unless its SHA-256
/// matches the digest published alongside it.
///
/// This is the only integrity check between a GitHub release and a binary the
/// user will execute, so it fails closed on anything unexpected: a missing sums
/// file, a missing line, or a digest that does not match.
async fn download_verified(base: &str, asset: &str) -> Result<Vec<u8>> {
    let client = http_client()?;
    let sums = get_text(&client, &format!("{base}/SHA256SUMS")).await?;
    let expected = expected_checksum(&sums, asset)
        .with_context(|| format!("{asset} is missing from SHA256SUMS"))?;

    let bytes = get_bytes(&client, &format!("{base}/{asset}")).await?;
    let actual = hex_digest(&bytes);
    if actual != expected {
        bail!("checksum mismatch for {asset}: expected {expected}, got {actual}");
    }
    Ok(bytes)
}

async fn get_text(client: &reqwest::Client, url: &str) -> Result<String> {
    Ok(client
        .get(url)
        .send()
        .await?
        .error_for_status()
        .with_context(|| format!("fetching {url}"))?
        .text()
        .await?)
}

async fn get_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await?
        .error_for_status()
        .with_context(|| format!("fetching {url}"))?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BINARY_BYTES)
    {
        bail!("{url} advertised more than {MAX_BINARY_BYTES} bytes");
    }
    let bytes = response.bytes().await?;
    if bytes.len() as u64 > MAX_BINARY_BYTES {
        bail!(
            "{url} returned {} bytes, past any plausible binary",
            bytes.len()
        );
    }
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_published_target_maps_to_its_release_asset() {
        assert_eq!(
            asset_name("macos", "aarch64").as_deref(),
            Some("repotracer-darwin-arm64")
        );
        assert_eq!(
            asset_name("macos", "x86_64").as_deref(),
            Some("repotracer-darwin-x64")
        );
        assert_eq!(
            asset_name("linux", "x86_64").as_deref(),
            Some("repotracer-linux-x64")
        );
        assert_eq!(
            asset_name("linux", "aarch64").as_deref(),
            Some("repotracer-linux-arm64")
        );
        // Only Windows carries an extension, and getting that wrong 404s.
        assert_eq!(
            asset_name("windows", "x86_64").as_deref(),
            Some("repotracer-windows-x64.exe")
        );
        assert_eq!(asset_name("freebsd", "x86_64"), None);
        assert_eq!(asset_name("windows", "aarch64"), None);
    }

    #[test]
    fn the_host_we_are_running_on_is_one_we_publish_for() {
        // Runs on all three CI platforms. A target we build but cannot name an
        // asset for would ship a binary that can never update itself.
        assert!(asset_name(std::env::consts::OS, std::env::consts::ARCH).is_some());
    }

    #[test]
    fn checksums_match_on_basename_because_the_release_records_paths() {
        let sums = "\
0000000000000000000000000000000000000000000000000000000000000000  ./SHA256SUMS
1111111111111111111111111111111111111111111111111111111111111111  ./repotracer-linux-x64/repotracer-linux-x64
2222222222222222222222222222222222222222222222222222222222222222  ./repotracer-windows-x64/repotracer-windows-x64.exe
3333333333333333333333333333333333333333333333333333333333333333 *repotracer-darwin-arm64
";
        assert_eq!(
            expected_checksum(sums, "repotracer-linux-x64").as_deref(),
            Some("1111111111111111111111111111111111111111111111111111111111111111")
        );
        assert_eq!(
            expected_checksum(sums, "repotracer-windows-x64.exe").as_deref(),
            Some("2222222222222222222222222222222222222222222222222222222222222222")
        );
        assert_eq!(
            expected_checksum(sums, "repotracer-darwin-arm64").as_deref(),
            Some("3333333333333333333333333333333333333333333333333333333333333333")
        );
        // A near-miss must not resolve: linux-x64 is not a match for arm64.
        assert_eq!(expected_checksum(sums, "repotracer-linux-arm64"), None);
    }

    #[test]
    fn a_malformed_sums_line_is_not_a_checksum() {
        assert_eq!(expected_checksum("garbage\n", "repotracer-linux-x64"), None);
        assert_eq!(
            expected_checksum("abc  repotracer-linux-x64\n", "repotracer-linux-x64"),
            None
        );
        assert_eq!(expected_checksum("", "repotracer-linux-x64"), None);
    }

    #[test]
    fn version_comparison_handles_the_shapes_we_publish() {
        assert!(is_newer("0.1.9", "0.1.3"));
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.3", "0.1.3"));
        assert!(!is_newer("0.1.2", "0.1.3"));
        // A malformed tag must never trigger a download.
        assert!(!is_newer("garbage", "0.1.3"));
        assert!(!is_newer("", "0.1.3"));
    }

    #[test]
    fn the_managed_path_is_the_one_codex_launches() {
        let home = Path::new("/home/someone");
        let path = managed_path(home);
        assert_eq!(
            path.file_name().unwrap(),
            if cfg!(windows) {
                "repotracer.exe"
            } else {
                "repotracer"
            }
        );
        assert!(path.starts_with(home.join(".repotracer").join("bin")));
    }

    #[test]
    fn the_stamp_sits_beside_the_config_not_inside_bin() {
        let stamp = check_stamp(&managed_path(Path::new("/home/someone")));
        assert_eq!(stamp, Path::new("/home/someone/.repotracer/update-check"));
    }

    #[test]
    fn the_staged_file_is_a_sibling_of_the_target() {
        let target = managed_path(Path::new("/home/someone"));
        let staged = staged_path(&target);
        assert_eq!(staged.parent(), target.parent());
        assert!(staged
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(".new-"));
    }

    #[test]
    fn a_staged_file_is_cleaned_up_when_it_is_never_committed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repotracer.new-1");
        write_executable(&path, b"downloaded").unwrap();
        assert!(path.exists());

        drop(Staged {
            version: "0.9.9".into(),
            path: path.clone(),
        });

        assert!(!path.exists(), "a failed update left debris in bin/");
    }

    #[test]
    fn a_downloaded_binary_lands_runnable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("repotracer");
        write_executable(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"#!/bin/sh\nexit 0\n");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "mode was {mode:o}");
        }
    }

    #[test]
    fn the_check_is_due_once_a_day_and_not_before() {
        let dir = tempfile::tempdir().unwrap();
        let stamp = dir.path().join("update-check");

        // Never checked: must check now, or a fresh install never updates.
        assert!(check_is_due(&stamp, CHECK_INTERVAL));

        record_check(&stamp);
        assert!(!check_is_due(&stamp, CHECK_INTERVAL));
        // A zero interval always fires.
        assert!(check_is_due(&stamp, Duration::from_secs(0)));

        // An unreadable stamp must fail open rather than freeze updates forever.
        std::fs::write(&stamp, "not a timestamp").unwrap();
        assert!(check_is_due(&stamp, CHECK_INTERVAL));
    }

    #[test]
    fn digests_are_lowercase_hex() {
        // Known vector: sha256 of the empty input.
        assert_eq!(
            hex_digest(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn the_download_url_puts_the_tag_prefix_back() {
        // The release feed reports `v0.1.9` and we strip the `v` to compare
        // versions, but the download path needs it again.
        assert!(download_base("0.1.9").ends_with("/releases/download/v0.1.9"));
    }
}

/// A stand-in for the GitHub release endpoints, so the download-and-verify path
/// is exercised for real on every platform CI builds for rather than mocked.
///
/// Hand-rolled rather than pulled from a crate: it answers three fixed paths and
/// speaks just enough HTTP/1.1 to satisfy reqwest, which is not worth a
/// dependency that only test builds would use.
#[cfg(test)]
mod fake_release {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};

    pub struct Server {
        pub base: String,
    }

    impl Server {
        /// Serve `routes` as (path, body) pairs until the test process exits.
        pub fn start(routes: Vec<(String, Vec<u8>)>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
            let base = format!("http://{}", listener.local_addr().unwrap());
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let _ = handle(stream, &routes);
                }
            });
            Server { base }
        }
    }

    fn handle(mut stream: TcpStream, routes: &[(String, Vec<u8>)]) -> std::io::Result<()> {
        let mut request = String::new();
        BufReader::new(stream.try_clone()?).read_line(&mut request)?;
        let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();

        let body = routes
            .iter()
            .find(|(route, _)| *route == path)
            .map(|(_, body)| body.clone());
        let (status, body) = match body {
            Some(body) => ("200 OK", body),
            None => ("404 Not Found", b"missing".to_vec()),
        };
        // Connection: close keeps reqwest from holding the socket open and
        // stalling a single-threaded test runtime.
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )?;
        stream.write_all(&body)?;
        stream.flush()
    }
}

#[cfg(test)]
mod network_tests {
    use super::fake_release::Server;
    use super::*;

    const BINARY: &[u8] = b"\x7fELF pretend this is a real binary";

    fn sums_for(asset: &str, bytes: &[u8]) -> Vec<u8> {
        // Same shape the release workflow produces: digest, two spaces, a path
        // inside the per-artifact directory.
        format!("{}  ./{asset}/{asset}\n", hex_digest(bytes)).into_bytes()
    }

    fn release_server(asset: &str, sums: Vec<u8>, binary: &[u8]) -> Server {
        Server::start(vec![
            (
                "/releases/latest".into(),
                br#"{"tag_name": "v99.0.0"}"#.to_vec(),
            ),
            ("/SHA256SUMS".into(), sums),
            (format!("/{asset}"), binary.to_vec()),
        ])
    }

    #[tokio::test]
    async fn the_release_feed_reports_the_version_without_its_tag_prefix() {
        let server = release_server("x", vec![], b"");
        let version = fetch_latest_version(&format!("{}/releases/latest", server.base))
            .await
            .unwrap();
        assert_eq!(version, "99.0.0");
    }

    #[tokio::test]
    async fn a_missing_release_feed_is_an_error_and_not_a_phantom_update() {
        let server = release_server("x", vec![], b"");
        let error = fetch_latest_version(&format!("{}/nope", server.base))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("404"), "{error}");
    }

    /// The whole path a real update takes, on whatever platform this runs.
    #[tokio::test]
    async fn a_matching_asset_downloads_and_lands_runnable() {
        let asset = asset_name(std::env::consts::OS, std::env::consts::ARCH).unwrap();
        let server = release_server(&asset, sums_for(&asset, BINARY), BINARY);

        let bytes = download_verified(&server.base, &asset).await.unwrap();
        assert_eq!(bytes, BINARY);

        let dir = tempfile::tempdir().unwrap();
        let target = managed_path(dir.path());
        let staged = staged_path(&target);
        write_executable(&staged, &bytes).unwrap();

        assert_eq!(std::fs::read(&staged).unwrap(), BINARY);
        assert_eq!(staged.parent(), target.parent());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&staged).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "staged binary was not executable");
        }
    }

    #[tokio::test]
    async fn a_tampered_binary_is_refused() {
        let asset = asset_name(std::env::consts::OS, std::env::consts::ARCH).unwrap();
        // Digests describe the real binary; the server serves something else.
        let server = release_server(&asset, sums_for(&asset, BINARY), b"malicious payload");

        let error = download_verified(&server.base, &asset).await.unwrap_err();
        assert!(error.to_string().contains("checksum mismatch"), "{error}");
    }

    #[tokio::test]
    async fn an_asset_with_no_published_digest_is_refused() {
        let asset = asset_name(std::env::consts::OS, std::env::consts::ARCH).unwrap();
        let server = release_server(&asset, sums_for("some-other-asset", BINARY), BINARY);

        let error = download_verified(&server.base, &asset).await.unwrap_err();
        assert!(
            error.to_string().contains("missing from SHA256SUMS"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_release_with_no_sums_file_at_all_is_refused() {
        let asset = asset_name(std::env::consts::OS, std::env::consts::ARCH).unwrap();
        let server = Server::start(vec![(format!("/{asset}"), BINARY.to_vec())]);

        let error = download_verified(&server.base, &asset).await.unwrap_err();
        assert!(error.to_string().contains("SHA256SUMS"), "{error}");
    }

    #[tokio::test]
    async fn a_missing_asset_is_an_error_rather_than_an_empty_binary() {
        let asset = asset_name(std::env::consts::OS, std::env::consts::ARCH).unwrap();
        let server = Server::start(vec![("/SHA256SUMS".into(), sums_for(&asset, BINARY))]);

        let error = download_verified(&server.base, &asset).await.unwrap_err();
        assert!(error.to_string().contains(&asset), "{error}");
    }
}
