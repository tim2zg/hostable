use axum::{
    extract::{Path, State, ws::{WebSocketUpgrade, WebSocket, Message}},
    response::Response,
    Json,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::time::{interval, Duration};
use std::time::SystemTime;
use std::env;

use crate::{AppState, RequireAuth};

pub async fn get_lxcs(_auth: RequireAuth, State(state): State<Arc<AppState>>) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let resources = state.proxmox.get_cluster_resources().await.map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let mut mapped = Vec::new();
    
    if let Some(data) = resources["data"].as_array() {
        for item in data {
            if item["type"] == "lxc" || item["type"] == "qemu" {
                let id = item["vmid"].as_i64().unwrap_or(0);
                let name = item["name"].as_str().unwrap_or("unknown");
                let status = item["status"].as_str().unwrap_or("stopped");
                let mem = item["maxmem"].as_i64().unwrap_or(0) / 1024 / 1024;
                let cpu = item["cpu"].as_f64().unwrap_or(0.0) * 100.0;
                let tags = item["tags"].as_str().unwrap_or("");
                let vm_type = item["type"].as_str().unwrap_or("unknown");
                
                mapped.push(json!({
                    "id": id,
                    "name": name,
                    "status": status,
                    "mem": format!("{} MB", mem),
                    "cpu": format!("{:.1}%", cpu),
                    "tags": tags,
                    "type": vm_type
                }));
            }
        }
    }
    
    Ok(Json(json!(mapped)))
}

pub async fn start_lxc_handler(_auth: RequireAuth, State(state): State<Arc<AppState>>, Path(vmid): Path<u32>) -> Json<Value> {
    let node = env::var("PROXMOX_NODE").unwrap_or_else(|_| "pve".to_string());
    match state.proxmox.start_lxc(&node, vmid).await {
        Ok(_) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"status": "error", "error": e.to_string()})),
    }
}

pub async fn stop_lxc_handler(_auth: RequireAuth, State(state): State<Arc<AppState>>, Path(vmid): Path<u32>) -> Json<Value> {
    let node = env::var("PROXMOX_NODE").unwrap_or_else(|_| "pve".to_string());
    match state.proxmox.stop_lxc(&node, vmid).await {
        Ok(_) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"status": "error", "error": e.to_string()})),
    }
}

pub async fn restart_lxc_handler(_auth: RequireAuth, State(state): State<Arc<AppState>>, Path(vmid): Path<u32>) -> Json<Value> {
    let node = env::var("PROXMOX_NODE").unwrap_or_else(|_| "pve".to_string());
    let _ = state.proxmox.stop_lxc(&node, vmid).await;
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
    match state.proxmox.start_lxc(&node, vmid).await {
        Ok(_) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"status": "error", "error": e.to_string()})),
    }
}

pub async fn ws_logs_handler(_auth: RequireAuth, Path(vmid): Path<String>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_log_socket(socket, vmid))
}

async fn handle_log_socket(mut socket: WebSocket, vmid: String) {
    let mut ticker = interval(Duration::from_secs(1));
    let mut iterations = 0;
    loop {
        ticker.tick().await;
        iterations += 1;
        if iterations > 300 {
            let _ = socket.send(Message::Text("[Hostable] Connection auto-closed: Session limit of 5 minutes reached to save server resources.".into())).await;
            break;
        }
        let timestamp = SystemTime::now();
        let msg = format!("[{}] VM {} log line", timestamp.elapsed().unwrap_or_default().as_secs(), vmid);
        if socket.send(Message::Text(msg.into())).await.is_err() {
            break;
        }
    }
}
