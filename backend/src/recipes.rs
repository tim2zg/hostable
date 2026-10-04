use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HealthCheck {
    pub port: Option<u16>,
    pub http_path: Option<String>,
    pub expected_status: Option<u16>,
}
impl HealthCheck {
    pub fn validate(&self) -> Result<(), String> {
        if self.port == Some(0)
            || self.http_path.as_ref().is_some_and(|p| {
                !p.starts_with('/') || p.len() > 512 || p.contains(['\r', '\n', '#'])
            })
            || self.http_path.is_some() && self.port.is_none()
            || self
                .expected_status
                .is_some_and(|s| !(200..=399).contains(&s))
        {
            return Err("Invalid TCP/HTTP health check".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdatePolicy {
    pub mode: String,
    #[serde(default)]
    pub script: String,
    #[serde(default)]
    pub restart_script: String,
    #[serde(default = "update_timeout")]
    pub timeout_seconds: u32,
    #[serde(default)]
    pub interval_hours: u32,
    #[serde(default)]
    pub backup_storage: Option<String>,
}
fn update_timeout() -> u32 {
    900
}
impl Default for UpdatePolicy {
    fn default() -> Self {
        Self {
            mode: "image".into(),
            script: String::new(),
            restart_script: String::new(),
            timeout_seconds: 900,
            interval_hours: 0,
            backup_storage: None,
        }
    }
}
impl UpdatePolicy {
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.mode.as_str(), "image" | "apt" | "apk" | "shell")
            || !(30..=3600).contains(&self.timeout_seconds)
            || self.interval_hours > 8760
            || self
                .backup_storage
                .as_ref()
                .is_some_and(|s| !crate::ansible::identifier(s))
            || self.script.len() > 16384
            || self.restart_script.len() > 4096
            || self.script.contains('\0')
            || self.restart_script.contains('\0')
            || self.mode == "shell" && self.script.trim().is_empty()
        {
            return Err("Invalid update policy".into());
        }
        if self.interval_hours > 0 && self.mode == "image" {
            return Err("Image replacements require a reviewed update plan; scheduling is supported for package/shell recipes".into());
        }
        Ok(())
    }
    pub fn command(&self) -> Result<String, String> {
        self.validate()?;
        let update = match self.mode.as_str() {
            "apt" => {
                "command -v apt-get >/dev/null\nexport DEBIAN_FRONTEND=noninteractive\napt-get update\napt-get -y -o Dpkg::Options::=--force-confold upgrade"
            }
            "apk" => "command -v apk >/dev/null\napk update\napk upgrade",
            "shell" => self.script.as_str(),
            _ => return Err("This recipe uses image replacement".into()),
        };
        Ok(format!("set -eu\n{}\n{}\n", update, self.restart_script))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recipe {
    pub schema_version: u32,
    pub id: String,
    pub version: u32,
    pub title: String,
    pub description: String,
    pub image: String,
    #[serde(default)]
    pub required_env: Vec<String>,
    #[serde(default)]
    pub volumes: Vec<crate::ansible::MountPointParam>,
    #[serde(default)]
    pub health: HealthCheck,
    #[serde(default)]
    pub update: UpdatePolicy,
    #[serde(default)]
    pub validation: String,
}
pub fn validate(r: &Recipe) -> Result<(), String> {
    if r.schema_version != 1
        || r.version == 0
        || !crate::ansible::identifier(&r.id)
        || r.title.is_empty()
        || r.title.len() > 100
        || r.description.len() > 2048
        || !crate::oci::validate_image_ref(&r.image)
        || r.required_env.len() > 64
        || r.required_env.iter().any(|e| !crate::oci::valid_env_key(e))
    {
        return Err("Invalid versioned recipe".into());
    }
    r.health.validate()?;
    r.update.validate()?;
    for v in &r.volumes {
        v.spec()?;
    }
    Ok(())
}
pub fn builtin() -> Vec<Recipe> {
    vec![Recipe {
        schema_version: 1,
        id: "nginx".into(),
        version: 1,
        title: "Nginx".into(),
        description: "HTTP server converted from an OCI image into an unprivileged LXC".into(),
        image: "nginx:1.28-alpine".into(),
        required_env: vec![],
        volumes: vec![],
        health: HealthCheck {
            port: Some(80),
            http_path: Some("/".into()),
            expected_status: Some(200),
        },
        update: UpdatePolicy::default(),
        validation: "Proxmox smoke test pending".into(),
    }]
}
pub async fn all(state: &crate::AppState) -> Result<Vec<Recipe>, String> {
    let mut rows = builtin();
    for v in state
        .db
        .as_ref()
        .ok_or("Metadata unavailable")?
        .list_records("recipe")
        .await?
    {
        rows.push(serde_json::from_value(v).map_err(|e| e.to_string())?);
    }
    rows.sort_by_key(|r| (r.id.clone(), r.version));
    Ok(rows)
}
pub async fn resolve(state: &crate::AppState, id: &str, version: u32) -> Result<Recipe, String> {
    all(state)
        .await?
        .into_iter()
        .find(|r| r.id == id && r.version == version)
        .ok_or("Unknown recipe version".into())
}
pub async fn list(
    _auth: crate::RequireAuth,
    State(state): State<Arc<crate::AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    all(&state)
        .await
        .map(|r| Json(json!(r)))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))
}
pub async fn import(
    _auth: crate::RequireAuth,
    State(state): State<Arc<crate::AppState>>,
    Json(mut recipe): Json<Recipe>,
) -> Response {
    if let Err(e) = validate(&recipe) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":e}))).into_response();
    }
    let _guard = match state
        .ansible
        .lock_resource(&format!("recipe:{}@{}", recipe.id, recipe.version))
        .await
    {
        Ok(guard) => guard,
        Err(e) => return (StatusCode::CONFLICT, Json(json!({"error":e}))).into_response(),
    };
    if let Ok(old) = resolve(&state, &recipe.id, recipe.version).await {
        return (StatusCode::CONFLICT, Json(json!({"error":"Recipe versions are immutable. Import a new version.","existing":old.id}))).into_response();
    }
    recipe.validation = "Custom administrator recipe; live acceptance pending".into();
    let id = format!("{}@{}", recipe.id, recipe.version);
    match state
        .db
        .as_ref()
        .unwrap()
        .put_record("recipe", &id, &json!(recipe))
        .await
    {
        Ok(()) => (StatusCode::CREATED, Json(json!(recipe))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error":e}))).into_response(),
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecipeRef {
    pub id: String,
    pub version: u32,
}
pub async fn get(
    _auth: crate::RequireAuth,
    State(state): State<Arc<crate::AppState>>,
    Path((id, version)): Path<(String, u32)>,
) -> Result<Json<Value>, (StatusCode, String)> {
    resolve(&state, &id, version)
        .await
        .map(|r| Json(json!(r)))
        .map_err(|e| (StatusCode::NOT_FOUND, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recipes_require_explicit_bounded_updates_and_relative_health_paths() {
        let mut r = builtin().remove(0);
        assert!(validate(&r).is_ok());
        r.health.http_path = Some("http://elsewhere/".into());
        assert!(validate(&r).is_err());
        r.health.http_path = Some("/".into());
        r.update.mode = "shell".into();
        assert!(validate(&r).is_err());
        r.update.script = "my-application update".into();
        assert!(validate(&r).is_ok());
        r.update.timeout_seconds = 0;
        assert!(validate(&r).is_err());
    }
}
