#![allow(dead_code, unused_variables, unused_imports)]
use reqwest::Client;

use std::env;

#[derive(Clone)]
pub struct ProxmoxClient {
    client: Client,
    base_url: String,
    token_id: String,
    token_secret: String,
}

impl ProxmoxClient {
    pub fn new() -> Self {
        let host = env::var("PROXMOX_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let token_id = env::var("PROXMOX_TOKEN_ID").unwrap_or_default();
        let token_secret = env::var("PROXMOX_TOKEN_SECRET").unwrap_or_default();
        let insecure = env::var("PROXMOX_INSECURE_TLS").unwrap_or_else(|_| "true".to_string()) == "true";
        
        let client = Client::builder()
            .danger_accept_invalid_certs(insecure)
            .build()
            .expect("Failed to build Proxmox reqwest client");

        Self {
            client,
            base_url: format!("https://{}:8006/api2/json", host),
            token_id,
            token_secret,
        }
    }

    fn auth_header(&self) -> String {
        format!("PVEAPIToken={}={}", self.token_id, self.token_secret)
    }

    async fn handle_response(url: &str, res: reqwest::Response) -> Result<serde_json::Value, String> {
        let status = res.status();
        let text = res.text().await.map_err(|e| format!("Failed to read response text: {}", e))?;
        
        tracing::debug!("Proxmox API {} - Status: {} - Response: {}", url, status, text);
        
        if !status.is_success() {
            tracing::error!("Proxmox API Request Failed! URL: {} Status: {} Response: {}", url, status, text);
            // It might not be JSON, but let's try to parse it anyway or just return the text inside JSON.
            match serde_json::from_str(&text) {
                Ok(json) => return Ok(json),
                Err(_) => return Err(format!("Proxmox returned {}: {}", status, text)),
            }
        }
        
        serde_json::from_str(&text).map_err(|e| format!("Failed to parse JSON (Status: {}): {} \nRaw Text: {}", status, e, text))
    }

    pub async fn get_cluster_resources(&self) -> Result<serde_json::Value, String> {
        let url = format!("{}/cluster/resources", self.base_url);
        tracing::info!("GET {}", url);
        
        let res = self.client.get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
            
        Self::handle_response(&url, res).await
    }

    pub async fn get_nodes(&self) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes", self.base_url);
        tracing::info!("GET {}", url);
        
        let res = self.client.get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
            
        Self::handle_response(&url, res).await
    }
    
    pub async fn get_node_status(&self, node: &str) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/status", self.base_url, node);
        tracing::info!("GET {}", url);
        
        let res = self.client.get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
            
        Self::handle_response(&url, res).await
    }
    
    pub async fn get_lxcs(&self, node: &str) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/lxc", self.base_url, node);
        tracing::info!("GET {}", url);
        
        let res = self.client.get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
            
        Self::handle_response(&url, res).await
    }

    pub async fn upload_template(&self, node: &str, storage: &str, file_path: &std::path::Path, filename: &str) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/storage/{}/upload", self.base_url, node, storage);
        tracing::info!("POST {}", url);
        
        let file_bytes = tokio::fs::read(file_path).await.map_err(|e| e.to_string())?;
        
        let part = reqwest::multipart::Part::bytes(file_bytes)
            .file_name(filename.to_string())
            .mime_str("application/x-xz").unwrap();
            
        let form = reqwest::multipart::Form::new()
            .text("content", "vztmpl")
            .text("filename", filename.to_string())
            .part("filename", part);
            
        let res = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .multipart(form)
            .send()
            .await
            .map_err(|e| e.to_string())?;
            
        Self::handle_response(&url, res).await
    }

    pub async fn create_lxc(&self, node: &str, vmid: u32, params: std::collections::HashMap<String, String>) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/lxc", self.base_url, node);
        tracing::info!("POST {} with params: {:?}", url, params);
        
        let res = self.client.post(&url)
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
        let res = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, res).await
    }

    pub async fn start_lxc(&self, node: &str, vmid: u32) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/lxc/{}/status/start", self.base_url, node, vmid);
        tracing::info!("POST {}", url);
        let res = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, res).await
    }

    pub async fn delete_lxc(&self, node: &str, vmid: u32) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/lxc/{}", self.base_url, node, vmid);
        tracing::info!("DELETE {}", url);
        let res = self.client.delete(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, res).await
    }

    pub async fn get_rrddata(&self, node: &str, vmid: Option<u32>, timeframe: &str) -> Result<serde_json::Value, String> {
        let url = if let Some(id) = vmid {
            format!("{}/nodes/{}/lxc/{}/rrddata?timeframe={}", self.base_url, node, id, timeframe)
        } else {
            format!("{}/nodes/{}/rrddata?timeframe={}", self.base_url, node, timeframe)
        };
        tracing::info!("GET {}", url);
        let res = self.client.get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| e.to_string())?;
        Self::handle_response(&url, res).await
    }
}
