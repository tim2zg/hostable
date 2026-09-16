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

    pub fn hash_token(token: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        hex::encode(hasher.finalize())
    }

    async fn ensure_admin_token_pg(pool: &Pool<Postgres>) -> Result<String, String> {
        let existing: Option<(String,)> =
            sqlx::query_as("SELECT api_token FROM users WHERE username = 'admin'")
                .fetch_optional(pool)
                .await
                .map_err(|e| e.to_string())?;

        if let Some((stored,)) = existing {
            // If stored token is in plaintext legacy format (starts with hst_), migrate to hash
            if stored.starts_with("hst_") {
                let hashed = Self::hash_token(&stored);
                let _ = sqlx::query("UPDATE users SET api_token = $1 WHERE username = 'admin'")
                    .bind(&hashed)
                    .execute(pool)
                    .await;
                return Ok(stored);
            }
            return Ok("[hashed token configured in database]".to_string());
        }

        let raw_token = format!("hst_{}", Alphanumeric.sample_string(&mut rand::rng(), 32));
        let hashed_token = Self::hash_token(&raw_token);
        sqlx::query("INSERT INTO users (username, api_token) VALUES ($1, $2)")
            .bind("admin")
            .bind(&hashed_token)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;

        Ok(raw_token)
    }

    async fn ensure_admin_token_sqlite(pool: &Pool<Sqlite>) -> Result<String, String> {
        let existing: Option<(String,)> =
            sqlx::query_as("SELECT api_token FROM users WHERE username = 'admin'")
                .fetch_optional(pool)
                .await
                .map_err(|e| e.to_string())?;

        if let Some((stored,)) = existing {
            if stored.starts_with("hst_") {
                let hashed = Self::hash_token(&stored);
                let _ = sqlx::query("UPDATE users SET api_token = ? WHERE username = 'admin'")
                    .bind(&hashed)
                    .execute(pool)
                    .await;
                return Ok(stored);
            }
            return Ok("[hashed token configured in database]".to_string());
        }

        let raw_token = format!("hst_{}", Alphanumeric.sample_string(&mut rand::rng(), 32));
        let hashed_token = Self::hash_token(&raw_token);
        sqlx::query("INSERT INTO users (username, api_token) VALUES (?, ?)")
            .bind("admin")
            .bind(&hashed_token)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;

        Ok(raw_token)
    }

    pub async fn verify_token(&self, raw_token: &str) -> Result<bool, String> {
        let hashed = Self::hash_token(raw_token);
        match self {
            DbBackend::Postgres(pool) => {
                let user: Option<(i32, String)> =
                    sqlx::query_as("SELECT id, api_token FROM users WHERE api_token = $1 OR api_token = $2")
                        .bind(&hashed)
                        .bind(raw_token)
                        .fetch_optional(pool)
                        .await
                        .map_err(|e| e.to_string())?;

                if let Some((id, stored)) = user {
                    if stored == raw_token {
                        // In-place migration of legacy token to SHA-256 hash
                        let _ = sqlx::query("UPDATE users SET api_token = $1 WHERE id = $2")
                            .bind(&hashed)
                            .bind(id)
                            .execute(pool)
                            .await;
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            DbBackend::Sqlite(pool) => {
                let user: Option<(i64, String)> =
                    sqlx::query_as("SELECT id, api_token FROM users WHERE api_token = ? OR api_token = ?")
                        .bind(&hashed)
                        .bind(raw_token)
                        .fetch_optional(pool)
                        .await
                        .map_err(|e| e.to_string())?;

                if let Some((id, stored)) = user {
                    if stored == raw_token {
                        let _ = sqlx::query("UPDATE users SET api_token = ? WHERE id = ?")
                            .bind(&hashed)
                            .bind(id)
                            .execute(pool)
                            .await;
                    }
                    Ok(true)
                } else {
                    Ok(false)
                }
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

        // Verify the token stored in database is hashed, NOT plaintext
        if let DbBackend::Sqlite(pool) = &backend {
            let (stored_token,): (String,) = sqlx::query_as("SELECT api_token FROM users WHERE username = 'admin'")
                .fetch_one(pool)
                .await
                .expect("fetch stored token failed");
            assert_ne!(stored_token, token, "Stored token must not be plaintext");
            assert_eq!(stored_token, DbBackend::hash_token(&token), "Stored token must match SHA-256 hash");
        }

        let valid = backend.verify_token(&token).await.expect("verify failed");
        assert!(valid);

        let invalid = backend.verify_token("wrong_token").await.expect("verify failed");
        assert!(!invalid);

        let _ = std::fs::remove_file(db_path);
    }

    #[tokio::test]
    async fn test_legacy_token_migration() {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("test_hostable_legacy_{}.db", rand::random::<u32>()));
        let db_url = format!("sqlite://{}?mode=rwc", db_path.display());

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect(&db_url)
            .await
            .unwrap();

        sqlx::query(
            "CREATE TABLE users (id INTEGER PRIMARY KEY AUTOINCREMENT, username TEXT NOT NULL UNIQUE, api_token TEXT NOT NULL UNIQUE, created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let legacy_token = "hst_legacy_plaintext_token_123456";
        sqlx::query("INSERT INTO users (username, api_token) VALUES (?, ?)")
            .bind("legacy_user")
            .bind(legacy_token)
            .execute(&pool)
            .await
            .unwrap();

        let backend = DbBackend::Sqlite(pool.clone());

        // Verification must succeed with the legacy token
        let valid = backend.verify_token(legacy_token).await.expect("verify legacy failed");
        assert!(valid);

        // Verification must have migrated the record in-place to SHA-256
        let (migrated_token,): (String,) = sqlx::query_as("SELECT api_token FROM users WHERE username = 'legacy_user'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(migrated_token, DbBackend::hash_token(legacy_token));

        let _ = std::fs::remove_file(db_path);
    }
}
