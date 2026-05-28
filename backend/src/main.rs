use clap::{Parser, Subcommand};
use std::fs;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use axum::{
    routing::{get, post, any},
    Router,
    Json,
    body::Body,
    http::{Request, Response, StatusCode, request::Parts},
    extract::{ws::{WebSocketUpgrade, WebSocket, Message}, FromRequestParts, FromRef, State, Path as AxumPath},
};
use tokio::time::{interval, Duration};
use std::time::SystemTime;
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use std::env;
use std::net::SocketAddr;
use tracing_subscriber;
use rand::distr::{Alphanumeric, SampleString};
use std::sync::Arc;

mod proxmox;
mod converter;
mod oci;

pub struct AppState {
    pub proxmox: proxmox::ProxmoxClient,
    pub pool: Option<sqlx::PgPool>,
}

use sqlx::Row;

#[derive(Parser)]
#[command(name = "hostable")]
#[command(about = "Hostable: Native Proxmox Container Manager and Edge Router", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Launches the Hostable Web Application and Reverse Proxy Router
    Start,
    
    /// Converts a Dockerfile to distrobuilder YAML format
    Convert {
        /// Path to the input Dockerfile
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,

        /// Optional path to save the generated Distrobuilder YAML file
        #[arg(short, long, value_name = "OUT")]
        out: Option<PathBuf>,
    },
    
    /// Deploys a Dockerfile directly to Proxmox (simulation)
    Deploy {
        /// Path to the input Dockerfile
        #[arg(short, long, value_name = "FILE")]
        file: PathBuf,
    },
    
    /// Pulls and decompresses a Docker registry OCI image into a native LXC template
    PullDocker {
        /// Docker image reference (e.g. alpine:latest or linuxserver/jellyfin)
        #[arg(short, long, value_name = "IMAGE")]
        image: String,

        /// Output path for the generated .tar.xz file
        #[arg(short, long, value_name = "OUT")]
        out: PathBuf,
    },
    
    /// Checks for a Docker image update and applies it to a specific Proxmox LXC
    UpdateLxc {
        /// Docker image reference (e.g. alpine:latest)
        #[arg(short, long, value_name = "IMAGE")]
        image: String,

        /// The target Proxmox VMID to update
        #[arg(short, long, value_name = "VMID")]
        vmid: u32,
        
        /// Output path for the extracted rootfs .tar.xz file
        #[arg(short, long, value_name = "OUT")]
        out: PathBuf,
    },
    
    /// Runs the daemon to continuously update deployments based on a config file
    Manage {
        /// Path to the config.yaml file
        #[arg(short, long, value_name = "CONFIG")]
        config: PathBuf,
    }
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

    match &cli.command {
        Commands::Start => {
            let database_url = env::var("DATABASE_URL").unwrap_or_else(|_| "postgres://postgres:postgres@localhost/hostable".to_string());
            
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
                        
                    println!("\n=======================================================");
                    println!("🚀 HOSTABLE INITIALIZED!");
                    println!("🔐 Your Admin API Token is: {}", token_str);
                    println!("Please save this token. You will need it to log in.");
                    println!("=======================================================\n");
                }
            } else {
                println!("\n=======================================================");
                println!("🚀 HOSTABLE INITIALIZED IN MOCK MODE!");
                println!("🔐 Your Mock API Token is: hostable_mock_token");
                println!("=======================================================\n");
            }

            let shared_state = Arc::new(AppState {
                proxmox: proxmox::ProxmoxClient::new(),
                pool: _pool,
            });

            let app = Router::new()
                .route("/api/verify", get(verify_token))
                .route("/api/health", get(health_check))
                .route("/api/stats", get(get_stats))
                .route("/api/lxcs", get(get_lxcs))
                .route("/api/proxy-rules", get(get_proxy_rules).post(add_proxy_rule))
                .route("/api/convert", post(converter::convert_dockerfile_endpoint))
                .route("/api/deploy", post(converter::deploy_lxc_endpoint))
                .route("/api/db/execute", post(db_execute))
                .route("/api/db/schema", get(db_schema))
                .route("/api/ws/logs/{vmid}", get(ws_logs_handler))
                .route("/api/catalog", get(get_catalog))
                .fallback(any(proxy_handler))
                .with_state(shared_state);

            let port = env::var("PORT").unwrap_or_else(|_| "3000".to_string());
            let addr: SocketAddr = format!("0.0.0.0:{}", port).parse().unwrap();
            tracing::info!("Listening on {}", addr);
            
            let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
            axum::serve(listener, app).await.unwrap();
        },
        
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
        },

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
        },

        Commands::PullDocker { image, out } => {
            println!("Pulling OCI/Docker image '{}'...", image);
            let extractor = oci::OciExtractor::new();
            match extractor.extract_to_dir(image, out).await {
                Ok(_) => {
                    println!("------------------------------------------------------------");
                    println!("Successfully pulled and compressed to: {}", out.display());
                    println!("------------------------------------------------------------");
                },
                Err(err) => {
                    eprintln!("Error pulling/extracting image: {}", err);
                    std::process::exit(1);
                }
            }
        },

        Commands::UpdateLxc { image, vmid, out } => {
            println!("Updating Proxmox LXC Container {} using image '{}'...", vmid, image);
            let extractor = oci::OciExtractor::new();
            match extractor.extract_to_dir(image, out).await {
                Ok(status) => {
                    let proxmox = proxmox::ProxmoxClient::new();
                    let node = "pve";
                    match status {
                        oci::UpdateStatus::Unchanged => {
                            println!("Image has not changed. No update needed for container {}.", vmid);
                        },
                        oci::UpdateStatus::InPlaceUpdate => {
                            println!("In-place update detected. Restarting container {}...", vmid);
                            let _ = proxmox.stop_lxc(node, *vmid).await;
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            let _ = proxmox.start_lxc(node, *vmid).await;
                        },
                        oci::UpdateStatus::Recreated => {
                            println!("Base image changed. Recreating container {}...", vmid);
                            let _ = proxmox.stop_lxc(node, *vmid).await;
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            let _ = proxmox.delete_lxc(node, *vmid).await;
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            let _ = proxmox.create_lxc(*vmid, &format!("hostable-{}", vmid)).await;
                            tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                            let _ = proxmox.start_lxc(node, *vmid).await;
                        }
                    }
                },
                Err(err) => {
                    eprintln!("Error pulling/extracting image: {}", err);
                    std::process::exit(1);
                }
            }
        },

        Commands::Manage { config } => {
            if !config.exists() {
                eprintln!("Error: Config file '{}' not found", config.display());
                std::process::exit(1);
            }
            println!("Starting Hostable Daemon...");
            let content = fs::read_to_string(config)?;
            let cfg: Config = serde_yaml::from_str(&content).expect("Invalid config.yaml format");
            println!("Loaded {} deployments. Interval: {} seconds.", cfg.deployments.len(), cfg.interval_seconds);

            let proxmox = proxmox::ProxmoxClient::new();
            let node = "pve";

            loop {
                println!("--- Checking for updates ---");
                for dep in &cfg.deployments {
                    println!("Checking deployment for VMID {} (Image: {})", dep.vmid, dep.image);
                    let extractor = oci::OciExtractor::new();
                    let out_path = std::path::PathBuf::from(format!("/cache/hostable_vmid_{}.tar.xz", dep.vmid));
                    match extractor.extract_to_dir(&dep.image, &out_path).await {
                        Ok(status) => {
                            match status {
                                oci::UpdateStatus::Unchanged => {
                                    println!("VMID {}: Image unchanged.", dep.vmid);
                                },
                                oci::UpdateStatus::InPlaceUpdate => {
                                    println!("VMID {}: In-place update detected. Restarting...", dep.vmid);
                                    let _ = proxmox.stop_lxc(node, dep.vmid).await;
                                    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                                    let _ = proxmox.start_lxc(node, dep.vmid).await;
                                },
                                oci::UpdateStatus::Recreated => {
                                    println!("VMID {}: Base layers changed. Recreating...", dep.vmid);
                                    let _ = proxmox.stop_lxc(node, dep.vmid).await;
                                    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                                    let _ = proxmox.delete_lxc(node, dep.vmid).await;
                                    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                                    let _ = proxmox.create_lxc(dep.vmid, &format!("hostable-{}", dep.vmid)).await;
                                    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                                    let _ = proxmox.start_lxc(node, dep.vmid).await;
                                }
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

async fn proxy_handler(State(state): State<Arc<AppState>>, req: Request<Body>) -> Result<Response<Body>, StatusCode> {
    let host_header = req.headers().get("host").and_then(|h| h.to_str().ok()).unwrap_or("");
    let domain = host_header.split(':').next().unwrap_or(host_header);

    if let Some(pool) = &state.pool {
        let rule = sqlx::query("SELECT target_ip, target_port, auth_enabled FROM proxy_rules WHERE domain = $1")
            .bind(domain)
            .fetch_optional(pool)
            .await
            .map_err(|e| {
                tracing::error!("DB error in proxy: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        if let Some(r) = rule {
            let target_ip: String = r.get("target_ip");
            let target_port: i32 = r.get("target_port");
            let auth_enabled: Option<bool> = r.try_get("auth_enabled").unwrap_or(Some(false));
            
            if auth_enabled.unwrap_or(false) {
                let authelia_url = env::var("AUTHELIA_URL").unwrap_or_else(|_| "http://localhost:9091".into());
                let verify_url = format!("{}/api/verify", authelia_url);
                
                let mut auth_req = reqwest::Client::new().get(&verify_url);
                for (name, value) in req.headers() {
                    auth_req = auth_req.header(name, value);
                }
                
                if let Ok(res) = auth_req.send().await {
                    if res.status() == StatusCode::UNAUTHORIZED {
                        let redirect_url = format!("{}/?rd=https://{}", authelia_url, domain);
                        return Ok(Response::builder()
                            .status(StatusCode::FOUND)
                            .header(axum::http::header::LOCATION, redirect_url)
                            .body(Body::empty())
                            .unwrap());
                    } else if !res.status().is_success() {
                        return Err(StatusCode::FORBIDDEN);
                    }
                } else {
                    tracing::error!("Failed to contact Authelia");
                    return Err(StatusCode::INTERNAL_SERVER_ERROR);
                }
            }

            let path_query = req.uri().path_and_query().map(|x| x.as_str()).unwrap_or("/");
            let target_uri = format!("http://{}:{}{}", target_ip, target_port, path_query);
            
            tracing::info!("Proxying request from {} to {}", domain, target_uri);

            let method = req.method().clone();
            let mut request_builder = reqwest::Client::new().request(method, &target_uri);
            
            for (name, value) in req.headers() {
                if name != axum::http::header::HOST {
                    request_builder = request_builder.header(name, value);
                }
            }
            
            let resp = request_builder.send().await.map_err(|e| {
                tracing::error!("Proxy upstream error: {}", e);
                StatusCode::BAD_GATEWAY
            })?;
            
            let mut axum_resp = Response::builder().status(resp.status());
            for (name, value) in resp.headers() {
                axum_resp = axum_resp.header(name, value);
            }
            
            let bytes = resp.bytes().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
            return Ok(axum_resp.body(Body::from(bytes)).unwrap());
        }
    }

    Ok(Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from(format!("Hostable Proxy: No routing rule found for domain '{}'", domain)))
        .unwrap())
}

async fn get_catalog(_auth: RequireAuth) -> Json<Value> {
    let url = "https://api.linuxserver.io/api/v1/images?include_config=false&include_deprecated=false";
    match reqwest::get(url).await {
        Ok(res) => {
            if let Ok(json) = res.json::<Value>().await {
                Json(json)
            } else {
                Json(json!({"status": "error", "error": "Failed to parse catalog JSON"}))
            }
        }
        Err(e) => {
            Json(json!({"status": "error", "error": e.to_string()}))
        }
    }
}

async fn verify_token(_auth: RequireAuth) -> Json<Value> {
    Json(json!({"status": "ok"}))
}

async fn health_check() -> Json<Value> {
    Json(json!({"status": "ok", "message": "Hostable Proxmox Server is running!"}))
}

async fn get_stats(_auth: RequireAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    if let Ok(nodes_data) = state.proxmox.get_nodes().await {
        if let Some(nodes) = nodes_data["data"].as_array() {
            if let Some(first_node) = nodes.first() {
                if let Some(node_name) = first_node["node"].as_str() {
                    if let Ok(status) = state.proxmox.get_node_status(node_name).await {
                        let data = &status["data"];
                        let cpu = data["cpu"].as_f64().unwrap_or(0.0) * 100.0;
                        let mem_used = data["memory"]["used"].as_f64().unwrap_or(0.0);
                        let mem_total = data["memory"]["total"].as_f64().unwrap_or(1.0);
                        let disk_used = data["rootfs"]["used"].as_f64().unwrap_or(0.0);
                        let disk_total = data["rootfs"]["total"].as_f64().unwrap_or(1.0);

                        return Json(json!({
                            "cpu": cpu.round() as u64,
                            "ram": ((mem_used / mem_total) * 100.0).round() as u64,
                            "disk": ((disk_used / disk_total) * 100.0).round() as u64,
                            "activeLxcs": 3
                        }));
                    }
                }
            }
        }
    }

    Json(json!({
        "cpu": 18,
        "ram": 55,
        "disk": 42,
        "activeLxcs": 3
    }))
}

async fn get_lxcs(_auth: RequireAuth, State(state): State<Arc<AppState>>) -> Json<Value> {
    if let Ok(nodes_data) = state.proxmox.get_nodes().await {
        if let Some(nodes) = nodes_data["data"].as_array() {
            if let Some(first_node) = nodes.first() {
                if let Some(node_name) = first_node["node"].as_str() {
                    if let Ok(lxcs) = state.proxmox.get_lxcs(node_name).await {
                        if let Some(lxc_array) = lxcs["data"].as_array() {
                            let mapped: Vec<Value> = lxc_array.iter().map(|lxc| {
                                let id = lxc["vmid"].as_i64().unwrap_or(0);
                                let name = lxc["name"].as_str().unwrap_or("unknown");
                                let status = lxc["status"].as_str().unwrap_or("stopped");
                                let mem = lxc["maxmem"].as_i64().unwrap_or(0) / 1024 / 1024;
                                let cpu = lxc["cpu"].as_f64().unwrap_or(0.0) * 100.0;

                                json!({
                                    "id": id,
                                    "name": name,
                                    "status": status,
                                    "mem": format!("{} MB", mem),
                                    "cpu": format!("{:.1}%", cpu)
                                })
                            }).collect();
                            return Json(json!(mapped));
                        }
                    }
                }
            }
        }
    }

    Json(json!([
        { "id": 101, "name": "nginx-proxy", "status": "running", "mem": "128 MB", "cpu": "1.0%" },
        { "id": 102, "name": "postgres-db", "status": "stopped", "mem": "0 MB", "cpu": "0.0%" },
        { "id": 103, "name": "pihole", "status": "running", "mem": "84 MB", "cpu": "0.5%" }
    ]))
}

#[derive(Serialize, Deserialize)]
struct ProxyRule {
    id: Option<i32>,
    domain: String,
    target_ip: String,
    target_port: i32,
    container_id: Option<i32>,
    auth_enabled: Option<bool>,
}

async fn get_proxy_rules(_auth: RequireAuth, State(state): State<Arc<AppState>>) -> Json<Vec<ProxyRule>> {
    if let Some(pool) = &state.pool {
        let rules = sqlx::query("SELECT id, domain, target_ip, target_port, container_id, auth_enabled FROM proxy_rules")
            .fetch_all(pool)
            .await;
        if let Ok(r_list) = rules {
            let mapped: Vec<ProxyRule> = r_list.iter().map(|r| {
                ProxyRule {
                    id: Some(r.get("id")),
                    domain: r.get("domain"),
                    target_ip: r.get("target_ip"),
                    target_port: r.get("target_port"),
                    container_id: r.get("container_id"),
                    auth_enabled: r.try_get("auth_enabled").ok(),
                }
            }).collect();
            return Json(mapped);
        }
    }

    Json(vec![
        ProxyRule { id: Some(1), domain: "pihole.local".into(), target_ip: "10.0.0.5".into(), target_port: 80, container_id: Some(103), auth_enabled: Some(false) },
        ProxyRule { id: Some(2), domain: "jellyfin.local".into(), target_ip: "10.0.0.6".into(), target_port: 8096, container_id: Some(104), auth_enabled: Some(true) },
    ])
}

async fn add_proxy_rule(_auth: RequireAuth, State(state): State<Arc<AppState>>, Json(payload): Json<ProxyRule>) -> Json<Value> {
    if let Some(pool) = &state.pool {
        let _ = sqlx::query("INSERT INTO proxy_rules (domain, target_ip, target_port, container_id, auth_enabled) VALUES ($1, $2, $3, $4, $5)")
            .bind(&payload.domain)
            .bind(&payload.target_ip)
            .bind(payload.target_port)
            .bind(payload.container_id)
            .bind(payload.auth_enabled.unwrap_or(false))
            .execute(pool)
            .await;
    }
    Json(json!({"status": "ok"}))
}

async fn ws_logs_handler(_auth: RequireAuth, AxumPath(vmid): AxumPath<String>, ws: WebSocketUpgrade) -> axum::response::Response {
    ws.on_upgrade(move |socket| handle_log_socket(socket, vmid))
}

async fn handle_log_socket(mut socket: WebSocket, vmid: String) {
    let mut ticker = interval(Duration::from_secs(1));
    let mut iterations = 0;
    loop {
        ticker.tick().await;
        iterations += 1;
        if iterations > 300 {
            let _ = socket.send(Message::Text("[Hostable] Connection auto-closed: Session limit of 5 minutes reached to save server resources.".into())).await;
            break;
        }
        let timestamp = SystemTime::now();
        let msg = format!("[{}] VM {} log line", timestamp.elapsed().unwrap_or_default().as_secs(), vmid);
        if socket.send(Message::Text(msg.into())).await.is_err() {
            break;
        }
    }
}

#[derive(Deserialize)]
struct DbQuery {
    sql: String,
}

async fn db_execute(_auth: RequireAuth, State(state): State<Arc<AppState>>, Json(payload): Json<DbQuery>) -> Json<serde_json::Value> {
    let sql_lower = payload.sql.to_lowercase();
    let is_modifying = sql_lower.contains("drop") 
        || sql_lower.contains("truncate") 
        || sql_lower.contains("delete") 
        || sql_lower.contains("alter") 
        || sql_lower.contains("update")
        || sql_lower.contains("insert");
    let targets_system = sql_lower.contains("users") || sql_lower.contains("proxy_rules");

    if is_modifying && targets_system {
        return Json(serde_json::json!({
            "status": "error",
            "error": "Security Error: Modifications to core Hostable system tables (users, proxy_rules) are prohibited through the DB Provisioning tool."
        }));
    }

    if let Some(pool) = &state.pool {
        match sqlx::query(&payload.sql).execute(pool).await {
            Ok(res) => Json(serde_json::json!({"status": "ok", "rows_affected": res.rows_affected()})),
            Err(e) => Json(serde_json::json!({"status": "error", "error": e.to_string()})),
        }
    } else {
        Json(serde_json::json!({"status": "mock", "message": "No DB connection in mock mode"}))
    }
}

async fn db_schema(_auth: RequireAuth, State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    if let Some(pool) = &state.pool {
        let rows = sqlx::query("SELECT tablename FROM pg_tables WHERE schemaname='public'")
            .fetch_all(pool)
            .await;
        match rows {
            Ok(r) => {
                let tables: Vec<String> = r.iter().map(|row| row.get::<String, _>("tablename")).collect();
                Json(serde_json::json!({"status": "ok", "tables": tables}))
            }
            Err(e) => Json(serde_json::json!({"status": "error", "error": e.to_string()})),
        }
    } else {
        Json(serde_json::json!({"status": "mock", "tables": ["users", "proxy_rules", "example"]}))
    }
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
        
        let auth_header = parts.headers.get(axum::http::header::AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "));
            
        let query = parts.uri.query().unwrap_or("");
        let query_token = query.split('&').find(|s| s.starts_with("token=")).map(|s| &s[6..]);
            
        let token = if let Some(t) = auth_header {
            t
        } else if let Some(t) = query_token {
            t
        } else {
            return Err((StatusCode::UNAUTHORIZED, "Missing or invalid Authorization header"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;

    #[tokio::test]
    async fn test_db_execute_security_guards() {
        let state = Arc::new(AppState {
            proxmox: proxmox::ProxmoxClient::new(),
            pool: None,
        });

        let query = DbQuery {
            sql: "CREATE TABLE selfhost_app (id SERIAL PRIMARY KEY, name VARCHAR(255))".to_string(),
        };
        let Json(res) = db_execute(RequireAuth, State(state.clone()), Json(query)).await;
        assert_eq!(res["status"], "mock");

        let query_danger = DbQuery {
            sql: "DROP TABLE users".to_string(),
        };
        let Json(res_danger) = db_execute(RequireAuth, State(state.clone()), Json(query_danger)).await;
        assert_eq!(res_danger["status"], "error");
        assert!(res_danger["error"].as_str().unwrap().contains("prohibited"));

        let query_danger_2 = DbQuery {
            sql: "ALTER TABLE proxy_rules ADD COLUMN hack VARCHAR(100)".to_string(),
        };
        let Json(res_danger_2) = db_execute(RequireAuth, State(state.clone()), Json(query_danger_2)).await;
        assert_eq!(res_danger_2["status"], "error");
    }

    #[tokio::test]
    async fn test_db_schema_mock() {
        let state = Arc::new(AppState {
            proxmox: proxmox::ProxmoxClient::new(),
            pool: None,
        });

        let Json(res) = db_schema(RequireAuth, State(state)).await;
        assert_eq!(res["status"], "mock");
        assert!(res["tables"].as_array().is_some());
        let tables: Vec<String> = res["tables"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t.as_str().unwrap().to_string())
            .collect();
        assert!(tables.contains(&"users".to_string()));
        assert!(tables.contains(&"proxy_rules".to_string()));
    }
}
