use axum::{
    Json,
    extract::{
        Path, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use futures::{SinkExt, stream::StreamExt};
use serde_json::{Value, json};
use std::sync::Arc;

use crate::{AppState, RequireAuth};

pub async fn get_lxcs(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
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

pub async fn start_lxc_handler(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(vmid): Path<u32>,
) -> Json<Value> {
    let node = state.default_node.clone();
    match state.proxmox.start_lxc(&node, vmid).await {
        Ok(_) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"status": "error", "error": e.to_string()})),
    }
}

pub async fn stop_lxc_handler(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(vmid): Path<u32>,
) -> Json<Value> {
    let node = state.default_node.clone();
    match state.proxmox.stop_lxc(&node, vmid).await {
        Ok(_) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"status": "error", "error": e.to_string()})),
    }
}

pub async fn get_snapshots_handler(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(vmid): Path<u32>,
) -> Json<Value> {
    let node = state.default_node.clone();
    match state.proxmox.get_snapshots(&node, vmid).await {
        Ok(data) => Json(data),
        Err(e) => Json(json!({"status": "error", "error": e.to_string()})),
    }
}

#[derive(serde::Deserialize)]
pub struct CreateSnapshotReq {
    pub snapname: String,
    pub description: Option<String>,
}

pub async fn create_snapshot_handler(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(vmid): Path<u32>,
    Json(payload): Json<CreateSnapshotReq>,
) -> Json<Value> {
    let node = state.default_node.clone();
    let desc = payload.description.unwrap_or_default();
    match state
        .proxmox
        .create_snapshot(&node, vmid, &payload.snapname, &desc)
        .await
    {
        Ok(_) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"status": "error", "error": e.to_string()})),
    }
}

pub async fn rollback_snapshot_handler(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path((vmid, snapname)): Path<(u32, String)>,
) -> Json<Value> {
    let node = state.default_node.clone();
    match state
        .proxmox
        .rollback_snapshot(&node, vmid, &snapname)
        .await
    {
        Ok(_) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"status": "error", "error": e.to_string()})),
    }
}

pub async fn delete_snapshot_handler(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path((vmid, snapname)): Path<(u32, String)>,
) -> Json<Value> {
    let node = state.default_node.clone();
    match state.proxmox.delete_snapshot(&node, vmid, &snapname).await {
        Ok(_) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"status": "error", "error": e.to_string()})),
    }
}

pub async fn restart_lxc_handler(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(vmid): Path<u32>,
) -> Json<Value> {
    let node = state.default_node.clone();
    let _ = state.proxmox.stop_lxc(&node, vmid).await;
    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
    match state.proxmox.start_lxc(&node, vmid).await {
        Ok(_) => Json(json!({"status": "ok"})),
        Err(e) => Json(json!({"status": "error", "error": e.to_string()})),
    }
}

pub async fn rrddata_lxc_handler(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(vmid): Path<u32>,
) -> Result<Json<Value>, (axum::http::StatusCode, String)> {
    let node = state.default_node.clone();
    match state.proxmox.get_rrddata(&node, Some(vmid), "hour").await {
        Ok(data) => Ok(Json(data)),
        Err(e) => Err((axum::http::StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

pub async fn ws_logs_handler(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    Path(vmid): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| handle_log_socket(socket, vmid, state))
}

async fn handle_log_socket(mut socket: WebSocket, vmid_str: String, state: Arc<AppState>) {
    if std::env::var("PROXMOX_MOCK").is_ok() {
        let _ = socket
            .send(Message::Text(
                "Mock mode active. Terminal unavailable.\r\n".into(),
            ))
            .await;
        return;
    }

    let vmid: u32 = match vmid_str.parse() {
        Ok(v) => v,
        Err(_) => {
            let _ = socket.send(Message::Text("Invalid VMID.\r\n".into())).await;
            return;
        }
    };

    let node = state.default_node.clone();
    let (ticket, port) = match state.proxmox.create_termproxy(&node, vmid).await {
        Ok((t, p)) => (t, p),
        Err(e) => {
            let _ = socket
                .send(Message::Text(
                    format!("Failed to create termproxy: {}\r\n", e).into(),
                ))
                .await;
            return;
        }
    };

    let host = state.proxmox.get_host();
    let safe_ticket = ticket
        .replace(":", "%3A")
        .replace("=", "%3D")
        .replace("/", "%2F");
    let ws_url = format!(
        "wss://{}:8006/api2/json/nodes/{}/lxc/{}/vncwebsocket?port={}&vncticket={}",
        host, node, vmid, port, safe_ticket
    );

    use native_tls::TlsConnector;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::{Connector, connect_async_tls_with_config};

    let mut builder = TlsConnector::builder();
    if state.proxmox.is_insecure() {
        builder.danger_accept_invalid_certs(true);
        builder.danger_accept_invalid_hostnames(true);
    }
    let connector = match builder.build() {
        Ok(c) => Connector::NativeTls(c),
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("TLS config error: {}\r\n", e).into()))
                .await;
            return;
        }
    };

    let mut request = match ws_url.into_client_request() {
        Ok(r) => r,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("WS URL error: {}\r\n", e).into()))
                .await;
            return;
        }
    };

    request.headers_mut().insert(
        "Cookie",
        format!("PVEAuthCookie={}", ticket).parse().unwrap(),
    );

    let (proxmox_ws, _) =
        match connect_async_tls_with_config(request, None, false, Some(connector)).await {
            Ok(ws) => ws,
            Err(e) => {
                let _ = socket
                    .send(Message::Text(
                        format!("Failed to connect to Proxmox WS: {}\r\n", e).into(),
                    ))
                    .await;
                return;
            }
        };

    let (mut px_sender, mut px_receiver) = proxmox_ws.split();
    let (mut client_sender, mut client_receiver) = socket.split();

    // Authenticate with termproxy
    let auth_msg = format!("root@pam:{}\n", ticket);
    let _ = px_sender
        .send(tokio_tungstenite::tungstenite::Message::Text(auth_msg))
        .await;

    let client_to_proxmox = tokio::spawn(async move {
        while let Some(Ok(msg)) = client_receiver.next().await {
            match msg {
                Message::Text(t) => {
                    let _ = px_sender
                        .send(tokio_tungstenite::tungstenite::Message::Text(t.to_string()))
                        .await;
                }
                Message::Binary(b) => {
                    let _ = px_sender
                        .send(tokio_tungstenite::tungstenite::Message::Binary(b.to_vec()))
                        .await;
                }
                _ => {}
            }
        }
    });

    let proxmox_to_client = tokio::spawn(async move {
        while let Some(Ok(msg)) = px_receiver.next().await {
            match msg {
                tokio_tungstenite::tungstenite::Message::Text(t) => {
                    let _ = client_sender.send(Message::Text(t.into())).await;
                }
                tokio_tungstenite::tungstenite::Message::Binary(b) => {
                    let _ = client_sender.send(Message::Binary(b.into())).await;
                }
                _ => {}
            }
        }
    });

    tokio::select! {
        _ = client_to_proxmox => {},
        _ = proxmox_to_client => {},
    }
}
