//! MCP (Model Context Protocol) server for beads_rust.
//!
//! Exposes the issue tracker as an MCP server so that AI agents can
//! query, create, and manage issues through the standard MCP protocol
//! instead of shelling out to the `br` CLI.
//!
//! This module is feature-gated behind `mcp` and is **not** included
//! in the default feature set.

mod prompts;
mod resources;
mod tools;

use std::path::{Path, PathBuf};

use fastmcp_rust::{McpError, McpErrorCode};
use serde_json::json;

use crate::storage::SqliteStorage;
use crate::{BeadsError, config};

/// Map any `Display` error into a flat `McpError::tool_error`.
///
/// Used by resources and prompts for non-structured error mapping.
/// Tools use the richer `beads_to_mcp` in `tools.rs` instead.
pub(super) fn to_mcp(err: impl std::fmt::Display) -> McpError {
    McpError::tool_error(err.to_string())
}

fn auto_flush_mcp_error(
    beads_dir: &Path,
    jsonl_path: &Path,
    err: impl std::fmt::Display,
) -> McpError {
    let message = "Mutation succeeded, but automatic JSONL export failed";
    McpError::with_data(
        McpErrorCode::ToolExecutionError,
        message,
        json!({
            "error_type": "AUTO_FLUSH_FAILED",
            "recoverable": true,
            "message": message,
            "beads_dir": beads_dir.display().to_string(),
            "jsonl_path": jsonl_path.display().to_string(),
            "error": err.to_string(),
            "recovery": "Run br sync --flush-only after fixing the export problem before committing .beads/issues.jsonl",
        }),
    )
}

fn sync_lock_mcp_error(
    beads_dir: &Path,
    jsonl_path: &Path,
    err: impl std::fmt::Display,
) -> McpError {
    let message = "Mutation was not attempted because the JSONL sync lock is unavailable";
    McpError::with_data(
        McpErrorCode::ToolExecutionError,
        message,
        json!({
            "error_type": "SYNC_LOCK_UNAVAILABLE",
            "recoverable": true,
            "message": message,
            "beads_dir": beads_dir.display().to_string(),
            "jsonl_path": jsonl_path.display().to_string(),
            "error": err.to_string(),
            "recovery": "Retry after the active sync finishes or fix the .beads/.sync.lock path.",
        }),
    )
}

fn sync_lock_busy_error(beads_dir: &Path) -> BeadsError {
    BeadsError::Config(format!(
        "Automatic JSONL export skipped because sync lock at {} is held by another process",
        beads_dir.join(".sync.lock").display()
    ))
}

fn dirty_auto_flush_incomplete_error(remaining_dirty: usize) -> BeadsError {
    BeadsError::Config(format!(
        "Automatic JSONL export did not flush {remaining_dirty} dirty issue(s)"
    ))
}

/// Shared configuration available to every MCP handler.
///
/// Storage is intentionally **not** held open: `fsqlite::Connection` uses
/// `Rc` internally and therefore cannot satisfy `Send + Sync`.  Each
/// handler call opens a fresh connection via [`open_storage`].
pub struct BeadsState {
    pub db_path: PathBuf,
    pub beads_dir: PathBuf,
    pub jsonl_path: PathBuf,
    pub allow_external_jsonl: bool,
    pub actor: String,
    pub issue_prefix: Option<String>,
}

impl BeadsState {
    /// Open a fresh `SqliteStorage` connection.
    ///
    /// # Errors
    ///
    /// Returns an error if the database file cannot be opened.
    pub fn open_storage(&self) -> crate::Result<SqliteStorage> {
        SqliteStorage::open(&self.db_path)
    }

    /// Execute a mutating closure against the storage, acquiring the cross-process
    /// write lock and triggering an auto-flush upon success.
    pub fn with_mutation<F, R>(&self, mut f: F) -> fastmcp_rust::McpResult<R>
    where
        F: FnMut(&mut SqliteStorage) -> fastmcp_rust::McpResult<R>,
    {
        // 1. Acquire the cross-process write lock.
        let _write_lock = crate::sync::blocking_write_lock(&self.beads_dir).map_err(to_mcp)?;

        // 2. Acquire the sync lock before committing a mutation. MCP writes
        // should not report success when JSONL export is known to be unguarded
        // or impossible.
        let _sync_lock = match crate::sync::try_sync_lock(&self.beads_dir) {
            Ok(Some(lock)) => lock,
            Ok(None) => {
                return Err(sync_lock_mcp_error(
                    &self.beads_dir,
                    &self.jsonl_path,
                    sync_lock_busy_error(&self.beads_dir),
                ));
            }
            Err(err) => {
                return Err(sync_lock_mcp_error(&self.beads_dir, &self.jsonl_path, err));
            }
        };

        // 3. Open storage.
        let mut storage = self.open_storage().map_err(to_mcp)?;

        // 4. Execute the mutation.
        let result = f(&mut storage)?;

        // 5. Auto-flush.
        let dirty_before_flush = storage.get_dirty_issue_count().map_err(to_mcp)?;
        let flush_result = crate::sync::auto_flush(
            &mut storage,
            &self.beads_dir,
            &self.jsonl_path,
            self.allow_external_jsonl,
        )
        .map_err(|err| auto_flush_mcp_error(&self.beads_dir, &self.jsonl_path, err))?;

        if dirty_before_flush > 0 && !flush_result.flushed {
            let remaining_dirty = storage.get_dirty_issue_count().map_err(to_mcp)?;
            if remaining_dirty > 0 {
                return Err(auto_flush_mcp_error(
                    &self.beads_dir,
                    &self.jsonl_path,
                    dirty_auto_flush_incomplete_error(remaining_dirty),
                ));
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;
    use std::path::PathBuf;
    use std::rc::Rc;

    use chrono::Utc;
    use tempfile::TempDir;

    use super::*;
    use crate::model::Issue;

    fn test_issue(id: &str, title: &str) -> Issue {
        let now = Utc::now();
        Issue {
            id: id.to_string(),
            title: title.to_string(),
            created_at: now,
            updated_at: now,
            created_by: Some("mcp-test".to_string()),
            ..Issue::default()
        }
    }

    fn test_state(temp: &TempDir, jsonl_path: PathBuf) -> BeadsState {
        let beads_dir = temp.path().join(".beads");
        fs::create_dir_all(&beads_dir).unwrap();
        let db_path = beads_dir.join("beads.db");
        SqliteStorage::open(&db_path).unwrap();

        BeadsState {
            db_path,
            beads_dir,
            jsonl_path,
            allow_external_jsonl: false,
            actor: "mcp-test".to_string(),
            issue_prefix: Some("br".to_string()),
        }
    }

    #[test]
    fn with_mutation_requires_openable_sync_lock_before_mutating() {
        let temp = TempDir::new().unwrap();
        let beads_dir = temp.path().join(".beads");
        let jsonl_path = beads_dir.join("issues.jsonl");
        let state = test_state(&temp, jsonl_path);
        fs::create_dir(state.beads_dir.join(".sync.lock")).unwrap();
        let called = Rc::new(Cell::new(false));
        let called_for_closure = Rc::clone(&called);

        let err = state
            .with_mutation(|storage| {
                called_for_closure.set(true);
                storage
                    .create_issue(
                        &test_issue("br-mcp-lock", "should not be created"),
                        "mcp-test",
                    )
                    .map_err(to_mcp)?;
                Ok(())
            })
            .unwrap_err();

        assert!(
            !called.get(),
            "mutation closure must not run without sync lock"
        );
        assert_eq!(err.code, McpErrorCode::ToolExecutionError);
        assert_eq!(
            err.data
                .as_ref()
                .and_then(|data| data.get("error_type"))
                .and_then(serde_json::Value::as_str),
            Some("SYNC_LOCK_UNAVAILABLE")
        );
        let storage = SqliteStorage::open(&state.db_path).unwrap();
        assert!(!storage.id_exists("br-mcp-lock").unwrap());
    }

    #[test]
    fn with_mutation_reports_auto_flush_failure_and_preserves_dirty_state() {
        let temp = TempDir::new().unwrap();
        let beads_dir = temp.path().join(".beads");
        let jsonl_path = beads_dir.join("issues.jsonl");
        let state = test_state(&temp, jsonl_path.clone());
        fs::write(
            &jsonl_path,
            "<<<<<<< HEAD\n{}\n=======\n{}\n>>>>>>> branch\n",
        )
        .unwrap();

        let err = state
            .with_mutation(|storage| {
                storage
                    .create_issue(&test_issue("br-mcp-dirty", "dirty issue"), "mcp-test")
                    .map_err(to_mcp)?;
                Ok(())
            })
            .unwrap_err();

        assert_eq!(err.code, McpErrorCode::ToolExecutionError);
        assert_eq!(
            err.data
                .as_ref()
                .and_then(|data| data.get("error_type"))
                .and_then(serde_json::Value::as_str),
            Some("AUTO_FLUSH_FAILED")
        );

        let storage = SqliteStorage::open(&state.db_path).unwrap();
        assert!(storage.id_exists("br-mcp-dirty").unwrap());
        assert_eq!(storage.get_dirty_issue_count().unwrap(), 1);
        let jsonl = fs::read_to_string(jsonl_path).unwrap();
        assert!(jsonl.contains("<<<<<<<"));
    }
}

/// CLI arguments for `br serve`.
#[derive(clap::Args, Debug, Clone)]
pub struct ServeArgs {
    /// Actor name for mutations (defaults to "mcp")
    #[arg(long, default_value = "mcp")]
    pub actor: String,
}

/// Entry point: build and run the MCP server on stdio.
///
/// # Errors
///
/// Returns an error if the beads workspace is not initialised or storage
/// cannot be opened.
pub fn run_serve(args: &ServeArgs, overrides: &config::CliOverrides) -> crate::Result<()> {
    let beads_dir = config::discover_beads_dir_with_cli(overrides)?;
    let res = config::open_storage_with_cli(&beads_dir, overrides)?;

    let prefix = res.storage.get_config("issue_prefix")?;
    let db_path = res.paths.db_path.clone();
    let jsonl_path = res.paths.jsonl_path.clone();
    let allow_external_jsonl =
        config::implicit_external_jsonl_allowed(&beads_dir, &db_path, &jsonl_path);

    // Eagerly drop the bootstrap connection; handlers will open their own.
    drop(res.storage);

    let state = std::sync::Arc::new(BeadsState {
        db_path,
        beads_dir,
        jsonl_path,
        allow_external_jsonl,
        actor: args.actor.clone(),
        issue_prefix: prefix,
    });

    let server = fastmcp_rust::Server::new("br", env!("CARGO_PKG_VERSION"))
        .instructions(
            "beads_rust (br) issue tracker MCP server.\n\n\
             Use tools to query, create, and manage issues. All mutations are \
             recorded with full audit trails.\n\n\
             Getting started:\n\
             1. Call project_overview to understand the project state\n\
             2. Read beads://schema for valid field values and bead anatomy guidance\n\
             3. Read beads://labels to discover existing labels\n\
             4. Use list_issues to find specific issues\n\n\
             Discovery resources: beads://project/info, beads://schema, \
             beads://labels, beads://issues/ready, beads://issues/blocked, \
             beads://issues/deferred, beads://issues/bottlenecks, \
             beads://graph/health, beads://events/recent\n\n\
             Guided workflows:\n\
             - 'triage' — backlog triage (blocked, unassigned, deferred)\n\
             - 'status_report' — project status report generation\n\
             - 'plan_next_work' — graph-aware work planning (bottlenecks, quick wins)\n\
             - 'polish_backlog' — review issue quality and dependency health",
        )
        // Tools (7 — at the ≤7 cluster ceiling)
        .tool(tools::ListIssuesTool::new(state.clone()))
        .tool(tools::ShowIssueTool::new(state.clone()))
        .tool(tools::CreateIssueTool::new(state.clone()))
        .tool(tools::UpdateIssueTool::new(state.clone()))
        .tool(tools::CloseIssueTool::new(state.clone()))
        .tool(tools::ManageDependenciesTool::new(state.clone()))
        .tool(tools::ProjectOverviewTool::new(state.clone()))
        // Resources (11)
        .resource(resources::ProjectInfoResource::new(state.clone()))
        .resource(resources::IssueResource::new(state.clone()))
        .resource(resources::SchemaResource)
        .resource(resources::LabelsResource::new(state.clone()))
        .resource(resources::ReadyIssuesResource::new(state.clone()))
        .resource(resources::BlockedIssuesResource::new(state.clone()))
        .resource(resources::InProgressResource::new(state.clone()))
        .resource(resources::EventsResource::new(state.clone()))
        .resource(resources::DeferredIssuesResource::new(state.clone()))
        .resource(resources::GraphHealthResource::new(state.clone()))
        .resource(resources::BottlenecksResource::new(state.clone()))
        // Prompts (4)
        .prompt(prompts::TriagePrompt::new(state.clone()))
        .prompt(prompts::StatusReportPrompt::new(state.clone()))
        .prompt(prompts::PlanNextWorkPrompt::new(state.clone()))
        .prompt(prompts::PolishBacklogPrompt::new(state))
        .build();

    server.run_stdio();
}
