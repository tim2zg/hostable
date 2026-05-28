use axum::{
    extract::State,
    Json,
};
use serde_json::{Value, json};
use std::sync::Arc;

use crate::{AppState, RequireAuth};

pub async fn get_catalog(_auth: RequireAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    {
        let cache = state.catalog_cache.read().await;
        if let Some((time, data)) = &*cache {
            if time.elapsed() < std::time::Duration::from_secs(3600) {
                return Json(data.clone());
            }
        }
    }

    let url = "https://api.linuxserver.io/api/v1/images?include_config=false&include_deprecated=false";
    match reqwest::get(url).await {
        Ok(res) => {
            if let Ok(json) = res.json::<Value>().await {
                let mut cache = state.catalog_cache.write().await;
                *cache = Some((std::time::Instant::now(), json.clone()));
                Json(json)
            } else {
                Json(json!({"status": "error", "error": "Failed to parse catalog JSON"}))
            }
        }
        Err(e) => {
            Json(json!({"status": "error", "error": e.to_string()}))
        }
    }
}
