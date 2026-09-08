use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::env;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecureWebRoute {
    pub domain: String,
    pub target_ip: String,
    pub target_port: u16,
    pub service_name: String,
    pub vmid: u32,
    #[serde(default = "default_zero_trust")]
    pub mode: String,
    pub created_at: Option<String>,
}

fn default_zero_trust() -> String {
    "zero-trust".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecureWebStatus {
    pub connected: bool,
    pub gateway_url: String,
    pub active_routes: usize,
    pub e2ee_mode: String,
}

#[derive(Clone)]
pub struct SecureWebClient {
    pub gateway_url: String,
    client: reqwest::Client,
    local_routes: Arc<RwLock<Vec<SecureWebRoute>>>,
}

impl SecureWebClient {
    pub fn new() -> Self {
        let gateway_url = env::var("SECUREWEB_GATEWAY_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:3000".to_string());

        Self {
            gateway_url,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(3))
                .build()
                .unwrap(),
            local_routes: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn check_health(&self) -> bool {
        let url = format!("{}/api/health", self.gateway_url);
        if let Ok(res) = self.client.get(&url).send().await {
            res.status().is_success()
        } else {
            false
        }
    }

    pub async fn register_route(&self, route: SecureWebRoute) -> Result<(), String> {
        // First add to local state
        {
            let mut routes = self.local_routes.write().await;
            routes.retain(|r| r.domain != route.domain);
            routes.push(route.clone());
        }

        let url = format!("{}/api/routes", self.gateway_url);
        tracing::info!(
            "Registering upstream route with SecureWeb Gateway: {} -> {}:{}",
            route.domain,
            route.target_ip,
            route.target_port
        );

        let _ = self.client.post(&url).json(&route).send().await;

        Ok(())
    }

    pub async fn list_routes(&self) -> Vec<SecureWebRoute> {
        let url = format!("{}/api/routes", self.gateway_url);
        if let Ok(res) = self.client.get(&url).send().await {
            if let Ok(routes) = res.json::<Vec<SecureWebRoute>>().await {
                let mut local = self.local_routes.write().await;
                *local = routes.clone();
                return routes;
            }
        }
        self.local_routes.read().await.clone()
    }

    pub async fn remove_route(&self, domain: &str) -> Result<(), String> {
        {
            let mut routes = self.local_routes.write().await;
            routes.retain(|r| r.domain != domain);
        }

        let url = format!("{}/api/routes/{}", self.gateway_url, domain);
        let _ = self.client.delete(&url).send().await;
        Ok(())
    }
}

// ==============================================================================
// Axum Route Handlers for SecureWeb Gateway Integration
// ==============================================================================

pub async fn get_status(
    _auth: crate::RequireAuth,
    State(state): State<Arc<crate::AppState>>,
) -> impl IntoResponse {
    let connected = state.secureweb.check_health().await;
    let routes = state.secureweb.list_routes().await;

    Json(SecureWebStatus {
        connected,
        gateway_url: state.secureweb.gateway_url.clone(),
        active_routes: routes.len(),
        e2ee_mode: if connected {
            "Post-Quantum ML-KEM + X25519 (Zero-Trust)".to_string()
        } else {
            "Local Standalone Gateway Mode".to_string()
        },
    })
}

pub async fn get_routes(
    _auth: crate::RequireAuth,
    State(state): State<Arc<crate::AppState>>,
) -> impl IntoResponse {
    let routes = state.secureweb.list_routes().await;
    Json(routes)
}

pub async fn add_route(
    _auth: crate::RequireAuth,
    State(state): State<Arc<crate::AppState>>,
    Json(payload): Json<SecureWebRoute>,
) -> impl IntoResponse {
    match state.secureweb.register_route(payload).await {
        Ok(_) => (StatusCode::OK, Json(json!({"status": "ok"}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"status": "error", "error": e})),
        ),
    }
}

pub async fn delete_route(
    _auth: crate::RequireAuth,
    State(state): State<Arc<crate::AppState>>,
    Path(domain): Path<String>,
) -> impl IntoResponse {
    match state.secureweb.remove_route(&domain).await {
        Ok(_) => (StatusCode::OK, Json(json!({"status": "ok"}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"status": "error", "error": e})),
        ),
    }
}
