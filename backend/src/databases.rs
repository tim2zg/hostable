use crate::{AppState, RequireAuth, runtime};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use rand::distr::{Alphanumeric, SampleString};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sqlx::{
    PgPool,
    postgres::{PgConnectOptions, PgPoolOptions, PgSslMode},
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, OnceLock},
};

#[derive(Debug)]
pub struct ApiError(StatusCode, String);
impl From<String> for ApiError {
    fn from(e: String) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, e)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"status":"error","error":self.1}))).into_response()
    }
}
type ApiResult = Result<Json<Value>, ApiError>;
fn bad(s: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, s.into())
}
fn meta(state: &AppState) -> Result<&crate::db::DbBackend, String> {
    state
        .db
        .as_deref()
        .ok_or_else(|| "Platform metadata unavailable".into())
}
async fn load<T: DeserializeOwned>(state: &AppState, kind: &str, id: &str) -> Result<T, String> {
    let value = meta(state)?
        .get_record(kind, id)
        .await?
        .ok_or_else(|| format!("Unknown {}", kind))?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}
fn password() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), 48)
}
pub fn sql_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !s.starts_with("pg_")
        && !s.starts_with("hostable_")
        && s != "postgres"
}
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}
fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
pub fn valid_cidr(s: &str) -> bool {
    let Some((ip, prefix)) = s.split_once('/') else {
        return false;
    };
    let Ok(ip) = ip.parse::<std::net::IpAddr>() else {
        return false;
    };
    let Ok(prefix) = prefix.parse::<u8>() else {
        return false;
    };
    prefix <= if ip.is_ipv4() { 32 } else { 128 }
}
fn validate_access(cidrs: &[String]) -> Result<(), String> {
    if cidrs.is_empty() || cidrs.len() > 32 || cidrs.iter().any(|c| !valid_cidr(c)) {
        Err("Supply 1 to 32 valid client CIDR ranges".into())
    } else {
        Ok(())
    }
}
fn secret_path(id: &str) -> PathBuf {
    runtime::data_dir()
        .join("secrets")
        .join(format!("{}.json", id))
}
fn certificate(id: &str) -> PathBuf {
    runtime::data_dir()
        .join("certificates")
        .join(format!("{}.crt", id))
}
fn secret(id: &str) -> Result<String, String> {
    let value: Value = serde_json::from_slice(
        &std::fs::read(secret_path(id)).map_err(|_| "Credential file is unavailable")?,
    )
    .map_err(|_| "Invalid credential file")?;
    value["password"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "Missing credential".into())
}
fn set_secret(id: &str, value: &str) -> Result<(), String> {
    runtime::write_secret(
        &secret_path(id),
        json!({"password":value}).to_string().as_bytes(),
    )
}
static LOCKS: OnceLock<tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>> =
    OnceLock::new();
async fn lock(id: &str) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
    let semaphore = LOCKS
        .get_or_init(Default::default)
        .lock()
        .await
        .entry(id.into())
        .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(1)))
        .clone();
    semaphore.acquire_owned().await.map_err(|e| e.to_string())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub name: String,
    pub node: String,
    pub vmid: u32,
    pub host: String,
    pub port: u16,
    pub status: String,
    pub created_at: u64,
    pub task_id: String,
    pub storage: String,
    pub data_size_gb: u32,
    pub data_volume: Option<String>,
    pub allowed_cidrs: Vec<String>,
    pub manager_cidr: String,
    pub backup_interval_hours: u32,
    pub retention_count: usize,
    pub postgres_version: Option<String>,
    pub error: Option<String>,
    #[serde(default = "legacy_tls")]
    pub tls_mode: String,
}
fn legacy_tls() -> String {
    "legacy_self_signed".into()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApplicationDatabase {
    #[serde(default = "application_ready")]
    pub status: String,
    pub id: String,
    pub instance_id: String,
    pub name: String,
    pub username: String,
    pub created_at: u64,
    pub last_backup_at: Option<u64>,
    pub last_backup_error: Option<String>,
    #[serde(default)]
    pub credentials_rotated_at: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Backup {
    pub id: String,
    pub app_id: String,
    pub instance_id: String,
    pub created_at: u64,
    pub filename: String,
    pub bytes: u64,
    pub status: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub tables: Vec<TableCount>,
    #[serde(default)]
    pub encrypted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableCount {
    pub schema: String,
    pub name: String,
    pub rows: i64,
}
#[derive(Deserialize)]
pub struct CreateInstance {
    pub name: String,
    pub node: Option<String>,
    pub vmid: Option<u32>,
    pub ostemplate: String,
    pub storage: String,
    pub bridge: String,
    pub ip_address: String,
    pub gateway: Option<String>,
    pub data_size_gb: u32,
    pub allowed_cidrs: Vec<String>,
    #[serde(default = "backup_interval")]
    pub backup_interval_hours: u32,
    #[serde(default = "retention")]
    pub retention_count: usize,
}
fn application_ready() -> String {
    "ready".into()
}
fn backup_interval() -> u32 {
    24
}
fn retention() -> usize {
    7
}
#[derive(Deserialize)]
pub struct CreateApplication {
    pub name: String,
    pub username: String,
}
#[derive(Deserialize)]
pub struct AccessUpdate {
    pub allowed_cidrs: Vec<String>,
    pub backup_interval_hours: u32,
    pub retention_count: usize,
}
#[derive(Deserialize)]
pub struct RestoreRequest {
    pub target_instance_id: String,
    pub name: String,
    pub username: String,
}

fn public_key(path: &std::path::Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".pub");
    PathBuf::from(name)
}
async fn ssh_key() -> Result<PathBuf, String> {
    let _lock = lock("guest-key").await?;
    if let Some(path) = std::env::var_os("HOSTABLE_GUEST_SSH_KEY") {
        let path =
            std::fs::canonicalize(path).map_err(|_| "HOSTABLE_GUEST_SSH_KEY is unavailable")?;
        if !public_key(&path).exists() {
            return Err("SSH public key companion (.pub) is missing".into());
        }
        return Ok(path);
    }
    let path = runtime::data_dir().join("secrets").join("guest_ed25519");
    runtime::private_dir(path.parent().ok_or("Invalid SSH key path")?)?;
    if !path.exists() {
        let result = tokio::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", ""])
            .arg("-f")
            .arg(&path)
            .output()
            .await
            .map_err(|e| format!("ssh-keygen is required: {}", e))?;
        if !result.status.success() {
            return Err("Could not generate guest management key".into());
        }
    }
    Ok(path)
}
async fn configure(instance: &Instance, bootstrap: &str) -> Result<(), String> {
    write_tls_extensions(instance)?;
    let key = ssh_key().await?;
    let hosts = std::env::var_os("HOSTABLE_GUEST_KNOWN_HOSTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            runtime::data_dir()
                .join("secrets")
                .join(format!("{}_known_hosts", instance.id))
        });
    runtime::private_dir(
        certificate(&instance.id)
            .parent()
            .ok_or("Missing certificate directory")?,
    )?;
    let policy = if std::env::var_os("HOSTABLE_GUEST_KNOWN_HOSTS").is_some() {
        "yes"
    } else {
        "accept-new"
    };
    runtime::ansible("database.yml", &json!({
        "guest_ip": instance.host, "ssh_key": key, "known_hosts": hosts, "host_key_policy": policy,
        "manager_cidr": instance.manager_cidr, "allowed_cidrs": instance.allowed_cidrs,
        "bootstrap_sql": bootstrap, "certificate_path": certificate(&instance.id),
        "managed_ca": instance.tls_mode == "managed_ca", "ca_cert":certificate(&instance.id),
        "ca_key":ca_key(&instance.id), "csr_path":secret_path(&format!("{}_csr",instance.id)),
        "leaf_path":certificate(&format!("{}_leaf",instance.id)),
        "extension_path":tls_extension_path(&instance.id),
    })).await
}
async fn pool_as(
    instance: &Instance,
    user: &str,
    password: &str,
    database: &str,
) -> Result<PgPool, String> {
    let options = PgConnectOptions::new()
        .host(&instance.host)
        .port(instance.port)
        .username(user)
        .password(password)
        .database(database)
        .ssl_mode(PgSslMode::VerifyFull)
        .ssl_root_cert(certificate(&instance.id))
        .application_name("hostable");
    tokio::time::timeout(
        std::time::Duration::from_secs(8),
        PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect_with(options),
    )
    .await
    .map_err(|_| "Database connection timed out".to_string())?
    .map_err(|_| {
        "Database connection failed; check access rules, TLS certificate and service status".into()
    })
}
async fn control_pool(instance: &Instance) -> Result<PgPool, String> {
    pool_as(
        instance,
        "hostable_control",
        &secret(&instance.id)?,
        "postgres",
    )
    .await
}
fn require_ready(instance: &Instance) -> Result<(), String> {
    if instance.status == "ready" {
        Ok(())
    } else {
        Err("Database instance is not ready".into())
    }
}

pub async fn list(_auth: RequireAuth, State(state): State<Arc<AppState>>) -> ApiResult {
    Ok(Json(json!(meta(&state)?.list_records("database").await?)))
}
pub async fn list_apps(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult {
    let _: Instance = load(&state, "database", &id).await?;
    let rows = meta(&state)?
        .list_records("app_database")
        .await?
        .into_iter()
        .filter(|r| r["instance_id"] == id)
        .collect::<Vec<_>>();
    Ok(Json(json!(rows)))
}
pub async fn get_instance(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult {
    let instance: Instance = load(&state, "database", &id).await?;
    let mut value = json!(instance);
    value["connection_ready"] = json!(if instance.status == "ready" {
        control_pool(&instance).await.is_ok()
    } else {
        false
    });
    Ok(Json(value))
}
#[derive(Deserialize)]
pub struct TemplateQuery {
    node: Option<String>,
}
pub async fn templates(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Query(query): Query<TemplateQuery>,
) -> ApiResult {
    let node = query.node.unwrap_or_else(|| state.default_node.clone());
    if !crate::ansible::identifier(&node) {
        return Err(bad("Invalid node"));
    }
    let storages = state.proxmox.get_storages(&node).await?;
    let mut templates = vec![];
    if let Some(rows) = storages["data"].as_array() {
        for storage in rows.iter().filter(|s| {
            s["content"]
                .as_str()
                .unwrap_or("")
                .split(',')
                .any(|s| s == "vztmpl")
        }) {
            let name = storage["storage"]
                .as_str()
                .ok_or_else(|| bad("Invalid storage response"))?;
            let list = state.proxmox.get_templates(&node, name).await?;
            if let Some(items) = list["data"].as_array() {
                templates.extend(items.iter().cloned());
            }
        }
    }
    Ok(Json(json!(templates)))
}
pub async fn create(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateInstance>,
) -> Result<Response, ApiError> {
    let file = request.ostemplate.split(":vztmpl/").nth(1).unwrap_or("");
    if !(file.starts_with("debian-12-") || file.starts_with("debian-13-")) {
        return Err(bad(
            "Database hosting requires a Debian 12 or 13 standard OS template",
        ));
    }
    if !crate::proxmox::valid_hostname(&request.name) {
        return Err(bad("Invalid instance name"));
    }
    crate::ansible::validate_ip(&request.ip_address).map_err(bad)?;
    if request.ip_address == "dhcp" {
        return Err(bad(
            "Database instances require a stable static address with CIDR",
        ));
    }
    validate_access(&request.allowed_cidrs).map_err(bad)?;
    if !(1..=65536).contains(&request.data_size_gb)
        || request.retention_count == 0
        || request.retention_count > 100
        || request.backup_interval_hours > 8760
    {
        return Err(bad("Invalid data size, retention or backup interval"));
    }
    let manager = std::env::var("HOSTABLE_MANAGER_CIDR").unwrap_or_else(|_| {
        local_ip_address::local_ip()
            .map(|ip| format!("{}/{}", ip, if ip.is_ipv4() { 32 } else { 128 }))
            .unwrap_or_default()
    });
    if !valid_cidr(&manager) {
        return Err(bad(
            "Configure HOSTABLE_MANAGER_CIDR with the manager's reachable address",
        ));
    }
    let key = ssh_key().await?;
    let public_key = std::fs::read_to_string(public_key(&key)).map_err(|e| e.to_string())?;
    let params: crate::ansible::AnsibleDeployParams = serde_json::from_value(json!({
        "vmid": request.vmid.unwrap_or(0), "node": request.node, "hostname":request.name, "ostemplate":request.ostemplate,
        "rootfs_storage":request.storage, "net_bridge":request.bridge, "ip_address":request.ip_address, "gateway":request.gateway,
        "cores":2, "memory":1024, "disk_size":"4G", "ssh_public_key":public_key.trim(),
        "mountpoints":[{"storage":request.storage,"size_gb":request.data_size_gb,"container":"/var/lib/hostable-postgres"}],
    })).map_err(|e| bad(e.to_string()))?;
    crate::ansible::validate_deploy_params(&params).map_err(bad)?;
    let _lock = lock("database-create").await?;
    if meta(&state)?
        .list_records("database")
        .await?
        .iter()
        .any(|r| {
            r["name"] == request.name
                || r["host"] == request.ip_address.split('/').next().unwrap_or("")
        })
    {
        return Err(bad("Instance name or address already registered"));
    }
    let id = runtime::id("db");
    set_secret(&id, &password())?;
    let task = state
        .ansible
        .create("database-create", &request.name)
        .await?;
    let instance = Instance {
        id: id.clone(),
        name: request.name,
        node: params
            .node
            .clone()
            .unwrap_or_else(|| state.default_node.clone()),
        vmid: params.vmid,
        host: params.ip_address.split('/').next().unwrap_or("").into(),
        port: 5432,
        status: "provisioning".into(),
        created_at: runtime::now(),
        task_id: task.clone(),
        storage: request.storage,
        data_size_gb: request.data_size_gb,
        data_volume: None,
        allowed_cidrs: request.allowed_cidrs,
        manager_cidr: manager,
        backup_interval_hours: request.backup_interval_hours,
        retention_count: request.retention_count,
        postgres_version: None,
        error: None,
        tls_mode: "managed_ca".into(),
    };
    ensure_ca(&instance).await?;
    meta(&state)?
        .put_record("database", &id, &json!(instance))
        .await?;
    tokio::spawn(provision(state.clone(), task.clone(), instance, params));
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"id":id,"task_id":task,"status":"provisioning"})),
    )
        .into_response())
}
async fn provision(
    state: Arc<AppState>,
    task: String,
    mut instance: Instance,
    mut params: crate::ansible::AnsibleDeployParams,
) {
    let result: Result<(), String> = async {
        let deployed = crate::ansible::deploy(&state, &task, &mut params).await?;
        instance.vmid = deployed.vmid; instance.node = deployed.node; instance.host = deployed.ip;
        meta(&state)?.put_record("database", &instance.id, &json!(instance)).await?;
        state.ansible.event(&task,"task","DATABASE_SETUP","Installing PostgreSQL and configuring persistent storage and TLS").await?;
        let bootstrap = format!("CREATE ROLE hostable_apps NOLOGIN;\nCREATE ROLE hostable_control LOGIN SUPERUSER PASSWORD {};\n", literal(&secret(&instance.id)?));
        configure(&instance, &bootstrap).await?;
        let pool = control_pool(&instance).await?;
        let version: (String,) = sqlx::query_as("SHOW server_version").fetch_one(&pool).await.map_err(|e| e.to_string())?;
        instance.postgres_version = Some(version.0);
        let config = state.proxmox.get_lxc_config(&instance.node, instance.vmid).await?;
        instance.data_volume = config["data"]["mp0"].as_str().and_then(|s| s.split(',').next()).map(str::to_owned);
        instance.status = "ready".into();
        meta(&state)?.put_record("database", &instance.id, &json!(instance)).await?;
        state.ansible.event(&task,"ok","COMPLETE","PostgreSQL is ready. Create an application database to obtain credentials.").await?;
        Ok(())
    }.await;
    if let Err(error) = result {
        instance.status = "failed".into();
        instance.error = Some(error.clone());
        instance.vmid = params.vmid;
        if let Ok(db) = meta(&state) {
            let _ = db
                .put_record("database", &instance.id, &json!(instance))
                .await;
        }
        let _ = state.ansible.event(&task, "error", "FAILED", &error).await;
    }
}
async fn add_app(
    state: &AppState,
    instance: &Instance,
    name: &str,
    username: &str,
) -> Result<ApplicationDatabase, String> {
    require_ready(instance)?;
    if !sql_name(name) || !sql_name(username) {
        return Err(
            "Database and username must be lowercase SQL identifiers without reserved prefixes"
                .into(),
        );
    }
    let pool = control_pool(instance).await?;
    let exists: (bool,) = sqlx::query_as("SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname=$1) OR EXISTS(SELECT 1 FROM pg_roles WHERE rolname=$2)").bind(name).bind(username).fetch_one(&pool).await.map_err(|e| e.to_string())?;
    if exists.0 {
        return Err("Database or role already exists; existing data is never overwritten".into());
    }
    let app = ApplicationDatabase {
        status: "creating".into(),
        id: runtime::id("appdb"),
        instance_id: instance.id.clone(),
        name: name.into(),
        username: username.into(),
        created_at: runtime::now(),
        last_backup_at: None,
        last_backup_error: None,
        credentials_rotated_at: None,
    };
    let pwd = password();
    set_secret(&app.id, &pwd)?;
    meta(state)?
        .put_record("app_database", &app.id, &json!(app))
        .await?;
    sqlx::query(&format!(
        "CREATE ROLE {} LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION PASSWORD {}",
        quote(username),
        literal(&pwd)
    ))
    .execute(&pool)
    .await
    .map_err(|e| e.to_string())?;
    if let Err(e) = sqlx::query(&format!(
        "CREATE DATABASE {} OWNER {}",
        quote(name),
        quote(username)
    ))
    .execute(&pool)
    .await
    {
        let _ = sqlx::query(&format!("DROP ROLE {}", quote(username)))
            .execute(&pool)
            .await;
        return Err(e.to_string());
    }
    sqlx::query(&format!(
        "REVOKE CONNECT ON DATABASE {} FROM PUBLIC",
        quote(name)
    ))
    .execute(&pool)
    .await
    .map_err(|e| e.to_string())?;
    sqlx::query(&format!(
        "GRANT CONNECT ON DATABASE {} TO {}",
        quote(name),
        quote(username)
    ))
    .execute(&pool)
    .await
    .map_err(|e| e.to_string())?;
    sqlx::query(&format!("GRANT hostable_apps TO {}", quote(username)))
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;
    let connection = pool_as(instance, username, &pwd, name).await?;
    sqlx::query("SELECT 1")
        .execute(&connection)
        .await
        .map_err(|e| e.to_string())?;
    let app = ApplicationDatabase {
        status: "ready".into(),
        credentials_rotated_at: Some(runtime::now()),
        ..app
    };
    meta(state)?
        .put_record("app_database", &app.id, &json!(app))
        .await?;
    Ok(app)
}
pub async fn create_app(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<CreateApplication>,
) -> ApiResult {
    let _lock = lock(&id).await?;
    let instance: Instance = load(&state, "database", &id).await?;
    let app = add_app(&state, &instance, &request.name, &request.username)
        .await
        .map_err(bad)?;
    Ok(Json(json!(app)))
}
pub async fn application_uri(state: &AppState, id: &str) -> Result<String, String> {
    let app: ApplicationDatabase = load(state, "app_database", id).await?;
    let instance: Instance = load(state, "database", &app.instance_id).await?;
    require_ready(&instance)?;
    if app.status != "ready" {
        return Err("Application database is not ready or its login was revoked".into());
    }
    let mut url = reqwest::Url::parse("postgresql://localhost").map_err(|e| e.to_string())?;
    url.set_host(Some(&instance.host))
        .map_err(|_| "Invalid database host")?;
    url.set_port(Some(instance.port))
        .map_err(|_| "Invalid database port")?;
    url.set_username(&app.username)
        .map_err(|_| "Invalid username")?;
    url.set_password(Some(&secret(&app.id)?))
        .map_err(|_| "Invalid password")?;
    url.set_path(&app.name);
    url.query_pairs_mut().append_pair("sslmode", "verify-full");
    Ok(url.to_string())
}
pub async fn credentials(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let app: ApplicationDatabase = load(&state, "app_database", &id).await?;
    let instance: Instance = load(&state, "database", &app.instance_id).await?;
    let crt = std::fs::read_to_string(certificate(&instance.id))
        .map_err(|_| "Server certificate unavailable".to_string())?;
    Ok(([(axum::http::header::CACHE_CONTROL,"no-store")], Json(json!({"host":instance.host,"port":instance.port,"database":app.name,"username":app.username,"password":secret(&id)?,"uri":application_uri(&state,&id).await?,"ca_certificate":crt,"sslmode":"verify-full"}))).into_response())
}
pub async fn rotate(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult {
    let app: ApplicationDatabase = load(&state, "app_database", &id).await?;
    if !matches!(
        app.status.as_str(),
        "ready" | "revoked" | "rotating" | "rotation_interrupted"
    ) {
        return Err(bad(
            "Resolve the incomplete database creation or restore before rotating credentials",
        ));
    }
    let _lock = lock(&app.instance_id).await?;
    let instance: Instance = load(&state, "database", &app.instance_id).await?;
    require_ready(&instance)?;
    let pool = control_pool(&instance).await?;
    let new = password();
    let pending = secret_path(&id).with_extension("rotation.json");
    runtime::write_secret(&pending, json!({"new_password":new}).to_string().as_bytes())?;
    let rotating = ApplicationDatabase {
        status: "rotating".into(),
        ..app.clone()
    };
    meta(&state)?
        .put_record("app_database", &id, &json!(rotating))
        .await?;
    if let Err(e) = sqlx::query(&format!(
        "ALTER ROLE {} LOGIN PASSWORD {}",
        quote(&app.username),
        literal(&new)
    ))
    .execute(&pool)
    .await
    {
        meta(&state)?
            .put_record("app_database", &id, &json!(app))
            .await?;
        let _ = std::fs::remove_file(&pending);
        return Err(e.to_string().into());
    }
    set_secret(&id, &new)?;
    let app = ApplicationDatabase {
        status: "ready".into(),
        credentials_rotated_at: Some(runtime::now()),
        ..app
    };
    meta(&state)?
        .put_record("app_database", &id, &json!(app))
        .await?;
    let _ = std::fs::remove_file(pending);
    let mut attached = Vec::new();
    for v in meta(&state)?.list_records("workload").await? {
        if let Ok(w) = serde_json::from_value::<crate::workloads::Workload>(v) {
            if crate::workloads::request(&w.active.spec)
                .is_ok_and(|p| p.database_id.as_deref() == Some(&id))
            {
                attached.push(w.id);
            }
        }
    }
    Ok(Json(
        json!({"status":"ok","message":"Password rotated. Refresh attached application credentials from Applications and updates; external clients must receive the new password.","attached_workloads":attached}),
    ))
}
pub async fn update_access(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<AccessUpdate>,
) -> ApiResult {
    validate_access(&request.allowed_cidrs).map_err(bad)?;
    if request.retention_count == 0
        || request.retention_count > 100
        || request.backup_interval_hours > 8760
    {
        return Err(bad("Invalid backup configuration"));
    }
    let _lock = lock(&id).await?;
    let mut instance: Instance = load(&state, "database", &id).await?;
    require_ready(&instance)?;
    instance.allowed_cidrs = request.allowed_cidrs;
    instance.backup_interval_hours = request.backup_interval_hours;
    instance.retention_count = request.retention_count;
    configure(&instance, "").await?;
    control_pool(&instance).await?;
    meta(&state)?
        .put_record("database", &id, &json!(instance))
        .await?;
    Ok(Json(json!(instance)))
}
pub fn backup_root() -> PathBuf {
    std::env::var_os("HOSTABLE_BACKUP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| runtime::data_dir().join("backups"))
}
pub async fn list_backups(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult {
    let _: ApplicationDatabase = load(&state, "app_database", &id).await?;
    Ok(Json(json!(
        meta(&state)?
            .list_records("backup")
            .await?
            .into_iter()
            .filter(|v| v["app_id"] == id)
            .collect::<Vec<_>>()
    )))
}
async fn pg_tool(
    tool: &str,
    args: &[String],
    instance: &Instance,
    app: &ApplicationDatabase,
    file: &std::path::Path,
) -> Result<(), String> {
    let pwd = secret(&app.id)?;
    let escape = |s: &str| s.replace('\\', "\\\\").replace(':', "\\:");
    let passfile = runtime::data_dir()
        .join("secrets")
        .join(format!("{}.pgpass", runtime::id("client")));
    runtime::write_secret(
        &passfile,
        format!(
            "{}:{}:{}:{}:{}\n",
            escape(&instance.host),
            instance.port,
            escape(&app.name),
            escape(&app.username),
            escape(&pwd)
        )
        .as_bytes(),
    )?;
    let mut cmd = tokio::process::Command::new(tool);
    cmd.kill_on_drop(true)
        .args(args)
        .env_remove("PGPASSWORD")
        .env_remove("PGSERVICE")
        .env_remove("PGSERVICEFILE")
        .env("PGHOST", &instance.host)
        .env("PGPORT", instance.port.to_string())
        .env("PGUSER", &app.username)
        .env("PGDATABASE", &app.name)
        .env("PGPASSFILE", &passfile)
        .env("PGSSLMODE", "verify-full")
        .env("PGSSLROOTCERT", certificate(&instance.id))
        .env("PGCONNECT_TIMEOUT", "10");
    if tool == "pg_dump" {
        cmd.arg("--file").arg(file);
    } else {
        cmd.arg("--dbname").arg(&app.name).arg(file);
    }
    let result = tokio::time::timeout(std::time::Duration::from_secs(3600), cmd.output()).await;
    let _ = std::fs::remove_file(passfile);
    match result {
        Ok(Ok(output)) if output.status.success() => Ok(()),
        Ok(Ok(_)) => Err(format!(
            "{} failed. Check client/server versions, connectivity and free disk space.",
            tool
        )),
        Ok(Err(e)) => Err(format!(
            "{} is required on the Hostable manager: {}",
            tool, e
        )),
        Err(_) => Err(format!("{} exceeded the operation timeout", tool)),
    }
}
async fn run_backup(state: Arc<AppState>, id: String, task: String) -> Result<(), String> {
    let mut app: ApplicationDatabase = load(&state, "app_database", &id).await?;
    let _lock = lock(&app.instance_id).await?;
    let instance: Instance = load(&state, "database", &app.instance_id).await?;
    require_ready(&instance)?;
    if app.status != "ready" {
        return Err("Application database is not ready for backup".into());
    }
    state
        .ansible
        .event(
            &task,
            "task",
            "BACKUP",
            "Creating a consistent logical backup",
        )
        .await?;
    let root = backup_root();
    runtime::private_dir(&root)?;
    let backup_id = runtime::id("backup");
    let encrypted = std::env::var("HOSTABLE_BACKUP_GPG_RECIPIENT").is_ok_and(|v| !v.is_empty());
    let filename = format!("{}.dump{}", backup_id, if encrypted { ".gpg" } else { "" });
    let spool = runtime::data_dir().join("cache").join(&backup_id);
    let _spool_guard = if encrypted {
        runtime::private_dir(&spool)?;
        Some(runtime::CacheGuard(spool.clone()))
    } else {
        None
    };
    let path = if encrypted {
        spool.join("backup.dump")
    } else {
        root.join(&filename)
    };
    // Precreate with restrictive permissions; pg_dump truncates this same file.
    runtime::write_secret(&path, b"")?;
    let pool = pool_as(&instance, &app.username, &secret(&app.id)?, &app.name).await?;
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    let snapshot: (String,) = sqlx::query_as("SELECT pg_export_snapshot()")
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    let names: Vec<(String,String)> = sqlx::query_as("SELECT n.nspname,c.relname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.relkind IN ('r','p') AND n.nspname NOT LIKE 'pg_%' AND n.nspname <> 'information_schema' ORDER BY 1,2").fetch_all(&mut *tx).await.map_err(|e| e.to_string())?;
    let mut tables = Vec::new();
    for (schema, name) in names {
        let count: (i64,) = sqlx::query_as(&format!(
            "SELECT COUNT(*) FROM {}.{}",
            quote(&schema),
            quote(&name)
        ))
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        tables.push(TableCount {
            schema,
            name,
            rows: count.0,
        });
    }
    pg_tool(
        "pg_dump",
        &[
            format!("--snapshot={}", snapshot.0),
            "--format=custom".into(),
            "--no-owner".into(),
            "--no-acl".into(),
        ],
        &instance,
        &app,
        &path,
    )
    .await?;
    tx.commit().await.map_err(|e| e.to_string())?;
    let stored = root.join(&filename);
    if encrypted {
        crate::recovery::encrypt(&path, &stored).await?;
    }
    let record = Backup {
        id: backup_id.clone(),
        app_id: id.clone(),
        instance_id: instance.id.clone(),
        created_at: runtime::now(),
        filename,
        bytes: std::fs::metadata(&stored).map_err(|e| e.to_string())?.len(),
        status: "ready".into(),
        sha256: hash_file(&stored)?,
        tables,
        encrypted,
    };
    meta(&state)?
        .put_record("backup", &backup_id, &json!(record))
        .await?;
    app.last_backup_at = Some(runtime::now());
    app.last_backup_error = None;
    meta(&state)?
        .put_record("app_database", &id, &json!(app))
        .await?;
    let mut backups: Vec<Backup> = meta(&state)?
        .list_records("backup")
        .await?
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Backup>(v).ok())
        .filter(|b| b.app_id == id)
        .collect();
    backups.sort_by_key(|b| std::cmp::Reverse((b.created_at, b.id == backup_id)));
    for old in backups.into_iter().skip(instance.retention_count) {
        std::fs::remove_file(root.join(&old.filename)).map_err(|e| e.to_string())?;
        meta(&state)?.delete_record("backup", &old.id).await?;
    }
    state
        .ansible
        .event(
            &task,
            "ok",
            "COMPLETE",
            "Backup completed and retention applied",
        )
        .await?;
    Ok(())
}
async fn launch_backup(state: Arc<AppState>, id: String) -> Result<String, String> {
    let app: ApplicationDatabase = load(&state, "app_database", &id).await?;
    let task = state.ansible.create("backup", &id).await?;
    let task_copy = task.clone();
    tokio::spawn(async move {
        if let Err(error) = run_backup(state.clone(), id.clone(), task_copy.clone()).await {
            let _ = state
                .ansible
                .event(&task_copy, "error", "FAILED", &error)
                .await;
            if let Ok(mut app) = load::<ApplicationDatabase>(&state, "app_database", &id).await {
                app.last_backup_error = Some(error);
                if let Ok(db) = meta(&state) {
                    let _ = db.put_record("app_database", &app.id, &json!(app)).await;
                }
            }
        }
    });
    let _ = app;
    Ok(task)
}
pub async fn backup(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let task = launch_backup(state, id).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"status":"queued"})),
    )
        .into_response())
}
pub async fn restore(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<RestoreRequest>,
) -> Result<Response, ApiError> {
    let backup: Backup = load(&state, "backup", &id).await?;
    if !sql_name(&request.name) || !sql_name(&request.username) {
        return Err(bad("Invalid target database or role name"));
    }
    if backup.status != "ready" {
        return Err(bad("Backup integrity check failed; no target was created"));
    }
    let materialized = crate::recovery::materialize(&backup).await.map_err(bad)?;
    let instance: Instance = load(&state, "database", &request.target_instance_id).await?;
    require_ready(&instance)?;
    let task = state
        .ansible
        .create("restore", &request.target_instance_id)
        .await?;
    let copy = task.clone();
    tokio::spawn(async move {
        let result: Result<String, String> = async {
            let _lock = lock(&instance.id).await?;
            state
                .ansible
                .event(
                    &copy,
                    "task",
                    "RESTORE",
                    "Creating a new database for restore; existing databases are preserved",
                )
                .await?;
            let mut app = add_app(&state, &instance, &request.name, &request.username).await?;
            app.status = "restoring".into();
            meta(&state)?
                .put_record("app_database", &app.id, &json!(app))
                .await?;
            pg_tool(
                "pg_restore",
                &[
                    "--no-owner".into(),
                    "--no-acl".into(),
                    "--exit-on-error".into(),
                ],
                &instance,
                &app,
                &materialized.path,
            )
            .await?;
            let pool = pool_as(&instance, &app.username, &secret(&app.id)?, &app.name).await?;
            for table in &backup.tables {
                let count: (i64,) = sqlx::query_as(&format!(
                    "SELECT COUNT(*) FROM {}.{}",
                    quote(&table.schema),
                    quote(&table.name)
                ))
                .fetch_one(&pool)
                .await
                .map_err(|e| e.to_string())?;
                if count.0 != table.rows {
                    return Err(format!(
                        "Restore row count mismatch in {}.{}",
                        table.schema, table.name
                    ));
                }
            }
            app.status = "ready".into();
            meta(&state)?
                .put_record("app_database", &app.id, &json!(app))
                .await?;
            Ok(app.id)
        }
        .await;
        match result {
            Ok(id) => {
                let _ = state
                    .ansible
                    .event(
                        &copy,
                        "ok",
                        "COMPLETE",
                        &format!("Restore completed into application database {}", id),
                    )
                    .await;
            }
            Err(e) => {
                let _ = state.ansible.event(&copy, "error", "FAILED", &e).await;
            }
        }
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"status":"queued"})),
    )
        .into_response())
}
pub fn start_scheduler(state: Arc<AppState>) {
    let certificates = state.clone();
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(900));
        loop {
            timer.tick().await;
            let Ok(values) = meta(&certificates).unwrap().list_records("database").await else {
                continue;
            };
            for v in values {
                let Ok(instance) = serde_json::from_value::<Instance>(v) else {
                    continue;
                };
                if instance.status != "ready"
                    || instance.tls_mode != "managed_ca"
                    || !certificate_due(&instance).await.unwrap_or(false)
                {
                    continue;
                }
                if certificates.ansible.list().await.iter().any(|j| {
                    j.kind == "certificate_renewal"
                        && j.resource == instance.id
                        && (matches!(j.status.as_str(), "queued" | "running")
                            || j.created_at + 3600 > runtime::now())
                }) {
                    continue;
                }
                if let Err(e) = launch_renewal(certificates.clone(), instance.id).await {
                    tracing::warn!("Certificate renewal: {}", e);
                }
            }
        }
    });
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            let Ok(db) = meta(&state) else {
                continue;
            };
            let Ok(apps) = db.list_records("app_database").await else {
                continue;
            };
            for value in apps {
                let Ok(app) = serde_json::from_value::<ApplicationDatabase>(value) else {
                    continue;
                };
                let Ok(instance) = load::<Instance>(&state, "database", &app.instance_id).await
                else {
                    continue;
                };
                if app.status != "ready"
                    || instance.status != "ready"
                    || instance.backup_interval_hours == 0
                {
                    continue;
                }
                let due = app.last_backup_at.unwrap_or(app.created_at)
                    + u64::from(instance.backup_interval_hours) * 3600;
                if runtime::now() < due {
                    continue;
                }
                let jobs = state.ansible.list().await;
                if jobs.iter().any(|j| {
                    j.kind == "backup"
                        && j.resource == app.id
                        && (j.status == "running"
                            || j.status == "queued"
                            || j.created_at + 3600 > runtime::now())
                }) {
                    continue;
                }
                if let Err(e) = launch_backup(state.clone(), app.id).await {
                    tracing::error!("Cannot schedule backup: {}", e);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identifiers_and_access_cannot_inject_sql_or_configuration() {
        assert!(sql_name("my_app2"));
        assert!(!sql_name("users;DROP TABLE"));
        assert!(!sql_name("pg_catalog"));
        assert!(!sql_name("hostable_control"));
        assert!(valid_cidr("10.0.0.0/24"));
        assert!(!valid_cidr("10.0.0.0/33"));
        assert!(!valid_cidr("0.0.0.0/0\nlocal all all trust"));
        assert_eq!(literal("a'b"), "'a''b'");
        assert_eq!(quote("a\"b"), "\"a\"\"b\"");
    }
}

pub async fn revoke(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult {
    let mut app: ApplicationDatabase = load(&state, "app_database", &id).await?;
    if !matches!(app.status.as_str(), "ready" | "revoked") {
        return Err(bad(
            "Application database is incomplete; login management cannot mark it ready",
        ));
    }
    let _lock = lock(&app.instance_id).await?;
    let instance: Instance = load(&state, "database", &app.instance_id).await?;
    require_ready(&instance)?;
    let pool = control_pool(&instance).await?;
    sqlx::query(&format!("ALTER ROLE {} NOLOGIN", quote(&app.username)))
        .execute(&pool)
        .await
        .map_err(|e| e.to_string())?;
    // Existing sessions are terminated so a revoked credential cannot retain access.
    sqlx::query("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename=$1 AND pid <> pg_backend_pid()").bind(&app.username).execute(&pool).await.map_err(|e| e.to_string())?;
    app.status = "revoked".into();
    meta(&state)?
        .put_record("app_database", &id, &json!(app))
        .await?;
    Ok(Json(json!({"status":"ok"})))
}
pub async fn retire(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult {
    let _lock = lock(&id).await?;
    let mut instance: Instance = load(&state, "database", &id).await?;
    if instance.status == "retired" {
        return Ok(Json(json!(instance)));
    }
    if instance.vmid == 0 {
        return Err(bad(
            "No allocated container was recorded; inspect the provisioning job",
        ));
    }
    let node = state.proxmox.resolve_lxc_node(instance.vmid).await?;
    let config = state.proxmox.get_lxc_config(&node, instance.vmid).await?;
    if config["data"]["hostname"] != instance.name
        || config["data"]["description"] != format!("hostable.task={}", instance.task_id)
    {
        return Err(bad(
            "Guest ownership changed; refusing to stop a reused VMID",
        ));
    }
    if state.proxmox.get_lxc_status(&node, instance.vmid).await?["data"]["status"] != "stopped" {
        let task = state.proxmox.stop_lxc(&node, instance.vmid).await?;
        state.proxmox.wait_response_task(&node, task).await?;
        state
            .proxmox
            .wait_lxc_status(&node, instance.vmid, "stopped")
            .await?;
    }
    instance.status = "retired".into();
    instance.backup_interval_hours = 0;
    meta(&state)?
        .put_record("database", &id, &json!(instance))
        .await?;
    Ok(Json(json!(instance)))
}
pub async fn application_certificate(state: &AppState, id: &str) -> Result<String, String> {
    let app: ApplicationDatabase = load(state, "app_database", id).await?;
    std::fs::read_to_string(certificate(&app.instance_id))
        .map_err(|_| "Database certificate unavailable".into())
}

fn ca_key(id: &str) -> PathBuf {
    secret_path(&format!("{}_ca", id)).with_extension("key")
}
fn tls_extension_path(id: &str) -> PathBuf {
    secret_path(&format!("{}_tls", id)).with_extension("cnf")
}
fn write_tls_extensions(instance: &Instance) -> Result<(), String> {
    let ip: std::net::IpAddr = instance
        .host
        .parse()
        .map_err(|_| "Invalid database TLS address")?;
    runtime::write_secret(&tls_extension_path(&instance.id),format!("basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:{}\n",ip).as_bytes())
}
async fn ensure_ca(instance: &Instance) -> Result<(), String> {
    if instance.tls_mode != "managed_ca" || certificate(&instance.id).exists() {
        return Ok(());
    }
    runtime::private_dir(
        certificate(&instance.id)
            .parent()
            .ok_or("Invalid CA path")?,
    )?;
    runtime::private_dir(ca_key(&instance.id).parent().ok_or("Invalid CA key path")?)?;
    let output = tokio::process::Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:3072",
            "-nodes",
            "-days",
            "3650",
            "-addext",
            "basicConstraints=critical,CA:TRUE",
            "-addext",
            "keyUsage=critical,keyCertSign,cRLSign",
            "-subj",
        ])
        .arg(format!("/CN=Hostable {} CA", instance.id))
        .arg("-keyout")
        .arg(ca_key(&instance.id))
        .arg("-out")
        .arg(certificate(&instance.id))
        .output()
        .await
        .map_err(|e| format!("OpenSSL is required for database CA creation: {}", e))?;
    if !output.status.success() {
        return Err("Database CA generation failed".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(ca_key(&instance.id), std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
async fn certificate_due(instance: &Instance) -> Result<bool, String> {
    let path = if instance.tls_mode == "managed_ca" {
        certificate(&format!("{}_leaf", instance.id))
    } else {
        certificate(&instance.id)
    };
    let output = tokio::process::Command::new("openssl")
        .args(["x509", "-noout", "-checkend", "2592000", "-in"])
        .arg(path)
        .output()
        .await
        .map_err(|_| "OpenSSL certificate inspection unavailable")?;
    match output.status.code() {
        Some(0) => Ok(false),
        Some(1) => Ok(true),
        _ => Err("Cannot inspect database certificate".into()),
    }
}
async fn renew(instance: &Instance) -> Result<(), String> {
    if instance.tls_mode != "managed_ca" {
        return Err("This legacy instance uses a self-signed leaf certificate. Migrate its client trust to a managed CA before enabling automatic renewal.".into());
    }
    let key = ssh_key().await?;
    write_tls_extensions(instance)?;
    let hosts = std::env::var_os("HOSTABLE_GUEST_KNOWN_HOSTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            runtime::data_dir()
                .join("secrets")
                .join(format!("{}_known_hosts", instance.id))
        });
    runtime::ansible("certificate.yml",&json!({"guest_ip":instance.host,"ssh_key":key,"known_hosts":hosts,"host_key_policy":"yes","managed_ca":true,"force_renew":true,"ca_cert":certificate(&instance.id),"ca_key":ca_key(&instance.id),"csr_path":secret_path(&format!("{}_csr",instance.id)),"leaf_path":certificate(&format!("{}_leaf",instance.id)),"extension_path":tls_extension_path(&instance.id)})).await?;
    control_pool(instance).await?;
    Ok(())
}
pub async fn renew_certificate(
    _auth: RequireAuth,
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult {
    Ok(Json(
        json!({"status":"queued","task_id":launch_renewal(s,id).await?}),
    ))
}
async fn launch_renewal(s: Arc<AppState>, id: String) -> Result<String, String> {
    let instance: Instance = load(&s, "database", &id).await?;
    require_ready(&instance)?;
    if instance.tls_mode != "managed_ca" {
        return Err("Automatic renewal requires a managed CA instance".into());
    }
    let guard = s
        .ansible
        .lock_resource(&format!("certificate:{}", id))
        .await?;
    let job = s.ansible.create("certificate_renewal", &id).await?;
    let copy = job.clone();
    tokio::spawn(async move {
        let _guard = guard;
        let result:Result<(),String>=async{let _lock=lock(&id).await?;s.ansible.event(&copy,"task","RENEW_CERTIFICATE","Renewing the leaf certificate using the existing trusted CA").await?;renew(&instance).await?;s.ansible.event(&copy,"ok","COMPLETE","Certificate renewed and a TLS SQL connection verified; existing client trust is retained").await}.await;
        if let Err(e) = result {
            s.ansible.finish_error(&copy, &e).await;
        }
    });
    Ok(job)
}
pub async fn health_report(s: &AppState, id: &str) -> Result<Value, String> {
    let instance: Instance = load(s, "database", id).await?;
    let mut alerts = Vec::<String>::new();
    let mut report = json!({"id":id,"kind":"database","name":instance.name,"status":instance.status,"observed_at":runtime::now(),"tls_mode":instance.tls_mode,"alerts":[]});
    match certificate_due(&instance).await {
        Ok(true) => {
            alerts.push("Database TLS certificate expires within 30 days or is invalid".into())
        }
        Ok(false) => {}
        Err(e) => alerts.push(e),
    }
    if instance.status == "ready" {
        let pool = control_pool(&instance).await?;
        let counts:(i64,i64)=sqlx::query_as("SELECT count(*)::bigint,current_setting('max_connections')::bigint FROM pg_stat_activity").fetch_one(&pool).await.map_err(|e|e.to_string())?;
        let sizes:Vec<(String,i64)>=sqlx::query_as("SELECT datname,pg_database_size(oid)::bigint FROM pg_database WHERE datistemplate=false ORDER BY 1").fetch_all(&pool).await.map_err(|e|e.to_string())?;
        report["sql_ready"] = json!(true);
        report["connections"] = json!({"used":counts.0,"max":counts.1});
        report["database_bytes"] = json!(sizes);
        if counts.1 > 0 && counts.0 * 100 / counts.1 >= 80 {
            alerts.push("PostgreSQL connections are above 80% of capacity".into());
        }
    }
    if instance.status == "ready" && crate::guest::available(&instance.node) {
        report["filesystem_usage_source"] =
            json!("See the matching container report for mounted filesystem usage");
    }
    let mut apps = Vec::new();
    for v in meta(s)?.list_records("app_database").await? {
        if v["instance_id"] != id {
            continue;
        }
        let a: ApplicationDatabase = serde_json::from_value(v).map_err(|e| e.to_string())?;
        if instance.backup_interval_hours > 0
            && a.last_backup_at.unwrap_or(a.created_at)
                + u64::from(instance.backup_interval_hours) * 3600
                < runtime::now()
        {
            alerts.push(format!("Backup overdue for {}", a.name));
        }
        if let Some(e) = &a.last_backup_error {
            alerts.push(format!("Backup failed for {}: {}", a.name, e));
        }
        apps.push(json!({"id":a.id,"name":a.name,"last_backup_at":a.last_backup_at}));
    }
    report["backups"] = json!(apps);
    report["alerts"] = json!(alerts);
    Ok(report)
}
pub async fn validate_verification_target(s: &AppState, id: &str) -> Result<(), String> {
    let instance: Instance = load(s, "database", id).await?;
    require_ready(&instance)
}
pub async fn verify_backup(
    s: &Arc<AppState>,
    id: &str,
    target: &str,
    task: &str,
) -> Result<(), String> {
    let backup: Backup = load(s, "backup", id).await?;
    if backup.instance_id == target {
        return Err("Verification requires a separate instance".into());
    }
    let materialized = crate::recovery::materialize(&backup).await?;
    let _lock = lock(target).await?;
    let instance: Instance = load(s, "database", target).await?;
    require_ready(&instance)?;
    let name = format!("drill_{}", runtime::id("v").to_ascii_lowercase());
    s.ansible
        .event(
            task,
            "task",
            "RESTORE",
            "Restoring into a generated verification database on the separate instance",
        )
        .await?;
    let mut app = add_app(s, &instance, &name, &name).await?;
    app.status = "restoring".into();
    meta(s)?
        .put_record("app_database", &app.id, &json!(app))
        .await?;
    meta(s)?
        .put_record(
            "drill_target",
            id,
            &json!({"app_id":app.id,"instance_id":target,"name":name,"task_id":task}),
        )
        .await?;
    pg_tool(
        "pg_restore",
        &[
            "--no-owner".into(),
            "--no-acl".into(),
            "--exit-on-error".into(),
        ],
        &instance,
        &app,
        &materialized.path,
    )
    .await?;
    let pool = pool_as(&instance, &app.username, &secret(&app.id)?, &app.name).await?;
    for t in &backup.tables {
        let count: (i64,) = sqlx::query_as(&format!(
            "SELECT COUNT(*) FROM {}.{}",
            quote(&t.schema),
            quote(&t.name)
        ))
        .fetch_one(&pool)
        .await
        .map_err(|e| e.to_string())?;
        if count.0 != t.rows {
            return Err(format!(
                "Verification failed in {}.{}; generated target {} is retained",
                t.schema, t.name, app.id
            ));
        }
    }
    pool.close().await;
    let control = control_pool(&instance).await?;
    // The role/database were created by this invocation under a reserved random name, and the instance lock is still held.
    sqlx::query(&format!("DROP DATABASE {} WITH (FORCE)", quote(&app.name)))
        .execute(&control)
        .await
        .map_err(|e| e.to_string())?;
    sqlx::query(&format!("DROP ROLE {}", quote(&app.username)))
        .execute(&control)
        .await
        .map_err(|e| e.to_string())?;
    meta(s)?.delete_record("app_database", &app.id).await?;
    let _ = std::fs::remove_file(secret_path(&app.id));
    meta(s)?.delete_record("drill_target", id).await?;
    Ok(())
}

pub fn hash_file(path: &std::path::Path) -> Result<String, String> {
    use sha2::Digest;
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = sha2::Sha256::new();
    let mut bytes = [0u8; 65536];
    loop {
        let n = file.read(&mut bytes).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(hex::encode(hash.finalize()))
}

pub async fn reconcile(state: &AppState) -> Result<(), String> {
    let db = meta(state)?;
    let containers = db.list_records("container").await?;
    for value in db.list_records("database").await? {
        let mut instance: Instance = serde_json::from_value(value).map_err(|e| e.to_string())?;
        if instance.status == "provisioning" {
            instance.status = "interrupted".into();
            instance.error = Some(
                "Manager restarted during provisioning. Inspect the guest and job before retrying."
                    .into(),
            );
            if let Some(record) = containers.iter().find(|c| c["task_id"] == instance.task_id) {
                instance.vmid = record["id"].as_u64().unwrap_or(instance.vmid.into()) as u32;
            }
            db.put_record("database", &instance.id, &json!(instance))
                .await?;
        }
    }
    for value in db.list_records("app_database").await? {
        let mut app: ApplicationDatabase =
            serde_json::from_value(value).map_err(|e| e.to_string())?;
        if matches!(app.status.as_str(), "creating" | "restoring" | "rotating") {
            app.status = if app.status == "rotating" {
                "rotation_interrupted".into()
            } else {
                "interrupted".into()
            };
            db.put_record("app_database", &app.id, &json!(app)).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod postgres_acceptance {
    use super::*;
    #[tokio::test]
    #[ignore = "Requires OpenSSL and a disposable HOSTABLE_DATA_DIR; run explicitly in CI"]
    async fn managed_ca_renewal_keeps_trust_and_constrains_leaf_extensions() {
        let instance:Instance=serde_json::from_value(json!({"id":"db_ca_fixture","name":"ca-fixture","node":"test","vmid":111,"host":"127.0.0.1","port":5432,"status":"ready","created_at":1,"task_id":"fixture","storage":"test","data_size_gb":1,"data_volume":null,"allowed_cidrs":[],"manager_cidr":"127.0.0.1/32","backup_interval_hours":0,"retention_count":1,"postgres_version":null,"error":null,"tls_mode":"managed_ca"})).unwrap();
        ensure_ca(&instance).await.unwrap();
        write_tls_extensions(&instance).unwrap();
        let before = hash_file(&certificate(&instance.id)).unwrap();
        let key = secret_path("fixture_leaf_key");
        let csr = secret_path("fixture_leaf_csr");
        let generated = tokio::process::Command::new("openssl")
            .args([
                "req",
                "-new",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-subj",
                "/CN=malicious.example",
                "-addext",
                "basicConstraints=critical,CA:TRUE",
            ])
            .arg("-keyout")
            .arg(&key)
            .arg("-out")
            .arg(&csr)
            .output()
            .await
            .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        for i in 0..2 {
            let leaf = certificate(&format!("fixture_leaf_{}", i));
            let signed = tokio::process::Command::new("openssl")
                .args(["x509", "-req", "-days", "365", "-CAcreateserial", "-in"])
                .arg(&csr)
                .arg("-CA")
                .arg(certificate(&instance.id))
                .arg("-CAkey")
                .arg(ca_key(&instance.id))
                .arg("-extfile")
                .arg(tls_extension_path(&instance.id))
                .arg("-out")
                .arg(&leaf)
                .output()
                .await
                .unwrap();
            assert!(signed.status.success());
            let verify = tokio::process::Command::new("openssl")
                .args(["verify", "-verify_ip", "127.0.0.1", "-CAfile"])
                .arg(certificate(&instance.id))
                .arg(&leaf)
                .output()
                .await
                .unwrap();
            assert!(
                verify.status.success(),
                "{}",
                String::from_utf8_lossy(&verify.stderr)
            );
            let text = tokio::process::Command::new("openssl")
                .args(["x509", "-noout", "-text", "-in"])
                .arg(&leaf)
                .output()
                .await
                .unwrap();
            let text = String::from_utf8_lossy(&text.stdout);
            assert!(text.contains("CA:FALSE"));
        }
        ensure_ca(&instance).await.unwrap();
        assert_eq!(before, hash_file(&certificate(&instance.id)).unwrap());
    }
    #[tokio::test]
    #[ignore = "requires disposable TLS PostgreSQL and pg_dump/pg_restore; run explicitly in CI"]
    async fn postgres_roles_rotation_backup_and_verified_restore() {
        let url =
            std::env::var("HOSTABLE_TEST_PG_URL").expect("Set disposable HOSTABLE_TEST_PG_URL");
        let ca = std::env::var("HOSTABLE_TEST_PG_CA").expect("Set trusted HOSTABLE_TEST_PG_CA");
        let root = PgPool::connect(&url).await.unwrap();
        sqlx::raw_sql("CREATE ROLE hostable_apps NOLOGIN; CREATE ROLE hostable_control LOGIN SUPERUSER PASSWORD 'control-test-password';").execute(&root).await.unwrap();
        let (db, _) = crate::db::DbBackend::init(Some("sqlite::memory:".into()))
            .await
            .unwrap();
        db.migrate_metadata().await.unwrap();
        let db = Arc::new(db);
        let engine = Arc::new(crate::jobs::JobEngine::new(db.clone()).await.unwrap());
        let state = Arc::new(crate::AppState {
            proxmox: crate::proxmox::ProxmoxClient::new(),
            db: Some(db.clone()),
            mock_token: None,
            catalog_cache: tokio::sync::RwLock::new(None),
            default_node: "test".into(),
            ansible: engine,
            secureweb: Arc::new(crate::secureweb::SecureWebClient::new(db.clone())),
            deployment_lock: Arc::new(tokio::sync::Semaphore::new(1)),
            manager_updates: Default::default(),
        });
        let parsed = reqwest::Url::parse(&url).unwrap();
        let instance = Instance {
            id: "db_pg_acceptance".into(),
            name: "pg-acceptance".into(),
            node: "test".into(),
            vmid: 110,
            host: "127.0.0.1".into(),
            port: parsed.port().unwrap_or(5432),
            status: "ready".into(),
            created_at: runtime::now(),
            task_id: "test".into(),
            storage: "test".into(),
            data_size_gb: 16,
            data_volume: None,
            allowed_cidrs: vec!["127.0.0.1/32".into()],
            manager_cidr: "127.0.0.1/32".into(),
            backup_interval_hours: 0,
            retention_count: 7,
            postgres_version: None,
            error: None,
            tls_mode: "legacy_self_signed".into(),
        };
        set_secret(&instance.id, "control-test-password").unwrap();
        runtime::write_secret(&certificate(&instance.id), &std::fs::read(&ca).unwrap()).unwrap();
        db.put_record("database", &instance.id, &json!(instance))
            .await
            .unwrap();
        let first = add_app(&state, &instance, "source_app", "source_user")
            .await
            .unwrap();
        let second = add_app(&state, &instance, "isolated_app", "isolated_user")
            .await
            .unwrap();
        let pool = pool_as(
            &instance,
            &first.username,
            &secret(&first.id).unwrap(),
            &first.name,
        )
        .await
        .unwrap();
        sqlx::raw_sql("CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT NOT NULL); INSERT INTO items VALUES (1,'persisted'),(2,'second row');").execute(&pool).await.unwrap();
        pool.close().await;
        assert!(
            pool_as(
                &instance,
                &second.username,
                &secret(&second.id).unwrap(),
                &first.name
            )
            .await
            .is_err(),
            "Other applications must not connect to this database"
        );
        assert!(
            add_app(&state, &instance, "source_app", "another_user")
                .await
                .is_err(),
            "Existing databases must never be overwritten"
        );
        let old = secret(&first.id).unwrap();
        let _ = rotate(RequireAuth, State(state.clone()), Path(first.id.clone()))
            .await
            .unwrap();
        assert!(
            pool_as(&instance, &first.username, &old, &first.name)
                .await
                .is_err(),
            "Rotated password must stop authenticating"
        );
        let task = state.ansible.create("backup", &first.id).await.unwrap();
        run_backup(state.clone(), first.id.clone(), task.clone())
            .await
            .unwrap();
        assert_eq!(state.ansible.get(&task).await.unwrap().status, "succeeded");
        let backup: Backup =
            serde_json::from_value(db.list_records("backup").await.unwrap().pop().unwrap())
                .unwrap();
        assert_eq!(
            backup
                .tables
                .iter()
                .find(|t| t.name == "items")
                .unwrap()
                .rows,
            2
        );
        let target_url = std::env::var("HOSTABLE_TEST_PG_TARGET_URL")
            .expect("Set a separate disposable target PostgreSQL URL");
        let target_admin = PgPool::connect(&target_url).await.unwrap();
        sqlx::raw_sql("CREATE ROLE hostable_apps NOLOGIN; CREATE ROLE hostable_control LOGIN SUPERUSER PASSWORD 'control-test-password';").execute(&target_admin).await.unwrap();
        let target = Instance {
            id: "db_verification_acceptance".into(),
            port: reqwest::Url::parse(&target_url)
                .unwrap()
                .port()
                .unwrap_or(5432),
            vmid: 112,
            ..instance.clone()
        };
        set_secret(&target.id, "control-test-password").unwrap();
        runtime::write_secret(&certificate(&target.id), &std::fs::read(&ca).unwrap()).unwrap();
        db.put_record("database", &target.id, &json!(target))
            .await
            .unwrap();
        let verification = state
            .ansible
            .create("restore_drill", &backup.id)
            .await
            .unwrap();
        verify_backup(&state, &backup.id, &target.id, &verification)
            .await
            .unwrap();
        let leftovers: (i64,) =
            sqlx::query_as("SELECT count(*)::bigint FROM pg_database WHERE datname LIKE 'drill_%'")
                .fetch_one(&target_admin)
                .await
                .unwrap();
        assert_eq!(
            leftovers.0, 0,
            "Verification must clean up only its generated target"
        );
        let response = restore(
            RequireAuth,
            State(state.clone()),
            Path(backup.id.clone()),
            Json(RestoreRequest {
                target_instance_id: instance.id.clone(),
                name: "restored_app".into(),
                username: "restored_user".into(),
            }),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        for _ in 0..1000 {
            if state
                .ansible
                .list()
                .await
                .iter()
                .any(|j| j.kind == "restore" && matches!(j.status.as_str(), "succeeded" | "failed"))
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let job = state
            .ansible
            .list()
            .await
            .into_iter()
            .find(|j| j.kind == "restore")
            .unwrap();
        assert_eq!(job.status, "succeeded", "{:?}", job.events);
        let restored: ApplicationDatabase = db
            .list_records("app_database")
            .await
            .unwrap()
            .into_iter()
            .filter_map(|v| serde_json::from_value::<ApplicationDatabase>(v).ok())
            .find(|a| a.name == "restored_app")
            .unwrap();
        assert_eq!(restored.status, "ready");
        let pool = pool_as(
            &instance,
            &restored.username,
            &secret(&restored.id).unwrap(),
            &restored.name,
        )
        .await
        .unwrap();
        let value: (String,) = sqlx::query_as("SELECT value FROM items WHERE id=1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(value.0, "persisted");
        pool.close().await;
        let _ = revoke(RequireAuth, State(state.clone()), Path(first.id.clone()))
            .await
            .unwrap();
        assert!(
            pool_as(
                &instance,
                &first.username,
                &secret(&first.id).unwrap(),
                &first.name
            )
            .await
            .is_err()
        );
        std::fs::write(backup_root().join(&backup.filename), b"corrupted backup").unwrap();
        let error = restore(
            RequireAuth,
            State(state),
            Path(backup.id),
            Json(RestoreRequest {
                target_instance_id: instance.id,
                name: "must_not_exist".into(),
                username: "must_not_exist_user".into(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.0, StatusCode::BAD_REQUEST);
        let exists: (bool,) = sqlx::query_as(
            "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname='must_not_exist')",
        )
        .fetch_one(&root)
        .await
        .unwrap();
        assert!(!exists.0);
    }
}
