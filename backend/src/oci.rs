#![allow(dead_code, unused_variables, unused_imports)]
use flate2::read::GzDecoder;
use futures::StreamExt;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Deserialize;
use std::fs;
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};
use tar::Archive;
use tokio::io::AsyncWriteExt;

// ==============================================================================
// Data Models
// ==============================================================================

#[derive(Debug, Clone)]
pub struct OciImageRef {
    pub registry: String,
    pub repository: String,
    pub tag: String,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum ManifestResponse {
    Single(SingleManifest),
    List(ManifestList),
}

#[derive(Deserialize, Debug)]
struct SingleManifest {
    layers: Vec<Descriptor>,
    config: Descriptor,
}

#[derive(Deserialize, Debug)]
struct ImageConfigBlob {
    config: ConfigContainer,
}

#[derive(Deserialize, Debug)]
pub struct ConfigContainer {
    #[serde(rename = "User", default)]
    pub user: String,
    #[serde(rename = "Env", default, deserialize_with = "nullable_env")]
    pub env: Vec<String>,
    #[serde(rename = "Cmd", default)]
    pub cmd: Option<Vec<String>>,
    #[serde(rename = "Entrypoint", default)]
    pub entrypoint: Option<Vec<String>>,
    #[serde(rename = "WorkingDir", default)]
    pub working_dir: String,
}

fn nullable_env<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(Option::<Vec<String>>::deserialize(d)?.unwrap_or_default())
}
async fn manifest_json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
    expected: Option<&str>,
) -> Result<T, Box<dyn std::error::Error + Send + Sync>> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
            return Err("OCI manifest exceeds 4 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if let Some(digest) = expected {
        verify_digest(digest, &bytes)?;
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[derive(Deserialize, Debug)]
struct ManifestList {
    manifests: Vec<ManifestDescriptor>,
}

#[derive(Deserialize, Debug)]
struct ManifestDescriptor {
    digest: String,
    platform: Platform,
}

#[derive(Deserialize, Debug)]
struct Platform {
    architecture: String,
    os: String,
}

#[derive(Deserialize, Debug)]
struct Descriptor {
    digest: String,
    #[serde(rename = "mediaType")]
    media_type: String,
    size: u64,
}

#[derive(Deserialize, Debug)]
struct TokenResponse {
    token: Option<String>,
    access_token: Option<String>,
}

// ==============================================================================
// Image Reference Parser
// ==============================================================================

impl OciImageRef {
    /// Parses an image reference string like "ubuntu:latest" or "ghcr.io/owner/repo:tag"
    pub fn parse(img_ref: &str) -> Result<Self, String> {
        let parts: Vec<&str> = img_ref.split('/').collect();
        let (registry, repo_part) =
            if parts.len() > 1 && (parts[0].contains('.') || parts[0].contains(':')) {
                // First part contains dots or colons, so it's a registry domain (e.g. ghcr.io)
                (parts[0].to_string(), parts[1..].join("/"))
            } else {
                // Default to Docker Hub
                ("registry-1.docker.io".to_string(), img_ref.to_string())
            };

        // Split repository and tag/digest
        let (mut repository, tag) = if let Some(idx) = repo_part.rfind('@') {
            (
                repo_part[..idx].to_string(),
                repo_part[idx + 1..].to_string(),
            )
        } else if let Some(idx) = repo_part.rfind(':') {
            (
                repo_part[..idx].to_string(),
                repo_part[idx + 1..].to_string(),
            )
        } else {
            (repo_part, "latest".to_string())
        };

        // Standard Docker Hub library expansion
        if registry == "registry-1.docker.io" && !repository.contains('/') {
            repository = format!("library/{}", repository);
        }

        // Validate components before they are interpolated into registry URLs:
        // no traversal segments, query strings, or path separators in tags.
        let (reg_host, reg_port) = match registry.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (registry.as_str(), None),
        };
        let registry_ok = !reg_host.is_empty()
            && reg_host.len() <= 253
            && reg_host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            && reg_port
                .map(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
                .unwrap_or(true);
        let repository_ok = !repository.is_empty()
            && repository.split('/').all(|seg| {
                !seg.is_empty()
                    && seg.len() <= 128
                    && seg != "."
                    && seg != ".."
                    && seg
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
            });
        let tag_ok = if let Some(digest) = tag.strip_prefix("sha256:") {
            digest.len() == 64 && digest.chars().all(|c| c.is_ascii_hexdigit())
        } else {
            !tag.is_empty()
                && tag.len() <= 128
                && tag
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        };
        if !registry_ok || !repository_ok || !tag_ok {
            return Err(format!("Invalid image reference: {}", img_ref));
        }

        Ok(Self {
            registry,
            repository,
            tag,
        })
    }
}

impl OciExtractor {
    /// Resolve a tag once to its immutable linux/amd64 manifest before conversion or preview.
    pub async fn resolved_image(&self, image: &str) -> Result<String, String> {
        let img = OciImageRef::parse(image)?;
        let token = self.fetch_token(&img).await.map_err(|e| e.to_string())?;
        let url = format!(
            "https://{}/v2/{}/manifests/{}",
            img.registry, img.repository, img.tag
        );
        let mut request = self.client.get(url).header(ACCEPT,"application/vnd.docker.distribution.manifest.v2+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json");
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
        let mut response = request
            .send()
            .await
            .map_err(|e| e.to_string())?
            .error_for_status()
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
            if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                return Err("OCI manifest exceeds 4 MiB".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        if img.tag.starts_with("sha256:") {
            verify_digest(&img.tag, &bytes).map_err(|e| e.to_string())?;
        }
        use sha2::Digest;
        let body: ManifestResponse = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let digest = match body {
            ManifestResponse::Single(_) => {
                format!("sha256:{}", hex::encode(sha2::Sha256::digest(&bytes)))
            }
            ManifestResponse::List(list) => {
                list.manifests
                    .into_iter()
                    .find(|m| m.platform.os == "linux" && m.platform.architecture == "amd64")
                    .ok_or("No linux/amd64 manifest")?
                    .digest
            }
        };
        Ok(format!("{}/{}@{}", img.registry, img.repository, digest))
    }
}

use std::collections::HashMap;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
struct PosixMetadata {
    entry_type: u8,
    mode: u32,
    uid: u64,
    gid: u64,
    size: u64,
    mtime: u64,
    link_name: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
struct ImageState {
    layers: Vec<String>,
    files: HashMap<String, PosixMetadata>,
}

/// The unpacked directory beside this caller-selected archive belongs to the conversion.
/// Keep its resolved parent so cleanup never selects a broader directory.
struct ExtractionCacheGuard {
    path: PathBuf,
    parent: PathBuf,
}

impl ExtractionCacheGuard {
    fn for_output(output: &Path) -> io::Result<Self> {
        let parent = fs::canonicalize(
            output
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
        let name = output
            .file_name()
            .ok_or_else(|| io::Error::other("Missing archive filename"))?;
        let mut cache_name = name.to_os_string();
        cache_name.push(".cache");
        let path = parent.join(cache_name);
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink() || !m.is_dir()) {
            return Err(io::Error::other(
                "Extraction cache must be a plain directory",
            ));
        }
        Ok(Self { path, parent })
    }
}

impl Drop for ExtractionCacheGuard {
    fn drop(&mut self) {
        if self.path.is_absolute() && self.path.parent() == Some(self.parent.as_path()) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn compress_to_tar_xz(
    src_dir: &Path,
    out_file: &Path,
    file_map: &HashMap<String, PosixMetadata>,
) -> io::Result<()> {
    let tar_xz = std::fs::File::create(out_file)?;
    // Use fast level 2 compression to avoid excessive CPU usage and timeouts
    let enc = xz2::write::XzEncoder::new(tar_xz, 2);
    let mut builder = tar::Builder::new(enc);

    // Sort paths to ensure deterministic tar order and parents before children
    let mut paths: Vec<&String> = file_map.keys().collect();
    paths.sort();

    for path_str in paths {
        // Defense in depth: keys may come from on-disk cache metadata. Never
        // read source files or emit tar entries outside the normalized layout.
        let clean = match sanitize_entry_path(Path::new(path_str)) {
            Some(c) => c.to_string_lossy().replace('\\', "/"),
            None => {
                tracing::warn!("Skipping unsafe file map entry: {}", path_str);
                continue;
            }
        };
        let meta = &file_map[path_str];
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::new(meta.entry_type));
        header.set_mode(meta.mode);
        header.set_uid(meta.uid);
        header.set_gid(meta.gid);
        header.set_size(meta.size);
        header.set_mtime(meta.mtime);

        if let Some(link) = &meta.link_name {
            header.set_link_name(link)?;
        }

        header.set_cksum();

        let etype = tar::EntryType::new(meta.entry_type);
        if etype.is_file() {
            // Resolve with chroot semantics so reads land where extraction
            // wrote, even when intermediate dirs are (absolute) symlinks.
            let real_path = resolve_in_rootfs(src_dir, Path::new(&clean))
                .ok_or_else(|| io::Error::other("Archive source escapes extraction root"))?;
            let mut file = std::fs::File::open(&real_path)?;
            builder.append_data(&mut header, &clean, &mut file)?;
        } else {
            // Directories, symlinks, etc. don't have file body bytes
            header.set_size(0);
            builder.append_data(&mut header, &clean, std::io::empty())?;
        }
    }

    builder.into_inner()?.finish()?.sync_all()?;
    Ok(())
}

/// Offload compression to a blocking threadpool so the Tokio async reactor is never starved,
/// and immediately prune the temporary unpacked cache folder upon completion.
async fn compress_and_cleanup(
    cache_dir: PathBuf,
    target_path: PathBuf,
    file_map: HashMap<String, PosixMetadata>,
) -> io::Result<()> {
    let cache_dir_clone = cache_dir.clone();
    tokio::task::spawn_blocking(move || {
        let temporary = target_path.with_extension(crate::runtime::id("archive_tmp"));
        let result = (|| {
            compress_to_tar_xz(&cache_dir_clone, &temporary, &file_map)?;
            fs::rename(&temporary, &target_path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    })
    .await
    .map_err(|e| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("Compression join error: {}", e),
        )
    })??;

    if cache_dir.exists() {
        tracing::debug!(
            "Cleaning up unpacked cache directory: {}",
            cache_dir.display()
        );
        let _ = fs::remove_dir_all(&cache_dir);
    }
    Ok(())
}

fn add_injected_file(
    cache_dir: &Path,
    rel_path: &str,
    content: &[u8],
    mode: u32,
    file_map: &mut HashMap<String, PosixMetadata>,
) -> io::Result<()> {
    let clean = sanitize_entry_path(Path::new(rel_path))
        .ok_or_else(|| io::Error::other("Unsafe injected file path"))?;
    let full = resolve_in_rootfs(cache_dir, &clean)
        .ok_or_else(|| io::Error::other("Injected file escapes extraction root"))?;
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::symlink_metadata(&full).is_ok_and(|m| m.file_type().is_symlink()) {
        fs::remove_file(&full)?;
    }
    fs::write(&full, content)?;
    let physical = full
        .strip_prefix(cache_dir)
        .map_err(|_| io::Error::other("Injected file escapes extraction root"))?
        .to_string_lossy()
        .replace('\\', "/");
    // A merged /usr image may expose /sbin through a symlink. Replace the physical
    // metadata too, so the old init symlink cannot overwrite the launcher in the tar.
    file_map.remove(&clean.to_string_lossy().replace('\\', "/"));
    file_map.insert(
        physical,
        PosixMetadata {
            entry_type: b'0',
            mode,
            uid: 0,
            gid: 0,
            size: content.len() as u64,
            mtime: crate::runtime::now(),
            link_name: None,
        },
    );
    Ok(())
}

fn add_injected_symlink(
    rel_path: &str,
    target: &str,
    file_map: &mut HashMap<String, PosixMetadata>,
) {
    let clean_rel = rel_path.trim_start_matches('/');
    let meta = PosixMetadata {
        entry_type: b'2', // symlink
        mode: 0o777,
        uid: 0,
        gid: 0,
        size: 0,
        mtime: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        link_name: Some(target.to_string()),
    };
    file_map.insert(clean_rel.to_string(), meta);
}

fn inject_init_and_entrypoint_services(
    cache_dir: &Path,
    image_config: Option<&ConfigContainer>,
    env_vars: Option<&[String]>,
    extra_files: Option<&HashMap<String, String>>,
    file_map: &mut HashMap<String, PosixMetadata>,
) -> io::Result<()> {
    let has_init = file_map.contains_key("sbin/init")
        || file_map.contains_key("bin/init")
        || file_map.contains_key("usr/sbin/init");
    let systemd_init = has_init
        && ["lib/systemd/systemd", "usr/lib/systemd/systemd"]
            .iter()
            .any(|p| file_map.contains_key(*p))
        && ["sbin/init", "usr/sbin/init", "bin/init"].iter().any(|p| {
            file_map
                .get(*p)
                .and_then(|m| m.link_name.as_deref())
                .is_some_and(|s| s.ends_with("/systemd"))
        });
    let openrc_init = has_init
        && file_map.contains_key("etc/inittab")
        && ["sbin/openrc-run", "usr/sbin/openrc-run"]
            .iter()
            .any(|p| file_map.contains_key(*p));
    // 1. Injected environment variables
    let mut combined_envs: Vec<String> = Vec::new();
    if let Some(cfg) = image_config {
        for e in &cfg.env {
            if !e.trim().is_empty() {
                combined_envs.push(e.clone());
            }
        }
    }
    if let Some(envs) = env_vars {
        for e in envs {
            if !e.trim().is_empty() {
                combined_envs.push(e.clone());
            }
        }
    }

    if !combined_envs.is_empty() {
        let mut env_script = String::from("#!/bin/sh\n# Hostable Injected Environment Variables\n");
        for e in &combined_envs {
            match e.split_once('=') {
                Some((k, v)) if valid_env_key(k) => {
                    env_script.push_str(&format!("export {}={}\n", k, shell_single_quote(v)));
                }
                _ => {
                    tracing::warn!("Skipping malformed environment entry");
                }
            }
        }
        add_injected_file(
            cache_dir,
            "etc/profile.d/hostable-env.sh",
            env_script.as_bytes(),
            0o600,
            file_map,
        )?;
    }

    // 2. Determine command & working dir
    let raw_workdir = image_config
        .map(|c| c.working_dir.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("/");
    // WorkingDir is untrusted (image config) and lands in shell scripts and
    // unit files: accept only a plain absolute path, else fall back to "/".
    let workdir = if raw_workdir.starts_with('/')
        && raw_workdir.len() <= 512
        && raw_workdir
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+".contains(c))
        && !raw_workdir.split('/').any(|seg| seg == "..")
    {
        raw_workdir
    } else {
        tracing::warn!("Rejecting unusual WorkingDir {:?}; using /", raw_workdir);
        "/"
    };

    let mut cmd_parts: Vec<String> = Vec::new();
    if let Some(cfg) = image_config {
        if let Some(entry) = &cfg.entrypoint {
            for e in entry {
                cmd_parts.push(e.clone());
            }
        }
        if let Some(cmd) = &cfg.cmd {
            for c in cmd {
                cmd_parts.push(c.clone());
            }
        }
    }

    let exec_cmd = if !cmd_parts.is_empty() {
        cmd_parts
            .iter()
            .map(|p| shell_single_quote(p))
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        String::from("/bin/sh")
    };

    // 3. Write hostable-entrypoint.sh wrapper script
    let entrypoint_script = format!(
        "#!/bin/sh\n# Hostable PID 1 Init Wrapper\n[ -f /etc/profile.d/hostable-env.sh ] && . /etc/profile.d/hostable-env.sh\ncd {}\nexec {} \"$@\"\n",
        shell_single_quote(workdir),
        exec_cmd
    );
    add_injected_file(
        cache_dir,
        "usr/local/bin/hostable-entrypoint.sh",
        entrypoint_script.as_bytes(),
        0o755,
        file_map,
    )?;

    // 4. Write systemd service file & enable symlink
    let systemd_service = format!(
        "[Unit]\nDescription=Hostable OCI Application Container Service\nAfter=network.target network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nWorkingDirectory={}\nExecStart=/usr/local/bin/hostable-entrypoint.sh\nRestart=always\nRestartSec=3\nKillMode=mixed\n\n[Install]\nWantedBy=multi-user.target\n",
        workdir
    );
    add_injected_file(
        cache_dir,
        "etc/systemd/system/hostable-app.service",
        systemd_service.as_bytes(),
        0o644,
        file_map,
    )?;
    add_injected_symlink(
        "etc/systemd/system/multi-user.target.wants/hostable-app.service",
        "/etc/systemd/system/hostable-app.service",
        file_map,
    );

    // 5. Write OpenRC init service & default runlevel symlink
    let openrc_service = format!(
        "#!/sbin/openrc-run\ndescription=\"Hostable OCI Application Container Service\"\ncommand=\"/usr/local/bin/hostable-entrypoint.sh\"\ncommand_background=\"true\"\npidfile=\"/run/hostable-app.pid\"\ndirectory=\"{}\"\noutput_log=\"/var/log/hostable-application.log\"\nerror_log=\"/var/log/hostable-application.log\"\n\nstart_pre() {{\n    mkdir -p /var/log\n    touch \"$output_log\"\n    chmod 600 \"$output_log\"\n}}\n\ndepend() {{\n    need net\n    after firewall\n}}\n",
        workdir
    );
    add_injected_file(
        cache_dir,
        "etc/init.d/hostable-app",
        openrc_service.as_bytes(),
        0o755,
        file_map,
    )?;
    add_injected_symlink(
        "etc/runlevels/default/hostable-app",
        "/etc/init.d/hostable-app",
        file_map,
    );

    // 6. A BusyBox init symlink alone does not start the OCI entrypoint. Keep
    // configured systemd/OpenRC guests; otherwise install the managed launcher.
    let fallback_init = r#"#!/bin/sh
# Configure the interface generated by Proxmox before launching the application.
mkdir -p /run /var/log
if command -v ifup >/dev/null 2>&1; then ifup -a || exit 1; fi
command -v setsid >/dev/null 2>&1 || { echo 'Hostable init requires setsid'; exit 1; }
touch /var/log/hostable-application.log
chmod 600 /var/log/hostable-application.log
setsid /usr/local/bin/hostable-entrypoint.sh >> /var/log/hostable-application.log 2>&1 &
child=$!
shutdown() { kill -TERM -"$child" 2>/dev/null || true; }
trap shutdown TERM INT HUP PWR
while kill -0 "$child" 2>/dev/null; do wait "$child"; result=$?; done
wait 2>/dev/null || true
exit "${result:-0}"
"#;
    add_injected_file(
        cache_dir,
        "usr/local/bin/hostable-init.sh",
        fallback_init.as_bytes(),
        0o755,
        file_map,
    )?;
    if !systemd_init && !openrc_init {
        add_injected_file(
            cache_dir,
            "sbin/init",
            fallback_init.as_bytes(),
            0o755,
            file_map,
        )?;
    }

    // 7. Inject any user extra files
    if let Some(files) = extra_files {
        for (rel, content) in files {
            add_injected_file(cache_dir, rel, content.as_bytes(), 0o644, file_map)?;
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
pub enum UpdateStatus {
    Unchanged,
    InPlaceUpdate,
    Recreated,
}

pub struct OciExtractor {
    client: reqwest::Client,
}

impl OciExtractor {
    pub fn new() -> Result<Self, String> {
        let mut builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::default())
            .connect_timeout(std::time::Duration::from_secs(20))
            .timeout(std::time::Duration::from_secs(1800));
        if let Some(path) = std::env::var_os("HOSTABLE_OCI_CA_CERT") {
            let pem = fs::read(path).map_err(|_| "Cannot read HOSTABLE_OCI_CA_CERT")?;
            let cert =
                reqwest::Certificate::from_pem(&pem).map_err(|_| "Invalid HOSTABLE_OCI_CA_CERT")?;
            builder = builder.add_root_certificate(cert);
        }
        Ok(Self {
            client: builder.build().map_err(|e| e.to_string())?,
        })
    }

    /// Fetches the dynamic authentication token required to access the registry repository.
    async fn fetch_token(
        &self,
        img: &OciImageRef,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        // 1. Try to ping the registry manifests endpoint unauthenticated to discover auth server via Www-Authenticate header
        let url = format!(
            "https://{}/v2/{}/manifests/{}",
            img.registry, img.repository, img.tag
        );

        tracing::info!(
            "Discovering authentication endpoint for {}...",
            img.registry
        );
        let res = self.client.get(&url).send().await?;

        if res.status().is_success() {
            // No auth required (very rare, but possible)
            return Ok(String::new());
        }

        let auth_header = match res.headers().get("Www-Authenticate") {
            Some(h) => h.to_str()?,
            None => {
                return Err(
                    "Registry returned unauthorized but provided no Www-Authenticate header".into(),
                );
            }
        };

        // Parse header: Bearer realm="https://auth.docker.io/token",service="registry.docker.io",scope="..."
        if !auth_header.starts_with("Bearer ") {
            return Err(format!("Unsupported authentication type: {}", auth_header).into());
        }

        let params = auth_header["Bearer ".len()..].trim();
        let mut realm = None;
        let mut service = None;

        for part in params.split(',') {
            let key_val: Vec<&str> = part.split('=').collect();
            if key_val.len() == 2 {
                let key = key_val[0].trim();
                let val = key_val[1].trim().trim_matches('"');
                if key == "realm" {
                    realm = Some(val.to_string());
                } else if key == "service" {
                    service = Some(val.to_string());
                }
            }
        }

        let realm = match realm {
            Some(r) => r,
            None => return Err("Www-Authenticate header did not contain a realm parameter".into()),
        };
        if !realm_allowed(&realm, &img.registry) {
            return Err(format!("Registry auth realm rejected: {}", realm).into());
        }

        // 2. Fetch the token from the authentication server
        let mut token_url = format!("{}?scope=repository:{}:pull", realm, img.repository);
        if let Some(srv) = service {
            if srv
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
            {
                token_url.push_str(&format!("&service={}", srv));
            }
        }

        tracing::info!("Fetching pull token from: {}...", token_url);
        let token_res = self.client.get(&token_url).send().await?;

        if !token_res.status().is_success() {
            return Err(format!("Failed to retrieve token: {}", token_res.status()).into());
        }

        let payload: TokenResponse = manifest_json(token_res, None).await?;
        let token = payload
            .token
            .or(payload.access_token)
            .ok_or("No token returned in auth server payload")?;

        Ok(token)
    }

    /// Pulls the manifests and downloads all OCI image layers, extracting them to a target directory.
    pub async fn extract_to_dir(
        &self,
        image_str: &str,
        target_path: &Path,
        env_vars: Option<&[String]>,
        extra_files: Option<std::collections::HashMap<String, String>>,
    ) -> Result<UpdateStatus, Box<dyn std::error::Error + Send + Sync>> {
        let img = OciImageRef::parse(image_str)?;
        let token = self.fetch_token(&img).await?;

        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.docker.distribution.manifest.v2+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json"),
        );
        if !token.is_empty() {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", token))?,
            );
        }

        // 1. Get manifest
        let manifest_url = format!(
            "https://{}/v2/{}/manifests/{}",
            img.registry, img.repository, img.tag
        );
        tracing::info!("Downloading manifest from {}...", manifest_url);

        let res = self
            .client
            .get(&manifest_url)
            .headers(headers.clone())
            .send()
            .await?;

        if !res.status().is_success() {
            return Err(format!("Failed to fetch manifest: {}", res.status()).into());
        }

        let manifest_response: ManifestResponse = manifest_json(
            res,
            if img.tag.starts_with("sha256:") {
                Some(img.tag.as_str())
            } else {
                None
            },
        )
        .await?;

        // Resolve Manifest List to a single arch manifest if needed
        let manifest = match manifest_response {
            ManifestResponse::Single(single) => single,
            ManifestResponse::List(list) => {
                // Find matching architecture and OS (default: amd64 / linux)
                let target = list.manifests.iter()
                    .find(|m| m.platform.architecture == "amd64" && m.platform.os == "linux")
                    .ok_or_else(|| {
                        let available_archs: Vec<String> = list.manifests.iter()
                            .map(|m| format!("{}/{}", m.platform.os, m.platform.architecture))
                            .collect();
                        format!("No matching amd64/linux platform found in manifest list. Available platforms: {:?}", available_archs)
                    })?;

                let digest_url = format!(
                    "https://{}/v2/{}/manifests/{}",
                    img.registry, img.repository, target.digest
                );
                tracing::info!(
                    "Resolving architecture-specific manifest [amd64/linux] from {}...",
                    &target.digest[..target.digest.len().min(12)]
                );

                let res = self
                    .client
                    .get(&digest_url)
                    .headers(headers.clone())
                    .send()
                    .await?;

                if !res.status().is_success() {
                    return Err(format!(
                        "Failed to fetch resolved single manifest: {}",
                        res.status()
                    )
                    .into());
                }

                let single: SingleManifest = manifest_json(res, Some(&target.digest)).await?;
                single
            }
        };

        // Fetch image config blob to inspect ENTRYPOINT, CMD, WORKINGDIR, ENV
        let config_url = format!(
            "https://{}/v2/{}/blobs/{}",
            img.registry, img.repository, manifest.config.digest
        );
        let mut response = self
            .client
            .get(&config_url)
            .headers(headers.clone())
            .send()
            .await?
            .error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > 4 * 1024 * 1024 {
                return Err("Image config exceeds 4 MiB".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        verify_digest(&manifest.config.digest, &bytes)?;
        let config: ImageConfigBlob = serde_json::from_slice(&bytes)?;
        if !matches!(
            config.config.user.as_str(),
            "" | "0" | "root" | "0:0" | "root:root"
        ) {
            return Err("Non-root OCI User is unsupported. Supply an image whose entrypoint drops privileges explicitly.".into());
        }
        let image_config = Some(config.config);

        let new_layers: Vec<String> = manifest.layers.iter().map(|l| l.digest.clone()).collect();
        tracing::info!(
            "Found {} layers in resolved image manifest.",
            manifest.layers.len()
        );

        let cleanup = ExtractionCacheGuard::for_output(target_path)?;
        let cache_dir = cleanup.path.clone();
        let meta_file = PathBuf::from(format!("{}.cache.meta", target_path.display()));
        let cache_existed = cache_dir.is_dir();
        crate::runtime::private_dir(&cache_dir)?;
        let _cleanup = cleanup;

        let mut needs_recreate = true;
        let mut layers_to_download = new_layers.clone();
        let mut start_layer_index = 0;
        let mut file_map: HashMap<String, PosixMetadata> = HashMap::new();

        if cache_existed && meta_file.exists() {
            if let Ok(meta_content) = fs::read_to_string(&meta_file) {
                if let Ok(state) = serde_json::from_str::<ImageState>(&meta_content) {
                    let mut matches = true;
                    let common_len = std::cmp::min(state.layers.len(), new_layers.len());
                    if common_len > 0 {
                        for i in 0..state.layers.len() {
                            if i < new_layers.len() && state.layers[i] == new_layers[i] {
                                continue;
                            } else {
                                matches = false;
                                break;
                            }
                        }
                    } else {
                        matches = false;
                    }

                    if matches {
                        needs_recreate = false;
                        start_layer_index = state.layers.len();
                        layers_to_download = new_layers
                            .get(start_layer_index..)
                            .unwrap_or_default()
                            .to_vec();
                        file_map = state.files;
                    }
                }
            }
        }

        if layers_to_download.is_empty() {
            tracing::info!("Image hasn't changed. Using existing cache...");
            if !target_path.exists() {
                tracing::info!("Compressing rootfs to {}...", target_path.display());
                inject_init_and_entrypoint_services(
                    &cache_dir,
                    image_config.as_ref(),
                    env_vars,
                    extra_files.as_ref(),
                    &mut file_map,
                )?;
                compress_and_cleanup(
                    cache_dir.clone(),
                    target_path.to_path_buf(),
                    file_map.clone(),
                )
                .await?;
            }
            return Ok(UpdateStatus::Unchanged);
        }

        let final_status = if needs_recreate {
            tracing::info!(
                "Base layers changed or cache missing. Recreating container from scratch..."
            );
            if cache_dir.exists() {
                fs::remove_dir_all(&cache_dir)?;
            }
            crate::runtime::private_dir(&cache_dir)?;
            file_map.clear();
            UpdateStatus::Recreated
        } else {
            tracing::info!("Top layers changed. Performing in-place update...");
            UpdateStatus::InPlaceUpdate
        };

        // 2. Download and extract layers sequentially
        for i in start_layer_index..manifest.layers.len() {
            let layer = &manifest.layers[i];
            tracing::info!(
                "Processing Layer {}/{} [Digest: {}]...",
                i + 1,
                manifest.layers.len(),
                &layer.digest[..layer.digest.len().min(12)]
            );

            let blob_url = format!(
                "https://{}/v2/{}/blobs/{}",
                img.registry, img.repository, layer.digest
            );

            let mut blob_res = self
                .client
                .get(&blob_url)
                .headers(headers.clone())
                .send()
                .await?;

            if !blob_res.status().is_success() {
                return Err(format!("Failed to fetch layer blob: {}", blob_res.status()).into());
            }

            if !layer.media_type.ends_with(".gzip") && !layer.media_type.ends_with("+gzip") {
                return Err("Only gzip-compressed OCI layers are supported".into());
            }
            if layer.size > 8 * 1024 * 1024 * 1024 {
                return Err("Compressed layer exceeds 8 GiB".into());
            }
            // Stream response chunks to a temporary file on disk to prevent RAM OOM
            let clean_digest: String = layer
                .digest
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            let tmp_layer_name =
                format!(".layer_{}.tmp", &clean_digest[..clean_digest.len().min(16)]);
            let tmp_layer_path = cache_dir.join(&tmp_layer_name);

            let mut tmp_file = tokio::fs::File::create(&tmp_layer_path).await?;
            use sha2::Digest;
            let mut hash = sha2::Sha256::new();
            let mut size = 0u64;
            while let Some(chunk) = blob_res.chunk().await? {
                size += chunk.len() as u64;
                if size > layer.size || size > 8 * 1024 * 1024 * 1024 {
                    return Err("Downloaded layer exceeds declared size".into());
                }
                hash.update(&chunk);
                tmp_file.write_all(&chunk).await?;
            }
            if size != layer.size
                || layer.digest != format!("sha256:{}", hex::encode(hash.finalize()))
            {
                return Err("OCI layer size or digest mismatch".into());
            }
            tmp_file.flush().await?;
            drop(tmp_file);

            // Extract the tar.gz stream directly from disk file
            let file = std::fs::File::open(&tmp_layer_path)?;
            let extract_res = extract_layer_stream(file, &cache_dir, &mut file_map);
            let _ = std::fs::remove_file(&tmp_layer_path);
            extract_res?;
        }

        // Save metadata
        let state = ImageState {
            layers: new_layers,
            files: file_map.clone(),
        };
        fs::write(&meta_file, serde_json::to_string(&state)?)?;

        tracing::info!(
            "Extract completed successfully. Compressing rootfs to {}...",
            target_path.display()
        );

        inject_init_and_entrypoint_services(
            &cache_dir,
            image_config.as_ref(),
            env_vars,
            extra_files.as_ref(),
            &mut file_map,
        )?;

        compress_and_cleanup(cache_dir, target_path.to_path_buf(), file_map).await?;

        Ok(final_status)
    }

    pub async fn get_image_config(
        &self,
        image_str: &str,
    ) -> Result<ConfigContainer, Box<dyn std::error::Error + Send + Sync>> {
        let img = OciImageRef::parse(image_str)?;
        let token = self.fetch_token(&img).await?;

        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.docker.distribution.manifest.v2+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json"),
        );
        if !token.is_empty() {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", token))?,
            );
        }

        let manifest_url = format!(
            "https://{}/v2/{}/manifests/{}",
            img.registry, img.repository, img.tag
        );
        let res = self
            .client
            .get(&manifest_url)
            .headers(headers.clone())
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(format!(
                "Failed to fetch manifest for {}: {}",
                image_str,
                res.status()
            )
            .into());
        }

        let manifest_response: ManifestResponse = manifest_json(
            res,
            if img.tag.starts_with("sha256:") {
                Some(&img.tag)
            } else {
                None
            },
        )
        .await?;
        let manifest = match manifest_response {
            ManifestResponse::Single(single) => single,
            ManifestResponse::List(list) => {
                let target = list
                    .manifests
                    .iter()
                    .find(|m| m.platform.architecture == "amd64" && m.platform.os == "linux")
                    .ok_or_else(|| {
                        format!(
                            "No amd64/linux platform found in manifest list for {}",
                            image_str
                        )
                    })?;

                let digest_url = format!(
                    "https://{}/v2/{}/manifests/{}",
                    img.registry, img.repository, target.digest
                );
                let res = self
                    .client
                    .get(&digest_url)
                    .headers(headers.clone())
                    .send()
                    .await?;
                if !res.status().is_success() {
                    return Err(format!(
                        "Failed to resolve manifest for {}: {}",
                        image_str,
                        res.status()
                    )
                    .into());
                }
                manifest_json(res, Some(&target.digest)).await?
            }
        };

        self.fetch_config_blob(image_str, &manifest.config.digest)
            .await
    }

    pub async fn fetch_config_blob(
        &self,
        image_str: &str,
        digest: &str,
    ) -> Result<ConfigContainer, Box<dyn std::error::Error + Send + Sync>> {
        let img = OciImageRef::parse(image_str)?;
        let token = self.fetch_token(&img).await?;

        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT, HeaderValue::from_static("application/vnd.docker.container.image.v1+json, application/vnd.oci.image.config.v1+json"));
        if !token.is_empty() {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", token))?,
            );
        }

        let blob_url = format!(
            "https://{}/v2/{}/blobs/{}",
            img.registry, img.repository, digest
        );
        let res = self.client.get(&blob_url).headers(headers).send().await?;
        if !res.status().is_success() {
            return Err(format!("Failed to fetch config blob: {}", res.status()).into());
        }

        let config_blob: ImageConfigBlob = manifest_json(res, Some(digest)).await?;
        Ok(config_blob.config)
    }

    /// Extracts multiple images into a single directory sequentially, effectively merging their filesystems.
    pub async fn extract_multiple_to_dir(
        &self,
        image_strs: &[String],
        target_path: &Path,
        extra_files: Option<std::collections::HashMap<String, String>>,
    ) -> Result<UpdateStatus, Box<dyn std::error::Error + Send + Sync>> {
        let cache_dir = PathBuf::from(format!("{}.cache", target_path.display()));

        if cache_dir.exists() {
            fs::remove_dir_all(&cache_dir)?;
        }
        fs::create_dir_all(&cache_dir)?;

        let mut master_file_map: HashMap<String, PosixMetadata> = HashMap::new();

        for (idx, image_str) in image_strs.iter().enumerate() {
            tracing::info!(
                "--- Merging Image {}/{} [{}] ---",
                idx + 1,
                image_strs.len(),
                image_str
            );

            let img = OciImageRef::parse(image_str)?;
            let token = self.fetch_token(&img).await?;

            let mut headers = HeaderMap::new();
            headers.insert(
                ACCEPT,
                HeaderValue::from_static("application/vnd.docker.distribution.manifest.v2+json, application/vnd.docker.distribution.manifest.list.v2+json, application/vnd.oci.image.manifest.v1+json, application/vnd.oci.image.index.v1+json"),
            );
            if !token.is_empty() {
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {}", token))?,
                );
            }

            let manifest_url = format!(
                "https://{}/v2/{}/manifests/{}",
                img.registry, img.repository, img.tag
            );
            let res = self
                .client
                .get(&manifest_url)
                .headers(headers.clone())
                .send()
                .await?;
            if !res.status().is_success() {
                return Err(format!(
                    "Failed to fetch manifest for {}: {}",
                    image_str,
                    res.status()
                )
                .into());
            }

            let manifest_response: ManifestResponse = res.json().await?;
            let manifest = match manifest_response {
                ManifestResponse::Single(single) => single,
                ManifestResponse::List(list) => {
                    let target = list
                        .manifests
                        .iter()
                        .find(|m| m.platform.architecture == "amd64" && m.platform.os == "linux")
                        .ok_or_else(|| {
                            format!(
                                "No amd64/linux platform found in manifest list for {}",
                                image_str
                            )
                        })?;

                    let digest_url = format!(
                        "https://{}/v2/{}/manifests/{}",
                        img.registry, img.repository, target.digest
                    );
                    let res = self
                        .client
                        .get(&digest_url)
                        .headers(headers.clone())
                        .send()
                        .await?;
                    if !res.status().is_success() {
                        return Err(format!(
                            "Failed to resolve manifest for {}: {}",
                            image_str,
                            res.status()
                        )
                        .into());
                    }
                    res.json().await?
                }
            };

            for (lidx, layer) in manifest.layers.iter().enumerate() {
                tracing::info!(
                    "Processing Layer {}/{} [Digest: {}]...",
                    lidx + 1,
                    manifest.layers.len(),
                    &layer.digest[..layer.digest.len().min(12)]
                );
                let blob_url = format!(
                    "https://{}/v2/{}/blobs/{}",
                    img.registry, img.repository, layer.digest
                );
                let mut blob_res = self
                    .client
                    .get(&blob_url)
                    .headers(headers.clone())
                    .send()
                    .await?;
                if !blob_res.status().is_success() {
                    return Err(format!("Failed to fetch layer blob: {}", blob_res.status()).into());
                }

                // Stream response chunks to a temporary file on disk to prevent RAM OOM
                let clean_digest: String = layer
                    .digest
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .collect();
                let tmp_layer_name =
                    format!(".layer_{}.tmp", &clean_digest[..clean_digest.len().min(16)]);
                let tmp_layer_path = cache_dir.join(&tmp_layer_name);

                let mut tmp_file = tokio::fs::File::create(&tmp_layer_path).await?;
                while let Some(chunk) = blob_res.chunk().await? {
                    tmp_file.write_all(&chunk).await?;
                }
                tmp_file.flush().await?;
                drop(tmp_file);

                // Extract the tar.gz stream directly from disk file
                let file = std::fs::File::open(&tmp_layer_path)?;
                let extract_res = extract_layer_stream(file, &cache_dir, &mut master_file_map);
                let _ = std::fs::remove_file(&tmp_layer_path);
                extract_res?;
            }
        }

        if let Some(files) = extra_files {
            for (path_str, content) in files {
                // Keys may come from API input: never join unnormalized paths
                // (an absolute key would replace the cache root entirely).
                let clean_key = match sanitize_entry_path(Path::new(&path_str)) {
                    Some(p) => p.to_string_lossy().replace('\\', "/"),
                    None => {
                        tracing::warn!("Skipping extra file with unsafe path: {}", path_str);
                        continue;
                    }
                };
                let full_path = resolve_in_rootfs(&cache_dir, Path::new(&clean_key))
                    .unwrap_or_else(|| cache_dir.join(&clean_key));
                if let Some(p) = full_path.parent() {
                    let _ = fs::create_dir_all(p);
                }
                if let Ok(meta) = fs::symlink_metadata(&full_path) {
                    if meta.file_type().is_symlink() {
                        let _ = fs::remove_file(&full_path);
                    }
                }

                let mut meta = PosixMetadata {
                    entry_type: b'0',
                    mode: 0o755,
                    uid: 0,
                    gid: 0,
                    size: content.len() as u64,
                    mtime: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    link_name: None,
                };

                if content.starts_with("symlink:") {
                    let target = content.trim_start_matches("symlink:").to_string();
                    meta.entry_type = b'2'; // Symlink
                    meta.link_name = Some(target);
                    meta.size = 0;
                    // Don't write file content if it's a symlink in tar
                } else {
                    let _ = fs::write(&full_path, &content);
                }

                master_file_map.insert(path_str.trim_start_matches('/').to_string(), meta);
            }
        }

        tracing::info!("All images extracted and merged! Compressing merged rootfs...");
        compress_and_cleanup(cache_dir, target_path.to_path_buf(), master_file_map).await?;

        Ok(UpdateStatus::Recreated)
    }
}

// ==============================================================================
// Layer Extraction and OCI Whiteout Handler
// ==============================================================================

/// Normalize a tar entry path to a safe relative path: drops root/prefix and
/// current-dir components and rejects any parent-dir (`..`) traversal.
fn verify_digest(expected: &str, bytes: &[u8]) -> Result<(), String> {
    use sha2::Digest;
    if expected == format!("sha256:{}", hex::encode(sha2::Sha256::digest(bytes))) {
        Ok(())
    } else {
        Err("OCI content digest mismatch".into())
    }
}
fn sanitize_entry_path(path: &Path) -> Option<PathBuf> {
    let mut clean = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::Normal(c) => clean.push(c),
            std::path::Component::CurDir => {}
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {}
            std::path::Component::ParentDir => return None,
        }
    }
    if clean.as_os_str().is_empty() {
        None
    } else {
        Some(clean)
    }
}

/// Resolve `rel` inside `root` with container (chroot) semantics so extraction
/// can never touch the host outside the rootfs: absolute symlink targets are
/// re-rooted at `root`, `..` never climbs above `root`, and symlink chains are
/// depth-bounded. Only ancestor components are resolved â€” the final component
/// is returned as-is so callers can replace it (tar entry semantics).
fn link_components(parent: &[String], target: &Path) -> Option<Vec<String>> {
    // OCI absolute paths are rooted in the guest, including on Windows hosts.
    let mut next = if target.has_root() {
        vec![]
    } else {
        parent.to_vec()
    };
    for tc in target.components() {
        match tc {
            std::path::Component::Normal(s) => next.push(s.to_string_lossy().to_string()),
            std::path::Component::CurDir | std::path::Component::RootDir => {}
            std::path::Component::ParentDir => {
                next.pop()?;
            }
            std::path::Component::Prefix(_) => return None,
        }
    }
    Some(next)
}

fn resolve_in_rootfs(root: &Path, rel: &Path) -> Option<PathBuf> {
    let mut comps: Vec<String> = Vec::new();
    for c in rel.components() {
        match c {
            std::path::Component::Normal(s) => comps.push(s.to_string_lossy().to_string()),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    if comps.is_empty() {
        return Some(root.to_path_buf());
    }
    let leaf = comps.pop()?;

    for _ in 0..32 {
        let mut symlink_at = None;
        let mut cur = root.to_path_buf();
        for (i, c) in comps.iter().enumerate() {
            cur.push(c);
            if let Ok(meta) = fs::symlink_metadata(&cur) {
                if meta.file_type().is_symlink() {
                    symlink_at = Some(i);
                    break;
                }
            }
        }
        let i = match symlink_at {
            None => {
                let mut out = root.to_path_buf();
                for c in &comps {
                    out.push(c);
                }
                out.push(leaf);
                return Some(out);
            }
            Some(i) => i,
        };

        let mut link_path = root.to_path_buf();
        for c in &comps[..=i] {
            link_path.push(c);
        }
        let target = fs::read_link(&link_path).ok()?;

        let mut next = link_components(&comps[..i], &target)?;
        next.extend(comps[i + 1..].iter().cloned());
        comps = next;
    }
    None // symlink chain too deep
}

/// Single-quote a value for POSIX sh, neutralizing all shell metacharacters.
pub fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Environment variable names must be plain POSIX identifiers.
pub fn valid_env_key(k: &str) -> bool {
    let mut chars = k.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Validates an image reference before it is used to build registry URLs.
pub fn validate_image_ref(img_ref: &str) -> bool {
    OciImageRef::parse(img_ref).is_ok()
}

/// Registry auth realms come from response headers (attacker-influenced when
/// pulling a hostile image): allow https anywhere, but plain http only when the
/// auth server sits on the registry host itself. Rejects userinfo and control
/// characters outright.
fn realm_allowed(realm: &str, registry: &str) -> bool {
    if realm.contains(['@', ' ', '\t', '\n', '\r', '#']) {
        return false;
    }
    if realm.starts_with("https://") {
        return true;
    }
    if let Some(rest) = realm.strip_prefix("http://") {
        let host = rest.split(['/', '?']).next().unwrap_or("");
        return !host.is_empty() && host.split(':').next() == registry.split(':').next();
    }
    false
}

fn extract_layer_stream<R: Read>(
    stream: R,
    target_dir: &Path,
    file_map: &mut HashMap<String, PosixMetadata>,
) -> io::Result<()> {
    let dec = GzDecoder::new(stream);
    let mut archive = Archive::new(dec);

    let mut expanded = 0u64;
    let mut count = 0usize;
    'entries: for entry_res in archive.entries()? {
        count += 1;
        if count > 1_000_000 {
            return Err(io::Error::other("OCI layer exceeds file count limit"));
        }
        let mut entry = entry_res?;
        expanded = expanded
            .checked_add(entry.size())
            .ok_or_else(|| io::Error::other("Layer size overflow"))?;
        if expanded > 32 * 1024 * 1024 * 1024 {
            return Err(io::Error::other("Expanded layer exceeds 32 GiB"));
        }
        let raw_path = entry.path()?.to_path_buf();
        let path = match sanitize_entry_path(&raw_path) {
            Some(p) => p,
            None => {
                tracing::warn!("Skipping unsafe tar entry path: {}", raw_path.display());
                continue;
            }
        };
        let path_str = path.to_string_lossy().replace("\\", "/");

        if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
            // Opaque Directory Whiteout (.wh..wh..opq)
            if file_name == ".wh..wh..opq" {
                if let Some(parent) = path.parent() {
                    let parent_str = parent.to_string_lossy().replace("\\", "/");
                    let prefix = if parent_str.is_empty() {
                        String::new()
                    } else {
                        format!("{}/", parent_str)
                    };

                    file_map.retain(|k, _| {
                        if parent_str.is_empty() {
                            false // Wipe everything
                        } else {
                            !(k.starts_with(&prefix) || k == &parent_str)
                        }
                    });

                    let target_parent = match resolve_in_rootfs(target_dir, parent) {
                        Some(p) => p,
                        None => {
                            tracing::warn!(
                                "Skipping opaque whiteout escaping extraction root: {}",
                                path_str
                            );
                            continue 'entries;
                        }
                    };
                    if let Ok(meta) = fs::symlink_metadata(&target_parent) {
                        tracing::debug!(
                            "Handling opaque whiteout: cleaning parent folder {}",
                            parent.display()
                        );
                        if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
                            let _ = fs::remove_file(&target_parent);
                        } else {
                            let _ = fs::remove_dir_all(&target_parent);
                        }
                        let _ = fs::create_dir_all(&target_parent);
                    }
                }
                continue;
            }

            // Standard OCI File/Directory Whiteout (.wh.<filename>)
            if file_name.starts_with(".wh.") {
                let target_name = &file_name[4..];
                // The target must be a plain single path segment: "", "." and
                // ".." would delete the parent or escape the extraction root.
                if target_name.is_empty()
                    || target_name == "."
                    || target_name == ".."
                    || target_name.contains('/')
                    || target_name.contains('\\')
                {
                    tracing::warn!("Ignoring invalid whiteout entry: {}", path_str);
                    continue;
                }
                let parent = path.parent().unwrap_or(Path::new(""));
                let target_path = parent.join(target_name);
                let target_str = target_path.to_string_lossy().replace("\\", "/");
                let prefix = format!("{}/", target_str);

                file_map.retain(|k, _| !(k.starts_with(&prefix) || k == &target_str));

                let path_to_delete = match resolve_in_rootfs(target_dir, &target_path) {
                    Some(p) => p,
                    None => {
                        tracing::warn!(
                            "Skipping whiteout deletion escaping extraction root: {}",
                            path_str
                        );
                        continue;
                    }
                };
                if let Ok(meta) = fs::symlink_metadata(&path_to_delete) {
                    tracing::debug!(
                        "Handling whiteout deletion: removing {}",
                        path_to_delete.display()
                    );
                    // remove_dir_all only for real directories; symlinks and
                    // files are unlinked without following anything.
                    if meta.file_type().is_dir() {
                        let _ = fs::remove_dir_all(&path_to_delete);
                    } else {
                        let _ = fs::remove_file(&path_to_delete);
                    }
                }
                continue;
            }
        }

        // Write target confined to the extraction root: parents resolve with
        // chroot semantics so nothing can land outside the rootfs.
        let out_path = match resolve_in_rootfs(target_dir, &path) {
            Some(p) => p,
            None => {
                tracing::warn!(
                    "Skipping entry escaping extraction root via symlink: {}",
                    path_str
                );
                continue 'entries;
            }
        };

        // Replace any pre-existing symlink at the target so unpacking cannot
        // write through it to a location outside the extraction root.
        if let Ok(meta) = fs::symlink_metadata(&out_path) {
            if meta.file_type().is_symlink() {
                let _ = fs::remove_file(&out_path);
            }
        }

        // Store POSIX Metadata
        let header = entry.header();
        let mut meta = PosixMetadata {
            entry_type: header.entry_type().as_byte(),
            mode: header.mode().unwrap_or(0o644),
            uid: header.uid().unwrap_or(0),
            gid: header.gid().unwrap_or(0),
            size: header.size().unwrap_or(0),
            mtime: header.mtime().unwrap_or(0),
            link_name: header
                .link_name()
                .ok()
                .flatten()
                .map(|p| p.to_string_lossy().to_string()),
        };
        if header.entry_type().is_hard_link() {
            let target = meta
                .link_name
                .as_ref()
                .and_then(|s| sanitize_entry_path(Path::new(s)))
                .ok_or_else(|| io::Error::other("Unsafe OCI hard link"))?;
            meta.link_name = Some(target.to_string_lossy().replace('\\', "/"));
        }
        file_map.insert(path_str.clone(), meta);

        // Standard OCI File/Directory unpacking to cache dir
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            fs::create_dir_all(&out_path)?;
        } else if entry_type.is_hard_link() {
            if let Some(link_name) = entry.header().link_name()? {
                if let Some(clean_link) = sanitize_entry_path(&link_name) {
                    if let Some(source_path) = resolve_in_rootfs(target_dir, &clean_link) {
                        if source_path.exists() {
                            if let Err(_) = fs::hard_link(&source_path, &out_path) {
                                if fs::symlink_metadata(&source_path)
                                    .map(|m| m.is_file())
                                    .unwrap_or(false)
                                {
                                    let _ = fs::copy(&source_path, &out_path);
                                }
                            }
                        }
                    }
                }
            }
        } else if entry_type.is_symlink() {
            if let Some(link_name) = entry.header().link_name()? {
                #[cfg(windows)]
                {
                    let target_str = link_name.to_string_lossy().to_string();
                    let _ = fs::write(&out_path, format!("symlink:{}", target_str));
                }
                #[cfg(not(windows))]
                {
                    let _ = entry.unpack(&out_path);
                }
            }
        } else {
            if entry_type.is_file() {
                let mut file = fs::File::create(&out_path)?;
                io::copy(&mut entry, &mut file)?;
            } else if !entry_type.is_contiguous() {
                // Device nodes and other special entries are represented by metadata, never created on the manager.
            }
        }
    }

    Ok(())
}

/// Scans a cache directory and prunes any orphaned temporary layer files (.layer_*.tmp)
/// or unpacked cache directories (*.cache) older than max_age_secs.
pub fn prune_stale_cache_files(cache_dir: &Path, max_age_secs: u64) {
    if !cache_dir.exists() {
        return;
    }
    let now = std::time::SystemTime::now();
    if let Ok(entries) = fs::read_dir(cache_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if (name.starts_with(".layer_") && name.ends_with(".tmp")) || name.ends_with(".cache") {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(modified) = meta.modified() {
                        if let Ok(age) = now.duration_since(modified) {
                            if age.as_secs() >= max_age_secs {
                                tracing::info!("Pruning stale cache artifact: {}", path.display());
                                if meta.is_dir() {
                                    let _ = fs::remove_dir_all(&path);
                                } else {
                                    let _ = fs::remove_file(&path);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ==============================================================================
// Unit Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDirectoryGuard(PathBuf);
    impl Drop for TestDirectoryGuard {
        fn drop(&mut self) {
            if let Ok(parent) = fs::canonicalize(std::env::temp_dir()) {
                if self.0.parent() == Some(parent.as_path())
                    && self
                        .0
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("hostable_"))
                {
                    let _ = fs::remove_dir_all(&self.0);
                }
            }
        }
    }

    #[test]
    fn test_parse_image_references() {
        // Standard Library
        let img = OciImageRef::parse("alpine:3.19").unwrap();
        assert_eq!(img.registry, "registry-1.docker.io");
        assert_eq!(img.repository, "library/alpine");
        assert_eq!(img.tag, "3.19");

        // Custom Repository
        let img = OciImageRef::parse("linuxserver/jellyfin:noble").unwrap();
        assert_eq!(img.registry, "registry-1.docker.io");
        assert_eq!(img.repository, "linuxserver/jellyfin");
        assert_eq!(img.tag, "noble");

        // Custom Registry
        let img = OciImageRef::parse("ghcr.io/linuxserver/radarr:latest").unwrap();
        assert_eq!(img.registry, "ghcr.io");
        assert_eq!(img.repository, "linuxserver/radarr");
        assert_eq!(img.tag, "latest");
    }

    #[test]
    fn test_sanitize_entry_path_hardening() {
        assert_eq!(sanitize_entry_path(Path::new("../etc/passwd")), None);
        assert_eq!(sanitize_entry_path(Path::new("a/../../b")), None);
        assert_eq!(sanitize_entry_path(Path::new(".")), None);
        assert_eq!(
            sanitize_entry_path(Path::new("/etc/passwd")),
            Some(PathBuf::from("etc/passwd"))
        );
        assert_eq!(
            sanitize_entry_path(Path::new("./usr/bin")),
            Some(PathBuf::from("usr/bin"))
        );
    }

    #[test]
    fn test_shell_quoting_and_env_keys() {
        assert_eq!(shell_single_quote("simple"), "'simple'");
        assert_eq!(shell_single_quote("a'b"), r"'a'\''b'");
        assert_eq!(shell_single_quote("$(rm -rf /)"), "'$(rm -rf /)'");
        assert!(valid_env_key("PATH_2"));
        assert!(!valid_env_key("2PATH"));
        assert!(!valid_env_key("A B"));
        assert!(!valid_env_key("A\nB"));
        assert!(!valid_env_key(""));
    }

    #[test]
    fn test_workdir_is_sanitized_in_init_script() {
        let temp_dir = std::env::temp_dir().join(format!("hostable_wd_{}", rand::random::<u32>()));
        fs::create_dir_all(&temp_dir).unwrap();
        let mut file_map: HashMap<String, PosixMetadata> = HashMap::new();

        let cfg = ConfigContainer {
            user: String::new(),
            env: vec![],
            entrypoint: Some(vec!["/bin/app".to_string()]),
            cmd: None,
            working_dir: "/app$(touch /tmp/pwned)".to_string(),
        };
        inject_init_and_entrypoint_services(&temp_dir, Some(&cfg), None, None, &mut file_map)
            .unwrap();

        let script =
            fs::read_to_string(temp_dir.join("usr/local/bin/hostable-entrypoint.sh")).unwrap();
        // Hostile workdir must fall back to "/" â€” never reach the script raw.
        assert!(!script.contains("touch /tmp/pwned"));
        assert!(script.contains("cd '/'"));
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_parse_rejects_malicious_refs() {
        assert!(OciImageRef::parse("alpine:../evil").is_err());
        assert!(OciImageRef::parse("ghcr.io/../evil:tag").is_err());
        assert!(OciImageRef::parse("alpine:ta/g").is_err());
        assert!(OciImageRef::parse("alpine:tag?x=1").is_err());
        assert!(OciImageRef::parse("bad_host!/repo:tag").is_err());
        assert!(OciImageRef::parse("alpine:#frag").is_err());
        assert!(OciImageRef::parse("alpine:sha256:zz").is_err());
        assert!(validate_image_ref("alpine:3.19"));
        assert!(validate_image_ref(
            "ghcr.io/owner/app@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
    }

    #[test]
    fn test_resolve_in_rootfs_containment() {
        let root = std::env::temp_dir().join(format!("hostable_resolve_{}", rand::random::<u32>()));
        fs::create_dir_all(root.join("usr/bin")).unwrap();

        // Plain paths resolve inside the root.
        assert_eq!(
            resolve_in_rootfs(&root, Path::new("usr/bin/app")).unwrap(),
            root.join("usr/bin/app")
        );

        // Parent traversal never escapes the rootfs.
        assert_eq!(resolve_in_rootfs(&root, Path::new("..")), None);
        assert_eq!(resolve_in_rootfs(&root, Path::new("a/../../b")), None);

        // Leaf symlinks are replaced, not followed (tar semantics).
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", root.join("evil")).unwrap();
            assert_eq!(
                resolve_in_rootfs(&root, Path::new("evil")).unwrap(),
                root.join("evil")
            );

            // Absolute ancestor symlinks re-root at the rootfs (chroot semantics)
            // instead of escaping to the host.
            std::os::unix::fs::symlink("/usr/bin", root.join("binlink")).unwrap();
            assert_eq!(
                resolve_in_rootfs(&root, Path::new("binlink/app")).unwrap(),
                root.join("usr/bin/app")
            );

            // Dangling ancestors resolve in-root too.
            std::os::unix::fs::symlink("/nowhere", root.join("dangle")).unwrap();
            assert_eq!(
                resolve_in_rootfs(&root, Path::new("dangle/x")).unwrap(),
                root.join("nowhere/x")
            );
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_realm_allowed() {
        assert!(realm_allowed(
            "https://auth.docker.io/token",
            "registry-1.docker.io"
        ));
        assert!(realm_allowed(
            "http://registry.local:5000/auth",
            "registry.local:5000"
        ));
        // Plain http to a foreign host is rejected (token theft / SSRF).
        assert!(!realm_allowed(
            "http://evil.example.com/token",
            "registry.local:5000"
        ));
        // Userinfo tricks and control characters are rejected outright.
        assert!(!realm_allowed(
            "https://good.example.com@evil.com/x",
            "registry-1.docker.io"
        ));
        assert!(!realm_allowed("https://a.example\n.com/", "a.example"));
        // Non-http schemes are rejected.
        assert!(!realm_allowed("file:///etc/passwd", "a.example"));
    }

    #[test]
    fn test_whiteout_cleanup_logic() {
        let mut target_dir = std::env::temp_dir();
        target_dir.push(format!("hostable_test_{}", rand::random::<u32>()));
        fs::create_dir_all(&target_dir).unwrap();

        // 1. Setup pre-existing folder & files representing a previous layer
        let dummy_dir = target_dir.join("etc/nginx");
        fs::create_dir_all(&dummy_dir).unwrap();

        let dummy_file1 = dummy_dir.join("nginx.conf");
        let dummy_file2 = dummy_dir.join("mime.types");
        fs::write(&dummy_file1, "contents").unwrap();
        fs::write(&dummy_file2, "contents").unwrap();

        assert!(dummy_file1.exists());
        assert!(dummy_file2.exists());

        // 2. Perform a simulated standard whiteout delete of nginx.conf
        let path_to_delete = target_dir.join("etc/nginx/.wh.nginx.conf");
        let file_name = path_to_delete.file_name().unwrap().to_str().unwrap();
        assert!(file_name.starts_with(".wh."));

        let target_name = &file_name[4..];
        let parent = path_to_delete
            .parent()
            .unwrap()
            .strip_prefix(&target_dir)
            .unwrap();
        let path_to_rm = target_dir.join(parent).join(target_name);

        if path_to_rm.exists() {
            fs::remove_file(&path_to_rm).unwrap();
        }

        assert!(!dummy_file1.exists());
        assert!(dummy_file2.exists());

        // Cleanup temp folder
        let _ = fs::remove_dir_all(&target_dir);
    }

    #[test]
    fn test_inject_init_and_entrypoint_services() {
        let temp_dir =
            std::env::temp_dir().join(format!("hostable_init_test_{}", rand::random::<u32>()));
        let mut file_map = HashMap::new();

        let cfg = ConfigContainer {
            user: String::new(),
            env: vec!["APP_PORT=8080".to_string()],
            cmd: Some(vec!["-g".to_string(), "daemon off;".to_string()]),
            entrypoint: Some(vec!["/docker-entrypoint.sh".to_string()]),
            working_dir: "/app".to_string(),
        };

        inject_init_and_entrypoint_services(
            &temp_dir,
            Some(&cfg),
            Some(&["CUSTOM_KEY=custom_value".to_string()]),
            None,
            &mut file_map,
        )
        .unwrap();

        // Verify entrypoint script was generated
        assert!(file_map.contains_key("usr/local/bin/hostable-entrypoint.sh"));
        assert!(
            temp_dir
                .join("usr/local/bin/hostable-entrypoint.sh")
                .exists()
        );
        let script =
            fs::read_to_string(temp_dir.join("usr/local/bin/hostable-entrypoint.sh")).unwrap();
        assert!(script.contains("cd '/app'"));
        assert!(script.contains("/docker-entrypoint.sh"));

        // Verify systemd service and symlink was generated
        assert!(file_map.contains_key("etc/systemd/system/hostable-app.service"));
        assert!(
            file_map
                .contains_key("etc/systemd/system/multi-user.target.wants/hostable-app.service")
        );
        assert!(
            temp_dir
                .join("etc/systemd/system/hostable-app.service")
                .exists()
        );

        // Verify openrc service and symlink was generated
        assert!(file_map.contains_key("etc/init.d/hostable-app"));
        assert!(file_map.contains_key("etc/runlevels/default/hostable-app"));
        assert!(temp_dir.join("etc/init.d/hostable-app").exists());

        // Verify fallback /sbin/init was generated
        assert!(file_map.contains_key("sbin/init"));
        assert!(temp_dir.join("sbin/init").exists());

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn busybox_init_does_not_hide_the_application_and_native_init_is_preserved() {
        for native in ["busybox", "systemd", "openrc"] {
            let root = fs::canonicalize(std::env::temp_dir())
                .unwrap()
                .join(crate::runtime::id("hostable_init_selection"));
            let _cleanup = TestDirectoryGuard(root.clone());
            let mut files = HashMap::new();
            add_injected_file(&root, "sbin/init", b"original init", 0o755, &mut files).unwrap();
            files.get_mut("sbin/init").unwrap().link_name = Some(
                if native == "systemd" {
                    "/lib/systemd/systemd"
                } else {
                    "/bin/busybox"
                }
                .into(),
            );
            if native == "systemd" {
                add_injected_file(
                    &root,
                    "lib/systemd/systemd",
                    b"native systemd",
                    0o755,
                    &mut files,
                )
                .unwrap();
            }
            if native == "openrc" {
                add_injected_file(
                    &root,
                    "etc/inittab",
                    b"::sysinit:/sbin/openrc sysinit",
                    0o644,
                    &mut files,
                )
                .unwrap();
                add_injected_file(
                    &root,
                    "sbin/openrc-run",
                    b"native openrc",
                    0o755,
                    &mut files,
                )
                .unwrap();
            }
            inject_init_and_entrypoint_services(&root, None, None, None, &mut files).unwrap();
            let init = fs::read_to_string(root.join("sbin/init")).unwrap();
            if native == "busybox" {
                assert!(init.contains("setsid /usr/local/bin/hostable-entrypoint.sh"));
                assert!(init.contains("trap shutdown TERM INT HUP PWR"));
                assert_eq!(files["sbin/init"].entry_type, b'0');
                assert!(files["sbin/init"].link_name.is_none());
            } else {
                assert_eq!(init, "original init");
            }
        }
    }

    #[test]
    fn absolute_image_links_are_rerooted_without_parent_escape() {
        let parent = vec!["var".to_string()];
        assert_eq!(
            link_components(&parent, Path::new("/usr/bin")),
            Some(vec!["usr".into(), "bin".into()])
        );
        assert_eq!(
            link_components(&parent, Path::new("../run")),
            Some(vec!["run".into()])
        );
        assert!(link_components(&parent, Path::new("../../outside")).is_none());
        assert!(link_components(&parent, Path::new("/../outside")).is_none());
        #[cfg(windows)]
        assert!(link_components(&parent, Path::new("C:/outside")).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn injected_init_replaces_physical_metadata_in_merged_usr_images() {
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(crate::runtime::id("hostable_merged_usr"));
        let _cleanup = TestDirectoryGuard(root.clone());
        fs::create_dir_all(root.join("usr/sbin")).unwrap();
        std::os::unix::fs::symlink("/usr/sbin", root.join("sbin")).unwrap();
        let mut files = HashMap::new();
        add_injected_file(&root, "usr/sbin/init", b"old", 0o755, &mut files).unwrap();
        files.get_mut("usr/sbin/init").unwrap().link_name = Some("/bin/busybox".into());
        files.insert("sbin/init".into(), files["usr/sbin/init"].clone());
        inject_init_and_entrypoint_services(&root, None, None, None, &mut files).unwrap();
        assert!(!files.contains_key("sbin/init"));
        assert_eq!(files["usr/sbin/init"].entry_type, b'0');
        assert!(files["usr/sbin/init"].link_name.is_none());
        assert!(
            fs::read_to_string(root.join("usr/sbin/init"))
                .unwrap()
                .contains("hostable-entrypoint.sh")
        );
    }

    #[test]
    fn test_prune_stale_cache_files() {
        let temp_dir =
            std::env::temp_dir().join(format!("hostable_prune_test_{}", rand::random::<u32>()));
        fs::create_dir_all(&temp_dir).unwrap();

        let dummy_tmp = temp_dir.join(".layer_abcdef12.tmp");
        fs::write(&dummy_tmp, b"dummy layer data").unwrap();

        let dummy_cache_dir = temp_dir.join("test_container.tar.xz.cache");
        fs::create_dir_all(&dummy_cache_dir).unwrap();
        fs::write(dummy_cache_dir.join("some_file"), b"cache").unwrap();

        let keep_file = temp_dir.join("keep_me.txt");
        fs::write(&keep_file, b"keep this").unwrap();

        assert!(dummy_tmp.exists());
        assert!(dummy_cache_dir.exists());
        assert!(keep_file.exists());

        // Run pruning with max_age_secs = 0 (prunes all matching cache files)
        prune_stale_cache_files(&temp_dir, 0);

        assert!(
            !dummy_tmp.exists(),
            "Temporary layer file should have been pruned"
        );
        assert!(
            !dummy_cache_dir.exists(),
            "Cache directory should have been pruned"
        );
        assert!(keep_file.exists(), "Non-cache file must remain intact");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn test_compress_and_cleanup() {
        let temp_dir =
            std::env::temp_dir().join(format!("hostable_comp_test_{}", rand::random::<u32>()));
        let cache_dir = temp_dir.join("rootfs.cache");
        fs::create_dir_all(&cache_dir).unwrap();

        let hello_file = cache_dir.join("hello.txt");
        fs::write(&hello_file, b"Hello Hostable World").unwrap();

        let out_tar_xz = temp_dir.join("rootfs.tar.xz");
        let mut file_map = HashMap::new();
        file_map.insert(
            "hello.txt".to_string(),
            PosixMetadata {
                entry_type: b'0',
                mode: 0o644,
                uid: 0,
                gid: 0,
                size: 20,
                mtime: 1000,
                link_name: None,
            },
        );

        compress_and_cleanup(cache_dir.clone(), out_tar_xz.clone(), file_map)
            .await
            .unwrap();

        assert!(out_tar_xz.exists(), "Output tar.xz must exist");
        assert!(
            !cache_dir.exists(),
            "Unpacked cache directory must be cleaned up"
        );

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn failed_conversion_preserves_existing_template_and_removes_partial_archive() {
        let root = fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(crate::runtime::id("hostable_atomic_archive"));
        let _cleanup = TestDirectoryGuard(root.clone());
        let cache = root.join("rootfs.cache");
        fs::create_dir_all(&cache).unwrap();
        let output = root.join("rootfs.tar.xz");
        fs::write(&output, b"previous verified template").unwrap();
        let files = HashMap::from([(
            "missing-file".into(),
            PosixMetadata {
                entry_type: b'0',
                mode: 0o644,
                uid: 0,
                gid: 0,
                size: 1,
                mtime: 0,
                link_name: None,
            },
        )]);
        assert!(
            compress_and_cleanup(cache, output.clone(), files)
                .await
                .is_err()
        );
        assert_eq!(fs::read(output).unwrap(), b"previous verified template");
        assert!(
            !fs::read_dir(&root)
                .unwrap()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().contains("archive_tmp"))
        );
    }
}

#[cfg(test)]
mod integrity_tests {
    use super::*;
    #[test]
    fn absent_or_null_image_environment_is_valid() {
        let config: ConfigContainer =
            serde_json::from_value(serde_json::json!({"Env":null,"Cmd":["app",""]})).unwrap();
        assert!(config.env.is_empty());
        assert_eq!(config.cmd.unwrap()[1], "");
    }
    #[test]
    fn detects_content_corruption() {
        use sha2::Digest;
        let expected = format!("sha256:{}", hex::encode(sha2::Sha256::digest(b"verified")));
        assert!(verify_digest(&expected, b"verified").is_ok());
        assert!(verify_digest(&expected, b"changed").is_err());
        assert!(verify_digest("bad", b"verified").is_err());
    }
}
