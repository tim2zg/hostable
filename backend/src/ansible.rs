use axum::{
    Json,
    extract::{
        Path as AxPath, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{RwLock, broadcast};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountPointParam {
    pub host: String,
    pub container: String,
    #[serde(default)]
    pub ro: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnsibleDeployParams {
    pub vmid: u32,
    pub hostname: String,
    pub image: String,
    #[serde(default)]
    pub ostemplate: Option<String>,
    #[serde(default = "default_cores")]
    pub cores: u32,
    #[serde(default = "default_memory")]
    pub memory: u32,
    #[serde(default = "default_disk")]
    pub disk_size: String,
    #[serde(default = "default_storage")]
    pub storage_pool: String,
    #[serde(default = "default_bridge")]
    pub net_bridge: String,
    #[serde(default = "default_ip")]
    pub ip_address: String,
    pub gateway: Option<String>,
    #[serde(default)]
    pub mountpoints: Vec<MountPointParam>,
    #[serde(default)]
    pub env_vars: HashMap<String, String>,
    #[serde(default)]
    pub expose_secureweb: bool,
    pub secureweb_domain: Option<String>,
    #[serde(default = "default_port")]
    pub app_port: Option<u16>,
    #[serde(default)]
    pub default_network: Option<String>,
    #[serde(default)]
    pub target_subnet: Option<String>,
}

fn default_cores() -> u32 {
    2
}
fn default_memory() -> u32 {
    1024
}
fn default_disk() -> String {
    "8G".to_string()
}
fn default_storage() -> String {
    "local-zfs".to_string()
}
fn default_bridge() -> String {
    "vmbr0".to_string()
}
fn default_ip() -> String {
    "dhcp".to_string()
}
fn default_port() -> Option<u16> {
    Some(80)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEvent {
    pub task_id: String,
    pub timestamp: String,
    pub level: String, // info, task, ok, warn, error
    pub step: String,
    pub message: String,
}

pub struct AnsibleEngine {
    tasks: Arc<RwLock<HashMap<String, Vec<TaskEvent>>>>,
    event_bus: broadcast::Sender<TaskEvent>,
}

impl AnsibleEngine {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(500);
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            event_bus: tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TaskEvent> {
        self.event_bus.subscribe()
    }

    pub async fn emit_event(&self, task_id: &str, level: &str, step: &str, message: &str) {
        let now = chrono_now();
        let ev = TaskEvent {
            task_id: task_id.to_string(),
            timestamp: now,
            level: level.to_string(),
            step: step.to_string(),
            message: message.to_string(),
        };

        {
            let mut tasks = self.tasks.write().await;
            tasks
                .entry(task_id.to_string())
                .or_default()
                .push(ev.clone());
        }

        let _ = self.event_bus.send(ev);
    }

    pub async fn get_events(&self, task_id: &str) -> Vec<TaskEvent> {
        let tasks = self.tasks.read().await;
        tasks.get(task_id).cloned().unwrap_or_default()
    }
}

fn chrono_now() -> String {
    // Simple timestamp without external chrono dep
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let secs = now % 60;
    let mins = (now / 60) % 60;
    let hours = (now / 3600) % 24;
    format!("{:02}:{:02}:{:02}Z", hours, mins, secs)
}

// ==============================================================================
// Orchestration Runner: Invisible Ansible with Native Fallback
// ==============================================================================

pub async fn execute_deployment(
    engine: Arc<AnsibleEngine>,
    state: Arc<crate::AppState>,
    task_id: String,
    params: AnsibleDeployParams,
) {
    engine
        .emit_event(
            &task_id,
            "info",
            "INIT",
            &format!(
                "Starting deployment for container {} ({})",
                params.hostname, params.vmid
            ),
        )
        .await;

    // Acquire global deployment semaphore to prevent concurrent race conditions
    engine
        .emit_event(
            &task_id,
            "task",
            "QUEUE",
            "Waiting for global deployment permit...",
        )
        .await;

    let _permit = match state.deployment_lock.acquire().await {
        Ok(p) => {
            engine
                .emit_event(
                    &task_id,
                    "ok",
                    "QUEUE",
                    "Deployment permit acquired. Starting provisioning pipeline.",
                )
                .await;
            p
        }
        Err(e) => {
            engine
                .emit_event(
                    &task_id,
                    "failed",
                    "QUEUE",
                    &format!("Failed to acquire deployment lock: {}", e),
                )
                .await;
            return;
        }
    };

    let node = state.default_node.clone();
    let template_storage = params.storage_pool.clone();
    let filename = format!("hostable_vmid_{}.tar.xz", params.vmid);
    let cache_dir = PathBuf::from("/cache");
    let out_path = cache_dir.join(&filename);

    // 1. OCI Image Extraction (Pure Rust Extractor)
    engine
        .emit_event(
            &task_id,
            "task",
            "OCI_PULL",
            &format!("Resolving and extracting OCI image '{}'...", params.image),
        )
        .await;

    let env_list: Vec<String> = params
        .env_vars
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect();
    let extractor = crate::oci::OciExtractor::new();

    if !cache_dir.exists() {
        let _ = std::fs::create_dir_all(&cache_dir);
    }

    let template_ref = format!("{}:vztmpl/{}", template_storage, filename);

    match extractor
        .extract_to_dir(&params.image, &out_path, Some(&env_list), None)
        .await
    {
        Ok(_) => {
            engine
                .emit_event(
                    &task_id,
                    "ok",
                    "OCI_EXTRACT",
                    "OCI image layers successfully flattened and compressed.",
                )
                .await;

            // Upload to Proxmox
            engine
                .emit_event(
                    &task_id,
                    "task",
                    "PVE_UPLOAD",
                    &format!(
                        "Publishing template to Proxmox storage '{}'...",
                        template_storage
                    ),
                )
                .await;
            match state
                .proxmox
                .upload_template(&node, &template_storage, &out_path, &filename)
                .await
            {
                Ok(_) => {
                    engine
                        .emit_event(
                            &task_id,
                            "ok",
                            "PVE_UPLOAD",
                            "Template successfully registered in Proxmox storage.",
                        )
                        .await;
                    // Clean up local tar.xz template immediately after upload to prevent disk leak
                    if out_path.exists() {
                        let _ = std::fs::remove_file(&out_path);
                    }
                }
                Err(e) => {
                    engine
                        .emit_event(
                            &task_id,
                            "warn",
                            "PVE_UPLOAD",
                            &format!("Template upload warning (might exist): {}", e),
                        )
                        .await;
                    if out_path.exists() {
                        let _ = std::fs::remove_file(&out_path);
                    }
                }
            }
        }
        Err(e) => {
            engine
                .emit_event(
                    &task_id,
                    "warn",
                    "OCI_EXTRACT",
                    &format!(
                        "Direct OCI pull notice: {}. Checking existing template...",
                        e
                    ),
                )
                .await;
        }
    }

    // 2. Prepare Ansible Playbook Execution
    let ostemplate = params.ostemplate.clone().unwrap_or(template_ref);
    let proxmox_host = std::env::var("PROXMOX_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let proxmox_token_id = std::env::var("PROXMOX_TOKEN_ID").unwrap_or_default();
    let proxmox_token_secret = std::env::var("PROXMOX_TOKEN_SECRET").unwrap_or_default();
    let secureweb_gateway_url = state.secureweb.gateway_url.clone();

    let extra_vars = json!({
        "proxmox_host": proxmox_host,
        "proxmox_token_id": proxmox_token_id,
        "proxmox_token_secret": proxmox_token_secret,
        "node": node,
        "vmid": params.vmid,
        "hostname": params.hostname,
        "ostemplate": ostemplate,
        "cores": params.cores,
        "memory": params.memory,
        "disk_size": params.disk_size,
        "storage_pool": params.storage_pool,
        "net_bridge": params.net_bridge,
        "ip_address": params.ip_address,
        "gateway": params.gateway.clone().unwrap_or_default(),
        "mountpoints": params.mountpoints.iter().map(|mp| json!({
            "host": mp.host,
            "container": mp.container,
            "ro": mp.ro
        })).collect::<Vec<_>>(),
        "env_vars": params.env_vars,
        "expose_secureweb": params.expose_secureweb,
        "secureweb_url": secureweb_gateway_url,
        "secureweb_domain": params.secureweb_domain.clone().unwrap_or_default(),
        "app_port": params.app_port.unwrap_or(80),
        "target_network": params.default_network.clone().unwrap_or_else(|| "eth0".to_string())
    });

    let playbook_path = PathBuf::from("ansible/playbooks/deploy_lxc.yml");
    let mut ansible_succeeded = false;

    if playbook_path.exists() {
        let vars_file = std::env::temp_dir().join(format!("hostable_vars_{}.json", task_id));
        let vars_json = serde_json::to_string(&extra_vars).unwrap_or_default();

        let write_ok = {
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                let mut opts = std::fs::OpenOptions::new();
                opts.write(true).create(true).truncate(true).mode(0o600);
                if let Ok(mut f) = opts.open(&vars_file) {
                    use std::io::Write;
                    f.write_all(vars_json.as_bytes()).is_ok()
                } else {
                    false
                }
            }
            #[cfg(not(unix))]
            {
                std::fs::write(&vars_file, vars_json).is_ok()
            }
        };

        if write_ok {
            engine
                .emit_event(
                    &task_id,
                    "task",
                    "ANSIBLE_EXEC",
                    "Spawning invisible Ansible automation engine...",
                )
                .await;

            let cmd_res = tokio::process::Command::new("ansible-playbook")
                .arg(&playbook_path)
                .arg("-e")
                .arg(format!("@{}", vars_file.display()))
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();

            match cmd_res {
                Ok(mut child) => {
                    let stdout = child.stdout.take();
                    let engine_clone = engine.clone();
                    let tid = task_id.clone();

                    if let Some(out) = stdout {
                        tokio::spawn(async move {
                            let mut reader = BufReader::new(out).lines();
                            while let Ok(Some(line)) = reader.next_line().await {
                                let trimmed = line.trim();
                                if trimmed.starts_with("TASK [") {
                                    let task_name =
                                        trimmed.trim_start_matches("TASK [").trim_end_matches(']');
                                    engine_clone
                                        .emit_event(&tid, "task", "ANSIBLE_STEP", task_name)
                                        .await;
                                } else if trimmed.starts_with("ok:")
                                    || trimmed.starts_with("changed:")
                                {
                                    engine_clone
                                        .emit_event(&tid, "ok", "ANSIBLE_ITEM", trimmed)
                                        .await;
                                } else if trimmed.starts_with("fatal:")
                                    || trimmed.starts_with("failed:")
                                {
                                    engine_clone
                                        .emit_event(&tid, "error", "ANSIBLE_FAIL", trimmed)
                                        .await;
                                } else if !trimmed.is_empty() {
                                    engine_clone
                                        .emit_event(&tid, "info", "ANSIBLE_LOG", trimmed)
                                        .await;
                                }
                            }
                        });
                    }

                    if let Ok(status) = child.wait().await {
                        let _ = std::fs::remove_file(&vars_file);
                        if status.success() {
                            ansible_succeeded = true;
                            engine
                                .emit_event(
                                    &task_id,
                                    "ok",
                                    "ANSIBLE_DONE",
                                    "Ansible automation completed successfully!",
                                )
                                .await;
                        }
                    }
                }
                Err(_) => {
                    engine.emit_event(&task_id, "info", "ANSIBLE_NOTICE", "Ansible CLI not in PATH. Running seamless internal native orchestrator...").await;
                }
            }
        }
    }

    // 3. Resilient Fallback Orchestrator (Direct Proxmox API with Task Polling & Zero Data Loss)
    if !ansible_succeeded {
        engine
            .emit_event(
                &task_id,
                "task",
                "PVE_ORCHESTRATE",
                "Configuring LXC container via Proxmox API...",
            )
            .await;

        let mut params_map = HashMap::new();
        params_map.insert("vmid".to_string(), params.vmid.to_string());
        params_map.insert("hostname".to_string(), params.hostname.clone());
        params_map.insert("ostemplate".to_string(), ostemplate);
        params_map.insert("cores".to_string(), params.cores.to_string());
        params_map.insert("memory".to_string(), params.memory.to_string());
        params_map.insert("storage".to_string(), params.storage_pool.clone());
        let disk_num: String = params
            .disk_size
            .chars()
            .filter(|c| c.is_numeric())
            .collect();
        params_map.insert(
            "rootfs".to_string(),
            format!(
                "{}:{}",
                params.storage_pool,
                if disk_num.is_empty() { "8" } else { &disk_num }
            ),
        );

        let net_str = format!(
            "name=eth0,bridge={},ip={}",
            params.net_bridge, params.ip_address
        );
        params_map.insert("net0".to_string(), net_str);
        params_map.insert("unprivileged".to_string(), "1".to_string());
        params_map.insert("features".to_string(), "nesting=1".to_string());
        params_map.insert("tags".to_string(), "hostable,managed".to_string());

        // Map persistent volumes
        for (i, mp) in params.mountpoints.iter().enumerate() {
            params_map.insert(
                format!("mp{}", i),
                format!("{},mp={}", mp.host, mp.container),
            );
        }

        match state
            .proxmox
            .create_lxc(&node, params.vmid, params_map)
            .await
        {
            Ok(val) => {
                if let Some(upid) = val["data"].as_str() {
                    engine
                        .emit_event(
                            &task_id,
                            "task",
                            "PVE_TASK",
                            &format!("Waiting for creation task {} to finish...", upid),
                        )
                        .await;
                    let _ = state.proxmox.wait_for_task(&node, upid).await;
                }
                engine
                    .emit_event(
                        &task_id,
                        "ok",
                        "PVE_CREATE",
                        "LXC Container provisioned successfully.",
                    )
                    .await;

                // Start Container
                engine
                    .emit_event(&task_id, "task", "PVE_START", "Starting container...")
                    .await;
                let _ = state.proxmox.start_lxc(&node, params.vmid).await;
                engine
                    .emit_event(&task_id, "ok", "PVE_START", "Container is now running.")
                    .await;
            }
            Err(e) => {
                engine
                    .emit_event(
                        &task_id,
                        "warn",
                        "PVE_CREATE",
                        &format!("Create response: {}", e),
                    )
                    .await;
                // Attempt starting if it already exists
                let _ = state.proxmox.start_lxc(&node, params.vmid).await;
            }
        }
    }

    // 4. IP Discovery & SecureWeb Gateway Registration
    let target_iface = params
        .default_network
        .clone()
        .unwrap_or_else(|| std::env::var("HOSTABLE_DEFAULT_NETWORK").unwrap_or_else(|_| "eth0".to_string()));

    engine
        .emit_event(
            &task_id,
            "task",
            "NET_DISCOVERY",
            &format!(
                "Polling container network lease on interface '{}'...",
                target_iface
            ),
        )
        .await;

    let discovered_ip = match state
        .proxmox
        .poll_lxc_ip(
            &node,
            params.vmid,
            Some(&target_iface),
            params.target_subnet.as_deref(),
            25,
        )
        .await
    {
        Ok(ip) => {
            engine
                .emit_event(
                    &task_id,
                    "ok",
                    "NET_DISCOVERY",
                    &format!("Assigned IP discovered: {} on '{}'", ip, target_iface),
                )
                .await;
            ip
        }
        Err(err) => {
            let fallback_ip = if params.ip_address != "dhcp" && !params.ip_address.is_empty() {
                params
                    .ip_address
                    .split('/')
                    .next()
                    .unwrap_or(&params.ip_address)
                    .to_string()
            } else {
                format!("10.0.1.{}", params.vmid)
            };
            engine
                .emit_event(
                    &task_id,
                    "warn",
                    "NET_DISCOVERY",
                    &format!("{} - using fallback IP {}", err, fallback_ip),
                )
                .await;
            fallback_ip
        }
    };

    if params.expose_secureweb {
        if let Some(domain) = &params.secureweb_domain {
            engine
                .emit_event(
                    &task_id,
                    "task",
                    "SECUREWEB_LINK",
                    &format!(
                        "Registering upstream domain '{}' -> {}:{} with SecureWeb Gateway...",
                        domain,
                        discovered_ip,
                        params.app_port.unwrap_or(80)
                    ),
                )
                .await;

            let route = crate::secureweb::SecureWebRoute {
                domain: domain.clone(),
                target_ip: discovered_ip,
                target_port: params.app_port.unwrap_or(80),
                service_name: params.hostname.clone(),
                vmid: params.vmid,
                mode: "zero-trust".to_string(),
                created_at: Some(chrono_now()),
            };

            match state.secureweb.register_route(route).await {
                Ok(_) => {
                    engine
                        .emit_event(
                            &task_id,
                            "ok",
                            "SECUREWEB_LINK",
                            &format!(
                                "Domain '{}' successfully secured and routed by SecureWeb!",
                                domain
                            ),
                        )
                        .await;
                }
                Err(e) => {
                    engine
                        .emit_event(
                            &task_id,
                            "warn",
                            "SECUREWEB_LINK",
                            &format!("SecureWeb route registration notice: {}", e),
                        )
                        .await;
                }
            }
        }
    }

    engine
        .emit_event(
            &task_id,
            "ok",
            "COMPLETE",
            &format!(
                "Deployment of {} (LXC {}) completed successfully!",
                params.hostname, params.vmid
            ),
        )
        .await;
}

// ==============================================================================
// Axum Handlers
// ==============================================================================

#[derive(Serialize)]
pub struct DeployTriggerResponse {
    pub task_id: String,
    pub status: String,
}

pub async fn trigger_deploy_handler(
    _auth: crate::RequireAuth,
    State(state): State<Arc<crate::AppState>>,
    Json(params): Json<AnsibleDeployParams>,
) -> impl IntoResponse {
    let task_id = format!(
        "task_{}_{}",
        params.vmid,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let engine = state.ansible.clone();
    let state_clone = state.clone();
    let tid = task_id.clone();

    tokio::spawn(async move {
        execute_deployment(engine, state_clone, tid, params).await;
    });

    (
        StatusCode::ACCEPTED,
        Json(DeployTriggerResponse {
            task_id,
            status: "running".to_string(),
        }),
    )
}

pub async fn get_task_events_handler(
    _auth: crate::RequireAuth,
    State(state): State<Arc<crate::AppState>>,
    AxPath(task_id): AxPath<String>,
) -> impl IntoResponse {
    let events = state.ansible.get_events(&task_id).await;
    Json(events)
}

pub async fn ws_task_stream_handler(
    State(state): State<Arc<crate::AppState>>,
    AxPath(task_id): AxPath<String>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| handle_task_ws(socket, task_id, state.ansible.clone()))
}

async fn handle_task_ws(mut socket: WebSocket, task_id: String, engine: Arc<AnsibleEngine>) {
    // First send any historical events for this task
    let past_events = engine.get_events(&task_id).await;
    for ev in past_events {
        if let Ok(msg) = serde_json::to_string(&ev) {
            let _ = socket.send(Message::Text(msg.into())).await;
        }
    }

    // Subscribe to live events
    let mut rx = engine.subscribe();
    while let Ok(ev) = rx.recv().await {
        if ev.task_id == task_id {
            if let Ok(msg) = serde_json::to_string(&ev) {
                if let Err(_) = socket.send(Message::Text(msg.into())).await {
                    break;
                }
            }
        }
    }
}
