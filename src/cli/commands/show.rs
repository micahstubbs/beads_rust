//! Show command implementation.

use crate::cli::{ShowArgs, resolve_output_format_basic_with_outer_mode};
use crate::config;
use crate::error::{BeadsError, Result};
use crate::format::{
    IssueDetails, IssueWithDependencyMetadata, format_priority_label, format_status_icon_colored,
};
use crate::model::{Dependency, Issue, Priority, Status};
use crate::output::{IssuePanel, OutputContext, OutputMode};
use crate::storage::SqliteStorage;
use crate::sync::read_issues_from_jsonl;
use crate::util::id::{IdResolver, ResolverConfig};
use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as FmtWrite;
use std::path::{Path, PathBuf};

/// Execute the show command.
///
/// # Errors
///
/// Returns an error if the database cannot be opened or issues are not found.
pub fn execute(
    args: &ShowArgs,
    _json: bool,
    cli: &config::CliOverrides,
    outer_ctx: &OutputContext,
) -> Result<()> {
    let beads_dir = config::discover_beads_dir_with_cli(cli)?;
    execute_routed(args, cli, outer_ctx, &beads_dir, None, None)
}

/// Execute show using storage that was already opened by the caller.
///
/// # Errors
///
/// Returns an error if issue resolution or rendering fails.
pub fn execute_with_storage(
    args: &ShowArgs,
    cli: &config::CliOverrides,
    outer_ctx: &OutputContext,
    beads_dir: &Path,
    storage: &SqliteStorage,
) -> Result<()> {
    execute_routed(args, cli, outer_ctx, beads_dir, Some(storage), None)
}

/// Execute show using the caller's preopened storage context.
///
/// # Errors
///
/// Returns an error if issue resolution or rendering fails.
pub fn execute_with_storage_ctx(
    args: &ShowArgs,
    cli: &config::CliOverrides,
    outer_ctx: &OutputContext,
    beads_dir: &Path,
    storage_ctx: &config::OpenStorageResult,
) -> Result<()> {
    execute_routed(args, cli, outer_ctx, beads_dir, None, Some(storage_ctx))
}

fn execute_routed(
    args: &ShowArgs,
    cli: &config::CliOverrides,
    outer_ctx: &OutputContext,
    beads_dir: &Path,
    preloaded_storage: Option<&SqliteStorage>,
    preloaded_storage_ctx: Option<&config::OpenStorageResult>,
) -> Result<()> {
    let target_ids = requested_target_ids(args, beads_dir)?;
    let routed_batches = config::routing::group_issue_inputs_by_route(&target_ids, beads_dir)?;
    if !routed_batches.iter().any(|batch| batch.is_external) {
        return execute_inner(
            args,
            cli,
            outer_ctx,
            beads_dir,
            preloaded_storage,
            preloaded_storage_ctx,
        );
    }

    let output_format = resolve_output_format_basic_with_outer_mode(
        args.format,
        outer_ctx.inherited_output_mode(),
        false,
    );
    let normalized_local_beads_dir =
        dunce::canonicalize(beads_dir).unwrap_or_else(|_| beads_dir.to_path_buf());

    if matches!(
        output_format,
        crate::cli::OutputFormat::Json | crate::cli::OutputFormat::Toon
    ) {
        let mut details_list = Vec::new();
        for batch in routed_batches {
            let mut batch_args = args.clone();
            batch_args.ids = batch.issue_inputs;

            let mut batch_cli = cli.clone();
            batch_cli.db = None;

            let batch_beads_dir = batch.beads_dir;
            let normalized_batch_beads_dir =
                dunce::canonicalize(&batch_beads_dir).unwrap_or_else(|_| batch_beads_dir.clone());
            let use_preloaded = normalized_batch_beads_dir == normalized_local_beads_dir;
            let (batch_details, _) = load_issue_details_for_route(
                &batch_args,
                &batch_cli,
                &batch_beads_dir,
                if use_preloaded {
                    preloaded_storage
                } else {
                    None
                },
                if use_preloaded {
                    preloaded_storage_ctx
                } else {
                    None
                },
            )?;
            details_list.extend(batch_details);
        }

        match output_format {
            crate::cli::OutputFormat::Json => outer_ctx.json_pretty(&details_list),
            crate::cli::OutputFormat::Toon => {
                outer_ctx.toon_with_stats(&details_list, args.stats);
            }
            crate::cli::OutputFormat::Text | crate::cli::OutputFormat::Csv => unreachable!(),
        }
        return Ok(());
    }

    for (index, batch) in routed_batches.into_iter().enumerate() {
        if index > 0 {
            println!();
        }

        let mut batch_args = args.clone();
        batch_args.ids = batch.issue_inputs;

        let mut batch_cli = cli.clone();
        batch_cli.db = None;

        let normalized_batch_beads_dir =
            dunce::canonicalize(&batch.beads_dir).unwrap_or_else(|_| batch.beads_dir.clone());
        let use_preloaded = normalized_batch_beads_dir == normalized_local_beads_dir;
        execute_inner(
            &batch_args,
            &batch_cli,
            outer_ctx,
            &batch.beads_dir,
            if use_preloaded {
                preloaded_storage
            } else {
                None
            },
            if use_preloaded {
                preloaded_storage_ctx
            } else {
                None
            },
        )?;
    }

    Ok(())
}

fn requested_target_ids(args: &ShowArgs, beads_dir: &Path) -> Result<Vec<String>> {
    let mut target_ids = args.ids.clone();
    if target_ids.is_empty() {
        let last_touched = crate::util::get_last_touched_id(beads_dir);
        if last_touched.is_empty() {
            return Err(BeadsError::validation(
                "ids",
                "no issue IDs provided and no last-touched issue",
            ));
        }
        target_ids.push(last_touched);
    }
    Ok(target_ids)
}

fn execute_inner(
    args: &ShowArgs,
    cli: &config::CliOverrides,
    outer_ctx: &OutputContext,
    beads_dir: &Path,
    preloaded_storage: Option<&SqliteStorage>,
    preloaded_storage_ctx: Option<&config::OpenStorageResult>,
) -> Result<()> {
    let (details_list, use_color) = load_issue_details_for_route(
        args,
        cli,
        beads_dir,
        preloaded_storage,
        preloaded_storage_ctx,
    )?;
    let output_format = resolve_output_format_basic_with_outer_mode(
        args.format,
        outer_ctx.inherited_output_mode(),
        false,
    );
    let quiet = cli.quiet.unwrap_or(false);
    let ctx = OutputContext::from_output_format(output_format, quiet, !use_color);

    if matches!(ctx.mode(), OutputMode::Quiet) {
        return Ok(());
    }
    match output_format {
        crate::cli::OutputFormat::Json => {
            ctx.json_pretty(&details_list);
        }
        crate::cli::OutputFormat::Toon => {
            ctx.toon_with_stats(&details_list, args.stats);
        }
        crate::cli::OutputFormat::Text | crate::cli::OutputFormat::Csv => {
            for (i, details) in details_list.iter().enumerate() {
                if i > 0 {
                    println!(); // Separate multiple issues
                }
                if matches!(ctx.mode(), OutputMode::Rich) {
                    let panel = IssuePanel::from_details(details, ctx.theme());
                    panel.print(&ctx, args.wrap);
                } else {
                    print_issue_details(details, use_color);
                }
            }
        }
    }

    Ok(())
}

fn load_issue_details_for_route(
    args: &ShowArgs,
    cli: &config::CliOverrides,
    beads_dir: &Path,
    preloaded_storage: Option<&SqliteStorage>,
    preloaded_storage_ctx: Option<&config::OpenStorageResult>,
) -> Result<(Vec<IssueDetails>, bool)> {
    let target_ids = requested_target_ids(args, beads_dir)?;

    if let Some(storage_ctx) = preloaded_storage_ctx {
        let config_layer = storage_ctx.load_config(cli)?;
        let use_color = config::should_use_color(&config_layer);
        let id_config = config::id_config_from_layer(&config_layer);
        let resolver = IdResolver::new(ResolverConfig::with_prefix(id_config.prefix));
        let external_db_paths = config::external_project_db_paths(&config_layer, beads_dir);
        let details_list = load_issue_details_from_storage(
            &target_ids,
            &resolver,
            &storage_ctx.storage,
            &external_db_paths,
        )?;
        return Ok((details_list, use_color));
    }

    let startup = config::load_startup_config_with_paths(beads_dir, cli.db.as_ref())?;
    let mut bootstrap_config = startup.merged_config.clone();
    bootstrap_config.merge_from(&cli.as_layer());
    let no_db = config::no_db_from_layer(&bootstrap_config).unwrap_or(false);
    let owned_storage_ctx = if no_db || preloaded_storage.is_some() {
        None
    } else {
        Some(config::open_storage_with_cli(beads_dir, cli)?)
    };
    let storage = preloaded_storage.unwrap_or_else(|| {
        &owned_storage_ctx
            .as_ref()
            .expect("show should have an open storage handle")
            .storage
    });
    let config_layer = if let Some(storage_ctx) = owned_storage_ctx.as_ref() {
        storage_ctx.load_config(cli)?
    } else {
        config::load_config(beads_dir, Some(storage), cli)?
    };
    let use_color = config::should_use_color(&config_layer);
    let id_config = config::id_config_from_layer(&config_layer);
    let resolver = IdResolver::new(ResolverConfig::with_prefix(id_config.prefix));
    let external_db_paths = config::external_project_db_paths(&config_layer, beads_dir);
    let details_list = if no_db {
        load_issue_details_from_jsonl(
            &target_ids,
            &resolver,
            &startup.paths.jsonl_path,
            &external_db_paths,
        )?
    } else {
        load_issue_details_from_storage(&target_ids, &resolver, storage, &external_db_paths)?
    };

    Ok((details_list, use_color))
}

fn load_issue_details_from_storage(
    target_ids: &[String],
    resolver: &IdResolver,
    storage: &SqliteStorage,
    external_db_paths: &HashMap<String, PathBuf>,
) -> Result<Vec<IssueDetails>> {
    let mut external_statuses: Option<HashMap<String, bool>> = None;
    let mut details_list = Vec::with_capacity(target_ids.len());

    for id_input in target_ids {
        let resolution = resolver.resolve_fallible(
            id_input,
            |id| storage.id_exists(id),
            |hash| storage.find_ids_by_hash(hash),
        )?;

        let Some(mut details) = storage.get_issue_details(&resolution.id, true, false, 10)? else {
            return Err(BeadsError::IssueNotFound { id: resolution.id });
        };

        if issue_details_have_external_dependencies(&details) {
            if external_statuses.is_none() {
                external_statuses =
                    Some(storage.resolve_external_dependency_statuses(external_db_paths, false)?);
            }
            if let Some(statuses) = external_statuses.as_ref() {
                apply_external_dependency_metadata(&mut details.dependencies, statuses);
                apply_external_dependency_metadata(&mut details.dependents, statuses);
            }
        }

        details_list.push(details);
    }

    Ok(details_list)
}

fn load_issue_details_from_jsonl(
    target_ids: &[String],
    resolver: &IdResolver,
    jsonl_path: &Path,
    external_db_paths: &HashMap<String, PathBuf>,
) -> Result<Vec<IssueDetails>> {
    let issues = read_issues_from_jsonl(jsonl_path)?;
    let mut issues_by_id = HashMap::with_capacity(issues.len());
    for issue in issues {
        issues_by_id.insert(issue.id.clone(), issue);
    }

    let mut details_list = Vec::with_capacity(target_ids.len());
    for id_input in target_ids {
        let resolution = resolver.resolve_fallible(
            id_input,
            |id| Ok(issues_by_id.contains_key(id)),
            |hash| Ok(find_ids_by_hash_in_memory(&issues_by_id, hash)),
        )?;
        let issue = issues_by_id
            .get(&resolution.id)
            .ok_or_else(|| BeadsError::IssueNotFound {
                id: resolution.id.clone(),
            })?;
        details_list.push(build_issue_details_from_jsonl(issue, &issues_by_id)?);
    }

    let external_ids = collect_external_dependency_ids(&details_list);
    if !external_ids.is_empty() {
        let statuses = SqliteStorage::resolve_external_dependency_statuses_for_ids(
            &external_ids,
            external_db_paths,
        );
        for details in &mut details_list {
            apply_external_dependency_metadata(&mut details.dependencies, &statuses);
            apply_external_dependency_metadata(&mut details.dependents, &statuses);
        }
    }

    Ok(details_list)
}

fn build_issue_details_from_jsonl(
    issue: &Issue,
    issues_by_id: &HashMap<String, Issue>,
) -> Result<IssueDetails> {
    let mut dependencies = issue
        .dependencies
        .iter()
        .map(|dep| dependency_metadata_from_jsonl(dep, issues_by_id, true))
        .collect::<Result<Vec<_>>>()?;
    dependencies.sort_by(|left, right| {
        left.1
            .cmp(&right.1)
            .then_with(|| right.2.cmp(&left.2))
            .then_with(|| left.0.id.cmp(&right.0.id))
    });

    let mut dependents = issues_by_id
        .values()
        .flat_map(|candidate| {
            candidate
                .dependencies
                .iter()
                .filter(move |dep| dep.depends_on_id == issue.id)
                .map(move |dep| (candidate, dep))
        })
        .map(|(candidate, dep)| {
            Ok((
                IssueWithDependencyMetadata {
                    id: candidate.id.clone(),
                    title: candidate.title.clone(),
                    status: candidate.status.clone(),
                    priority: candidate.priority,
                    dep_type: dep.dep_type.as_str().to_string(),
                },
                candidate.priority,
                candidate.created_at,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    dependents.sort_by(|left, right| {
        left.1
            .cmp(&right.1)
            .then_with(|| right.2.cmp(&left.2))
            .then_with(|| left.0.id.cmp(&right.0.id))
    });

    let mut issue_without_relations = issue.clone();
    let labels = issue_without_relations.labels.clone();
    let comments = issue_without_relations.comments.clone();
    issue_without_relations.labels.clear();
    issue_without_relations.dependencies.clear();
    issue_without_relations.comments.clear();

    Ok(IssueDetails {
        issue: issue_without_relations,
        labels,
        dependencies: dependencies.into_iter().map(|(item, _, _)| item).collect(),
        dependents: dependents.into_iter().map(|(item, _, _)| item).collect(),
        comments,
        events: Vec::new(),
        parent: issue
            .dependencies
            .iter()
            .rev()
            .find(|dep| dep.dep_type.as_str() == "parent-child")
            .map(|dep| dep.depends_on_id.clone()),
    })
}

fn dependency_metadata_from_jsonl(
    dep: &Dependency,
    issues_by_id: &HashMap<String, Issue>,
    allow_external_placeholder: bool,
) -> Result<(IssueWithDependencyMetadata, Priority, DateTime<Utc>)> {
    if let Some(target) = issues_by_id.get(&dep.depends_on_id) {
        return Ok((
            IssueWithDependencyMetadata {
                id: target.id.clone(),
                title: target.title.clone(),
                status: target.status.clone(),
                priority: target.priority,
                dep_type: dep.dep_type.as_str().to_string(),
            },
            target.priority,
            target.created_at,
        ));
    }

    if allow_external_placeholder && dep.depends_on_id.starts_with("external:") {
        return Ok((
            IssueWithDependencyMetadata {
                id: dep.depends_on_id.clone(),
                title: dep
                    .depends_on_id
                    .strip_prefix("external:")
                    .unwrap_or(&dep.depends_on_id)
                    .to_string(),
                status: Status::Blocked,
                priority: Priority::MEDIUM,
                dep_type: dep.dep_type.as_str().to_string(),
            },
            Priority::MEDIUM,
            dep.created_at,
        ));
    }

    Err(BeadsError::Config(format!(
        "dependency row references missing issue {}",
        dep.depends_on_id
    )))
}

fn find_ids_by_hash_in_memory(
    issues_by_id: &HashMap<String, Issue>,
    hash_suffix: &str,
) -> Vec<String> {
    issues_by_id
        .keys()
        .filter(|id| {
            id.split_once('-')
                .is_some_and(|(_, suffix)| suffix.contains(hash_suffix))
        })
        .cloned()
        .collect()
}

fn collect_external_dependency_ids(details_list: &[IssueDetails]) -> HashSet<String> {
    details_list
        .iter()
        .flat_map(|details| details.dependencies.iter().chain(details.dependents.iter()))
        .filter(|item| item.id.starts_with("external:"))
        .map(|item| item.id.clone())
        .collect()
}

fn issue_details_have_external_dependencies(details: &IssueDetails) -> bool {
    details
        .dependencies
        .iter()
        .chain(details.dependents.iter())
        .any(|item| item.id.starts_with("external:"))
}

fn apply_external_dependency_metadata(
    items: &mut [IssueWithDependencyMetadata],
    external_statuses: &HashMap<String, bool>,
) {
    for item in items {
        if !item.id.starts_with("external:") {
            continue;
        }

        let satisfied = external_statuses.get(&item.id).copied().unwrap_or(false);
        item.status = if satisfied {
            crate::model::Status::Closed
        } else {
            crate::model::Status::Blocked
        };

        let placeholder_title = item.id.strip_prefix("external:").unwrap_or(&item.id);
        if item.title.is_empty() || item.title == placeholder_title {
            item.title = format_external_dependency_title(&item.id, satisfied);
        }
    }
}

fn format_external_dependency_title(dep_id: &str, satisfied: bool) -> String {
    let prefix = if satisfied { "✓" } else { "⏳" };
    parse_external_dep_id(dep_id).map_or_else(
        || format!("{prefix} {dep_id}"),
        |(project, capability)| format!("{prefix} {project}:{capability}"),
    )
}

fn parse_external_dep_id(dep_id: &str) -> Option<(String, String)> {
    let mut parts = dep_id.splitn(3, ':');
    let prefix = parts.next()?;
    if prefix != "external" {
        return None;
    }
    let project = parts.next()?.to_string();
    let capability = parts.next()?.to_string();
    if project.is_empty() || capability.is_empty() {
        return None;
    }
    Some((project, capability))
}

fn print_issue_details(details: &IssueDetails, use_color: bool) {
    let output = format_issue_details(details, use_color);
    print!("{output}");
}

#[allow(clippy::too_many_lines)]
fn format_issue_details(details: &IssueDetails, use_color: bool) -> String {
    let mut output = String::new();
    let issue = &details.issue;
    let status_icon = format_status_icon_colored(&issue.status, use_color);
    let priority_label = format_priority_label(&issue.priority, use_color);
    let status_upper = issue.status.as_str().to_uppercase();

    // Match bd format: {status_icon} {id} · {title}   [● {priority} · {STATUS}]
    let _ = writeln!(
        output,
        "{} {} · {}   [● {} · {}]",
        status_icon, issue.id, issue.title, priority_label, status_upper
    );

    // Owner/Type line: Owner: {owner} · Type: {type}
    let owner = issue
        .owner
        .clone()
        .unwrap_or_else(|| std::env::var("USER").unwrap_or_else(|_| "unknown".to_string()));
    let _ = writeln!(
        output,
        "Owner: {} · Type: {}",
        owner,
        issue.issue_type.as_str()
    );

    // Created/Updated line
    let _ = writeln!(
        output,
        "Created: {} · Updated: {}",
        issue.created_at.format("%Y-%m-%d"),
        issue.updated_at.format("%Y-%m-%d")
    );

    if let Some(assignee) = &issue.assignee {
        let _ = writeln!(output, "Assignee: {assignee}");
    }

    if !details.labels.is_empty() {
        let _ = writeln!(output, "Labels: {}", details.labels.join(", "));
    }

    if let Some(ext_ref) = &issue.external_ref
        && !ext_ref.is_empty()
    {
        let _ = writeln!(output, "Ref: {ext_ref}");
    }

    if let Some(due) = &issue.due_at {
        let _ = writeln!(output, "Due: {}", due.format("%Y-%m-%d"));
    }

    if let Some(defer) = &issue.defer_until {
        let _ = writeln!(output, "Deferred until: {}", defer.format("%Y-%m-%d"));
    }

    if let Some(minutes) = issue.estimated_minutes
        && minutes > 0
    {
        let hours = minutes / 60;
        let remaining = minutes % 60;
        if hours > 0 && remaining > 0 {
            let _ = writeln!(output, "Estimate: {hours}h {remaining}m");
        } else if hours > 0 {
            let _ = writeln!(output, "Estimate: {hours}h");
        } else {
            let _ = writeln!(output, "Estimate: {remaining}m");
        }
    }

    if let Some(closed) = &issue.closed_at {
        let reason_str = issue.close_reason.as_deref().unwrap_or("closed");
        let _ = writeln!(
            output,
            "Closed: {} ({})",
            closed.format("%Y-%m-%d"),
            reason_str
        );
    }

    if let Some(desc) = &issue.description {
        output.push('\n');
        let _ = writeln!(output, "{desc}");
    }

    if let Some(design) = &issue.design
        && !design.is_empty()
    {
        output.push('\n');
        let _ = writeln!(output, "Design:");
        let _ = writeln!(output, "{design}");
    }

    if let Some(ac) = &issue.acceptance_criteria
        && !ac.is_empty()
    {
        output.push('\n');
        let _ = writeln!(output, "Acceptance Criteria:");
        let _ = writeln!(output, "{ac}");
    }

    if let Some(notes) = &issue.notes
        && !notes.is_empty()
    {
        output.push('\n');
        let _ = writeln!(output, "Notes:");
        let _ = writeln!(output, "{notes}");
    }

    if !details.dependencies.is_empty() {
        output.push('\n');
        let _ = writeln!(output, "Dependencies:");
        for dep in &details.dependencies {
            let _ = writeln!(output, "  -> {} ({}) - {}", dep.id, dep.dep_type, dep.title);
        }
    }

    if !details.dependents.is_empty() {
        output.push('\n');
        let _ = writeln!(output, "Dependents:");
        for dep in &details.dependents {
            let _ = writeln!(output, "  <- {} ({}) - {}", dep.id, dep.dep_type, dep.title);
        }
    }

    if !details.comments.is_empty() {
        output.push('\n');
        let _ = writeln!(output, "Comments:");
        for comment in &details.comments {
            let _ = writeln!(
                output,
                "  [{}] {}: {}",
                comment.created_at.format("%Y-%m-%d %H:%M UTC"),
                comment.author,
                comment.body
            );
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::{
        apply_external_dependency_metadata, build_issue_details_from_jsonl, format_issue_details,
    };
    use crate::format::{IssueDetails, IssueWithDependencyMetadata};
    use crate::model::{Comment, Dependency, DependencyType, Issue, IssueType, Priority, Status};
    use crate::storage::SqliteStorage;
    use crate::util::id::{IdResolver, ResolverConfig};
    use chrono::{TimeZone, Utc};
    use std::collections::HashMap;
    use tracing::info;

    fn init_logging() {
        crate::logging::init_test_logging();
    }

    fn make_test_issue(id: &str, title: &str) -> Issue {
        Issue {
            id: id.to_string(),
            content_hash: None,
            title: title.to_string(),
            description: Some("Test description".to_string()),
            design: None,
            acceptance_criteria: None,
            notes: None,
            status: Status::Open,
            priority: Priority::MEDIUM,
            issue_type: IssueType::Task,
            assignee: None,
            owner: None,
            estimated_minutes: None,
            created_at: Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
            created_by: None,
            updated_at: Utc.with_ymd_and_hms(2025, 1, 1, 0, 0, 0).unwrap(),
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
    fn test_show_retrieves_issue_by_id() {
        init_logging();
        info!("test_show_retrieves_issue_by_id: starting");
        let mut storage = SqliteStorage::open_memory().unwrap();

        let issue = make_test_issue("bd-001", "Test Issue");
        storage.create_issue(&issue, "tester").unwrap();

        let retrieved = storage.get_issue("bd-001").unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.id, "bd-001");
        assert_eq!(retrieved.title, "Test Issue");
        info!("test_show_retrieves_issue_by_id: assertions passed");
    }

    #[test]
    fn test_show_returns_none_for_missing_id() {
        init_logging();
        info!("test_show_returns_none_for_missing_id: starting");
        let storage = SqliteStorage::open_memory().unwrap();

        let retrieved = storage.get_issue("nonexistent").unwrap();
        assert!(retrieved.is_none());
        info!("test_show_returns_none_for_missing_id: assertions passed");
    }

    #[test]
    fn test_show_multiple_issues() {
        init_logging();
        info!("test_show_multiple_issues: starting");
        let mut storage = SqliteStorage::open_memory().unwrap();

        let issue1 = make_test_issue("bd-001", "First Issue");
        let issue2 = make_test_issue("bd-002", "Second Issue");
        storage.create_issue(&issue1, "tester").unwrap();
        storage.create_issue(&issue2, "tester").unwrap();

        let retrieved1 = storage.get_issue("bd-001").unwrap().unwrap();
        let retrieved2 = storage.get_issue("bd-002").unwrap().unwrap();

        assert_eq!(retrieved1.title, "First Issue");
        assert_eq!(retrieved2.title, "Second Issue");
        info!("test_show_multiple_issues: assertions passed");
    }

    #[test]
    fn test_issue_json_serialization() {
        init_logging();
        info!("test_issue_json_serialization: starting");
        let issue = make_test_issue("bd-001", "Test Issue");
        let json = serde_json::to_string_pretty(&issue).unwrap();

        assert!(json.contains("\"id\": \"bd-001\""));
        assert!(json.contains("\"title\": \"Test Issue\""));
        assert!(json.contains("\"status\": \"open\""));
        info!("test_issue_json_serialization: assertions passed");
    }

    #[test]
    fn test_issue_json_serialization_multiple() {
        init_logging();
        info!("test_issue_json_serialization_multiple: starting");
        let issues = vec![
            make_test_issue("bd-001", "First"),
            make_test_issue("bd-002", "Second"),
        ];

        let json = serde_json::to_string_pretty(&issues).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["id"], "bd-001");
        assert_eq!(parsed[1]["id"], "bd-002");
        info!("test_issue_json_serialization_multiple: assertions passed");
    }

    #[test]
    fn test_show_resolves_full_id() {
        init_logging();
        info!("test_show_resolves_full_id: starting");
        let resolver = IdResolver::new(ResolverConfig::with_prefix("bd"));
        let resolved_id = resolver
            .resolve("bd-abc123", |id| id == "bd-abc123", |_hash| Vec::new())
            .unwrap();
        assert_eq!(resolved_id.id, "bd-abc123");
        info!("test_show_resolves_full_id: assertions passed");
    }

    #[test]
    fn test_show_resolves_prefixed_id() {
        init_logging();
        info!("test_show_resolves_prefixed_id: starting");
        let resolver = IdResolver::new(ResolverConfig::with_prefix("bd"));
        let resolved_id = resolver
            .resolve("abc123", |id| id == "bd-abc123", |_hash| Vec::new())
            .unwrap();
        assert_eq!(resolved_id.id, "bd-abc123");
        info!("test_show_resolves_prefixed_id: assertions passed");
    }

    #[test]
    fn test_show_resolves_partial_id() {
        init_logging();
        info!("test_show_resolves_partial_id: starting");
        let resolver = IdResolver::new(ResolverConfig::with_prefix("bd"));
        let resolved_id = resolver
            .resolve(
                "abc",
                |_id| false,
                |hash| {
                    if hash == "abc" {
                        vec!["bd-abc123".to_string()]
                    } else {
                        Vec::new()
                    }
                },
            )
            .unwrap();
        assert_eq!(resolved_id.id, "bd-abc123");
        info!("test_show_resolves_partial_id: assertions passed");
    }

    #[test]
    fn test_show_not_found_error() {
        init_logging();
        info!("test_show_not_found_error: starting");
        let resolver = IdResolver::new(ResolverConfig::with_prefix("bd"));
        let result = resolver.resolve("missing", |_id| false, |_hash| Vec::new());
        assert!(result.is_err());
        info!("test_show_not_found_error: assertions passed");
    }

    #[test]
    fn test_show_json_output_shape() {
        init_logging();
        info!("test_show_json_output_shape: starting");
        let issue = make_test_issue("bd-001", "Test Issue");
        let details = IssueDetails {
            issue: issue.clone(),
            labels: vec!["bug".to_string()],
            dependencies: vec![IssueWithDependencyMetadata {
                id: "bd-002".to_string(),
                title: "Dep".to_string(),
                status: Status::Open,
                priority: Priority::MEDIUM,
                dep_type: "blocks".to_string(),
            }],
            dependents: Vec::new(),
            comments: Vec::new(),
            events: Vec::new(),
            parent: None,
        };
        let json = serde_json::to_string_pretty(&vec![details]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 1);
        assert_eq!(parsed[0]["id"], issue.id);
        assert!(parsed[0]["labels"].is_array());
        assert!(parsed[0]["dependencies"].is_array());
        info!("test_show_json_output_shape: assertions passed");
    }

    #[test]
    fn test_show_text_includes_dependencies_and_comments() {
        init_logging();
        info!("test_show_text_includes_dependencies_and_comments: starting");
        let mut issue = make_test_issue("bd-001", "Test Issue");
        issue.description = None;
        let details = IssueDetails {
            issue,
            labels: Vec::new(),
            dependencies: vec![IssueWithDependencyMetadata {
                id: "bd-002".to_string(),
                title: "Dep".to_string(),
                status: Status::Open,
                priority: Priority::MEDIUM,
                dep_type: "blocks".to_string(),
            }],
            dependents: Vec::new(),
            comments: vec![Comment {
                id: 1,
                issue_id: "bd-001".to_string(),
                author: "alice".to_string(),
                body: "Looks good".to_string(),
                created_at: Utc.with_ymd_and_hms(2025, 1, 2, 3, 4, 0).unwrap(),
            }],
            events: Vec::new(),
            parent: None,
        };
        let output = format_issue_details(&details, false);
        assert!(output.contains("Dependencies:"));
        assert!(output.contains("-> bd-002 (blocks) - Dep"));
        assert!(output.contains("Comments:"));
        assert!(output.contains("alice: Looks good"));
        info!("test_show_text_includes_dependencies_and_comments: assertions passed");
    }

    #[test]
    fn test_apply_external_dependency_metadata_updates_generated_placeholder() {
        init_logging();
        info!("test_apply_external_dependency_metadata_updates_generated_placeholder: starting");
        let mut dependencies = vec![IssueWithDependencyMetadata {
            id: "external:proj:cap".to_string(),
            title: "proj:cap".to_string(),
            status: Status::Blocked,
            priority: Priority::MEDIUM,
            dep_type: "blocks".to_string(),
        }];

        let mut statuses = HashMap::new();
        statuses.insert("external:proj:cap".to_string(), true);

        apply_external_dependency_metadata(&mut dependencies, &statuses);

        assert_eq!(dependencies[0].status, Status::Closed);
        assert_eq!(dependencies[0].title, "✓ proj:cap");
        info!(
            "test_apply_external_dependency_metadata_updates_generated_placeholder: assertions passed"
        );
    }

    #[test]
    fn test_build_issue_details_from_jsonl_derives_parent_and_dependents() {
        init_logging();
        info!("test_build_issue_details_from_jsonl_derives_parent_and_dependents: starting");

        let mut parent = make_test_issue("bd-parent", "Parent");
        parent.priority = Priority::HIGH;

        let mut child = make_test_issue("bd-child", "Child");
        child.labels = vec!["backend".to_string()];
        child.comments = vec![Comment {
            id: 7,
            issue_id: "bd-child".to_string(),
            author: "alice".to_string(),
            body: "Investigating".to_string(),
            created_at: Utc.with_ymd_and_hms(2025, 1, 2, 3, 4, 0).unwrap(),
        }];
        child.dependencies = vec![Dependency {
            issue_id: "bd-child".to_string(),
            depends_on_id: "bd-parent".to_string(),
            dep_type: DependencyType::ParentChild,
            created_at: Utc.with_ymd_and_hms(2025, 1, 1, 1, 0, 0).unwrap(),
            created_by: Some("tester".to_string()),
            metadata: None,
            thread_id: None,
        }];

        let issues_by_id = HashMap::from([
            (parent.id.clone(), parent.clone()),
            (child.id.clone(), child.clone()),
        ]);

        let child_details = build_issue_details_from_jsonl(&child, &issues_by_id).unwrap();
        assert_eq!(child_details.parent.as_deref(), Some("bd-parent"));
        assert_eq!(child_details.labels, vec!["backend".to_string()]);
        assert_eq!(child_details.comments.len(), 1);
        assert!(child_details.issue.labels.is_empty());
        assert!(child_details.issue.dependencies.is_empty());
        assert!(child_details.issue.comments.is_empty());

        let parent_details = build_issue_details_from_jsonl(&parent, &issues_by_id).unwrap();
        assert_eq!(parent_details.dependents.len(), 1);
        assert_eq!(parent_details.dependents[0].id, "bd-child");
        assert_eq!(parent_details.dependents[0].dep_type, "parent-child");
        info!(
            "test_build_issue_details_from_jsonl_derives_parent_and_dependents: assertions passed"
        );
    }
}
