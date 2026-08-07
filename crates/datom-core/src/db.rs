//! Database connectivity for datom-connect.
//!
//! Connections target the Postgres instance defined in the repo's
//! `compose.yml`. The connection URL is read from [`DATABASE_URL_ENV`],
//! defaulting to [`DEFAULT_DATABASE_URL`].

use sqlx::postgres::PgPoolOptions;

use crate::Result;

/// Environment variable that overrides the database connection URL.
pub const DATABASE_URL_ENV: &str = "DATOM_DATABASE_URL";

/// Default connection URL, matching the `db` service in the repo's `compose.yml`.
/// TODO: Make this configurable via a `datom.toml` or a CLI flag.
pub const DEFAULT_DATABASE_URL: &str = "postgres://datom:datom@localhost:54325/datom";

/// Resolve the database connection URL from the environment, falling back to
/// [`DEFAULT_DATABASE_URL`] when [`DATABASE_URL_ENV`] is unset.
pub fn database_url() -> String {
    resolve_database_url(std::env::var(DATABASE_URL_ENV).ok())
}

/// Pick between an override (the value of [`DATABASE_URL_ENV`], if set) and
/// [`DEFAULT_DATABASE_URL`].
fn resolve_database_url(override_url: Option<String>) -> String {
    override_url.unwrap_or_else(|| DEFAULT_DATABASE_URL.to_string())
}

/// Connect to the database and verify connectivity by running `SELECT 1`.
///
/// # Errors
///
/// Returns [`CoreError::Database`](crate::CoreError::Database) if the
/// connection cannot be established or the query fails.
pub async fn ping() -> Result<()> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url())
        .await?;

    let one: i32 = sqlx::query_scalar("SELECT 1").fetch_one(&pool).await?;
    debug_assert_eq!(one, 1);

    pool.close().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_url_uses_override_or_default() {
        assert_eq!(resolve_database_url(None), DEFAULT_DATABASE_URL);
        assert_eq!(
            resolve_database_url(Some("postgres://user:pw@host:5432/db".to_string())),
            "postgres://user:pw@host:5432/db"
        );
    }
}
