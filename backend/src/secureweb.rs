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
    "https".to_string()
}

pub fn valid_domain(domain: &str) -> bool {
    !domain.is_empty()
        && domain.len() <= 253
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
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
    db: Arc<crate::db::DbBackend>,
    token: String,
}

impl SecureWebClient {
    pub fn new(db: Arc<crate::db::DbBackend>) -> Self {
        Self {
            gateway_url: env::var("SECUREWEB_GATEWAY_URL")
                .unwrap_or_default()
                .trim_end_matches('/')
                .into(),
            token: env::var("SECUREWEB_GATEWAY_TOKEN").unwrap_or_default(),
            db,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(8))
                .build()
                .expect("Gateway HTTP client"),
        }
    }
    fn request(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, String> {
        if self.gateway_url.is_empty() {
            return Err("Configure SECUREWEB_GATEWAY_URL before adding ingress".into());
        }
        let url = reqwest::Url::parse(&self.gateway_url).map_err(|_| "Invalid gateway URL")?;
        if !matches!(url.scheme(), "https" | "http")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err("Invalid gateway URL".into());
        }
        let mut request = self
            .client
            .request(method, format!("{}{}", self.gateway_url, path));
        if !self.token.is_empty() {
            request = request.bearer_auth(&self.token);
        }
        Ok(request)
    }
    pub async fn check_health(&self) -> bool {
        match self.request(reqwest::Method::GET, "/api/health") {
            Ok(request) => request.send().await.is_ok_and(|r| r.status().is_success()),
            Err(_) => false,
        }
    }
    pub async fn register_route(&self, route: SecureWebRoute) -> Result<(), String> {
        if !valid_domain(&route.domain)
            || route.target_ip.parse::<std::net::IpAddr>().is_err()
            || route.target_port == 0
            || route.mode != "https"
        {
            return Err("A valid domain, IP, port and https mode are required".into());
        }
        self.db
            .put_record("gateway_desired", &route.domain, &json!(route))
            .await?;
        self.request(reqwest::Method::POST, "/api/routes")?
            .json(&route)
            .send()
            .await
            .map_err(|_| "Gateway unavailable")?
            .error_for_status()
            .map_err(|e| {
                format!(
                    "Gateway rejected route: {}",
                    e.status().unwrap_or(StatusCode::BAD_GATEWAY)
                )
            })?;
        self.db
            .put_record("gateway_route", &route.domain, &json!(route))
            .await?;
        Ok(())
    }
    pub async fn list_routes(&self) -> Vec<SecureWebRoute> {
        if let Ok(request) = self.request(reqwest::Method::GET, "/api/routes") {
            if let Ok(response) = request.send().await {
                if response.status().is_success() {
                    if let Ok(routes) = response.json::<Vec<SecureWebRoute>>().await {
                        return routes;
                    }
                }
            }
        }
        self.db
            .list_records("gateway_route")
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect()
    }
    pub async fn remove_route(&self, domain: &str) -> Result<(), String> {
        if !valid_domain(domain) {
            return Err("Invalid domain".into());
        }
        self.request(reqwest::Method::DELETE, &format!("/api/routes/{}", domain))?
            .send()
            .await
            .map_err(|_| "Gateway unavailable")?
            .error_for_status()
            .map_err(|_| "Gateway rejected route deletion")?;
        self.db.delete_record("gateway_route", domain).await?;
        self.db.delete_record("gateway_desired", domain).await
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
        active_routes: if connected { routes.len() } else { 0 },
        e2ee_mode: "HTTPS configuration is managed by the external gateway; encryption has not been independently verified".into(),
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
    if !valid_domain(&payload.domain) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"status": "error", "error": "Invalid domain"})),
        );
    }
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
    if !valid_domain(&domain) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"status": "error", "error": "Invalid domain"})),
        );
    }
    match state.secureweb.remove_route(&domain).await {
        Ok(_) => (StatusCode::OK, Json(json!({"status": "ok"}))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"status": "error", "error": e})),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::valid_domain;

    #[test]
    fn test_domain_validation() {
        assert!(valid_domain("app.example.com"));
        assert!(valid_domain("localhost"));
        assert!(valid_domain("a-b.c"));
        assert!(!valid_domain(""));
        assert!(!valid_domain("."));
        assert!(!valid_domain(".."));
        assert!(!valid_domain("bad/../domain"));
        assert!(!valid_domain("bad?domain"));
        assert!(!valid_domain("a..b"));
        assert!(!valid_domain(".lead.example"));
        assert!(!valid_domain("trail.example."));
        assert!(!valid_domain("-dash.example"));
        assert!(!valid_domain("under_score.example"));
        assert!(!valid_domain(&format!("{}.example", "x".repeat(64))));
    }
}
