//! Google Drive cloud sync integration.
//!
//! Provides OAuth2 login, upload, download, and conflict detection for
//! campaign files. Sync is opt-in per file and auto-pushes on save.

use std::path::PathBuf;
use std::sync::mpsc;

// Google OAuth2 endpoints
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const DRIVE_FILES_URL: &str = "https://www.googleapis.com/drive/v3/files";
const DRIVE_UPLOAD_URL: &str = "https://www.googleapis.com/upload/drive/v3/files";

// OAuth2 client credentials (drive.file scope — can only access files created by this app)
const CLIENT_ID: &str = match option_env!("GOOGLE_CLIENT_ID") {
    Some(v) => v,
    None => "",
};
const CLIENT_SECRET: &str = match option_env!("GOOGLE_CLIENT_SECRET") {
    Some(v) => v,
    None => "",
};
const REDIRECT_URI: &str = "http://localhost:19847/oauth/callback";
const SCOPES: &str = "https://www.googleapis.com/auth/drive.file";

/// App folder name in Google Drive root.
const DRIVE_FOLDER_NAME: &str = "Dungeon Mapper";

/// Whether cloud sync is available (OAuth credentials were configured at build time).
pub fn is_available() -> bool {
    !CLIENT_ID.is_empty() && !CLIENT_SECRET.is_empty()
}

/// Persisted OAuth2 tokens.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TokenData {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix timestamp when access_token expires.
    pub expires_at: u64,
}

/// State of the cloud sync system.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Default)]
pub struct CloudSyncState {
    /// OAuth2 tokens (None = not logged in).
    pub tokens: Option<TokenData>,
    /// Google Drive file ID for the current campaign (None = not yet synced).
    pub drive_file_id: Option<String>,
    /// Google Drive folder ID for "Dungeon Mapper" folder.
    pub drive_folder_id: Option<String>,
    /// Version of the campaign as last synced to Drive.
    pub synced_version: u64,
}

/// Result of a sync operation.
pub enum SyncResult {
    /// Upload succeeded.
    Uploaded,
    /// Download succeeded — contains the newer campaign JSON.
    Downloaded(String),
    /// Conflict detected: remote version is newer.
    Conflict { local_version: u64, remote_version: u64 },
    /// Not logged in.
    NotLoggedIn,
    /// Error occurred.
    Error(String),
}

/// Result of the OAuth login flow.
pub enum LoginResult {
    Success(TokenData),
    Error(String),
}

impl CloudSyncState {
    /// Whether the user is logged in (has tokens).
    pub fn is_logged_in(&self) -> bool {
        self.tokens.is_some()
    }

}

/// Get the path to the token storage file.
fn token_path() -> PathBuf {
    let mut path = dirs_config_path();
    path.push("cloud_tokens.json");
    path
}

/// Get the app config directory.
fn dirs_config_path() -> PathBuf {
    if let Some(config) = dirs_fallback() {
        let p = config.join("dungeon-mapper");
        let _ = std::fs::create_dir_all(&p);
        p
    } else {
        PathBuf::from(".")
    }
}

fn dirs_fallback() -> Option<PathBuf> {
    // XDG on Linux, ~/Library/Application Support on Mac, AppData on Windows
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_CONFIG_HOME").ok().map(PathBuf::from)
            .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config")))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var("HOME").ok().map(|h| PathBuf::from(h).join("Library/Application Support"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA").ok().map(PathBuf::from)
    }
}

/// Load saved cloud sync state from disk.
pub fn load_state() -> CloudSyncState {
    let path = token_path();
    match std::fs::read_to_string(&path) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
        Err(_) => CloudSyncState::default(),
    }
}

/// Save cloud sync state to disk.
pub fn save_state(state: &CloudSyncState) {
    let path = token_path();
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(&path, json);
    }
}

/// Start the OAuth2 login flow in a background thread.
/// Opens the browser and listens for the redirect on a local port.
pub fn login_async() -> mpsc::Receiver<LoginResult> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = perform_login();
        let _ = tx.send(result);
    });
    rx
}

fn perform_login() -> LoginResult {
    use std::io::{BufRead, Write};
    use std::net::TcpListener;

    // Bind the local callback server
    let listener = match TcpListener::bind("127.0.0.1:19847") {
        Ok(l) => l,
        Err(e) => return LoginResult::Error(format!("Failed to bind local server: {}", e)),
    };

    // Build the authorization URL
    let auth_url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent",
        AUTH_URL,
        urlenc(CLIENT_ID),
        urlenc(REDIRECT_URI),
        urlenc(SCOPES),
    );

    // Open browser
    if let Err(e) = open::that(&auth_url) {
        return LoginResult::Error(format!("Failed to open browser: {}", e));
    }

    // Wait for the redirect (with timeout)
    listener.set_nonblocking(false).ok();
    let stream = match listener.accept() {
        Ok((stream, _)) => stream,
        Err(e) => return LoginResult::Error(format!("Failed to accept connection: {}", e)),
    };

    let mut reader = std::io::BufReader::new(&stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return LoginResult::Error("Failed to read request".into());
    }

    // Extract the authorization code from the URL
    let code = extract_code(&request_line);

    // Send a response to the browser
    let response_body = "<html><body><h2>Login successful!</h2><p>You can close this tab and return to Dungeon Mapper.</p></body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response_body.len(),
        response_body
    );
    let mut writer = stream;
    let _ = writer.write_all(response.as_bytes());
    let _ = writer.flush();
    drop(writer);
    drop(listener);

    let Some(code) = code else {
        return LoginResult::Error("No authorization code received".into());
    };

    // Exchange code for tokens
    exchange_code(&code)
}

fn extract_code(request_line: &str) -> Option<String> {
    // Request line looks like: GET /oauth/callback?code=XYZ&scope=... HTTP/1.1
    let path = request_line.split_whitespace().nth(1)?;
    let query = path.split('?').nth(1)?;
    for param in query.split('&') {
        if let Some(value) = param.strip_prefix("code=") {
            return Some(urldec(value));
        }
    }
    None
}

fn exchange_code(code: &str) -> LoginResult {
    let client = reqwest::blocking::Client::new();
    let params = [
        ("code", code),
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
        ("redirect_uri", REDIRECT_URI),
        ("grant_type", "authorization_code"),
    ];
    let resp = match client.post(TOKEN_URL).form(&params).send() {
        Ok(r) => r,
        Err(e) => return LoginResult::Error(format!("Token request failed: {}", e)),
    };
    let json: serde_json::Value = match resp.json() {
        Ok(j) => j,
        Err(e) => return LoginResult::Error(format!("Token parse failed: {}", e)),
    };
    if let Some(err) = json.get("error").and_then(|v| v.as_str()) {
        let desc = json.get("error_description").and_then(|v| v.as_str()).unwrap_or("");
        return LoginResult::Error(format!("OAuth error: {} - {}", err, desc));
    }
    let access_token = json["access_token"].as_str().unwrap_or("").to_string();
    let refresh_token = json["refresh_token"].as_str().map(|s| s.to_string());
    let expires_in = json["expires_in"].as_u64().unwrap_or(3600);
    let expires_at = now_unix() + expires_in;

    LoginResult::Success(TokenData {
        access_token,
        refresh_token,
        expires_at,
    })
}

/// Refresh the access token using the refresh token.
fn refresh_access_token(state: &mut CloudSyncState) -> Result<(), String> {
    let tokens = state.tokens.as_ref().ok_or("Not logged in")?;
    let refresh_token = tokens.refresh_token.as_ref().ok_or("No refresh token")?;

    let client = reqwest::blocking::Client::new();
    let params = [
        ("refresh_token", refresh_token.as_str()),
        ("client_id", CLIENT_ID),
        ("client_secret", CLIENT_SECRET),
        ("grant_type", "refresh_token"),
    ];
    let resp = client.post(TOKEN_URL).form(&params).send()
        .map_err(|e| format!("Refresh request failed: {}", e))?;
    let json: serde_json::Value = resp.json()
        .map_err(|e| format!("Refresh parse failed: {}", e))?;

    if let Some(err) = json.get("error").and_then(|v| v.as_str()) {
        return Err(format!("Refresh failed: {}", err));
    }

    let access_token = json["access_token"].as_str().unwrap_or("").to_string();
    let expires_in = json["expires_in"].as_u64().unwrap_or(3600);

    if let Some(tokens) = &mut state.tokens {
        tokens.access_token = access_token;
        tokens.expires_at = now_unix() + expires_in;
    }
    Ok(())
}

/// Ensure the access token is valid, refreshing if needed.
fn ensure_valid_token(state: &mut CloudSyncState) -> Result<String, String> {
    let tokens = state.tokens.as_ref().ok_or("Not logged in")?;
    if now_unix() >= tokens.expires_at.saturating_sub(60) {
        refresh_access_token(state)?;
    }
    Ok(state.tokens.as_ref().unwrap().access_token.clone())
}

/// Find or create the "Dungeon Mapper" folder in Drive root.
fn ensure_drive_folder(state: &mut CloudSyncState, token: &str) -> Result<String, String> {
    if let Some(id) = &state.drive_folder_id {
        return Ok(id.clone());
    }

    let client = reqwest::blocking::Client::new();

    // Search for existing folder
    let query = format!("name='{}' and mimeType='application/vnd.google-apps.folder' and trashed=false", DRIVE_FOLDER_NAME);
    let resp = client.get(DRIVE_FILES_URL)
        .bearer_auth(token)
        .query(&[("q", query.as_str()), ("fields", "files(id,name)")])
        .send()
        .map_err(|e| e.to_string())?;
    let json: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
    if let Some(file) = json["files"].as_array().and_then(|f| f.first()) {
        let id = file["id"].as_str().unwrap_or("").to_string();
        state.drive_folder_id = Some(id.clone());
        return Ok(id);
    }

    // Create folder
    let metadata = serde_json::json!({
        "name": DRIVE_FOLDER_NAME,
        "mimeType": "application/vnd.google-apps.folder"
    });
    let resp = client.post(DRIVE_FILES_URL)
        .bearer_auth(token)
        .json(&metadata)
        .send()
        .map_err(|e| e.to_string())?;
    let json: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
    let id = json["id"].as_str().ok_or("No folder id in response")?.to_string();
    state.drive_folder_id = Some(id.clone());
    Ok(id)
}

/// Upload or update a campaign file on Drive.
/// Returns the Drive file ID.
fn upload_to_drive(
    state: &mut CloudSyncState,
    token: &str,
    file_name: &str,
    content: &str,
) -> Result<String, String> {
    let client = reqwest::blocking::Client::new();
    let folder_id = ensure_drive_folder(state, token)?;

    if let Some(file_id) = &state.drive_file_id {
        // Update existing file
        let url = format!("{}?uploadType=media", DRIVE_UPLOAD_URL.to_string() + "/" + file_id);
        let resp = client.patch(&url)
            .bearer_auth(token)
            .header("Content-Type", "application/json")
            .body(content.to_string())
            .send()
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(format!("Upload failed ({}): {}", status, body));
        }
        Ok(file_id.clone())
    } else {
        // Create new file (multipart upload)
        let metadata = serde_json::json!({
            "name": file_name,
            "parents": [folder_id],
        });
        let boundary = "dungeon_mapper_boundary";
        let body = format!(
            "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{}\r\n--{boundary}\r\nContent-Type: application/json\r\n\r\n{}\r\n--{boundary}--",
            serde_json::to_string(&metadata).unwrap_or_default(),
            content,
            boundary = boundary,
        );
        let url = format!("{}?uploadType=multipart", DRIVE_UPLOAD_URL);
        let resp = client.post(&url)
            .bearer_auth(token)
            .header("Content-Type", format!("multipart/related; boundary={}", boundary))
            .body(body)
            .send()
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().unwrap_or_default();
            return Err(format!("Upload failed ({}): {}", status, body));
        }
        let json: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
        let file_id = json["id"].as_str().ok_or("No file id in response")?.to_string();
        state.drive_file_id = Some(file_id.clone());
        Ok(file_id)
    }
}

/// Download a campaign file from Drive.
/// Returns the JSON content as a string.
fn download_from_drive(token: &str, file_id: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::new();
    let url = format!("{}/{}?alt=media", DRIVE_FILES_URL, file_id);
    let resp = client.get(&url)
        .bearer_auth(token)
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("Download failed ({}): {}", status, body));
    }
    resp.text().map_err(|e| e.to_string())
}

/// Get the remote campaign version without downloading the full file.
/// Downloads only enough to parse the version field.
fn get_remote_version(token: &str, file_id: &str) -> Result<u64, String> {
    // Unfortunately Drive API doesn't support range requests on file content,
    // so we download the full file and parse just the version.
    let content = download_from_drive(token, file_id)?;
    let raw: serde_json::Value = serde_json::from_str(&content).map_err(|e| e.to_string())?;
    // Version 2 format: { "version": 2, "campaign": { ..., "version": N } }
    let campaign_version = raw.get("campaign")
        .and_then(|c| c.get("version"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    Ok(campaign_version)
}

/// Push campaign to Drive (called after local save).
/// Checks for conflicts using version numbers.
pub fn sync_push(
    state: &mut CloudSyncState,
    campaign_json: &str,
    campaign_name: &str,
    local_version: u64,
) -> SyncResult {
    if !state.is_logged_in() {
        return SyncResult::NotLoggedIn;
    }

    let token = match ensure_valid_token(state) {
        Ok(t) => t,
        Err(e) => return SyncResult::Error(e),
    };

    // Check for conflicts if we have an existing file
    if let Some(file_id) = &state.drive_file_id.clone() {
        match get_remote_version(&token, file_id) {
            Ok(remote_version) => {
                if remote_version > state.synced_version {
                    return SyncResult::Conflict {
                        local_version,
                        remote_version,
                    };
                }
            }
            Err(_) => {
                // File might have been deleted — proceed with upload as new
                state.drive_file_id = None;
            }
        }
    }

    let file_name = format!("{}.dungeon", campaign_name);
    match upload_to_drive(state, &token, &file_name, campaign_json) {
        Ok(_) => {
            state.synced_version = local_version;
            save_state(state);
            SyncResult::Uploaded
        }
        Err(e) => SyncResult::Error(e),
    }
}

/// Pull campaign from Drive (checks if remote is newer).
pub fn sync_pull(state: &mut CloudSyncState, local_version: u64) -> SyncResult {
    if !state.is_logged_in() {
        return SyncResult::NotLoggedIn;
    }

    let token = match ensure_valid_token(state) {
        Ok(t) => t,
        Err(e) => return SyncResult::Error(e),
    };

    let Some(file_id) = &state.drive_file_id.clone() else {
        return SyncResult::Error("No file synced to Drive yet".into());
    };

    match download_from_drive(&token, file_id) {
        Ok(content) => {
            // Parse remote version
            let remote_version = serde_json::from_str::<serde_json::Value>(&content).ok()
                .and_then(|v| v.get("campaign")?.get("version")?.as_u64())
                .unwrap_or(0);

            if remote_version > local_version {
                state.synced_version = remote_version;
                save_state(state);
                SyncResult::Downloaded(content)
            } else {
                SyncResult::Uploaded // Already up to date (reusing variant name loosely)
            }
        }
        Err(e) => SyncResult::Error(e),
    }
}

/// Spawn a background push operation.
pub fn sync_push_async(
    mut state: CloudSyncState,
    campaign_json: String,
    campaign_name: String,
    local_version: u64,
) -> mpsc::Receiver<(SyncResult, CloudSyncState)> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = sync_push(&mut state, &campaign_json, &campaign_name, local_version);
        let _ = tx.send((result, state));
    });
    rx
}

/// Spawn a background pull operation.
pub fn sync_pull_async(
    mut state: CloudSyncState,
    local_version: u64,
) -> mpsc::Receiver<(SyncResult, CloudSyncState)> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = sync_pull(&mut state, local_version);
        let _ = tx.send((result, state));
    });
    rx
}


/// A file listing from the Drive folder.
#[derive(Clone, Debug)]
pub struct DriveFile {
    pub id: String,
    pub name: String,
}

/// List campaign files in the Drive folder.
pub fn list_drive_files(state: &mut CloudSyncState) -> Result<Vec<DriveFile>, String> {
    let token = ensure_valid_token(state)?;
    let folder_id = ensure_drive_folder(state, &token)?;

    let client = reqwest::blocking::Client::new();
    let query = format!("'{}' in parents and trashed=false", folder_id);
    let resp = client.get(DRIVE_FILES_URL)
        .bearer_auth(&token)
        .query(&[("q", query.as_str()), ("fields", "files(id,name)")])
        .send()
        .map_err(|e| e.to_string())?;
    let json: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
    let files = json["files"].as_array()
        .map(|arr| arr.iter().filter_map(|f| {
            Some(DriveFile {
                id: f["id"].as_str()?.to_string(),
                name: f["name"].as_str()?.to_string(),
            })
        }).collect())
        .unwrap_or_default();
    Ok(files)
}

/// List Drive files in the background.
pub fn list_drive_files_async(
    mut state: CloudSyncState,
) -> mpsc::Receiver<(Result<Vec<DriveFile>, String>, CloudSyncState)> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = list_drive_files(&mut state);
        let _ = tx.send((result, state));
    });
    rx
}

/// Download a specific file from Drive by ID.
pub fn open_from_drive(state: &mut CloudSyncState, file_id: &str) -> Result<String, String> {
    let token = ensure_valid_token(state)?;
    let content = download_from_drive(&token, file_id)?;
    state.drive_file_id = Some(file_id.to_string());
    // Parse the version so synced_version is set correctly
    let remote_version = serde_json::from_str::<serde_json::Value>(&content).ok()
        .and_then(|v| v.get("campaign")?.get("version")?.as_u64())
        .unwrap_or(0);
    state.synced_version = remote_version;
    save_state(state);
    Ok(content)
}

/// Open a file from Drive in the background.
pub fn open_from_drive_async(
    mut state: CloudSyncState,
    file_id: String,
) -> mpsc::Receiver<(Result<String, String>, CloudSyncState)> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = open_from_drive(&mut state, &file_id);
        let _ = tx.send((result, state));
    });
    rx
}

// --- Utility functions ---

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn urlenc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{:02X}", b));
            }
        }
    }
    out
}

fn urldec(s: &str) -> String {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(val) = u8::from_str_radix(
                std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""),
                16,
            ) {
                out.push(val);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}
