use rand::distr::{Alphanumeric, SampleString};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

pub fn data_dir() -> PathBuf {
    let path = std::env::var_os("HOSTABLE_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if Path::new("/etc/hostable").is_dir() {
                PathBuf::from("/etc/hostable")
            } else {
                PathBuf::from("./hostable-data")
            }
        });
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .expect("Working directory unavailable")
            .join(path)
    }
}

pub fn id(prefix: &str) -> String {
    format!(
        "{}_{}",
        prefix,
        Alphanumeric.sample_string(&mut rand::rng(), 24)
    )
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn private_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn write_secret(path: &Path, data: &[u8]) -> Result<(), String> {
    private_dir(path.parent().ok_or("Missing parent directory")?)?;
    write_private_file(path, data)
}

/// Private file contents without changing an existing caller-selected directory's permissions.
pub fn write_private_file(path: &Path, data: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(path.parent().ok_or("Missing parent directory")?)
        .map_err(|e| e.to_string())?;
    let tmp = path.with_extension(id("tmp"));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp).map_err(|e| e.to_string())?;
    let result = file.write_all(data).and_then(|_| file.sync_all());
    drop(file);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    let result = std::fs::rename(&tmp, path).map_err(|e| e.to_string());
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

#[derive(rust_embed::RustEmbed)]
#[folder = "../ansible/"]
struct AnsibleAssets;

pub fn playbooks() -> Result<PathBuf, String> {
    let root = data_dir().join("automation");
    private_dir(&root)?;
    for name in AnsibleAssets::iter() {
        let asset = AnsibleAssets::get(&name).ok_or("Missing embedded playbook")?;
        let path = root.join(name.as_ref());
        std::fs::create_dir_all(path.parent().ok_or("Invalid asset path")?)
            .map_err(|e| e.to_string())?;
        if std::fs::read(&path).ok().as_deref() != Some(asset.data.as_ref()) {
            write_secret(&path, asset.data.as_ref())?;
        }
    }
    Ok(root)
}

pub async fn ansible(playbook: &str, variables: &serde_json::Value) -> Result<(), String> {
    let root = playbooks()?;
    let vars = data_dir()
        .join("secrets")
        .join(format!("{}.json", id("vars")));
    write_secret(&vars, variables.to_string().as_bytes())?;
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(900),
        tokio::process::Command::new("ansible-playbook")
            .kill_on_drop(true)
            .current_dir(&root)
            .arg(root.join("playbooks").join(playbook))
            .arg("-i")
            .arg("localhost,")
            .arg("-e")
            .arg(format!(
                "@{}",
                std::fs::canonicalize(&vars)
                    .map_err(|e| e.to_string())?
                    .display()
            ))
            .env("ANSIBLE_ROLES_PATH", root.join("roles"))
            .env("ANSIBLE_HOST_KEY_CHECKING", "True")
            .output(),
    )
    .await;
    let _ = std::fs::remove_file(&vars);
    match result {
        Ok(Ok(output)) if output.status.success() => Ok(()),
        Ok(Ok(output)) => Err(format!(
            "Guest configuration failed (exit {}). Inspect the guest and the selected recipe; secret-bearing output is suppressed.",
            output.status
        )),
        Ok(Err(e)) => Err(format!("Cannot launch Ansible: {}", e)),
        Err(_) => Err("Guest configuration timed out".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protected_files_can_be_replaced_without_losing_contents() {
        let dir = std::env::temp_dir().join(id("hostable_secret_test"));
        let path = dir.join("secret");
        write_secret(&path, b"first").unwrap();
        write_secret(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

pub struct CacheGuard(pub PathBuf);
impl Drop for CacheGuard {
    fn drop(&mut self) {
        // Only remove this operation's generated directory beneath the owned cache root.
        let root = data_dir().join("cache");
        if self.0.is_absolute() && self.0.parent() == Some(root.as_path()) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
