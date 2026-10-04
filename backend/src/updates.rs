use axum::{Json, extract::State, http::StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, RwLock};

pub const VERSION: &str = match option_env!("HOSTABLE_BUILD_VERSION") {
    Some(value) => value,
    None => env!("CARGO_PKG_VERSION"),
};
pub const COMMIT: &str = match option_env!("HOSTABLE_BUILD_COMMIT") {
    Some(value) => value,
    None => "local",
};
const API: &str = "https://api.github.com/repos/tim2zg/hostable/releases/latest";
const REPOSITORY: &str = "https://github.com/tim2zg/hostable";
type ApiError = (StatusCode, String);
fn error(message: String) -> ApiError {
    (StatusCode::BAD_REQUEST, message)
}

pub struct ManagerUpdates {
    serial: Mutex<()>,
    pub mutations: Arc<RwLock<()>>,
    pub maintenance: AtomicBool,
}
impl Default for ManagerUpdates {
    fn default() -> Self {
        Self {
            serial: Mutex::new(()),
            mutations: Arc::new(RwLock::new(())),
            maintenance: AtomicBool::new(false),
        }
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Policy {
    pub automatic: bool,
    pub check_interval_hours: u64,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            automatic: false,
            check_interval_hours: 6,
        }
    }
}
impl Policy {
    fn validate(&self) -> Result<(), String> {
        if !(1..=168).contains(&self.check_interval_hours) {
            return Err("Check interval must be 1–168 hours".into());
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct Release {
    version: String,
    url: String,
    binary_url: String,
    checksum_url: String,
}
#[derive(Serialize, Deserialize, Default)]
struct Check {
    checked_at: u64,
    release: Option<Release>,
    error: Option<String>,
}
fn version_parts(version: &str) -> Option<(u64, u64, u64)> {
    let parts: Vec<_> = version
        .strip_prefix('v')
        .unwrap_or(version)
        .split('.')
        .collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ))
}
fn parse_release(value: &Value) -> Result<Release, String> {
    let version = value["tag_name"].as_str().ok_or("Release tag missing")?;
    if !version.starts_with('v')
        || version_parts(version).is_none()
        || value["draft"] != false
        || value["prerelease"] != false
    {
        return Err("Only published stable releases are eligible".into());
    }
    let assets = value["assets"].as_array().ok_or("Release assets missing")?;
    let asset = |name: &str, limit: u64| -> Result<String, String> {
        let item = assets
            .iter()
            .find(|a| a["name"] == name)
            .ok_or_else(|| format!("Release is missing {name}"))?;
        let expected = format!("{REPOSITORY}/releases/download/{version}/{name}");
        if item["state"] != "uploaded"
            || item["browser_download_url"] != expected
            || !item["size"].as_u64().is_some_and(|n| n > 0 && n <= limit)
        {
            return Err("Release asset has an invalid origin, state or size".into());
        }
        Ok(expected)
    };
    Ok(Release {
        version: version.into(),
        url: format!("{REPOSITORY}/releases/tag/{version}"),
        binary_url: asset("hostable-linux-amd64", 128 * 1024 * 1024)?,
        checksum_url: asset("hostable-linux-amd64.sha256", 4096)?,
    })
}
async fn fetch_release(client: &reqwest::Client, url: &str) -> Result<Release, String> {
    let mut response = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if data.len() + chunk.len() > 1024 * 1024 {
            return Err("Release metadata exceeds 1 MiB".into());
        }
        data.extend_from_slice(&chunk);
    }
    parse_release(&serde_json::from_slice(&data).map_err(|e| e.to_string())?)
}
async fn policy(state: &crate::AppState) -> Result<Policy, String> {
    state
        .db
        .as_ref()
        .ok_or("Metadata unavailable")?
        .get_record("manager_update", "policy")
        .await?
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| e.to_string())
        .map(|v| v.unwrap_or_default())
}
async fn saved_check(state: &crate::AppState) -> Result<Check, String> {
    state
        .db
        .as_ref()
        .ok_or("Metadata unavailable")?
        .get_record("manager_update", "check")
        .await?
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| e.to_string())
        .map(|v| v.unwrap_or_default())
}
fn operation() -> Value {
    std::fs::read(crate::runtime::data_dir().join("updates/operation.json"))
        .ok()
        .and_then(|v| serde_json::from_slice(&v).ok())
        .unwrap_or(Value::Null)
}
fn pending(value: &Value) -> bool {
    matches!(
        value["phase"].as_str(),
        Some("queued" | "applying" | "interrupted")
    )
}
fn unavailable_reason() -> Option<String> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Some("Automatic installation requires the Linux amd64 manager service".into());
    }
    if std::env::var("HOSTABLE_SELF_UPDATE_ENABLED").as_deref() != Ok("true") {
        return Some("Enable the installed service updater with install_in_lxc.sh; source and Docker installs update through their deployment tools".into());
    }
    if std::env::current_exe().ok().as_deref() != Some(Path::new("/usr/local/bin/hostable")) {
        return Some(
            "Only /usr/local/bin/hostable service installations can update themselves".into(),
        );
    }
    if !Path::new("/run/systemd/system").is_dir() && !Path::new("/etc/init.d/hostable").is_file() {
        return Some("A systemd or OpenRC Hostable service is required".into());
    }
    let root = crate::runtime::data_dir();
    let url = std::env::var("DATABASE_URL").unwrap_or_default();
    let database = url
        .strip_prefix("sqlite://")
        .map(|p| Path::new(p.split('?').next().unwrap_or("")));
    if !database
        .and_then(|p| p.canonicalize().ok())
        .is_some_and(|p| root.canonicalize().is_ok_and(|r| p.starts_with(r)))
    {
        return Some("Self-updates require SQLite metadata inside the manager data directory for rollback; upgrade PostgreSQL metadata installations manually".into());
    }
    None
}
async fn view(state: &crate::AppState) -> Result<Value, String> {
    let check = saved_check(state).await?;
    let available = check
        .release
        .as_ref()
        .is_some_and(|r| version_parts(&r.version) > version_parts(VERSION));
    let reason = unavailable_reason();
    Ok(
        json!({"current_version":VERSION,"commit":COMMIT,"policy":policy(state).await?,"checked_at":check.checked_at,"latest":check.release,"error":check.error,"update_available":available,"can_install":reason.is_none(),"unavailable_reason":reason,"operation":operation(),"maintenance":state.manager_updates.maintenance.load(Ordering::Acquire)}),
    )
}
async fn check(state: &crate::AppState) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent("Hostable-updater")
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let result = fetch_release(&client, API).await;
    let record = match result {
        Ok(release) => Check {
            checked_at: crate::runtime::now(),
            release: Some(release),
            error: None,
        },
        Err(error) => Check {
            checked_at: crate::runtime::now(),
            release: None,
            error: Some(error),
        },
    };
    state
        .db
        .as_ref()
        .ok_or("Metadata unavailable")?
        .put_record("manager_update", "check", &json!(record))
        .await
}
pub async fn status(
    _auth: crate::RequireAuth,
    State(state): State<Arc<crate::AppState>>,
) -> Result<Json<Value>, ApiError> {
    view(&state).await.map(Json).map_err(error)
}
pub async fn check_now(
    _auth: crate::RequireAuth,
    State(state): State<Arc<crate::AppState>>,
) -> Result<Json<Value>, ApiError> {
    let _guard = state
        .manager_updates
        .serial
        .try_lock()
        .map_err(|_| error("An update check is already running".into()))?;
    check(&state).await.map_err(error)?;
    view(&state).await.map(Json).map_err(error)
}
pub async fn save_policy(
    _auth: crate::RequireAuth,
    State(state): State<Arc<crate::AppState>>,
    Json(value): Json<Policy>,
) -> Result<Json<Value>, ApiError> {
    value.validate().map_err(error)?;
    if value.automatic {
        if let Some(reason) = unavailable_reason() {
            return Err(error(reason));
        }
    }
    state
        .db
        .as_ref()
        .ok_or_else(|| error("Metadata unavailable".into()))?
        .put_record("manager_update", "policy", &json!(value))
        .await
        .map_err(error)?;
    view(&state).await.map(Json).map_err(error)
}
#[derive(Deserialize)]
pub struct Apply {
    version: String,
}
pub async fn install(
    _auth: crate::RequireAuth,
    State(state): State<Arc<crate::AppState>>,
    Json(value): Json<Apply>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    apply(&state, &value.version).await.map_err(error)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"status":"queued","version":value.version})),
    ))
}
async fn apply(state: &crate::AppState, version: &str) -> Result<(), String> {
    let _serial = state
        .manager_updates
        .serial
        .try_lock()
        .map_err(|_| "Another manager update is running")?;
    if let Some(reason) = unavailable_reason() {
        return Err(reason);
    }
    if pending(&operation()) {
        return Err("Inspect the previous manager update before starting another".into());
    }
    check(state).await?;
    let release = saved_check(state)
        .await?
        .release
        .ok_or("No verified release is available")?;
    if release.version != version || version_parts(version) <= version_parts(VERSION) {
        return Err("The reviewed release changed or is not newer; check updates again".into());
    }
    let _mutations = state
        .manager_updates
        .mutations
        .clone()
        .try_write_owned()
        .map_err(|_| "Wait for current API changes to finish")?;
    state.ansible.begin_maintenance().await?;
    state
        .manager_updates
        .maintenance
        .store(true, Ordering::Release);
    let result = stage_and_dispatch(&release).await;
    if let Err(message) = result {
        state
            .manager_updates
            .maintenance
            .store(false, Ordering::Release);
        state.ansible.end_maintenance().await;
        return Err(message);
    }
    Ok(())
}
async fn stage_and_dispatch(release: &Release) -> Result<(), String> {
    let root = crate::runtime::data_dir()
        .canonicalize()
        .map_err(|_| "Manager data directory is unavailable")?;
    let updates = root.join("updates");
    let stage = updates.join(crate::runtime::id("update"));
    crate::runtime::private_dir(&stage)?;
    let helper = updates.join("worker.py");
    crate::runtime::write_secret(&helper, include_bytes!("../../scripts/update_manager.py"))?;
    let port: u16 = std::env::var("PORT")
        .unwrap_or_else(|_| "3000".into())
        .parse()
        .map_err(|_| "Invalid manager port")?;
    let request = stage.join("request.json");
    crate::runtime::write_secret(&request, json!({"version":release.version,"current_version":VERSION,"stage":stage,"port":port,"binary_url":release.binary_url,"checksum_url":release.checksum_url}).to_string().as_bytes())?;
    let result = tokio::time::timeout(
        Duration::from_secs(180),
        tokio::process::Command::new("python3")
            .arg(&helper)
            .arg("stage")
            .arg(&request)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "Release preparation timed out")?
    .map_err(|_| "Cannot launch Python update worker")?;
    if !result.status.success() {
        return Err("Release download, checksum or binary validation failed; inspect the retained staging directory".into());
    }
    crate::runtime::write_secret(&updates.join("operation.json"), json!({"phase":"queued","version":release.version,"stage":stage,"updated_at":crate::runtime::now(),"message":"Release verified; manager restart scheduled"}).to_string().as_bytes())?;
    let mut command;
    if Path::new("/run/systemd/system").is_dir() {
        command = tokio::process::Command::new("systemd-run");
        command
            .arg("--collect")
            .arg(format!(
                "--unit=hostable-{}",
                stage.file_name().unwrap().to_string_lossy()
            ))
            .arg("--on-active=5s")
            .arg("--timer-property=AccuracySec=1s")
            .arg("python3");
    } else {
        command = tokio::process::Command::new("setsid");
        command.arg("--fork").arg("python3");
    }
    let success = command
        .arg(helper)
        .arg("apply")
        .arg(request)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|status| status.success())
        .unwrap_or(false);
    if !success {
        crate::runtime::write_secret(&updates.join("operation.json"), json!({"phase":"failed","version":release.version,"stage":stage,"updated_at":crate::runtime::now(),"message":"Could not schedule update worker; current service remains unchanged"}).to_string().as_bytes())?;
        return Err("Could not schedule update worker".into());
    }
    Ok(())
}

pub async fn initialize(state: &crate::AppState) -> Result<(), String> {
    if pending(&operation()) && unavailable_reason().is_none() {
        state.ansible.begin_maintenance().await?;
        state
            .manager_updates
            .maintenance
            .store(true, Ordering::Release);
    }
    Ok(())
}
pub fn start_scheduler(state: Arc<crate::AppState>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let record = operation();
            if state.manager_updates.maintenance.load(Ordering::Acquire) {
                if matches!(record["phase"].as_str(), Some("succeeded" | "failed")) {
                    state.ansible.end_maintenance().await;
                    state
                        .manager_updates
                        .maintenance
                        .store(false, Ordering::Release);
                }
                continue;
            }
            if unavailable_reason().is_some() {
                continue;
            }
            let Ok(policy) = policy(&state).await else {
                continue;
            };
            let Ok(previous) = saved_check(&state).await else {
                continue;
            };
            if crate::runtime::now().saturating_sub(previous.checked_at)
                < policy.check_interval_hours * 3600
            {
                continue;
            }
            {
                let Ok(_guard) = state.manager_updates.serial.try_lock() else {
                    continue;
                };
                if check(&state).await.is_err() {
                    continue;
                }
            }
            if policy.automatic && !pending(&record) {
                if let Ok(Check {
                    release: Some(release),
                    ..
                }) = saved_check(&state).await
                {
                    if version_parts(&release.version) > version_parts(VERSION)
                        && record["version"] != release.version
                    {
                        if let Err(error) = apply(&state, &release.version).await {
                            tracing::warn!("Automatic manager update deferred: {error}");
                        }
                    }
                }
            }
        }
    });
}

pub async fn guard_mutations(
    State(state): State<Arc<crate::AppState>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if matches!(
        *request.method(),
        axum::http::Method::POST | axum::http::Method::PUT | axum::http::Method::DELETE
    ) && request.uri().path() != "/api/manager/updates/install"
    {
        let _guard = state.manager_updates.mutations.clone().read_owned().await;
        if state.manager_updates.maintenance.load(Ordering::Acquire) {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Manager update in progress; changes are paused",
            )
                .into_response();
        }
        return next.run(request).await;
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn update_policy_has_bounded_checks_and_requires_opt_in() {
        assert!(!Policy::default().automatic);
        assert!(Policy::default().validate().is_ok());
        assert!(
            Policy {
                automatic: false,
                check_interval_hours: 0
            }
            .validate()
            .is_err()
        );
        assert!(
            Policy {
                automatic: true,
                check_interval_hours: 169
            }
            .validate()
            .is_err()
        );
    }
    fn fixture() -> Value {
        json!({"tag_name":"v1.5.20","draft":false,"prerelease":false,"assets":[{"name":"hostable-linux-amd64","state":"uploaded","size":120,"browser_download_url":format!("{REPOSITORY}/releases/download/v1.5.20/hostable-linux-amd64")},{"name":"hostable-linux-amd64.sha256","state":"uploaded","size":86,"browser_download_url":format!("{REPOSITORY}/releases/download/v1.5.20/hostable-linux-amd64.sha256")}]})
    }
    #[test]
    fn releases_require_stable_versions_complete_assets_and_repository_origin() {
        assert!(parse_release(&fixture()).is_ok());
        for key in ["draft", "prerelease"] {
            let mut value = fixture();
            value[key] = json!(true);
            assert!(parse_release(&value).is_err());
        }
        let mut value = fixture();
        value["assets"][0]["browser_download_url"] = json!("https://example.invalid/binary");
        assert!(parse_release(&value).is_err());
        let mut value = fixture();
        value["assets"] = json!([]);
        assert!(parse_release(&value).is_err());
        assert!(version_parts("v1.5.20") > version_parts("v1.5.9"));
        assert!(version_parts("v1.5.0-rc.1").is_none());
    }
    #[tokio::test]
    async fn release_metadata_is_fetched_through_the_protocol_and_validated() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/release", listener.local_addr().unwrap());
        let body = fixture().to_string();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 2048];
            stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        assert_eq!(
            fetch_release(&reqwest::Client::new(), &url)
                .await
                .unwrap()
                .version,
            "v1.5.20"
        );
        server.await.unwrap();
    }
}
