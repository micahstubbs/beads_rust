use crate::storage::SqliteStorage;

pub mod agents;
pub mod audit;
pub mod blocked;
pub mod changelog;
pub mod close;
pub mod comments;
pub mod completions;
pub mod config;
pub mod count;
pub mod create;
pub mod defer;
pub mod delete;
pub mod dep;
pub mod doctor;
pub mod epic;
pub mod graph;
pub mod history;
pub mod info;
pub mod init;
pub mod label;
pub mod lint;
pub mod list;
pub mod orphans;
pub mod q;
pub mod query;
pub mod ready;
pub mod reopen;
pub mod schema;
pub mod search;
pub mod show;
pub mod stale;
pub mod stats;
pub mod sync;
pub mod update;
pub mod version;
pub mod r#where;

#[cfg(feature = "self_update")]
pub mod upgrade;

pub(super) fn rebuild_blocked_cache_after_partial_mutation(
    storage: &mut SqliteStorage,
    cache_dirty: bool,
    command: &str,
) -> crate::Result<()> {
    if !cache_dirty {
        return Ok(());
    }

    storage
        .rebuild_blocked_cache(true)
        .map(|_| ())
        .map_err(|rebuild_err| crate::error::BeadsError::WithContext {
            context: format!("failed to rebuild blocked cache after partial {command} mutation"),
            source: Box::new(rebuild_err),
        })
}

pub(super) fn preserve_blocked_cache_on_error<T>(
    storage: &mut SqliteStorage,
    cache_dirty: bool,
    command: &str,
    result: crate::Result<T>,
) -> crate::Result<T> {
    match result {
        Ok(value) => Ok(value),
        Err(operation_err) => {
            if let Err(rebuild_err) =
                rebuild_blocked_cache_after_partial_mutation(storage, cache_dirty, command)
            {
                return Err(crate::error::BeadsError::WithContext {
                    context: format!(
                        "failed to preserve blocked cache after partial {command} mutation; original operation error: {operation_err}"
                    ),
                    source: Box::new(rebuild_err),
                });
            }
            Err(operation_err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{preserve_blocked_cache_on_error, rebuild_blocked_cache_after_partial_mutation};
    use crate::error::BeadsError;
    use crate::storage::SqliteStorage;
    use fsqlite::Connection;
    use tempfile::TempDir;

    #[test]
    fn partial_mutation_rebuild_skips_clean_state() {
        let mut storage = SqliteStorage::open_memory().expect("storage");
        rebuild_blocked_cache_after_partial_mutation(&mut storage, false, "close")
            .expect("clean state should not rebuild");
    }

    #[test]
    fn preserve_returns_original_error_when_rebuild_succeeds() {
        let mut storage = SqliteStorage::open_memory().expect("storage");
        let result: crate::Result<()> = Err(BeadsError::validation("ids", "boom"));
        let err = preserve_blocked_cache_on_error::<()>(&mut storage, true, "close", result)
            .expect_err("operation should still fail");

        assert!(matches!(err, BeadsError::Validation { .. }));
    }

    #[test]
    fn preserve_surfaces_rebuild_failure() {
        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("beads.db");
        let mut storage = SqliteStorage::open(&db_path).expect("storage");
        let conn = Connection::open(db_path.to_string_lossy().into_owned()).expect("conn");
        conn.execute("DROP TABLE blocked_issues_cache")
            .expect("drop blocked cache table");

        let result: crate::Result<()> = Err(BeadsError::validation("ids", "boom"));
        let err = preserve_blocked_cache_on_error::<()>(&mut storage, true, "reopen", result)
            .expect_err("rebuild failure should be surfaced");

        match err {
            BeadsError::WithContext { context, .. } => {
                assert!(context.contains("partial reopen mutation"));
                assert!(context.contains("Validation failed: ids: boom"));
            }
            other => panic!("expected WithContext, got {other:?}"),
        }
    }
}
