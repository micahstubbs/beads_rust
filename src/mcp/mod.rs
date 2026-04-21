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

use std::path::PathBuf;

use fastmcp_rust::McpError;

use crate::config;
use crate::storage::SqliteStorage;

/// Map any `Display` error into a flat `McpError::tool_error`.
///
/// Used by resources and prompts for non-structured error mapping.
/// Tools use the richer `beads_to_mcp` in `tools.rs` instead.
pub(super) fn to_mcp(err: impl std::fmt::Display) -> McpError {
    McpError::tool_error(err.to_string())
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

        // 2. Open storage.
        let mut storage = self.open_storage().map_err(to_mcp)?;

        // 3. Execute the mutation.
        let result = f(&mut storage)?;

        // 4. Auto-flush.
        let _sync_lock = crate::sync::try_sync_lock(&self.beads_dir);
        if let Err(e) = crate::sync::auto_flush(
            &mut storage,
            &self.beads_dir,
            &self.jsonl_path,
            self.allow_external_jsonl,
        ) {
            tracing::debug!(?e, "MCP auto-flush failed (non-fatal)");
        }

        Ok(result)
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
