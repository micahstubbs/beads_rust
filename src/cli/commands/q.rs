use crate::cli::QuickArgs;
use crate::config;
use crate::error::{BeadsError, Result};
use crate::model::{Dependency, DependencyType, Issue, IssueType, Priority, Status};
use crate::output::{OutputContext, OutputMode};
use crate::util::id::{IdGenerator, child_id};
use crate::validation::LabelValidator;
use chrono::Utc;
use rich_rust::prelude::*;
use std::collections::HashSet;
use std::str::FromStr;

fn split_labels(values: &[String]) -> Vec<String> {
    let mut labels = Vec::new();
    for value in values {
        for part in value.split(',') {
            let label = part.trim();
            if !label.is_empty() {
                labels.push(label.to_string());
            }
        }
    }
    labels
}

fn push_unique_label(labels: &mut Vec<String>, label: &str) {
    if !labels.iter().any(|existing| existing == label) {
        labels.push(label.to_string());
    }
}

/// Execute the quick capture command.
///
/// # Errors
///
/// Returns an error if validation fails, the database cannot be opened, or creation fails.
#[allow(clippy::too_many_lines)]
pub fn execute(args: QuickArgs, cli: &config::CliOverrides, ctx: &OutputContext) -> Result<()> {
    let title = args.title.join(" ").trim().to_string();
    if title.is_empty() {
        return Err(BeadsError::validation("title", "cannot be empty"));
    }

    let beads_dir = config::discover_beads_dir_with_cli(cli)?;
    let mut storage_ctx = config::open_storage_with_cli(&beads_dir, cli)?;
    let layer = config::load_config(&beads_dir, Some(&storage_ctx.storage), cli)?;
    let id_config = config::id_config_from_layer(&layer);
    let default_priority = config::default_priority_from_layer(&layer)?;
    let default_issue_type = config::default_issue_type_from_layer(&layer)?;
    let storage = &mut storage_ctx.storage;

    let priority = if let Some(p) = args.priority {
        Priority::from_str(&p)?
    } else {
        default_priority
    };

    let issue_type = if let Some(t) = args.type_ {
        IssueType::from_str(&t)?
    } else {
        default_issue_type
    };

    let now = Utc::now();

    // When a parent is specified, generate a child ID (parent.1, parent.2, etc.)
    let id = if let Some(ref parent_id) = args.parent {
        if !storage.id_exists(parent_id)? {
            return Err(BeadsError::IssueNotFound {
                id: parent_id.clone(),
            });
        }
        let next_num = storage.next_child_number(parent_id)?;
        let candidate = child_id(parent_id, next_num);
        if storage.id_exists(&candidate)? {
            let mut num = next_num + 1;
            loop {
                let alt = child_id(parent_id, num);
                if !storage.id_exists(&alt)? {
                    break alt;
                }
                num += 1;
                if num > next_num.saturating_add(100) {
                    return Err(BeadsError::validation(
                        "parent",
                        "could not find available child ID",
                    ));
                }
            }
        } else {
            candidate
        }
    } else {
        let id_gen = IdGenerator::new(id_config);
        let count = storage.count_issues()?;
        let existing_ids: HashSet<String> = storage.get_all_ids()?.into_iter().collect();
        id_gen.generate(&title, None, None, now, count, |candidate| {
            existing_ids.contains(candidate)
        })
    };

    let mut valid_labels = Vec::new();
    let labels = split_labels(&args.labels);
    for label in labels {
        if let Err(err) = LabelValidator::validate(&label) {
            eprintln!("Warning: invalid label '{label}': {}", err.message);
            continue;
        }
        push_unique_label(&mut valid_labels, &label);
    }

    let mut issue = Issue {
        id,
        title,
        description: args.description,
        status: Status::Open,
        priority,
        issue_type,
        created_at: now,
        updated_at: now,
        content_hash: None,
        design: None,
        acceptance_criteria: None,
        notes: None,
        assignee: None,
        owner: None,
        estimated_minutes: args.estimate,
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
        labels: valid_labels,
        dependencies: vec![],
        comments: vec![],
    };

    // Resolve actor and set created_by
    let actor = config::resolve_actor(&layer);
    issue.created_by = Some(actor.clone());

    // Parent dependency
    if let Some(ref parent_id) = args.parent {
        // Double-check cycle even though we are a new issue, to catch logic errors
        // and ensure the storage would_create_cycle works correctly for prospective links
        if storage.would_create_cycle(&issue.id, parent_id, true)? {
            return Err(BeadsError::DependencyCycle {
                path: format!("{} -> {}", issue.id, parent_id),
            });
        }

        issue.dependencies.push(Dependency {
            issue_id: issue.id.clone(),
            depends_on_id: parent_id.clone(),
            dep_type: DependencyType::ParentChild,
            created_at: now,
            created_by: Some(actor.clone()),
            metadata: None,
            thread_id: None,
        });
    }

    // Compute content hash
    issue.content_hash = Some(issue.compute_content_hash());

    storage.create_issue(&issue, &actor)?;

    // Output
    if ctx.is_json() {
        let output = serde_json::json!({
            "id": issue.id,
            "title": issue.title,
        });
        ctx.json(&output);
    } else if matches!(ctx.mode(), OutputMode::Rich) {
        render_quick_created_rich(&issue.id, &issue.title, ctx);
    } else {
        println!("{}", issue.id);
    }

    storage_ctx.flush_no_db_if_dirty()?;
    Ok(())
}

/// Render quick create result with rich formatting.
fn render_quick_created_rich(id: &str, title: &str, ctx: &OutputContext) {
    let console = Console::default();
    let theme = ctx.theme();
    let width = ctx.width();

    let mut content = Text::new("");
    content.append_styled("\u{2713} ", theme.success.clone());
    content.append_styled("Created ", theme.success.clone());
    content.append_styled(id, theme.emphasis.clone());
    content.append("\n");
    content.append_styled("  \"", theme.dimmed.clone());
    content.append(title);
    content.append_styled("\"", theme.dimmed.clone());
    content.append("\n");

    let panel = Panel::from_rich_text(&content, width)
        .title(Text::styled("Quick Create", theme.panel_title.clone()))
        .box_style(theme.box_style);

    console.print_renderable(&panel);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_unique_label_deduplicates_repeated_values() {
        let mut labels = Vec::new();
        push_unique_label(&mut labels, "backend");
        push_unique_label(&mut labels, "backend");
        push_unique_label(&mut labels, "ops");

        assert_eq!(labels, vec!["backend".to_string(), "ops".to_string()]);
    }
}
