use axum::{
    extract::{Path, State, ws::{WebSocketUpgrade, WebSocket, Message}},
    response::Response,
    Json,
};
use serde_json::{Value, json};
use std::sync::Arc;
use std::env;
use tokio::process::Command;
use std::process::Stdio;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use futures::{stream::StreamExt, SinkExt};
use tokio::sync::mpsc;

use crate::{AppState, RequireAuth};

pub async fn get_lxcs(_auth: RequireAuth, State(state): State<Arc<AppState>>) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let resources = state.proxmox.get_cluster_resources().await.map_err(|e| {
        tracing::error!("Failed to fetch cluster resources: {}", e);
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e)
    })?;
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

pub async fn rrddata_lxc_handler(_auth: RequireAuth, State(state): State<Arc<AppState>>, Path(vmid): Path<u32>) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let node = env::var("PROXMOX_NODE").unwrap_or_else(|_| "pve".to_string());
    match state.proxmox.get_rrddata(&node, Some(vmid), "hour").await {
        Ok(data) => Ok(Json(data)),
        Err(e) => Err((axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))
    }
}

pub async fn ws_logs_handler(_auth: RequireAuth, Path(vmid): Path<String>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_log_socket(socket, vmid))
}

async fn handle_log_socket(socket: WebSocket, vmid: String) {
    let (mut sender, mut receiver) = socket.split();
    
    // Check if mock mode is active
    if std::env::var("PROXMOX_MOCK").is_ok() {
        let _ = sender.send(Message::Text("Mock mode active. Terminal unavailable.\r\n".into())).await;
        return;
    }

    let mut child = match Command::new("pct")
        .arg("exec")
        .arg(&vmid)
        .arg("--")
        .arg("/bin/bash")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            let _ = sender.send(Message::Text(format!("Failed to start shell: {}\r\n", e).into())).await;
            return;
        }
    };

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();

    let (tx, mut rx) = mpsc::channel::<String>(100);
    let tx_out = tx.clone();
    let tx_err = tx.clone();

    // Read stdout
    tokio::spawn(async move {
        let mut buf = [0; 1024];
        while let Ok(n) = stdout.read(&mut buf).await {
            if n == 0 { break; }
            if let Ok(s) = String::from_utf8(buf[..n].to_vec()) {
                let _ = tx_out.send(s).await;
            }
        }
    });

    // Read stderr
    tokio::spawn(async move {
        let mut buf = [0; 1024];
        while let Ok(n) = stderr.read(&mut buf).await {
            if n == 0 { break; }
            if let Ok(s) = String::from_utf8(buf[..n].to_vec()) {
                let _ = tx_err.send(s).await;
            }
        }
    });

    // Forward to WebSocket
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sender.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    // Read from WebSocket and send to stdin
    while let Some(Ok(msg)) = receiver.next().await {
        if let Message::Text(text) = msg {
            if stdin.write_all(text.as_bytes()).await.is_err() {
                break;
            }
        }
    }
    
    let _ = child.kill().await;
}
