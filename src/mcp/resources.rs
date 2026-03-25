//! MCP resource handlers for the beads issue tracker.
//!
//! Resources provide read-only discovery endpoints that agents can inspect
//! before calling tools.

use std::collections::HashMap;
use std::sync::Arc;

use fastmcp_rust::{
    McpContext, McpError, McpErrorCode, McpResult, Resource, ResourceContent, ResourceHandler,
    ResourceTemplate,
};
use serde_json::json;

use crate::error::StructuredError;
use crate::model::{Event, Issue, Status};
use crate::storage::{ListFilters, ReadyFilters, ReadySortPolicy, SqliteStorage};

use super::{BeadsState, to_mcp};

const IN_PROGRESS_RESOURCE_LIMIT: usize = 50;
const DEFERRED_RESOURCE_LIMIT: usize = 50;
const BOTTLENECK_ISSUES_DISPLAY_LIMIT: usize = 15;

/// Issues blocking this many or more others are considered high-fan-out.
const HIGH_FAN_OUT_THRESHOLD: usize = 3;
/// Issues not updated in this many days are considered stale.
const STALE_THRESHOLD_DAYS: i64 = 30;
/// Graph density above this is considered very high (heavily coupled).
const DENSITY_VERY_HIGH: f64 = 0.5;
/// Graph density above this is considered moderate.
const DENSITY_MODERATE: f64 = 0.2;
/// Chain depth above this is deep (hard to parallelize).
const CHAIN_DEPTH_DEEP: usize = 5;
/// Chain depth above this is moderate.
const CHAIN_DEPTH_MODERATE: usize = 2;

/// Build a structured "issue not found" error with fuzzy suggestions,
/// mirroring the tools.rs pattern for consistent agent UX.
fn issue_not_found_resource(storage: &SqliteStorage, id: &str) -> McpError {
    let all_ids = storage.get_all_ids().unwrap_or_default();
    let structured = StructuredError::issue_not_found(id, &all_ids);

    let mut data = json!({
        "error_type": "ISSUE_NOT_FOUND",
        "recoverable": true,
        "message": structured.message,
        "discovery_hint": "Use list_issues tool to find valid issue IDs",
    });

    if let Some(hint) = &structured.hint {
        data["hint"] = json!(hint);
    }
    if let Some(ctx) = &structured.context
        && let Some(similar) = ctx.get("similar_ids")
    {
        data["suggestions"] = similar.clone();
    }

    data["suggested_tool_calls"] = json!([{"tool": "list_issues", "arguments": {}}]);

    McpError::with_data(McpErrorCode::ToolExecutionError, structured.message, data)
}

fn count_and_sample_issues(
    storage: &SqliteStorage,
    mut filters: ListFilters,
    sample_limit: usize,
) -> McpResult<(usize, Vec<Issue>)> {
    let total = storage
        .count_issues_with_filters(&filters)
        .map_err(to_mcp)?;
    filters.limit = Some(sample_limit);
    let issues = storage.list_issues(&filters).map_err(to_mcp)?;
    Ok((total, issues))
}

// ---------------------------------------------------------------------------
// 1. project/info — static project metadata
// ---------------------------------------------------------------------------

pub struct ProjectInfoResource(Arc<BeadsState>);
impl ProjectInfoResource {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ResourceHandler for ProjectInfoResource {
    fn definition(&self) -> Resource {
        Resource {
            uri: "beads://project/info".into(),
            name: "Project Info".into(),
            description: Some(
                "Workspace metadata: beads directory, issue prefix, configuration. \
                 Read this first to understand the project context. \
                 Used by: project_overview tool returns similar data with more detail."
                    .into(),
            ),
            mime_type: Some("application/json".into()),
            icon: None,
            version: None,
            tags: vec![],
        }
    }

    fn read(&self, _ctx: &McpContext) -> McpResult<Vec<ResourceContent>> {
        let storage = self.0.open_storage().map_err(to_mcp)?;

        let config = storage.get_all_config().unwrap_or_default();
        let prefix = self.0.issue_prefix.as_deref().unwrap_or("br");

        let info = json!({
            "beads_dir": self.0.beads_dir.display().to_string(),
            "issue_prefix": prefix,
            "actor": self.0.actor,
            "config": config,
        });

        Ok(vec![ResourceContent {
            uri: "beads://project/info".into(),
            mime_type: Some("application/json".into()),
            text: Some(info.to_string()),
            blob: None,
        }])
    }
}

// ---------------------------------------------------------------------------
// 2. issues/{id} — individual issue resource template
// ---------------------------------------------------------------------------

pub struct IssueResource(Arc<BeadsState>);
impl IssueResource {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ResourceHandler for IssueResource {
    fn definition(&self) -> Resource {
        Resource {
            uri: "beads://issues/{id}".into(),
            name: "Issue Details".into(),
            description: Some(
                "Full issue details by ID. Discovery: use list_issues tool to find IDs. \
                 Used by: Complements show_issue tool which returns the same data."
                    .into(),
            ),
            mime_type: Some("application/json".into()),
            icon: None,
            version: None,
            tags: vec![],
        }
    }

    fn template(&self) -> Option<ResourceTemplate> {
        Some(ResourceTemplate {
            uri_template: "beads://issues/{id}".into(),
            name: "Issue Details".into(),
            description: Some("Full issue details by ID".into()),
            mime_type: Some("application/json".into()),
            icon: None,
            version: None,
            tags: vec![],
        })
    }

    fn read(&self, _ctx: &McpContext) -> McpResult<Vec<ResourceContent>> {
        Err(McpError::invalid_params(
            "Provide an issue ID via the URI template: beads://issues/{id}",
        ))
    }

    fn read_with_uri(
        &self,
        _ctx: &McpContext,
        uri: &str,
        params: &HashMap<String, String>,
    ) -> McpResult<Vec<ResourceContent>> {
        let id = params.get("id").ok_or_else(|| {
            McpError::invalid_params("'id' parameter is required in the URI template")
        })?;

        let storage = self.0.open_storage().map_err(to_mcp)?;

        let details = storage
            .get_issue_details(id, true, true, 20)
            .map_err(to_mcp)?
            .ok_or_else(|| issue_not_found_resource(&storage, id))?;

        let mut result = serde_json::to_value(&details.issue).unwrap_or_default();
        if let Some(obj) = result.as_object_mut() {
            obj.insert("labels".into(), json!(details.labels));
            obj.insert("comments".into(), json!(details.comments));
            obj.insert(
                "dependencies".into(),
                json!(details.dependencies.iter().map(|d| {
                    json!({"id": d.id, "title": d.title, "status": d.status, "dep_type": d.dep_type})
                }).collect::<Vec<_>>()),
            );
            obj.insert(
                "dependents".into(),
                json!(details.dependents.iter().map(|d| {
                    json!({"id": d.id, "title": d.title, "status": d.status, "dep_type": d.dep_type})
                }).collect::<Vec<_>>()),
            );
            if let Some(parent) = &details.parent {
                obj.insert("parent".into(), json!(parent));
            }
        }

        Ok(vec![ResourceContent {
            uri: uri.to_string(),
            mime_type: Some("application/json".into()),
            text: Some(result.to_string()),
            blob: None,
        }])
    }
}

// ---------------------------------------------------------------------------
// 3. schema — JSON schema reference
// ---------------------------------------------------------------------------

pub struct SchemaResource;

impl ResourceHandler for SchemaResource {
    fn definition(&self) -> Resource {
        Resource {
            uri: "beads://schema".into(),
            name: "Issue Schema Reference".into(),
            description: Some(
                "Reference for issue fields, valid statuses, priorities, types, \
                 and dependency types. Read this to understand what values are accepted."
                    .into(),
            ),
            mime_type: Some("application/json".into()),
            icon: None,
            version: None,
            tags: vec![],
        }
    }

    fn read(&self, _ctx: &McpContext) -> McpResult<Vec<ResourceContent>> {
        let schema = json!({
            "statuses": {
                "values": ["open", "in_progress", "blocked", "deferred", "draft", "closed", "pinned"],
                "aliases": {
                    "open": ["new", "todo"],
                    "in_progress": ["wip", "working", "active", "started", "in-progress", "inprogress"],
                    "blocked": ["stuck", "waiting"],
                    "deferred": ["later", "postponed", "backlogged"],
                    "closed": ["done", "completed", "resolved", "fixed", "wontfix", "cancelled"],
                    "pinned": ["sticky", "hold", "on_hold", "on-hold"]
                }
            },
            "priorities": {
                "values": ["critical", "high", "medium", "low", "backlog"],
                "aliases": {
                    "critical": ["p0", "urgent", "asap", "emergency"],
                    "high": ["p1", "important"],
                    "medium": ["p2", "normal", "default", "mid"],
                    "low": ["p3", "minor", "trivial", "nice_to_have", "nice-to-have"],
                    "backlog": ["p4", "someday", "eventually", "whenever"]
                }
            },
            "issue_types": {
                "values": ["task", "bug", "feature", "epic", "chore", "docs", "question"],
                "aliases": {
                    "task": ["issue"],
                    "bug": ["bugfix", "defect", "regression"],
                    "feature": ["feat", "enhancement", "story", "request"],
                    "chore": ["maintenance", "cleanup", "refactor", "tech_debt", "tech-debt"],
                    "docs": ["documentation", "doc"],
                    "question": ["q", "help"]
                }
            },
            "dependency_types": [
                "blocks", "related", "parent-child", "waits-for", "duplicates",
                "supersedes", "caused-by", "conditional-blocks", "discovered-from",
                "replies-to", "relates-to"
            ],
            "issue_fields": {
                "id": "string — unique ID (e.g. br-abc123)",
                "title": "string — 1-500 characters",
                "description": "string|null — detailed description",
                "status": "string — see statuses above",
                "priority": "object — {value: 0-4}",
                "issue_type": "string — see issue_types above",
                "assignee": "string|null",
                "owner": "string|null",
                "labels": "string[] — attached labels",
                "parent": "string|null — parent issue ID (via parent-child dependency; read-only in show_issue)",
                "created_at": "ISO 8601 timestamp",
                "updated_at": "ISO 8601 timestamp",
                "closed_at": "ISO 8601 timestamp|null",
                "close_reason": "string|null",
                "due_at": "ISO 8601 timestamp|null",
                "defer_until": "ISO 8601 timestamp|null",
                "estimated_minutes": "integer|null",
                "external_ref": "string|null — external tracker reference"
            },
            "bead_anatomy": {
                "purpose": "Recommended structure for issue descriptions to ensure self-containment and completeness",
                "sections": {
                    "background": "Why this issue exists — context and motivation",
                    "technical_approach": "How to implement — key design decisions and approach",
                    "success_criteria": "How to verify done — concrete, testable conditions",
                    "test_plan": "Unit and integration tests required — specific test cases",
                    "considerations": "Edge cases, risks, and things to watch out for"
                },
                "principles": [
                    "Self-contained: understandable without consulting external plans",
                    "Granular: one coherent piece of work per issue",
                    "Complete: preserve ALL complexity, do not oversimplify",
                    "Dependency-aware: make ALL blocking relationships explicit",
                    "Test-inclusive: every feature issue should have a companion test plan"
                ]
            }
        });

        Ok(vec![ResourceContent {
            uri: "beads://schema".into(),
            mime_type: Some("application/json".into()),
            text: Some(schema.to_string()),
            blob: None,
        }])
    }
}

// ---------------------------------------------------------------------------
// 4. labels — discovery resource for valid label values
// ---------------------------------------------------------------------------

pub struct LabelsResource(Arc<BeadsState>);
impl LabelsResource {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ResourceHandler for LabelsResource {
    fn definition(&self) -> Resource {
        Resource {
            uri: "beads://labels".into(),
            name: "Labels".into(),
            description: Some(
                "All labels in use with issue counts. Read this to discover valid \
                 label values before filtering with list_issues or tagging with update_issue. \
                 Used by: list_issues (labels filter), update_issue (labels_add/labels_remove), \
                 create_issue (labels param)."
                    .into(),
            ),
            mime_type: Some("application/json".into()),
            icon: None,
            version: None,
            tags: vec![],
        }
    }

    fn read(&self, _ctx: &McpContext) -> McpResult<Vec<ResourceContent>> {
        let storage = self.0.open_storage().map_err(to_mcp)?;
        let labels = storage.get_unique_labels_with_counts().map_err(to_mcp)?;

        let result = json!({
            "labels": labels.iter().map(|(name, count)| {
                json!({"name": name, "count": count})
            }).collect::<Vec<_>>(),
        });

        Ok(vec![ResourceContent {
            uri: "beads://labels".into(),
            mime_type: Some("application/json".into()),
            text: Some(result.to_string()),
            blob: None,
        }])
    }
}

// ---------------------------------------------------------------------------
// 5. issues/ready — actionable work items
// ---------------------------------------------------------------------------

pub struct ReadyIssuesResource(Arc<BeadsState>);
impl ReadyIssuesResource {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ResourceHandler for ReadyIssuesResource {
    fn definition(&self) -> Resource {
        Resource {
            uri: "beads://issues/ready".into(),
            name: "Ready Issues".into(),
            description: Some(
                "Issues ready for work: open, not blocked, not deferred. \
                 Quick view of actionable items sorted by priority. \
                 Used by: project_overview returns the same data. Use list_issues for \
                 filtered queries."
                    .into(),
            ),
            mime_type: Some("application/json".into()),
            icon: None,
            version: None,
            tags: vec![],
        }
    }

    fn read(&self, _ctx: &McpContext) -> McpResult<Vec<ResourceContent>> {
        let storage = self.0.open_storage().map_err(to_mcp)?;
        let ready = storage
            .get_ready_issues(&ReadyFilters::default(), ReadySortPolicy::Hybrid)
            .map_err(to_mcp)?;

        let result = json!({
            "count": ready.len(),
            "issues": ready.iter().map(|issue| {
                json!({
                    "id": issue.id,
                    "title": issue.title,
                    "priority": issue.priority,
                    "type": issue.issue_type,
                })
            }).collect::<Vec<_>>(),
        });

        Ok(vec![ResourceContent {
            uri: "beads://issues/ready".into(),
            mime_type: Some("application/json".into()),
            text: Some(result.to_string()),
            blob: None,
        }])
    }
}

// ---------------------------------------------------------------------------
// 6. issues/blocked — blocked work items
// ---------------------------------------------------------------------------

pub struct BlockedIssuesResource(Arc<BeadsState>);
impl BlockedIssuesResource {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ResourceHandler for BlockedIssuesResource {
    fn definition(&self) -> Resource {
        Resource {
            uri: "beads://issues/blocked".into(),
            name: "Blocked Issues".into(),
            description: Some(
                "Issues that are blocked by other issues. Shows what's stuck and \
                 which issues are blocking progress. \
                 Used by: manage_dependencies can unblock issues. Use show_issue on \
                 blockers to investigate."
                    .into(),
            ),
            mime_type: Some("application/json".into()),
            icon: None,
            version: None,
            tags: vec![],
        }
    }

    fn read(&self, _ctx: &McpContext) -> McpResult<Vec<ResourceContent>> {
        let storage = self.0.open_storage().map_err(to_mcp)?;
        let blocked = storage.get_blocked_issues().map_err(to_mcp)?;

        let result = json!({
            "count": blocked.len(),
            "issues": blocked.iter().map(|(issue, blockers)| {
                json!({
                    "id": issue.id,
                    "title": issue.title,
                    "blocked_by": blockers,
                })
            }).collect::<Vec<_>>(),
        });

        Ok(vec![ResourceContent {
            uri: "beads://issues/blocked".into(),
            mime_type: Some("application/json".into()),
            text: Some(result.to_string()),
            blob: None,
        }])
    }
}

// ---------------------------------------------------------------------------
// 7. issues/in_progress — work currently being done
// ---------------------------------------------------------------------------

pub struct InProgressResource(Arc<BeadsState>);
impl InProgressResource {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ResourceHandler for InProgressResource {
    fn definition(&self) -> Resource {
        Resource {
            uri: "beads://issues/in_progress".into(),
            name: "In-Progress Issues".into(),
            description: Some(format!(
                "Issues currently being worked on (status: in_progress). \
                 Shows who is working on what with priorities, with an exact total \
                 count and up to {IN_PROGRESS_RESOURCE_LIMIT} sample issues. \
                 Used by: update_issue to change assignee/status. Use show_issue for \
                 full details."
            )),
            mime_type: Some("application/json".into()),
            icon: None,
            version: None,
            tags: vec![],
        }
    }

    fn read(&self, _ctx: &McpContext) -> McpResult<Vec<ResourceContent>> {
        let storage = self.0.open_storage().map_err(to_mcp)?;
        let filters = ListFilters {
            statuses: Some(vec![Status::InProgress]),
            include_closed: false,
            ..ListFilters::default()
        };
        let (count, issues) =
            count_and_sample_issues(&storage, filters, IN_PROGRESS_RESOURCE_LIMIT)?;

        let result = json!({
            "count": count,
            "issues": issues.iter().map(|issue| {
                json!({
                    "id": issue.id,
                    "title": issue.title,
                    "priority": issue.priority,
                    "type": issue.issue_type,
                    "assignee": issue.assignee,
                })
            }).collect::<Vec<_>>(),
        });

        Ok(vec![ResourceContent {
            uri: "beads://issues/in_progress".into(),
            mime_type: Some("application/json".into()),
            text: Some(result.to_string()),
            blob: None,
        }])
    }
}

// ---------------------------------------------------------------------------
// 8. events/recent — recent audit events
// ---------------------------------------------------------------------------

pub struct EventsResource(Arc<BeadsState>);
impl EventsResource {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ResourceHandler for EventsResource {
    fn definition(&self) -> Resource {
        Resource {
            uri: "beads://events/recent".into(),
            name: "Recent Activity".into(),
            description: Some(
                "Recent audit events across all issues: status changes, field updates, \
                 comments added. Shows the 50 most recent events. \
                 Used by: Helpful for understanding what changed recently. \
                 Use show_issue for events on a specific issue."
                    .into(),
            ),
            mime_type: Some("application/json".into()),
            icon: None,
            version: None,
            tags: vec![],
        }
    }

    fn read(&self, _ctx: &McpContext) -> McpResult<Vec<ResourceContent>> {
        let storage = self.0.open_storage().map_err(to_mcp)?;
        let events = storage.get_all_events(50).map_err(to_mcp)?;

        let result = json!({
            "count": events.len(),
            "events": events.iter().map(|e: &Event| {
                json!({
                    "issue_id": e.issue_id,
                    "event_type": e.event_type,
                    "actor": e.actor,
                    "old_value": e.old_value,
                    "new_value": e.new_value,
                    "created_at": e.created_at,
                })
            }).collect::<Vec<_>>(),
        });

        Ok(vec![ResourceContent {
            uri: "beads://events/recent".into(),
            mime_type: Some("application/json".into()),
            text: Some(result.to_string()),
            blob: None,
        }])
    }
}

// ---------------------------------------------------------------------------
// 9. issues/deferred — deferred work items
// ---------------------------------------------------------------------------

pub struct DeferredIssuesResource(Arc<BeadsState>);
impl DeferredIssuesResource {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ResourceHandler for DeferredIssuesResource {
    fn definition(&self) -> Resource {
        Resource {
            uri: "beads://issues/deferred".into(),
            name: "Deferred Issues".into(),
            description: Some(format!(
                "Issues that have been deferred (status: deferred). Useful for triage — \
                 review what has been postponed and whether it should be revisited, \
                 with an exact total count and up to {DEFERRED_RESOURCE_LIMIT} sample issues. \
                 Used by: update_issue to change status. Use show_issue for full details."
            )),
            mime_type: Some("application/json".into()),
            icon: None,
            version: None,
            tags: vec![],
        }
    }

    fn read(&self, _ctx: &McpContext) -> McpResult<Vec<ResourceContent>> {
        let storage = self.0.open_storage().map_err(to_mcp)?;
        let filters = ListFilters {
            statuses: Some(vec![Status::Deferred]),
            include_deferred: true,
            ..ListFilters::default()
        };
        let (count, issues) = count_and_sample_issues(&storage, filters, DEFERRED_RESOURCE_LIMIT)?;

        let result = json!({
            "count": count,
            "issues": issues.iter().map(|issue| {
                json!({
                    "id": issue.id,
                    "title": issue.title,
                    "priority": issue.priority,
                    "type": issue.issue_type,
                    "defer_until": issue.defer_until,
                })
            }).collect::<Vec<_>>(),
        });

        Ok(vec![ResourceContent {
            uri: "beads://issues/deferred".into(),
            mime_type: Some("application/json".into()),
            text: Some(result.to_string()),
            blob: None,
        }])
    }
}

// ---------------------------------------------------------------------------
// 10. graph/health — dependency graph health metrics (bv-inspired)
// ---------------------------------------------------------------------------

/// Compute the longest path length in the "blocks" DAG from a given node.
/// Uses a `visiting` set to detect cycles and avoid infinite recursion.
fn longest_chain_from(
    node: &str,
    edges: &HashMap<String, Vec<String>>,
    cache: &mut HashMap<String, usize>,
    visiting: &mut std::collections::HashSet<String>,
) -> usize {
    if let Some(&cached) = cache.get(node) {
        return cached;
    }
    // Cycle detection: if we're already visiting this node, stop.
    if !visiting.insert(node.to_string()) {
        return 0;
    }
    let depth = edges.get(node).map_or(0, |children| {
        children
            .iter()
            .map(|c| 1 + longest_chain_from(c, edges, cache, visiting))
            .max()
            .unwrap_or(0)
    });
    visiting.remove(node);
    cache.insert(node.to_string(), depth);
    depth
}

fn graph_has_cycle(
    node: &str,
    edges: &HashMap<String, Vec<String>>,
    visiting: &mut std::collections::HashSet<String>,
    visited: &mut std::collections::HashSet<String>,
) -> bool {
    if visited.contains(node) {
        return false;
    }

    if !visiting.insert(node.to_string()) {
        return true;
    }

    let detected = edges.get(node).is_some_and(|children| {
        children
            .iter()
            .any(|child| graph_has_cycle(child, edges, visiting, visited))
    });

    visiting.remove(node);
    visited.insert(node.to_string());
    detected
}

/// Compute graph health metrics from the dependency edges.
fn compute_graph_health(storage: &SqliteStorage) -> McpResult<serde_json::Value> {
    let all_edges = storage.get_blocks_dep_edges().map_err(to_mcp)?;
    let open_filters = ListFilters {
        include_closed: false,
        ..ListFilters::default()
    };
    let open_issues = storage.list_issues(&open_filters).map_err(to_mcp)?;
    let open_ids: std::collections::HashSet<&str> =
        open_issues.iter().map(|i| i.id.as_str()).collect();

    // Filter edges to only open→open relationships
    let open_edges: Vec<(String, String)> = all_edges
        .into_iter()
        .filter(|(from, to)| open_ids.contains(from.as_str()) && open_ids.contains(to.as_str()))
        .collect();

    let edge_count = open_edges.len();
    let node_count = open_ids.len();
    let density = if node_count > 1 {
        #[allow(clippy::cast_precision_loss)]
        let d = edge_count as f64 / (node_count as f64 * (node_count as f64 - 1.0));
        (d * 1000.0).round() / 1000.0
    } else {
        0.0
    };

    // Build adjacency list for chain depth computation
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for (from, to) in &open_edges {
        adj.entry(from.clone()).or_default().push(to.clone());
    }

    // Longest chain depth
    let mut depth_cache: HashMap<String, usize> = HashMap::new();
    let mut visiting: std::collections::HashSet<String> = std::collections::HashSet::new();
    let max_chain_depth = open_ids
        .iter()
        .map(|id| longest_chain_from(id, &adj, &mut depth_cache, &mut visiting))
        .max()
        .unwrap_or(0);

    // High-fan-out issues (block 3+ others)
    let high_fan_out: Vec<_> = adj
        .iter()
        .filter(|(_, targets)| targets.len() >= HIGH_FAN_OUT_THRESHOLD)
        .map(|(id, targets)| json!({"id": id, "blocks_count": targets.len()}))
        .collect();

    // Stale issues (not updated in 30+ days)
    let thirty_days_ago = chrono::Utc::now() - chrono::Duration::days(STALE_THRESHOLD_DAYS);
    let stale_filters = ListFilters {
        include_closed: false,
        updated_before: Some(thirty_days_ago),
        ..ListFilters::default()
    };
    let stale_issues = storage.list_issues(&stale_filters).map_err(to_mcp)?;

    let mut cycle_visiting = std::collections::HashSet::new();
    let mut cycle_visited = std::collections::HashSet::new();
    let has_cycles = open_ids
        .iter()
        .any(|id| graph_has_cycle(id, &adj, &mut cycle_visiting, &mut cycle_visited));

    Ok(json!({
        "open_issue_count": node_count,
        "dependency_edge_count": edge_count,
        "density": density,
        "density_interpretation": if density > DENSITY_VERY_HIGH {
            "Very high — issues are heavily coupled, hard to parallelize"
        } else if density > DENSITY_MODERATE {
            "Moderate — some coupling, review if all deps are necessary"
        } else if density > 0.0 {
            "Healthy — dependencies are focused"
        } else {
            "No dependencies — issues are fully independent"
        },
        "max_chain_depth": max_chain_depth,
        "max_chain_interpretation": if max_chain_depth > CHAIN_DEPTH_DEEP {
            "Deep chain — critical path is long, hard to parallelize"
        } else if max_chain_depth > CHAIN_DEPTH_MODERATE {
            "Moderate chain depth"
        } else {
            "Shallow — good parallelization potential"
        },
        "high_fan_out_issues": high_fan_out,
        "cycle_detected": has_cycles,
        "stale_issue_count": stale_issues.len(),
        "stale_threshold_days": STALE_THRESHOLD_DAYS,
    }))
}

pub struct GraphHealthResource(Arc<BeadsState>);
impl GraphHealthResource {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ResourceHandler for GraphHealthResource {
    fn definition(&self) -> Resource {
        Resource {
            uri: "beads://graph/health".into(),
            name: "Dependency Graph Health".into(),
            description: Some(
                "Graph-level health metrics for the dependency network: density, \
                 chain depth, fan-out hotspots, stale issues, cycle detection. \
                 Inspired by bv's graph analysis. Read this to understand project \
                 structure and identify bottlenecks. \
                 Used by: plan_next_work and triage prompts for context."
                    .into(),
            ),
            mime_type: Some("application/json".into()),
            icon: None,
            version: None,
            tags: vec![],
        }
    }

    fn read(&self, _ctx: &McpContext) -> McpResult<Vec<ResourceContent>> {
        let storage = self.0.open_storage().map_err(to_mcp)?;
        let health = compute_graph_health(&storage)?;

        Ok(vec![ResourceContent {
            uri: "beads://graph/health".into(),
            mime_type: Some("application/json".into()),
            text: Some(health.to_string()),
            blob: None,
        }])
    }
}

// ---------------------------------------------------------------------------
// 11. issues/bottlenecks — highest-impact blockers (bv-inspired)
// ---------------------------------------------------------------------------

/// Compute bottleneck issues: those that block the most other open issues.
/// This is a practical approximation of PageRank/betweenness from bv.
fn compute_bottlenecks(storage: &SqliteStorage) -> McpResult<serde_json::Value> {
    let edges = storage.get_blocks_dep_edges().map_err(to_mcp)?;
    let open_filters = ListFilters {
        include_closed: false,
        ..ListFilters::default()
    };
    let open_issues = storage.list_issues(&open_filters).map_err(to_mcp)?;
    let open_map: HashMap<&str, &Issue> = open_issues.iter().map(|i| (i.id.as_str(), i)).collect();

    // Count how many open issues each open issue blocks
    let mut blocks_count: HashMap<&str, usize> = HashMap::new();
    for (blocker, blocked) in &edges {
        if open_map.contains_key(blocker.as_str()) && open_map.contains_key(blocked.as_str()) {
            *blocks_count.entry(blocker.as_str()).or_default() += 1;
        }
    }

    // Sort by blocks_count descending
    let mut ranked: Vec<_> = blocks_count.into_iter().collect();
    ranked.sort_by_key(|b| std::cmp::Reverse(b.1));

    let bottlenecks: Vec<_> = ranked
        .iter()
        .take(BOTTLENECK_ISSUES_DISPLAY_LIMIT)
        .filter_map(|(id, count)| {
            open_map.get(id).map(|issue| {
                json!({
                    "id": issue.id,
                    "title": issue.title,
                    "priority": issue.priority,
                    "status": issue.status,
                    "blocks_count": count,
                    "interpretation": if *count >= 5 {
                        "Critical bottleneck — blocks many issues, prioritize resolving"
                    } else if *count >= 3 {
                        "Significant blocker — resolve to unblock multiple work streams"
                    } else {
                        "Blocker — has downstream impact"
                    }
                })
            })
        })
        .collect();

    Ok(json!({
        "count": bottlenecks.len(),
        "issues": bottlenecks,
        "analysis_hint": "Issues sorted by how many other open issues they block. \
            High blocks_count = high PageRank equivalent. Resolve these first to \
            maximize unblocked work.",
    }))
}

pub struct BottlenecksResource(Arc<BeadsState>);
impl BottlenecksResource {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ResourceHandler for BottlenecksResource {
    fn definition(&self) -> Resource {
        Resource {
            uri: "beads://issues/bottlenecks".into(),
            name: "Bottleneck Issues".into(),
            description: Some(
                "Issues that block the most other work, sorted by impact. \
                 Equivalent to bv's PageRank-based prioritization. Resolve these \
                 first to maximize throughput. \
                 Used by: plan_next_work prompt uses this data for recommendations."
                    .into(),
            ),
            mime_type: Some("application/json".into()),
            icon: None,
            version: None,
            tags: vec![],
        }
    }

    fn read(&self, _ctx: &McpContext) -> McpResult<Vec<ResourceContent>> {
        let storage = self.0.open_storage().map_err(to_mcp)?;
        let bottlenecks = compute_bottlenecks(&storage)?;

        Ok(vec![ResourceContent {
            uri: "beads://issues/bottlenecks".into(),
            mime_type: Some("application/json".into()),
            text: Some(bottlenecks.to_string()),
            blob: None,
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Dependency, DependencyType, IssueType, Priority};
    use chrono::{Duration, Utc};

    fn make_issue(id: &str, updated_at: chrono::DateTime<Utc>) -> Issue {
        let created_at = updated_at - Duration::minutes(5);
        Issue {
            id: id.to_string(),
            content_hash: None,
            title: format!("Issue {id}"),
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
            created_at,
            created_by: None,
            updated_at,
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

    fn make_dependency(issue_id: &str, depends_on_id: &str) -> Dependency {
        Dependency {
            issue_id: issue_id.to_string(),
            depends_on_id: depends_on_id.to_string(),
            dep_type: DependencyType::Blocks,
            created_at: Utc::now(),
            created_by: Some("tester".to_string()),
            metadata: None,
            thread_id: None,
        }
    }

    #[test]
    fn compute_graph_health_detects_non_mutual_cycles() {
        let mut storage = SqliteStorage::open_memory().unwrap();
        let now = Utc::now();

        for id in ["bd-a", "bd-b", "bd-c"] {
            storage
                .create_issue(&make_issue(id, now), "tester")
                .unwrap();
        }

        storage
            .sync_dependencies_for_import("bd-a", &[make_dependency("bd-a", "bd-b")])
            .unwrap();
        storage
            .sync_dependencies_for_import("bd-b", &[make_dependency("bd-b", "bd-c")])
            .unwrap();
        storage
            .sync_dependencies_for_import("bd-c", &[make_dependency("bd-c", "bd-a")])
            .unwrap();

        let health = compute_graph_health(&storage).unwrap();
        assert_eq!(health["cycle_detected"], json!(true));
    }

    #[test]
    fn compute_graph_health_counts_all_stale_open_issues() {
        let mut storage = SqliteStorage::open_memory().unwrap();
        let stale_time = Utc::now() - Duration::days(31);

        for index in 0..101 {
            let issue = make_issue(&format!("bd-stale-{index:03}"), stale_time);
            storage.create_issue(&issue, "tester").unwrap();
        }

        let health = compute_graph_health(&storage).unwrap();
        assert_eq!(health["stale_issue_count"], json!(101));
        assert_eq!(health["open_issue_count"], json!(101));
    }

    #[test]
    fn resource_sampling_preserves_exact_in_progress_count() {
        let mut storage = SqliteStorage::open_memory().unwrap();
        let now = Utc::now();

        for index in 0..51 {
            storage
                .create_issue(
                    &Issue {
                        status: Status::InProgress,
                        ..make_issue(&format!("bd-ip-{index:02}"), now)
                    },
                    "tester",
                )
                .unwrap();
        }

        let (count, issues) = count_and_sample_issues(
            &storage,
            ListFilters {
                statuses: Some(vec![Status::InProgress]),
                include_closed: false,
                ..ListFilters::default()
            },
            IN_PROGRESS_RESOURCE_LIMIT,
        )
        .unwrap();

        assert_eq!(count, 51);
        assert_eq!(issues.len(), IN_PROGRESS_RESOURCE_LIMIT);
    }
}
