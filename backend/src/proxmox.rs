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

impl ProxmoxClient {
    pub fn new() -> Self {
        let host = env::var("PROXMOX_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let token_id = env::var("PROXMOX_TOKEN_ID").unwrap_or_default();
        let token_secret = env::var("PROXMOX_TOKEN_SECRET").unwrap_or_default();
        let insecure =
            env::var("PROXMOX_INSECURE_TLS").unwrap_or_else(|_| "true".to_string()) == "true";

        let client = Client::builder()
            .danger_accept_invalid_certs(insecure)
            .build()
            .expect("Failed to build Proxmox reqwest client");

        Self {
            client,
            base_url: format!("https://{}:8006/api2/json", host),
            token_id,
            token_secret,
            insecure,
        }
    }

    pub fn is_insecure(&self) -> bool {
        self.insecure
    }

    fn auth_header(&self) -> String {
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
            // It might not be JSON, but let's try to parse it anyway or just return the text inside JSON.
            match serde_json::from_str(&text) {
                Ok(json) => return Ok(json),
                Err(_) => return Err(format!("Proxmox returned {}: {}", status, text)),
            }
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

        let file_bytes = tokio::fs::read(file_path)
            .await
            .map_err(|e| e.to_string())?;

        let part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name(filename.to_string())
            .mime_str("application/x-xz")
            .unwrap();

        let form = reqwest::multipart::Form::new()
            .text("content", "vztmpl")
            .text("filename", filename.to_string())
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
        tracing::info!("POST {} with params: {:?}", url, params);

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

        for _ in 0..120 {
            let res = self
                .client
                .get(&url)
                .header("Authorization", self.auth_header())
                .send()
                .await
                .map_err(|e| e.to_string())?;

            if let Ok(json) = Self::handle_response(&url, res).await {
                if let Some(status) = json["data"]["status"].as_str() {
                    if status == "stopped" {
                        let exit_status = json["data"]["exitstatus"].as_str().unwrap_or("OK");
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
                    let mut fallback_ip: Option<String> = None;

                    for item in data {
                        let name = item["name"].as_str().unwrap_or("");
                        if name == "lo" {
                            continue;
                        }

                        if let Some(inet) = item["inet"].as_str() {
                            let ip = inet.split('/').next().unwrap_or(inet).trim();
                            if !ip.is_empty() && !ip.starts_with("127.") {
                                if name == target_iface {
                                    if let Some(subnet) = preferred_subnet {
                                        if ip.starts_with(subnet) {
                                            return Ok(ip.to_string());
                                        }
                                    } else {
                                        return Ok(ip.to_string());
                                    }
                                }

                                if let Some(subnet) = preferred_subnet {
                                    if ip.starts_with(subnet) {
                                        return Ok(ip.to_string());
                                    }
                                }

                                if fallback_ip.is_none() {
                                    fallback_ip = Some(ip.to_string());
                                }
                            }
                        }
                    }

                    if let Some(ip) = fallback_ip {
                        return Ok(ip);
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
