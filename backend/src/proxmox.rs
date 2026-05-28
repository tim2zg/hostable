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

    pub async fn get_cluster_resources(&self) -> Result<serde_json::Value, reqwest::Error> {
        let url = format!("{}/cluster/resources", self.base_url);
        
        let res = self.client.get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
            
        res.json().await
    }

    pub async fn get_nodes(&self) -> Result<serde_json::Value, reqwest::Error> {
        let url = format!("{}/nodes", self.base_url);
        
        let res = self.client.get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
            
        res.json().await
    }
    
    pub async fn get_node_status(&self, node: &str) -> Result<serde_json::Value, reqwest::Error> {
        let url = format!("{}/nodes/{}/status", self.base_url, node);
        
        let res = self.client.get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
            
        res.json().await
    }
    
    pub async fn get_lxcs(&self, node: &str) -> Result<serde_json::Value, reqwest::Error> {
        let url = format!("{}/nodes/{}/lxc", self.base_url, node);
        
        let res = self.client.get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
            
        res.json().await
    }

    pub async fn upload_template(&self, node: &str, storage: &str, file_path: &std::path::Path, filename: &str) -> Result<serde_json::Value, String> {
        let url = format!("{}/nodes/{}/storage/{}/upload", self.base_url, node, storage);
        
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
            
        res.json().await.map_err(|e| e.to_string())
    }

    pub async fn create_lxc(&self, node: &str, vmid: u32, params: std::collections::HashMap<String, String>) -> Result<serde_json::Value, reqwest::Error> {
        let url = format!("{}/nodes/{}/lxc", self.base_url, node);
        
        let res = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .json(&params)
            .send()
            .await?;
            
        res.json().await
    }

    pub async fn stop_lxc(&self, node: &str, vmid: u32) -> Result<serde_json::Value, reqwest::Error> {
        let url = format!("{}/nodes/{}/lxc/{}/status/stop", self.base_url, node, vmid);
        let res = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        res.json().await
    }

    pub async fn start_lxc(&self, node: &str, vmid: u32) -> Result<serde_json::Value, reqwest::Error> {
        let url = format!("{}/nodes/{}/lxc/{}/status/start", self.base_url, node, vmid);
        let res = self.client.post(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        res.json().await
    }

    pub async fn delete_lxc(&self, node: &str, vmid: u32) -> Result<serde_json::Value, reqwest::Error> {
        let url = format!("{}/nodes/{}/lxc/{}", self.base_url, node, vmid);
        let res = self.client.delete(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        res.json().await
    }
}
