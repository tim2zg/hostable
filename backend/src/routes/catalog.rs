use axum::{Json, extract::State};
use serde_json::{Value, json};
use std::sync::Arc;
pub async fn get_catalog(
    _auth: crate::RequireAuth,
    State(_state): State<Arc<crate::AppState>>,
) -> Json<Value> {
    Json(
        json!([{"name":"nginx","version":1,"title":"Nginx","description":"Standalone HTTP server","image":"nginx:1.28-alpine","port":80,"volumes":[],"validation":"Proxmox smoke test pending"}]),
    )
}
