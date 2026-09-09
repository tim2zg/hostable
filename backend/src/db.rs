use rand::distr::{Alphanumeric, SampleString};
use sqlx::postgres::PgPoolOptions;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Pool, Postgres, Sqlite};
use std::fs;
use std::path::Path;

#[derive(Clone)]
pub enum DbBackend {
    Postgres(Pool<Postgres>),
    Sqlite(Pool<Sqlite>),
}

impl DbBackend {
    pub async fn init(database_url: Option<String>) -> Result<(Self, String), String> {
        let (db_url, is_postgres) = match database_url {
            Some(url) if url.starts_with("postgres://") || url.starts_with("postgresql://") => {
                (url, true)
            }
            Some(url) if url.starts_with("sqlite://") => (url, false),
            Some(url) if !url.trim().is_empty() => {
                (format!("sqlite://{}?mode=rwc", url), false)
            }
            _ => {
                let default_path = if Path::new("/etc/hostable").is_dir() {
                    "/etc/hostable/hostable.db"
                } else {
                    "./hostable.db"
                };
                (format!("sqlite://{}?mode=rwc", default_path), false)
            }
        };

        if is_postgres {
            tracing::info!("Initializing PostgreSQL database connection ({})", db_url);
            let pool = PgPoolOptions::new()
                .max_connections(5)
                .connect(&db_url)
                .await
                .map_err(|e| format!("Failed to connect to PostgreSQL: {}", e))?;

            sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS users (
                    id SERIAL PRIMARY KEY,
                    username VARCHAR(255) NOT NULL UNIQUE,
                    api_token VARCHAR(255) NOT NULL UNIQUE,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                )
                "#,
            )
            .execute(&pool)
            .await
            .map_err(|e| format!("Failed to initialize PostgreSQL schema: {}", e))?;

            let admin_token = Self::ensure_admin_token_pg(&pool).await?;
            Ok((DbBackend::Postgres(pool), admin_token))
        } else {
            tracing::info!("Initializing Embedded SQLite database ({})", db_url);
            if let Some(path_str) = db_url.strip_prefix("sqlite://") {
                let clean_path = path_str.split('?').next().unwrap_or(path_str);
                if let Some(parent) = Path::new(clean_path).parent() {
                    if !parent.as_os_str().is_empty() && !parent.exists() {
                        let _ = fs::create_dir_all(parent);
                    }
                }
            }

            let pool = SqlitePoolOptions::new()
                .max_connections(5)
                .connect(&db_url)
                .await
                .map_err(|e| format!("Failed to connect to SQLite: {}", e))?;

            sqlx::query(
                r#"
                CREATE TABLE IF NOT EXISTS users (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    username TEXT NOT NULL UNIQUE,
                    api_token TEXT NOT NULL UNIQUE,
                    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
                )
                "#,
            )
            .execute(&pool)
            .await
            .map_err(|e| format!("Failed to initialize SQLite schema: {}", e))?;

            let admin_token = Self::ensure_admin_token_sqlite(&pool).await?;
            Ok((DbBackend::Sqlite(pool), admin_token))
        }
    }

    async fn ensure_admin_token_pg(pool: &Pool<Postgres>) -> Result<String, String> {
        let existing: Option<(String,)> =
            sqlx::query_as("SELECT api_token FROM users WHERE username = 'admin'")
                .fetch_optional(pool)
                .await
                .map_err(|e| e.to_string())?;

        if let Some((token,)) = existing {
            return Ok(token);
        }

        let token = format!("hst_{}", Alphanumeric.sample_string(&mut rand::rng(), 32));
        sqlx::query("INSERT INTO users (username, api_token) VALUES ($1, $2)")
            .bind("admin")
            .bind(&token)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;

        Ok(token)
    }

    async fn ensure_admin_token_sqlite(pool: &Pool<Sqlite>) -> Result<String, String> {
        let existing: Option<(String,)> =
            sqlx::query_as("SELECT api_token FROM users WHERE username = 'admin'")
                .fetch_optional(pool)
                .await
                .map_err(|e| e.to_string())?;

        if let Some((token,)) = existing {
            return Ok(token);
        }

        let token = format!("hst_{}", Alphanumeric.sample_string(&mut rand::rng(), 32));
        sqlx::query("INSERT INTO users (username, api_token) VALUES (?, ?)")
            .bind("admin")
            .bind(&token)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;

        Ok(token)
    }

    pub async fn verify_token(&self, token: &str) -> Result<bool, String> {
        match self {
            DbBackend::Postgres(pool) => {
                let user: Option<(i32,)> =
                    sqlx::query_as("SELECT id FROM users WHERE api_token = $1")
                        .bind(token)
                        .fetch_optional(pool)
                        .await
                        .map_err(|e| e.to_string())?;
                Ok(user.is_some())
            }
            DbBackend::Sqlite(pool) => {
                let user: Option<(i64,)> =
                    sqlx::query_as("SELECT id FROM users WHERE api_token = ?")
                        .bind(token)
                        .fetch_optional(pool)
                        .await
                        .map_err(|e| e.to_string())?;
                Ok(user.is_some())
            }
        }
    }

    pub async fn create_database(&self, name: &str) -> Result<(), String> {
        match self {
            DbBackend::Postgres(pool) => {
                let safe_db = name.replace("\"", "").replace("'", "");
                let q = format!("CREATE DATABASE \"{}\"", safe_db);
                if let Err(e) = sqlx::query(&q).execute(pool).await {
                    tracing::warn!("Postgres database creation notice: {}", e);
                }
                Ok(())
            }
            DbBackend::Sqlite(_) => Ok(()),
        }
    }

    pub fn engine_name(&self) -> &'static str {
        match self {
            DbBackend::Postgres(_) => "PostgreSQL",
            DbBackend::Sqlite(_) => "Embedded SQLite",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sqlite_init_and_verify() {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_hostable_{}.db", rand::random::<u32>()));
        let db_url = format!("sqlite://{}?mode=rwc", db_path.display());

        let (backend, token) = DbBackend::init(Some(db_url)).await.expect("SQLite init failed");
        assert_eq!(backend.engine_name(), "Embedded SQLite");
        assert!(token.starts_with("hst_"));

        let valid = backend.verify_token(&token).await.expect("verify failed");
        assert!(valid);

        let invalid = backend.verify_token("wrong_token").await.expect("verify failed");
        assert!(!invalid);

        let _ = std::fs::remove_file(db_path);
    }
}
