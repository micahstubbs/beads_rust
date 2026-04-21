use crate::config::OpenStorageResult;
use crate::error::BeadsError;
use crate::model::Issue;
use crate::storage::{IssueUpdate, SqliteStorage};
use crate::sync::auto_import_if_stale;
use crate::util::id::IdResolver;

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

/// Resolve an issue ID from a potentially partial input.
pub(super) fn resolve_issue_id(
    storage: &SqliteStorage,
    resolver: &IdResolver,
    input: &str,
) -> crate::Result<String> {
    resolver
        .resolve_fallible(
            input,
            |id| storage.id_exists(id),
            |hash| storage.find_ids_by_hash(hash),
        )
        .map(|resolved| resolved.id)
}

pub(super) fn resolve_issue_ids(
    storage: &SqliteStorage,
    resolver: &IdResolver,
    inputs: &[String],
) -> crate::Result<Vec<String>> {
    resolver
        .resolve_all_fallible(
            inputs,
            |id| storage.id_exists(id),
            |hash| storage.find_ids_by_hash(hash),
        )
        .map(|resolved| resolved.into_iter().map(|entry| entry.id).collect())
}

pub(super) fn rebuild_blocked_cache_after_partial_mutation(
    storage: &mut SqliteStorage,
    cache_dirty: bool,
    command: &str,
) -> crate::Result<()> {
    if !cache_dirty {
        return Ok(());
    }

    match storage.mark_blocked_cache_stale() {
        Ok(()) => {
            tracing::debug!(
                command = command,
                "Blocked cache repair deferred after partial mutation; cache remains marked stale"
            );
            Ok(())
        }
        Err(mark_error) => {
            tracing::warn!(
                command = command,
                error = %mark_error,
                "Failed to pre-mark blocked cache stale before rebuilding after partial mutation"
            );
            storage
                .rebuild_blocked_cache(true)
                .map(|_| ())
                .map_err(|rebuild_err| crate::error::BeadsError::WithContext {
                    context: format!(
                        "failed to rebuild blocked cache after partial {command} mutation; \
                         pre-marking it stale also failed: {mark_error}"
                    ),
                    source: Box::new(rebuild_err),
                })
        }
    }
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

pub(super) fn finalize_batched_blocked_cache_refresh(
    storage: &mut SqliteStorage,
    cache_dirty: bool,
    command: &str,
) -> crate::Result<()> {
    if !cache_dirty {
        return Ok(());
    }

    if storage.blocked_cache_marked_stale().unwrap_or(false) {
        tracing::debug!(
            command = command,
            "Blocked cache already marked stale inside the mutation transaction; skipping eager batched refresh"
        );
        return Ok(());
    }

    match storage.mark_blocked_cache_stale() {
        Ok(()) => {
            tracing::debug!(
                command = command,
                "Blocked cache refresh deferred after successful batched mutation; cache remains marked stale"
            );
            Ok(())
        }
        Err(mark_error) => {
            tracing::warn!(
                command = command,
                error = %mark_error,
                "Failed to pre-mark blocked cache stale before batched refresh"
            );
            storage
                .rebuild_blocked_cache(true)
                .map(|_| ())
                .map_err(|rebuild_err| crate::error::BeadsError::WithContext {
                    context: format!(
                        "failed to rebuild blocked cache after successful batched {command} mutation; \
                         leaving the cache stale also failed first: {mark_error}"
                    ),
                    source: Box::new(rebuild_err),
                })
        }
    }
}

pub(super) fn update_issue_with_recovery(
    storage_ctx: &mut OpenStorageResult,
    allow_recovery: bool,
    command: &str,
    issue_id: &str,
    update: &IssueUpdate,
    actor: &str,
) -> crate::Result<Issue> {
    retry_mutation_with_jsonl_recovery(
        storage_ctx,
        allow_recovery,
        command,
        Some(issue_id),
        |storage| storage.update_issue(issue_id, update, actor),
    )
}

fn should_attempt_mutation_jsonl_recovery(
    storage_ctx: &OpenStorageResult,
    operation_err: &BeadsError,
    probe_err: Option<&BeadsError>,
) -> bool {
    matches!(operation_err, BeadsError::Database(_))
        && (storage_ctx.should_attempt_jsonl_recovery(operation_err)
            || probe_err.is_some_and(|err| storage_ctx.should_attempt_jsonl_recovery(err)))
}

pub(super) fn auto_import_storage_ctx_if_stale(
    storage_ctx: &mut OpenStorageResult,
    cli: &crate::config::CliOverrides,
) -> crate::Result<()> {
    // Issue #229: skip auto-import in --no-db mode.  The in-memory database
    // was just populated from the JSONL file during `open_storage_with_cli`,
    // so there is no staleness to detect.  Running the staleness probe here
    // is actively harmful because `compute_staleness_refreshing_witnesses`
    // calls `get_metadata` via `query_row_with_params`, which routes through
    // frankensqlite's prepared-statement fast path.  On in-memory databases
    // that fast path can warm up cached root-page references that become
    // stale after the bulk import's DELETE + INSERT cycle, causing subsequent
    // `get_issue_from_conn` calls inside write transactions to silently
    // return zero rows — the mechanism behind the "Issue not found" errors
    // on `br --no-db update`.
    if storage_ctx.no_db {
        return Ok(());
    }

    let config_layer = storage_ctx.load_config(cli)?;
    let no_auto_import = crate::config::no_auto_import_from_layer(&config_layer).unwrap_or(false);
    let allow_external_jsonl = crate::config::implicit_external_jsonl_allowed(
        &storage_ctx.paths.beads_dir,
        &storage_ctx.paths.db_path,
        &storage_ctx.paths.jsonl_path,
    );
    let expected_prefix = crate::config::id_config_from_layer(&config_layer).prefix;

    auto_import_if_stale(
        &mut storage_ctx.storage,
        &storage_ctx.paths.beads_dir,
        &storage_ctx.paths.jsonl_path,
        Some(expected_prefix.as_str()),
        allow_external_jsonl,
        cli.allow_stale.unwrap_or(false),
        no_auto_import,
    )
    .map(|_| ())
}

pub(super) fn retry_mutation_with_jsonl_recovery<T, F>(
    storage_ctx: &mut OpenStorageResult,
    allow_recovery: bool,
    command: &str,
    probe_issue_id: Option<&str>,
    mut operation: F,
) -> crate::Result<T>
where
    F: FnMut(&mut SqliteStorage) -> crate::Result<T>,
{
    match operation(&mut storage_ctx.storage) {
        Ok(value) => Ok(value),
        Err(operation_err) => {
            if !allow_recovery || !matches!(operation_err, BeadsError::Database(_)) {
                return Err(operation_err);
            }

            let mut recovery_signal =
                should_attempt_mutation_jsonl_recovery(storage_ctx, &operation_err, None);
            let mut probe_error: Option<BeadsError> = None;

            if !recovery_signal && let Some(issue_id) = probe_issue_id {
                match storage_ctx
                    .storage
                    .probe_issue_mutation_write_path(issue_id)
                {
                    Ok(()) => return Err(operation_err),
                    Err(probe_err) => {
                        recovery_signal = should_attempt_mutation_jsonl_recovery(
                            storage_ctx,
                            &operation_err,
                            Some(&probe_err),
                        );
                        probe_error = Some(probe_err);
                    }
                }
            }

            if !recovery_signal {
                return Err(operation_err);
            }

            let issue_id_label = probe_issue_id.unwrap_or("<none>");
            let probe_error_display = probe_error
                .as_ref()
                .map_or_else(|| "n/a".to_string(), std::string::ToString::to_string);
            tracing::warn!(
                command = command,
                issue_id = issue_id_label,
                original_error = %operation_err,
                probe_error = %probe_error_display,
                db_path = %storage_ctx.paths.db_path.display(),
                jsonl_path = %storage_ctx.paths.jsonl_path.display(),
                "Mutation hit a recoverable database corruption path; rebuilding from JSONL and retrying once"
            );

            let original_error = operation_err.to_string();
            storage_ctx.recover_database_from_jsonl().map_err(|recovery_err| {
                BeadsError::WithContext {
                    context: probe_issue_id.map_or_else(
                        || {
                            format!(
                                "automatic database recovery failed after {command} write; original write error: {original_error}"
                            )
                        },
                        |issue_id| {
                        format!(
                            "automatic database recovery failed after {command} write for issue '{issue_id}'; original write error: {original_error}"
                        )
                        },
                    ),
                    source: Box::new(recovery_err),
                }
            })?;

            operation(&mut storage_ctx.storage)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        finalize_batched_blocked_cache_refresh, preserve_blocked_cache_on_error,
        rebuild_blocked_cache_after_partial_mutation, retry_mutation_with_jsonl_recovery,
        should_attempt_mutation_jsonl_recovery,
    };
    use crate::config::{CliOverrides, OpenStorageResult, open_storage_with_cli};
    use crate::error::BeadsError;
    use crate::model::Issue;
    use crate::storage::SqliteStorage;
    use crate::sync::{ExportConfig, export_to_jsonl_with_policy};
    use fsqlite::Connection;
    use fsqlite_error::FrankenError;
    use std::fs;
    use tempfile::TempDir;

    fn storage_ctx_with_exported_issue() -> (TempDir, OpenStorageResult) {
        let temp = TempDir::new().expect("tempdir");
        let beads_dir = temp.path().join(".beads");
        fs::create_dir_all(&beads_dir).expect("create beads dir");
        let db_path = beads_dir.join("beads.db");
        let jsonl_path = beads_dir.join("issues.jsonl");

        // Scope the initial storage so the connection is closed before
        // recovery opens a new one at the same path.  fsqlite tracks pages
        // by file path, so an older connection causes BusySnapshot.
        {
            let mut storage = SqliteStorage::open(&db_path).expect("storage");
            let issue = Issue {
                id: "bd-1".to_string(),
                title: "test".to_string(),
                ..Issue::default()
            };
            storage
                .create_issue(&issue, "tester")
                .expect("create issue");
            let export_config = ExportConfig {
                beads_dir: Some(beads_dir.clone()),
                ..Default::default()
            };
            export_to_jsonl_with_policy(&storage, &jsonl_path, &export_config)
                .expect("export jsonl");
        }

        let storage_ctx =
            open_storage_with_cli(&beads_dir, &CliOverrides::default()).expect("storage ctx");
        (temp, storage_ctx)
    }

    #[test]
    fn partial_mutation_rebuild_skips_clean_state() {
        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("beads.db");
        let mut storage = SqliteStorage::open(&db_path).expect("storage");
        rebuild_blocked_cache_after_partial_mutation(&mut storage, false, "close")
            .expect("clean state should not rebuild");
    }

    #[test]
    fn preserve_returns_original_error_when_cache_is_marked_stale() {
        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("beads.db");
        let mut storage = SqliteStorage::open(&db_path).expect("storage");
        let result: crate::Result<()> = Err(BeadsError::validation("ids", "boom"));
        let err = preserve_blocked_cache_on_error::<()>(&mut storage, true, "close", result)
            .expect_err("operation should still fail");

        assert!(matches!(err, BeadsError::Validation { .. }));
    }

    #[test]
    fn preserve_surfaces_rebuild_failure_when_stale_marker_write_also_fails() {
        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("beads.db");
        let mut storage = SqliteStorage::open(&db_path).expect("storage");
        let conn = Connection::open(db_path.to_string_lossy().into_owned()).expect("conn");
        conn.execute("DROP TABLE blocked_issues_cache")
            .expect("drop blocked cache table");
        conn.execute("DROP TABLE metadata")
            .expect("drop metadata table");

        let result: crate::Result<()> = Err(BeadsError::validation("ids", "boom"));
        let err = preserve_blocked_cache_on_error::<()>(&mut storage, true, "reopen", result)
            .expect_err("rebuild failure should be surfaced");

        assert!(
            matches!(err, BeadsError::WithContext { .. }),
            "expected WithContext, got {err:?}"
        );
        if let BeadsError::WithContext { context, .. } = err {
            assert!(context.contains("partial reopen mutation"));
            assert!(context.contains("Validation failed: ids: boom"));
        }

        let metadata_probe = storage.get_metadata("blocked_cache_state");
        assert!(
            metadata_probe.is_err(),
            "metadata lookup should fail once the metadata table has been dropped"
        );
    }

    #[test]
    fn finalize_batched_refresh_marks_cache_stale_when_rebuild_fails() {
        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("beads.db");
        let mut storage = SqliteStorage::open(&db_path).expect("storage");
        let conn = Connection::open(db_path.to_string_lossy().into_owned()).expect("conn");
        conn.execute("DROP TABLE blocked_issues_cache")
            .expect("drop blocked cache table");

        finalize_batched_blocked_cache_refresh(&mut storage, true, "close")
            .expect("batched refresh should degrade to a stale marker");

        assert_eq!(
            storage
                .get_metadata("blocked_cache_state")
                .unwrap()
                .as_deref(),
            Some("stale")
        );
    }

    #[test]
    fn finalize_batched_refresh_degrades_when_cache_was_already_stale() {
        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("beads.db");
        let mut storage = SqliteStorage::open(&db_path).expect("storage");
        storage
            .mark_blocked_cache_stale()
            .expect("mark cache stale before finalization");

        let conn = Connection::open(db_path.to_string_lossy().into_owned()).expect("conn");
        conn.execute("DROP TABLE blocked_issues_cache")
            .expect("drop blocked cache table");

        finalize_batched_blocked_cache_refresh(&mut storage, true, "close")
            .expect("pre-marked stale cache should let finalization degrade cleanly");

        assert_eq!(
            storage
                .get_metadata("blocked_cache_state")
                .unwrap()
                .as_deref(),
            Some("stale")
        );
    }

    #[test]
    fn retry_mutation_recovers_from_recoverable_database_error() {
        let (_temp, mut storage_ctx) = storage_ctx_with_exported_issue();
        let mut attempts = 0;

        let result = retry_mutation_with_jsonl_recovery(
            &mut storage_ctx,
            true,
            "test-mutation",
            Some("bd-1"),
            |_storage| {
                attempts += 1;
                if attempts == 1 {
                    Err(BeadsError::Database(FrankenError::DatabaseCorrupt {
                        detail: "synthetic corruption".to_string(),
                    }))
                } else {
                    Ok("recovered")
                }
            },
        )
        .expect("recovered mutation");

        assert_eq!(result, "recovered");
        assert_eq!(attempts, 2);
        assert!(
            storage_ctx
                .storage
                .get_issue("bd-1")
                .expect("load issue")
                .is_some()
        );
    }

    #[test]
    fn mutation_recovery_can_be_signaled_by_probe_after_constraint_style_error() {
        let (_temp, storage_ctx) = storage_ctx_with_exported_issue();
        let operation_err = BeadsError::Database(FrankenError::Internal(
            "constraint verification failed".to_string(),
        ));
        let probe_err = BeadsError::Database(FrankenError::Internal(
            "database disk image is malformed".to_string(),
        ));

        assert!(
            !should_attempt_mutation_jsonl_recovery(&storage_ctx, &operation_err, None),
            "constraint-style write errors should not recover without a corruption probe"
        );
        assert!(
            should_attempt_mutation_jsonl_recovery(&storage_ctx, &operation_err, Some(&probe_err)),
            "a recoverable rollback-only write probe should trigger JSONL recovery"
        );
    }
}
