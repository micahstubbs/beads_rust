//! Defer and Undefer command implementations.

use crate::cli::commands::{
    finalize_batched_blocked_cache_refresh, preserve_blocked_cache_on_error, resolve_issue_ids,
    update_issue_with_recovery,
};
use crate::cli::{DeferArgs, UndeferArgs};
use crate::config;
use crate::error::{BeadsError, Result};
use crate::model::{Issue, Status};
use crate::output::{OutputContext, OutputMode};
use crate::storage::IssueUpdate;
use crate::util::id::{IdResolver, ResolverConfig};
use crate::util::time::parse_flexible_timestamp;
use rich_rust::prelude::*;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::Path;

/// Result of deferring a single issue (for text output).
#[derive(Debug, Clone, Serialize)]
pub struct DeferredIssue {
    pub id: String,
    pub title: String,
    pub previous_status: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub defer_until: Option<String>,
}

/// Issue that was skipped during defer.
#[derive(Debug, Clone, Serialize)]
pub struct SkippedIssue {
    pub id: String,
    pub reason: String,
}

#[derive(Debug, Serialize)]
struct DeferResult {
    pub deferred: Vec<DeferredIssue>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<SkippedIssue>,
    #[serde(skip)]
    ordered_outcomes: Vec<DeferredOutcome>,
}

#[derive(Debug, Serialize)]
struct UndeferResult {
    pub undeferred: Vec<DeferredIssue>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<SkippedIssue>,
    #[serde(skip)]
    ordered_outcomes: Vec<DeferredOutcome>,
}

#[derive(Debug, Clone)]
enum DeferredOutcome {
    Changed(DeferredIssue),
    Skipped(SkippedIssue),
}

fn restored_status_after_undefer(issue: &Issue) -> Status {
    if issue.status == Status::Deferred {
        Status::Open
    } else {
        issue.status.clone()
    }
}

/// Execute the defer command.
///
/// # Errors
///
/// Returns an error if database operations fail or IDs cannot be resolved.
pub fn execute_defer(
    args: &DeferArgs,
    json: bool,
    cli: &config::CliOverrides,
    ctx: &OutputContext,
) -> Result<()> {
    tracing::info!("Executing defer command");

    if args.ids.is_empty() {
        return Err(BeadsError::validation(
            "ids",
            "at least one issue ID is required",
        ));
    }

    let beads_dir = config::discover_beads_dir_with_cli(cli)?;
    let routed_batches = config::routing::group_issue_inputs_by_route(&args.ids, &beads_dir)?;
    let mut deferred_issues = Vec::new();
    let mut skipped_issues = Vec::new();

    if routed_batches.iter().any(|batch| batch.is_external) {
        let normalized_local_beads_dir =
            dunce::canonicalize(&beads_dir).unwrap_or_else(|_| beads_dir.clone());
        let mut routed_outcomes = Vec::new();

        for batch in routed_batches {
            let mut batch_args = args.clone();
            batch_args.ids.clone_from(&batch.issue_inputs);

            let normalized_batch_beads_dir =
                dunce::canonicalize(&batch.beads_dir).unwrap_or_else(|_| batch.beads_dir.clone());
            let mut batch_cli = cli.clone();
            batch_cli.db = if normalized_batch_beads_dir == normalized_local_beads_dir {
                cli.db.clone()
            } else {
                None
            };

            let result =
                execute_defer_route(&batch_args, &batch_cli, &batch.beads_dir, batch.is_external)?;
            routed_outcomes.push((batch.issue_inputs.clone(), result.ordered_outcomes));
        }

        let ordered_outcomes =
            reorder_routed_items_by_requested_inputs(&args.ids, routed_outcomes, "defer routing")?;
        for outcome in ordered_outcomes {
            match outcome {
                DeferredOutcome::Changed(issue) => deferred_issues.push(issue),
                DeferredOutcome::Skipped(issue) => skipped_issues.push(issue),
            }
        }
    } else {
        let result = execute_defer_route(args, cli, &beads_dir, false)?;
        deferred_issues = result.deferred;
        skipped_issues = result.skipped;
    }

    if let Some(last_deferred) = deferred_issues.last() {
        crate::util::set_last_touched_id(&beads_dir, &last_deferred.id);
    }

    render_defer_output(&deferred_issues, &skipped_issues, args, json, ctx)?;
    Ok(())
}

fn render_defer_output(
    deferred_issues: &[DeferredIssue],
    skipped_issues: &[SkippedIssue],
    args: &DeferArgs,
    json: bool,
    ctx: &OutputContext,
) -> Result<()> {
    let use_structured_output = json || ctx.is_json() || ctx.is_toon() || args.robot;
    if use_structured_output {
        if skipped_issues.is_empty() {
            emit_structured_output(&deferred_issues.to_vec(), ctx)?;
        } else {
            let result = DeferResult {
                deferred: deferred_issues.to_vec(),
                skipped: skipped_issues.to_vec(),
                ordered_outcomes: Vec::new(),
            };
            emit_structured_output(&result, ctx)?;
        }
    } else if matches!(ctx.mode(), OutputMode::Quiet) {
        return Ok(());
    } else if matches!(ctx.mode(), OutputMode::Rich) {
        render_defer_rich(deferred_issues, skipped_issues, ctx);
    } else {
        for deferred in deferred_issues {
            print!("\u{23f1} Deferred {}: {}", deferred.id, deferred.title);
            if let Some(ref until) = deferred.defer_until {
                println!(" (until {until})");
            } else {
                println!(" (indefinitely)");
            }
        }
        for skipped in skipped_issues {
            println!("\u{2298} Skipped {}: {}", skipped.id, skipped.reason);
        }
        if deferred_issues.is_empty() && skipped_issues.is_empty() {
            println!("No issues to defer.");
        }
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
fn execute_defer_route(
    args: &DeferArgs,
    cli: &config::CliOverrides,
    beads_dir: &Path,
    auto_flush_external: bool,
) -> Result<DeferResult> {
    let mut storage_ctx = config::open_storage_with_cli(beads_dir, cli)?;

    let config_layer = storage_ctx.load_config(cli)?;
    let actor = config::resolve_actor(&config_layer);
    let id_config = config::id_config_from_layer(&config_layer);
    let resolver = IdResolver::new(ResolverConfig::with_prefix(id_config.prefix));

    let defer_until = args
        .until
        .as_ref()
        .map(|s| parse_flexible_timestamp(s, "defer_until"))
        .transpose()?;

    let resolved_ids = resolve_issue_ids(&storage_ctx.storage, &resolver, &args.ids)?;

    let mut deferred_issues: Vec<DeferredIssue> = Vec::new();
    let mut skipped_issues: Vec<SkippedIssue> = Vec::new();
    let mut ordered_outcomes = Vec::with_capacity(resolved_ids.len());
    let mut cache_dirty = false;

    for id in &resolved_ids {
        tracing::info!(id = %id, until = ?defer_until, "Deferring issue");

        let issue_result = storage_ctx.storage.get_issue(id);
        let Some(issue) = preserve_blocked_cache_on_error(
            &mut storage_ctx.storage,
            cache_dirty,
            "defer",
            issue_result,
        )?
        else {
            let skipped = SkippedIssue {
                id: id.clone(),
                reason: "issue not found".to_string(),
            };
            ordered_outcomes.push(DeferredOutcome::Skipped(skipped.clone()));
            skipped_issues.push(skipped);
            continue;
        };

        if issue.status.is_terminal() {
            tracing::debug!(id = %id, status = ?issue.status, "Issue is terminal");
            let skipped = SkippedIssue {
                id: id.clone(),
                reason: format!("cannot defer {} issue", issue.status.as_str()),
            };
            ordered_outcomes.push(DeferredOutcome::Skipped(skipped.clone()));
            skipped_issues.push(skipped);
            continue;
        }

        if issue.status == Status::Deferred && issue.defer_until == defer_until {
            tracing::debug!(id = %id, "Issue already deferred with same time");
            let skipped = SkippedIssue {
                id: id.clone(),
                reason: "already deferred".to_string(),
            };
            ordered_outcomes.push(DeferredOutcome::Skipped(skipped.clone()));
            skipped_issues.push(skipped);
            continue;
        }

        let update = IssueUpdate {
            status: Some(Status::Deferred),
            defer_until: Some(defer_until),
            skip_cache_rebuild: true,
            ..Default::default()
        };

        let update_result = update_issue_with_recovery(
            &mut storage_ctx,
            !cache_dirty,
            "defer",
            id,
            &update,
            &actor,
        );
        preserve_blocked_cache_on_error(
            &mut storage_ctx.storage,
            cache_dirty,
            "defer",
            update_result,
        )?;
        cache_dirty = true;
        tracing::info!(id = %id, defer_until = ?defer_until, "Issue deferred");

        let deferred = DeferredIssue {
            id: id.clone(),
            title: issue.title.clone(),
            previous_status: issue.status.as_str().to_string(),
            status: "deferred".to_string(),
            defer_until: defer_until.map(|dt| dt.to_rfc3339()),
        };
        ordered_outcomes.push(DeferredOutcome::Changed(deferred.clone()));
        deferred_issues.push(deferred);
    }

    if cache_dirty {
        tracing::info!(
            "Rebuilding blocked cache after deferring {} issues",
            deferred_issues.len()
        );
        finalize_batched_blocked_cache_refresh(&mut storage_ctx.storage, cache_dirty, "defer")?;
    }

    storage_ctx.flush_no_db_if_dirty()?;
    if auto_flush_external && let Err(error) = storage_ctx.auto_flush_if_enabled() {
        tracing::debug!(
            beads_dir = %storage_ctx.paths.beads_dir.display(),
            error = %error,
            "Routed auto-flush failed (non-fatal)"
        );
    }

    Ok(DeferResult {
        deferred: deferred_issues,
        skipped: skipped_issues,
        ordered_outcomes,
    })
}

/// Execute the undefer command.
///
/// # Errors
///
/// Returns an error if database operations fail or IDs cannot be resolved.
pub fn execute_undefer(
    args: &UndeferArgs,
    json: bool,
    cli: &config::CliOverrides,
    ctx: &OutputContext,
) -> Result<()> {
    tracing::info!("Executing undefer command");

    if args.ids.is_empty() {
        return Err(BeadsError::validation(
            "ids",
            "at least one issue ID is required",
        ));
    }

    let beads_dir = config::discover_beads_dir_with_cli(cli)?;
    let routed_batches = config::routing::group_issue_inputs_by_route(&args.ids, &beads_dir)?;
    let mut undeferred_issues = Vec::new();
    let mut skipped_issues = Vec::new();

    if routed_batches.iter().any(|batch| batch.is_external) {
        let normalized_local_beads_dir =
            dunce::canonicalize(&beads_dir).unwrap_or_else(|_| beads_dir.clone());
        let mut routed_outcomes = Vec::new();

        for batch in routed_batches {
            let mut batch_args = args.clone();
            batch_args.ids.clone_from(&batch.issue_inputs);

            let normalized_batch_beads_dir =
                dunce::canonicalize(&batch.beads_dir).unwrap_or_else(|_| batch.beads_dir.clone());
            let mut batch_cli = cli.clone();
            batch_cli.db = if normalized_batch_beads_dir == normalized_local_beads_dir {
                cli.db.clone()
            } else {
                None
            };

            let result = execute_undefer_route(
                &batch_args,
                &batch_cli,
                &batch.beads_dir,
                batch.is_external,
            )?;
            routed_outcomes.push((batch.issue_inputs.clone(), result.ordered_outcomes));
        }

        let ordered_outcomes = reorder_routed_items_by_requested_inputs(
            &args.ids,
            routed_outcomes,
            "undefer routing",
        )?;
        for outcome in ordered_outcomes {
            match outcome {
                DeferredOutcome::Changed(issue) => undeferred_issues.push(issue),
                DeferredOutcome::Skipped(issue) => skipped_issues.push(issue),
            }
        }
    } else {
        let result = execute_undefer_route(args, cli, &beads_dir, false)?;
        undeferred_issues = result.undeferred;
        skipped_issues = result.skipped;
    }

    if let Some(last_undeferred) = undeferred_issues.last() {
        crate::util::set_last_touched_id(&beads_dir, &last_undeferred.id);
    }

    render_undefer_output(&undeferred_issues, &skipped_issues, json, args, ctx)?;
    Ok(())
}

fn render_undefer_output(
    undeferred_issues: &[DeferredIssue],
    skipped_issues: &[SkippedIssue],
    json: bool,
    args: &UndeferArgs,
    ctx: &OutputContext,
) -> Result<()> {
    let use_structured_output = json || ctx.is_json() || ctx.is_toon() || args.robot;
    if use_structured_output {
        if skipped_issues.is_empty() {
            emit_structured_output(&undeferred_issues.to_vec(), ctx)?;
        } else {
            let result = UndeferResult {
                undeferred: undeferred_issues.to_vec(),
                skipped: skipped_issues.to_vec(),
                ordered_outcomes: Vec::new(),
            };
            emit_structured_output(&result, ctx)?;
        }
    } else if matches!(ctx.mode(), OutputMode::Quiet) {
        return Ok(());
    } else if matches!(ctx.mode(), OutputMode::Rich) {
        render_undefer_rich(undeferred_issues, skipped_issues, ctx);
    } else {
        for undeferred in undeferred_issues {
            println!(
                "\u{2713} Undeferred {}: {} (now {})",
                undeferred.id, undeferred.title, undeferred.status
            );
        }
        for skipped in skipped_issues {
            println!("\u{2298} Skipped {}: {}", skipped.id, skipped.reason);
        }
        if undeferred_issues.is_empty() && skipped_issues.is_empty() {
            println!("No issues to undefer.");
        }
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
fn execute_undefer_route(
    args: &UndeferArgs,
    cli: &config::CliOverrides,
    beads_dir: &Path,
    auto_flush_external: bool,
) -> Result<UndeferResult> {
    let mut storage_ctx = config::open_storage_with_cli(beads_dir, cli)?;

    let config_layer = storage_ctx.load_config(cli)?;
    let actor = config::resolve_actor(&config_layer);
    let id_config = config::id_config_from_layer(&config_layer);
    let resolver = IdResolver::new(ResolverConfig::with_prefix(id_config.prefix));
    let resolved_ids = resolve_issue_ids(&storage_ctx.storage, &resolver, &args.ids)?;

    let mut undeferred_issues: Vec<DeferredIssue> = Vec::new();
    let mut skipped_issues: Vec<SkippedIssue> = Vec::new();
    let mut ordered_outcomes = Vec::with_capacity(resolved_ids.len());
    let mut cache_dirty = false;

    for id in &resolved_ids {
        tracing::info!(id = %id, "Undeferring issue");

        let issue_result = storage_ctx.storage.get_issue(id);
        let Some(issue) = preserve_blocked_cache_on_error(
            &mut storage_ctx.storage,
            cache_dirty,
            "undefer",
            issue_result,
        )?
        else {
            let skipped = SkippedIssue {
                id: id.clone(),
                reason: "issue not found".to_string(),
            };
            ordered_outcomes.push(DeferredOutcome::Skipped(skipped.clone()));
            skipped_issues.push(skipped);
            continue;
        };

        if issue.status != Status::Deferred && issue.defer_until.is_none() {
            tracing::debug!(id = %id, status = ?issue.status, "Issue is not deferred");
            let skipped = SkippedIssue {
                id: id.clone(),
                reason: format!("not deferred (status: {})", issue.status.as_str()),
            };
            ordered_outcomes.push(DeferredOutcome::Skipped(skipped.clone()));
            skipped_issues.push(skipped);
            continue;
        }

        let restored_status = restored_status_after_undefer(&issue);
        let status_update = if issue.status == Status::Deferred {
            Some(Status::Open)
        } else {
            None
        };

        let update = IssueUpdate {
            status: status_update,
            defer_until: Some(None),
            skip_cache_rebuild: true,
            ..Default::default()
        };

        let update_result = update_issue_with_recovery(
            &mut storage_ctx,
            !cache_dirty,
            "undefer",
            id,
            &update,
            &actor,
        );
        preserve_blocked_cache_on_error(
            &mut storage_ctx.storage,
            cache_dirty,
            "undefer",
            update_result,
        )?;
        cache_dirty = true;
        tracing::info!(id = %id, "Issue undeferred");

        let undeferred = DeferredIssue {
            id: id.clone(),
            title: issue.title.clone(),
            previous_status: issue.status.as_str().to_string(),
            status: restored_status.as_str().to_string(),
            defer_until: None,
        };
        ordered_outcomes.push(DeferredOutcome::Changed(undeferred.clone()));
        undeferred_issues.push(undeferred);
    }

    if cache_dirty {
        tracing::info!(
            "Rebuilding blocked cache after undeferring {} issues",
            undeferred_issues.len()
        );
        finalize_batched_blocked_cache_refresh(&mut storage_ctx.storage, cache_dirty, "undefer")?;
    }

    storage_ctx.flush_no_db_if_dirty()?;
    if auto_flush_external && let Err(error) = storage_ctx.auto_flush_if_enabled() {
        tracing::debug!(
            beads_dir = %storage_ctx.paths.beads_dir.display(),
            error = %error,
            "Routed auto-flush failed (non-fatal)"
        );
    }

    Ok(UndeferResult {
        undeferred: undeferred_issues,
        skipped: skipped_issues,
        ordered_outcomes,
    })
}

fn emit_structured_output<T: Serialize>(payload: &T, ctx: &OutputContext) -> Result<()> {
    if ctx.is_toon() {
        ctx.toon(payload);
    } else if ctx.is_json() {
        ctx.json_pretty(payload);
    } else {
        let json = serde_json::to_string_pretty(payload)?;
        println!("{json}");
    }
    Ok(())
}

fn reorder_routed_items_by_requested_inputs<T>(
    requested_inputs: &[String],
    routed_items: Vec<(Vec<String>, Vec<T>)>,
    context: &str,
) -> Result<Vec<T>> {
    let mut positions_by_input: HashMap<&str, VecDeque<usize>> = HashMap::new();
    for (index, input) in requested_inputs.iter().enumerate() {
        positions_by_input
            .entry(input.as_str())
            .or_default()
            .push_back(index);
    }

    let mut ordered_items: Vec<Option<T>> = (0..requested_inputs.len()).map(|_| None).collect();
    for (batch_inputs, batch_items) in routed_items {
        if batch_inputs.len() != batch_items.len() {
            return Err(BeadsError::Config(format!(
                "{context} produced mismatched issue/result counts"
            )));
        }

        for (input, item) in batch_inputs.into_iter().zip(batch_items) {
            let Some(index) = positions_by_input
                .get_mut(input.as_str())
                .and_then(VecDeque::pop_front)
            else {
                return Err(BeadsError::Config(format!(
                    "{context} returned unexpected issue input {input}"
                )));
            };
            ordered_items[index] = Some(item);
        }
    }

    ordered_items
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            item.ok_or_else(|| {
                BeadsError::Config(format!(
                    "{context} did not produce a result for {}",
                    requested_inputs[index]
                ))
            })
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────
// Rich Output Rendering
// ─────────────────────────────────────────────────────────────

/// Render defer results with rich formatting.
fn render_defer_rich(deferred: &[DeferredIssue], skipped: &[SkippedIssue], ctx: &OutputContext) {
    let console = Console::default();
    let theme = ctx.theme();
    let width = ctx.width();

    let mut content = Text::new("");

    if deferred.is_empty() && skipped.is_empty() {
        content.append("No issues to defer.\n");
    } else {
        for item in deferred {
            content.append_styled("\u{23f1} ", theme.warning.clone());
            content.append_styled("Deferred ", theme.warning.clone());
            content.append_styled(&item.id, theme.emphasis.clone());
            content.append(": ");
            content.append(&item.title);
            content.append("\n");
            content.append_styled("  Status: ", theme.dimmed.clone());
            content.append_styled(&item.previous_status, theme.success.clone());
            content.append(" \u{2192} ");
            content.append_styled("deferred", theme.warning.clone());
            content.append("\n");
            content.append_styled("  Until:  ", theme.dimmed.clone());
            if let Some(ref until) = item.defer_until {
                content.append_styled(until, theme.accent.clone());
            } else {
                content.append_styled("indefinitely", theme.dimmed.clone());
            }
            content.append("\n");
        }

        for item in skipped {
            content.append_styled("\u{2298} ", theme.dimmed.clone());
            content.append_styled("Skipped ", theme.dimmed.clone());
            content.append_styled(&item.id, theme.emphasis.clone());
            content.append(": ");
            content.append_styled(&item.reason, theme.dimmed.clone());
            content.append("\n");
        }
    }

    let title = if deferred.len() == 1 && skipped.is_empty() {
        "Issue Deferred"
    } else {
        "Defer Results"
    };

    let panel = Panel::from_rich_text(&content, width)
        .title(Text::styled(title, theme.panel_title.clone()))
        .box_style(theme.box_style);

    console.print_renderable(&panel);
}

/// Render undefer results with rich formatting.
fn render_undefer_rich(
    undeferred: &[DeferredIssue],
    skipped: &[SkippedIssue],
    ctx: &OutputContext,
) {
    let console = Console::default();
    let theme = ctx.theme();
    let width = ctx.width();

    let mut content = Text::new("");

    if undeferred.is_empty() && skipped.is_empty() {
        content.append("No issues to undefer.\n");
    } else {
        for item in undeferred {
            content.append_styled("\u{2713} ", theme.success.clone());
            content.append_styled("Undeferred ", theme.success.clone());
            content.append_styled(&item.id, theme.emphasis.clone());
            content.append(": ");
            content.append(&item.title);
            content.append("\n");
            content.append_styled("  Status: ", theme.dimmed.clone());
            content.append_styled(&item.previous_status, theme.warning.clone());
            if item.previous_status == item.status {
                content.append_styled(" (unchanged)", theme.dimmed.clone());
            } else {
                content.append(" \u{2192} ");
                content.append_styled(&item.status, theme.success.clone());
            }
            content.append("\n");
        }

        for item in skipped {
            content.append_styled("\u{2298} ", theme.dimmed.clone());
            content.append_styled("Skipped ", theme.dimmed.clone());
            content.append_styled(&item.id, theme.emphasis.clone());
            content.append(": ");
            content.append_styled(&item.reason, theme.dimmed.clone());
            content.append("\n");
        }
    }

    let title = if undeferred.len() == 1 && skipped.is_empty() {
        "Issue Undeferred"
    } else {
        "Undefer Results"
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
    use crate::storage::SqliteStorage;
    use chrono::{Datelike, Duration, Local, Utc};
    use std::sync::Mutex;
    use tempfile::TempDir;

    static TEST_DIR_LOCK: Mutex<()> = Mutex::new(());

    fn make_issue(id: &str, title: &str) -> Issue {
        let now = Utc::now();
        Issue {
            id: id.to_string(),
            title: title.to_string(),
            description: None,
            status: Status::Open,
            priority: Priority::MEDIUM,
            issue_type: IssueType::Task,
            created_at: now,
            updated_at: now,
            content_hash: None,
            design: None,
            acceptance_criteria: None,
            notes: None,
            assignee: None,
            owner: None,
            estimated_minutes: None,
            created_by: None,
            closed_at: None,
            close_reason: None,
            closed_by_session: None,
            due_at: None,
            defer_until: None,
            external_ref: None,
            source_system: None,
            source_repo: None,
            deleted_at: None,
            deleted_by: None,
            delete_reason: None,
            original_type: None,
            compaction_level: None,
            compacted_at: None,
            compacted_at_commit: None,
            original_size: None,
            sender: None,
            ephemeral: false,
            pinned: false,
            is_template: false,
            labels: vec![],
            dependencies: vec![],
            comments: vec![],
        }
    }

    #[test]
    fn test_parse_defer_time_rfc3339() {
        let result = parse_flexible_timestamp("2025-01-15T12:00:00Z", "defer_until").unwrap();
        assert_eq!(result.year(), 2025);
        assert_eq!(result.month(), 1);
        assert_eq!(result.day(), 15);
    }

    #[test]
    fn test_parse_defer_time_simple_date() {
        let result = parse_flexible_timestamp("2025-06-20", "defer_until").unwrap();
        assert_eq!(result.year(), 2025);
        assert_eq!(result.month(), 6);
        assert_eq!(result.day(), 20);
    }

    #[test]
    fn test_parse_defer_time_relative_hours() {
        let before = Utc::now();
        let result = parse_flexible_timestamp("+2h", "defer_until").unwrap();
        let after = Utc::now();

        // Result should be about 2 hours from now
        assert!(result > before + Duration::hours(1));
        assert!(result < after + Duration::hours(3));
    }

    #[test]
    fn test_parse_defer_time_relative_days() {
        let before = Utc::now();
        let result = parse_flexible_timestamp("+1d", "defer_until").unwrap();
        let after = Utc::now();

        // Result should be about 1 day from now
        assert!(result > before + Duration::hours(23));
        assert!(result < after + Duration::hours(25));
    }

    #[test]
    fn test_parse_defer_time_relative_weeks() {
        let before = Utc::now();
        let result = parse_flexible_timestamp("+1w", "defer_until").unwrap();
        let after = Utc::now();

        // Result should be about 1 week from now
        assert!(result > before + Duration::days(6));
        assert!(result < after + Duration::days(8));
    }

    #[test]
    fn test_parse_defer_time_tomorrow() {
        let result = parse_flexible_timestamp("tomorrow", "defer_until").unwrap();
        let expected_date = Local::now().date_naive() + Duration::days(1);

        // Check it's tomorrow (in UTC, might differ by a day due to timezone)
        let result_local = result.with_timezone(&Local);
        assert_eq!(result_local.date_naive(), expected_date);
    }

    #[test]
    fn test_parse_defer_time_next_week() {
        let result = parse_flexible_timestamp("next-week", "defer_until").unwrap();
        let expected_date = Local::now().date_naive() + Duration::weeks(1);

        let result_local = result.with_timezone(&Local);
        assert_eq!(result_local.date_naive(), expected_date);
    }

    #[test]
    fn test_parse_defer_time_invalid() {
        let result = parse_flexible_timestamp("invalid-time", "defer_until");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_defer_time_minutes() {
        let before = Utc::now();
        let result = parse_flexible_timestamp("+30m", "defer_until").unwrap();
        let after = Utc::now();

        // Result should be about 30 minutes from now
        assert!(result > before + Duration::minutes(29));
        assert!(result < after + Duration::minutes(31));
    }

    #[test]
    fn test_parse_defer_time_negative() {
        let before = Utc::now();
        let result = parse_flexible_timestamp("-1d", "defer_until").unwrap();
        let after = Utc::now();

        // Result should be about 1 day ago
        assert!(result < before - Duration::hours(23));
        assert!(result > after - Duration::hours(25));
    }

    #[test]
    fn execute_defer_sets_status_and_until() {
        let _lock = TEST_DIR_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp = TempDir::new().expect("tempdir");
        let ctx = OutputContext::from_flags(false, false, true);
        commands::init::execute(None, false, Some(temp.path()), &ctx).expect("init");

        let beads_dir = temp.path().join(".beads");
        let mut storage = SqliteStorage::open(&beads_dir.join("beads.db")).expect("storage");
        let issue_id = format!(
            "{}-defer-1",
            storage
                .get_config("issue_prefix")
                .expect("prefix config")
                .expect("workspace prefix")
        );
        let issue = make_issue(&issue_id, "Defer me");
        storage.create_issue(&issue, "tester").expect("create");

        let cli = CliOverrides {
            db: Some(beads_dir.join("beads.db")),
            ..CliOverrides::default()
        };
        let args = DeferArgs {
            ids: vec![issue_id.clone()],
            until: Some("+1d".to_string()),
            robot: true,
        };
        execute_defer(&args, true, &cli, &ctx).expect("defer");

        let updated = storage.get_issue(&issue_id).expect("get").unwrap();
        assert_eq!(updated.status, Status::Deferred);
        assert!(updated.defer_until.is_some());
    }

    #[test]
    fn execute_defer_without_until_sets_indefinite() {
        let _lock = TEST_DIR_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp = TempDir::new().expect("tempdir");
        let ctx = OutputContext::from_flags(false, false, true);
        commands::init::execute(None, false, Some(temp.path()), &ctx).expect("init");

        let beads_dir = temp.path().join(".beads");
        let mut storage = SqliteStorage::open(&beads_dir.join("beads.db")).expect("storage");
        let issue_id = format!(
            "{}-defer-2",
            storage
                .get_config("issue_prefix")
                .expect("prefix config")
                .expect("workspace prefix")
        );
        let issue = make_issue(&issue_id, "Defer me later");
        storage.create_issue(&issue, "tester").expect("create");

        let cli = CliOverrides {
            db: Some(beads_dir.join("beads.db")),
            ..CliOverrides::default()
        };
        let args = DeferArgs {
            ids: vec![issue_id.clone()],
            until: None,
            robot: true,
        };
        execute_defer(&args, true, &cli, &ctx).expect("defer");

        let updated = storage.get_issue(&issue_id).expect("get").unwrap();
        assert_eq!(updated.status, Status::Deferred);
        assert!(updated.defer_until.is_none());
    }

    #[test]
    fn execute_undefer_clears_defer_until() {
        let _lock = TEST_DIR_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp = TempDir::new().expect("tempdir");
        let ctx = OutputContext::from_flags(false, false, true);
        commands::init::execute(None, false, Some(temp.path()), &ctx).expect("init");

        let beads_dir = temp.path().join(".beads");
        let issue_id = {
            let storage = SqliteStorage::open(&beads_dir.join("beads.db")).expect("storage");
            format!(
                "{}-defer-3",
                storage
                    .get_config("issue_prefix")
                    .expect("prefix config")
                    .expect("workspace prefix")
            )
        };
        {
            let mut storage = SqliteStorage::open(&beads_dir.join("beads.db")).expect("storage");
            let issue = make_issue(&issue_id, "Undefer me");
            storage.create_issue(&issue, "tester").expect("create");
        }

        let cli = CliOverrides {
            db: Some(beads_dir.join("beads.db")),
            ..CliOverrides::default()
        };
        let defer_args = DeferArgs {
            ids: vec![issue_id.clone()],
            until: Some("+1d".to_string()),
            robot: true,
        };
        execute_defer(&defer_args, true, &cli, &ctx).expect("defer");

        let undefer_args = UndeferArgs {
            ids: vec![issue_id.clone()],
            robot: true,
        };
        execute_undefer(&undefer_args, true, &cli, &ctx).expect("undefer");

        let storage = SqliteStorage::open(&beads_dir.join("beads.db")).expect("reopen");
        let updated = storage.get_issue(&issue_id).expect("get").unwrap();
        assert_eq!(updated.status, Status::Open);
        assert!(updated.defer_until.is_none());
    }

    #[test]
    fn execute_undefer_preserves_non_deferred_status_for_soft_defer() {
        let _lock = TEST_DIR_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp = TempDir::new().expect("tempdir");
        let ctx = OutputContext::from_flags(false, false, true);
        commands::init::execute(None, false, Some(temp.path()), &ctx).expect("init");

        let beads_dir = temp.path().join(".beads");
        let mut storage = SqliteStorage::open(&beads_dir.join("beads.db")).expect("storage");
        let issue_id = format!(
            "{}-soft-defer-1",
            storage
                .get_config("issue_prefix")
                .expect("prefix config")
                .expect("workspace prefix")
        );
        let mut issue = make_issue(&issue_id, "Soft defer in progress");
        issue.status = Status::InProgress;
        issue.defer_until = Some(Utc::now() + Duration::days(1));
        storage.create_issue(&issue, "tester").expect("create");

        let cli = CliOverrides {
            db: Some(beads_dir.join("beads.db")),
            ..CliOverrides::default()
        };
        let undefer_args = UndeferArgs {
            ids: vec![issue_id.clone()],
            robot: true,
        };
        execute_undefer(&undefer_args, true, &cli, &ctx).expect("undefer");

        let updated = storage.get_issue(&issue_id).expect("get").unwrap();
        assert_eq!(updated.status, Status::InProgress);
        assert!(updated.defer_until.is_none());
    }
}
