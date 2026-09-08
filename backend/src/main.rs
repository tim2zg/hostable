use axum::{
    Json, Router,
    body::Body,
    extract::{FromRef, FromRequestParts, State},
    http::{Method, Request, StatusCode, request::Parts},
    response::IntoResponse,
    routing::{any, delete, get, post},
};
use clap::{Parser, Subcommand};
use rand::distr::{Alphanumeric, SampleString};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber;

pub mod ansible;
mod converter;
mod oci;
mod proxmox;
mod routes;
pub mod secureweb;

#[derive(RustEmbed)]
#[folder = "../frontend/dist/"]
struct Assets;

pub struct AppState {
    pub proxmox: proxmox::ProxmoxClient,
    pub pool: Option<sqlx::PgPool>,
    pub catalog_cache: tokio::sync::RwLock<Option<(std::time::Instant, serde_json::Value)>>,
    pub default_node: String,
    pub ansible: Arc<ansible::AnsibleEngine>,
    pub secureweb: Arc<secureweb::SecureWebClient>,
}

#[derive(Parser)]
#[command(name = "hostable")]
#[command(about = "Hostable: Native Proxmox Container Manager and Edge Router", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Start,
    Convert {
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,
        #[arg(short, long, value_name = "OUT")]
        out: Option<PathBuf>,
    },
    Deploy {
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,
    },
    PullDocker {
        #[arg(short, long, value_name = "IMAGE")]
        image: String,
        #[arg(short, long, value_name = "OUT")]
        out: PathBuf,
    },
    UpdateLxc {
        #[arg(short, long, value_name = "IMAGE")]
        image: String,
        #[arg(short, long, value_name = "VMID")]
        vmid: u32,
        #[arg(short, long, value_name = "OUT")]
        out: PathBuf,
    },
    Manage {
        #[arg(short, long, value_name = "CONFIG")]
        config: PathBuf,
    },
}

#[derive(Serialize, Deserialize, Debug)]
struct Deployment {
    image: String,
    vmid: u32,
}

#[derive(Serialize, Deserialize, Debug)]
struct Config {
    interval_seconds: u64,
    deployments: Vec<Deployment>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    tracing_subscriber::fmt::init();
    dotenvy::dotenv().ok();
    dotenvy::from_path("/etc/hostable/.env").ok();

    match &cli.command {
        Commands::Start => {
            let database_url = env::var("DATABASE_URL")
                .unwrap_or_else(|_| "postgres://postgres:postgres@localhost/hostable".to_string());

            tracing::info!("Connecting to database...");
            let _pool = PgPoolOptions::new()
                .max_connections(5)
                .connect(&database_url)
                .await
                .ok();

            if let Some(pool) = &_pool {
                tracing::info!("Running database migrations...");
                if let Err(e) = sqlx::migrate!().run(pool).await {
                    tracing::error!("Failed to run migrations: {}", e);
                }

                let count: (i64,) = sqlx::query_as("SELECT count(*) FROM users")
                    .fetch_one(pool)
                    .await
                    .unwrap_or((0,));

                if count.0 == 0 {
                    let token = Alphanumeric.sample_string(&mut rand::rng(), 32);
                    let token_str = format!("hst_{}", token);

                    sqlx::query("INSERT INTO users (username, api_token) VALUES ($1, $2)")
                        .bind("admin")
                        .bind(&token_str)
                        .execute(pool)
                        .await
                        .unwrap();
                }

                let row: (String,) =
                    sqlx::query_as("SELECT api_token FROM users WHERE username = 'admin'")
                        .fetch_one(pool)
                        .await
                        .unwrap_or(("unknown_token".to_string(),));

                println!("\n=======================================================");
                println!("🚀 HOSTABLE INITIALIZED!");
                println!("🔐 Your Admin API Token is: {}", row.0);
                println!("Please save this token. You will need it to log in.");
                println!("=======================================================\n");
            } else {
                println!("\n=======================================================");
                println!("🚀 HOSTABLE INITIALIZED IN MOCK MODE!");
                println!("🔐 Your Mock API Token is: hostable_mock_token");
                println!("=======================================================\n");
            }

            let proxmox_client = proxmox::ProxmoxClient::new();
            let default_node = proxmox_client.get_default_node().await;
            let ansible_engine = Arc::new(ansible::AnsibleEngine::new());
            let secureweb_client = Arc::new(secureweb::SecureWebClient::new());

            let shared_state = Arc::new(AppState {
                proxmox: proxmox_client,
                pool: _pool,
                catalog_cache: tokio::sync::RwLock::new(None),
                default_node,
                ansible: ansible_engine,
                secureweb: secureweb_client,
            });

            let cors = CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([
                    Method::GET,
                    Method::POST,
                    Method::PUT,
                    Method::DELETE,
                    Method::OPTIONS,
                ])
                .allow_headers(Any);

            let app = Router::new()
                .route("/api/verify", get(verify_token))
                .route("/api/health", get(health_check))
                .route("/api/stats", get(get_stats))
                .route("/api/lxcs", get(routes::lxc::get_lxcs))
                .route(
                    "/api/lxcs/{vmid}/start",
                    post(routes::lxc::start_lxc_handler),
                )
                .route("/api/lxcs/{vmid}/stop", post(routes::lxc::stop_lxc_handler))
                .route(
                    "/api/lxc/{vmid}/start",
                    post(routes::lxc::start_lxc_handler),
                )
                .route("/api/lxc/{vmid}/stop", post(routes::lxc::stop_lxc_handler))
                .route(
                    "/api/lxcs/{vmid}/restart",
                    post(routes::lxc::restart_lxc_handler),
                )
                .route(
                    "/api/lxcs/{vmid}/rrddata",
                    get(routes::lxc::rrddata_lxc_handler),
                )
                .route(
                    "/api/lxcs/{vmid}/snapshots",
                    get(routes::lxc::get_snapshots_handler)
                        .post(routes::lxc::create_snapshot_handler),
                )
                .route(
                    "/api/lxcs/{vmid}/snapshots/{snapname}/rollback",
                    post(routes::lxc::rollback_snapshot_handler),
                )
                .route(
                    "/api/lxcs/{vmid}/snapshots/{snapname}",
                    delete(routes::lxc::delete_snapshot_handler),
                )
                .route("/api/node/rrddata", get(rrddata_node_handler))
                .route("/api/node/storages", get(get_storages_handler))
                .route("/api/node/bridges", get(get_bridges_handler))
                .route("/api/node/next-vmid", get(get_next_vmid_handler))
                .route("/api/ansible/deploy", post(ansible::trigger_deploy_handler))
                .route(
                    "/api/ansible/tasks/{task_id}/events",
                    get(ansible::get_task_events_handler),
                )
                .route(
                    "/api/ws/tasks/{task_id}",
                    get(ansible::ws_task_stream_handler),
                )
                .route("/api/secureweb/status", get(secureweb::get_status))
                .route(
                    "/api/secureweb/routes",
                    get(secureweb::get_routes).post(secureweb::add_route),
                )
                .route(
                    "/api/secureweb/routes/{domain}",
                    delete(secureweb::delete_route),
                )
                .route("/api/convert", post(converter::convert_dockerfile_endpoint))
                .route("/api/deploy", post(converter::deploy_lxc_endpoint))
                .route(
                    "/api/lxc/stack/deploy",
                    post(converter::deploy_stack_endpoint),
                )
                .route("/api/ws/logs/{vmid}", get(routes::lxc::ws_logs_handler))
                .route("/api/catalog", get(routes::catalog::get_catalog))
                .fallback(any(fallback_handler))
                .layer(cors)
                .layer(TraceLayer::new_for_http())
                .with_state(shared_state);

            let port = env::var("PORT").unwrap_or_else(|_| "3000".to_string());
            let addr: SocketAddr = format!("0.0.0.0:{}", port).parse().unwrap();
            tracing::info!("Listening on {}", addr);

            let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
            axum::serve(listener, app).await.unwrap();
        }

        Commands::Convert { file, out } => {
            if !file.exists() {
                eprintln!("Error: Dockerfile '{}' not found", file.display());
                std::process::exit(1);
            }
            let content = fs::read_to_string(file)?;
            let yaml = converter::convert_dockerfile_to_distrobuilder(&content);
            if let Some(out_path) = out {
                fs::write(out_path, yaml)?;
                println!("Successfully converted and saved to {}", out_path.display());
            } else {
                println!("--- Generated Distrobuilder YAML ---");
                println!("{}", yaml);
            }
        }

        Commands::Deploy { file } => {
            if !file.exists() {
                eprintln!("Error: Dockerfile '{}' not found", file.display());
                std::process::exit(1);
            }
            println!("Deploying {} to Proxmox...", file.display());
            let content = fs::read_to_string(file)?;
            let _yaml = converter::convert_dockerfile_to_distrobuilder(&content);
            println!("Successfully generated YAML, simulated deploy.");
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            println!("Deployed successfully! (Simulation)");
        }

        Commands::PullDocker { image, out } => {
            println!("Pulling OCI/Docker image '{}'...", image);
            let extractor = oci::OciExtractor::new();
            match extractor.extract_to_dir(image, out, None, None).await {
                Ok(_) => {
                    println!("------------------------------------------------------------");
                    println!("Successfully pulled and compressed to: {}", out.display());
                    println!("------------------------------------------------------------");
                }
                Err(err) => {
                    eprintln!("Error pulling/extracting image: {}", err);
                    std::process::exit(1);
                }
            }
        }

        Commands::UpdateLxc { image, vmid, out } => {
            println!(
                "Updating Proxmox LXC Container {} using image '{}'...",
                vmid, image
            );
            let extractor = oci::OciExtractor::new();
            match extractor.extract_to_dir(image, out, None, None).await {
                Ok(status) => {
                    let proxmox = proxmox::ProxmoxClient::new();
                    let node = proxmox.get_default_node().await;
                    match status {
                        oci::UpdateStatus::Unchanged => {
                            println!(
                                "Image has not changed. No update needed for container {}.",
                                vmid
                            );
                        }
                        oci::UpdateStatus::InPlaceUpdate => {
                            println!("In-place update detected. Restarting container {}...", vmid);
                            let _ = proxmox.stop_lxc(&node, *vmid).await;
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            let _ = proxmox.start_lxc(&node, *vmid).await;
                        }
                        oci::UpdateStatus::Recreated => {
                            println!("Base image changed. Recreating container {}...", vmid);
                            let _ = proxmox.stop_lxc(&node, *vmid).await;
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            let _ = proxmox.delete_lxc(&node, *vmid).await;
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            let mut params = std::collections::HashMap::new();
                            params.insert("vmid".to_string(), vmid.to_string());
                            params.insert(
                                "ostemplate".to_string(),
                                format!("local:vztmpl/hostable_vmid_{}.tar.xz", vmid),
                            );
                            params.insert("hostname".to_string(), format!("hostable-{}", vmid));
                            params.insert("memory".to_string(), "512".to_string());
                            params.insert(
                                "net0".to_string(),
                                "name=eth0,bridge=vmbr0,ip=dhcp".to_string(),
                            );
                            params.insert("storage".to_string(), "local-lvm".to_string());
                            params.insert("rootfs".to_string(), "local-lvm:8".to_string());
                            params.insert("tags".to_string(), "hostable".to_string());
                            let _ = proxmox.create_lxc(&node, *vmid, params).await;
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            let _ = proxmox.start_lxc(&node, *vmid).await;
                        }
                    }
                }
                Err(err) => {
                    eprintln!("Error pulling/extracting image: {}", err);
                    std::process::exit(1);
                }
            }
        }

        Commands::Manage { config } => {
            if !config.exists() {
                eprintln!("Error: Config file '{}' not found", config.display());
                std::process::exit(1);
            }
            println!("Starting Hostable Daemon...");
            let content = fs::read_to_string(config)?;
            let cfg: Config = serde_yaml::from_str(&content).expect("Invalid config.yaml format");
            println!(
                "Loaded {} deployments. Interval: {} seconds.",
                cfg.deployments.len(),
                cfg.interval_seconds
            );

            let proxmox = proxmox::ProxmoxClient::new();
            let node = proxmox.get_default_node().await;

            loop {
                println!("--- Checking for updates ---");
                for dep in &cfg.deployments {
                    println!(
                        "Checking deployment for VMID {} (Image: {})",
                        dep.vmid, dep.image
                    );
                    let extractor = oci::OciExtractor::new();
                    let out_path = std::path::PathBuf::from(format!(
                        "/cache/hostable_vmid_{}.tar.xz",
                        dep.vmid
                    ));
                    match extractor
                        .extract_to_dir(&dep.image, &out_path, None, None)
                        .await
                    {
                        Ok(status) => match status {
                            oci::UpdateStatus::Unchanged => {
                                println!("VMID {}: Image unchanged.", dep.vmid);
                            }
                            oci::UpdateStatus::InPlaceUpdate => {
                                println!(
                                    "VMID {}: In-place update detected. Restarting...",
                                    dep.vmid
                                );
                                let _ = proxmox.stop_lxc(&node, dep.vmid).await;
                                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                                let _ = proxmox.start_lxc(&node, dep.vmid).await;
                            }
                            oci::UpdateStatus::Recreated => {
                                println!("VMID {}: Base layers changed. Recreating...", dep.vmid);
                                let _ = proxmox.stop_lxc(&node, dep.vmid).await;
                                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                                let _ = proxmox.delete_lxc(&node, dep.vmid).await;
                                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                                let mut params = std::collections::HashMap::new();
                                params.insert("vmid".to_string(), dep.vmid.to_string());
                                params.insert(
                                    "ostemplate".to_string(),
                                    format!("local:vztmpl/hostable_vmid_{}.tar.xz", dep.vmid),
                                );
                                params.insert(
                                    "hostname".to_string(),
                                    format!("hostable-{}", dep.vmid),
                                );
                                params.insert("memory".to_string(), "512".to_string());
                                params.insert(
                                    "net0".to_string(),
                                    "name=eth0,bridge=vmbr0,ip=dhcp".to_string(),
                                );
                                params.insert("storage".to_string(), "local-lvm".to_string());
                                params.insert("rootfs".to_string(), "local-lvm:8".to_string());
                                params.insert("tags".to_string(), "hostable".to_string());
                                let _ = proxmox.create_lxc(&node, dep.vmid, params).await;
                                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                                let _ = proxmox.start_lxc(&node, dep.vmid).await;
                            }
                        },
                        Err(e) => eprintln!("Error checking update for VMID {}: {}", dep.vmid, e),
                    }
                }
                println!("--- Sleep for {} seconds ---", cfg.interval_seconds);
                tokio::time::sleep(tokio::time::Duration::from_secs(cfg.interval_seconds)).await;
            }
        }
    }
    Ok(())
}

async fn fallback_handler(req: Request<Body>) -> axum::response::Response {
    static_handler(req.uri().clone()).await
}

pub async fn get_bridges_handler(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<String>>, (StatusCode, String)> {
    let node = state.default_node.clone();
    match state.proxmox.get_network_bridges(&node).await {
        Ok(bridges) => Ok(Json(bridges)),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

pub async fn get_next_vmid_handler(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    match state.proxmox.get_next_vmid().await {
        Ok(vmid) => Ok(Json(json!({ "next_vmid": vmid }))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

async fn static_handler(uri: axum::http::Uri) -> axum::response::Response {
    let mut path = uri.path().trim_start_matches('/').to_string();

    if path.is_empty() {
        path = "index.html".to_string();
    }

    match Assets::get(&path) {
        Some(content) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            (
                [(axum::http::header::CONTENT_TYPE, mime.as_ref())],
                content.data,
            )
                .into_response()
        }
        None => {
            if path.starts_with("api/") {
                return (StatusCode::NOT_FOUND, "API route not found").into_response();
            }
            match Assets::get("index.html") {
                Some(content) => {
                    let mime = mime_guess::from_path("index.html").first_or_octet_stream();
                    (
                        [(axum::http::header::CONTENT_TYPE, mime.as_ref())],
                        content.data,
                    )
                        .into_response()
                }
                None => (StatusCode::NOT_FOUND, "Hostable UI not bundled").into_response(),
            }
        }
    }
}

async fn verify_token(_auth: RequireAuth) -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn health_check() -> Json<Value> {
    Json(json!({"status": "ok", "message": "Hostable Proxmox Server is running!"}))
}

pub async fn get_storages_handler(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let node = state.default_node.clone();
    match state.proxmox.get_storages(&node).await {
        Ok(data) => Ok(Json(data)),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

pub async fn rrddata_node_handler(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let node = state.default_node.clone();
    match state.proxmox.get_rrddata(&node, None, "hour").await {
        Ok(data) => Ok(Json(data)),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e)),
    }
}

pub async fn get_stats(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let resources = state.proxmox.get_cluster_resources().await.map_err(|e| {
        tracing::error!("Failed to fetch cluster resources: {}", e);
        (StatusCode::INTERNAL_SERVER_ERROR, e)
    })?;

    let mut total_cpu = 0.0;
    let mut total_mem_used = 0.0;
    let mut total_mem_max = 0.0;
    let mut total_disk_used = 0.0;
    let mut total_disk_max = 0.0;
    let mut active_lxcs = 0;
    let mut node_count = 0.0;

    if let Some(data) = resources["data"].as_array() {
        for item in data {
            if item["type"] == "node" {
                total_cpu += item["cpu"].as_f64().unwrap_or(0.0);
                total_mem_used += item["mem"].as_f64().unwrap_or(0.0);
                total_mem_max += item["maxmem"].as_f64().unwrap_or(1.0);
                total_disk_used += item["disk"].as_f64().unwrap_or(0.0);
                total_disk_max += item["maxdisk"].as_f64().unwrap_or(1.0);
                node_count += 1.0;
            } else if item["type"] == "lxc" || item["type"] == "qemu" {
                if item["status"] == "running" {
                    active_lxcs += 1;
                }
            }
        }
    }

    if node_count == 0.0 {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "No nodes found in cluster".to_string(),
        ));
    }

    Ok(Json(json!({
        "cpu": ((total_cpu / node_count) * 100.0).round() as u64,
        "ram": ((total_mem_used / total_mem_max) * 100.0).round() as u64,
        "disk": ((total_disk_used / total_disk_max) * 100.0).round() as u64,
        "activeLxcs": active_lxcs
    })))
}

pub struct RequireAuth;

impl<S> FromRequestParts<S> for RequireAuth
where
    S: Send + Sync,
    Arc<AppState>: FromRef<S>,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = Arc::<AppState>::from_ref(state);

        let auth_header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "));

        let query = parts.uri.query().unwrap_or("");
        let query_token = query
            .split('&')
            .find(|s| s.starts_with("token="))
            .map(|s| &s[6..]);

        let token = if let Some(t) = auth_header {
            t
        } else if let Some(t) = query_token {
            t
        } else {
            return Err((
                StatusCode::UNAUTHORIZED,
                "Missing or invalid Authorization header",
            ));
        };

        if let Some(pool) = &app_state.pool {
            let user = sqlx::query("SELECT id FROM users WHERE api_token = $1")
                .bind(token)
                .fetch_optional(pool)
                .await
                .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database error"))?;

            if user.is_some() {
                return Ok(RequireAuth);
            }
        } else {
            if token == "hostable_mock_token" {
                return Ok(RequireAuth);
            }
        }

        Err((StatusCode::UNAUTHORIZED, "Invalid API token"))
    }
}
