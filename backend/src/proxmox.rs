#![allow(dead_code, unused_variables, unused_imports)]
use reqwest::Client;

use std::env;

#[derive(Clone)]
pub struct ProxmoxClient {
    client: Client,
    base_url: String,
    token_id: String,
    token_secret: String,
    insecure: bool,
}

/// Value destined for a Proxmox comma-separated option string (net0, mpN, ...):
/// must not smuggle in extra options or line breaks.
pub fn valid_opt_value(s: &str) -> bool {
    !s.is_empty() && s.len() <= 512 && !s.contains([',', '=', '\n', '\r'])
}

/// Hostnames for LXC containers: plain DNS label-ish characters only.
pub fn valid_hostname(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && !s.starts_with('-')
        && !s.ends_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

/// Absolute container mount paths without option-string metacharacters.
pub fn valid_mount_path(s: &str) -> bool {
    s.starts_with('/') && s.len() <= 256 && !s.contains([',', '=', '\n', '\r', '\0'])
}

/// IP configuration values: dhcp/manual keywords or address[/prefix] text.
pub fn valid_ip_value(s: &str) -> bool {
    s == "dhcp"
        || s == "manual"
        || (!s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_hexdigit() || matches!(c, '.' | ':' | '/')))
}

/// Disk sizes as accepted by Proxmox: digits plus an optional size suffix.
pub fn valid_disk_size(s: &str) -> bool {
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    digits > 0
        && matches!(
            s[digits..].to_ascii_uppercase().as_str(),
            "" | "K" | "M" | "G" | "T" | "KB" | "MB" | "GB" | "TB"
        )
}

/// Network interface names: plain identifier characters only.
pub fn valid_iface_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
}

#[cfg(test)]
mod opt_tests {
    use super::*;

    #[test]
    fn test_option_value_rejects_injection() {
        assert!(valid_opt_value("local-lvm"));
        assert!(valid_opt_value("/mnt/data"));
        assert!(!valid_opt_value(""));
        assert!(!valid_opt_value("vmbr0,tag=10"));
        assert!(!valid_opt_value("x=y"));
        assert!(!valid_opt_value("a\nb"));
        assert!(!valid_opt_value(&"x".repeat(513)));
    }

    #[test]
    fn test_hostname_validation() {
        assert!(valid_hostname("web-1"));
        assert!(valid_hostname("my.host.local"));
        assert!(!valid_hostname(""));
        assert!(!valid_hostname("-bad"));
        assert!(!valid_hostname("bad-"));
        assert!(!valid_hostname("a b"));
        assert!(!valid_hostname("a,b"));
    }

    #[test]
    fn test_mount_path_validation() {
        assert!(valid_mount_path("/var/lib/data"));
        assert!(!valid_mount_path("relative/path"));
        assert!(!valid_mount_path("/x,mp=/y"));
        assert!(!valid_mount_path("/x=y"));
        assert!(!valid_mount_path("/x\ny"));
    }

    #[test]
    fn test_ip_value_validation() {
        assert!(valid_ip_value("dhcp"));
        assert!(valid_ip_value("manual"));
        assert!(valid_ip_value("10.0.0.5/24"));
        assert!(valid_ip_value("fe80::1"));
        assert!(!valid_ip_value(""));
        assert!(!valid_ip_value("1.2.3.4,gw=5.6.7.8"));
    }

    #[test]
    fn test_disk_size_validation() {
        assert!(valid_disk_size("8"));
        assert!(valid_disk_size("512M"));
        assert!(valid_disk_size("8GB"));
        assert!(!valid_disk_size(""));
        assert!(!valid_disk_size("G"));
        assert!(!valid_disk_size("8G;onboot=1"));
        assert!(!valid_disk_size("8,mp=/x"));
    }

    #[test]
    fn test_iface_name_validation() {
        assert!(valid_iface_name("eth0"));
        assert!(valid_iface_name("enp3s0.100"));
        assert!(!valid_iface_name(""));
        assert!(!valid_iface_name("eth0 x"));
        assert!(!valid_iface_name("eth0,onboot=1"));
    }
}

impl ProxmoxClient {
    pub async fn task_status(&self, node: &str, upid: &str) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/tasks/{}/status", self.base_url, node, upid);
        let response = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, response).await
    }
    pub async fn move_mount(
        &self,
        node: &str,
        source: u32,
        target: u32,
        key: &str,
    ) -> Result<serde_json::Value, String> {
        if !key
            .strip_prefix("mp")
            .is_some_and(|n| n.parse::<u8>().is_ok())
            || source == target
        {
            return Err("Invalid volume transfer".into());
        }
        let from = self.get_lxc_config(node, source).await?;
        let to = self.get_lxc_config(node, target).await?;
        let mut fields = std::collections::HashMap::from([
            ("volume", key.to_string()),
            ("target-vmid", target.to_string()),
            ("target-volume", key.to_string()),
        ]);
        if let Some(d) = from["data"]["digest"].as_str() {
            fields.insert("digest", d.into());
        }
        if let Some(d) = to["data"]["digest"].as_str() {
            fields.insert("target-digest", d.into());
        }
        let url = format!(
            "{}/nodes/{}/lxc/{}/move_volume",
            self.base_url, node, source
        );
        let response = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .form(&fields)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, response).await
    }
    pub async fn backup_lxc(
        &self,
        node: &str,
        vmid: u32,
        storage: &str,
    ) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/vzdump", self.base_url, node);
        let response = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .form(&[
                ("vmid", vmid.to_string()),
                ("storage", storage.into()),
                ("mode", "stop".into()),
                ("compress", "zstd".into()),
                ("remove", "0".into()),
            ])
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, response).await
    }
    pub async fn get_templates(
        &self,
        node: &str,
        storage: &str,
    ) -> Result<serde_json::Value, String> {
        let url = format!(
            "{}/nodes/{}/storage/{}/content",
            self.base_url, node, storage
        );
        let response = self
            .client
            .get(&url)
            .query(&[("content", "vztmpl")])
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, response).await
    }
    pub async fn resolve_lxc_node(&self, vmid: u32) -> Result<String, String> {
        let resources = self.get_cluster_resources().await?;
        let item = resources["data"]
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|v| v["vmid"].as_u64() == Some(vmid as u64))
            })
            .ok_or_else(|| format!("Container {} not found", vmid))?;
        if item["type"] != "lxc" {
            return Err("This operation supports LXC containers only".into());
        }
        item["node"]
            .as_str()
            .filter(|n| valid_opt_value(n))
            .map(str::to_owned)
            .ok_or_else(|| "Invalid resource node".into())
    }

    pub async fn get_lxc_status(&self, node: &str, vmid: u32) -> Result<serde_json::Value, String> {
        let url = format!(
            "{}/nodes/{}/lxc/{}/status/current",
            self.base_url, node, vmid
        );
        let response = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, response).await
    }

    pub async fn get_lxc_config(&self, node: &str, vmid: u32) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/lxc/{}/config", self.base_url, node, vmid);
        let response = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, response).await
    }

    pub async fn update_lxc_config(
        &self,
        node: &str,
        vmid: u32,
        params: &std::collections::HashMap<String, String>,
    ) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/lxc/{}/config", self.base_url, node, vmid);
        let response = self
            .client
            .put(&url)
            .header("Authorization", self.auth_header())
            .form(params)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, response).await
    }

    pub async fn wait_response_task(
        &self,
        node: &str,
        result: serde_json::Value,
    ) -> Result<(), String> {
        let upid = result["data"]
            .as_str()
            .filter(|s| s.starts_with("UPID:"))
            .ok_or_else(|| "Proxmox did not return a task ID".to_string())?;
        self.wait_for_task(node, upid).await
    }

    pub async fn wait_lxc_status(
        &self,
        node: &str,
        vmid: u32,
        expected: &str,
    ) -> Result<(), String> {
        for _ in 0..30 {
            if self.get_lxc_status(node, vmid).await?["data"]["status"].as_str() == Some(expected) {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        Err(format!("Container {} did not reach {}", vmid, expected))
    }

    pub async fn validate_storage(
        &self,
        node: &str,
        name: &str,
        content: &str,
    ) -> Result<(), String> {
        let storages = self.get_storages(node).await?;
        let storage = storages["data"]
            .as_array()
            .and_then(|s| s.iter().find(|s| s["storage"] == name))
            .ok_or_else(|| format!("Storage {} not found on {}", name, node))?;
        if storage["content"]
            .as_str()
            .unwrap_or("")
            .split(',')
            .any(|c| c == content)
            && storage["active"].as_i64().unwrap_or(1) == 1
        {
            Ok(())
        } else {
            Err(format!("Storage {} is not available for {}", name, content))
        }
    }

    pub async fn delete_template(
        &self,
        node: &str,
        storage: &str,
        filename: &str,
    ) -> Result<(), String> {
        let mut url = reqwest::Url::parse(&format!(
            "{}/nodes/{}/storage/{}/content/",
            self.base_url, node, storage
        ))
        .map_err(|e| e.to_string())?;
        url.path_segments_mut()
            .map_err(|_| "Invalid template URL")?
            .pop_if_empty()
            .push(&format!("{}:vztmpl/{}", storage, filename));
        let response = self
            .client
            .delete(url.clone())
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let value = Self::handle_response(url.as_str(), response).await?;
        if value["data"]
            .as_str()
            .is_some_and(|s| s.starts_with("UPID:"))
        {
            self.wait_response_task(node, value).await?;
        }
        if self.get_templates(node, storage).await?["data"]
            .as_array()
            .is_some_and(|rows| {
                rows.iter()
                    .any(|v| v["volid"] == format!("{}:vztmpl/{}", storage, filename))
            })
        {
            return Err("Generated template still exists after deletion".into());
        }
        Ok(())
    }
    pub fn new() -> Self {
        let host = env::var("PROXMOX_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let token_id = env::var("PROXMOX_TOKEN_ID").unwrap_or_default();
        let token_secret = env::var("PROXMOX_TOKEN_SECRET").unwrap_or_default();
        // TLS verification is ON by default. Self-signed Proxmox certificates
        // require an explicit PROXMOX_INSECURE_TLS=true opt-in.
        let insecure = env::var("PROXMOX_INSECURE_TLS")
            .map(|v| v == "true")
            .unwrap_or(false);

        let mut builder = Client::builder()
            .danger_accept_invalid_certs(insecure)
            .timeout(std::time::Duration::from_secs(300));
        if let Ok(path) = env::var("PROXMOX_CA_CERT") {
            let pem = std::fs::read(path).expect("Cannot read PROXMOX_CA_CERT");
            builder = builder.add_root_certificate(
                reqwest::Certificate::from_pem(&pem).expect("Invalid Proxmox CA certificate"),
            );
        }
        let client = builder
            .build()
            .expect("Failed to build Proxmox reqwest client");

        Self {
            client,
            base_url: {
                let value = env::var("PROXMOX_API_URL")
                    .unwrap_or_else(|_| format!("https://{}:8006/api2/json", host));
                let url = reqwest::Url::parse(&value).expect("Invalid PROXMOX_API_URL");
                let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
                assert!(
                    (url.scheme() == "https" || (url.scheme() == "http" && local))
                        && url.username().is_empty()
                        && url.password().is_none(),
                    "Proxmox API requires HTTPS (HTTP is allowed only for loopback tests)"
                );
                value.trim_end_matches('/').to_string()
            },
            token_id,
            token_secret,
            insecure,
        }
    }

    pub fn console_url(
        &self,
        node: &str,
        vmid: u32,
        port: u16,
        ticket: &str,
    ) -> Result<String, String> {
        let mut url = reqwest::Url::parse(&format!(
            "{}/nodes/{}/lxc/{}/vncwebsocket",
            self.base_url, node, vmid
        ))
        .map_err(|e| e.to_string())?;
        url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" })
            .map_err(|_| "Invalid console scheme")?;
        url.query_pairs_mut()
            .append_pair("port", &port.to_string())
            .append_pair("vncticket", ticket);
        Ok(url.to_string())
    }
    pub fn is_insecure(&self) -> bool {
        self.insecure
    }

    pub fn auth_user(&self) -> String {
        // API token IDs are "user@realm!tokenname"; termproxy authentication
        // must use the owning user instead of a hardcoded root@pam.
        let user = self.token_id.split('!').next().unwrap_or("").trim();
        if user.is_empty() {
            "root@pam".to_string()
        } else {
            user.to_string()
        }
    }

    pub fn auth_header(&self) -> String {
        format!("PVEAPIToken={}={}", self.token_id, self.token_secret)
    }

    async fn handle_response(
        url: &str,
        res: reqwest::Response,
    ) -> Result<serde_json::Value, String> {
        let status = res.status();
        let text = res
            .text()
            .await
            .map_err(|e| format!("Failed to read response text: {}", e))?;

        tracing::debug!(
            "Proxmox API {} - Status: {} - Response: {}",
            url,
            status,
            text
        );

        if !status.is_success() {
            tracing::error!(
                "Proxmox API Request Failed! URL: {} Status: {} Response: {}",
                url,
                status,
                text
            );
            return Err(format!("Proxmox returned {}: {}", status, text));
        }

        serde_json::from_str(&text).map_err(|e| {
            format!(
                "Failed to parse JSON (Status: {}): {} \nRaw Text: {}",
                status, e, text
            )
        })
    }

    pub async fn get_cluster_resources(&self) -> Result<serde_json::Value, String> {
        let url = format!("{}/cluster/resources", self.base_url);
        tracing::info!("GET {}", url);

        let res = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;

        Self::handle_response(&url, res).await
    }

    pub async fn get_default_node(&self) -> String {
        if let Ok(val) = env::var("PROXMOX_NODE") {
            return val;
        }
        if let Ok(nodes) = self.get_nodes().await {
            if let Some(data) = nodes["data"].as_array() {
                if let Some(first) = data.first() {
                    if let Some(node_name) = first["node"].as_str() {
                        return node_name.to_string();
                    }
                }
            }
        }
        "pve".to_string()
    }

    pub async fn get_nodes(&self) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes", self.base_url);
        tracing::info!("GET {}", url);

        let res = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;

        Self::handle_response(&url, res).await
    }

    pub async fn get_node_status(&self, node: &str) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/status", self.base_url, node);
        tracing::info!("GET {}", url);

        let res = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;

        Self::handle_response(&url, res).await
    }

    pub async fn get_lxcs(&self, node: &str) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/lxc", self.base_url, node);
        tracing::info!("GET {}", url);

        let res = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;

        Self::handle_response(&url, res).await
    }

    pub async fn upload_template(
        &self,
        node: &str,
        storage: &str,
        file_path: &std::path::Path,
        filename: &str,
    ) -> Result<serde_json::Value, String> {
        let url = format!(
            "{}/nodes/{}/storage/{}/upload",
            self.base_url, node, storage
        );
        tracing::info!("POST {}", url);

        let file = tokio::fs::File::open(file_path)
            .await
            .map_err(|e| e.to_string())?;
        let len = file.metadata().await.map_err(|e| e.to_string())?.len();
        let stream = futures::stream::try_unfold(file, |mut file| async move {
            use tokio::io::AsyncReadExt;
            let mut bytes = vec![0u8; 1024 * 1024];
            let n = file.read(&mut bytes).await?;
            bytes.truncate(n);
            Ok::<_, std::io::Error>(if n == 0 { None } else { Some((bytes, file)) })
        });
        let part =
            reqwest::multipart::Part::stream_with_length(reqwest::Body::wrap_stream(stream), len)
                .file_name(filename.to_string())
                .mime_str("application/x-xz")
                .unwrap();

        let form = reqwest::multipart::Form::new()
            .text("content", "vztmpl")
            .part("filename", part);

        let res = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .multipart(form)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        Self::handle_response(&url, res).await
    }

    pub async fn create_termproxy(
        &self,
        node: &str,
        vmid: u32,
    ) -> Result<(String, String), String> {
        let url = format!("{}/nodes/{}/lxc/{}/termproxy", self.base_url, node, vmid);
        tracing::info!("POST {}", url);

        let res = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json = Self::handle_response(&url, res).await?;
        if let Some(data) = json.get("data") {
            let ticket = data
                .get("ticket")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            let port = if let Some(p) = data.get("port") {
                if let Some(p_str) = p.as_str() {
                    p_str.to_string()
                } else if let Some(p_i64) = p.as_i64() {
                    p_i64.to_string()
                } else {
                    "".to_string()
                }
            } else {
                "".to_string()
            };

            if !ticket.is_empty() && !port.is_empty() {
                return Ok((ticket, port));
            }
        }
        Err("Failed to parse termproxy ticket/port".to_string())
    }

    pub fn get_host(&self) -> String {
        // Extract host from base_url
        if let Some(host_port) = self.base_url.split("https://").nth(1) {
            if let Some(host) = host_port.split(":").next() {
                return host.to_string();
            }
        }
        "127.0.0.1".to_string()
    }

    pub async fn create_lxc(
        &self,
        node: &str,
        vmid: u32,
        params: std::collections::HashMap<String, String>,
    ) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/lxc", self.base_url, node);
        tracing::info!("Creating LXC {} on {}", vmid, node);

        let res = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(&params)
            .send()
            .await
            .map_err(|e| e.to_string())?;

        Self::handle_response(&url, res).await
    }

    pub async fn stop_lxc(&self, node: &str, vmid: u32) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/lxc/{}/status/stop", self.base_url, node, vmid);
        tracing::info!("POST {}", url);
        let res = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, res).await
    }

    pub async fn start_lxc(&self, node: &str, vmid: u32) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/lxc/{}/status/start", self.base_url, node, vmid);
        tracing::info!("POST {}", url);
        let res = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, res).await
    }

    pub async fn delete_lxc(&self, node: &str, vmid: u32) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/lxc/{}", self.base_url, node, vmid);
        tracing::info!("DELETE {}", url);
        let res = self
            .client
            .delete(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, res).await
    }

    pub async fn get_rrddata(
        &self,
        node: &str,
        vmid: Option<u32>,
        timeframe: &str,
    ) -> Result<serde_json::Value, String> {
        let url = if let Some(id) = vmid {
            format!(
                "{}/nodes/{}/lxc/{}/rrddata?timeframe={}",
                self.base_url, node, id, timeframe
            )
        } else {
            format!(
                "{}/nodes/{}/rrddata?timeframe={}",
                self.base_url, node, timeframe
            )
        };
        tracing::info!("GET {}", url);
        let res = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, res).await
    }

    pub async fn get_storages(&self, node: &str) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/storage", self.base_url, node);
        tracing::info!("GET {}", url);

        let res = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;

        Self::handle_response(&url, res).await
    }
    pub async fn get_snapshots(&self, node: &str, vmid: u32) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/lxc/{}/snapshot", self.base_url, node, vmid);
        tracing::info!("GET {}", url);
        let res = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, res).await
    }

    pub async fn create_snapshot(
        &self,
        node: &str,
        vmid: u32,
        snapname: &str,
        description: &str,
    ) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/lxc/{}/snapshot", self.base_url, node, vmid);
        tracing::info!("POST {}", url);

        let mut params = std::collections::HashMap::new();
        params.insert("snapname", snapname);
        if !description.is_empty() {
            params.insert("description", description);
        }

        let res = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(&params)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, res).await
    }

    pub async fn rollback_snapshot(
        &self,
        node: &str,
        vmid: u32,
        snapname: &str,
    ) -> Result<serde_json::Value, String> {
        let url = format!(
            "{}/nodes/{}/lxc/{}/snapshot/{}/rollback",
            self.base_url, node, vmid, snapname
        );
        tracing::info!("POST {}", url);
        let res = self
            .client
            .post(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, res).await
    }

    pub async fn delete_snapshot(
        &self,
        node: &str,
        vmid: u32,
        snapname: &str,
    ) -> Result<serde_json::Value, String> {
        let url = format!(
            "{}/nodes/{}/lxc/{}/snapshot/{}",
            self.base_url, node, vmid, snapname
        );
        tracing::info!("DELETE {}", url);
        let res = self
            .client
            .delete(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, res).await
    }

    pub async fn wait_for_task(&self, node: &str, upid: &str) -> Result<(), String> {
        let url = format!("{}/nodes/{}/tasks/{}/status", self.base_url, node, upid);
        tracing::info!("Polling task status: {}", upid);

        for _ in 0..900 {
            let res = self
                .client
                .get(&url)
                .header("Authorization", self.auth_header())
                .send()
                .await
                .map_err(|e| e.to_string())?;

            {
                let json = Self::handle_response(&url, res).await?;
                if let Some(status) = json["data"]["status"].as_str() {
                    if status == "stopped" {
                        let exit_status = json["data"]["exitstatus"]
                            .as_str()
                            .unwrap_or("MISSING_EXIT_STATUS");
                        if exit_status == "OK" {
                            return Ok(());
                        } else {
                            return Err(format!("Task failed with exitstatus: {}", exit_status));
                        }
                    }
                }
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
        }
        Err(format!("Timed out waiting for task {}", upid))
    }

    pub async fn get_network_bridges(&self, node: &str) -> Result<Vec<String>, String> {
        let url = format!("{}/nodes/{}/network", self.base_url, node);
        tracing::info!("GET {}", url);
        let res = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let json = Self::handle_response(&url, res).await?;
        let mut bridges = Vec::new();
        if let Some(arr) = json["data"].as_array() {
            for item in arr {
                let iface_type = item["type"].as_str().unwrap_or("");
                if iface_type == "bridge" {
                    if let Some(iface) = item["iface"].as_str() {
                        bridges.push(iface.to_string());
                    }
                }
            }
        }
        if bridges.is_empty() {
            bridges.push("vmbr0".to_string());
        }
        Ok(bridges)
    }

    pub async fn get_next_vmid(&self) -> Result<u32, String> {
        let resources = self.get_cluster_resources().await?;
        let mut max_id: u32 = 99;
        if let Some(arr) = resources["data"].as_array() {
            for item in arr {
                if let Some(vmid) = item["vmid"].as_u64() {
                    let id = vmid as u32;
                    if id > max_id {
                        max_id = id;
                    }
                }
            }
        }
        Ok(max_id + 1)
    }

    pub async fn get_lxc_interfaces(
        &self,
        node: &str,
        vmid: u32,
    ) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/lxc/{}/interfaces", self.base_url, node, vmid);
        tracing::info!("GET {}", url);
        let res = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, res).await
    }

    pub async fn poll_lxc_ip(
        &self,
        node: &str,
        vmid: u32,
        preferred_iface: Option<&str>,
        preferred_subnet: Option<&str>,
        timeout_secs: u64,
    ) -> Result<String, String> {
        let target_iface = preferred_iface.unwrap_or("eth0");
        let start = std::time::Instant::now();
        let max_duration = std::time::Duration::from_secs(timeout_secs);

        while start.elapsed() < max_duration {
            if let Ok(res) = self.get_lxc_interfaces(node, vmid).await {
                if let Some(data) = res["data"].as_array() {
                    for item in data {
                        if item["name"].as_str() != Some(target_iface) {
                            continue;
                        }
                        if let Some(inet) = item["inet"].as_str() {
                            let text = inet.split('/').next().unwrap_or(inet).trim();
                            let Ok(ip) = text.parse::<std::net::Ipv4Addr>() else {
                                continue;
                            };
                            if ip.is_loopback()
                                || ip.is_unspecified()
                                || ip.is_multicast()
                                || ip.is_link_local()
                            {
                                continue;
                            }
                            if preferred_subnet.is_some_and(|subnet| !ipv4_in_subnet(ip, subnet)) {
                                continue;
                            }
                            return Ok(ip.to_string());
                        }
                    }
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
        }

        Err(format!(
            "Timed out after {}s waiting for IP lease on interface '{}' for LXC {}",
            timeout_secs, target_iface, vmid
        ))
    }
}

fn ipv4_in_subnet(ip: std::net::Ipv4Addr, subnet: &str) -> bool {
    let Some((network, prefix)) = subnet.split_once('/') else {
        return false;
    };
    let (Ok(network), Ok(prefix)) = (network.parse::<std::net::Ipv4Addr>(), prefix.parse::<u8>())
    else {
        return false;
    };
    if prefix > 32 {
        return false;
    }
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    (u32::from(ip) & mask) == (u32::from(network) & mask)
}
#[cfg(test)]
mod network_tests {
    use super::*;
    #[test]
    fn subnet_selection_respects_cidr_boundaries() {
        assert!(ipv4_in_subnet("10.0.1.255".parse().unwrap(), "10.0.1.0/24"));
        assert!(!ipv4_in_subnet("10.0.10.1".parse().unwrap(), "10.0.1.0/24"));
        assert!(ipv4_in_subnet("192.168.1.1".parse().unwrap(), "0.0.0.0/0"));
        assert!(!ipv4_in_subnet("10.0.1.1".parse().unwrap(), "10.0.1.0/99"));
    }
}
