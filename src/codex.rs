//! Codex account and usage provider.
//!
//! Codex remains the owner of credentials. Each profile is an isolated
//! `CODEX_HOME`; this module never serializes credentials into ai-monitor's
//! configuration or output.

use std::collections::VecDeque;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use directories::BaseDirs;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use thiserror::Error;

const DEFAULT_HOME: &str = ".chatgpt-status";
const PRIVATE_CREDITS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";

#[derive(Debug, Error)]
pub enum Error {
    #[error("could not determine the profile storage directory")]
    HomeUnavailable,
    #[error("profile name may only contain letters, numbers, '-', '_', and '.'")]
    InvalidProfileName,
    #[error("profile was not found")]
    ProfileNotFound,
    #[error("no default profile is configured")]
    NoDefaultProfile,
    #[error("profile configuration is invalid")]
    InvalidConfig,
    #[error("profile storage operation failed")]
    Storage(#[source] std::io::Error),
    #[error("could not start Codex")]
    Launch(#[source] std::io::Error),
    #[error("profile '{profile}' is already authenticated; use --force to log in again")]
    AlreadyAuthenticated { profile: String },
    #[error("Codex app-server request timed out")]
    Timeout,
    #[error("Codex app-server closed unexpectedly")]
    Closed,
    #[error("Codex app-server protocol error")]
    Protocol,
    #[error("Codex app-server returned an error")]
    Rpc,
    #[error("private credit lookup failed")]
    PrivateCredits,
    #[error("private credit lookup was rate limited; retry after {retry_after_seconds:?} seconds")]
    PrivateCreditsRateLimited { retry_after_seconds: Option<u64> },
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Profile {
    pub name: String,
    /// The local path is used internally but is not part of JSON output.
    #[serde(skip_serializing)]
    pub home: PathBuf,
    pub authenticated: bool,
}

impl Profile {
    pub fn auth_file(&self) -> PathBuf {
        self.home.join("auth.json")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Config {
    #[serde(default = "default_schema")]
    schema_version: u32,
    #[serde(default)]
    default_profile: Option<String>,
}

fn default_schema() -> u32 {
    3
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: default_schema(),
            default_profile: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProfileStore {
    root: PathBuf,
}

impl ProfileStore {
    pub fn from_env() -> Result<Self> {
        let root = env::var_os("AI_MONITOR_HOME")
            .or_else(|| env::var_os("CHATGPT_STATUS_HOME"))
            .map(PathBuf::from)
            .or_else(|| BaseDirs::new().map(|dirs| dirs.home_dir().join(DEFAULT_HOME)))
            .ok_or(Error::HomeUnavailable)?;
        Ok(Self { root })
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn list(&self) -> Result<Vec<Profile>> {
        let profiles_dir = self.root.join("profiles");
        if !profiles_dir.exists() {
            return Ok(Vec::new());
        }
        let mut profiles = fs::read_dir(profiles_dir)
            .map_err(Error::Storage)?
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_owned();
                validate_profile_name(&name).ok()?;
                let home = entry.path();
                Some(Profile {
                    name,
                    authenticated: home.join("auth.json").is_file(),
                    home,
                })
            })
            .collect::<Vec<_>>();
        profiles.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(profiles)
    }

    pub fn default_name(&self) -> Result<Option<String>> {
        Ok(self.load_config()?.default_profile)
    }

    pub fn resolve(&self, name: Option<&str>) -> Result<Profile> {
        let profiles = self.list()?;
        let selected = name
            .map(str::to_owned)
            .or(self.default_name()?)
            .or_else(|| (profiles.len() == 1).then(|| profiles[0].name.clone()))
            .ok_or(Error::NoDefaultProfile)?;
        profiles
            .into_iter()
            .find(|profile| profile.name == selected)
            .ok_or(Error::ProfileNotFound)
    }

    pub fn create(&self, name: &str) -> Result<Profile> {
        validate_profile_name(name)?;
        create_private_dir(&self.root)?;
        let profiles_dir = self.root.join("profiles");
        create_private_dir(&profiles_dir)?;
        let home = profiles_dir.join(name);
        create_private_dir(&home)?;
        let config_file = home.join("config.toml");
        if !config_file.exists() {
            fs::write(&config_file, "cli_auth_credentials_store = \"file\"\n")
                .map_err(Error::Storage)?;
            set_private_file(&config_file)?;
        }
        Ok(Profile {
            name: name.to_owned(),
            authenticated: home.join("auth.json").is_file(),
            home,
        })
    }

    pub fn set_default(&self, name: &str) -> Result<()> {
        self.resolve(Some(name))?;
        let config = Config {
            schema_version: 3,
            default_profile: Some(name.to_owned()),
        };
        self.save_config(&config)
    }

    fn load_config(&self) -> Result<Config> {
        let path = self.root.join("config.json");
        if !path.exists() {
            return Ok(Config::default());
        }
        let contents = fs::read(path).map_err(Error::Storage)?;
        let config: Config = serde_json::from_slice(&contents).map_err(|_| Error::InvalidConfig)?;
        if !matches!(config.schema_version, 2 | 3) {
            return Err(Error::InvalidConfig);
        }
        Ok(config)
    }

    fn save_config(&self, config: &Config) -> Result<()> {
        create_private_dir(&self.root)?;
        let destination = self.root.join("config.json");
        let temporary = self.root.join(".config.json.tmp");
        let mut contents = serde_json::to_vec_pretty(config).map_err(|_| Error::InvalidConfig)?;
        contents.push(b'\n');
        fs::write(&temporary, contents).map_err(Error::Storage)?;
        set_private_file(&temporary)?;
        fs::rename(temporary, destination).map_err(Error::Storage)
    }
}

pub fn validate_profile_name(name: &str) -> Result<()> {
    if name.is_empty()
        || matches!(name, "." | "..")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(Error::InvalidProfileName);
    }
    Ok(())
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path).map_err(Error::Storage)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(Error::Storage)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(Error::Storage)
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(Error::Storage)
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<()> {
    Ok(())
}

fn codex_binary() -> PathBuf {
    env::var_os("AI_MONITOR_CODEX_BIN")
        .or_else(|| env::var_os("CHATGPT_STATUS_CODEX_BIN"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("codex"))
}

fn isolated_command(profile: &Profile) -> Command {
    let mut command = Command::new(codex_binary());
    command
        .env("CODEX_HOME", &profile.home)
        .env_remove("OPENAI_API_KEY");
    command
}

pub fn login(store: &ProfileStore, name: &str, force: bool) -> Result<ExitStatus> {
    let profile = store.create(name)?;
    if profile.authenticated && !force {
        return Err(Error::AlreadyAuthenticated {
            profile: profile.name,
        });
    }
    isolated_command(&profile)
        .arg("login")
        .status()
        .map_err(Error::Launch)
}

pub fn logout(profile: &Profile) -> Result<ExitStatus> {
    isolated_command(profile)
        .arg("logout")
        .status()
        .map_err(Error::Launch)
}

pub fn run(profile: &Profile, args: &[String]) -> Result<ExitStatus> {
    isolated_command(profile)
        .args(args)
        .status()
        .map_err(Error::Launch)
}

enum Event {
    Line(String),
    Closed,
}

struct AppServer {
    child: Child,
    stdin: ChildStdin,
    events: Receiver<Event>,
    stderr: Arc<Mutex<VecDeque<String>>>,
    next_id: u64,
    timeout: Duration,
}

impl AppServer {
    fn connect(profile: &Profile) -> Result<Self> {
        let mut child = isolated_command(profile)
            .args(["app-server", "--listen", "stdio://"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(Error::Launch)?;
        let stdin = child.stdin.take().ok_or(Error::Protocol)?;
        let stdout = child.stdout.take().ok_or(Error::Protocol)?;
        let stderr_pipe = child.stderr.take().ok_or(Error::Protocol)?;
        let (sender, events) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(Event::Line(line)).is_err() {
                    return;
                }
            }
            let _ = sender.send(Event::Closed);
        });
        let stderr = Arc::new(Mutex::new(VecDeque::with_capacity(50)));
        let stderr_target = Arc::clone(&stderr);
        thread::spawn(move || read_stderr(stderr_pipe, stderr_target));
        let mut server = Self {
            child,
            stdin,
            events,
            stderr,
            next_id: 1,
            timeout: Duration::from_secs(25),
        };
        server.request(
            "initialize",
            Some(json!({
                "clientInfo": {
                    "name": "ai-monitor",
                    "title": "AI Monitor",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {}
            })),
        )?;
        server.notify("initialized", Some(json!({})))?;
        Ok(server)
    }

    fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let mut message = json!({"id": id, "method": method});
        if let Some(params) = params {
            message["params"] = params;
        }
        self.send(&message)?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(Error::Timeout);
            }
            match self.events.recv_timeout(remaining) {
                Ok(Event::Line(line)) => {
                    let Ok(response) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    if response.get("id").and_then(Value::as_u64) != Some(id) {
                        continue;
                    }
                    if response.get("error").is_some() {
                        return Err(Error::Rpc);
                    }
                    return response.get("result").cloned().ok_or(Error::Protocol);
                }
                Ok(Event::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::Closed);
                }
                Err(RecvTimeoutError::Timeout) => return Err(Error::Timeout),
            }
        }
    }

    fn notify(&mut self, method: &str, params: Option<Value>) -> Result<()> {
        let mut message = json!({"method": method});
        if let Some(params) = params {
            message["params"] = params;
        }
        self.send(&message)
    }

    fn send(&mut self, value: &Value) -> Result<()> {
        serde_json::to_writer(&mut self.stdin, value).map_err(|_| Error::Protocol)?;
        self.stdin.write_all(b"\n").map_err(|_| Error::Closed)?;
        self.stdin.flush().map_err(|_| Error::Closed)
    }

    #[allow(dead_code)]
    fn stderr_tail(&self) -> Vec<String> {
        self.stderr
            .lock()
            .map(|lines| lines.iter().cloned().collect())
            .unwrap_or_default()
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_stderr(mut reader: impl Read, target: Arc<Mutex<VecDeque<String>>>) {
    let mut buffer = [0_u8; 4096];
    let mut pending = String::new();
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        pending.push_str(&String::from_utf8_lossy(&buffer[..read]));
        while let Some(newline) = pending.find('\n') {
            let line = pending[..newline].trim_end_matches('\r').to_owned();
            pending.drain(..=newline);
            if let Ok(mut lines) = target.lock() {
                if lines.len() == 50 {
                    lines.pop_front();
                }
                lines.push_back(line);
            }
        }
    }
    if !pending.is_empty() {
        if let Ok(mut lines) = target.lock() {
            if lines.len() == 50 {
                lines.pop_front();
            }
            lines.push_back(pending);
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Account {
    pub email: Option<String>,
    pub plan_type: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LimitWindow {
    pub name: String,
    pub used_percent: f64,
    pub window_minutes: Option<u64>,
    pub resets_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_window_minutes: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResetCredit {
    pub id: Option<String>,
    pub status: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub granted_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ResetCredits {
    pub available_count: Option<u64>,
    pub credits: Option<Vec<ResetCredit>>,
    pub source: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub profile: String,
    pub account: Account,
    pub limits: Vec<LimitWindow>,
    pub reset_credits: ResetCredits,
    pub usage: Option<Value>,
    pub usage_error: Option<String>,
}

pub fn fetch(profile: &Profile) -> Result<Snapshot> {
    let mut server = AppServer::connect(profile)?;
    let account_result = server.request("account/read", Some(json!({"refreshToken": false})))?;
    let limits_result = server.request("account/rateLimits/read", None)?;
    let (usage, usage_error) = match server.request("account/usage/read", None) {
        Ok(value) => (Some(value), None),
        Err(error) => (None, Some(error.to_string())),
    };
    Ok(normalize_snapshot(
        &profile.name,
        &account_result,
        &limits_result,
        usage,
        usage_error,
    ))
}

fn normalize_snapshot(
    profile: &str,
    account_result: &Value,
    limits_result: &Value,
    usage: Option<Value>,
    usage_error: Option<String>,
) -> Snapshot {
    let account_value = account_result.get("account").unwrap_or(account_result);
    let account = Account {
        email: string_field(account_value, &["email"]),
        plan_type: string_field(account_value, &["planType", "plan_type"]),
    };
    let limits = normalize_rate_limits(limits_result);
    let standard_limits = limits_result
        .pointer("/rateLimitsByLimitId/codex")
        .or_else(|| limits_result.get("rateLimits"))
        .unwrap_or(limits_result);
    let credits_value = limits_result
        .get("rateLimitResetCredits")
        .or_else(|| standard_limits.get("rateLimitResetCredits"));
    let reset_credits = credits_value
        .map(|value| normalize_credits(value, "app-server"))
        .unwrap_or_else(|| ResetCredits {
            source: "app-server".to_owned(),
            ..ResetCredits::default()
        });
    Snapshot {
        profile: profile.to_owned(),
        account,
        limits,
        reset_credits,
        usage,
        usage_error,
    }
}

fn normalize_rate_limits(limits_result: &Value) -> Vec<LimitWindow> {
    let by_limit_id = limits_result
        .get("rateLimitsByLimitId")
        .and_then(Value::as_object);
    let standard = by_limit_id
        .and_then(|buckets| buckets.get("codex"))
        .or_else(|| limits_result.get("rateLimits"))
        .unwrap_or(limits_result);

    let mut limits = Vec::new();
    append_limit_windows(&mut limits, standard, None, None);

    if let Some(buckets) = by_limit_id {
        let mut additional = buckets
            .iter()
            .filter(|(limit_id, _)| limit_id.as_str() != "codex")
            .collect::<Vec<_>>();
        additional.sort_by(|(left_id, left), (right_id, right)| {
            additional_bucket_label(left_id, left)
                .cmp(&additional_bucket_label(right_id, right))
                .then_with(|| left_id.cmp(right_id))
        });
        for (limit_id, bucket) in additional {
            let limit_name = string_field(bucket, &["limitName", "limit_name"]);
            append_limit_windows(&mut limits, bucket, Some(limit_id), limit_name.as_deref());
        }
    }
    limits
}

fn append_limit_windows(
    limits: &mut Vec<LimitWindow>,
    bucket: &Value,
    limit_id: Option<&str>,
    limit_name: Option<&str>,
) {
    let additional = limit_id.is_some();
    for slot in ["primary", "secondary"] {
        let Some(window) = bucket.get(slot) else {
            continue;
        };
        if !window.is_object() {
            continue;
        }
        let source_window_minutes =
            number_field(window, &["windowDurationMins", "window_duration_mins"])
                .map(|value| value.max(0.0).round() as u64);
        let name = if let Some(limit_id) = limit_id {
            additional_window_label(
                &additional_bucket_label_with_name(limit_id, limit_name),
                source_window_minutes,
                slot,
            )
        } else {
            slot.to_owned()
        };
        limits.push(LimitWindow {
            name,
            used_percent: number_field(window, &["usedPercent", "used_percent"])
                .unwrap_or_default(),
            window_minutes: if additional {
                None
            } else {
                source_window_minutes
            },
            resets_at: field(window, &["resetsAt", "resets_at"]).and_then(parse_datetime),
            limit_id: limit_id.map(str::to_owned),
            limit_name: limit_name.map(str::to_owned),
            source_window_minutes: additional.then_some(source_window_minutes).flatten(),
        });
    }
}

fn additional_bucket_label(limit_id: &str, bucket: &Value) -> String {
    additional_bucket_label_with_name(
        limit_id,
        string_field(bucket, &["limitName", "limit_name"]).as_deref(),
    )
}

fn additional_bucket_label_with_name(limit_id: &str, limit_name: Option<&str>) -> String {
    let candidate = limit_name.unwrap_or(limit_id);
    let candidate_lower = candidate.to_ascii_lowercase();
    let id_lower = limit_id.to_ascii_lowercase();
    if candidate_lower.contains("spark") || id_lower.contains("bengalfox") {
        return "Spark".to_owned();
    }
    if candidate_lower.contains("reserve") || id_lower == "base_model_inference" {
        return "Reserve".to_owned();
    }

    let cleaned = candidate
        .chars()
        .map(|character| {
            if matches!(character, '_' | '-') {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let compact = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(10).collect()
}

fn additional_window_label(base: &str, minutes: Option<u64>, slot: &str) -> String {
    let suffix = match minutes {
        Some(300) => "5h".to_owned(),
        Some(10_080) => "W".to_owned(),
        Some(minutes) if minutes % 1_440 == 0 => format!("{}d", minutes / 1_440),
        Some(minutes) if minutes % 60 == 0 => format!("{}h", minutes / 60),
        Some(minutes) => format!("{minutes}m"),
        None if slot == "primary" => "P".to_owned(),
        None => "S".to_owned(),
    };
    format!("{base} {suffix}")
}

pub fn detailed_credits(
    profile: &Profile,
    snapshot: &Snapshot,
    allow_private: bool,
) -> Result<ResetCredits> {
    if snapshot.reset_credits.credits.is_some()
        || !allow_private
        || snapshot.reset_credits.available_count == Some(0)
    {
        return Ok(snapshot.reset_credits.clone());
    }
    fetch_private_credits(profile)
}

fn fetch_private_credits(profile: &Profile) -> Result<ResetCredits> {
    let auth = fs::read(profile.auth_file()).map_err(|_| Error::PrivateCredits)?;
    let auth: Value = serde_json::from_slice(&auth).map_err(|_| Error::PrivateCredits)?;
    let tokens = auth.get("tokens").unwrap_or(&auth);
    let token = string_field(tokens, &["access_token"]).ok_or(Error::PrivateCredits)?;
    let account_id =
        string_field(tokens, &["account_id"]).or_else(|| string_field(&auth, &["account_id"]));
    let mut request = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| Error::PrivateCredits)?
        .get(PRIVATE_CREDITS_URL)
        .bearer_auth(token)
        .header("OpenAI-Beta", "codex-1")
        .header("originator", "Codex Desktop")
        .header(
            "User-Agent",
            concat!("ai-monitor/", env!("CARGO_PKG_VERSION")),
        );
    if let Some(account_id) = account_id {
        request = request.header("ChatGPT-Account-ID", account_id);
    }
    let response = request.send().map_err(|_| Error::PrivateCredits)?;
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after_seconds = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok());
        return Err(Error::PrivateCreditsRateLimited {
            retry_after_seconds,
        });
    }
    if !response.status().is_success() {
        return Err(Error::PrivateCredits);
    }
    let value: Value = response.json().map_err(|_| Error::PrivateCredits)?;
    Ok(normalize_credits(&value, "private-fallback"))
}

fn normalize_credits(value: &Value, source: &str) -> ResetCredits {
    let available_count = number_field(value, &["availableCount", "available_count"])
        .map(|value| value.max(0.0).round() as u64);
    let credits = value.get("credits").and_then(Value::as_array).map(|rows| {
        rows.iter()
            .filter_map(|row| {
                row.as_object()?;
                Some(ResetCredit {
                    id: string_field(row, &["id", "creditId", "credit_id"]),
                    status: string_field(row, &["status"]),
                    title: string_field(row, &["title"]),
                    description: string_field(row, &["description", "reason"]),
                    granted_at: field(row, &["grantedAt", "granted_at"]).and_then(parse_datetime),
                    expires_at: field(row, &["expiresAt", "expires_at"]).and_then(parse_datetime),
                })
            })
            .collect()
    });
    ResetCredits {
        available_count,
        credits,
        source: source.to_owned(),
    }
}

pub fn expiring(credits: &ResetCredits, days: u32, now: DateTime<Utc>) -> Vec<ResetCredit> {
    let deadline = now + chrono::Duration::days(i64::from(days));
    let mut rows = credits
        .credits
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter(|credit| {
            let active = credit
                .status
                .as_deref()
                .map(|status| {
                    matches!(status.to_ascii_lowercase().as_str(), "available" | "active")
                })
                .unwrap_or(true);
            active
                && credit
                    .expires_at
                    .is_some_and(|expires| expires >= now && expires <= deadline)
        })
        .cloned()
        .collect::<Vec<_>>();
    rows.sort_by_key(|credit| credit.expires_at);
    rows
}

fn field<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| value.get(*name))
}

fn string_field(value: &Value, names: &[&str]) -> Option<String> {
    field(value, names)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn number_field(value: &Value, names: &[&str]) -> Option<f64> {
    let value = field(value, names)?;
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        .filter(|value| value.is_finite())
}

fn parse_datetime(value: &Value) -> Option<DateTime<Utc>> {
    if let Some(seconds) = value.as_f64() {
        return DateTime::from_timestamp(seconds.round() as i64, 0);
    }
    let value = value.as_str()?;
    if let Ok(seconds) = value.parse::<f64>() {
        return DateTime::from_timestamp(seconds.round() as i64, 0);
    }
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|datetime| datetime.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_store_lists_profile_directories() {
        let temp = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(temp.path());
        let profile = store.create("main").unwrap();
        fs::write(profile.auth_file(), "{}").unwrap();
        fs::write(
            temp.path().join("config.json"),
            r#"{"schema_version":2,"default_profile":"main"}"#,
        )
        .unwrap();

        let profiles = store.list().unwrap();
        assert_eq!(profiles.len(), 1);
        assert!(profiles[0].authenticated);
        assert_eq!(store.resolve(None).unwrap().name, "main");
    }

    #[test]
    fn rejects_unsafe_profile_names() {
        assert!(validate_profile_name("main").is_ok());
        assert!(validate_profile_name("../main").is_err());
        assert!(validate_profile_name("bad name").is_err());
    }

    #[test]
    fn login_requires_force_for_authenticated_profile() {
        let temp = tempfile::tempdir().unwrap();
        let store = ProfileStore::new(temp.path());
        let profile = store.create("main").unwrap();
        fs::write(profile.auth_file(), "{}").unwrap();

        let error = login(&store, "main", false).unwrap_err();
        assert!(matches!(
            &error,
            Error::AlreadyAuthenticated { profile } if profile == "main"
        ));
        assert_eq!(
            error.to_string(),
            "profile 'main' is already authenticated; use --force to log in again"
        );
    }

    #[test]
    fn normalizes_current_app_server_shape() {
        let snapshot = normalize_snapshot(
            "main",
            &json!({"account":{"email":"me@example.test","planType":"plus"}}),
            &json!({
                "rateLimitsByLimitId":{"codex":{
                    "primary":{"usedPercent":20,"windowDurationMins":300,"resetsAt":4102444800_u64}
                }},
                "rateLimitResetCredits":{"availableCount":2}
            }),
            None,
            None,
        );
        assert_eq!(snapshot.account.email.as_deref(), Some("me@example.test"));
        assert_eq!(snapshot.limits[0].used_percent, 20.0);
        assert_eq!(snapshot.limits[0].name, "primary");
        assert_eq!(snapshot.limits[0].window_minutes, Some(300));
        assert_eq!(snapshot.reset_credits.available_count, Some(2));
    }

    #[test]
    fn normalizes_additional_rate_limit_buckets() {
        let snapshot = normalize_snapshot(
            "main",
            &json!({"account":{"email":"me@example.test","planType":"plus"}}),
            &json!({
                "rateLimits": {
                    "limitId":"codex",
                    "primary":{"usedPercent":10,"windowDurationMins":300,"resetsAt":4102444800_u64},
                    "secondary":{"usedPercent":35,"windowDurationMins":10080,"resetsAt":4103049600_u64}
                },
                "rateLimitsByLimitId": {
                    "base_model_inference": {
                        "limitId":"base_model_inference",
                        "limitName":"gpt-reserve",
                        "primary":{"usedPercent":8,"windowDurationMins":10080,"resetsAt":4103049600_u64}
                    },
                    "codex_bengalfox": {
                        "limitId":"codex_bengalfox",
                        "limitName":"GPT-5.3-Codex-Spark",
                        "primary":{"usedPercent":4,"windowDurationMins":300,"resetsAt":4102444800_u64},
                        "secondary":{"usedPercent":12,"windowDurationMins":10080,"resetsAt":4103049600_u64}
                    }
                }
            }),
            None,
            None,
        );

        assert_eq!(snapshot.limits.len(), 5);
        assert_eq!(snapshot.limits[0].name, "primary");
        assert_eq!(snapshot.limits[1].name, "secondary");
        let reserve = snapshot
            .limits
            .iter()
            .find(|limit| limit.name == "Reserve W")
            .unwrap();
        assert_eq!(reserve.used_percent, 8.0);
        assert_eq!(reserve.limit_id.as_deref(), Some("base_model_inference"));
        assert_eq!(reserve.source_window_minutes, Some(10_080));
        let spark_5h = snapshot
            .limits
            .iter()
            .find(|limit| limit.name == "Spark 5h")
            .unwrap();
        assert_eq!(spark_5h.used_percent, 4.0);
        assert_eq!(spark_5h.window_minutes, None);
    }

    #[test]
    fn filters_expiring_active_credits() {
        let now = DateTime::parse_from_rfc3339("2026-08-11T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let credits = ResetCredits {
            available_count: Some(2),
            source: "test".to_owned(),
            credits: Some(vec![
                ResetCredit {
                    id: Some("soon".to_owned()),
                    status: Some("available".to_owned()),
                    title: None,
                    description: None,
                    granted_at: None,
                    expires_at: Some(now + chrono::Duration::days(2)),
                },
                ResetCredit {
                    id: Some("used".to_owned()),
                    status: Some("consumed".to_owned()),
                    title: None,
                    description: None,
                    granted_at: None,
                    expires_at: Some(now + chrono::Duration::days(1)),
                },
            ]),
        };
        assert_eq!(expiring(&credits, 7, now)[0].id.as_deref(), Some("soon"));
    }
}
