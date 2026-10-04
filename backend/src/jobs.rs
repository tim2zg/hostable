use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{RwLock, broadcast};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEvent {
    pub task_id: String,
    pub sequence: u64,
    pub timestamp: String,
    pub level: String,
    pub step: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub resource: String,
    pub status: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub events: Vec<TaskEvent>,
    pub sequence: u64,
    #[serde(default)]
    pub cancel_requested: bool,
    #[serde(default)]
    pub cancellable: bool,
}

pub struct JobEngine {
    db: Arc<crate::db::DbBackend>,
    jobs: RwLock<HashMap<String, Job>>,
    bus: broadcast::Sender<TaskEvent>,
    locks: tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>,
    maintenance: RwLock<bool>,
}

impl JobEngine {
    pub async fn new(db: Arc<crate::db::DbBackend>) -> Result<Self, String> {
        let (bus, _) = broadcast::channel(500);
        let mut jobs = HashMap::new();
        for value in db.list_records("job").await? {
            let mut job: Job = serde_json::from_value(value).map_err(|e| e.to_string())?;
            if job.status == "running" || job.status == "queued" {
                job.status = "interrupted".into();
                job.updated_at = crate::runtime::now();
                job.sequence += 1;
                job.events.push(TaskEvent { task_id: job.id.clone(), sequence: job.sequence, timestamp: job.updated_at.to_string(), level: "error".into(), step: "INTERRUPTED".into(), message: "Hostable restarted during this operation. Check the resource before retrying.".into() });
                db.put_record("job", &job.id, &json!(job)).await?;
            }
            jobs.insert(job.id.clone(), job);
        }
        Ok(Self {
            db,
            jobs: RwLock::new(jobs),
            bus,
            locks: Default::default(),
            maintenance: RwLock::new(false),
        })
    }

    pub async fn create(&self, kind: &str, resource: &str) -> Result<String, String> {
        let maintenance = self.maintenance.read().await;
        if *maintenance {
            return Err("Manager update in progress; new operations are paused".into());
        }
        let id = crate::runtime::id("task");
        let now = crate::runtime::now();
        let job = Job {
            id: id.clone(),
            kind: kind.into(),
            resource: resource.into(),
            status: "queued".into(),
            created_at: now,
            updated_at: now,
            events: vec![],
            sequence: 0,
            cancel_requested: false,
            cancellable: matches!(kind, "deploy" | "replace" | "shell_update" | "rollback"),
        };
        self.db.put_record("job", &id, &json!(job)).await?;
        self.jobs.write().await.insert(id.clone(), job);
        Ok(id)
    }

    pub async fn begin_maintenance(&self) -> Result<(), String> {
        let mut gate = self.maintenance.write().await;
        if *gate {
            return Err("Manager update already in progress".into());
        }
        if self
            .jobs
            .read()
            .await
            .values()
            .any(|j| matches!(j.status.as_str(), "queued" | "running"))
        {
            return Err("Wait for active operations to finish before updating the manager".into());
        }
        *gate = true;
        Ok(())
    }

    pub async fn end_maintenance(&self) {
        *self.maintenance.write().await = false;
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TaskEvent> {
        self.bus.subscribe()
    }
    pub async fn get_events(&self, id: &str) -> Vec<TaskEvent> {
        self.jobs
            .read()
            .await
            .get(id)
            .map(|j| j.events.clone())
            .unwrap_or_default()
    }
    pub async fn get(&self, id: &str) -> Option<Job> {
        self.jobs.read().await.get(id).cloned()
    }
    pub async fn list(&self) -> Vec<Job> {
        let mut jobs: Vec<_> = self.jobs.read().await.values().cloned().collect();
        jobs.sort_by_key(|j| std::cmp::Reverse(j.created_at));
        jobs.truncate(200);
        jobs
    }

    pub async fn event(
        &self,
        id: &str,
        level: &str,
        step: &str,
        message: &str,
    ) -> Result<(), String> {
        let mut jobs = self.jobs.write().await;
        let job = jobs.get_mut(id).ok_or("Unknown job")?;
        job.sequence += 1;
        job.updated_at = crate::runtime::now();
        job.status = match step {
            "COMPLETE" => "succeeded",
            "FAILED" => "failed",
            "CANCELLED" => "cancelled",
            "INSPECT" | "CANCEL_REQUESTED" => &job.status,
            "INTERRUPTED" => "interrupted",
            "QUEUE" => "queued",
            _ => "running",
        }
        .into();
        let event = TaskEvent {
            task_id: id.into(),
            sequence: job.sequence,
            timestamp: job.updated_at.to_string(),
            level: level.into(),
            step: step.into(),
            message: message.chars().take(2000).collect(),
        };
        job.events.push(event.clone());
        if job.events.len() > 400 {
            job.events.remove(0);
        }
        self.db.put_record("job", id, &json!(job)).await?;
        let _ = self.bus.send(event);
        Ok(())
    }

    pub async fn lock_resource(
        &self,
        resource: &str,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
        let semaphore = self
            .locks
            .lock()
            .await
            .entry(resource.into())
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(1)))
            .clone();
        semaphore
            .try_acquire_owned()
            .map_err(|_| "Another operation is already using this container".into())
    }

    pub async fn check_cancel(&self, id: &str) -> Result<(), String> {
        if self.get(id).await.is_some_and(|j| j.cancel_requested) {
            Err("Cancellation requested at a safe checkpoint".into())
        } else {
            Ok(())
        }
    }

    pub async fn request_cancel(&self, id: &str) -> Result<(), String> {
        {
            let mut jobs = self.jobs.write().await;
            let job = jobs.get_mut(id).ok_or("Unknown job")?;
            if !job.cancellable || !matches!(job.status.as_str(), "queued" | "running") {
                return Err("This operation cannot be cancelled in its current state".into());
            }
            job.cancel_requested = true;
            self.db.put_record("job", id, &json!(job)).await?;
        }
        self.event(id, "warn", "CANCEL_REQUESTED", "Cancellation requested. The current Proxmox task will finish before stopping at a safe checkpoint.").await
    }

    pub async fn finish_error(&self, id: &str, error: &str) {
        let cancelled = error.starts_with("Cancellation requested");
        let _ = self
            .event(
                id,
                "error",
                if cancelled { "CANCELLED" } else { "FAILED" },
                error,
            )
            .await;
    }
}

pub async fn cancel_job(
    _auth: crate::RequireAuth,
    axum::extract::State(state): axum::extract::State<Arc<crate::AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<axum::Json<Value>, (axum::http::StatusCode, String)> {
    state
        .ansible
        .request_cancel(&id)
        .await
        .map_err(|e| (axum::http::StatusCode::CONFLICT, e))?;
    Ok(axum::Json(json!({"status":"cancel_requested"})))
}

pub async fn list_jobs(
    _auth: crate::RequireAuth,
    axum::extract::State(state): axum::extract::State<Arc<crate::AppState>>,
) -> axum::Json<Value> {
    axum::Json(json!(state.ansible.list().await))
}
pub async fn get_job(
    _auth: crate::RequireAuth,
    axum::extract::State(state): axum::extract::State<Arc<crate::AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<axum::Json<Value>, (axum::http::StatusCode, String)> {
    state
        .ansible
        .get(&id)
        .await
        .map(|j| axum::Json(json!(j)))
        .ok_or((axum::http::StatusCode::NOT_FOUND, "Unknown job".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn manager_updates_wait_for_jobs_and_prevent_new_jobs_until_released() {
        let (db, _) = crate::db::DbBackend::init(Some("sqlite::memory:".into()))
            .await
            .unwrap();
        let db = Arc::new(db);
        db.migrate_metadata().await.unwrap();
        let engine = JobEngine::new(db).await.unwrap();
        let id = engine.create("deploy", "web").await.unwrap();
        assert!(engine.begin_maintenance().await.is_err());
        engine
            .event(&id, "info", "COMPLETE", "finished")
            .await
            .unwrap();
        engine.begin_maintenance().await.unwrap();
        assert!(engine.create("backup", "database").await.is_err());
        assert!(engine.begin_maintenance().await.is_err());
        engine.end_maintenance().await;
        assert!(engine.create("backup", "database").await.is_ok());
    }
    #[tokio::test]
    async fn cancellation_and_resource_locks_are_explicit_and_persistent() {
        let (db, _) = crate::db::DbBackend::init(Some("sqlite::memory:".into()))
            .await
            .unwrap();
        let db = Arc::new(db);
        db.migrate_metadata().await.unwrap();
        let engine = JobEngine::new(db.clone()).await.unwrap();
        let guard = engine.lock_resource("lxc:100").await.unwrap();
        assert!(engine.lock_resource("lxc:100").await.is_err());
        assert!(engine.lock_resource("lxc:101").await.is_ok());
        drop(guard);
        assert!(engine.lock_resource("lxc:100").await.is_ok());
        let id = engine.create("deploy", "web").await.unwrap();
        engine.request_cancel(&id).await.unwrap();
        assert!(engine.check_cancel(&id).await.is_err());
        engine
            .finish_error(&id, "Cancellation requested at a safe checkpoint")
            .await;
        assert_eq!(engine.get(&id).await.unwrap().status, "cancelled");
        assert!(engine.request_cancel(&id).await.is_err());
        let restored = JobEngine::new(db).await.unwrap();
        assert_eq!(restored.get(&id).await.unwrap().status, "cancelled");
        assert!(restored.get(&id).await.unwrap().cancel_requested);
    }
    #[tokio::test]
    async fn jobs_persist_and_interrupted_jobs_are_not_successful() {
        let (db, _) = crate::db::DbBackend::init(Some("sqlite::memory:".into()))
            .await
            .unwrap();
        let db = Arc::new(db);
        db.migrate_metadata().await.unwrap();
        let engine = JobEngine::new(db.clone()).await.unwrap();
        let id = engine.create("deploy", "web").await.unwrap();
        engine
            .event(&id, "task", "CREATE", "Creating")
            .await
            .unwrap();
        let restarted = JobEngine::new(db.clone()).await.unwrap();
        assert_eq!(restarted.get(&id).await.unwrap().status, "interrupted");
        let id2 = restarted.create("backup", "db").await.unwrap();
        restarted
            .event(&id2, "ok", "COMPLETE", "Done")
            .await
            .unwrap();
        assert_eq!(
            JobEngine::new(db)
                .await
                .unwrap()
                .get(&id2)
                .await
                .unwrap()
                .status,
            "succeeded"
        );
    }
}
