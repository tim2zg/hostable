//! Image-to-LXC lifecycle built around the existing OCI converter. No root disks are overwritten.
use crate::{
    AppState, RequireAuth,
    ansible::AnsibleDeployParams,
    recipes::{HealthCheck, UpdatePolicy},
    runtime,
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Revision {
    pub vmid: u32,
    pub owner: String,
    pub image: String,
    pub pinned_image: Option<String>,
    pub spec: String,
    pub created_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Workload {
    pub id: String,
    pub name: String,
    pub node: String,
    pub active: Revision,
    pub previous: Vec<Revision>,
    pub status: String,
    pub ip: Option<String>,
    pub health: HealthCheck,
    pub update: UpdatePolicy,
    pub operation: Option<String>,
    pub last_update_at: Option<u64>,
    pub last_update_error: Option<String>,
    #[serde(default)]
    pub last_completed_operation: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct UpdatePlan {
    id: String,
    workload_id: String,
    source: Revision,
    config_digest: Option<String>,
    mode: String,
    image: Option<String>,
    pinned_image: Option<String>,
    backup_storage: Option<String>,
    #[serde(default)]
    health: HealthCheck,
    #[serde(default)]
    update: UpdatePolicy,
    #[serde(default)]
    database_id: Option<String>,
    created_at: u64,
    used: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rollout {
    pub id: String,
    pub workload_id: String,
    pub mode: String,
    pub source: Revision,
    pub target: Option<Revision>,
    pub stage: String,
    pub network: String,
    pub source_onboot: String,
    pub source_running: bool,
    pub volumes: Vec<String>,
    pub snapshot: Option<String>,
    pub pending_upid: Option<String>,
}
type Error = (StatusCode, String);
fn api_error(e: String) -> Error {
    (StatusCode::BAD_REQUEST, e)
}
fn db(s: &AppState) -> Result<&crate::db::DbBackend, String> {
    s.db.as_deref().ok_or("Metadata unavailable".into())
}
pub async fn load(s: &AppState, id: &str) -> Result<Workload, String> {
    serde_json::from_value(
        db(s)?
            .get_record("workload", id)
            .await?
            .ok_or("Unknown managed workload")?,
    )
    .map_err(|e| e.to_string())
}
async fn save(s: &AppState, w: &Workload) -> Result<(), String> {
    db(s)?.put_record("workload", &w.id, &json!(w)).await
}
fn request_file(id: &str) -> Result<std::path::PathBuf, String> {
    if !crate::ansible::identifier(id) {
        return Err("Invalid deployment reference".into());
    }
    Ok(runtime::data_dir()
        .join("secrets")
        .join(format!("deploy_{}.json", id)))
}
pub fn request(id: &str) -> Result<AnsibleDeployParams, String> {
    serde_json::from_slice(
        &std::fs::read(request_file(id)?).map_err(|_| "Deployment configuration is unavailable")?,
    )
    .map_err(|e| e.to_string())
}
pub async fn save_request(s: &AppState, task: &str, p: &AnsibleDeployParams) -> Result<(), String> {
    runtime::write_secret(
        &request_file(task)?,
        &serde_json::to_vec(p).map_err(|e| e.to_string())?,
    )?;
    db(s)?.put_record("deployment",task,&json!({"id":task,"vmid":p.vmid,"node":p.node.as_deref().unwrap_or(&s.default_node),"name":p.hostname})).await
}
pub async fn record_image(
    s: &AppState,
    task: &str,
    image: &str,
    pinned: &str,
) -> Result<(), String> {
    db(s)?
        .put_record(
            "image",
            task,
            &json!({"image":image,"pinned_image":pinned,"converted_at":runtime::now()}),
        )
        .await
}
pub async fn register(
    s: &AppState,
    task: &str,
    p: &AnsibleDeployParams,
    node: &str,
    managed: bool,
) -> Result<(), String> {
    let pinned = db(s)?
        .get_record("image", task)
        .await?
        .and_then(|v| v["pinned_image"].as_str().map(str::to_owned));
    if managed {
        let w = Workload {
            id: p.vmid.to_string(),
            name: p.hostname.clone(),
            node: node.into(),
            active: Revision {
                vmid: p.vmid,
                owner: task.into(),
                image: p.image.clone(),
                pinned_image: pinned,
                spec: task.into(),
                created_at: runtime::now(),
            },
            previous: vec![],
            status: "created".into(),
            ip: None,
            health: p.health.clone(),
            update: p.update.clone(),
            operation: None,
            last_update_at: None,
            last_update_error: None,
            last_completed_operation: None,
        };
        save(s, &w).await?;
    }
    Ok(())
}
pub async fn mark_ready(s: &AppState, vmid: u32, ip: &str) -> Result<(), String> {
    if let Ok(mut w) = load(s, &vmid.to_string()).await {
        w.status = "ready".into();
        w.ip = Some(ip.into());
        save(s, &w).await?;
    }
    Ok(())
}
pub async fn owned(s: &AppState, node: &str, vmid: u32, owner: &str) -> Result<Value, String> {
    if s.proxmox.resolve_lxc_node(vmid).await? != node {
        return Err("Container changed node; refresh the plan".into());
    }
    let config = s.proxmox.get_lxc_config(node, vmid).await?;
    if !config["data"]["description"]
        .as_str()
        .is_some_and(|d| d.lines().any(|l| l == format!("hostable.task={}", owner)))
    {
        return Err("Container ownership changed; refusing to operate on a reused VMID".into());
    }
    Ok(config["data"].clone())
}
pub async fn check_health(ip: &str, h: &HealthCheck) -> Result<(), String> {
    h.validate()?;
    let ip: std::net::IpAddr = ip.parse().map_err(|_| "Invalid workload address")?;
    if let Some(port) = h.port {
        if let Some(path) = &h.http_path {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| e.to_string())?;
            let url = format!("http://{}:{}{}", ip, port, path);
            let result = client
                .get(url)
                .send()
                .await
                .map_err(|_| "HTTP health endpoint is unreachable")?;
            if result.status().as_u16() != h.expected_status.unwrap_or(200) {
                return Err(format!("HTTP health check returned {}", result.status()));
            }
        } else {
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                tokio::net::TcpStream::connect((ip, port)),
            )
            .await
            .map_err(|_| "TCP health check timed out")?
            .map_err(|_| "Application port is unavailable")?;
        }
    }
    Ok(())
}
pub async fn list(
    _auth: RequireAuth,
    State(s): State<Arc<AppState>>,
) -> Result<Json<Value>, Error> {
    Ok(Json(json!(
        db(&s)
            .map_err(api_error)?
            .list_records("workload")
            .await
            .map_err(api_error)?
    )))
}
pub async fn get(
    _auth: RequireAuth,
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    Ok(Json(json!(load(&s, &id).await.map_err(api_error)?)))
}
#[derive(Deserialize)]
pub struct PolicyRequest {
    pub health: HealthCheck,
    pub update: UpdatePolicy,
}
pub async fn policy(
    _auth: RequireAuth,
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(r): Json<PolicyRequest>,
) -> Result<Json<Value>, Error> {
    r.health.validate().map_err(api_error)?;
    r.update.validate().map_err(api_error)?;
    if r.update.interval_hours > 0 && r.health.port.is_none() {
        return Err(api_error(
            "Automatic updates require an application health port".into(),
        ));
    }
    let _lock = s
        .ansible
        .lock_resource(&format!("workload:{}", id))
        .await
        .map_err(|e| (StatusCode::CONFLICT, e))?;
    let mut w = load(&s, &id).await.map_err(api_error)?;
    if w.operation.is_some() || !matches!(w.status.as_str(), "ready" | "stopped") {
        return Err((
            StatusCode::CONFLICT,
            "Resolve the active/interrupted operation first".into(),
        ));
    }
    if s.ansible
        .get(&w.active.owner)
        .await
        .is_some_and(|j| matches!(j.status.as_str(), "queued" | "running" | "interrupted"))
    {
        return Err((
            StatusCode::CONFLICT,
            "Inspect and recover the original deployment before updating this guest".into(),
        ));
    }
    owned(&s, &w.node, w.active.vmid, &w.active.owner)
        .await
        .map_err(api_error)?;
    if r.update.mode != "image" && !crate::guest::available(&w.node) {
        return Err(api_error(
            "Configure verified node SSH before enabling guest updates".into(),
        ));
    }
    w.health = r.health;
    w.update = r.update;
    save(&s, &w).await.map_err(api_error)?;
    Ok(Json(json!(w)))
}
#[derive(Deserialize)]
pub struct PlanRequest {
    pub mode: String,
    pub image: Option<String>,
    pub backup_storage: Option<String>,
    pub database_id: Option<String>,
}
fn numbered_field(key: &str, prefix: &str) -> bool {
    key.strip_prefix(prefix)
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}
fn replacement_config(config: &Value) -> Result<(), String> {
    let unprivileged = &config["unprivileged"];
    if !(unprivileged == 1 || unprivileged == "1" || unprivileged == true)
        || config.get("lxc.idmap").is_some()
        || config
            .get("lxc")
            .is_some_and(|v| !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()))
        || config
            .as_object()
            .is_some_and(|o| o.keys().any(|k| k.starts_with("lxc.")))
        || config
            .as_object()
            .is_some_and(|o| o.keys().any(|k| numbered_field(k, "net") && k != "net0"))
    {
        return Err("Replacement requires the standard unprivileged mapping, a single net0 interface and no custom LXC passthrough options. Resolve live configuration drift before updating.".into());
    }
    Ok(())
}
fn validate_mounts(config: &Value, params: &AnsibleDeployParams) -> Result<(), String> {
    let live_count = config
        .as_object()
        .map_or(0, |o| o.keys().filter(|k| numbered_field(k, "mp")).count());
    if live_count != params.mountpoints.len() {
        return Err(
            "Live mounts differ from the saved deployment; resolve configuration drift".into(),
        );
    }
    for (i, m) in params.mountpoints.iter().enumerate() {
        let key = format!("mp{}", i);
        let spec = config[&key].as_str().ok_or("A recorded mount is missing")?;
        let mut fields = spec.split(',');
        let source = fields.next().unwrap_or("");
        let options: Vec<_> = fields.collect();
        let path = format!("mp={}", m.container);
        let ro = options.contains(&"ro=1");
        if !options.contains(&path.as_str())
            || ro != m.ro
            || m.host.as_deref().is_some_and(|h| h != source)
            || m.host.is_none() && source.starts_with('/')
            || m.host.is_none() && !options.contains(&"backup=1")
        {
            return Err(
                "Live mounts differ from the saved deployment; resolve configuration drift".into(),
            );
        }
    }
    Ok(())
}
pub async fn plan(
    _auth: RequireAuth,
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(r): Json<PlanRequest>,
) -> Result<Json<Value>, Error> {
    if !matches!(
        r.mode.as_str(),
        "image" | "recipe" | "rollback" | "refresh_credentials"
    ) {
        return Err(api_error("Invalid update method".into()));
    }
    if r.database_id.is_some() && !matches!(r.mode.as_str(), "image" | "refresh_credentials") {
        return Err(api_error(
            "Changing a database attachment requires an image replacement plan".into(),
        ));
    }
    let w = load(&s, &id).await.map_err(api_error)?;
    if w.operation.is_some() || !matches!(w.status.as_str(), "ready" | "stopped") {
        return Err((
            StatusCode::CONFLICT,
            "Resolve the active/interrupted operation first".into(),
        ));
    }
    if s.ansible
        .get(&w.active.owner)
        .await
        .is_some_and(|j| matches!(j.status.as_str(), "queued" | "running" | "interrupted"))
    {
        return Err((
            StatusCode::CONFLICT,
            "Inspect and recover the original deployment before updating this guest".into(),
        ));
    }
    if db(&s)
        .map_err(api_error)?
        .list_records("database")
        .await
        .map_err(api_error)?
        .iter()
        .any(|v| v["vmid"].as_u64() == Some(w.active.vmid as u64))
    {
        return Err(api_error("Managed PostgreSQL instances use the database maintenance workflow; image replacement and generic package upgrades are disabled".into()));
    }
    let config = owned(&s, &w.node, w.active.vmid, &w.active.owner)
        .await
        .map_err(api_error)?;
    let params = request(&w.active.spec).map_err(api_error)?;
    validate_mounts(&config, &params).map_err(api_error)?;
    if r.mode != "recipe" {
        replacement_config(&config).map_err(api_error)?;
    }
    if params.mountpoints.iter().any(|m| m.host.is_some() && !m.ro) {
        return Err(api_error("Writable bind mounts are outside Proxmox backups/snapshots. Use managed volumes or an explicit external-data recovery procedure before automated updates.".into()));
    }
    let backup_storage = r.backup_storage.or(w.update.backup_storage.clone());
    if let Some(storage) = &backup_storage {
        if !crate::ansible::identifier(storage) {
            return Err(api_error("Invalid backup storage".into()));
        }
        s.proxmox
            .validate_storage(&w.node, storage, "backup")
            .await
            .map_err(api_error)?;
    }
    if !params.mountpoints.is_empty() && backup_storage.is_none() {
        return Err(api_error(
            "Stateful updates require a Proxmox backup storage".into(),
        ));
    }
    if matches!(
        r.mode.as_str(),
        "image" | "refresh_credentials" | "rollback"
    ) && !params.mountpoints.is_empty()
    {
        let snapshots = s
            .proxmox
            .get_snapshots(&w.node, w.active.vmid)
            .await
            .map_err(api_error)?;
        if snapshots["data"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v["name"] != "current"))
        {
            return Err(api_error("Volume transfer is blocked by existing snapshots. Retain a verified backup and resolve snapshots in Proxmox before replacement.".into()));
        }
    }
    if r.mode == "rollback" && w.previous.is_empty() {
        return Err(api_error("No retained revision to roll back to".into()));
    }
    if r.mode == "recipe" {
        w.update.command().map_err(api_error)?;
        if !crate::guest::available(&w.node) {
            return Err(api_error(
                "Verified node SSH is required for guest updates".into(),
            ));
        }
    }
    let image = if matches!(r.mode.as_str(), "image" | "refresh_credentials") {
        let image = if r.mode == "refresh_credentials" {
            w.active.pinned_image.clone().ok_or_else(|| {
                api_error("Credential refresh requires a recorded image digest".into())
            })?
        } else {
            r.image.unwrap_or(params.image)
        };
        if !crate::oci::validate_image_ref(&image) {
            return Err(api_error("An OCI image is required".into()));
        }
        Some(image)
    } else {
        None
    };
    let pinned = if let Some(image) = &image {
        Some(
            crate::oci::OciExtractor::new()
                .map_err(api_error)?
                .resolved_image(image)
                .await
                .map_err(api_error)?,
        )
    } else {
        None
    };
    if let Some(id) = &r.database_id {
        crate::databases::application_uri(&s, id)
            .await
            .map_err(api_error)?;
    }
    let p = UpdatePlan {
        id: runtime::id("plan"),
        workload_id: id,
        source: w.active.clone(),
        config_digest: Some(
            config["digest"]
                .as_str()
                .ok_or_else(|| {
                    api_error(
                        "Proxmox configuration digest is missing; cannot create a safe update plan"
                            .into(),
                    )
                })?
                .into(),
        ),
        mode: r.mode,
        image,
        pinned_image: pinned,
        backup_storage,
        health: w.health.clone(),
        update: w.update.clone(),
        database_id: r.database_id,
        created_at: runtime::now(),
        used: false,
    };
    db(&s)
        .map_err(api_error)?
        .put_record("update_plan", &p.id, &json!(p))
        .await
        .map_err(api_error)?;
    Ok(Json(
        json!({"plan":p,"health":w.health,"update":w.update,"volumes":params.mountpoints,"root_disk":if p.mode=="recipe" {"The update runs in the existing root disk; a pre-update snapshot is retained."} else if p.mode=="rollback" {"Rollback uses the retained previous root disk and transfers the current persistent mounts back to it."} else {"Replacement uses a new root disk. Files outside persistent mounts stay only in the retained old LXC."},"downtime":if p.mode=="recipe" {"The guest stops for its snapshot and restarts before updating. The configured service procedure or an LXC restart activates the update."} else {"Both guests stop during volume/network transfer; the old LXC is retained with onboot disabled."},"rollback":if p.mode=="recipe" {"A failed update stops the guest before restoring its snapshot and checking the original service. The pre-update backup remains available for stateful recovery."} else {"The previous root disk is retained. Moving data back does not undo application schema changes; restore the pre-update backup if required."},"guest_ssh":crate::guest::available(&w.node)}),
    ))
}

async fn save_rollout(s: &AppState, r: &Rollout) -> Result<(), String> {
    db(s)?.put_record("rollout", &r.id, &json!(r)).await
}
async fn stage(s: &AppState, r: &mut Rollout, name: &str, message: &str) -> Result<(), String> {
    r.stage = name.into();
    save_rollout(s, r).await?;
    s.ansible.event(&r.id, "task", name, message).await
}
async fn task(s: &AppState, r: &mut Rollout, response: Value) -> Result<(), String> {
    r.pending_upid = Some(
        response["data"]
            .as_str()
            .filter(|s| s.starts_with("UPID:"))
            .ok_or("Missing Proxmox task ID")?
            .into(),
    );
    save_rollout(s, r).await?;
    s.proxmox
        .wait_response_task(&load(s, &r.workload_id).await?.node, response)
        .await?;
    r.pending_upid = None;
    save_rollout(s, r).await
}
async fn stop(s: &AppState, node: &str, vmid: u32) -> Result<(), String> {
    if s.proxmox.get_lxc_status(node, vmid).await?["data"]["status"] == "running" {
        s.proxmox
            .wait_response_task(node, s.proxmox.stop_lxc(node, vmid).await?)
            .await?;
    }
    s.proxmox.wait_lxc_status(node, vmid, "stopped").await
}
async fn start(s: &AppState, node: &str, vmid: u32) -> Result<(), String> {
    if s.proxmox.get_lxc_status(node, vmid).await?["data"]["status"] != "running" {
        s.proxmox
            .wait_response_task(node, s.proxmox.start_lxc(node, vmid).await?)
            .await?;
    }
    s.proxmox.wait_lxc_status(node, vmid, "running").await
}
async fn ready(s: &AppState, w: &Workload, vmid: u32) -> Result<String, String> {
    let parameters = request(&w.active.spec)?;
    let ip = s
        .proxmox
        .poll_lxc_ip(
            &w.node,
            vmid,
            Some(parameters.default_network.as_deref().unwrap_or("eth0")),
            parameters.target_subnet.as_deref(),
            30,
        )
        .await?;
    for attempt in 0..30 {
        match check_health(&ip, &w.health).await {
            Ok(()) => return Ok(ip),
            Err(e) if attempt == 29 => return Err(e),
            _ => tokio::time::sleep(std::time::Duration::from_secs(2)).await,
        }
    }
    Err("Application did not become healthy".into())
}
async fn transfer(
    s: &AppState,
    w: &Workload,
    r: &mut Rollout,
    from: &Revision,
    to: &Revision,
) -> Result<(), String> {
    replacement_config(&owned(s, &w.node, from.vmid, &from.owner).await?)?;
    replacement_config(&owned(s, &w.node, to.vmid, &to.owner).await?)?;
    stop(s, &w.node, from.vmid).await?;
    stop(s, &w.node, to.vmid).await?;
    for key in r.volumes.clone() {
        let source = s.proxmox.get_lxc_config(&w.node, from.vmid).await?["data"].clone();
        let target = s.proxmox.get_lxc_config(&w.node, to.vmid).await?["data"].clone();
        match (source[&key].as_str(), target[&key].as_str()) {
            (Some(spec), None) if spec.starts_with('/') => {
                s.proxmox
                    .update_lxc_config(
                        &w.node,
                        to.vmid,
                        &std::collections::HashMap::from([(key.clone(), spec.into())]),
                    )
                    .await?;
                s.proxmox
                    .update_lxc_config(
                        &w.node,
                        from.vmid,
                        &std::collections::HashMap::from([("delete".into(), key.clone())]),
                    )
                    .await?;
            }
            (Some(_), None) => {
                let response = s
                    .proxmox
                    .move_mount(&w.node, from.vmid, to.vmid, &key)
                    .await?;
                task(s, r, response).await?;
            }
            (None, Some(_)) => {}
            _ => {
                return Err(format!(
                    "Ambiguous volume ownership for {}. Both guests remain stopped for inspection.",
                    key
                ));
            }
        }
        save_rollout(s, r).await?;
    }
    Ok(())
}
async fn run_update(s: Arc<AppState>, mut w: Workload, p: UpdatePlan, mut r: Rollout) {
    let initial = w.clone();
    let result: Result<(), String> = async {
        owned(&s, &w.node, w.active.vmid, &w.active.owner).await?;
        s.ansible.check_cancel(&r.id).await?;
        if let Some(storage) = &p.backup_storage {
            stage(
                &s,
                &mut r,
                "BACKUP",
                "Creating a pre-update Proxmox backup; archives are retained",
            )
            .await?;
            let response = s
                .proxmox
                .backup_lxc(&w.node, w.active.vmid, storage)
                .await?;
            task(&s, &mut r, response).await?;
        }
        s.ansible.check_cancel(&r.id).await?;
        if p.mode == "recipe" {
            let snap = format!("hostable_{}", &r.id[r.id.len() - 16..]);
            r.snapshot = Some(snap.clone());
            stage(
                &s,
                &mut r,
                "SNAPSHOT",
                "Saving a rollback snapshot before the guest update",
            )
            .await?;
            stop(&s, &w.node, w.active.vmid).await?;
            let response = s
                .proxmox
                .create_snapshot(
                    &w.node,
                    w.active.vmid,
                    &snap,
                    "Hostable pre-update recovery point",
                )
                .await?;
            task(&s, &mut r, response).await?;
            start(&s, &w.node, w.active.vmid).await?;
            s.ansible.check_cancel(&r.id).await?;
            stage(
                &s,
                &mut r,
                "GUEST_UPDATE",
                "Running the recipe update inside the container",
            )
            .await?;
            crate::guest::exec(
                &s,
                &w.node,
                w.active.vmid,
                &w.active.owner,
                &w.update.command()?,
                w.update.timeout_seconds,
            )
            .await?;
            s.ansible.check_cancel(&r.id).await?;
            if w.update.restart_script.trim().is_empty() {
                stage(
                    &s,
                    &mut r,
                    "RESTART",
                    "Restarting the LXC to activate updated packages",
                )
                .await?;
                stop(&s, &w.node, w.active.vmid).await?;
                start(&s, &w.node, w.active.vmid).await?;
            }
            stage(&s, &mut r, "HEALTH", "Checking the updated application").await?;
            w.ip = Some(ready(&s, &w, w.active.vmid).await?);
        } else {
            if p.mode == "rollback" {
                r.target = Some(w.previous.last().ok_or("No previous revision")?.clone());
            } else {
                let mut params = request(&w.active.spec)?;
                params.vmid = 0;
                params.image = p
                    .pinned_image
                    .clone()
                    .ok_or("No pinned replacement image")?;
                params.ostemplate = None;
                params.node = Some(w.node.clone());
                params.expose_secureweb = false;
                params.mountpoints.clear();
                params.recipe = None;
                params.health = w.health.clone();
                params.update = w.update.clone();
                if p.database_id.is_some() {
                    params.database_id = p.database_id.clone();
                }
                stage(
                    &s,
                    &mut r,
                    "PREPARE",
                    "Converting the pinned image and creating a stopped replacement LXC",
                )
                .await?;
                let prepared = crate::ansible::prepare(&s, &r.id, &mut params).await?;
                // Restore the original persistent mount declaration in the saved spec without allocating new volumes.
                params.mountpoints = request(&w.active.spec)?.mountpoints;
                params.expose_secureweb = request(&w.active.spec)?.expose_secureweb;
                save_request(&s, &r.id, &params).await?;
                r.target = Some(Revision {
                    vmid: prepared.vmid,
                    owner: r.id.clone(),
                    image: p.image.clone().unwrap_or(params.image.clone()),
                    pinned_image: p.pinned_image.clone(),
                    spec: r.id.clone(),
                    created_at: runtime::now(),
                });
                save_rollout(&s, &r).await?;
            }
            s.ansible.check_cancel(&r.id).await?;
            stage(
                &s,
                &mut r,
                "CUTOVER",
                "Stopping the old LXC and transferring persistent volumes and network",
            )
            .await?;
            let target = r.target.clone().ok_or("Missing replacement")?;
            let source = r.source.clone();
            transfer(&s, &w, &mut r, &source, &target).await?;
            s.proxmox
                .update_lxc_config(
                    &w.node,
                    source.vmid,
                    &std::collections::HashMap::from([("onboot".into(), "0".into())]),
                )
                .await?;
            s.proxmox
                .update_lxc_config(
                    &w.node,
                    target.vmid,
                    &std::collections::HashMap::from([
                        ("net0".into(), r.network.clone()),
                        ("onboot".into(), r.source_onboot.clone()),
                    ]),
                )
                .await?;
            stage(
                &s,
                &mut r,
                "HEALTH",
                "Starting and checking the replacement",
            )
            .await?;
            start(&s, &w.node, target.vmid).await?;
            let ip = ready(&s, &w, target.vmid).await?;
            s.ansible.check_cancel(&r.id).await?;
            let original = request(&source.spec)?;
            if original.expose_secureweb {
                s.secureweb
                    .register_route(crate::secureweb::SecureWebRoute {
                        domain: original.secureweb_domain.ok_or("Missing ingress domain")?,
                        target_ip: ip.clone(),
                        target_port: original.app_port.unwrap_or(80),
                        service_name: w.name.clone(),
                        vmid: target.vmid,
                        mode: "https".into(),
                        created_at: Some(runtime::now().to_string()),
                    })
                    .await?;
            }
            if p.mode == "rollback" {
                w.previous.pop();
            }
            w.previous.push(w.active.clone());
            w.active = target;
            w.ip = Some(ip);
        }
        w.status = "ready".into();
        w.operation = None;
        w.last_update_at = Some(runtime::now());
        w.last_update_error = None;
        w.last_completed_operation = Some(r.id.clone());
        save(&s, &w).await?;
        stage(
            &s,
            &mut r,
            "COMPLETE",
            "Update passed health checks; recovery points and old root disks are retained",
        )
        .await?;
        Ok(())
    }
    .await;
    if let Err(error) = result {
        let recovery = undo(&s, &w, &mut r).await;
        match recovery {
            Ok(()) => {
                w.active = initial.active.clone();
                w.previous = initial.previous.clone();
                w.ip = initial.ip.clone();
                w.last_update_at = initial.last_update_at;
                w.last_completed_operation = initial.last_completed_operation.clone();
                w.operation = None;
                w.status = if r.source_running { "ready" } else { "stopped" }.into();
                w.last_update_error = Some(error.clone());
                let _ = save(&s, &w).await;
                s.ansible.finish_error(&r.id, &error).await;
            }
            Err(recovery) => {
                w.status = "recovery_required".into();
                w.last_update_error = Some(format!("{}; recovery: {}", error, recovery));
                let _ = save(&s, &w).await;
                let _=s.ansible.event(&r.id,"error","INTERRUPTED","Automatic rollback could not be verified. Inspect and recover this operation before making further changes.").await;
            }
        }
    }
}
async fn undo(s: &Arc<AppState>, w: &Workload, r: &mut Rollout) -> Result<(), String> {
    if let Some(upid) = &r.pending_upid {
        if s.proxmox.task_status(&w.node, upid).await?["data"]["status"] != "stopped" {
            return Err(
                "A Proxmox task is still active. Wait for it to finish before recovering.".into(),
            );
        }
        r.pending_upid = None;
        save_rollout(s, r).await?;
    }
    owned(s, &w.node, r.source.vmid, &r.source.owner).await?;
    if r.mode == "recipe" {
        if let Some(snap) = &r.snapshot {
            let snapshots = s.proxmox.get_snapshots(&w.node, r.source.vmid).await?;
            if snapshots["data"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v["name"] == snap.as_str()))
            {
                stop(s, &w.node, r.source.vmid).await?;
                s.proxmox
                    .wait_response_task(
                        &w.node,
                        s.proxmox
                            .rollback_snapshot(&w.node, r.source.vmid, snap)
                            .await?,
                    )
                    .await?;
            }
        }
    } else if let Some(target) = r.target.clone() {
        owned(s, &w.node, target.vmid, &target.owner).await?;
        stop(s, &w.node, target.vmid).await?;
        let source = r.source.clone();
        transfer(s, w, r, &target, &source).await?;
        s.proxmox
            .update_lxc_config(
                &w.node,
                target.vmid,
                &std::collections::HashMap::from([("onboot".into(), "0".into())]),
            )
            .await?;
    }
    s.proxmox
        .update_lxc_config(
            &w.node,
            r.source.vmid,
            &std::collections::HashMap::from([
                ("net0".into(), r.network.clone()),
                ("onboot".into(), r.source_onboot.clone()),
            ]),
        )
        .await?;
    if r.source_running {
        start(s, &w.node, r.source.vmid).await?;
        let ip = ready(s, w, r.source.vmid).await?;
        let original = request(&r.source.spec)?;
        if original.expose_secureweb {
            s.secureweb
                .register_route(crate::secureweb::SecureWebRoute {
                    domain: original.secureweb_domain.ok_or("Missing ingress domain")?,
                    target_ip: ip,
                    target_port: original.app_port.unwrap_or(80),
                    service_name: w.name.clone(),
                    vmid: r.source.vmid,
                    mode: "https".into(),
                    created_at: Some(runtime::now().to_string()),
                })
                .await?;
        }
    }
    r.stage = "ROLLED_BACK".into();
    save_rollout(s, r).await
}

#[derive(Deserialize)]
pub struct ApplyRequest {
    pub plan_id: String,
}
pub async fn apply(
    _auth: RequireAuth,
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(r): Json<ApplyRequest>,
) -> Result<Response, Error> {
    let guard = s
        .ansible
        .lock_resource(&format!("workload:{}", id))
        .await
        .map_err(|e| (StatusCode::CONFLICT, e))?;
    let p: UpdatePlan = serde_json::from_value(
        db(&s)
            .map_err(api_error)?
            .get_record("update_plan", &r.plan_id)
            .await
            .map_err(api_error)?
            .ok_or_else(|| api_error("Unknown update plan".into()))?,
    )
    .map_err(|e| api_error(e.to_string()))?;
    let mut w = load(&s, &id).await.map_err(api_error)?;
    let lxc_guard = s
        .ansible
        .lock_resource(&format!("lxc:{}", w.active.vmid))
        .await
        .map_err(|e| (StatusCode::CONFLICT, e))?;
    if p.workload_id != id
        || p.used
        || p.created_at + 3600 < runtime::now()
        || p.source.vmid != w.active.vmid
        || p.source.owner != w.active.owner
        || json!(p.health) != json!(w.health)
        || json!(p.update) != json!(w.update)
        || w.operation.is_some()
        || !matches!(w.status.as_str(), "ready" | "stopped")
    {
        return Err((
            StatusCode::CONFLICT,
            "Plan expired or the workload changed; create a fresh plan".into(),
        ));
    }
    let config = owned(&s, &w.node, w.active.vmid, &w.active.owner)
        .await
        .map_err(api_error)?;
    if p.config_digest != config["digest"].as_str().map(str::to_owned) {
        return Err((
            StatusCode::CONFLICT,
            "Proxmox configuration changed; refresh the plan".into(),
        ));
    }
    let task_id = s
        .ansible
        .create(
            if p.mode == "recipe" {
                "shell_update"
            } else if p.mode == "rollback" {
                "rollback"
            } else {
                "replace"
            },
            &id,
        )
        .await
        .map_err(api_error)?;
    let params = request(&w.active.spec).map_err(api_error)?;
    let rollout = Rollout {
        id: task_id.clone(),
        workload_id: id.clone(),
        mode: p.mode.clone(),
        source: w.active.clone(),
        target: None,
        stage: "QUEUED".into(),
        network: config["net0"]
            .as_str()
            .ok_or_else(|| api_error("Container net0 is missing".into()))?
            .into(),
        source_onboot: if config["onboot"].as_u64() == Some(1)
            || config["onboot"].as_bool() == Some(true)
            || config["onboot"].as_str() == Some("1")
        {
            "1"
        } else {
            "0"
        }
        .into(),
        source_running: s
            .proxmox
            .get_lxc_status(&w.node, w.active.vmid)
            .await
            .map_err(api_error)?["data"]["status"]
            == "running",
        volumes: params
            .mountpoints
            .iter()
            .enumerate()
            .map(|(i, _)| format!("mp{}", i))
            .collect(),
        snapshot: None,
        pending_upid: None,
    };
    save_rollout(&s, &rollout).await.map_err(api_error)?;
    let consumed = UpdatePlan { used: true, ..p };
    db(&s)
        .map_err(api_error)?
        .put_record("update_plan", &consumed.id, &json!(consumed))
        .await
        .map_err(api_error)?;
    w.operation = Some(task_id.clone());
    w.status = "updating".into();
    save(&s, &w).await.map_err(api_error)?;
    tokio::spawn(async move {
        let _guard = guard;
        let _lxc_guard = lxc_guard;
        run_update(s, w, consumed, rollout).await;
    });
    Ok((StatusCode::ACCEPTED, Json(json!({"task_id":task_id}))).into_response())
}

/// All direct LXC mutations participate in the same operation guard as rollouts.
pub async fn mutation_guard(
    s: &Arc<AppState>,
    vmid: u32,
) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
    for value in db(s)?.list_records("workload").await? {
        let w: Workload = serde_json::from_value(value).map_err(|e| e.to_string())?;
        if w.previous.iter().any(|v| v.vmid == vmid) {
            return Err("This is a retained previous revision. Use a reviewed rollback plan to reactivate it safely.".into());
        }
        if w.active.vmid == vmid && w.operation.is_some() {
            return Err(
                "Resolve the active/interrupted update before changing this container".into(),
            );
        }
    }
    if let Some(record) = db(s)?.get_record("container", &vmid.to_string()).await? {
        if let Some(task) = record["task_id"].as_str() {
            if s.ansible
                .get(task)
                .await
                .is_some_and(|j| matches!(j.status.as_str(), "queued" | "running" | "interrupted"))
            {
                return Err("A deployment or update still owns this container; inspect that operation first".into());
            }
        }
    }
    s.ansible.lock_resource(&format!("lxc:{}", vmid)).await
}
pub async fn inspect(
    _auth: RequireAuth,
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Error> {
    let job = s
        .ansible
        .get(&id)
        .await
        .ok_or((StatusCode::NOT_FOUND, "Unknown job".into()))?;
    let mut details = json!({"job":job,"actions":[],"observed_at":runtime::now()});
    if let Some(value) = db(&s)
        .map_err(api_error)?
        .get_record("rollout", &id)
        .await
        .map_err(api_error)?
    {
        let r: Rollout = serde_json::from_value(value).map_err(|e| api_error(e.to_string()))?;
        let w = load(&s, &r.workload_id).await.map_err(api_error)?;
        details["rollout"] = json!(r);
        details["source"] = json!(
            owned(&s, &w.node, r.source.vmid, &r.source.owner)
                .await
                .map(|c| json!({"config":c}))
                .unwrap_or_else(|e| json!({"error":e}))
        );
        if let Some(t) = &r.target {
            details["target"] = json!(
                owned(&s, &w.node, t.vmid, &t.owner)
                    .await
                    .map(|c| json!({"config":c}))
                    .unwrap_or_else(|e| json!({"error":e}))
            );
        }
        let committed = w.operation.is_none()
            && (r.stage == "COMPLETE" || w.last_completed_operation.as_deref() == Some(&id));
        if matches!(job.status.as_str(), "interrupted" | "failed")
            && (w.operation.as_deref() == Some(&id) || committed)
        {
            details["actions"] = json!(if committed {
                vec!["confirm_complete"]
            } else {
                vec!["rollback"]
            });
        }
    } else if db(&s)
        .map_err(api_error)?
        .get_record("deployment", &id)
        .await
        .map_err(api_error)?
        .is_some()
    {
        let p = request(&id).map_err(api_error)?;
        let node = p.node.as_deref().unwrap_or(&s.default_node);
        details["resource"] = json!({"vmid":p.vmid,"node":node,"ownership":owned(&s,node,p.vmid,&id).await.map(|c| json!({"config":c})).unwrap_or_else(|e| json!({"error":e}))});
        if matches!(job.status.as_str(), "interrupted" | "failed" | "cancelled") {
            details["actions"] = json!(["resume"]);
        }
    } else {
        details["message"] = json!(
            "This operation has no automatic recovery procedure. Inspect the related database or guest; retrying an unknown shell command is not safe."
        );
    }
    s.ansible
        .event(
            &id,
            "info",
            "INSPECT",
            "Read live ownership/configuration without changing the resource",
        )
        .await
        .map_err(api_error)?;
    Ok(Json(details))
}
#[derive(Deserialize)]
pub struct RecoverRequest {
    pub action: String,
}
pub async fn recover(
    _auth: RequireAuth,
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(rq): Json<RecoverRequest>,
) -> Result<Response, Error> {
    let response_id = id.clone();
    let job = s
        .ansible
        .get(&id)
        .await
        .ok_or((StatusCode::NOT_FOUND, "Unknown job".into()))?;
    if !matches!(job.status.as_str(), "interrupted" | "failed" | "cancelled") {
        return Err((
            StatusCode::CONFLICT,
            "Only terminal failed or interrupted operations can be recovered".into(),
        ));
    }
    if let Some(value) = db(&s)
        .map_err(api_error)?
        .get_record("rollout", &id)
        .await
        .map_err(api_error)?
    {
        let mut r: Rollout = serde_json::from_value(value).map_err(|e| api_error(e.to_string()))?;
        let guard = s
            .ansible
            .lock_resource(&format!("workload:{}", r.workload_id))
            .await
            .map_err(|e| (StatusCode::CONFLICT, e))?;
        let mut w = load(&s, &r.workload_id).await.map_err(api_error)?;
        if (r.stage == "COMPLETE" || w.last_completed_operation.as_deref() == Some(&id))
            && rq.action == "confirm_complete"
            && w.operation.is_none()
        {
            owned(&s, &w.node, w.active.vmid, &w.active.owner)
                .await
                .map_err(api_error)?;
            ready(&s, &w, w.active.vmid).await.map_err(api_error)?;
            r.stage = "COMPLETE".into();
            save_rollout(&s, &r).await.map_err(api_error)?;
            s.ansible
                .event(
                    &id,
                    "ok",
                    "COMPLETE",
                    "Confirmed committed revision against live ownership and health",
                )
                .await
                .map_err(api_error)?;
            return Ok(Json(json!({"status":"succeeded"})).into_response());
        }
        if rq.action != "rollback" || w.operation.as_deref() != Some(&id) {
            return Err((
                StatusCode::CONFLICT,
                "This recovery action is not valid for the current workload".into(),
            ));
        }
        s.ansible
            .event(
                &id,
                "task",
                "RECOVERY",
                "Reconciling the interrupted update before restoring the previous container",
            )
            .await
            .map_err(api_error)?;
        tokio::spawn(async move {
            let _guard = guard;
            let result: Result<(), String> = async {
                if let Some(upid) = r.pending_upid.clone() {
                    if s.proxmox.task_status(&w.node, &upid).await?["data"]["status"] != "stopped" {
                        let _ = s.proxmox.wait_for_task(&w.node, &upid).await;
                    }
                }
                // A crash can occur after guest creation and before the rollout records the assigned ID.
                if r.target.is_none() && r.mode != "recipe" {
                    if let Ok(p) = request(&id) {
                        if owned(&s, &w.node, p.vmid, &id).await.is_ok() {
                            r.target = Some(Revision {
                                vmid: p.vmid,
                                owner: id.clone(),
                                image: p.image.clone(),
                                pinned_image: None,
                                spec: id.clone(),
                                created_at: runtime::now(),
                            });
                        }
                    }
                }
                undo(&s, &w, &mut r).await?;
                w.active = r.source.clone();
                w.operation = None;
                w.status = if r.source_running { "ready" } else { "stopped" }.into();
                save(&s, &w).await?;
                Ok(())
            }
            .await;
            match result {
                Ok(()) => {
                    let _=s.ansible.event(&id,"warn","FAILED","Recovered the previous container and verified its health. Create a new update plan to retry.").await;
                }
                Err(e) => {
                    let _ = s.ansible.event(&id, "error", "INTERRUPTED", &e).await;
                }
            }
        });
    } else {
        if rq.action != "resume" || job.kind != "deploy" {
            return Err(api_error(
                "This operation does not support automatic resume".into(),
            ));
        }
        let mut p = request(&id).map_err(api_error)?;
        let guard = s
            .ansible
            .lock_resource(&format!("lxc:{}", p.vmid))
            .await
            .map_err(|e| (StatusCode::CONFLICT, e))?;
        let node = p.node.clone().unwrap_or(s.default_node.clone());
        let resources = s.proxmox.get_cluster_resources().await.map_err(api_error)?;
        let exists = resources["data"]
            .as_array()
            .is_some_and(|a| a.iter().any(|v| v["vmid"].as_u64() == Some(p.vmid as u64)));
        if exists {
            owned(&s, &node, p.vmid, &id).await.map_err(api_error)?;
        }
        let new = s
            .ansible
            .create("deploy_recovery", &job.resource)
            .await
            .map_err(api_error)?;
        let copy = new.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let result: Result<(), String> = async {
                if exists {
                    owned(&s, &node, p.vmid, &id).await?;
                    if load(&s,&p.vmid.to_string()).await.is_err(){register(&s,&id,&p,&node,true).await?;db(&s)?.put_record("container",&p.vmid.to_string(),&json!({"id":p.vmid,"node":node,"name":p.hostname,"task_id":id,"status":"created"})).await?;}
                    start(&s, &node, p.vmid).await?;
                    let w = load(&s, &p.vmid.to_string()).await?;
                    let ip = ready(&s, &w, p.vmid).await?;
                    mark_ready(&s, p.vmid, &ip).await?;
                    if p.expose_secureweb {
                        s.secureweb
                            .register_route(crate::secureweb::SecureWebRoute {
                                domain: p.secureweb_domain.clone().ok_or("Missing domain")?,
                                target_ip: ip,
                                target_port: p.app_port.unwrap_or(80),
                                service_name: p.hostname.clone(),
                                vmid: p.vmid,
                                mode: "https".into(),
                                created_at: Some(runtime::now().to_string()),
                            })
                            .await?;
                    }
                } else {
                    crate::ansible::deploy(&s, &copy, &mut p).await?;
                }
                s.ansible
                    .event(
                        &copy,
                        "ok",
                        "COMPLETE",
                        "Deployment recovered and readiness verified",
                    )
                    .await?;
                s.ansible
                    .event(
                        &id,
                        "ok",
                        "COMPLETE",
                        "Recovered by a separate deployment recovery job",
                    )
                    .await?;
                Ok(())
            }
            .await;
            if let Err(e) = result {
                s.ansible.finish_error(&copy, &e).await;
            }
        });
        return Ok((StatusCode::ACCEPTED, Json(json!({"task_id":new}))).into_response());
    }
    Ok((StatusCode::ACCEPTED, Json(json!({"task_id":response_id}))).into_response())
}
pub async fn reconcile(s: &AppState) -> Result<(), String> {
    for value in db(s)?.list_records("workload").await? {
        let mut w: Workload = serde_json::from_value(value).map_err(|e| e.to_string())?;
        if w.operation.is_some() {
            w.status = "recovery_required".into();
            save(s, &w).await?;
        }
    }
    Ok(())
}

pub async fn schedule(s: &Arc<AppState>) -> Result<(), String> {
    for value in db(s)?.list_records("workload").await? {
        let w: Workload = serde_json::from_value(value).map_err(|e| e.to_string())?;
        if w.status != "ready"
            || w.operation.is_some()
            || w.update.interval_hours == 0
            || w.update.mode == "image"
            || w.health.port.is_none()
        {
            continue;
        }
        let last = db(s)?
            .get_record("update_attempt", &w.id)
            .await?
            .and_then(|v| v["at"].as_u64())
            .unwrap_or(w.active.created_at)
            .max(w.last_update_at.unwrap_or(0));
        if last + u64::from(w.update.interval_hours) * 3600 > runtime::now() {
            continue;
        }
        db(s)?
            .put_record("update_attempt", &w.id, &json!({"at":runtime::now()}))
            .await?;
        let preview = plan(
            RequireAuth,
            State(s.clone()),
            Path(w.id.clone()),
            Json(PlanRequest {
                mode: "recipe".into(),
                image: None,
                backup_storage: w.update.backup_storage.clone(),
                database_id: None,
            }),
        )
        .await
        .map_err(|(_, e)| e)?;
        let id = preview.0["plan"]["id"]
            .as_str()
            .ok_or("Missing scheduled plan")?
            .to_string();
        apply(
            RequireAuth,
            State(s.clone()),
            Path(w.id),
            Json(ApplyRequest { plan_id: id }),
        )
        .await
        .map_err(|(_, e)| e)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn application_health_checks_use_http_status_not_just_a_listening_port() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0; 1024];
            let _ = socket.read(&mut buf).await;
            socket
                .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        assert!(
            check_health(
                "127.0.0.1",
                &HealthCheck {
                    port: Some(port),
                    http_path: Some("/health".into()),
                    expected_status: Some(200)
                }
            )
            .await
            .unwrap_err()
            .contains("503")
        );
    }
}
