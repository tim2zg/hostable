use serde::Deserialize;
use std::{collections::HashMap, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Deserialize)]
struct Endpoint {
    host: String,
    #[serde(default = "root")]
    user: String,
    #[serde(default = "ssh_port")]
    port: u16,
}
fn root() -> String {
    "root".into()
}
fn ssh_port() -> u16 {
    22
}
fn endpoint(node: &str) -> Result<Endpoint, String> {
    let endpoints: HashMap<String, Endpoint> = serde_json::from_str(
        &std::env::var("HOSTABLE_NODE_SSH_ENDPOINTS")
            .map_err(|_| "Configure node SSH endpoints for in-container commands")?,
    )
    .map_err(|_| "Invalid node SSH endpoint configuration")?;
    let e = endpoints
        .into_iter()
        .find(|(n, _)| n == node)
        .map(|(_, e)| e)
        .ok_or("No SSH endpoint for this node")?;
    if e.port == 0
        || !crate::ansible::identifier(&e.user)
        || e.host.starts_with('-')
        || e.host.len() > 253
        || !e
            .host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
    {
        return Err("Invalid SSH endpoint".into());
    }
    Ok(e)
}
pub fn available(node: &str) -> bool {
    endpoint(node).is_ok()
        && std::env::var_os("HOSTABLE_NODE_SSH_KEY").is_some()
        && std::env::var_os("HOSTABLE_NODE_KNOWN_HOSTS").is_some()
}

async fn bounded_read<R: tokio::io::AsyncRead + Unpin>(mut r: R) -> Result<String, String> {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = r.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        if data.len() > 65536 {
            data.drain(..data.len() - 65536);
        }
    }
    Ok(String::from_utf8_lossy(&data).into_owned())
}
pub async fn exec(
    state: &Arc<crate::AppState>,
    node: &str,
    vmid: u32,
    provenance: &str,
    script: &str,
    timeout: u32,
) -> Result<String, String> {
    crate::workloads::owned(state, node, vmid, provenance).await?;
    if script.len() > 65536 || script.contains('\0') || !(10..=3600).contains(&timeout) {
        return Err("Invalid guest command".into());
    }
    if !crate::ansible::identifier(provenance) {
        return Err("Invalid container ownership marker".into());
    }
    let e = endpoint(node)?;
    let key = std::fs::canonicalize(
        std::env::var_os("HOSTABLE_NODE_SSH_KEY").ok_or("Node SSH key is required")?,
    )
    .map_err(|_| "Node SSH key unavailable")?;
    let hosts = std::fs::canonicalize(
        std::env::var_os("HOSTABLE_NODE_KNOWN_HOSTS")
            .ok_or("Pre-pinned node known_hosts is required")?,
    )
    .map_err(|_| "Node known_hosts unavailable")?;
    let mut cmd = tokio::process::Command::new("ssh");
    cmd.kill_on_drop(true)
        .args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "ConnectTimeout=10",
            "-o",
        ])
        .arg(format!("UserKnownHostsFile={}", hosts.display()))
        .arg("-i")
        .arg(key)
        .arg("-p")
        .arg(e.port.to_string())
        .arg(format!("{}@{}", e.user, e.host));
    // Only numeric identifiers enter the host command. Administrator scripts travel via stdin into the guest.
    cmd.arg(format!(
        "pct config {} | grep -Fx 'description: hostable.task={}' >/dev/null && timeout --signal=TERM --kill-after=30 {} pct exec {} -- /bin/sh -s",
        vmid,provenance,timeout, vmid
    ));
    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Cannot launch node SSH: {}", e))?;
    let mut input = child.stdin.take().ok_or("Missing guest stdin")?;
    let out = tokio::spawn(bounded_read(
        child.stdout.take().ok_or("Missing guest stdout")?,
    ));
    let err = tokio::spawn(bounded_read(
        child.stderr.take().ok_or("Missing guest stderr")?,
    ));
    let operation = async {
        input
            .write_all(script.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        input.shutdown().await.map_err(|e| e.to_string())?;
        drop(input);
        let status = child.wait().await.map_err(|e| e.to_string())?;
        let stdout = out.await.map_err(|e| e.to_string())??;
        let stderr = err.await.map_err(|e| e.to_string())??;
        if status.success() {
            Ok(stdout)
        } else {
            let _ = stderr;
            Err(format!(
                "Guest command failed ({}). Output suppressed to protect application secrets.",
                status
            ))
        }
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(u64::from(timeout) + 45),
        operation,
    )
    .await
    .map_err(|_| "Guest command timeout; inspect the guest before retrying".to_string())?
}
