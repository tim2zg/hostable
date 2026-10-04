use crate::{AppState, RequireAuth, runtime, workloads::Workload};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures::{StreamExt, stream};
use serde_json::{Value, json};
use std::sync::Arc;

async fn collect(s: &Arc<AppState>, w: Workload) -> Value {
    let mut report = json!({"id":w.id,"kind":"container","name":w.name,"vmid":w.active.vmid,"observed_at":runtime::now(),"status":"unknown","alerts":[],"guest_commands":crate::guest::available(&w.node)});
    let mut alerts = Vec::<String>::new();
    match crate::workloads::owned(s, &w.node, w.active.vmid, &w.active.owner).await {
        Ok(_) => match s.proxmox.get_lxc_status(&w.node, w.active.vmid).await {
            Ok(status) => {
                report["metrics"] = status["data"].clone();
                report["status"] = status["data"]["status"].clone();
                for (used, max, label) in [
                    ("disk", "maxdisk", "Root disk"),
                    ("mem", "maxmem", "Memory"),
                ] {
                    if let (Some(used), Some(max)) =
                        (status["data"][used].as_f64(), status["data"][max].as_f64())
                    {
                        if max > 0.0 && used / max >= 0.9 {
                            alerts.push(format!("{} is above 90% capacity", label));
                        }
                    }
                }
                if status["data"]["status"] == "running" {
                    if let Some(ip) = &w.ip {
                        match crate::workloads::check_health(ip, &w.health).await {
                            Ok(()) => {
                                report["application_health"] = json!(if w.health.port.is_some() {
                                    "healthy"
                                } else {
                                    "not_configured"
                                })
                            }
                            Err(e) => {
                                report["application_health"] = json!("unhealthy");
                                alerts.push(e);
                            }
                        }
                    }
                }
            }
            Err(e) => alerts.push(e),
        },
        Err(e) => {
            report["status"] = json!("ownership_error");
            alerts.push(e);
        }
    }
    if w.operation.is_some() {
        alerts.push("An update is active or needs recovery".into());
    }
    if crate::guest::available(&w.node) && w.operation.is_none() && w.status == "ready" {
        match crate::guest::exec(s, &w.node, w.active.vmid, &w.active.owner, "df -Pk\n", 30).await {
            Ok(usage) => {
                for line in usage.lines().skip(1) {
                    let fields: Vec<_> = line.split_whitespace().collect();
                    if fields
                        .get(4)
                        .and_then(|v| v.strip_suffix('%'))
                        .and_then(|v| v.parse::<u32>().ok())
                        .is_some_and(|p| p >= 90)
                    {
                        alerts.push(format!(
                            "Filesystem {} is above 90% capacity",
                            fields.get(5).unwrap_or(&"unknown")
                        ));
                    }
                }
                report["filesystem_usage"] = json!(usage);
            }
            Err(e) => alerts.push(format!("Filesystem inspection: {}", e)),
        }
    }
    if let Some(e) = w.last_update_error {
        alerts.push(format!("Last update: {}", e));
    }
    if let Ok(p) = crate::workloads::request(&w.active.spec) {
        if let Some(id) = p.database_id {
            if let Ok(Some(app)) = s.db.as_ref().unwrap().get_record("app_database", &id).await {
                if app["credentials_rotated_at"]
                    .as_u64()
                    .is_some_and(|t| t > w.active.created_at)
                {
                    alerts.push("Attached database credentials changed. Use Refresh attached database credentials in Applications and updates.".into());
                }
            }
        }
    }
    report["alerts"] = json!(alerts);
    report
}
pub async fn collect_all(s: &Arc<AppState>) -> Result<Vec<Value>, String> {
    let db = s.db.as_ref().ok_or("Metadata unavailable")?;
    let workloads = db
        .list_records("workload")
        .await?
        .into_iter()
        .filter_map(|v| serde_json::from_value::<Workload>(v).ok())
        .collect::<Vec<_>>();
    let mut reports = stream::iter(workloads)
        .map(|w| collect(s, w))
        .buffer_unordered(4)
        .collect::<Vec<_>>()
        .await;
    for instance in db.list_records("database").await? {
        if let Some(id) = instance["id"].as_str() {
            reports.push(crate::databases::health_report(s,id).await.unwrap_or_else(|e|json!({"id":id,"kind":"database","name":instance["name"],"status":"unreachable","observed_at":runtime::now(),"alerts":[e]})));
        }
    }
    for r in &reports {
        db.put_record(
            "health",
            r["id"].as_str().ok_or("Invalid health resource")?,
            r,
        )
        .await?;
    }
    Ok(reports)
}
pub async fn list(
    _auth: RequireAuth,
    State(s): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    Ok(Json(json!(
        s.db.as_ref()
            .unwrap()
            .list_records("health")
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
    )))
}
pub async fn refresh(
    _auth: RequireAuth,
    State(s): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    Ok(Json(json!(
        collect_all(&s)
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, e))?
    )))
}
pub async fn logs(
    _auth: RequireAuth,
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Response, (StatusCode, String)> {
    let w = crate::workloads::load(&s, &id)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    let script = "set -eu\nif command -v journalctl >/dev/null; then journalctl -u hostable-app -n 200 --no-pager; elif [ -f /var/log/hostable-application.log ]; then tail -n 200 /var/log/hostable-application.log; else echo 'No captured application logs. Configure the recipe to write to /var/log/hostable-application.log.'; fi\n";
    let mut output = crate::guest::exec(&s, &w.node, w.active.vmid, &w.active.owner, script, 30)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    if let Ok(p) = crate::workloads::request(&w.active.spec) {
        for value in p.env_vars.values().filter(|v| v.len() >= 4) {
            output = output.replace(value, "[redacted]");
        }
        if let Some(id) = p.database_id {
            if let Ok(uri) = crate::databases::application_uri(&s, &id).await {
                if let Ok(url) = reqwest::Url::parse(&uri) {
                    if let Some(password) = url.password() {
                        output = output.replace(password, "[redacted]");
                    }
                }
                output = output.replace(&uri, "[redacted database URI]");
            }
        }
    }
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(json!({"output":output,"source":"guest application logs","limit_bytes":65536})),
    )
        .into_response())
}
pub fn start_scheduler(s: Arc<AppState>) {
    let health = s.clone();
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            timer.tick().await;
            if let Err(e) = collect_all(&health).await {
                tracing::warn!("Health collection failed: {}", e);
            }
        }
    });
    tokio::spawn(async move {
        // Update scheduling is opt-in per workload, with a failure backoff.
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            timer.tick().await; // Scheduling is implemented through reviewed recipe policies in workloads::schedule.
            if let Err(e) = crate::workloads::schedule(&s).await {
                tracing::warn!("Update scheduling: {}", e);
            }
        }
    });
}
