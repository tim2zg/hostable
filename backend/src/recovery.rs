use crate::{AppState, RequireAuth, runtime};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path as FilePath, PathBuf},
    sync::Arc,
};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct RecoveryPolicy {
    pub interval_hours: u32,
    pub target_instance_id: Option<String>,
}
pub struct Materialized {
    pub path: PathBuf,
    _guard: Option<runtime::CacheGuard>,
}
async fn gpg(args: &[&str], input: &FilePath, output: &FilePath) -> Result<(), String> {
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3600),
        tokio::process::Command::new("gpg")
            .kill_on_drop(true)
            .args(["--batch", "--no-tty"])
            .args(args)
            .arg("--output")
            .arg(output)
            .arg(input)
            .output(),
    )
    .await
    .map_err(|_| "Backup encryption/decryption timed out")?
    .map_err(|e| format!("GPG is required: {}", e))?;
    if !result.status.success() {
        let _ = std::fs::remove_file(output);
        return Err("GPG encryption/decryption failed. Check the configured recipient/keyring; secret-bearing output is suppressed.".into());
    }
    Ok(())
}
pub async fn encrypt(input: &FilePath, output: &FilePath) -> Result<bool, String> {
    let Ok(recipient) = std::env::var("HOSTABLE_BACKUP_GPG_RECIPIENT") else {
        return Ok(false);
    };
    if recipient.is_empty()
        || recipient.starts_with('-')
        || recipient.len() > 256
        || recipient.contains(['\n', '\r', '\0'])
    {
        return Err("Invalid backup encryption recipient".into());
    }
    gpg(&["--encrypt", "--recipient", &recipient], input, output).await?;
    Ok(true)
}
pub async fn materialize(backup: &crate::databases::Backup) -> Result<Materialized, String> {
    if !crate::ansible::identifier(&backup.id) || backup.filename.contains(['/', '\\']) {
        return Err("Invalid backup reference".into());
    }
    let source = crate::databases::backup_root().join(&backup.filename);
    if backup.sha256.is_empty() || crate::databases::hash_file(&source)? != backup.sha256 {
        return Err("Backup integrity check failed; no target was created".into());
    }
    if !backup.encrypted {
        return Ok(Materialized {
            path: source,
            _guard: None,
        });
    }
    let root = runtime::data_dir()
        .join("cache")
        .join(runtime::id("decrypt"));
    runtime::private_dir(&root)?;
    let guard = runtime::CacheGuard(root.clone());
    let out = root.join("restore.dump");
    gpg(&["--decrypt"], &source, &out).await?;
    // The containing directory is private; restrict the plaintext file as well.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    Ok(Materialized {
        path: out,
        _guard: Some(guard),
    })
}
pub async fn status(
    _auth: RequireAuth,
    State(s): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let db = s.db.as_ref().unwrap();
    let p = db
        .get_record("recovery_policy", "default")
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?
        .unwrap_or(json!(RecoveryPolicy::default()));
    Ok(Json(
        json!({"policy":p,"encrypted_backups":std::env::var("HOSTABLE_BACKUP_GPG_RECIPIENT").is_ok_and(|v|!v.is_empty()),"backup_directory":crate::databases::backup_root(),"independent_destination_configured":std::env::var_os("HOSTABLE_BACKUP_DIR").is_some(),"verification":db.list_records("restore_drill").await.map_err(|e|(StatusCode::INTERNAL_SERVER_ERROR,e))?,"retained_targets":db.list_records("drill_target").await.map_err(|e|(StatusCode::INTERNAL_SERVER_ERROR,e))?,"pitr":"Logical backups restore to a backup boundary. WAL archiving and point-in-time recovery require a separate PostgreSQL recovery deployment."}),
    ))
}
pub async fn policy(
    _auth: RequireAuth,
    State(s): State<Arc<AppState>>,
    Json(p): Json<RecoveryPolicy>,
) -> Result<Json<Value>, (StatusCode, String)> {
    if p.interval_hours > 8760 || p.interval_hours > 0 && p.target_instance_id.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Select a separate verification instance and a valid interval".into(),
        ));
    }
    if let Some(id) = &p.target_instance_id {
        crate::databases::validate_verification_target(&s, id)
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    }
    s.db.as_ref()
        .unwrap()
        .put_record("recovery_policy", "default", &json!(p))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(json!(p)))
}
#[derive(Deserialize)]
pub struct VerifyRequest {
    pub target_instance_id: String,
}
pub async fn verify(
    _auth: RequireAuth,
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(r): Json<VerifyRequest>,
) -> Result<Response, (StatusCode, String)> {
    let task = launch(s, &id, &r.target_instance_id)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    Ok((StatusCode::ACCEPTED, Json(json!({"task_id":task}))).into_response())
}
async fn launch(s: Arc<AppState>, id: &str, target: &str) -> Result<String, String> {
    crate::databases::validate_verification_target(&s, target).await?;
    let backup =
        s.db.as_ref()
            .unwrap()
            .get_record("backup", id)
            .await?
            .ok_or("Unknown backup")?;
    if backup["instance_id"] == target {
        return Err("Restore drills require a separate PostgreSQL instance".into());
    }
    let guard = s
        .ansible
        .lock_resource(&format!("restore_drill:{}", id))
        .await?;
    let job = s.ansible.create("restore_drill", id).await?;
    let copy = job.clone();
    let id = id.to_string();
    let target = target.to_string();
    s.db.as_ref().unwrap().put_record("restore_drill",&id,&json!({"backup_id":id,"target_instance_id":target,"task_id":job,"status":"running","started_at":runtime::now()})).await?;
    tokio::spawn(async move {
        let _guard = guard;
        let result = crate::databases::verify_backup(&s, &id, &target, &copy).await;
        let status = if result.is_ok() { "verified" } else { "failed" };
        let error = result.err();
        let _=s.db.as_ref().unwrap().put_record("restore_drill",&id,&json!({"backup_id":id,"target_instance_id":target,"task_id":copy,"status":status,"finished_at":runtime::now(),"error":error})).await;
        if let Some(e) = error {
            s.ansible.finish_error(&copy, &e).await;
        } else {
            let _=s.ansible.event(&copy,"ok","COMPLETE","Restored into a separate instance, checked row counts, and removed only the generated verification database/role").await;
        }
    });
    Ok(job)
}
pub fn start_scheduler(s: Arc<AppState>) {
    tokio::spawn(async move {
        if let Ok(rows) = s.db.as_ref().unwrap().list_records("restore_drill").await {
            for mut row in rows {
                if row["status"] == "running" {
                    row["status"] = json!("interrupted");
                    row["finished_at"] = json!(runtime::now());
                    row["error"] = json!(
                        "Manager restarted during verification. Inspect the retained drill target before retrying."
                    );
                    if let Some(id) = row["backup_id"].as_str() {
                        let _ =
                            s.db.as_ref()
                                .unwrap()
                                .put_record("restore_drill", id, &row)
                                .await;
                    }
                }
            }
        }
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            timer.tick().await;
            let db = s.db.as_ref().unwrap();
            let Ok(Some(p)) = db.get_record("recovery_policy", "default").await else {
                continue;
            };
            let Ok(p) = serde_json::from_value::<RecoveryPolicy>(p) else {
                continue;
            };
            let Some(target) = p.target_instance_id else {
                continue;
            };
            if p.interval_hours == 0 {
                continue;
            }
            let Ok(backups) = db.list_records("backup").await else {
                continue;
            };
            let mut latest = std::collections::HashMap::<String, Value>::new();
            for b in backups {
                let key = b["app_id"].as_str().unwrap_or("").to_string();
                if latest
                    .get(&key)
                    .is_none_or(|old| old["created_at"].as_u64() < b["created_at"].as_u64())
                {
                    latest.insert(key, b);
                }
            }
            for b in latest.into_values() {
                let Some(id) = b["id"].as_str() else { continue };
                if b["instance_id"] == target {
                    continue;
                }
                if let Ok(Some(old)) = db.get_record("restore_drill", id).await {
                    let at = old["finished_at"]
                        .as_u64()
                        .or(old["started_at"].as_u64())
                        .unwrap_or(0);
                    if old["status"] == "running"
                        || at + u64::from(p.interval_hours) * 3600 > runtime::now()
                    {
                        continue;
                    }
                }
                if let Err(e) = launch(s.clone(), id, &target).await {
                    tracing::warn!("Cannot schedule restore verification: {}", e);
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    #[ignore = "Requires a disposable GPG keyring; run explicitly with HOSTABLE_TEST_GPG_RECIPIENT and GNUPGHOME"]
    async fn gpg_backup_encryption_materialization_and_integrity() {
        let recipient =
            std::env::var("HOSTABLE_TEST_GPG_RECIPIENT").expect("Set disposable GPG recipient");
        let root = crate::databases::backup_root();
        runtime::private_dir(&root).unwrap();
        let plain = root.join("fixture.dump");
        runtime::write_secret(&plain, b"sensitive fixture backup bytes").unwrap();
        let encrypted = root.join("fixture.dump.gpg");
        gpg(
            &["--encrypt", "--recipient", &recipient],
            &plain,
            &encrypted,
        )
        .await
        .unwrap();
        let b = crate::databases::Backup {
            id: "backup_fixture".into(),
            app_id: "fixture".into(),
            instance_id: "fixture".into(),
            created_at: 1,
            filename: "fixture.dump.gpg".into(),
            bytes: std::fs::metadata(&encrypted).unwrap().len(),
            status: "ready".into(),
            sha256: crate::databases::hash_file(&encrypted).unwrap(),
            tables: vec![],
            encrypted: true,
        };
        let restored = materialize(&b).await.unwrap();
        let path = restored.path.clone();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"sensitive fixture backup bytes"
        );
        drop(restored);
        assert!(!path.exists());
        runtime::write_secret(&encrypted, b"corrupted archive").unwrap();
        assert!(materialize(&b).await.is_err());
        std::fs::remove_file(plain).unwrap();
        std::fs::remove_file(encrypted).unwrap();
    }
}
