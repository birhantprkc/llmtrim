//! Managed [CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI) sidecar.
//!
//! `sub on` installs and starts this process. The interceptor then rewrites Anthropic
//! `/v1/messages` to the sidecar (which already speaks the Claude API) instead of doing
//! first-party Codex/Kimi/Grok protocol translation.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use super::SubProvider;
use super::UpstreamRewrite;

pub const REPO: &str = "router-for-me/CLIProxyAPI";
pub const DEFAULT_PORT: u16 = 18317;
const DEFAULT_HOST: &str = "127.0.0.1";

/// Override the sidecar base URL (`http://127.0.0.1:8317`). When set, llmtrim does not
/// manage a private binary — it just redirects to this instance.
pub const URL_ENV: &str = "LLMTRIM_CLIPROXY_URL";
/// API key for [`URL_ENV`] (or for the managed sidecar when you want to pin one).
pub const KEY_ENV: &str = "LLMTRIM_CLIPROXY_KEY";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    pub owned_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub enabled: bool,
    pub installed: bool,
    pub running: bool,
    pub managed: bool,
    pub version: Option<String>,
    pub base_url: String,
}

pub fn dir() -> Result<PathBuf> {
    Ok(crate::daemon::home_dir()?.join("cliproxy"))
}

pub fn bin_path() -> Result<PathBuf> {
    let name = if cfg!(windows) {
        "CLIProxyAPI.exe"
    } else {
        "CLIProxyAPI"
    };
    Ok(dir()?.join(name))
}

pub fn config_path() -> Result<PathBuf> {
    Ok(dir()?.join("config.yaml"))
}

pub fn version_path() -> Result<PathBuf> {
    Ok(dir()?.join("version"))
}

fn pidfile() -> Result<PathBuf> {
    Ok(crate::daemon::home_dir()?.join("cliproxy.pid"))
}

fn logfile() -> Result<PathBuf> {
    Ok(crate::daemon::home_dir()?.join("cliproxy.log"))
}

fn key_path() -> Result<PathBuf> {
    Ok(dir()?.join("api-key"))
}

pub fn is_installed() -> bool {
    bin_path().ok().is_some_and(|p| p.is_file())
}

pub fn is_managed_user() -> bool {
    is_installed() || is_enabled()
}

pub fn is_enabled() -> bool {
    llmtrim_core::config::sub_always_on()
}

pub fn installed_version() -> Option<String> {
    fs::read_to_string(version_path().ok()?).ok().map(|s| {
        s.trim()
            .trim_start_matches('v')
            .trim()
            .to_string()
    })
}

/// Release asset name for this OS/arch (`None` if we do not ship that target).
pub fn release_asset(version: &str) -> Option<String> {
    release_asset_for(version, std::env::consts::OS, std::env::consts::ARCH)
}

pub fn release_asset_for(version: &str, os: &str, arch: &str) -> Option<String> {
    let ver = version.trim_start_matches('v');
    let (os, arch, ext) = match (os, arch) {
        ("linux", "x86_64") => ("linux", "amd64", "tar.gz"),
        ("linux", "aarch64") => ("linux", "aarch64", "tar.gz"),
        ("macos", "x86_64") => ("darwin", "amd64", "tar.gz"),
        ("macos", "aarch64") => ("darwin", "aarch64", "tar.gz"),
        ("windows", "x86_64") => ("windows", "amd64", "zip"),
        ("windows", "aarch64") => ("windows", "aarch64", "zip"),
        _ => return None,
    };
    Some(format!("CLIProxyAPI_{ver}_{os}_{arch}.{ext}"))
}

pub fn default_base_url() -> String {
    format!("http://{DEFAULT_HOST}:{DEFAULT_PORT}")
}

pub fn base_url() -> String {
    std::env::var(URL_ENV)
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(default_base_url)
}

pub fn is_externally_configured() -> bool {
    std::env::var(URL_ENV)
        .ok()
        .is_some_and(|s| !s.trim().is_empty())
}

pub fn api_key() -> Result<String> {
    if let Ok(k) = std::env::var(KEY_ENV) {
        let k = k.trim().to_string();
        if !k.is_empty() {
            return Ok(k);
        }
    }
    let path = key_path()?;
    if let Ok(existing) = fs::read_to_string(&path) {
        let k = existing.trim().to_string();
        if !k.is_empty() {
            return Ok(k);
        }
    }
    let key = format!("llmtrim-{}", uuid::Uuid::new_v4().simple());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, &key)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(key)
}

pub fn auth_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        let shared = PathBuf::from(home).join(".cli-proxy-api");
        if shared.is_dir() {
            return shared;
        }
    }
    dir().map(|d| d.join("auth")).unwrap_or_else(|_| PathBuf::from("auth"))
}

pub fn config_yaml(port: u16, key: &str, auth: &Path) -> String {
    format!(
        "host: \"{DEFAULT_HOST}\"\n\
         port: {port}\n\
         auth-dir: \"{}\"\n\
         api-keys:\n\
           - \"{key}\"\n\
         remote-management:\n\
           allow-remote: false\n\
           secret-key: \"\"\n\
           disable-control-panel: true\n\
         debug: false\n",
        auth.display()
    )
}

pub fn ensure_config() -> Result<()> {
    if is_externally_configured() {
        return Ok(());
    }
    let dir = dir()?;
    fs::create_dir_all(dir.join("auth")).with_context(|| format!("create {}", dir.display()))?;
    let path = config_path()?;
    if path.is_file() {
        return Ok(());
    }
    let yaml = config_yaml(DEFAULT_PORT, &api_key()?, &auth_dir());
    fs::write(&path, yaml).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn pid_running() -> Option<u32> {
    let raw = fs::read_to_string(pidfile().ok()?).ok()?;
    let pid: u32 = raw.trim().parse().ok()?;
    if process_alive(pid) {
        Some(pid)
    } else {
        let _ = fs::remove_file(pidfile().ok()?);
        None
    }
}

fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .ok()
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

pub fn is_healthy() -> bool {
    probe_models().is_ok()
}

pub fn is_running() -> bool {
    is_healthy() || pid_running().is_some()
}

fn ureq_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .http_status_as_error(false)
        .build()
        .into()
}

fn models_url() -> String {
    format!("{}/v1/models", base_url())
}

fn probe_models() -> Result<Value> {
    let key = api_key().unwrap_or_default();
    let mut req = ureq_agent().get(models_url());
    if !key.is_empty() {
        req = req
            .header("Authorization", format!("Bearer {key}"))
            .header("x-api-key", &key);
    }
    let mut res = req.call().context("CLIProxyAPI /v1/models")?;
    let status = res.status();
    let body = res.body_mut().read_to_string().unwrap_or_default();
    if !status.is_success() {
        bail!("CLIProxyAPI /v1/models returned {status}: {body}");
    }
    serde_json::from_str(&body).context("parse CLIProxyAPI /v1/models")
}

pub fn parse_models(value: &Value) -> Vec<Model> {
    let Some(arr) = value.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        out.push(Model {
            id: id.to_string(),
            owned_by: item
                .get("owned_by")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

pub fn list_models() -> Result<Vec<Model>> {
    Ok(parse_models(&probe_models()?))
}

pub fn status() -> Status {
    Status {
        enabled: is_enabled(),
        installed: is_installed(),
        running: is_running(),
        managed: !is_externally_configured(),
        version: installed_version(),
        base_url: base_url(),
    }
}

/// Build the MITM rewrite that sends the (already Anthropic-shaped) body to CLIProxyAPI.
pub fn rewrite(anthropic_body: &Value) -> Result<UpstreamRewrite> {
    if !is_running() {
        bail!("CLIProxyAPI is not running — run `llmtrim sub on`");
    }
    let model = anthropic_body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let key = api_key()?;
    let url = base_url();
    let (host, path) = split_base_url(&url)?;
    Ok(UpstreamRewrite {
        host,
        path: format!("{path}/v1/messages"),
        headers: vec![
            ("authorization".into(), format!("Bearer {key}")),
            ("x-api-key".into(), key),
            ("content-type".into(), "application/json".into()),
        ],
        body: serde_json::to_vec(anthropic_body)?,
        model,
        provider: SubProvider::CliProxy,
        insecure_http: url.starts_with("http://"),
    })
}

pub fn split_base_url(url: &str) -> Result<(String, String)> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .context("CLIProxyAPI URL must be http(s)://host[:port]")?;
    let (host, prefix) = rest.split_once('/').unwrap_or((rest, ""));
    if host.is_empty() {
        bail!("CLIProxyAPI URL has no host");
    }
    let prefix = prefix.trim_end_matches('/');
    let prefix = if prefix.is_empty() {
        String::new()
    } else {
        format!("/{prefix}")
    };
    Ok((host.to_string(), prefix))
}

pub fn fetch_latest_tag() -> Result<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut req = ureq::get(&url)
        .config()
        .timeout_global(Some(Duration::from_secs(8)))
        .http_status_as_error(false)
        .build();
    req = req.header("User-Agent", "llmtrim-cliproxy");
    let body = req
        .call()
        .context("CLIProxyAPI releases")?
        .body_mut()
        .read_to_string()
        .context("read CLIProxyAPI release")?;
    let v: Value = serde_json::from_str(&body).context("parse CLIProxyAPI release")?;
    let tag = v
        .get("tag_name")
        .and_then(Value::as_str)
        .context("CLIProxyAPI release has no tag_name")?;
    Ok(tag.trim_start_matches('v').to_string())
}

pub fn ensure_installed() -> Result<String> {
    if is_externally_configured() {
        return Ok("external".into());
    }
    if is_installed() {
        return Ok(installed_version().unwrap_or_else(|| "unknown".into()));
    }
    install_latest()
}

pub fn install_latest() -> Result<String> {
    if is_externally_configured() {
        bail!("LLMTRIM_CLIPROXY_URL is set — I will not replace an external CLIProxyAPI");
    }
    let tag = fetch_latest_tag()?;
    install_tag(&tag)?;
    Ok(tag)
}

pub fn install_tag(tag: &str) -> Result<()> {
    let asset = release_asset(tag)
        .with_context(|| format!("no CLIProxyAPI build for {}/{}", std::env::consts::OS, std::env::consts::ARCH))?;
    let url = format!("https://github.com/{REPO}/releases/download/v{tag}/{asset}");
    let dest = dir()?;
    fs::create_dir_all(&dest)?;
    let archive = dest.join(&asset);
    download(&url, &archive)?;
    extract(&archive, &dest)?;
    let _ = fs::remove_file(&archive);
    locate_binary(&dest)?;
    fs::write(version_path()?, tag)?;
    ensure_config()?;
    Ok(())
}

fn download(url: &str, dest: &Path) -> Result<()> {
    let mut req = ureq::get(url)
        .config()
        .timeout_global(Some(Duration::from_secs(120)))
        .http_status_as_error(true)
        .build();
    req = req.header("User-Agent", "llmtrim-cliproxy");
    let mut res = req.call().with_context(|| format!("download {url}"))?;
    let bytes = res
        .body_mut()
        .read_to_vec()
        .context("read CLIProxyAPI archive")?;
    fs::write(dest, bytes).with_context(|| format!("write {}", dest.display()))?;
    Ok(())
}

fn extract(archive: &Path, dest: &Path) -> Result<()> {
    let name = archive.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if name.ends_with(".zip") {
        let status = if cfg!(windows) {
            std::process::Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-Command",
                    &format!(
                        "Expand-Archive -Force -Path '{}' -DestinationPath '{}'",
                        archive.display(),
                        dest.display()
                    ),
                ])
                .status()
        } else {
            std::process::Command::new("unzip")
                .args(["-o", &archive.to_string_lossy(), "-d", &dest.to_string_lossy()])
                .status()
        }
        .context("extract CLIProxyAPI zip")?;
        if !status.success() {
            bail!("failed to extract {}", archive.display());
        }
        return Ok(());
    }
    let status = std::process::Command::new("tar")
        .args([
            "-xzf",
            &archive.to_string_lossy(),
            "-C",
            &dest.to_string_lossy(),
        ])
        .status()
        .context("extract CLIProxyAPI tar.gz (tar required)")?;
    if !status.success() {
        bail!("failed to extract {}", archive.display());
    }
    Ok(())
}

fn locate_binary(dest: &Path) -> Result<PathBuf> {
    let expected = bin_path()?;
    if expected.is_file() {
        chmod_exec(&expected);
        return Ok(expected);
    }
    for entry in walkdir_bins(dest) {
        let Some(name) = entry.file_name() else {
            continue;
        };
        let name = name.to_string_lossy();
        if name == "CLIProxyAPI" || name == "CLIProxyAPI.exe" || name == "cli-proxy-api" {
            if entry != expected {
                let _ = fs::copy(&entry, &expected);
            }
            chmod_exec(&expected);
            return Ok(expected);
        }
    }
    bail!("CLIProxyAPI binary not found in {}", dest.display());
}

fn walkdir_bins(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(root) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walkdir_bins(&path));
        } else {
            out.push(path);
        }
    }
    out
}

fn chmod_exec(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(path) {
            let mut mode = meta.permissions().mode();
            mode |= 0o755;
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
        }
    }
    let _ = path;
}

pub fn ensure_running() -> Result<()> {
    if is_externally_configured() {
        if is_healthy() {
            return Ok(());
        }
        bail!(
            "CLIProxyAPI at {} is not reachable — start it, or unset {URL_ENV}",
            base_url()
        );
    }
    ensure_installed()?;
    ensure_config()?;
    if is_healthy() {
        return Ok(());
    }
    if pid_running().is_some() {
        // Process up but not healthy yet — give it a moment.
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(150));
            if is_healthy() {
                return Ok(());
            }
        }
    }
    start()?;
    for _ in 0..40 {
        if is_healthy() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    bail!(
        "CLIProxyAPI started but {}/v1/models is not answering — see {}",
        base_url(),
        logfile().map(|p| p.display().to_string()).unwrap_or_else(|_| "cliproxy.log".into())
    );
}

pub fn start() -> Result<u32> {
    if is_externally_configured() {
        bail!("LLMTRIM_CLIPROXY_URL is set — start that instance yourself");
    }
    if let Some(pid) = pid_running() {
        return Ok(pid);
    }
    ensure_installed()?;
    ensure_config()?;
    let bin = bin_path()?;
    let cfg = config_path()?;
    let log = fs::File::create(logfile()?)?;
    let err = log.try_clone()?;
    let mut cmd = std::process::Command::new(&bin);
    cmd.args(["--config", &cfg.to_string_lossy()])
        .stdin(std::process::Stdio::null())
        .stdout(log)
        .stderr(err);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    let child = cmd.spawn().with_context(|| format!("spawn {}", bin.display()))?;
    let pid = child.id();
    fs::write(pidfile()?, pid.to_string())?;
    Ok(pid)
}

pub fn stop() -> Result<Option<u32>> {
    if is_externally_configured() {
        return Ok(None);
    }
    let Some(pid) = pid_running() else {
        return Ok(None);
    };
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args([pid.to_string()])
            .status();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status();
    }
    for _ in 0..30 {
        if !process_alive(pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = fs::remove_file(pidfile()?);
    Ok(Some(pid))
}

/// Launch the CLIProxyAPI TUI so the user can sign in to providers.
pub fn auth_tui() -> Result<()> {
    ensure_installed()?;
    ensure_config()?;
    let bin = bin_path()?;
    let cfg = config_path()?;
    let status = std::process::Command::new(&bin)
        .args(["--tui", "--config", &cfg.to_string_lossy()])
        .status()
        .with_context(|| format!("run {} --tui", bin.display()))?;
    if !status.success() {
        bail!("CLIProxyAPI TUI exited non-zero");
    }
    Ok(())
}

pub fn update_if_used() -> Result<Option<String>> {
    if !is_managed_user() || is_externally_configured() {
        return Ok(None);
    }
    if !is_installed() {
        if !is_enabled() {
            return Ok(None);
        }
        return ensure_for_existing_user().map(Some);
    }
    let latest = fetch_latest_tag()?;
    if installed_version().as_deref() == Some(latest.as_str()) {
        let imported = migrate_legacy_tokens()?;
        if is_enabled() {
            let _ = ensure_running();
        }
        if imported.is_empty() {
            return Ok(Some(format!("CLIProxyAPI already {latest}")));
        }
        return Ok(Some(format!(
            "CLIProxyAPI already {latest}; imported {}",
            imported.join(", ")
        )));
    }
    let was_running = pid_running().is_some() || is_enabled();
    if pid_running().is_some() {
        let _ = stop();
    }
    install_tag(&latest)?;
    let imported = migrate_legacy_tokens()?;
    if was_running {
        let _ = ensure_running();
    }
    let mut msg = format!("CLIProxyAPI updated to {latest}");
    if !imported.is_empty() {
        msg.push_str(&format!("; imported {}", imported.join(", ")));
    }
    Ok(Some(msg))
}

/// Install, import existing `sub` tokens, and start the sidecar. Used by `ensure` / `update`
/// so a user who already had `sub = codex|kimi|grok` needs no extra command.
pub fn ensure_for_existing_user() -> Result<String> {
    if is_externally_configured() {
        if is_healthy() {
            return Ok(format!("CLIProxyAPI at {} reachable", base_url()));
        }
        bail!("CLIProxyAPI at {} is not reachable", base_url());
    }
    ensure_installed()?;
    ensure_config()?;
    let imported = migrate_legacy_tokens()?;
    ensure_running()?;
    if imported.is_empty() {
        Ok(format!("CLIProxyAPI ready at {}", base_url()))
    } else {
        Ok(format!(
            "CLIProxyAPI ready at {}; imported {}",
            base_url(),
            imported.join(", ")
        ))
    }
}

/// Copy first-party `~/.llmtrim/{{codex,kimi,grok}}/auth.json` into the CLIProxyAPI auth dir
/// once. Existing sidecar files are left alone.
pub fn migrate_legacy_tokens() -> Result<Vec<String>> {
    let dest = auth_dir();
    fs::create_dir_all(&dest)?;
    let home = crate::daemon::home_dir()?;
    let mut imported = Vec::new();
    if import_one(
        &home.join("codex").join("auth.json"),
        &dest.join("codex-llmtrim.json"),
        convert_codex_auth,
    )? {
        imported.push("codex".into());
    }
    if import_one(
        &home.join("kimi").join("auth.json"),
        &dest.join("kimi-llmtrim.json"),
        convert_kimi_auth,
    )? {
        imported.push("kimi".into());
    }
    if import_one(
        &home.join("grok").join("auth.json"),
        &dest.join("xai-llmtrim.json"),
        convert_grok_auth,
    )? {
        imported.push("grok".into());
    }
    Ok(imported)
}

fn import_one(
    src: &Path,
    dest: &Path,
    convert: fn(&Value) -> Option<Value>,
) -> Result<bool> {
    if dest.is_file() || !src.is_file() {
        return Ok(false);
    }
    let raw = fs::read_to_string(src)
        .with_context(|| format!("read {}", src.display()))?;
    let src_val: Value = serde_json::from_str(&raw)
        .with_context(|| format!("parse {}", src.display()))?;
    let Some(out) = convert(&src_val) else {
        return Ok(false);
    };
    let bytes = serde_json::to_vec_pretty(&out)?;
    fs::write(dest, bytes).with_context(|| format!("write {}", dest.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dest, fs::Permissions::from_mode(0o600));
    }
    Ok(true)
}

fn epoch_ms_to_rfc3339(ms: u64) -> String {
    use chrono::{TimeZone, Utc};
    Utc.timestamp_millis_opt(ms as i64)
        .single()
        .unwrap_or_else(Utc::now)
        .to_rfc3339()
}

fn json_str(v: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = v.get(*k).and_then(Value::as_str).filter(|s| !s.is_empty()) {
            return Some(s.to_string());
        }
    }
    None
}

fn json_expires_ms(v: &Value) -> Option<u64> {
    v.get("expires")
        .and_then(Value::as_u64)
        .or_else(|| v.get("expires").and_then(Value::as_i64).map(|n| n as u64))
}

pub(crate) fn convert_codex_auth(src: &Value) -> Option<Value> {
    let access = json_str(src, &["access", "access_token"])?;
    let refresh = json_str(src, &["refresh", "refresh_token"])?;
    let account = json_str(src, &["accountId", "account_id"]).unwrap_or_default();
    let expired = json_expires_ms(src)
        .map(epoch_ms_to_rfc3339)
        .unwrap_or_default();
    Some(serde_json::json!({
        "type": "codex",
        "access_token": access,
        "refresh_token": refresh,
        "id_token": json_str(src, &["id_token"]).unwrap_or_default(),
        "account_id": account,
        "email": "llmtrim-migrated",
        "last_refresh": chrono::Utc::now().to_rfc3339(),
        "expired": expired,
    }))
}

pub(crate) fn convert_kimi_auth(src: &Value) -> Option<Value> {
    let access = json_str(src, &["access", "access_token"])?;
    let refresh = json_str(src, &["refresh", "refresh_token"])?;
    let expired = json_expires_ms(src)
        .map(epoch_ms_to_rfc3339)
        .unwrap_or_default();
    Some(serde_json::json!({
        "type": "kimi",
        "access_token": access,
        "refresh_token": refresh,
        "token_type": "Bearer",
        "scope": json_str(src, &["scope"]).unwrap_or_default(),
        "device_id": json_str(src, &["device_id", "deviceId", "userId"]).unwrap_or_default(),
        "expired": expired,
    }))
}

pub(crate) fn convert_grok_auth(src: &Value) -> Option<Value> {
    let access = json_str(src, &["access", "access_token"])?;
    let refresh = json_str(src, &["refresh", "refresh_token"])?;
    let expired = json_expires_ms(src)
        .map(epoch_ms_to_rfc3339)
        .unwrap_or_default();
    Some(serde_json::json!({
        "type": "xai",
        "auth_kind": "oauth",
        "access_token": access,
        "refresh_token": refresh,
        "id_token": json_str(src, &["id_token"]).unwrap_or_default(),
        "token_type": "Bearer",
        "expired": expired,
        "last_refresh": chrono::Utc::now().to_rfc3339(),
        "email": "llmtrim-migrated",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn asset_name_linux_amd64() {
        assert_eq!(
            release_asset_for("7.2.130", "linux", "x86_64").as_deref(),
            Some("CLIProxyAPI_7.2.130_linux_amd64.tar.gz")
        );
    }

    #[test]
    fn asset_name_darwin_arm() {
        assert_eq!(
            release_asset_for("v7.2.130", "macos", "aarch64").as_deref(),
            Some("CLIProxyAPI_7.2.130_darwin_aarch64.tar.gz")
        );
    }

    #[test]
    fn asset_name_windows_zip() {
        assert_eq!(
            release_asset_for("7.2.130", "windows", "x86_64").as_deref(),
            Some("CLIProxyAPI_7.2.130_windows_amd64.zip")
        );
    }

    #[test]
    fn asset_name_unknown_none() {
        assert_eq!(release_asset_for("7.2.130", "linux", "riscv64"), None);
    }

    #[test]
    fn split_url_strips_scheme_and_prefix() {
        assert_eq!(
            split_base_url("http://127.0.0.1:18317").unwrap(),
            ("127.0.0.1:18317".into(), String::new())
        );
        assert_eq!(
            split_base_url("http://127.0.0.1:8317/proxy").unwrap(),
            ("127.0.0.1:8317".into(), "/proxy".into())
        );
    }

    #[test]
    fn config_yaml_is_localhost_only() {
        let yaml = config_yaml(18317, "llmtrim-test", Path::new("/tmp/auth"));
        assert!(yaml.contains("host: \"127.0.0.1\""));
        assert!(yaml.contains("port: 18317"));
        assert!(yaml.contains("llmtrim-test"));
        assert!(yaml.contains("allow-remote: false"));
    }

    #[test]
    fn parse_models_reads_openai_list() {
        let v = json!({
            "data": [
                {"id": "gpt-5.4", "owned_by": "openai"},
                {"id": "gemini-3-flash", "owned_by": "google"},
                {"id": "gpt-5.4", "owned_by": "dup"},
                {"owned_by": "skip-me"}
            ]
        });
        let models = parse_models(&v);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "gemini-3-flash");
        assert_eq!(models[1].id, "gpt-5.4");
        assert_eq!(models[1].owned_by, "openai");
    }

    #[test]
    fn convert_codex_maps_llmtrim_auth_json() {
        let src = json!({
            "access": "at-1",
            "refresh": "rt-1",
            "expires": 1_700_000_000_000u64,
            "accountId": "acct-9"
        });
        let out = convert_codex_auth(&src).unwrap();
        assert_eq!(out["type"], "codex");
        assert_eq!(out["access_token"], "at-1");
        assert_eq!(out["refresh_token"], "rt-1");
        assert_eq!(out["account_id"], "acct-9");
        assert!(out["expired"].as_str().unwrap().contains("2023"));
    }

    #[test]
    fn convert_kimi_and_grok_require_refresh() {
        assert!(convert_kimi_auth(&json!({"access": "a"})).is_none());
        let kimi = convert_kimi_auth(&json!({
            "access": "a",
            "refresh": "r",
            "expires": 1_700_000_000_000u64,
            "userId": "u1"
        }))
        .unwrap();
        assert_eq!(kimi["type"], "kimi");
        assert_eq!(kimi["device_id"], "u1");
        let grok = convert_grok_auth(&json!({
            "access": "a",
            "refresh": "r",
            "expires": 1_700_000_000_000u64
        }))
        .unwrap();
        assert_eq!(grok["type"], "xai");
        assert_eq!(grok["auth_kind"], "oauth");
    }
}
