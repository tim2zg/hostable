
use axum::{Json, response::IntoResponse, http::StatusCode};
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

#[derive(Deserialize)]
pub struct DeployRequest {
    pub image: String,
    pub hostname: String,
    pub vmid: u32,
    pub memory: String,
    pub template_storage: String,
    pub rootfs_storage: String,
    pub env_vars: Vec<String>,
    pub volumes: Vec<String>,
    pub use_hostable_db: bool,
    pub db_name: Option<String>,
}

#[derive(Serialize)]
pub struct DeployResponse {
    pub status: String,
    pub message: String,
}

pub async fn deploy_lxc_endpoint(
    _auth: crate::RequireAuth,
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::AppState>>,
    Json(payload): Json<DeployRequest>,
) -> impl IntoResponse {
    let node = state.default_node.clone();
    
    // 1. Process DB Provisioning
    let mut final_envs = payload.env_vars.clone();
    if payload.use_hostable_db {
        if let Some(db_name) = &payload.db_name {
            if let Some(pool) = &state.pool {
                // Warning: In production, sanitize this db_name
                let safe_db = db_name.replace("\"", "").replace("'", "");
                let q = format!("CREATE DATABASE \"{}\"", safe_db);
                if let Err(e) = sqlx::query(&q).execute(pool).await {
                    println!("Failed to create DB (might exist): {}", e);
                }
                
                if let Ok(ip) = local_ip_address::local_ip() {
                    // For now, assuming hostable connects as admin user, we will just pass the admin credentials or a dedicated user
                    let db_url = format!("postgres://postgres:postgres@{}:5432/{}", ip.to_string(), safe_db);
                    final_envs.push(format!("DATABASE_URL={}", db_url));
                }
            }
        }
    }

    // 2. Extract Docker Image to Proxmox Rootfs
    let extractor = crate::oci::OciExtractor::new();
    let cache_dir = std::path::PathBuf::from("/cache");
    if !cache_dir.exists() {
        let _ = std::fs::create_dir_all(&cache_dir);
    }
    let out_path = cache_dir.join(format!("hostable_vmid_{}.tar.xz", payload.vmid));
    let filename = format!("hostable_vmid_{}.tar.xz", payload.vmid);
    
    match extractor.extract_to_dir(&payload.image, &out_path, Some(&final_envs)).await {
        Ok(_) => {
            // 3. Upload to Proxmox
            match state.proxmox.upload_template(&node, &payload.template_storage, &out_path, &filename).await {
                Ok(_) => {
                    // 4. Create LXC
                    let mut params = std::collections::HashMap::new();
                    params.insert("vmid".to_string(), payload.vmid.to_string());
                    params.insert("ostemplate".to_string(), format!("{}:vztmpl/{}", payload.template_storage, filename));
                    params.insert("hostname".to_string(), payload.hostname.clone());
                    params.insert("memory".to_string(), payload.memory.clone());
                    params.insert("net0".to_string(), "name=eth0,bridge=vmbr0,ip=dhcp".to_string());
                    params.insert("storage".to_string(), payload.rootfs_storage.clone());
                    params.insert("rootfs".to_string(), format!("{}:8", payload.rootfs_storage));
                    params.insert("tags".to_string(), "hostable".to_string());
                    params.insert("unprivileged".to_string(), "1".to_string());
                    params.insert("features".to_string(), "nesting=1".to_string());

                    // Map Volumes
                    for (i, vol) in payload.volumes.iter().enumerate() {
                        let parts: Vec<&str> = vol.split(':').collect();
                        if parts.len() == 4 && parts[0] == "storage" {
                            // format: storage:storage_name:size_gb:container_path
                            let storage_name = parts[1];
                            let size_gb = parts[2];
                            let container_path = parts[3];
                            params.insert(format!("mp{}", i), format!("{}:{},mp={}", storage_name, size_gb, container_path));
                        } else if parts.len() == 3 && parts[0] == "bind" {
                            // format: bind:host_path:container_path
                            let host_path = parts[1];
                            let container_path = parts[2];
                            params.insert(format!("mp{}", i), format!("{},mp={}", host_path, container_path));
                        } else if parts.len() == 2 {
                            // format: host_path:container_path (fallback/legacy)
                            let host_path = parts[0];
                            let container_path = parts[1];
                            params.insert(format!("mp{}", i), format!("{},mp={}", host_path, container_path));
                        }
                    }

                    match state.proxmox.create_lxc(&node, payload.vmid, params).await {
                        Ok(_) => {
                            tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;
                            let _ = state.proxmox.start_lxc(&node, payload.vmid).await;
                            
                            (StatusCode::OK, Json(DeployResponse {
                                status: "ok".to_string(),
                                message: format!("Successfully deployed {} as LXC {}", payload.hostname, payload.vmid),
                            }))
                        },
                        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(DeployResponse { status: "error".to_string(), message: format!("Create failed: {}", e) }))
                    }
                },
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(DeployResponse { status: "error".to_string(), message: format!("Upload failed: {}", e) }))
            }
        },
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(DeployResponse { status: "error".to_string(), message: format!("Extraction failed: {}", e) }))
    }
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
            actions.push(format!("echo '{}' >> /etc/local.d/hostable.start", rest.replace("'", "'\\''")));
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
