//! Reopen command implementation.

use crate::cli::ReopenArgs;
use crate::cli::commands::preserve_blocked_cache_on_error;
use crate::config;
use crate::error::{BeadsError, Result};
use crate::model::Status;
use crate::output::{OutputContext, OutputMode};
use crate::storage::IssueUpdate;
use crate::util::id::{IdResolver, ResolverConfig, find_matching_ids};
use rich_rust::prelude::*;
use serde::Serialize;

/// Result of reopening a single issue.
#[derive(Debug, Serialize)]
pub struct ReopenedIssue {
    pub id: String,
    pub title: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
}

/// Issue that was skipped during reopen.
#[derive(Debug, Serialize)]
pub struct SkippedIssue {
    pub id: String,
    pub reason: String,
}

/// JSON output for reopen command.
#[derive(Debug, Serialize)]
pub struct ReopenResult {
    pub reopened: Vec<ReopenedIssue>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<SkippedIssue>,
}

/// Execute the reopen command.
///
/// # Errors
///
/// Returns an error if database operations fail or IDs cannot be resolved.
#[allow(clippy::too_many_lines)]
pub fn execute(
    args: &ReopenArgs,
    json: bool,
    cli: &config::CliOverrides,
    ctx: &OutputContext,
) -> Result<()> {
    let use_json = json || ctx.is_json() || args.robot;

    tracing::info!("Executing reopen command");

    let beads_dir = config::discover_beads_dir_with_cli(cli)?;
    let mut storage_ctx = config::open_storage_with_cli(&beads_dir, cli)?;

    let config_layer = storage_ctx.load_config(cli)?;
    let actor = config::resolve_actor(&config_layer);
    let id_config = config::id_config_from_layer(&config_layer);
    let resolver = IdResolver::new(ResolverConfig::with_prefix(id_config.prefix));
    let all_ids = storage_ctx.storage.get_all_ids()?;
    let storage = &mut storage_ctx.storage;

    // Get IDs - use last touched if none provided
    let mut ids = args.ids.clone();
    if ids.is_empty() {
        let last_touched = crate::util::get_last_touched_id(&beads_dir);
        if last_touched.is_empty() {
            return Err(BeadsError::validation(
                "ids",
                "no issue IDs provided and no last-touched issue",
            ));
        }
        ids.push(last_touched);
    }

    // Resolve all IDs
    let resolved_ids = resolver.resolve_all(
        &ids,
        |id| all_ids.binary_search_by(|p| p.as_str().cmp(id)).is_ok(),
        |hash| find_matching_ids(&all_ids, hash),
    )?;

    let mut reopened_issues: Vec<ReopenedIssue> = Vec::new();
    let mut skipped_issues: Vec<SkippedIssue> = Vec::new();
    let mut cache_dirty = false;

    for resolved in &resolved_ids {
        let id = &resolved.id;
        tracing::info!(id = %id, "Reopening issue");

        // Get current issue
        let Some(issue) =
            preserve_blocked_cache_on_error(storage, cache_dirty, "reopen", storage.get_issue(id))?
        else {
            skipped_issues.push(SkippedIssue {
                id: id.clone(),
                reason: "issue not found".to_string(),
            });
            continue;
        };

        // Tombstones are deletion markers and must not be resurrected through reopen.
        if issue.status == Status::Tombstone {
            tracing::debug!(id = %id, "Issue is tombstoned and cannot be reopened");
            skipped_issues.push(SkippedIssue {
                id: id.clone(),
                reason: "cannot reopen tombstone issue".to_string(),
            });
            continue;
        }

        // Only closed issues can be reopened.
        if issue.status != Status::Closed {
            tracing::debug!(id = %id, status = ?issue.status, "Issue is not closed");
            skipped_issues.push(SkippedIssue {
                id: id.clone(),
                reason: format!("already {}", issue.status.as_str()),
            });
            continue;
        }

        tracing::debug!(previous_status = ?issue.status, "Issue was previously {:?}", issue.status);

        // Build update: set status=open and clear close/defer metadata.
        let update = IssueUpdate {
            status: Some(Status::Open),
            closed_at: Some(None),         // Clear closed_at
            close_reason: Some(None),      // Clear close_reason
            closed_by_session: Some(None), // Clear closed_by_session
            defer_until: Some(None),       // Reopened issues should not stay deferred
            deleted_at: Some(None),        // Clear deleted_at
            deleted_by: Some(None),        // Clear deleted_by
            delete_reason: Some(None),     // Clear delete_reason
            skip_cache_rebuild: true,
            ..Default::default()
        };

        // Apply update
        let update_result = storage.update_issue(id, &update, &actor);
        preserve_blocked_cache_on_error(storage, cache_dirty, "reopen", update_result)?;
        cache_dirty = true;
        tracing::info!(id = %id, reason = ?args.reason, "Issue reopened");

        // Add comment if reason provided
        if let Some(ref reason) = args.reason {
            let comment_text = format!("Reopened: {reason}");
            tracing::debug!(id = %id, "Adding reopen comment");
            let comment_result = storage.add_comment(id, &actor, &comment_text);
            preserve_blocked_cache_on_error(storage, cache_dirty, "reopen", comment_result)?;
        }

        // Update last touched
        crate::util::set_last_touched_id(&beads_dir, id);

        reopened_issues.push(ReopenedIssue {
            id: id.clone(),
            title: issue.title.clone(),
            status: "open".to_string(),
            closed_at: None,
        });
    }

    if cache_dirty {
        tracing::info!(
            "Rebuilding blocked cache after reopening {} issues",
            reopened_issues.len()
        );
        storage.rebuild_blocked_cache(true)?;
    }

    // Output
    if use_json {
        let result = ReopenResult {
            reopened: reopened_issues,
            skipped: skipped_issues,
        };
        if ctx.is_json() {
            ctx.json_pretty(&result);
        } else {
            let json_ctx = OutputContext::from_flags(true, false, true);
            json_ctx.json_pretty(&result);
        }
    } else if matches!(ctx.mode(), OutputMode::Rich) {
        render_reopen_rich(
            &reopened_issues,
            &skipped_issues,
            args.reason.as_deref(),
            ctx,
        );
    } else {
        for reopened in &reopened_issues {
            print!("\u{2713} Reopened {}: {}", reopened.id, reopened.title);
            if let Some(ref reason) = args.reason {
                println!(" ({reason})");
            } else {
                println!();
            }
        }
        for skipped in &skipped_issues {
            println!("\u{2298} Skipped {}: {}", skipped.id, skipped.reason);
        }
        if reopened_issues.is_empty() && skipped_issues.is_empty() {
            println!("No issues to reopen.");
        }
    }

    storage_ctx.flush_no_db_if_dirty()?;
    Ok(())
}

/// Render reopen results with rich formatting.
fn render_reopen_rich(
    reopened: &[ReopenedIssue],
    skipped: &[SkippedIssue],
    reason: Option<&str>,
    ctx: &OutputContext,
) {
    let console = Console::default();
    let theme = ctx.theme();
    let width = ctx.width();

    let mut content = Text::new("");

    if reopened.is_empty() && skipped.is_empty() {
        content.append("No issues to reopen.\n");
    } else {
        for item in reopened {
            content.append_styled("\u{2713} ", theme.success.clone());
            content.append_styled("Reopened ", theme.success.clone());
            content.append_styled(&item.id, theme.emphasis.clone());
            content.append(": ");
            content.append(&item.title);
            if let Some(r) = reason {
                content.append_styled(&format!(" ({r})"), theme.dimmed.clone());
            }
            content.append("\n");
            content.append_styled("  Status: ", theme.dimmed.clone());
            content.append_styled("closed", theme.error.clone());
            content.append(" \u{2192} ");
            content.append_styled("open", theme.success.clone());
            content.append("\n");
        }

        for item in skipped {
            content.append_styled("\u{2298} ", theme.warning.clone());
            content.append_styled("Skipped ", theme.warning.clone());
            content.append_styled(&item.id, theme.emphasis.clone());
            content.append(": ");
            content.append_styled(&item.reason, theme.dimmed.clone());
            content.append("\n");
        }
    }

    let title = if reopened.len() == 1 && skipped.is_empty() {
        "Issue Reopened"
    } else {
        "Reopen Results"
    };

    let panel = Panel::from_rich_text(&content, width)
        .title(Text::styled(title, theme.panel_title.clone()))
        .box_style(theme.box_style);

    console.print_renderable(&panel);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::commands;
    use crate::config::CliOverrides;
    use crate::model::{Issue, IssueType, Priority, Status};
    use crate::output::OutputContext;
    use crate::storage::SqliteStorage;
    use chrono::{Duration, Utc};
    use std::env;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static TEST_DIR_LOCK: Mutex<()> = Mutex::new(());

    struct DirGuard {
        previous: PathBuf,
    }

    impl DirGuard {
        fn new(target: &std::path::Path) -> Self {
            let previous = env::current_dir().expect("current dir");
            env::set_current_dir(target).expect("set current dir");
            Self { previous }
        }
    }

    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = env::set_current_dir(&self.previous);
        }
    }

    fn make_closed_deferred_issue(id: &str, title: &str) -> Issue {
        let now = Utc::now();
        Issue {
            id: id.to_string(),
            title: title.to_string(),
            status: Status::Closed,
            priority: Priority::MEDIUM,
            issue_type: IssueType::Task,
            created_at: now,
            updated_at: now,
            closed_at: Some(now),
            defer_until: Some(now + Duration::days(7)),
            ..Issue::default()
        }
    }

    #[test]
    fn execute_clears_defer_until_when_reopening_closed_deferred_issue() {
        let _lock = TEST_DIR_LOCK.lock().expect("dir lock");
        let temp = TempDir::new().expect("tempdir");
        let ctx = OutputContext::from_flags(false, false, true);
        commands::init::execute(None, false, Some(temp.path()), &ctx).expect("init");

        let beads_dir = temp.path().join(".beads");
        let db_path = beads_dir.join("beads.db");
        let mut storage = SqliteStorage::open(&db_path).expect("storage");
        storage
            .create_issue(
                &make_closed_deferred_issue("bd-reopen-deferred", "Closed deferred issue"),
                "tester",
            )
            .expect("create issue");
        drop(storage);

        let _guard = DirGuard::new(temp.path());
        let args = ReopenArgs {
            ids: vec!["bd-reopen-deferred".to_string()],
            reason: None,
            robot: false,
        };
        execute(&args, false, &CliOverrides::default(), &ctx).expect("reopen");

        let storage = SqliteStorage::open(&db_path).expect("reopen storage");
        let issue = storage
            .get_issue("bd-reopen-deferred")
            .expect("get issue")
            .expect("issue exists");

        assert_eq!(issue.status, Status::Open);
        assert!(issue.defer_until.is_none());
        assert!(issue.closed_at.is_none());
    }

    #[test]
    fn execute_reopen_tombstone_skips_without_resurrecting_it() {
        let _lock = TEST_DIR_LOCK.lock().expect("dir lock");
        let temp = TempDir::new().expect("tempdir");
        let ctx = OutputContext::from_flags(false, false, true);
        commands::init::execute(None, false, Some(temp.path()), &ctx).expect("init");

        let beads_dir = temp.path().join(".beads");
        let db_path = beads_dir.join("beads.db");
        let mut storage = SqliteStorage::open(&db_path).expect("storage");
        let issue = Issue {
            id: "bd-reopen-tombstone".to_string(),
            title: "Deleted issue".to_string(),
            status: Status::Open,
            priority: Priority::MEDIUM,
            issue_type: IssueType::Task,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            ..Issue::default()
        };
        storage
            .create_issue(&issue, "tester")
            .expect("create issue");
        storage
            .delete_issue(
                "bd-reopen-tombstone",
                "tester",
                "delete for reopen test",
                None,
            )
            .expect("delete issue");
        drop(storage);

        let _guard = DirGuard::new(temp.path());
        let args = ReopenArgs {
            ids: vec!["bd-reopen-tombstone".to_string()],
            reason: None,
            robot: false,
        };
        execute(&args, false, &CliOverrides::default(), &ctx).expect("reopen");

        let storage = SqliteStorage::open(&db_path).expect("reopen storage");
        let issue = storage
            .get_issue("bd-reopen-tombstone")
            .expect("get issue")
            .expect("issue exists");

        assert_eq!(issue.status, Status::Tombstone);
        assert!(issue.deleted_at.is_some());
    }
}
