//! Dependency command implementation.

use super::{
    auto_import_storage_ctx_if_stale, resolve_issue_id, retry_mutation_with_jsonl_recovery,
};
use crate::cli::{
    DepAddArgs, DepCommands, DepCyclesArgs, DepDirection, DepListArgs, DepRemoveArgs, DepTreeArgs,
    OutputFormat, resolve_output_format_basic_with_outer_mode,
};
use crate::config;
use crate::error::{BeadsError, Result};
use crate::format::truncate_title;
use crate::model::DependencyType;
use crate::output::{OutputContext, OutputMode, Theme};
use crate::storage::SqliteStorage;
use crate::util::id::{IdResolver, ResolverConfig};
use rich_rust::prelude::*;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Execute the dep command.
///
/// # Errors
///
/// Returns an error if database operations fail or if inputs are invalid.
pub fn execute(
    command: &DepCommands,
    json: bool,
    cli: &config::CliOverrides,
    ctx: &OutputContext,
) -> Result<()> {
    let beads_dir = config::discover_beads_dir_with_cli(cli)?;
    match command {
        DepCommands::Add(args) => execute_dep_add(args, json, cli, ctx, &beads_dir),
        DepCommands::Remove(args) => execute_dep_remove(args, json, cli, ctx, &beads_dir),
        DepCommands::List(args) => execute_dep_list(args, cli, ctx, &beads_dir),
        DepCommands::Tree(args) => execute_dep_tree(args, json, cli, ctx, &beads_dir),
        DepCommands::Cycles(args) => {
            let storage_ctx = config::open_storage_with_cli(&beads_dir, cli)?;
            dep_cycles(args, &storage_ctx.storage, json, ctx)
        }
    }
}

fn execute_dep_add(
    args: &DepAddArgs,
    _json: bool,
    cli: &config::CliOverrides,
    ctx: &OutputContext,
    local_beads_dir: &Path,
) -> Result<()> {
    validate_dependency_target_route(local_beads_dir, &args.issue, &args.depends_on)?;
    let (mut storage_ctx, route_cli, auto_flush_external) =
        open_routed_storage_for_input(local_beads_dir, cli, &args.issue)?;
    let config_layer = storage_ctx.load_config(&route_cli)?;
    let id_config = config::id_config_from_layer(&config_layer);
    let resolver = IdResolver::new(ResolverConfig::with_prefix(id_config.prefix));
    let actor = config::resolve_actor(&config_layer);
    dep_add(
        args,
        &mut storage_ctx,
        &resolver,
        &actor,
        ctx,
        local_beads_dir,
        auto_flush_external,
    )
}

fn execute_dep_remove(
    args: &DepRemoveArgs,
    _json: bool,
    cli: &config::CliOverrides,
    ctx: &OutputContext,
    local_beads_dir: &Path,
) -> Result<()> {
    validate_dependency_target_route(local_beads_dir, &args.issue, &args.depends_on)?;
    let (mut storage_ctx, route_cli, auto_flush_external) =
        open_routed_storage_for_input(local_beads_dir, cli, &args.issue)?;
    let config_layer = storage_ctx.load_config(&route_cli)?;
    let id_config = config::id_config_from_layer(&config_layer);
    let resolver = IdResolver::new(ResolverConfig::with_prefix(id_config.prefix));
    let actor = config::resolve_actor(&config_layer);
    dep_remove(
        args,
        &mut storage_ctx,
        &resolver,
        &actor,
        ctx,
        local_beads_dir,
        auto_flush_external,
    )
}

fn execute_dep_list(
    args: &DepListArgs,
    cli: &config::CliOverrides,
    ctx: &OutputContext,
    local_beads_dir: &Path,
) -> Result<()> {
    let (storage_ctx, route_cli, _) =
        open_routed_storage_for_input(local_beads_dir, cli, &args.issue)?;
    let config_layer = storage_ctx.load_config(&route_cli)?;
    let use_color = config::should_use_color(&config_layer);
    let quiet = route_cli.quiet.unwrap_or(false);
    let id_config = config::id_config_from_layer(&config_layer);
    let resolver = IdResolver::new(ResolverConfig::with_prefix(id_config.prefix));
    let external_db_paths =
        config::external_project_db_paths(&config_layer, &storage_ctx.paths.beads_dir);

    dep_list(
        args,
        &storage_ctx.storage,
        &resolver,
        &external_db_paths,
        ctx,
        quiet,
        !use_color,
    )
}

fn execute_dep_tree(
    args: &DepTreeArgs,
    _json: bool,
    cli: &config::CliOverrides,
    ctx: &OutputContext,
    local_beads_dir: &Path,
) -> Result<()> {
    let (storage_ctx, route_cli, _) =
        open_routed_storage_for_input(local_beads_dir, cli, &args.issue)?;
    let config_layer = storage_ctx.load_config(&route_cli)?;
    let id_config = config::id_config_from_layer(&config_layer);
    let resolver = IdResolver::new(ResolverConfig::with_prefix(id_config.prefix));
    let external_db_paths =
        config::external_project_db_paths(&config_layer, &storage_ctx.paths.beads_dir);

    dep_tree(
        args,
        &storage_ctx.storage,
        &resolver,
        &external_db_paths,
        false,
        ctx,
    )
}

fn open_routed_storage_for_input(
    local_beads_dir: &Path,
    cli: &config::CliOverrides,
    issue_input: &str,
) -> Result<(config::OpenStorageResult, config::CliOverrides, bool)> {
    let route = config::routing::resolve_route(issue_input, local_beads_dir)?;
    let mut route_cli = cli.clone();
    if route.is_external {
        route_cli.db = None;
    }
    let mut storage_ctx = config::open_storage_with_cli(&route.beads_dir, &route_cli)?;
    auto_import_storage_ctx_if_stale(&mut storage_ctx, &route_cli)?;
    Ok((storage_ctx, route_cli, route.is_external))
}

fn validate_dependency_target_route(
    local_beads_dir: &Path,
    issue_input: &str,
    depends_on_input: &str,
) -> Result<()> {
    if depends_on_input.starts_with("external:") {
        return Ok(());
    }

    let issue_route = config::routing::resolve_route(issue_input, local_beads_dir)?;
    let depends_on_route = config::routing::resolve_route(depends_on_input, local_beads_dir)?;

    if issue_route.beads_dir == depends_on_route.beads_dir {
        return Ok(());
    }

    Err(BeadsError::validation(
        "depends_on",
        format!(
            "issue '{issue_input}' and dependency target '{depends_on_input}' resolve to different projects; use an explicit external:... dependency for cross-project links"
        ),
    ))
}

/// JSON output for dep add/remove operations
#[derive(Serialize)]
struct DepActionResult {
    status: String,
    issue_id: String,
    depends_on_id: String,
    #[serde(rename = "type")]
    dep_type: String,
    action: String,
}

/// JSON output for dep list
#[derive(Serialize)]
struct DepListItem {
    issue_id: String,
    depends_on_id: String,
    #[serde(rename = "type")]
    dep_type: String,
    title: String,
    status: String,
    priority: i32,
}

/// JSON output for dep tree
#[derive(Serialize)]
struct TreeNode {
    #[serde(skip_serializing)]
    node_key: String,
    id: String,
    title: String,
    depth: usize,
    parent_id: Option<String>,
    #[serde(skip_serializing)]
    parent_key: Option<String>,
    priority: i32,
    status: String,
    truncated: bool,
}

/// JSON output for dep cycles
#[derive(Serialize)]
struct CyclesResult {
    cycles: Vec<Vec<String>>,
    count: usize,
}

fn dep_add(
    args: &DepAddArgs,
    storage_ctx: &mut config::OpenStorageResult,
    resolver: &IdResolver,
    actor: &str,
    ctx: &OutputContext,
    local_beads_dir: &Path,
    auto_flush_external: bool,
) -> Result<()> {
    let issue_id = resolve_issue_id(&storage_ctx.storage, resolver, &args.issue)?;

    // External dependencies don't need resolution
    let depends_on_id = if args.depends_on.starts_with("external:") {
        args.depends_on.clone()
    } else {
        resolve_issue_id(&storage_ctx.storage, resolver, &args.depends_on)?
    };

    let dep_type = parse_dependency_type(&args.dep_type)?;

    // Self-dependency check
    if issue_id == depends_on_id {
        return Err(BeadsError::SelfDependency { id: issue_id });
    }

    // Cycle check for blocking types only
    if dep_type.is_blocking()
        && !depends_on_id.starts_with("external:")
        && storage_ctx
            .storage
            .would_create_cycle(&issue_id, &depends_on_id, true)?
    {
        return Err(BeadsError::DependencyCycle {
            path: format!("{issue_id} -> {depends_on_id}"),
        });
    }

    let added = retry_mutation_with_jsonl_recovery(
        storage_ctx,
        true,
        "dep add",
        Some(issue_id.as_str()),
        |storage| {
            storage.add_dependency_with_metadata(
                &issue_id,
                &depends_on_id,
                dep_type.as_str(),
                actor,
                args.metadata.as_deref(),
            )
        },
    )?;

    storage_ctx.flush_no_db_if_dirty()?;
    if auto_flush_external && let Err(error) = storage_ctx.auto_flush_if_enabled() {
        tracing::debug!(
            beads_dir = %storage_ctx.paths.beads_dir.display(),
            error = %error,
            "Routed auto-flush failed (non-fatal)"
        );
    }
    crate::util::set_last_touched_id(local_beads_dir, &issue_id);

    if ctx.is_json() || ctx.is_toon() {
        let result = DepActionResult {
            status: if added { "ok" } else { "exists" }.to_string(),
            issue_id: issue_id.clone(),
            depends_on_id: depends_on_id.clone(),
            dep_type: dep_type.as_str().to_string(),
            action: if added { "added" } else { "already_exists" }.to_string(),
        };
        if ctx.is_toon() {
            ctx.toon(&result);
        } else {
            ctx.json_pretty(&result);
        }
    } else if matches!(ctx.mode(), OutputMode::Quiet) {
        return Ok(());
    } else if added {
        if ctx.is_rich() {
            // Rich mode: Show detailed visual feedback
            ctx.success(&format!(
                "Added dependency: {} → {}",
                issue_id, depends_on_id
            ));
            let relationship = match dep_type {
                DependencyType::Blocks => format!("  {} now blocks {}", depends_on_id, issue_id),
                DependencyType::ParentChild => {
                    format!("  {} is parent of {}", depends_on_id, issue_id)
                }
                DependencyType::WaitsFor => {
                    format!("  {} waits for {}", issue_id, depends_on_id)
                }
                _ => format!("  Relationship: {}", dep_type.as_str()),
            };
            ctx.print_line(&relationship);
        } else {
            ctx.success(&format!(
                "Added dependency: {} -> {} ({})",
                issue_id,
                depends_on_id,
                dep_type.as_str()
            ));
        }
    } else {
        ctx.info(&format!(
            "Dependency already exists: {issue_id} → {depends_on_id}"
        ));
    }

    Ok(())
}

fn dep_remove(
    args: &DepRemoveArgs,
    storage_ctx: &mut config::OpenStorageResult,
    resolver: &IdResolver,
    actor: &str,
    ctx: &OutputContext,
    local_beads_dir: &Path,
    auto_flush_external: bool,
) -> Result<()> {
    let issue_id = resolve_issue_id(&storage_ctx.storage, resolver, &args.issue)?;

    // External dependencies don't need resolution
    let depends_on_id = if args.depends_on.starts_with("external:") {
        args.depends_on.clone()
    } else {
        resolve_issue_id(&storage_ctx.storage, resolver, &args.depends_on)?
    };

    let dep_type = dependency_type_for_pair(&storage_ctx.storage, &issue_id, &depends_on_id)?
        .unwrap_or_else(|| "unknown".to_string());
    let removed = retry_mutation_with_jsonl_recovery(
        storage_ctx,
        true,
        "dep remove",
        Some(issue_id.as_str()),
        |storage| storage.remove_dependency(&issue_id, &depends_on_id, actor),
    )?;

    storage_ctx.flush_no_db_if_dirty()?;
    if auto_flush_external && let Err(error) = storage_ctx.auto_flush_if_enabled() {
        tracing::debug!(
            beads_dir = %storage_ctx.paths.beads_dir.display(),
            error = %error,
            "Routed auto-flush failed (non-fatal)"
        );
    }
    crate::util::set_last_touched_id(local_beads_dir, &issue_id);

    if ctx.is_json() || ctx.is_toon() {
        let result = DepActionResult {
            status: if removed { "ok" } else { "not_found" }.to_string(),
            issue_id: issue_id.clone(),
            depends_on_id: depends_on_id.clone(),
            dep_type,
            action: if removed { "removed" } else { "not_found" }.to_string(),
        };
        if ctx.is_toon() {
            ctx.toon(&result);
        } else {
            ctx.json_pretty(&result);
        }
    } else if matches!(ctx.mode(), OutputMode::Quiet) {
        return Ok(());
    } else if removed {
        if ctx.is_rich() {
            ctx.success(&format!(
                "Removed dependency: {} → {}",
                issue_id, depends_on_id
            ));
            ctx.print_line(&format!(
                "  {} no longer depends on {}",
                issue_id, depends_on_id
            ));
        } else {
            ctx.success(&format!(
                "Removed dependency: {issue_id} -> {depends_on_id}"
            ));
        }
    } else {
        ctx.warning(&format!(
            "Dependency not found: {issue_id} → {depends_on_id}"
        ));
    }

    Ok(())
}

fn dependency_type_for_pair(
    storage: &SqliteStorage,
    issue_id: &str,
    depends_on_id: &str,
) -> Result<Option<String>> {
    Ok(storage
        .get_dependencies_full(issue_id)?
        .into_iter()
        .find(|dep| dep.depends_on_id == depends_on_id)
        .map(|dep| dep.dep_type.as_str().to_string()))
}

fn parse_dependency_type(dep_type: &str) -> Result<DependencyType> {
    let parsed: DependencyType = dep_type.parse().map_err(|_| BeadsError::Validation {
        field: "type".to_string(),
        reason: format!("Invalid dependency type: {dep_type}"),
    })?;

    if let DependencyType::Custom(_) = parsed {
        return Err(BeadsError::Validation {
            field: "type".to_string(),
            reason: format!(
                "Unknown dependency type: '{dep_type}'. \
                 Allowed types: blocks, parent-child, conditional-blocks, waits-for, \
                 related, discovered-from, replies-to, relates-to, duplicates, \
                 supersedes, caused-by"
            ),
        });
    }

    Ok(parsed)
}

fn normalize_dep_type_filter(dep_type: &str) -> Result<String> {
    Ok(parse_dependency_type(dep_type)?.as_str().to_string())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn dep_list(
    args: &DepListArgs,
    storage: &SqliteStorage,
    resolver: &IdResolver,
    external_db_paths: &HashMap<String, PathBuf>,
    outer_ctx: &OutputContext,
    quiet: bool,
    no_color: bool,
) -> Result<()> {
    let output_format = resolve_output_format_basic_with_outer_mode(
        args.format,
        outer_ctx.inherited_output_mode(),
        false,
    );
    let ctx = OutputContext::from_output_format(output_format, quiet, no_color);
    let issue_id = resolve_issue_id(storage, resolver, &args.issue)?;
    let dep_type_filter = args
        .dep_type
        .as_deref()
        .map(normalize_dep_type_filter)
        .transpose()?;

    let mut items = Vec::new();

    // Get dependencies (what this issue depends on)
    if matches!(args.direction, DepDirection::Down | DepDirection::Both) {
        let deps = storage.get_dependencies_with_metadata(&issue_id)?;
        for dep in deps {
            if let Some(ref filter_type) = dep_type_filter
                && dep.dep_type != *filter_type
            {
                continue;
            }
            items.push(DepListItem {
                issue_id: issue_id.clone(),
                depends_on_id: dep.id.clone(),
                dep_type: dep.dep_type.clone(),
                title: dep.title.clone(),
                status: dep.status.as_str().to_string(),
                priority: dep.priority.0,
            });
        }
    }

    // Get dependents (what depends on this issue)
    if matches!(args.direction, DepDirection::Up | DepDirection::Both) {
        let deps = storage.get_dependents_with_metadata(&issue_id)?;
        for dep in deps {
            if let Some(ref filter_type) = dep_type_filter
                && dep.dep_type != *filter_type
            {
                continue;
            }
            items.push(DepListItem {
                issue_id: dep.id.clone(),
                depends_on_id: issue_id.clone(),
                dep_type: dep.dep_type.clone(),
                title: dep.title.clone(),
                status: dep.status.as_str().to_string(),
                priority: dep.priority.0,
            });
        }
    }

    if !items.is_empty()
        && items.iter().any(|item| {
            item.depends_on_id.starts_with("external:") || item.issue_id.starts_with("external:")
        })
    {
        let external_statuses =
            storage.resolve_external_dependency_statuses(external_db_paths, false)?;
        apply_external_dep_list_metadata(&mut items, &external_statuses);
    }

    if matches!(ctx.mode(), OutputMode::Quiet) {
        return Ok(());
    }

    match output_format {
        OutputFormat::Json => {
            ctx.json_pretty(&items);
            return Ok(());
        }
        OutputFormat::Toon => {
            ctx.toon_with_stats(&items, args.stats);
            return Ok(());
        }
        OutputFormat::Text | OutputFormat::Csv => {}
    }

    sort_dep_list_items_for_human(&mut items);

    if items.is_empty() {
        let direction_str = match args.direction {
            DepDirection::Down => "dependencies",
            DepDirection::Up => "dependents",
            DepDirection::Both => "dependencies or dependents",
        };
        ctx.info(&format!("No {direction_str} for {issue_id}"));
        return Ok(());
    }

    if ctx.is_rich() {
        // Rich mode: Use panel with tree-like display
        render_dep_list_rich(&ctx, &issue_id, &items, args.direction);
    } else {
        // Plain mode: Simple text output
        let header = match args.direction {
            DepDirection::Down => format!("Dependencies of {} ({}):", issue_id, items.len()),
            DepDirection::Up => format!("Dependents of {} ({}):", issue_id, items.len()),
            DepDirection::Both => format!(
                "Dependencies and dependents of {} ({}):",
                issue_id,
                items.len()
            ),
        };
        ctx.info(&header);

        for item in &items {
            let arrow = if item.issue_id == issue_id {
                format!("  -> {} ({})", item.depends_on_id, item.dep_type)
            } else {
                format!("  <- {} ({})", item.issue_id, item.dep_type)
            };
            ctx.print_line(&format!(
                "{}: {} [P{}] [{}]",
                arrow, item.title, item.priority, item.status
            ));
        }
    }

    Ok(())
}

/// Render dependency list in rich mode with panel and tree-like display
fn render_dep_list_rich(
    ctx: &OutputContext,
    issue_id: &str,
    items: &[DepListItem],
    direction: DepDirection,
) {
    let theme = ctx.theme();

    // Separate items into dependencies (this issue depends on) and dependents (depend on this)
    let (deps, dependents): (Vec<_>, Vec<_>) =
        items.iter().partition(|item| item.issue_id == issue_id);

    let mut content = Text::new("");

    // Show dependencies (what this issue depends on)
    if !deps.is_empty() && matches!(direction, DepDirection::Down | DepDirection::Both) {
        append_dep_list_section(
            &mut content,
            &dep_list_section_title(true, deps.len()),
            &deps,
            true,
            theme,
        );
    }

    // Add separator if showing both directions
    if !deps.is_empty() && !dependents.is_empty() && matches!(direction, DepDirection::Both) {
        content.append("\n");
    }

    // Show dependents (what depends on this issue)
    if !dependents.is_empty() && matches!(direction, DepDirection::Up | DepDirection::Both) {
        append_dep_list_section(
            &mut content,
            &dep_list_section_title(false, dependents.len()),
            &dependents,
            false,
            theme,
        );
    }

    let panel = Panel::from_rich_text(&content, ctx.width())
        .title(Text::new(dep_list_panel_title(direction, issue_id)))
        .box_style(theme.box_style)
        .border_style(theme.panel_border.clone());

    ctx.render(&panel);
}

fn dep_list_panel_title(direction: DepDirection, issue_id: &str) -> String {
    match direction {
        DepDirection::Down => format!("Dependencies for {issue_id}"),
        DepDirection::Up => format!("Dependents for {issue_id}"),
        DepDirection::Both => format!("Dependency relations for {issue_id}"),
    }
}

fn dep_list_section_title(is_dependency_section: bool, count: usize) -> String {
    let label = if is_dependency_section {
        "Dependencies"
    } else {
        "Dependents"
    };
    format!("{label} ({count}):")
}

fn append_dep_list_section(
    content: &mut Text,
    title: &str,
    items: &[&DepListItem],
    use_depends_on_id: bool,
    theme: &Theme,
) {
    content.append_styled(&format!("{title}\n"), theme.emphasis.clone());

    for (i, item) in items.iter().enumerate() {
        let prefix = if i == items.len() - 1 {
            "└── "
        } else {
            "├── "
        };
        let target_id = if use_depends_on_id {
            &item.depends_on_id
        } else {
            &item.issue_id
        };

        content.append_styled(prefix, theme.dimmed.clone());
        content.append_styled(target_id, theme.issue_id.clone());
        content.append(" ");
        content.append_styled(&format!("({}) ", item.dep_type), theme.muted.clone());
        append_dep_list_status(content, &item.status, theme);
        content.append(" ");
        content.append_styled(&item.title, theme.issue_title.clone());
        content.append("\n");
    }
}

fn dep_list_status_label(status: &str) -> &str {
    match status {
        "open" => "[open]",
        "in_progress" => "[in-progress]",
        "closed" => "[closed] ✓",
        "blocked" => "[blocked]",
        _ => status,
    }
}

fn append_dep_list_status(content: &mut Text, status: &str, theme: &Theme) {
    let style = match status {
        "open" => theme.status_open.clone(),
        "in_progress" => theme.status_in_progress.clone(),
        "closed" => theme.status_closed.clone(),
        "blocked" => theme.status_blocked.clone(),
        _ => theme.dimmed.clone(),
    };
    content.append_styled(dep_list_status_label(status), style);
}

fn apply_external_dep_list_metadata(
    items: &mut [DepListItem],
    external_statuses: &HashMap<String, bool>,
) {
    for item in items {
        let external_id = if item.depends_on_id.starts_with("external:") {
            Some(item.depends_on_id.as_str())
        } else if item.issue_id.starts_with("external:") {
            Some(item.issue_id.as_str())
        } else {
            None
        };

        let Some(external_id) = external_id else {
            continue;
        };

        let satisfied = external_statuses.get(external_id).copied().unwrap_or(false);
        item.status = if satisfied {
            "closed".to_string()
        } else {
            "blocked".to_string()
        };

        let placeholder_title = external_id.strip_prefix("external:").unwrap_or(external_id);
        if item.title.is_empty() || item.title == placeholder_title {
            let prefix = if satisfied { "✓" } else { "⏳" };
            item.title = parse_external_dep_id(external_id).map_or_else(
                || format!("{prefix} {external_id}"),
                |(project, capability)| format!("{prefix} {project}:{capability}"),
            );
        }
    }
}

fn sort_dep_list_items_for_human(items: &mut [DepListItem]) {
    items.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| left.issue_id.cmp(&right.issue_id))
            .then_with(|| left.depends_on_id.cmp(&right.depends_on_id))
            .then_with(|| left.dep_type.cmp(&right.dep_type))
    });
}

fn resolve_dep_tree_node_metadata(
    storage: &SqliteStorage,
    root_id: &str,
    root_issue: &crate::model::Issue,
    node_id: &str,
    external_statuses: &HashMap<String, bool>,
) -> Result<(String, i32, String)> {
    if node_id == root_id {
        return Ok((
            root_issue.title.clone(),
            root_issue.priority.0,
            root_issue.status.as_str().to_string(),
        ));
    }

    if node_id.starts_with("external:") {
        let satisfied = external_statuses.get(node_id).copied().unwrap_or(false);
        let status = if satisfied { "closed" } else { "blocked" };
        let prefix = if satisfied { "✓" } else { "⏳" };
        let title = if let Some((project, capability)) = parse_external_dep_id(node_id) {
            format!("{prefix} {project}:{capability}")
        } else {
            format!("{prefix} {node_id}")
        };
        return Ok((title, 2, status.to_string()));
    }

    let issue_opt = storage.get_issue(node_id)?;
    if let Some(issue) = issue_opt {
        return Ok((
            issue.title.clone(),
            issue.priority.0,
            issue.status.as_str().to_string(),
        ));
    }

    // Handle missing/deleted issues gracefully instead of failing the whole tree
    Ok((
        format!("[missing issue: {node_id}]"),
        2,
        "deleted".to_string(),
    ))
}

fn dep_tree_truncated(depth: usize, max_depth: usize, dependency_count: usize) -> bool {
    depth >= max_depth && dependency_count > 0
}

#[allow(clippy::too_many_lines)]
fn dep_tree(
    args: &DepTreeArgs,
    storage: &SqliteStorage,
    resolver: &IdResolver,
    external_db_paths: &HashMap<String, PathBuf>,
    _json: bool,
    ctx: &OutputContext,
) -> Result<()> {
    let root_id = resolve_issue_id(storage, resolver, &args.issue)?;
    let root_issue = storage
        .get_issue(&root_id)?
        .ok_or_else(|| BeadsError::IssueNotFound {
            id: root_id.clone(),
        })?;

    // Helper struct for BFS
    #[allow(clippy::items_after_statements)]
    struct QueueItem {
        id: String,
        depth: usize,
        parent_id: Option<String>,
        parent_key: Option<String>,
        path: Vec<String>,
    }

    let external_statuses =
        storage.resolve_external_dependency_statuses(external_db_paths, false)?;

    // Optimization: Prefetch all active issue metadata to avoid N+1 queries during traversal
    let metadata_cache = storage.get_active_issues_metadata()?;

    let mut nodes = Vec::new();

    let mut queue = vec![QueueItem {
        id: root_id.clone(),
        depth: 0,
        parent_id: None,
        parent_key: None,
        path: Vec::new(),
    }];
    let mut next_node_key = 0usize;

    while let Some(item) = queue.pop() {
        // Cycle detection: check if current ID is already in the path
        if item.path.contains(&item.id) {
            continue;
        }

        let node_key = format!("n{next_node_key}");
        next_node_key += 1;

        let (title, priority, status) = if let Some(meta) = metadata_cache.get(&item.id) {
            meta.clone()
        } else {
            resolve_dep_tree_node_metadata(
                storage,
                &root_id,
                &root_issue,
                &item.id,
                &external_statuses,
            )?
        };

        let mut dependencies = Vec::new();
        let truncated = if item.id.starts_with("external:") {
            false
        } else {
            dependencies = match args.direction {
                DepDirection::Down => storage.get_dependencies(&item.id)?,
                DepDirection::Up => storage.get_dependents(&item.id)?,
                DepDirection::Both => {
                    let mut all = storage.get_dependencies(&item.id)?;
                    let mut up = storage.get_dependents(&item.id)?;
                    all.append(&mut up);
                    all.sort();
                    all.dedup();
                    all
                }
            };
            dep_tree_truncated(item.depth, args.max_depth, dependencies.len())
        };

        nodes.push(TreeNode {
            node_key: node_key.clone(),
            id: item.id.clone(),
            title,
            depth: item.depth,
            parent_id: item.parent_id.clone(),
            parent_key: item.parent_key.clone(),
            priority,
            status,
            truncated,
        });

        // Don't expand if at max depth
        if item.depth < args.max_depth && !item.id.starts_with("external:") {
            let mut new_path = item.path.clone();
            new_path.push(item.id.clone());

            // Get full issue details for sorting
            // This is slightly inefficient (N queries), but necessary for sorting by priority.
            // Optimization: fetch all at once or accept ID sort.
            // For now, let's sort by ID to be deterministic, or fetch details.
            // The original code sorted the FINAL list.
            // To maintain DFS order with sorted siblings, we must sort here.

            // Let's just sort by ID for stability and speed, priority sorting would require fetching issues.
            dependencies.sort();
            // Push in reverse order so first item pops first
            for dep_id in dependencies.into_iter().rev() {
                // No global visited check here
                queue.push(QueueItem {
                    id: dep_id,
                    depth: item.depth + 1,
                    parent_id: Some(item.id.clone()),
                    parent_key: Some(node_key.clone()),
                    path: new_path.clone(),
                });
            }
        }
    }

    if ctx.is_json() || ctx.is_toon() {
        if ctx.is_toon() {
            ctx.toon(&nodes);
        } else {
            ctx.json_pretty(&nodes);
        }
        return Ok(());
    }

    if matches!(ctx.mode(), OutputMode::Quiet) {
        return Ok(());
    }

    // Mermaid format output
    if args.format.eq_ignore_ascii_case("mermaid") {
        render_dep_tree_mermaid(&nodes);
        return Ok(());
    }

    // Text tree output
    if nodes.is_empty() {
        ctx.info(&format!("No dependency tree for {root_id}"));
        return Ok(());
    }

    if ctx.is_rich() {
        // Rich mode: Use tree component with styled output
        render_dep_tree_rich(ctx, &nodes);
    } else {
        // Plain mode: Simple indented text
        for node in &nodes {
            let indent = "  ".repeat(node.depth);
            let prefix = if node.depth == 0 {
                ""
            } else if node.truncated {
                "├── (truncated) "
            } else {
                "├── "
            };
            ctx.print_line(&format!(
                "{}{}{}: {} [P{}] [{}]",
                indent, prefix, node.id, node.title, node.priority, node.status
            ));
        }
    }

    Ok(())
}

fn sanitize_mermaid_label(text: &str) -> String {
    text.replace('"', "'").replace(['\n', '\r'], " ")
}

fn render_dep_tree_mermaid(nodes: &[TreeNode]) {
    // Use println! directly to avoid rich_rust markup interpretation
    println!("graph TD");

    for node in nodes {
        let escaped_id = sanitize_mermaid_label(&node.id);
        let escaped_title = sanitize_mermaid_label(&node.title);
        println!(
            "    {}[\"{}: {} [P{}]\"]",
            node.node_key, escaped_id, escaped_title, node.priority
        );
    }

    for node in nodes {
        if let Some(parent_key) = node.parent_key.as_deref() {
            println!("    {parent_key} --> {}", node.node_key);
        }
    }
}

/// Render dependency tree in rich mode using Tree component
fn render_dep_tree_rich(ctx: &OutputContext, nodes: &[TreeNode]) {
    if nodes.is_empty() {
        return;
    }

    let theme = ctx.theme();

    // Group nodes by parent_key for O(1) lookups
    let mut children_map: std::collections::HashMap<Option<&str>, Vec<&TreeNode>> =
        std::collections::HashMap::new();
    for node in nodes {
        children_map
            .entry(node.parent_key.as_deref())
            .or_default()
            .push(node);
    }

    // Build tree structure from flat nodes list
    let root = build_tree_node_rich(&nodes[0], &children_map);
    let tree = Tree::new(root)
        .guides(TreeGuides::Rounded)
        .guide_style(theme.dimmed.clone());

    ctx.render(&tree);
}

/// Recursively build a tree node for rich rendering
fn build_tree_node_rich<'a>(
    node: &'a TreeNode,
    children_map: &std::collections::HashMap<Option<&'a str>, Vec<&'a TreeNode>>,
) -> rich_rust::renderables::TreeNode {
    // Format the node label with status styling
    let status_style = match node.status.as_str() {
        "open" => "[green]",
        "in_progress" => "[yellow]",
        "closed" => "[dim]",
        "blocked" => "[red]",
        _ => "[white]",
    };
    let status_close = "[/]";

    let status_indicator = match node.status.as_str() {
        "closed" => " ✓",
        "blocked" => " ⚠",
        _ => "",
    };

    let label = if node.truncated {
        format!(
            "{} {}[{}]{}{} {} [dim](truncated)[/]",
            node.id,
            status_style,
            node.status,
            status_close,
            status_indicator,
            truncate_title(&node.title, 35)
        )
    } else {
        format!(
            "{} {}[{}]{}{} {}",
            node.id,
            status_style,
            node.status,
            status_close,
            status_indicator,
            truncate_title(&node.title, 40)
        )
    };

    let mut tree_node = rich_rust::renderables::TreeNode::new(Text::new(label));

    // Find and add children using the pre-computed map
    if let Some(children) = children_map.get(&Some(node.node_key.as_str())) {
        for child in children {
            let child_node = build_tree_node_rich(child, children_map);
            tree_node = tree_node.child(child_node);
        }
    }

    tree_node
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

fn dep_cycles(
    _args: &DepCyclesArgs,
    storage: &SqliteStorage,
    _json: bool,
    ctx: &OutputContext,
) -> Result<()> {
    let cycles = storage.detect_all_cycles()?;
    let count = cycles.len();

    if ctx.is_json() || ctx.is_toon() {
        let result = CyclesResult { cycles, count };
        if ctx.is_toon() {
            ctx.toon(&result);
        } else {
            ctx.json_pretty(&result);
        }
        return Ok(());
    }

    if matches!(ctx.mode(), OutputMode::Quiet) {
        return Ok(());
    }

    if count == 0 {
        ctx.success("No dependency cycles detected.");
    } else if ctx.is_rich() {
        // Rich mode: Show cycles with red highlighting in a panel
        render_cycles_rich(ctx, &cycles, count);
    } else {
        // Plain mode: Simple text output
        ctx.warning(&format!("Found {count} dependency cycle(s):"));
        for (i, cycle) in cycles.iter().enumerate() {
            ctx.print_line(&format!("  {}. {}", i + 1, cycle.join(" -> ")));
        }
    }

    Ok(())
}

/// Render cycles in rich mode with red highlighting
fn render_cycles_rich(ctx: &OutputContext, cycles: &[Vec<String>], count: usize) {
    let theme = ctx.theme();

    let mut content = String::new();
    content.push_str(&format!(
        "[bold red]⚠ {} dependency cycle(s) detected:[/]\n\n",
        count
    ));

    for (i, cycle) in cycles.iter().enumerate() {
        // Format cycle path with arrows
        let cycle_path = cycle.join(" [red]→[/] ");
        content.push_str(&format!("[bold]Cycle {}:[/]\n", i + 1));
        content.push_str(&format!("  [red]{}[/]\n", cycle_path));

        // Add underline visual
        let path_len = cycle.iter().map(|s| s.len() + 4).sum::<usize>();
        content.push_str(&format!("  [red]{}[/]\n", "^".repeat(path_len.min(60))));

        if i < cycles.len() - 1 {
            content.push('\n');
        }
    }

    content.push_str("\n[dim]Suggestion: Remove one dependency from each cycle to break it.[/]");

    let panel = Panel::from_text(&content)
        .title(Text::new("Dependency Cycles"))
        .border_style(theme.error.clone());

    ctx.render(&panel);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::init_test_logging;
    use crate::model::{Issue, IssueType, Priority, Status};
    use chrono::{TimeZone, Utc};
    use std::collections::HashMap;
    use tracing::info;

    fn make_test_issue(id: &str, title: &str) -> Issue {
        Issue {
            id: id.to_string(),
            content_hash: None,
            title: title.to_string(),
            description: None,
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

    fn test_dep_list_item(issue_id: &str, depends_on_id: &str, priority: i32) -> DepListItem {
        DepListItem {
            issue_id: issue_id.to_string(),
            depends_on_id: depends_on_id.to_string(),
            dep_type: "blocks".to_string(),
            title: depends_on_id.to_string(),
            status: "open".to_string(),
            priority,
        }
    }

    #[test]
    fn test_dependency_type_parsing() {
        init_test_logging();
        info!("test_dependency_type_parsing: starting");
        assert_eq!(
            "blocks".parse::<DependencyType>().unwrap(),
            DependencyType::Blocks
        );
        assert_eq!(
            "parent-child".parse::<DependencyType>().unwrap(),
            DependencyType::ParentChild
        );
        assert_eq!(
            "related".parse::<DependencyType>().unwrap(),
            DependencyType::Related
        );
        assert_eq!(
            "duplicates".parse::<DependencyType>().unwrap(),
            DependencyType::Duplicates
        );
        info!("test_dependency_type_parsing: assertions passed");
    }

    #[test]
    fn test_blocking_dependency_types() {
        init_test_logging();
        info!("test_blocking_dependency_types: starting");
        assert!(DependencyType::Blocks.is_blocking());
        assert!(DependencyType::ParentChild.is_blocking());
        assert!(!DependencyType::Related.is_blocking());
        assert!(!DependencyType::Duplicates.is_blocking());
        info!("test_blocking_dependency_types: assertions passed");
    }

    #[test]
    fn test_normalize_dep_type_filter_canonicalizes_standard_types() {
        assert_eq!(
            normalize_dep_type_filter("Parent-Child").unwrap(),
            "parent-child"
        );
        assert_eq!(normalize_dep_type_filter("BLOCKS").unwrap(), "blocks");
    }

    #[test]
    fn test_normalize_dep_type_filter_rejects_unknown_types() {
        let err = normalize_dep_type_filter("parent_child").unwrap_err();
        assert!(matches!(err, BeadsError::Validation { field, .. } if field == "type"));
    }

    #[test]
    fn test_add_dependency() {
        init_test_logging();
        info!("test_add_dependency: starting");
        let mut storage = SqliteStorage::open_memory().unwrap();

        let issue1 = make_test_issue("bd-001", "Issue 1");
        let issue2 = make_test_issue("bd-002", "Issue 2");
        storage.create_issue(&issue1, "tester").unwrap();
        storage.create_issue(&issue2, "tester").unwrap();

        // Add dependency: bd-001 depends on bd-002 (blocks)
        let added = storage
            .add_dependency("bd-001", "bd-002", "blocks", "tester")
            .unwrap();
        assert!(added);

        // Adding same dependency again should return false
        let added_again = storage
            .add_dependency("bd-001", "bd-002", "blocks", "tester")
            .unwrap();
        assert!(!added_again);
        info!("test_add_dependency: assertions passed");
    }

    #[test]
    fn test_remove_dependency() {
        init_test_logging();
        info!("test_remove_dependency: starting");
        let mut storage = SqliteStorage::open_memory().unwrap();

        let issue1 = make_test_issue("bd-001", "Issue 1");
        let issue2 = make_test_issue("bd-002", "Issue 2");
        storage.create_issue(&issue1, "tester").unwrap();
        storage.create_issue(&issue2, "tester").unwrap();

        storage
            .add_dependency("bd-001", "bd-002", "blocks", "tester")
            .unwrap();

        let removed = storage
            .remove_dependency("bd-001", "bd-002", "tester")
            .unwrap();
        assert!(removed);

        // Removing again should return false
        let removed_again = storage
            .remove_dependency("bd-001", "bd-002", "tester")
            .unwrap();
        assert!(!removed_again);
        info!("test_remove_dependency: assertions passed");
    }

    #[test]
    fn test_get_dependencies() {
        init_test_logging();
        info!("test_get_dependencies: starting");
        let mut storage = SqliteStorage::open_memory().unwrap();

        let issue1 = make_test_issue("bd-001", "Issue 1");
        let issue2 = make_test_issue("bd-002", "Issue 2");
        let issue3 = make_test_issue("bd-003", "Issue 3");
        storage.create_issue(&issue1, "tester").unwrap();
        storage.create_issue(&issue2, "tester").unwrap();
        storage.create_issue(&issue3, "tester").unwrap();

        // bd-001 depends on bd-002 and bd-003
        storage
            .add_dependency("bd-001", "bd-002", "blocks", "tester")
            .unwrap();
        storage
            .add_dependency("bd-001", "bd-003", "blocks", "tester")
            .unwrap();

        let deps = storage.get_dependencies("bd-001").unwrap();
        assert_eq!(deps.len(), 2);
        assert!(deps.contains(&"bd-002".to_string()));
        assert!(deps.contains(&"bd-003".to_string()));
        info!("test_get_dependencies: assertions passed");
    }

    #[test]
    fn test_get_dependents() {
        init_test_logging();
        info!("test_get_dependents: starting");
        let mut storage = SqliteStorage::open_memory().unwrap();

        let issue1 = make_test_issue("bd-001", "Issue 1");
        let issue2 = make_test_issue("bd-002", "Issue 2");
        let issue3 = make_test_issue("bd-003", "Issue 3");
        storage.create_issue(&issue1, "tester").unwrap();
        storage.create_issue(&issue2, "tester").unwrap();
        storage.create_issue(&issue3, "tester").unwrap();

        // bd-002 and bd-003 depend on bd-001
        storage
            .add_dependency("bd-002", "bd-001", "blocks", "tester")
            .unwrap();
        storage
            .add_dependency("bd-003", "bd-001", "blocks", "tester")
            .unwrap();

        let dependents = storage.get_dependents("bd-001").unwrap();
        assert_eq!(dependents.len(), 2);
        assert!(dependents.contains(&"bd-002".to_string()));
        assert!(dependents.contains(&"bd-003".to_string()));
        info!("test_get_dependents: assertions passed");
    }

    #[test]
    fn test_cycle_detection_simple() {
        init_test_logging();
        info!("test_cycle_detection_simple: starting");
        let mut storage = SqliteStorage::open_memory().unwrap();

        let issue1 = make_test_issue("bd-001", "Issue 1");
        let issue2 = make_test_issue("bd-002", "Issue 2");
        storage.create_issue(&issue1, "tester").unwrap();
        storage.create_issue(&issue2, "tester").unwrap();

        // bd-001 depends on bd-002
        storage
            .add_dependency("bd-001", "bd-002", "blocks", "tester")
            .unwrap();

        // bd-002 depends on bd-001 would create a cycle
        let would_cycle = storage
            .would_create_cycle("bd-002", "bd-001", true)
            .unwrap();
        assert!(would_cycle);
        info!("test_cycle_detection_simple: assertions passed");
    }

    #[test]
    fn test_cycle_detection_transitive() {
        init_test_logging();
        info!("test_cycle_detection_transitive: starting");
        let mut storage = SqliteStorage::open_memory().unwrap();

        let issue1 = make_test_issue("bd-001", "Issue 1");
        let issue2 = make_test_issue("bd-002", "Issue 2");
        let issue3 = make_test_issue("bd-003", "Issue 3");
        storage.create_issue(&issue1, "tester").unwrap();
        storage.create_issue(&issue2, "tester").unwrap();
        storage.create_issue(&issue3, "tester").unwrap();

        // bd-001 -> bd-002 -> bd-003
        storage
            .add_dependency("bd-001", "bd-002", "blocks", "tester")
            .unwrap();
        storage
            .add_dependency("bd-002", "bd-003", "blocks", "tester")
            .unwrap();

        // bd-003 -> bd-001 would create a cycle
        let would_cycle = storage
            .would_create_cycle("bd-003", "bd-001", true)
            .unwrap();
        assert!(would_cycle);

        // bd-003 -> bd-002 would also create a cycle
        let would_cycle = storage
            .would_create_cycle("bd-003", "bd-002", true)
            .unwrap();
        assert!(would_cycle);
        info!("test_cycle_detection_transitive: assertions passed");
    }

    #[test]
    fn test_no_false_positive_cycle() {
        init_test_logging();
        info!("test_no_false_positive_cycle: starting");
        let mut storage = SqliteStorage::open_memory().unwrap();

        let issue1 = make_test_issue("bd-001", "Issue 1");
        let issue2 = make_test_issue("bd-002", "Issue 2");
        let issue3 = make_test_issue("bd-003", "Issue 3");
        storage.create_issue(&issue1, "tester").unwrap();
        storage.create_issue(&issue2, "tester").unwrap();
        storage.create_issue(&issue3, "tester").unwrap();

        // bd-001 -> bd-002
        storage
            .add_dependency("bd-001", "bd-002", "blocks", "tester")
            .unwrap();

        // bd-003 -> bd-002 should NOT be a cycle
        let would_cycle = storage
            .would_create_cycle("bd-003", "bd-002", true)
            .unwrap();
        assert!(!would_cycle);
        info!("test_no_false_positive_cycle: assertions passed");
    }

    #[test]
    fn test_dep_action_result_json() {
        init_test_logging();
        info!("test_dep_action_result_json: starting");
        let result = DepActionResult {
            status: "ok".to_string(),
            issue_id: "bd-001".to_string(),
            depends_on_id: "bd-002".to_string(),
            dep_type: "blocks".to_string(),
            action: "added".to_string(),
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"status\":\"ok\""));
        assert!(json.contains("\"issue_id\":\"bd-001\""));
        assert!(json.contains("\"type\":\"blocks\"")); // Note: renamed field
        info!("test_dep_action_result_json: assertions passed");
    }

    #[test]
    fn test_dep_list_item_json() {
        init_test_logging();
        info!("test_dep_list_item_json: starting");
        let item = DepListItem {
            issue_id: "bd-001".to_string(),
            depends_on_id: "bd-002".to_string(),
            dep_type: "blocks".to_string(),
            title: "Test Issue".to_string(),
            status: "open".to_string(),
            priority: 2,
        };

        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("\"type\":\"blocks\"")); // Renamed field
        assert!(json.contains("\"priority\":2"));
        info!("test_dep_list_item_json: assertions passed");
    }

    #[test]
    fn test_sort_dep_list_items_for_human_orders_by_priority() {
        init_test_logging();
        info!("test_sort_dep_list_items_for_human_orders_by_priority: starting");
        let mut items = vec![
            test_dep_list_item("bd-root", "bd-low", 4),
            test_dep_list_item("bd-root", "bd-critical", 0),
            test_dep_list_item("bd-root", "bd-medium", 2),
        ];

        sort_dep_list_items_for_human(&mut items);

        let sorted_ids: Vec<_> = items
            .iter()
            .map(|item| item.depends_on_id.as_str())
            .collect();
        assert_eq!(sorted_ids, ["bd-critical", "bd-medium", "bd-low"]);
        info!("test_sort_dep_list_items_for_human_orders_by_priority: assertions passed");
    }

    #[test]
    fn test_sort_dep_list_items_for_human_uses_ids_as_tiebreakers() {
        init_test_logging();
        info!("test_sort_dep_list_items_for_human_uses_ids_as_tiebreakers: starting");
        let mut items = vec![
            test_dep_list_item("bd-root", "bd-b", 1),
            test_dep_list_item("bd-root", "bd-a", 1),
            test_dep_list_item("bd-root", "bd-c", 1),
        ];

        sort_dep_list_items_for_human(&mut items);

        let sorted_ids: Vec<_> = items
            .iter()
            .map(|item| item.depends_on_id.as_str())
            .collect();
        assert_eq!(sorted_ids, ["bd-a", "bd-b", "bd-c"]);
        info!("test_sort_dep_list_items_for_human_uses_ids_as_tiebreakers: assertions passed");
    }

    #[test]
    fn test_cycles_result_json() {
        init_test_logging();
        info!("test_cycles_result_json: starting");
        let result = CyclesResult {
            cycles: vec![
                vec!["bd-001".to_string(), "bd-002".to_string()],
                vec![
                    "bd-003".to_string(),
                    "bd-004".to_string(),
                    "bd-005".to_string(),
                ],
            ],
            count: 2,
        };

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"count\":2"));
        assert!(json.contains("bd-001"));
        info!("test_cycles_result_json: assertions passed");
    }

    #[test]
    fn test_external_dependency_prefix_check() {
        init_test_logging();
        info!("test_external_dependency_prefix_check: starting");
        let external = "external:jira-123";
        assert!(external.starts_with("external:"));

        let normal = "bd-001";
        assert!(!normal.starts_with("external:"));
        info!("test_external_dependency_prefix_check: assertions passed");
    }

    #[test]
    fn test_dep_direction_default() {
        init_test_logging();
        info!("test_dep_direction_default: starting");
        let direction = DepDirection::default();
        assert_eq!(direction, DepDirection::Down);
        info!("test_dep_direction_default: assertions passed");
    }

    #[test]
    fn test_apply_external_dep_list_metadata_sets_status_and_title() {
        init_test_logging();
        info!("test_apply_external_dep_list_metadata_sets_status_and_title: starting");
        let mut items = vec![
            DepListItem {
                issue_id: "bd-001".to_string(),
                depends_on_id: "external:proj:cap".to_string(),
                dep_type: "blocks".to_string(),
                title: String::new(),
                status: "open".to_string(),
                priority: 2,
            },
            DepListItem {
                issue_id: "bd-002".to_string(),
                depends_on_id: "external:proj:cap2".to_string(),
                dep_type: "blocks".to_string(),
                title: String::new(),
                status: "open".to_string(),
                priority: 2,
            },
        ];

        let mut statuses = HashMap::new();
        statuses.insert("external:proj:cap".to_string(), true);
        statuses.insert("external:proj:cap2".to_string(), false);

        apply_external_dep_list_metadata(&mut items, &statuses);

        assert_eq!(items[0].status, "closed");
        assert_eq!(items[0].title, "✓ proj:cap");
        assert_eq!(items[1].status, "blocked");
        assert_eq!(items[1].title, "⏳ proj:cap2");
        info!("test_apply_external_dep_list_metadata_sets_status_and_title: assertions passed");
    }

    #[test]
    fn test_apply_external_dep_list_metadata_preserves_title() {
        init_test_logging();
        info!("test_apply_external_dep_list_metadata_preserves_title: starting");
        let mut items = vec![DepListItem {
            issue_id: "bd-001".to_string(),
            depends_on_id: "external:proj:cap".to_string(),
            dep_type: "blocks".to_string(),
            title: "Already set".to_string(),
            status: "open".to_string(),
            priority: 2,
        }];
        let mut statuses = HashMap::new();
        statuses.insert("external:proj:cap".to_string(), false);

        apply_external_dep_list_metadata(&mut items, &statuses);

        assert_eq!(items[0].status, "blocked");
        assert_eq!(items[0].title, "Already set");
        info!("test_apply_external_dep_list_metadata_preserves_title: assertions passed");
    }

    #[test]
    fn test_apply_external_dep_list_metadata_rewrites_generated_placeholder_title() {
        init_test_logging();
        info!(
            "test_apply_external_dep_list_metadata_rewrites_generated_placeholder_title: starting"
        );
        let mut items = vec![DepListItem {
            issue_id: "bd-001".to_string(),
            depends_on_id: "external:proj:cap".to_string(),
            dep_type: "blocks".to_string(),
            title: "proj:cap".to_string(),
            status: "open".to_string(),
            priority: 2,
        }];
        let mut statuses = HashMap::new();
        statuses.insert("external:proj:cap".to_string(), false);

        apply_external_dep_list_metadata(&mut items, &statuses);

        assert_eq!(items[0].status, "blocked");
        assert_eq!(items[0].title, "⏳ proj:cap");
        info!(
            "test_apply_external_dep_list_metadata_rewrites_generated_placeholder_title: assertions passed"
        );
    }

    #[test]
    fn test_apply_external_dep_list_metadata_external_issue_id() {
        init_test_logging();
        info!("test_apply_external_dep_list_metadata_external_issue_id: starting");
        let mut items = vec![DepListItem {
            issue_id: "external:proj:cap".to_string(),
            depends_on_id: "bd-001".to_string(),
            dep_type: "blocks".to_string(),
            title: String::new(),
            status: "open".to_string(),
            priority: 2,
        }];
        let mut statuses = HashMap::new();
        statuses.insert("external:proj:cap".to_string(), true);

        apply_external_dep_list_metadata(&mut items, &statuses);

        assert_eq!(items[0].status, "closed");
        assert_eq!(items[0].title, "✓ proj:cap");
        info!("test_apply_external_dep_list_metadata_external_issue_id: assertions passed");
    }

    #[test]
    fn test_dep_list_section_title_uses_neutral_dependents_label() {
        init_test_logging();
        info!("test_dep_list_section_title_uses_neutral_dependents_label: starting");
        assert_eq!(dep_list_section_title(true, 2), "Dependencies (2):");
        assert_eq!(dep_list_section_title(false, 3), "Dependents (3):");
        info!("test_dep_list_section_title_uses_neutral_dependents_label: assertions passed");
    }

    #[test]
    fn test_dep_list_panel_title_matches_direction() {
        init_test_logging();
        info!("test_dep_list_panel_title_matches_direction: starting");
        assert_eq!(
            dep_list_panel_title(DepDirection::Down, "bd-1"),
            "Dependencies for bd-1"
        );
        assert_eq!(
            dep_list_panel_title(DepDirection::Up, "bd-1"),
            "Dependents for bd-1"
        );
        assert_eq!(
            dep_list_panel_title(DepDirection::Both, "bd-1"),
            "Dependency relations for bd-1"
        );
        info!("test_dep_list_panel_title_matches_direction: assertions passed");
    }

    #[test]
    fn test_dep_list_status_label_formats_known_statuses() {
        init_test_logging();
        info!("test_dep_list_status_label_formats_known_statuses: starting");
        assert_eq!(dep_list_status_label("open"), "[open]");
        assert_eq!(dep_list_status_label("closed"), "[closed] ✓");
        assert_eq!(dep_list_status_label("custom"), "custom");
        info!("test_dep_list_status_label_formats_known_statuses: assertions passed");
    }

    #[test]
    fn test_dep_tree_truncated_only_when_children_are_omitted() {
        init_test_logging();
        info!("test_dep_tree_truncated_only_when_children_are_omitted: starting");
        assert!(!dep_tree_truncated(2, 2, 0));
        assert!(dep_tree_truncated(2, 2, 1));
        assert!(!dep_tree_truncated(1, 2, 3));
        info!("test_dep_tree_truncated_only_when_children_are_omitted: assertions passed");
    }

    #[test]
    fn test_resolve_dep_tree_node_metadata_missing_internal_issue() {
        init_test_logging();
        info!("test_resolve_dep_tree_node_metadata_missing_internal_issue: starting");
        let storage = SqliteStorage::open_memory().unwrap();
        let root_issue = make_test_issue("bd-root", "Root");
        let statuses = HashMap::new();

        let (title, priority, status) = resolve_dep_tree_node_metadata(
            &storage,
            "bd-root",
            &root_issue,
            "bd-missing",
            &statuses,
        )
        .unwrap();

        assert_eq!(title, "[missing issue: bd-missing]");
        assert_eq!(priority, 2);
        assert_eq!(status, "deleted");
        info!("test_resolve_dep_tree_node_metadata_missing_internal_issue: assertions passed");
    }

    #[test]
    fn test_dep_direction_variants() {
        init_test_logging();
        info!("test_dep_direction_variants: starting");
        assert!(matches!(DepDirection::Down, DepDirection::Down));
        assert!(matches!(DepDirection::Up, DepDirection::Up));
        assert!(matches!(DepDirection::Both, DepDirection::Both));
        info!("test_dep_direction_variants: assertions passed");
    }
}
