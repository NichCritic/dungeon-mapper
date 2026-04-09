use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use std::sync::mpsc;

const UPDATE_PUBLIC_KEY_HEX: &str = env!("UPDATE_PUBLIC_KEY");
const GITHUB_REPO: &str = "NichCritic/dungeon-mapper";

pub struct UpdateInfo {
    pub version: String,
    pub download_url: String,
    pub sig_url: String,
    pub release_notes: String,
}

pub enum UpdateStatus {
    NoUpdate,
    Available(UpdateInfo),
    Error(String),
}

pub enum ApplyStatus {
    Success,
    Error(String),
}

fn is_dummy_key() -> bool {
    UPDATE_PUBLIC_KEY_HEX.chars().all(|c| c == '0')
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("Odd-length hex string".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn current_version() -> semver::Version {
    env!("CARGO_PKG_VERSION").parse().unwrap()
}

/// Extract semver from a tag like "v0.2.0-abc1234".
fn parse_tag_version(tag: &str) -> Option<semver::Version> {
    let s = tag.strip_prefix('v').unwrap_or(tag);
    // Try parsing the whole string first
    if let Ok(v) = s.parse::<semver::Version>() {
        return Some(v);
    }
    // Strip trailing -HEXSHA (7+ hex chars after the last dash)
    if let Some(dash_pos) = s.rfind('-') {
        let suffix = &s[dash_pos + 1..];
        if suffix.len() >= 7 && suffix.chars().all(|c| c.is_ascii_hexdigit()) {
            if let Ok(v) = s[..dash_pos].parse::<semver::Version>() {
                return Some(v);
            }
        }
    }
    None
}

fn binary_asset_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "dungeon-mapper.exe"
    } else {
        "dungeon-mapper"
    }
}

#[derive(serde::Deserialize)]
struct GhRelease {
    tag_name: String,
    body: Option<String>,
    assets: Vec<GhAsset>,
}

#[derive(serde::Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

fn check_for_update_blocking() -> Result<UpdateStatus, String> {
    let current = current_version();
    let url = format!("https://api.github.com/repos/{}/releases/latest", GITHUB_REPO);

    let client = reqwest::blocking::Client::builder()
        .user_agent("dungeon-mapper-updater")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client.get(&url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("GitHub API returned {}", resp.status()));
    }

    let release: GhRelease = resp.json().map_err(|e| e.to_string())?;

    let remote_version = match parse_tag_version(&release.tag_name) {
        Some(v) => v,
        None => return Err(format!("Cannot parse version from tag: {}", release.tag_name)),
    };

    if remote_version <= current {
        return Ok(UpdateStatus::NoUpdate);
    }

    let bin_name = binary_asset_name();
    let sig_name = format!("{}.sig", bin_name);

    let download_url = release.assets.iter()
        .find(|a| a.name == bin_name)
        .map(|a| a.browser_download_url.clone())
        .ok_or_else(|| format!("No {} asset in release", bin_name))?;

    let sig_url = release.assets.iter()
        .find(|a| a.name == sig_name)
        .map(|a| a.browser_download_url.clone())
        .ok_or_else(|| format!("No {} asset in release (unsigned release?)", sig_name))?;

    Ok(UpdateStatus::Available(UpdateInfo {
        version: remote_version.to_string(),
        download_url,
        sig_url,
        release_notes: release.body.unwrap_or_default(),
    }))
}

/// Spawn a background thread to check for updates. Returns immediately.
pub fn check_for_update() -> Option<mpsc::Receiver<UpdateStatus>> {
    if is_dummy_key() {
        return None; // Dev build, skip update check
    }

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let status = match check_for_update_blocking() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Update check failed: {}", e);
                UpdateStatus::Error(e)
            }
        };
        let _ = tx.send(status);
    });
    Some(rx)
}

fn download_and_apply_blocking(download_url: &str, sig_url: &str) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("dungeon-mapper-updater")
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    // Download binary
    let bin_bytes = client.get(download_url).send()
        .and_then(|r| r.bytes())
        .map_err(|e| format!("Download failed: {}", e))?;

    // Download signature
    let sig_bytes = client.get(sig_url).send()
        .and_then(|r| r.bytes())
        .map_err(|e| format!("Signature download failed: {}", e))?;

    // Parse public key
    let pubkey_bytes = hex_decode(UPDATE_PUBLIC_KEY_HEX)?;
    let verifying_key = VerifyingKey::from_bytes(
        pubkey_bytes.as_slice().try_into().map_err(|_| "Public key must be 32 bytes")?
    ).map_err(|e| format!("Invalid public key: {}", e))?;

    // Parse signature
    let signature = Signature::from_bytes(
        sig_bytes.as_ref().try_into().map_err(|_| "Signature must be 64 bytes")?
    );

    // Verify
    verifying_key.verify(&bin_bytes, &signature)
        .map_err(|_| "Signature verification failed. The update was NOT applied.".to_string())?;

    // Write to temp file, then self-replace
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("dungeon-mapper-update-{}", std::process::id()));
    std::fs::write(&temp_path, &bin_bytes)
        .map_err(|e| format!("Failed to write temp file: {}", e))?;

    // Set executable permission on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Failed to set permissions: {}", e))?;
    }

    self_replace::self_replace(&temp_path)
        .map_err(|e| format!("Failed to replace binary: {}", e))?;

    // Clean up temp file
    let _ = std::fs::remove_file(&temp_path);

    Ok(())
}

/// Spawn a background thread to download, verify, and apply an update.
pub fn download_and_apply(download_url: String, sig_url: String) -> mpsc::Receiver<ApplyStatus> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let status = match download_and_apply_blocking(&download_url, &sig_url) {
            Ok(()) => ApplyStatus::Success,
            Err(e) => {
                eprintln!("Update failed: {}", e);
                ApplyStatus::Error(e)
            }
        };
        let _ = tx.send(status);
    });
    rx
}
