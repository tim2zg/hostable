use axum::{
    extract::State,
    Json,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use std::sync::Arc;

use crate::{AppState, RequireAuth};

#[derive(Deserialize)]
pub struct DbQuery {
    pub sql: String,
}

pub async fn db_execute(_auth: RequireAuth, State(state): State<Arc<AppState>>, Json(payload): Json<DbQuery>) -> Json<Value> {
    let sql_lower = payload.sql.trim().to_lowercase();
    
    // Only allow SELECT statements to prevent SQL injection modifications
    if !sql_lower.starts_with("select") {
        return Json(json!({
            "status": "error",
            "error": "Security Error: Only SELECT queries are allowed through this interface."
        }));
    }
    
    // Prevent querying the users table completely
    if sql_lower.contains("users") {
        return Json(json!({
            "status": "error",
            "error": "Security Error: Querying the users table is prohibited."
        }));
    }

    if let Some(pool) = &state.pool {
        match sqlx::query(&payload.sql).execute(pool).await {
            Ok(res) => Json(json!({"status": "ok", "rows_affected": res.rows_affected()})),
            Err(e) => Json(json!({"status": "error", "error": e.to_string()})),
        }
    } else {
        Json(json!({"status": "mock", "message": "No DB connection in mock mode"}))
    }
}

pub async fn db_schema(_auth: RequireAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    if let Some(pool) = &state.pool {
        let rows = sqlx::query("SELECT tablename FROM pg_tables WHERE schemaname='public'")
            .fetch_all(pool)
            .await;
        match rows {
            Ok(r) => {
                let tables: Vec<String> = r.iter().map(|row| row.get::<String, _>("tablename")).collect();
                Json(json!({"status": "ok", "tables": tables}))
            }
            Err(e) => Json(json!({"status": "error", "error": e.to_string()})),
        }
    } else {
        Json(json!({"status": "mock", "tables": ["users", "proxy_rules", "example"]}))
    }
}
