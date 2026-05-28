use reqwest::Client;
use serde::{Deserialize, Serialize};
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
        
        let client = Client::builder()
            .danger_accept_invalid_certs(true) // Proxmox usually uses self-signed certificates
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

    pub async fn create_lxc(&self, vmid: u32, hostname: &str) -> Result<serde_json::Value, reqwest::Error> {
        // Assume default node 'pve' and a generic rootfs for scaffolding purposes.
        let node = "pve"; 
        let url = format!("{}/nodes/{}/lxc", self.base_url, node);
        
        let mut params = std::collections::HashMap::new();
        params.insert("vmid", vmid.to_string());
        params.insert("ostemplate", "local:vztmpl/custom-rootfs.tar.gz".to_string());
        params.insert("hostname", hostname.to_string());
        params.insert("memory", "512".to_string());
        params.insert("net0", "name=eth0,bridge=vmbr0,ip=dhcp".to_string());
        params.insert("storage", "local-lvm".to_string());
        params.insert("rootfs", "local-lvm:8".to_string());

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
