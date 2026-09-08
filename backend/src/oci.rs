#![allow(dead_code, unused_variables, unused_imports)]
use flate2::read::GzDecoder;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue};
use serde::Deserialize;
use std::fs;
use std::io::{self, Cursor, Read};
use std::path::{Path, PathBuf};
use tar::Archive;

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
    #[serde(rename = "Env", default)]
    pub env: Vec<String>,
    #[serde(rename = "Cmd", default)]
    pub cmd: Option<Vec<String>>,
    #[serde(rename = "Entrypoint", default)]
    pub entrypoint: Option<Vec<String>>,
    #[serde(rename = "WorkingDir", default)]
    pub working_dir: String,
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

        Ok(Self {
            registry,
            repository,
            tag,
        })
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

fn compress_to_tar_xz(
    src_dir: &Path,
    out_file: &Path,
    file_map: &HashMap<String, PosixMetadata>,
) -> io::Result<()> {
    let tar_xz = std::fs::File::create(out_file)?;
    let enc = xz2::write::XzEncoder::new(tar_xz, 6);
    let mut builder = tar::Builder::new(enc);

    // Sort paths to ensure deterministic tar order and parents before children
    let mut paths: Vec<&String> = file_map.keys().collect();
    paths.sort();

    for path_str in paths {
        let meta = &file_map[path_str];
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::new(meta.entry_type));
        header.set_mode(meta.mode);
        header.set_uid(meta.uid);
        header.set_gid(meta.gid);
        header.set_size(meta.size);
        header.set_mtime(meta.mtime);

        if let Some(link) = &meta.link_name {
            if let Err(e) = header.set_link_name(link) {
                tracing::warn!("Failed to set link name {}: {}", link, e);
            }
        }

        header.set_cksum();

        let etype = tar::EntryType::new(meta.entry_type);
        if etype.is_file() {
            let real_path = src_dir.join(path_str);
            if let Ok(mut f) = std::fs::File::open(&real_path) {
                builder.append_data(&mut header, path_str, &mut f)?;
            } else {
                builder.append_data(&mut header, path_str, std::io::empty())?;
            }
        } else {
            // Directories, symlinks, etc. don't have file body bytes
            header.set_size(0);
            builder.append_data(&mut header, path_str, std::io::empty())?;
        }
    }

    builder.into_inner()?.finish()?;
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
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::default())
                .build()
                .unwrap(),
        }
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

        // 2. Fetch the token from the authentication server
        let mut token_url = format!("{}?scope=repository:{}:pull", realm, img.repository);
        if let Some(srv) = service {
            token_url.push_str(&format!("&service={}", srv));
        }

        tracing::info!("Fetching pull token from: {}...", token_url);
        let token_res = self.client.get(&token_url).send().await?;

        if !token_res.status().is_success() {
            return Err(format!("Failed to retrieve token: {}", token_res.status()).into());
        }

        let payload: TokenResponse = token_res.json().await?;
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

        let manifest_response: ManifestResponse = res.json().await?;

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
                    &target.digest[..12]
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

                let single: SingleManifest = res.json().await?;
                single
            }
        };

        let new_layers: Vec<String> = manifest.layers.iter().map(|l| l.digest.clone()).collect();
        tracing::info!(
            "Found {} layers in resolved image manifest.",
            manifest.layers.len()
        );

        let cache_dir = PathBuf::from(format!("{}.cache", target_path.display()));
        let meta_file = PathBuf::from(format!("{}.cache.meta", target_path.display()));

        let mut needs_recreate = true;
        let mut layers_to_download = new_layers.clone();
        let mut start_layer_index = 0;
        let mut file_map: HashMap<String, PosixMetadata> = HashMap::new();

        if cache_dir.exists() && meta_file.exists() {
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
                        layers_to_download = new_layers[start_layer_index..].to_vec();
                        file_map = state.files;
                    }
                }
            }
        }

        if layers_to_download.is_empty() {
            tracing::info!("Image hasn't changed. Using existing cache...");
            if !target_path.exists() {
                tracing::info!("Compressing rootfs to {}...", target_path.display());

                if let Some(envs) = env_vars {
                    if !envs.is_empty() {
                        let profile_dir = cache_dir.join("etc").join("profile.d");
                        let _ = fs::create_dir_all(&profile_dir);
                        let mut content = String::from("#!/bin/sh\n");
                        for e in envs {
                            content.push_str(&format!("export {}\n", e));
                        }
                        let _ = fs::write(profile_dir.join("hostable-env.sh"), &content);
                        let meta = PosixMetadata {
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
                        file_map.insert("etc/profile.d/hostable-env.sh".to_string(), meta);
                    }
                }

                if let Some(files) = &extra_files {
                    for (path_str, content) in files {
                        let full_path = cache_dir.join(path_str);
                        if let Some(p) = full_path.parent() {
                            let _ = fs::create_dir_all(p);
                        }
                        let _ = fs::write(&full_path, content);
                        let meta = PosixMetadata {
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
                        file_map.insert(path_str.trim_start_matches('/').to_string(), meta);
                    }
                }

                compress_to_tar_xz(&cache_dir, target_path, &file_map)?;
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
            fs::create_dir_all(&cache_dir)?;
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
                &layer.digest[..12]
            );

            let blob_url = format!(
                "https://{}/v2/{}/blobs/{}",
                img.registry, img.repository, layer.digest
            );

            let blob_res = self
                .client
                .get(&blob_url)
                .headers(headers.clone())
                .send()
                .await?;

            if !blob_res.status().is_success() {
                return Err(format!("Failed to fetch layer blob: {}", blob_res.status()).into());
            }

            // Stream response bytes directly into memory buffer to avoid writing temporary files
            let bytes = blob_res.bytes().await?;
            let cursor = Cursor::new(bytes);

            // Extract the tar.gz stream directly
            extract_layer_stream(cursor, &cache_dir, &mut file_map)?;
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

        if let Some(envs) = env_vars {
            if !envs.is_empty() {
                let profile_dir = cache_dir.join("etc").join("profile.d");
                let _ = fs::create_dir_all(&profile_dir);
                let mut content = String::from("#!/bin/sh\n");
                for e in envs {
                    content.push_str(&format!("export {}\n", e));
                }
                let _ = fs::write(profile_dir.join("hostable-env.sh"), &content);
                let meta = PosixMetadata {
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
                file_map.insert("etc/profile.d/hostable-env.sh".to_string(), meta);
            }
        }

        if let Some(files) = &extra_files {
            for (path_str, content) in files {
                let full_path = cache_dir.join(path_str);
                if let Some(p) = full_path.parent() {
                    let _ = fs::create_dir_all(p);
                }
                let _ = fs::write(&full_path, content);
                let meta = PosixMetadata {
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
                file_map.insert(path_str.trim_start_matches('/').to_string(), meta);
            }
        }

        compress_to_tar_xz(&cache_dir, target_path, &file_map)?;

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

        let config_blob: ImageConfigBlob = res.json().await?;
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
                    &layer.digest[..12]
                );
                let blob_url = format!(
                    "https://{}/v2/{}/blobs/{}",
                    img.registry, img.repository, layer.digest
                );
                let blob_res = self
                    .client
                    .get(&blob_url)
                    .headers(headers.clone())
                    .send()
                    .await?;
                if !blob_res.status().is_success() {
                    return Err(format!("Failed to fetch layer blob: {}", blob_res.status()).into());
                }

                let bytes = blob_res.bytes().await?;
                let cursor = Cursor::new(bytes);
                extract_layer_stream(cursor, &cache_dir, &mut master_file_map)?;
            }
        }

        if let Some(files) = extra_files {
            for (path_str, content) in files {
                let full_path = cache_dir.join(&path_str);
                if let Some(p) = full_path.parent() {
                    let _ = fs::create_dir_all(p);
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
        compress_to_tar_xz(&cache_dir, target_path, &master_file_map)?;

        Ok(UpdateStatus::Recreated)
    }
}

// ==============================================================================
// Layer Extraction and OCI Whiteout Handler
// ==============================================================================

fn extract_layer_stream<R: Read>(
    stream: R,
    target_dir: &Path,
    file_map: &mut HashMap<String, PosixMetadata>,
) -> io::Result<()> {
    let dec = GzDecoder::new(stream);
    let mut archive = Archive::new(dec);

    for entry_res in archive.entries()? {
        let mut entry = entry_res?;
        let path = entry.path()?.to_path_buf();
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

                    let target_parent = target_dir.join(parent);
                    if target_parent.exists() {
                        tracing::debug!(
                            "Handling opaque whiteout: cleaning parent folder {}",
                            parent.display()
                        );
                        fs::remove_dir_all(&target_parent)?;
                        fs::create_dir_all(&target_parent)?;
                    }
                }
                continue;
            }

            // Standard OCI File/Directory Whiteout (.wh.<filename>)
            if file_name.starts_with(".wh.") {
                let target_name = &file_name[4..];
                let parent = path.parent().unwrap_or(Path::new(""));
                let target_path = parent.join(target_name);
                let target_str = target_path.to_string_lossy().replace("\\", "/");
                let prefix = format!("{}/", target_str);

                file_map.retain(|k, _| !(k.starts_with(&prefix) || k == &target_str));

                let path_to_delete = target_dir.join(&target_path);
                if path_to_delete.exists() {
                    tracing::debug!(
                        "Handling whiteout deletion: removing {}",
                        path_to_delete.display()
                    );
                    if path_to_delete.is_dir() {
                        fs::remove_dir_all(&path_to_delete)?;
                    } else {
                        fs::remove_file(&path_to_delete)?;
                    }
                }
                continue;
            }
        }

        // Store POSIX Metadata
        let header = entry.header();
        let meta = PosixMetadata {
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
        file_map.insert(path_str.clone(), meta);

        // Standard OCI File/Directory unpacking to Windows cache
        let out_path = target_dir.join(&path);

        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let entry_type = entry.header().entry_type();
        if entry_type.is_dir() {
            fs::create_dir_all(&out_path)?;
        } else if entry_type.is_hard_link() {
            if let Some(link_name) = entry.header().link_name()? {
                let source_path = target_dir.join(&*link_name);
                if source_path.exists() {
                    if let Err(_) = fs::hard_link(&source_path, &out_path) {
                        if source_path.is_file() {
                            let _ = fs::copy(&source_path, &out_path);
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
            if let Err(_) = entry.unpack(&out_path) {
                // Ignore platform metadata errors
            }
        }
    }

    Ok(())
}

// ==============================================================================
// Unit Tests
// ==============================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
}
