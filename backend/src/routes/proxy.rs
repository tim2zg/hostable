use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use std::sync::Arc;

use crate::{AppState, RequireAuth};

#[derive(Serialize, Deserialize)]
pub struct ProxyRule {
    pub id: Option<i32>,
    pub domain: String,
    pub target_ip: String,
    pub target_port: i32,
    pub container_id: Option<i32>,
    pub auth_enabled: Option<bool>,
}

pub async fn get_proxy_rules(_auth: RequireAuth, State(state): State<Arc<AppState>>) -> Result<Json<Vec<ProxyRule>>, (axum::http::StatusCode, String)> {
    if let Some(pool) = &state.pool {
        let rules = sqlx::query("SELECT id, domain, target_ip, target_port, container_id, auth_enabled FROM proxy_rules")
            .fetch_all(pool)
            .await
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            
        let mapped: Vec<ProxyRule> = rules.iter().map(|r| {
            ProxyRule {
                id: Some(r.get("id")),
                domain: r.get("domain"),
                target_ip: r.get("target_ip"),
                target_port: r.get("target_port"),
                container_id: r.get("container_id"),
                auth_enabled: r.try_get("auth_enabled").ok(),
            }
        }).collect();
        return Ok(Json(mapped));
    }
    Err((axum::http::StatusCode::INTERNAL_SERVER_ERROR, "Database not configured".to_string()))
}

pub async fn add_proxy_rule(_auth: RequireAuth, State(state): State<Arc<AppState>>, Json(payload): Json<ProxyRule>) -> Json<Value> {
    if let Some(pool) = &state.pool {
        let _ = sqlx::query("INSERT INTO proxy_rules (domain, target_ip, target_port, container_id, auth_enabled) VALUES ($1, $2, $3, $4, $5)")
            .bind(&payload.domain)
            .bind(&payload.target_ip)
            .bind(payload.target_port)
            .bind(payload.container_id)
            .bind(payload.auth_enabled.unwrap_or(false))
            .execute(pool)
            .await;
    }
    Json(json!({"status": "ok"}))
}

pub async fn delete_proxy_rule(_auth: RequireAuth, State(state): State<Arc<AppState>>, Path(id): Path<i32>) -> Json<Value> {
    if let Some(pool) = &state.pool {
        let _ = sqlx::query("DELETE FROM proxy_rules WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await;
    }
    Json(json!({"status": "ok"}))
}
