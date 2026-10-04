pub use crate::jobs::JobEngine as AnsibleEngine;
use axum::{
    Json,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountPointParam {
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub storage: Option<String>,
    #[serde(default)]
    pub size_gb: Option<u32>,
    pub container: String,
    #[serde(default)]
    pub ro: bool,
}
impl MountPointParam {
    pub fn spec(&self) -> Result<String, String> {
        if !crate::proxmox::valid_mount_path(&self.container)
            || self.container == "/"
            || self.container.split('/').any(|p| p == "..")
        {
            return Err("Invalid mount path".into());
        }
        let source = match (&self.host, &self.storage, self.size_gb) {
            (Some(host), None, None)
                if crate::proxmox::valid_mount_path(host)
                    && !host.split('/').any(|p| p == "..") =>
            {
                host.clone()
            }
            (None, Some(storage), Some(size))
                if identifier(storage) && (1..=65536).contains(&size) =>
            {
                format!("{}:{}", storage, size)
            }
            _ => {
                return Err(
                    "Specify either an absolute bind mount or a storage pool and size_gb".into(),
                );
            }
        };
        Ok(format!(
            "{},mp={},backup=1{}",
            source,
            self.container,
            if self.ro { ",ro=1" } else { "" }
        ))
    }
}
pub fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && !s.starts_with('.')
        && !s.contains("..")
        && !s.starts_with('.')
        && !s.contains("..")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnsibleDeployParams {
    #[serde(default)]
    pub vmid: u32,
    #[serde(default)]
    pub node: Option<String>,
    pub hostname: String,
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub ostemplate: Option<String>,
    #[serde(default = "default_cores")]
    pub cores: u32,
    #[serde(default = "default_memory")]
    pub memory: u32,
    #[serde(default = "default_disk")]
    pub disk_size: String,
    #[serde(default = "default_template")]
    pub template_storage: String,
    #[serde(default = "default_storage", alias = "rootfs_storage")]
    pub storage_pool: String,
    #[serde(default = "default_bridge")]
    pub net_bridge: String,
    #[serde(default = "default_ip")]
    pub ip_address: String,
    #[serde(default)]
    pub gateway: Option<String>,
    #[serde(default)]
    pub mountpoints: Vec<MountPointParam>,
    #[serde(default)]
    pub env_vars: HashMap<String, String>,
    #[serde(default)]
    pub expose_secureweb: bool,
    #[serde(default)]
    pub secureweb_domain: Option<String>,
    #[serde(default)]
    pub app_port: Option<u16>,
    #[serde(default)]
    pub default_network: Option<String>,
    #[serde(default)]
    pub target_subnet: Option<String>,
    #[serde(default)]
    pub database_id: Option<String>,
    #[serde(default)]
    pub ssh_public_key: Option<String>,
    #[serde(default)]
    pub recipe: Option<crate::recipes::RecipeRef>,
    #[serde(default)]
    pub health: crate::recipes::HealthCheck,
    #[serde(default)]
    pub update: crate::recipes::UpdatePolicy,
}
fn default_cores() -> u32 {
    2
}
fn default_memory() -> u32 {
    1024
}
fn default_disk() -> String {
    "8G".into()
}
fn default_template() -> String {
    "local".into()
}
fn default_storage() -> String {
    "local-lvm".into()
}
fn default_bridge() -> String {
    std::env::var("HOSTABLE_DEFAULT_BRIDGE").unwrap_or_else(|_| "vmbr0".into())
}
fn default_ip() -> String {
    "dhcp".into()
}

pub fn validate_deploy_params(p: &AnsibleDeployParams) -> Result<(), String> {
    p.health.validate()?;
    p.update.validate()?;
    if let Some(key) = &p.ssh_public_key {
        if key.len() > 4096
            || key.contains(['\n', '\r'])
            || !(key.starts_with("ssh-ed25519 ") || key.starts_with("ssh-rsa "))
        {
            return Err("Invalid SSH public key".into());
        }
    }
    if p.ostemplate.is_none() && !crate::oci::validate_image_ref(&p.image) {
        return Err("Invalid image reference".into());
    }
    if p.ostemplate.is_some() && (!p.env_vars.is_empty() || p.database_id.is_some()) {
        return Err("Application environment injection requires an OCI image".into());
    }
    if p.ostemplate.is_some() && !p.image.is_empty() {
        return Err("Choose an OS template or an OCI image".into());
    }
    if let Some(t) = &p.ostemplate {
        let (storage, file) = t
            .split_once(":vztmpl/")
            .ok_or("Invalid OS template reference")?;
        if !identifier(storage)
            || file.contains('/')
            || !(file.ends_with(".tar.xz")
                || file.ends_with(".tar.zst")
                || file.ends_with(".tar.gz"))
            || !crate::proxmox::valid_opt_value(file)
        {
            return Err("Invalid OS template".into());
        }
    }
    if !crate::proxmox::valid_hostname(&p.hostname) {
        return Err("Invalid hostname".into());
    }
    if !identifier(&p.template_storage)
        || !identifier(&p.storage_pool)
        || !identifier(&p.net_bridge)
        || p.node.as_ref().is_some_and(|n| !identifier(n))
    {
        return Err("Invalid storage, node or bridge".into());
    }
    if p.vmid != 0 && !(100..=999999999).contains(&p.vmid) {
        return Err("VMID must be at least 100 or zero for automatic allocation".into());
    }
    if !(1..=128).contains(&p.cores) || !(64..=1048576).contains(&p.memory) {
        return Err("CPU or memory outside supported bounds".into());
    }
    disk_gb(&p.disk_size)?;
    validate_ip(&p.ip_address)?;
    if p.target_subnet
        .as_ref()
        .is_some_and(|s| !crate::databases::valid_cidr(s) || s.contains(':'))
    {
        return Err("Preferred subnet must be an IPv4 CIDR".into());
    }
    if let Some(gw) = &p.gateway {
        if !gw.is_empty() && gw.parse::<std::net::IpAddr>().is_err() {
            return Err("Invalid gateway".into());
        }
    }
    if let Some(iface) = &p.default_network {
        if !crate::proxmox::valid_iface_name(iface) {
            return Err("Invalid network interface".into());
        }
    }
    let mut paths = std::collections::HashSet::new();
    for mp in &p.mountpoints {
        mp.spec()?;
        if !paths.insert(&mp.container) {
            return Err("Duplicate volume mount path".into());
        }
    }
    for (key, value) in &p.env_vars {
        if !crate::oci::valid_env_key(key) || value.contains('\0') {
            return Err("Invalid environment variable".into());
        }
    }
    if p.expose_secureweb
        && !p
            .secureweb_domain
            .as_ref()
            .is_some_and(|d| crate::secureweb::valid_domain(d))
    {
        return Err("A valid domain is required for ingress".into());
    }
    if p.app_port == Some(0) {
        return Err("Port must be greater than zero".into());
    }
    Ok(())
}
pub fn disk_gb(s: &str) -> Result<u32, String> {
    let s = s
        .strip_suffix("GB")
        .or_else(|| s.strip_suffix('G'))
        .unwrap_or(s);
    let n: u32 = s
        .parse()
        .map_err(|_| "Disk size must be an integer in GiB".to_string())?;
    if (1..=65536).contains(&n) {
        Ok(n)
    } else {
        Err("Invalid disk size".into())
    }
}
pub fn validate_ip(s: &str) -> Result<(), String> {
    if s == "dhcp" {
        return Ok(());
    }
    let (ip, prefix) = s
        .split_once('/')
        .ok_or("Static address requires a CIDR prefix")?;
    let ip: std::net::IpAddr = ip.parse().map_err(|_| "Invalid IP address")?;
    let prefix: u8 = prefix.parse().map_err(|_| "Invalid CIDR prefix")?;
    if !ip.is_ipv4() {
        return Err("IPv4 static networking is supported in this release".into());
    }
    if prefix > 32 {
        return Err("Invalid CIDR prefix".into());
    }
    Ok(())
}
#[derive(Debug, Serialize)]
pub struct DeploymentResult {
    pub node: String,
    pub vmid: u32,
    pub ip: String,
}

pub async fn execute_deployment(
    state: Arc<crate::AppState>,
    task: String,
    mut params: AnsibleDeployParams,
) {
    let result = deploy(&state, &task, &mut params).await;
    match result {
        Ok(result) => {
            let _ = state
                .ansible
                .event(
                    &task,
                    "ok",
                    "COMPLETE",
                    &format!(
                        "{} is running at {} (LXC {} on {})",
                        params.hostname, result.ip, result.vmid, result.node
                    ),
                )
                .await;
        }
        Err(error) => {
            tracing::error!("Deployment {} failed: {}", task, error);
            let error = format!(
                "{}. Requested/assigned VMID {} on {}. Inspect partial resources before retrying.",
                error,
                params.vmid,
                params.node.as_deref().unwrap_or(&state.default_node)
            );
            state.ansible.finish_error(&task, &error).await;
        }
    }
}

pub async fn deploy(
    state: &Arc<crate::AppState>,
    task: &str,
    params: &mut AnsibleDeployParams,
) -> Result<DeploymentResult, String> {
    deploy_mode(state, task, params, true).await
}

pub async fn prepare(
    state: &Arc<crate::AppState>,
    task: &str,
    params: &mut AnsibleDeployParams,
) -> Result<DeploymentResult, String> {
    deploy_mode(state, task, params, false).await
}

async fn deploy_mode(
    state: &Arc<crate::AppState>,
    task: &str,
    params: &mut AnsibleDeployParams,
    start: bool,
) -> Result<DeploymentResult, String> {
    validate_deploy_params(params)?;
    if let Some(reference) = &params.recipe {
        let recipe = crate::recipes::resolve(state, &reference.id, reference.version).await?;
        for key in &recipe.required_env {
            if !params.env_vars.contains_key(key) {
                return Err(format!("Recipe requires environment variable {}", key));
            }
        }
        if params.health.port.is_none() {
            params.health = recipe.health;
        }
        params.update = recipe.update;
    }
    if params.health.port.is_none() {
        params.health.port = params.app_port;
    }
    state.ansible.check_cancel(task).await?;
    state
        .ansible
        .event(task, "task", "QUEUE", "Waiting for deployment slot")
        .await?;
    let _permit = state
        .deployment_lock
        .acquire()
        .await
        .map_err(|e| e.to_string())?;
    let node = params
        .node
        .clone()
        .unwrap_or_else(|| state.default_node.clone());
    state
        .ansible
        .event(
            task,
            "task",
            "PREFLIGHT",
            "Checking resources, storage and network",
        )
        .await?;
    let resources = state.proxmox.get_cluster_resources().await?;
    let metadata = state.db.as_ref().ok_or("Metadata unavailable")?;
    let mut reserved = std::collections::HashSet::new();
    for w in metadata.list_records("workload").await? {
        if let Some(n) = w["active"]["vmid"].as_u64() {
            reserved.insert(n);
        }
        if let Some(rows) = w["previous"].as_array() {
            for r in rows {
                if let Some(n) = r["vmid"].as_u64() {
                    reserved.insert(n);
                }
            }
        }
        if let Some(n) = w["id"].as_str().and_then(|s| s.parse::<u64>().ok()) {
            reserved.insert(n);
        }
    }
    if params.vmid == 0 {
        params.vmid = state.proxmox.get_next_vmid().await?;
        while reserved.contains(&u64::from(params.vmid))
            || resources["data"].as_array().is_some_and(|a| {
                a.iter()
                    .any(|r| r["vmid"].as_u64() == Some(params.vmid as u64))
            })
        {
            params.vmid = params
                .vmid
                .checked_add(1)
                .filter(|v| *v <= 999999999)
                .ok_or("No available VMID")?;
        }
    }
    if reserved.contains(&u64::from(params.vmid)) {
        return Err("VMID is reserved by a managed workload or retained revision; its metadata cannot be overwritten".into());
    }
    if !(100..=999999999).contains(&params.vmid) {
        return Err("Proxmox returned an invalid VMID".into());
    }
    if resources["data"].as_array().is_some_and(|rows| {
        rows.iter()
            .any(|r| r["vmid"].as_u64() == Some(params.vmid as u64))
    }) {
        return Err(format!(
            "VMID {} is already allocated; existing containers are never overwritten",
            params.vmid
        ));
    }
    crate::workloads::save_request(state, task, params).await?;
    state
        .proxmox
        .validate_storage(&node, &params.storage_pool, "rootdir")
        .await?;
    if params.ostemplate.is_none() {
        state
            .proxmox
            .validate_storage(&node, &params.template_storage, "vztmpl")
            .await?;
    }
    for mp in &params.mountpoints {
        if let Some(storage) = &mp.storage {
            state
                .proxmox
                .validate_storage(&node, storage, "rootdir")
                .await?;
        }
    }
    if !state
        .proxmox
        .get_network_bridges(&node)
        .await?
        .contains(&params.net_bridge)
    {
        return Err("Selected network bridge is unavailable on this node".into());
    }
    let mut extra = HashMap::new();
    if let Some(database_id) = &params.database_id {
        let uri = crate::databases::application_uri(state, database_id).await?;
        let uri = format!("{}&sslrootcert=/etc/hostable/database-ca.crt", uri);
        extra.insert(
            "etc/hostable/database-ca.crt".into(),
            crate::databases::application_certificate(state, database_id).await?,
        );
        params.env_vars.insert("DATABASE_URL".into(), uri);
    }
    let cache = crate::runtime::data_dir().join("cache").join(task);
    crate::runtime::private_dir(&cache)?;
    let _cleanup = crate::runtime::CacheGuard(cache.clone());
    let template = if let Some(template) = &params.ostemplate {
        template.clone()
    } else {
        let pinned = crate::oci::OciExtractor::new()?
            .resolved_image(&params.image)
            .await?;
        crate::workloads::record_image(state, task, &params.image, &pinned).await?;
        state.ansible.check_cancel(task).await?;
        state
            .ansible
            .event(
                task,
                "task",
                "OCI_PULL",
                "Downloading and converting OCI image",
            )
            .await?;
        let filename = format!("hostable_{}.tar.xz", task);
        let archive = cache.join(&filename);
        let envs: Vec<_> = params
            .env_vars
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        crate::oci::OciExtractor::new()?
            .extract_to_dir(&pinned, &archive, Some(&envs), Some(extra))
            .await
            .map_err(|e| e.to_string())?;
        state
            .ansible
            .event(task, "task", "UPLOAD", "Uploading container template")
            .await?;
        let upload = state
            .proxmox
            .upload_template(&node, &params.template_storage, &archive, &filename)
            .await?;
        state.proxmox.wait_response_task(&node, upload).await?;
        let _ = std::fs::remove_file(&archive);
        format!("{}:vztmpl/{}", params.template_storage, filename)
    };
    state.ansible.check_cancel(task).await?;
    let iface = params
        .default_network
        .clone()
        .unwrap_or_else(|| "eth0".into());
    let mut network = format!(
        "name={},bridge={},ip={}",
        iface, params.net_bridge, params.ip_address
    );
    if let Some(gateway) = &params.gateway {
        if !gateway.is_empty() {
            network.push_str(&format!(",gw={}", gateway));
        }
    }
    let mut create = HashMap::from([
        ("vmid".into(), params.vmid.to_string()),
        ("hostname".into(), params.hostname.clone()),
        ("ostemplate".into(), template.clone()),
        ("cores".into(), params.cores.to_string()),
        ("memory".into(), params.memory.to_string()),
        (
            "rootfs".into(),
            format!("{}:{}", params.storage_pool, disk_gb(&params.disk_size)?),
        ),
        ("net0".into(), network),
        ("unprivileged".into(), "1".into()),
        ("onboot".into(), if start { "1" } else { "0" }.into()),
        ("features".into(), "nesting=1".into()),
        ("tags".into(), "hostable;managed".into()),
        ("description".into(), format!("hostable.task={}", task)),
    ]);
    for (i, mount) in params.mountpoints.iter().enumerate() {
        create.insert(format!("mp{}", i), mount.spec()?);
    }
    if let Some(key) = &params.ssh_public_key {
        create.insert("ssh-public-keys".into(), key.clone());
    }
    state
        .ansible
        .event(
            task,
            "task",
            "PVE_CREATE",
            "Creating unprivileged container",
        )
        .await?;
    state
        .proxmox
        .wait_response_task(
            &node,
            state.proxmox.create_lxc(&node, params.vmid, create).await?,
        )
        .await?;
    let db = state.db.as_ref().ok_or("Platform database unavailable")?;
    db.put_record("container", &params.vmid.to_string(), &json!({"id":params.vmid,"node":node,"name":params.hostname,"image":params.image,"volumes":params.mountpoints,"task_id":task,"status":"created"})).await?;
    crate::workloads::register(state, task, params, &node, start).await?;
    if params.ostemplate.is_none() {
        let filename = template
            .split(":vztmpl/")
            .nth(1)
            .ok_or("Invalid generated template")?;
        state.proxmox.delete_template(&node,&params.template_storage,filename).await.map_err(|_| "Container was created, but temporary template deletion failed. Remove the generated template from Proxmox storage before retrying.".to_string())?;
    }
    if !start {
        return Ok(DeploymentResult {
            node,
            vmid: params.vmid,
            ip: String::new(),
        });
    }
    state.ansible.check_cancel(task).await?;
    state
        .ansible
        .event(task, "task", "PVE_START", "Starting container")
        .await?;
    state
        .proxmox
        .wait_response_task(&node, state.proxmox.start_lxc(&node, params.vmid).await?)
        .await?;
    state
        .proxmox
        .wait_lxc_status(&node, params.vmid, "running")
        .await?;
    state
        .ansible
        .event(
            task,
            "task",
            "NETWORK",
            "Waiting for a real container address",
        )
        .await?;
    let ip = state
        .proxmox
        .poll_lxc_ip(
            &node,
            params.vmid,
            Some(&iface),
            params.target_subnet.as_deref(),
            30,
        )
        .await?;
    if let Some(port) = params.app_port {
        state
            .ansible
            .event(
                task,
                "task",
                "READINESS",
                "Checking the application's listening port",
            )
            .await?;
        let address = format!("{}:{}", ip, port);
        let mut ready = false;
        for _ in 0..30 {
            if matches!(
                tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    tokio::net::TcpStream::connect(&address)
                )
                .await,
                Ok(Ok(_))
            ) {
                ready = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        if !ready {
            return Err(format!(
                "Container is running but service on port {} is not ready",
                port
            ));
        }
    }
    let db = state.db.as_ref().ok_or("Platform database unavailable")?;
    db.put_record("container", &params.vmid.to_string(), &json!({"id": params.vmid, "node": node, "name": params.hostname, "image": params.image, "volumes": params.mountpoints, "ip": ip, "created_at": crate::runtime::now(), "task_id": task})).await?;
    crate::workloads::check_health(&ip, &params.health).await?;
    crate::workloads::mark_ready(state, params.vmid, &ip).await?;
    if params.expose_secureweb {
        state
            .ansible
            .event(
                task,
                "task",
                "INGRESS",
                "Registering the upstream with the configured gateway",
            )
            .await?;
        state
            .secureweb
            .register_route(crate::secureweb::SecureWebRoute {
                domain: params.secureweb_domain.clone().ok_or("Domain missing")?,
                target_ip: ip.clone(),
                target_port: params.app_port.unwrap_or(80),
                service_name: params.hostname.clone(),
                vmid: params.vmid,
                mode: "https".into(),
                created_at: Some(crate::runtime::now().to_string()),
            })
            .await?;
    }
    let _ = std::fs::remove_dir(&cache);
    Ok(DeploymentResult {
        node,
        vmid: params.vmid,
        ip,
    })
}

pub async fn trigger_deploy_handler(
    _auth: crate::RequireAuth,
    State(state): State<Arc<crate::AppState>>,
    Json(params): Json<AnsibleDeployParams>,
) -> Response {
    if let Err(error) = validate_deploy_params(&params) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"status":"error","error":error})),
        )
            .into_response();
    }
    let task = match state.ansible.create("deploy", &params.hostname).await {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"status":"error","error":e})),
            )
                .into_response();
        }
    };
    let id = task.clone();
    tokio::spawn(async move {
        execute_deployment(state, id, params).await;
    });
    (
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task,"status":"queued"})),
    )
        .into_response()
}
pub async fn get_task_events_handler(
    _auth: crate::RequireAuth,
    State(state): State<Arc<crate::AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if state.ansible.get(&id).await.is_none() {
        return Err((StatusCode::NOT_FOUND, "Unknown job".into()));
    }
    Ok(Json(json!(state.ansible.get_events(&id).await)))
}
pub async fn ws_task_stream_handler(
    _auth: crate::RequireAuth,
    State(state): State<Arc<crate::AppState>>,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    if state.ansible.get(&id).await.is_none() {
        return (StatusCode::NOT_FOUND, "Unknown job").into_response();
    }
    ws.on_upgrade(move |socket| handle_task_ws(socket, id, state.ansible.clone()))
}
async fn handle_task_ws(mut socket: WebSocket, id: String, engine: Arc<AnsibleEngine>) {
    let mut receiver = engine.subscribe();
    let mut last = 0;
    for event in engine.get_events(&id).await {
        last = event.sequence;
        if socket
            .send(Message::Text(json!(event).to_string().into()))
            .await
            .is_err()
        {
            return;
        }
    }
    if engine.get(&id).await.is_some_and(|j| {
        matches!(
            j.status.as_str(),
            "succeeded" | "failed" | "interrupted" | "cancelled"
        )
    }) {
        return;
    }
    loop {
        let received =
            tokio::select! { result = receiver.recv() => result, _ = socket.recv() => break };
        match received {
            Ok(event) if event.task_id == id && event.sequence > last => {
                last = event.sequence;
                if socket
                    .send(Message::Text(json!(event).to_string().into()))
                    .await
                    .is_err()
                {
                    break;
                }
                if matches!(
                    event.step.as_str(),
                    "COMPLETE" | "FAILED" | "INTERRUPTED" | "CANCELLED"
                ) {
                    break;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                for event in engine
                    .get_events(&id)
                    .await
                    .into_iter()
                    .filter(|e| e.sequence > last)
                    .collect::<Vec<_>>()
                {
                    last = event.sequence;
                    if socket
                        .send(Message::Text(json!(event).to_string().into()))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
            Err(_) => break,
            _ => {}
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_structured_volumes_and_static_networks() {
        let mp = MountPointParam {
            host: None,
            storage: Some("local-lvm".into()),
            size_gb: Some(8),
            container: "/data".into(),
            ro: false,
        };
        assert_eq!(mp.spec().unwrap(), "local-lvm:8,mp=/data,backup=1");
        assert!(validate_ip("192.168.1.4/24").is_ok());
        assert!(validate_ip("192.168.1.4/99").is_err());
        assert!(validate_ip("bad").is_err());
        assert!(disk_gb("8G,mp=/evil").is_err());
        let bad = MountPointParam {
            container: "/data/../etc".into(),
            ..mp
        };
        assert!(bad.spec().is_err());
    }
}
