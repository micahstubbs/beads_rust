//! Update command implementation.

use super::resolve_issue_id;
use crate::cli::UpdateArgs;
use crate::config;
use crate::error::{BeadsError, Result};
use crate::model::{Issue, Status};
use crate::output::OutputContext;
use crate::storage::{IssueUpdate, SqliteStorage};
use crate::util::id::{IdResolver, ResolverConfig};
use crate::util::time::parse_flexible_timestamp;
use crate::validation::LabelValidator;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::Path;

/// JSON output structure for updated issues.
#[derive(Serialize)]
struct UpdatedIssueOutput {
    id: String,
    title: String,
    status: String,
    priority: i32,
    updated_at: DateTime<Utc>,
}

impl From<&Issue> for UpdatedIssueOutput {
    fn from(issue: &Issue) -> Self {
        Self {
            id: issue.id.clone(),
            title: issue.title.clone(),
            status: issue.status.as_str().to_string(),
            priority: issue.priority.0,
            updated_at: issue.updated_at,
        }
    }
}

enum UpdateRenderItem {
    Summary {
        id: String,
        title: String,
        before: Box<Option<Issue>>,
        after: Box<Issue>,
    },
    NoUpdates {
        id: String,
    },
}

struct UpdateRouteOutput {
    updated_issues: Vec<UpdatedIssueOutput>,
    render_items: Vec<UpdateRenderItem>,
    resolved_ids: Vec<String>,
}

enum ParentUpdatePlan {
    Unchanged,
    Clear,
    Set(String),
}

struct PreparedUpdateRoute {
    storage_ctx: config::OpenStorageResult,
    actor: String,
    resolved_ids: Vec<String>,
    update: IssueUpdate,
    has_updates: bool,
    add_labels: Vec<String>,
    remove_labels: Vec<String>,
    set_labels: bool,
    valid_set_labels: Vec<String>,
    resolved_parent: ParentUpdatePlan,
}

/// Execute the update command.
///
/// # Errors
///
/// Returns an error if database operations fail or validation errors occur.
pub fn execute(args: &UpdateArgs, cli: &config::CliOverrides, ctx: &OutputContext) -> Result<()> {
    let beads_dir = config::discover_beads_dir_with_cli(cli)?;
    let mut target_inputs = args.ids.clone();
    if target_inputs.is_empty() {
        let last_touched = crate::util::get_last_touched_id(&beads_dir);
        if last_touched.is_empty() {
            return Err(BeadsError::validation(
                "ids",
                "no issue IDs provided and no last-touched issue",
            ));
        }
        target_inputs.push(last_touched);
    }

    let routed_batches = config::routing::group_issue_inputs_by_route(&target_inputs, &beads_dir)?;

    let (updated_issues, render_items, ordered_resolved_ids) =
        if routed_batches.iter().any(|batch| batch.is_external) {
            let normalized_local_beads_dir =
                dunce::canonicalize(&beads_dir).unwrap_or_else(|_| beads_dir.clone());
            let mut prepared_routes = Vec::new();
            let mut routed_updated_issues = Vec::new();
            let mut routed_render_items = Vec::new();
            let mut routed_resolved_ids = Vec::new();
            for batch in routed_batches {
                let mut batch_args = args.clone();
                batch_args.ids.clone_from(&batch.issue_inputs);

                let normalized_batch_beads_dir = dunce::canonicalize(&batch.beads_dir)
                    .unwrap_or_else(|_| batch.beads_dir.clone());
                let mut batch_cli = cli.clone();
                // Routed projects must resolve their own metadata-defined DB path
                // instead of being forced back to the local override. Preserve the
                // caller's explicit DB only for the local batch.
                batch_cli.db = if normalized_batch_beads_dir == normalized_local_beads_dir {
                    cli.db.clone()
                } else {
                    None
                };
                prepared_routes.push((
                    batch.issue_inputs.clone(),
                    prepare_single_route(&batch_args, &batch_cli, &batch.beads_dir)?,
                ));
            }

            for (issue_inputs, prepared_route) in prepared_routes {
                let route_output = execute_prepared_route(prepared_route, ctx)?;

                if ctx.is_json() || ctx.is_toon() {
                    routed_updated_issues.push((issue_inputs.clone(), route_output.updated_issues));
                } else if !ctx.is_quiet() {
                    routed_render_items.push((issue_inputs.clone(), route_output.render_items));
                }
                routed_resolved_ids.push((issue_inputs, route_output.resolved_ids));
            }

            let updated_issues = if ctx.is_json() || ctx.is_toon() {
                reorder_routed_items_by_requested_inputs(
                    &target_inputs,
                    routed_updated_issues,
                    "update routing",
                )?
            } else {
                Vec::new()
            };
            let render_items = if !ctx.is_quiet() && !ctx.is_json() && !ctx.is_toon() {
                reorder_routed_items_by_requested_inputs(
                    &target_inputs,
                    routed_render_items,
                    "update routing",
                )?
            } else {
                Vec::new()
            };
            let ordered_resolved_ids = reorder_routed_items_by_requested_inputs(
                &target_inputs,
                routed_resolved_ids,
                "update routing",
            )?;
            (updated_issues, render_items, ordered_resolved_ids)
        } else {
            let route_output =
                execute_prepared_route(prepare_single_route(args, cli, &beads_dir)?, ctx)?;
            (
                route_output.updated_issues,
                route_output.render_items,
                route_output.resolved_ids,
            )
        };

    if let Some(last_id) = ordered_resolved_ids.last() {
        crate::util::set_last_touched_id(&beads_dir, last_id);
    }

    if ctx.is_toon() {
        ctx.toon(&updated_issues);
    } else if ctx.is_json() {
        ctx.json_pretty(&updated_issues);
    } else if !ctx.is_quiet() {
        print_render_items(&render_items);
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
fn prepare_single_route(
    args: &UpdateArgs,
    cli: &config::CliOverrides,
    beads_dir: &Path,
) -> Result<PreparedUpdateRoute> {
    let storage_ctx = config::open_storage_with_cli(beads_dir, cli)?;

    let config_layer = storage_ctx.load_config(cli)?;
    let actor = config::resolve_actor(&config_layer);
    let resolver = build_resolver(&config_layer, &storage_ctx.storage);
    let all_ids = storage_ctx.storage.get_all_ids()?;
    let resolved_ids =
        resolve_target_ids(args, beads_dir, &resolver, &storage_ctx.storage, &all_ids)?;

    let claim_exclusive = config::claim_exclusive_from_layer(&config_layer);
    let update = build_update(args, &actor, claim_exclusive)?;
    let has_updates = !update.is_empty()
        || !args.add_label.is_empty()
        || !args.remove_label.is_empty()
        || !args.set_labels.is_empty()
        || args.parent.is_some();

    validate_mutable_target_issues(&storage_ctx.storage, &resolved_ids, has_updates)?;

    // Validate labels before making any database changes
    for label in &args.add_label {
        LabelValidator::validate(label).map_err(|e| BeadsError::validation("label", e.message))?;
    }

    let mut valid_set_labels = Vec::new();
    if !args.set_labels.is_empty() {
        let combined = args.set_labels.join(",");
        for label in combined.split(',') {
            let label = label.trim();
            if !label.is_empty() {
                LabelValidator::validate(label)
                    .map_err(|e| BeadsError::validation("label", e.message))?;
                valid_set_labels.push(label.to_string());
            }
        }
    }

    let resolved_parent = resolve_parent_update(
        args.parent.as_deref(),
        &resolver,
        &storage_ctx.storage,
        &all_ids,
    )?;
    validate_parent_updates(&storage_ctx.storage, &resolved_ids, &resolved_parent)?;

    validate_transition_to_in_progress(&storage_ctx.storage, &resolved_ids, args)?;

    Ok(PreparedUpdateRoute {
        storage_ctx,
        actor,
        resolved_ids,
        update,
        has_updates,
        add_labels: args.add_label.clone(),
        remove_labels: args.remove_label.clone(),
        set_labels: !args.set_labels.is_empty(),
        valid_set_labels,
        resolved_parent,
    })
}

#[allow(clippy::too_many_lines)]
fn execute_prepared_route(
    mut prepared: PreparedUpdateRoute,
    ctx: &OutputContext,
) -> Result<UpdateRouteOutput> {
    let mut updated_issues: Vec<UpdatedIssueOutput> = Vec::new();
    let mut render_items = Vec::new();
    let resolved_ids = prepared.resolved_ids.clone();
    let storage = &mut prepared.storage_ctx.storage;

    for id in &prepared.resolved_ids {
        // Get issue before update for change tracking
        let issue_before = storage.get_issue(id)?;

        // Apply basic field updates
        if !prepared.update.is_empty() {
            storage.update_issue(id, &prepared.update, &prepared.actor)?;
        }

        // Apply labels
        for label in &prepared.add_labels {
            storage.add_label(id, label, &prepared.actor)?;
        }
        for label in &prepared.remove_labels {
            storage.remove_label(id, label, &prepared.actor)?;
        }
        if prepared.set_labels {
            storage.set_labels(id, &prepared.valid_set_labels, &prepared.actor)?;
        }

        // Apply parent
        apply_parent_update(storage, id, &prepared.resolved_parent, &prepared.actor)?;

        // Get issue after update for output
        let issue_after = storage.get_issue(id)?;

        if let Some(issue) = issue_after {
            if ctx.is_json() || ctx.is_toon() {
                updated_issues.push(UpdatedIssueOutput::from(&issue));
            } else if ctx.is_quiet() {
            } else if prepared.has_updates {
                render_items.push(UpdateRenderItem::Summary {
                    id: id.clone(),
                    title: issue.title.clone(),
                    before: Box::new(issue_before),
                    after: Box::new(issue),
                });
            } else {
                render_items.push(UpdateRenderItem::NoUpdates { id: id.clone() });
            }
        }
    }

    prepared.storage_ctx.flush_no_db_if_dirty()?;

    Ok(UpdateRouteOutput {
        updated_issues,
        render_items,
        resolved_ids,
    })
}

fn validate_transition_to_in_progress(
    storage: &SqliteStorage,
    ids: &[String],
    args: &UpdateArgs,
) -> Result<()> {
    let transitioning_to_in_progress = args.claim
        || args
            .status
            .as_ref()
            .is_some_and(|status| status.eq_ignore_ascii_case("in_progress"));

    if !transitioning_to_in_progress || args.force {
        return Ok(());
    }

    for id in ids {
        if storage.is_blocked(id)? {
            let blockers = storage.get_blockers(id)?;
            let blocker_list = if blockers.is_empty() {
                "blocking dependencies".to_string()
            } else {
                blockers.join(", ")
            };
            return Err(BeadsError::validation(
                "claim",
                format!("cannot claim blocked issue: {blocker_list}"),
            ));
        }
    }

    Ok(())
}

/// Print a summary of what changed for the issue.
fn print_update_summary(id: &str, title: &str, before: Option<&Issue>, after: &Issue) {
    println!("Updated {id}: {title}");

    if let Some(before) = before {
        // Status change
        if before.status != after.status {
            println!(
                "  status: {} → {}",
                before.status.as_str(),
                after.status.as_str()
            );
        }
        // Priority change
        if before.priority != after.priority {
            println!("  priority: P{} → P{}", before.priority.0, after.priority.0);
        }
        // Type change
        if before.issue_type != after.issue_type {
            println!(
                "  type: {} → {}",
                before.issue_type.as_str(),
                after.issue_type.as_str()
            );
        }
        // Assignee change
        if before.assignee != after.assignee {
            let before_assignee = before.assignee.as_deref().unwrap_or("(none)");
            let after_assignee = after.assignee.as_deref().unwrap_or("(none)");
            println!("  assignee: {before_assignee} → {after_assignee}");
        }
        // Owner change
        if before.owner != after.owner {
            let before_owner = before.owner.as_deref().unwrap_or("(none)");
            let after_owner = after.owner.as_deref().unwrap_or("(none)");
            println!("  owner: {before_owner} → {after_owner}");
        }
    }
}

fn print_render_items(render_items: &[UpdateRenderItem]) {
    for item in render_items {
        match item {
            UpdateRenderItem::Summary {
                id,
                title,
                before,
                after,
            } => print_update_summary(id, title, before.as_ref().as_ref(), after.as_ref()),
            UpdateRenderItem::NoUpdates { id } => println!("No updates specified for {id}"),
        }
    }
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

fn build_resolver(config_layer: &config::ConfigLayer, _storage: &SqliteStorage) -> IdResolver {
    let id_config = config::id_config_from_layer(config_layer);
    IdResolver::new(ResolverConfig::with_prefix(id_config.prefix))
}

fn resolve_target_ids(
    args: &UpdateArgs,
    beads_dir: &std::path::Path,
    resolver: &IdResolver,
    storage: &SqliteStorage,
    all_ids: &[String],
) -> Result<Vec<String>> {
    let mut ids = args.ids.clone();
    if ids.is_empty() {
        let last_touched = crate::util::get_last_touched_id(beads_dir);
        if last_touched.is_empty() {
            return Err(BeadsError::validation(
                "ids",
                "no issue IDs provided and no last-touched issue",
            ));
        }
        ids.push(last_touched);
    }

    let resolved_ids = resolver.resolve_all_fallible(
        &ids,
        |id| storage.id_exists(id),
        |hash| Ok(crate::util::id::find_matching_ids(all_ids, hash)),
    )?;

    Ok(resolved_ids.into_iter().map(|r| r.id).collect())
}

fn validate_mutable_target_issues(
    storage: &SqliteStorage,
    ids: &[String],
    has_updates: bool,
) -> Result<()> {
    if !has_updates {
        return Ok(());
    }

    for id in ids {
        if storage
            .get_issue(id)?
            .as_ref()
            .is_some_and(|issue| issue.status == Status::Tombstone)
        {
            return Err(BeadsError::validation(
                "issue",
                format!("cannot update tombstone issue: {id}"),
            ));
        }
    }

    Ok(())
}

fn build_update(args: &UpdateArgs, actor: &str, claim_exclusive: bool) -> Result<IssueUpdate> {
    let status = if args.claim {
        Some(Status::InProgress)
    } else {
        args.status.as_ref().map(|s| s.parse()).transpose()?
    };

    let priority = args.priority.as_ref().map(|p| p.parse()).transpose()?;

    let issue_type = args.type_.as_ref().map(|t| t.parse()).transpose()?;

    let assignee = if args.claim {
        Some(Some(actor.to_string()))
    } else {
        optional_string_field(args.assignee.as_deref())
    };

    let owner = optional_string_field(args.owner.as_deref());
    let due_at = optional_date_field(args.due.as_deref())?;
    let defer_until = optional_date_field(args.defer.as_deref())?;

    let closed_at = match &status {
        Some(Status::Closed | Status::Tombstone) => Some(Some(Utc::now())),
        Some(_) => Some(None),
        None => None,
    };

    // Build update struct
    Ok(IssueUpdate {
        title: args.title.clone(),
        description: args.description.clone().map(Some),
        design: args.design.clone().map(Some),
        acceptance_criteria: args.acceptance_criteria.clone().map(Some),
        notes: args.notes.clone().map(Some),
        status,
        priority,
        issue_type,
        assignee,
        owner,
        estimated_minutes: args.estimate.map(Some),
        due_at,
        defer_until,
        external_ref: optional_string_field(args.external_ref.as_deref()),
        closed_at,
        close_reason: None,
        closed_by_session: args.session.clone().map(Some),
        deleted_at: None,
        deleted_by: None,
        delete_reason: None,
        skip_cache_rebuild: false,
        expect_unassigned: args.claim,
        claim_exclusive: args.claim && claim_exclusive,
        claim_actor: if args.claim {
            Some(actor.to_string())
        } else {
            None
        },
    })
}

#[allow(clippy::option_option, clippy::single_option_map)]
fn optional_string_field(value: Option<&str>) -> Option<Option<String>> {
    value.map(|v| {
        if v.is_empty() {
            None
        } else {
            Some(v.to_string())
        }
    })
}

#[allow(clippy::option_option)]
fn optional_date_field(value: Option<&str>) -> Result<Option<Option<DateTime<Utc>>>> {
    value
        .map(|v| {
            if v.is_empty() {
                Ok(None)
            } else {
                parse_date(v).map(Some)
            }
        })
        .transpose()
}

fn resolve_parent_update(
    parent: Option<&str>,
    resolver: &IdResolver,
    storage: &SqliteStorage,
    all_ids: &[String],
) -> Result<ParentUpdatePlan> {
    match parent {
        None => Ok(ParentUpdatePlan::Unchanged),
        Some("") => Ok(ParentUpdatePlan::Clear),
        Some(parent_value) => {
            resolve_issue_id(storage, resolver, all_ids, parent_value).map(ParentUpdatePlan::Set)
        }
    }
}

fn apply_parent_update(
    storage: &mut SqliteStorage,
    issue_id: &str,
    parent: &ParentUpdatePlan,
    actor: &str,
) -> Result<()> {
    match parent {
        ParentUpdatePlan::Unchanged => Ok(()),
        ParentUpdatePlan::Clear => storage.set_parent(issue_id, None, actor),
        ParentUpdatePlan::Set(parent_id) => storage.set_parent(issue_id, Some(parent_id), actor),
    }
}

fn validate_parent_updates(
    storage: &SqliteStorage,
    issue_ids: &[String],
    parent: &ParentUpdatePlan,
) -> Result<()> {
    let ParentUpdatePlan::Set(parent_id) = parent else {
        return Ok(());
    };

    for issue_id in issue_ids {
        if issue_id == parent_id {
            return Err(BeadsError::SelfDependency {
                id: issue_id.clone(),
            });
        }

        if storage.would_create_cycle(issue_id, parent_id, true)? {
            return Err(BeadsError::DependencyCycle {
                path: format!("Setting parent of {issue_id} to {parent_id} would create a cycle"),
            });
        }
    }

    Ok(())
}

fn parse_date(s: &str) -> Result<DateTime<Utc>> {
    parse_flexible_timestamp(s, "date")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::init_test_logging;
    use crate::model::{Issue, IssueType, Priority, Status};
    use crate::storage::SqliteStorage;
    use chrono::{Datelike, Timelike};
    use tracing::info;

    #[test]
    fn test_optional_string_field_with_value() {
        init_test_logging();
        info!("test_optional_string_field_with_value: starting");
        let result = optional_string_field(Some("test"));
        assert_eq!(result, Some(Some("test".to_string())));
        info!("test_optional_string_field_with_value: assertions passed");
    }

    #[test]
    fn test_optional_string_field_with_empty() {
        init_test_logging();
        info!("test_optional_string_field_with_empty: starting");
        let result = optional_string_field(Some(""));
        assert_eq!(result, Some(None));
        info!("test_optional_string_field_with_empty: assertions passed");
    }

    #[test]
    fn test_optional_string_field_with_none() {
        init_test_logging();
        info!("test_optional_string_field_with_none: starting");
        let result = optional_string_field(None);
        assert_eq!(result, None);
        info!("test_optional_string_field_with_none: assertions passed");
    }

    #[test]
    fn test_optional_date_field_with_valid() {
        init_test_logging();
        info!("test_optional_date_field_with_valid: starting");
        let result = optional_date_field(Some("2024-01-15T12:00:00Z")).unwrap();
        assert!(result.is_some());
        let date = result.unwrap().unwrap();
        assert_eq!(date.year(), 2024);
        assert_eq!(date.month(), 1);
        assert_eq!(date.day(), 15);
        info!("test_optional_date_field_with_valid: assertions passed");
    }

    #[test]
    fn test_optional_date_field_with_empty() {
        init_test_logging();
        info!("test_optional_date_field_with_empty: starting");
        let result = optional_date_field(Some("")).unwrap();
        assert_eq!(result, Some(None));
        info!("test_optional_date_field_with_empty: assertions passed");
    }

    #[test]
    fn test_optional_date_field_with_none() {
        init_test_logging();
        info!("test_optional_date_field_with_none: starting");
        let result = optional_date_field(None).unwrap();
        assert_eq!(result, None);
        info!("test_optional_date_field_with_none: assertions passed");
    }

    #[test]
    fn test_optional_date_field_invalid_format() {
        init_test_logging();
        info!("test_optional_date_field_invalid_format: starting");
        let result = optional_date_field(Some("not-a-date"));
        assert!(result.is_err());
        info!("test_optional_date_field_invalid_format: assertions passed");
    }

    #[test]
    fn test_parse_date_valid_rfc3339() {
        init_test_logging();
        info!("test_parse_date_valid_rfc3339: starting");
        let result = parse_date("2024-06-15T10:30:00+00:00").unwrap();
        assert_eq!(result.year(), 2024);
        assert_eq!(result.month(), 6);
        assert_eq!(result.day(), 15);
        info!("test_parse_date_valid_rfc3339: assertions passed");
    }

    #[test]
    fn test_parse_date_with_timezone() {
        init_test_logging();
        info!("test_parse_date_with_timezone: starting");
        let result = parse_date("2024-12-25T08:00:00-05:00").unwrap();
        // Should be converted to UTC
        assert_eq!(result.year(), 2024);
        assert_eq!(result.month(), 12);
        assert_eq!(result.day(), 25);
        assert_eq!(result.hour(), 13); // 8:00 EST = 13:00 UTC
        info!("test_parse_date_with_timezone: assertions passed");
    }

    #[test]
    fn test_parse_date_invalid() {
        init_test_logging();
        info!("test_parse_date_invalid: starting");
        let result = parse_date("invalid");
        assert!(result.is_err());
        info!("test_parse_date_invalid: assertions passed");
    }

    #[test]
    fn test_parse_date_partial_date() {
        init_test_logging();
        info!("test_parse_date_partial_date: starting");
        // Partial dates without time should now succeed
        let result = parse_date("2024-01-15");
        assert!(result.is_ok());
        let date = result.unwrap();
        assert_eq!(date.year(), 2024);
        assert_eq!(date.month(), 1);
        assert_eq!(date.day(), 15);
        info!("test_parse_date_partial_date: assertions passed");
    }

    #[test]
    fn test_build_update_with_claim() {
        init_test_logging();
        info!("test_build_update_with_claim: starting");
        let args = UpdateArgs {
            claim: true,
            ..Default::default()
        };
        let update = build_update(&args, "test_actor", false).unwrap();
        assert_eq!(update.status, Some(Status::InProgress));
        assert_eq!(update.assignee, Some(Some("test_actor".to_string())));
        info!("test_build_update_with_claim: assertions passed");
    }

    #[test]
    fn test_build_update_with_status() {
        init_test_logging();
        info!("test_build_update_with_status: starting");
        let args = UpdateArgs {
            status: Some("closed".to_string()),
            ..Default::default()
        };
        let update = build_update(&args, "test_actor", false).unwrap();
        assert_eq!(update.status, Some(Status::Closed));
        // closed_at should be set
        assert!(update.closed_at.is_some());

        let args_blocked = UpdateArgs {
            status: Some("blocked".to_string()),
            ..Default::default()
        };
        let update_blocked = build_update(&args_blocked, "test_actor", false).unwrap();
        assert_eq!(update_blocked.status, Some(Status::Blocked));
        // closed_at should be explicitly cleared for non-terminal statuses
        assert_eq!(update_blocked.closed_at, Some(None));
        info!("test_build_update_with_status: assertions passed");
    }

    #[test]
    fn test_build_update_with_priority() {
        init_test_logging();
        info!("test_build_update_with_priority: starting");
        let args = UpdateArgs {
            priority: Some("1".to_string()),
            ..Default::default()
        };
        let update = build_update(&args, "test_actor", false).unwrap();
        assert_eq!(update.priority, Some(Priority(1)));
        info!("test_build_update_with_priority: assertions passed");
    }

    #[test]
    fn test_build_update_empty() {
        init_test_logging();
        info!("test_build_update_empty: starting");
        let args = UpdateArgs::default();
        let update = build_update(&args, "test_actor", false).unwrap();
        assert!(update.is_empty());
        info!("test_build_update_empty: assertions passed");
    }

    #[test]
    fn test_validate_mutable_target_issues_rejects_tombstone() {
        init_test_logging();
        info!("test_validate_mutable_target_issues_rejects_tombstone: starting");

        let mut storage = SqliteStorage::open_memory().unwrap();
        let issue = Issue {
            id: "bd-tombstone".to_string(),
            title: "Deleted issue".to_string(),
            status: Status::Open,
            priority: Priority::MEDIUM,
            issue_type: IssueType::Task,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            ..Issue::default()
        };
        storage.create_issue(&issue, "tester").unwrap();
        storage
            .delete_issue("bd-tombstone", "tester", "delete for update test", None)
            .unwrap();

        let err = validate_mutable_target_issues(&storage, &["bd-tombstone".to_string()], true)
            .unwrap_err();

        match err {
            BeadsError::Validation { field, reason } => {
                assert_eq!(field, "issue");
                assert!(reason.contains("cannot update tombstone issue"));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        info!("test_validate_mutable_target_issues_rejects_tombstone: assertions passed");
    }

    #[test]
    fn test_validate_mutable_target_issues_allows_open_issue() {
        init_test_logging();
        info!("test_validate_mutable_target_issues_allows_open_issue: starting");

        let mut storage = SqliteStorage::open_memory().unwrap();
        let issue = Issue {
            id: "bd-open".to_string(),
            title: "Open issue".to_string(),
            status: Status::Open,
            priority: Priority::MEDIUM,
            issue_type: IssueType::Task,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            ..Issue::default()
        };
        storage.create_issue(&issue, "tester").unwrap();

        validate_mutable_target_issues(&storage, &["bd-open".to_string()], true).unwrap();

        info!("test_validate_mutable_target_issues_allows_open_issue: assertions passed");
    }
}
