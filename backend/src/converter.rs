use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct ConvertRequest {
    pub dockerfile: String,
}

#[derive(Serialize)]
pub struct ConvertResponse {
    pub yaml: String,
}

pub async fn convert_dockerfile_endpoint(
    _auth: crate::RequireAuth,
    Json(payload): Json<ConvertRequest>,
) -> impl IntoResponse {
    let yaml = convert_dockerfile_to_distrobuilder(&payload.dockerfile);
    (StatusCode::OK, Json(ConvertResponse { yaml }))
}

// Compatibility routes use the same validated, durable deployment executor.
fn canonical(mut value: serde_json::Value) -> Result<crate::ansible::AnsibleDeployParams, String> {
    let p = value
        .as_object_mut()
        .ok_or("Expected a deployment object")?;
    if p.get("use_hostable_db").and_then(|v| v.as_bool()) == Some(true) {
        return Err("Create a hosted application database and pass database_id; shared metadata credentials are unsupported".into());
    }
    if let Some(images) = p.remove("images") {
        let images = images.as_array().ok_or("images must be an array")?;
        if images.len() != 1 {
            return Err(
                "Deploy each service separately; merging multiple image filesystems is unsupported"
                    .into(),
            );
        }
        p.insert("image".into(), images[0].clone());
    }
    if let Some(memory) = p.get("memory").and_then(|v| v.as_str()) {
        let n: u32 = memory
            .parse()
            .map_err(|_| "memory must be an integer in MiB")?;
        p.insert("memory".into(), n.into());
    }
    if let Some(envs) = p.get("env_vars").and_then(|v| v.as_array()) {
        let mut map = serde_json::Map::new();
        for e in envs {
            let (k, v) = e
                .as_str()
                .and_then(|s| s.split_once('='))
                .ok_or("Invalid environment entry")?;
            map.insert(k.into(), v.into());
        }
        p.insert("env_vars".into(), map.into());
    }
    if let Some(volumes) = p.remove("volumes") {
        let mut mounts = Vec::new();
        for vol in volumes.as_array().ok_or("volumes must be an array")? {
            if vol.is_object() {
                mounts.push(vol.clone());
                continue;
            }
            let parts: Vec<_> = vol.as_str().ok_or("Invalid volume")?.split(':').collect();
            let mount = match parts.as_slice() {
                ["storage", storage, size, path] => {
                    serde_json::json!({"storage":storage,"size_gb":crate::ansible::disk_gb(size)?,"container":path})
                }
                ["bind", host, path] | [host, path] => {
                    serde_json::json!({"host":host,"container":path})
                }
                _ => {
                    return Err("Use structured volumes with storage, size_gb and container".into());
                }
            };
            mounts.push(mount);
        }
        p.insert("mountpoints".into(), mounts.into());
    }
    serde_json::from_value(value).map_err(|e| e.to_string())
}
pub async fn deploy_lxc_endpoint(
    auth: crate::RequireAuth,
    state: axum::extract::State<std::sync::Arc<crate::AppState>>,
    Json(value): Json<serde_json::Value>,
) -> axum::response::Response {
    match canonical(value) {
        Ok(params) => crate::ansible::trigger_deploy_handler(auth, state, Json(params)).await,
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"status":"error","error":error})),
        )
            .into_response(),
    }
}
pub async fn deploy_stack_endpoint(
    auth: crate::RequireAuth,
    state: axum::extract::State<std::sync::Arc<crate::AppState>>,
    value: Json<serde_json::Value>,
) -> axum::response::Response {
    deploy_lxc_endpoint(auth, state, value).await
}

pub fn convert_dockerfile_to_distrobuilder(dockerfile: &str) -> String {
    let mut actions = Vec::new();
    let mut env_vars = Vec::new();
    let mut base_image = "alpinelinux".to_string();
    let mut release = "3.19".to_string(); // Default to a recent Alpine release

    let mut logical_lines = Vec::new();
    let mut current_line = String::new();

    for line in dockerfile.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if trimmed.ends_with('\\') {
            let stripped = trimmed[..trimmed.len() - 1].trim_end();
            current_line.push_str(stripped);
            current_line.push(' ');
        } else {
            current_line.push_str(trimmed);
            logical_lines.push(current_line);
            current_line = String::new();
        }
    }

    // Push any remaining line if it ends abruptly
    if !current_line.is_empty() {
        logical_lines.push(current_line);
    }

    for line in logical_lines {
        if let Some(rest) = line.strip_prefix("FROM ") {
            let image_part = rest.split_whitespace().next().unwrap_or("alpine:latest");
            if image_part.starts_with("alpine") || image_part.contains("alpine") {
                base_image = "alpinelinux".to_string();
                if let Some((_, r)) = image_part.split_once(':') {
                    if r != "latest" {
                        release = r.to_string();
                    }
                }
            } else if image_part.starts_with("ubuntu") || image_part.contains("ubuntu") {
                base_image = "ubuntu".to_string();
                if let Some((_, r)) = image_part.split_once(':') {
                    release = r.to_string();
                } else {
                    release = "jammy".to_string(); // Placeholder
                }
            }
        } else if let Some(rest) = line.strip_prefix("RUN ") {
            // Append to actions shell script
            actions.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("ENV ") {
            // Split by space for multiple envs on one line, or just take the whole thing
            // Complex linuxserver ENV blocks have format: ENV KEY=VAL KEY2=VAL2
            // We can just dump the whole thing as a single export since they use KEY=VAL
            env_vars.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("CMD ") {
            actions.push(format!("echo '#!/bin/sh' > /etc/local.d/hostable.start"));
            // Safe escape for complex commands
            actions.push(format!(
                "echo '{}' >> /etc/local.d/hostable.start",
                rest.replace("'", "'\\''")
            ));
            actions.push("chmod +x /etc/local.d/hostable.start".to_string());
            actions.push("rc-update add local default".to_string());
        }
    }

    let mut yaml = String::new();
    yaml.push_str("image:\n");
    yaml.push_str(&format!("  distribution: {}\n", base_image));
    yaml.push_str(&format!("  release: {}\n", release));
    yaml.push_str("  architecture: x86_64\n\n");

    // Add default sources (simplistic example)
    yaml.push_str("source:\n");
    yaml.push_str("  downloader: alpinelinux-http\n");
    yaml.push_str("  url: http://dl-cdn.alpinelinux.org/alpine\n\n");

    if !actions.is_empty() || !env_vars.is_empty() {
        yaml.push_str("actions:\n");
        yaml.push_str("  - trigger: post-packages\n");
        yaml.push_str("    action: |-\n");
        yaml.push_str("      #!/bin/sh\n");
        yaml.push_str("      set -e\n");
        for env in env_vars {
            yaml.push_str(&format!("      export {}\n", env));
        }
        for act in actions {
            yaml.push_str(&format!("      {}\n", act));
        }
    }

    yaml
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_alpine_nginx() {
        let dockerfile = "FROM alpine:3.19\nRUN apk add nginx\nENV PORT=80\nCMD [\"nginx\", \"-g\", \"daemon off;\"]";
        let yaml = convert_dockerfile_to_distrobuilder(dockerfile);

        assert!(yaml.contains("distribution: alpinelinux"));
        assert!(yaml.contains("release: 3.19"));
        assert!(yaml.contains("export PORT=80"));
        assert!(yaml.contains("apk add nginx"));
        assert!(yaml.contains("nginx"));
    }

    #[test]
    fn test_convert_ubuntu_base() {
        let dockerfile = "FROM ubuntu:22.04\nRUN apt-get update";
        let yaml = convert_dockerfile_to_distrobuilder(dockerfile);

        assert!(yaml.contains("distribution: ubuntu"));
        assert!(yaml.contains("release: 22.04"));
        assert!(yaml.contains("apt-get update"));
    }
}
