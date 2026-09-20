pub mod models;
pub mod schema;

use std::time::Duration;

use diesel::connection::SimpleConnection;
use diesel::r2d2::{ConnectionManager, CustomizeConnection, Pool};
use diesel::{Connection, SqliteConnection};

pub type DbPool = Pool<ConnectionManager<SqliteConnection>>;

/// Per-connection pragmas applied when a pooled connection is created.
///
/// `journal_mode = WAL` is intentionally *not* set here: switching journal mode
/// requires a lock, and concurrent pool acquisitions at startup would race.
/// It is set once in [`build_pool`] instead.
///
/// Note: `r2d2`'s error type cannot be constructed publicly, so a failure here
/// is logged rather than propagated.
#[derive(Debug)]
struct SqlitePragmas;

impl CustomizeConnection<SqliteConnection, diesel::r2d2::Error> for SqlitePragmas {
    fn on_acquire(&self, conn: &mut SqliteConnection) -> Result<(), diesel::r2d2::Error> {
        if let Err(e) = conn.batch_execute(
            "PRAGMA busy_timeout = 5000;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        ) {
            tracing::error!("failed to configure sqlite connection: {e}");
        }
        Ok(())
    }
}

/// Build the connection pool, creating the database file if needed.
pub fn build_pool(db_path: &str) -> anyhow::Result<DbPool> {
    // Enable WAL exactly once, before any pooled connection can race for it.
    // WAL is a persistent property of the database file.
    {
        let mut conn = SqliteConnection::establish(db_path)?;
        conn.batch_execute("PRAGMA busy_timeout = 5000; PRAGMA journal_mode = WAL;")?;
    }

    let manager = ConnectionManager::<SqliteConnection>::new(db_path);
    let pool = Pool::builder()
        .max_size(16)
        .connection_timeout(Duration::from_secs(10))
        .connection_customizer(Box::new(SqlitePragmas))
        .build(manager)?;
    Ok(pool)
}
