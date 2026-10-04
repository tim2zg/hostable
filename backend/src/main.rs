use axum::{
    Json, Router,
    body::Body,
    extract::{FromRef, FromRequestParts, State},
    http::{Method, Request, StatusCode, request::Parts},
    response::IntoResponse,
    routing::{any, delete, get, post},
};
use clap::{Parser, Subcommand};
use rust_embed::RustEmbed;
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber;

pub mod ansible;
mod converter;
mod databases;
pub mod db;
mod guest;
mod jobs;
mod monitoring;
mod oci;
mod proxmox;
mod recipes;
mod recovery;
mod routes;
mod runtime;
pub mod secureweb;
mod updates;
mod workloads;

#[derive(RustEmbed)]
#[folder = "../frontend/dist/"]
struct Assets;

pub struct AppState {
    pub proxmox: proxmox::ProxmoxClient,
    pub db: Option<Arc<db::DbBackend>>,
    pub mock_token: Option<String>,
    pub catalog_cache: tokio::sync::RwLock<Option<(std::time::Instant, serde_json::Value)>>,
    pub default_node: String,
    pub ansible: Arc<ansible::AnsibleEngine>,
    pub secureweb: Arc<secureweb::SecureWebClient>,
    pub deployment_lock: Arc<tokio::sync::Semaphore>,
    pub manager_updates: updates::ManagerUpdates,
}

#[derive(Parser)]
#[command(name = "hostable")]
#[command(version = updates::VERSION)]
#[command(about = "Hostable: Native Proxmox Container Manager and Edge Router", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Start,
    ResetAdminToken,
    PlanUpdate {
        #[arg(long)]
        workload: String,
        #[arg(long)]
        image: Option<String>,
        #[arg(long, default_value = "image")]
        method: String,
        #[arg(long)]
        backup_storage: Option<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    ApplyUpdate {
        #[arg(long)]
        workload: String,
        #[arg(long)]
        plan: String,
    },
    Job {
        #[arg(long)]
        id: String,
    },
    RecoverJob {
        #[arg(long)]
        id: String,
        #[arg(long)]
        action: String,
    },
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    tracing_subscriber::fmt::init();
    dotenvy::dotenv().ok();
    dotenvy::from_path("/etc/hostable/.env").ok();

    match &cli.command {
        Commands::Start => {
            let database_url = env::var("DATABASE_URL").ok();
            let db_res = db::DbBackend::init(database_url).await;

            let (backend, token) = db_res.map_err(std::io::Error::other)?;
            backend
                .migrate_metadata()
                .await
                .map_err(std::io::Error::other)?;
            if token.starts_with("hst_") {
                let path = save_admin_token(&token).map_err(std::io::Error::other)?;
                println!("Admin API token saved to {}", path.display());
            }
            let db_backend = Some(Arc::new(backend));
            let mock_token = None;

            let proxmox_client = proxmox::ProxmoxClient::new();
            let default_node = proxmox_client.get_default_node().await;
            let ansible_engine = Arc::new(
                ansible::AnsibleEngine::new(
                    db_backend.clone().expect("Metadata database initialized"),
                )
                .await
                .map_err(std::io::Error::other)?,
            );
            let secureweb_client = Arc::new(secureweb::SecureWebClient::new(
                db_backend.clone().expect("Metadata initialized"),
            ));
            let deployment_lock = Arc::new(tokio::sync::Semaphore::new(1));

            // Startup cache hygiene: prune stale temporary artifacts older than 1 hour
            let cache_path = std::path::PathBuf::from("/cache");
            if cache_path.exists() {
                oci::prune_stale_cache_files(&cache_path, 3600);
            } else {
                let local_cache = std::path::PathBuf::from("./cache");
                oci::prune_stale_cache_files(&local_cache, 3600);
            }

            let shared_state = Arc::new(AppState {
                proxmox: proxmox_client,
                db: db_backend,
                mock_token,
                catalog_cache: tokio::sync::RwLock::new(None),
                default_node,
                ansible: ansible_engine,
                secureweb: secureweb_client,
                deployment_lock,
                manager_updates: Default::default(),
            });

            // Cross-origin access is opt-in via HOSTABLE_ALLOWED_ORIGINS (comma
            // separated). By default only same-origin requests (the embedded SPA)
            // are permitted.
            let allowed_origins: Vec<axum::http::HeaderValue> =
                env::var("HOSTABLE_ALLOWED_ORIGINS")
                    .unwrap_or_default()
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .filter_map(|s| s.parse().ok())
                    .collect();

            let cors = if allowed_origins.is_empty() {
                CorsLayer::new()
            } else {
                CorsLayer::new()
                    .allow_origin(allowed_origins)
                    .allow_methods([
                        Method::GET,
                        Method::POST,
                        Method::PUT,
                        Method::DELETE,
                        Method::OPTIONS,
                    ])
                    .allow_headers([
                        axum::http::header::AUTHORIZATION,
                        axum::http::header::CONTENT_TYPE,
                    ])
            };

            updates::initialize(&shared_state)
                .await
                .map_err(std::io::Error::other)?;
            updates::start_scheduler(shared_state.clone());
            databases::reconcile(&shared_state)
                .await
                .map_err(std::io::Error::other)?;
            databases::start_scheduler(shared_state.clone());
            workloads::reconcile(&shared_state)
                .await
                .map_err(std::io::Error::other)?;
            monitoring::start_scheduler(shared_state.clone());
            recovery::start_scheduler(shared_state.clone());
            let app = Router::new()
                .route("/api/manager/updates", get(updates::status))
                .route("/api/manager/updates/check", post(updates::check_now))
                .route("/api/manager/updates/policy", axum::routing::put(updates::save_policy))
                .route("/api/manager/updates/install", post(updates::install))
                .route("/api/jobs", get(jobs::list_jobs))
                .route("/api/jobs/{id}", get(jobs::get_job))
                .route("/api/jobs/{id}/cancel", post(jobs::cancel_job))
                .route("/api/jobs/{id}/inspect", post(workloads::inspect))
                .route("/api/jobs/{id}/recover", post(workloads::recover))
                .route("/api/workloads", get(workloads::list))
                .route("/api/workloads/{id}", get(workloads::get))
                .route("/api/workloads/{id}/policy", axum::routing::put(workloads::policy))
                .route("/api/workloads/{id}/plan", post(workloads::plan))
                .route("/api/workloads/{id}/apply", post(workloads::apply))
                .route("/api/workloads/{id}/logs", get(monitoring::logs))
                .route("/api/monitoring", get(monitoring::list))
                .route("/api/monitoring/refresh", post(monitoring::refresh))
                .route("/api/recipes", get(recipes::list).post(recipes::import))
                .route("/api/recipes/{id}/{version}", get(recipes::get))
                .route("/api/recovery", get(recovery::status))
                .route("/api/recovery/policy", axum::routing::put(recovery::policy))
                .route("/api/databases/backups/{id}/verify", post(recovery::verify))
                .route("/api/databases/{id}/certificate/renew", post(databases::renew_certificate))
                .route("/api/databases", get(databases::list).post(databases::create))
                .route("/api/databases/{id}", get(databases::get_instance))
                .route("/api/databases/{id}/retire", post(databases::retire))
                .route("/api/databases/apps/{id}/revoke", post(databases::revoke))
                .route("/api/databases/{id}/access", axum::routing::put(databases::update_access))
                .route("/api/databases/{id}/apps", get(databases::list_apps).post(databases::create_app))
                .route("/api/databases/apps/{id}/credentials", post(databases::credentials))
                .route("/api/databases/apps/{id}/rotate", post(databases::rotate))
                .route("/api/databases/apps/{id}/backups", get(databases::list_backups).post(databases::backup))
                .route("/api/databases/backups/{id}/restore", post(databases::restore))
                .route("/api/node/templates", get(databases::templates))
                .route("/api/nodes", get(get_nodes_handler))

                .route("/api/verify", get(verify_token))
                .route("/api/health", get(health_check))
                .route("/api/ready", get(readiness))
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
                .route(
                    "/api/node/default-network",
                    get(get_default_network_handler),
                )
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
                .layer(axum::middleware::from_fn_with_state(shared_state.clone(), updates::guard_mutations))
                .layer(cors)
                .layer(TraceLayer::new_for_http().make_span_with(
                    |req: &Request<Body>| {
                        // Log only the path: query strings may carry ?token= values.
                        tracing::info_span!("http_request", method = %req.method(), path = %req.uri().path())
                    },
                ))
                .with_state(shared_state);

            let port = env::var("PORT").unwrap_or_else(|_| "3000".to_string());
            let addr: SocketAddr = format!(
                "{}:{}",
                env::var("HOSTABLE_BIND").unwrap_or_else(|_| "0.0.0.0".into()),
                port
            )
            .parse()
            .unwrap();
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
            let params: ansible::AnsibleDeployParams=serde_json::from_slice(&fs::read(file)?).map_err(|_|std::io::Error::other("deploy --file expects a JSON deployment specification containing image, hostname, storage and network. Build Dockerfiles into an OCI image first."))?;
            ansible::validate_deploy_params(&params).map_err(std::io::Error::other)?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &manager_request(Method::POST, "/ansible/deploy", Some(json!(params))).await?
                )?
            );
        }

        Commands::PullDocker { image, out } => {
            println!("Pulling OCI/Docker image '{}'...", image);
            let extractor = oci::OciExtractor::new()?;
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

        Commands::UpdateLxc { .. } | Commands::Manage { .. } => {
            return Err(std::io::Error::other("Use Applications and updates or hostable plan-update / apply-update. Legacy destructive recreation is disabled; the managed workflow retains previous root disks and transfers persistent mounts.").into());
        }
        Commands::PlanUpdate {
            workload,
            image,
            method,
            backup_storage,
            out,
        } => {
            if !ansible::identifier(workload) {
                return Err(std::io::Error::other("Invalid workload ID").into());
            }
            let plan = manager_request(
                Method::POST,
                &format!("/workloads/{}/plan", workload),
                Some(json!({"mode":method,"image":image,"backup_storage":backup_storage})),
            )
            .await?;
            let data = serde_json::to_string_pretty(&plan)?;
            if let Some(path) = out {
                let path = if path.is_absolute() {
                    path.clone()
                } else {
                    env::current_dir()?.join(path)
                };
                runtime::write_private_file(&path, data.as_bytes())
                    .map_err(std::io::Error::other)?;
                println!(
                    "Update preview saved to {}. Plan {}",
                    path.display(),
                    plan["plan"]["id"]
                );
            } else {
                println!("{}", data);
            }
        }
        Commands::ApplyUpdate { workload, plan } => {
            if !ansible::identifier(workload) || !ansible::identifier(plan) {
                return Err(std::io::Error::other("Invalid workload/plan reference").into());
            }
            println!(
                "{}",
                manager_request(
                    Method::POST,
                    &format!("/workloads/{}/apply", workload),
                    Some(json!({"plan_id":plan}))
                )
                .await?
            );
        }
        Commands::Job { id } => {
            if !ansible::identifier(id) {
                return Err(std::io::Error::other("Invalid job ID").into());
            }
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &manager_request(Method::GET, &format!("/jobs/{}", id), None).await?
                )?
            );
        }
        Commands::RecoverJob { id, action } => {
            if !ansible::identifier(id) {
                return Err(std::io::Error::other("Invalid job ID").into());
            }
            let path = if action == "inspect" {
                format!("/jobs/{}/inspect", id)
            } else if action == "cancel" {
                format!("/jobs/{}/cancel", id)
            } else {
                format!("/jobs/{}/recover", id)
            };
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &manager_request(Method::POST, &path, Some(json!({"action":action}))).await?
                )?
            );
        }
        Commands::ResetAdminToken => {
            let (db, _) = db::DbBackend::init(env::var("DATABASE_URL").ok())
                .await
                .map_err(std::io::Error::other)?;
            let token = db
                .reset_admin_token()
                .await
                .map_err(std::io::Error::other)?;
            let path = save_admin_token(&token).map_err(std::io::Error::other)?;
            println!(
                "New admin token saved to {}. The previous token was revoked.",
                path.display()
            );
        }
    }
    Ok(())
}

async fn manager_request(
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, Box<dyn std::error::Error>> {
    let base = env::var("HOSTABLE_MANAGER_URL").unwrap_or_else(|_| {
        format!(
            "http://127.0.0.1:{}/api",
            env::var("PORT").unwrap_or("3000".into())
        )
    });
    let url = reqwest::Url::parse(&base)?;
    if !(url.scheme() == "https"
        || url.scheme() == "http"
            && matches!(
                url.host_str(),
                Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
            ))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(std::io::Error::other("Manager URL must use HTTPS or loopback HTTP and cannot contain credentials, a query or fragment").into());
    }
    let token = env::var("HOSTABLE_API_TOKEN")
        .or_else(|_| fs::read_to_string(runtime::data_dir().join("admin-token")))?;
    let mut client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none());
    if let Some(path) = env::var_os("HOSTABLE_MANAGER_CA_CERT") {
        client = client.add_root_certificate(reqwest::Certificate::from_pem(&fs::read(path)?)?);
    }
    let mut request = client
        .build()?
        .request(method, format!("{}{}", base.trim_end_matches('/'), path))
        .bearer_auth(token.trim());
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    let data: Value = response.json().await?;
    if !status.is_success() {
        return Err(std::io::Error::other(format!("Manager returned {}: {}", status, data)).into());
    }
    Ok(data)
}

/// Persist a first-boot admin token to a root-only file instead of leaving it
/// in world-readable service logs.
fn save_admin_token(token: &str) -> Result<PathBuf, String> {
    let path = runtime::data_dir().join("admin-token");
    runtime::write_secret(&path, format!("{}\n", token).as_bytes())?;
    Ok(path)
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn fallback_handler(req: Request<Body>) -> axum::response::Response {
    if req.uri().path().starts_with("/api/") {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"status":"error","error":"Unknown API route"})),
        )
            .into_response();
    }
    static_handler(req.uri().clone()).await
}

#[derive(serde::Deserialize)]
pub struct NodeQuery {
    pub node: Option<String>,
}
fn selected_node(state: &AppState, query: NodeQuery) -> Result<String, (StatusCode, String)> {
    let node = query.node.unwrap_or_else(|| state.default_node.clone());
    if !ansible::identifier(&node) {
        return Err((StatusCode::BAD_REQUEST, "Invalid node".into()));
    }
    Ok(node)
}
pub async fn get_nodes_handler(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state
        .proxmox
        .get_nodes()
        .await
        .map(|mut value| {
            value["default_node"] = json!(state.default_node);
            Json(value)
        })
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))
}
pub async fn get_bridges_handler(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<NodeQuery>,
) -> Result<Json<Vec<String>>, (StatusCode, String)> {
    let node = selected_node(&state, query)?;
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

pub async fn get_default_network_handler(
    _auth: RequireAuth,
) -> Result<Json<Value>, (StatusCode, String)> {
    let iface = env::var("HOSTABLE_DEFAULT_NETWORK").unwrap_or_else(|_| "eth0".to_string());
    let bridge = env::var("HOSTABLE_DEFAULT_BRIDGE").unwrap_or_else(|_| "vmbr0".to_string());
    let subnet = env::var("HOSTABLE_DEFAULT_SUBNET").ok();
    Ok(Json(json!({
        "default_interface": iface,
        "default_bridge": bridge,
        "default_subnet": subnet
    })))
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
    Json(
        json!({"status": "ok", "version": updates::VERSION, "commit":updates::COMMIT, "message": "Hostable Proxmox Server is running!"}),
    )
}

pub async fn get_storages_handler(
    _auth: RequireAuth,
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<NodeQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let node = selected_node(&state, query)?;
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

        // Query-string tokens are honored only on WebSocket routes (browsers cannot
        // set an Authorization header on WS upgrades). Everywhere else the Bearer
        // header is required, keeping tokens out of URLs and access logs.
        let query_token = if parts.uri.path().starts_with("/api/ws/") {
            parts
                .uri
                .query()
                .unwrap_or("")
                .split('&')
                .find(|s| s.starts_with("token="))
                .map(|s| &s[6..])
        } else {
            None
        };

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

        if let Some(db) = &app_state.db {
            match db.verify_token(token).await {
                Ok(valid) if valid => return Ok(RequireAuth),
                Ok(_) => return Err((StatusCode::UNAUTHORIZED, "Invalid API token")),
                Err(e) => {
                    tracing::error!("Database authentication error: {}", e);
                    return Err((StatusCode::INTERNAL_SERVER_ERROR, "Database error"));
                }
            }
        } else if let Some(mock_token) = &app_state.mock_token {
            if constant_time_eq(token, mock_token) {
                return Ok(RequireAuth);
            }
        }

        Err((StatusCode::UNAUTHORIZED, "Invalid API token"))
    }
}

async fn readiness(_auth: RequireAuth, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let proxmox = state.proxmox.get_nodes().await.is_ok();
    let metadata = state.db.is_some();
    (
        if proxmox && metadata {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(
            json!({"status": if proxmox && metadata {"ready"} else {"degraded"}, "proxmox": proxmox, "metadata": metadata}),
        ),
    )
}
