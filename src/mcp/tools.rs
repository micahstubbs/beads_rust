//! MCP tool handlers for the beads issue tracker.
//!
//! Seven tools — one per high-frequency agent workflow — following the
//! "≤ 7 tools per cluster" principle from the MCP design guide.
//!
//! Design philosophy: **Forgive by Default**.  Status/priority/type inputs
//! are auto-corrected ("wip" → `in_progress`, "urgent" → `critical`).
//! Placeholder IDs are detected and rejected with discovery hints.
//! Errors include `suggested_tool_calls` so agents know what to do next.

use std::sync::Arc;

use fastmcp_rust::{
    Content, McpContext, McpError, McpErrorCode, McpResult, Tool, ToolAnnotations, ToolHandler,
};
use serde_json::json;

use crate::error::{ErrorCode, StructuredError};
use crate::model::{Issue, IssueType, Priority, Status};
use crate::storage::{IssueUpdate, ListFilters, ReadyFilters, ReadySortPolicy, SqliteStorage};

use super::BeadsState;

// ---------------------------------------------------------------------------
// Constants — pre-computed sets for O(1) placeholder detection
// ---------------------------------------------------------------------------

/// Field keys in `update_issue` that map to `IssueUpdate` struct fields.
/// Used to distinguish field updates from label/comment side-effects.
const UPDATE_FIELD_KEYS: &[&str] = &[
    "title",
    "description",
    "status",
    "priority",
    "type",
    "assignee",
    "owner",
    "due_at",
    "defer_until",
    "estimated_minutes",
    "external_ref",
];

/// Known placeholder strings agents hallucinate instead of real IDs.
const PLACEHOLDER_EXACT: &[&str] = &[
    "your_id",
    "your-id",
    "yourid",
    "your_issue_id",
    "issue_id",
    "issue-id",
    "issueid",
    "example",
    "example_id",
    "example-id",
    "test",
    "test_id",
    "test-id",
    "foo",
    "bar",
    "baz",
    "qux",
    "xxx",
    "yyy",
    "zzz",
    "placeholder",
    "replace_me",
    "replace-me",
    "todo",
    "fixme",
    "tbd",
    "id",
    "issue",
    "some_id",
    "some-id",
    "abc",
    "abc123",
    "id_here",
    "insert_id",
    "none",
    "null",
    "undefined",
    "n/a",
];

// ---------------------------------------------------------------------------
// Placeholder detection
// ---------------------------------------------------------------------------

/// Detect if a string looks like a placeholder rather than a real issue ID.
/// Returns a structured error with discovery hints if detected.
fn detect_placeholder(s: &str) -> Option<McpError> {
    let lower = s.to_lowercase();

    // Exact match against known placeholders
    if PLACEHOLDER_EXACT.contains(&lower.as_str()) {
        return Some(placeholder_error(s));
    }

    // Pattern-based detection
    if lower.starts_with('$')
        || lower.starts_with('{')
        || lower.starts_with('<')
        || lower.starts_with('[')
    {
        return Some(placeholder_error(s));
    }

    // Substring detection — only match patterns specific to hallucinated IDs.
    // Broader terms like "replace" and "example" are already covered by exact
    // matches; substring matching them would reject legitimate IDs whose
    // prefix or hash portion happens to contain those words.
    if lower.contains("your_") || lower.contains("placeholder") {
        return Some(placeholder_error(s));
    }

    None
}

fn placeholder_error(s: &str) -> McpError {
    McpError::with_data(
        McpErrorCode::InvalidParams,
        format!("'{s}' looks like a placeholder, not a real issue ID"),
        json!({
            "error_type": "PLACEHOLDER_DETECTED",
            "provided": s,
            "recoverable": true,
            "hint": "Use list_issues to discover real issue IDs, or project_overview for a summary",
            "suggested_tool_calls": [
                {"tool": "list_issues", "arguments": {}},
                {"tool": "project_overview", "arguments": {}}
            ]
        }),
    )
}

/// Check for placeholder AND validate ID exists. Returns fuzzy suggestions
/// with `suggested_tool_calls` if not found.
fn require_valid_issue(storage: &SqliteStorage, id: &str) -> McpResult<()> {
    // Check DB first: if the ID exists, it's real regardless of name.
    if storage.id_exists(id).map_err(beads_to_mcp)? {
        return Ok(());
    }
    // ID not found — give a more helpful placeholder error if the ID looks
    // hallucinated, otherwise give a standard not-found error.
    if let Some(err) = detect_placeholder(id) {
        return Err(err);
    }
    Err(issue_not_found_err(storage, id))
}

// ---------------------------------------------------------------------------
// Structured error builders
// ---------------------------------------------------------------------------

/// Convert a `BeadsError` into a structured `McpError` with machine-readable
/// data payload, recovery hints, and fuzzy-match suggestions.
fn beads_to_mcp(err: impl Into<crate::BeadsError>) -> McpError {
    let beads_err = err.into();
    let structured = StructuredError::from_error(&beads_err);

    let mcp_code = match structured.code {
        // Validation errors → invalid params
        ErrorCode::ValidationFailed
        | ErrorCode::InvalidStatus
        | ErrorCode::InvalidType
        | ErrorCode::InvalidPriority
        | ErrorCode::RequiredField
        | ErrorCode::InvalidId => McpErrorCode::InvalidParams,
        // Issue / dependency / operational errors → tool execution error
        ErrorCode::IssueNotFound
        | ErrorCode::AmbiguousId
        | ErrorCode::NothingToDo
        | ErrorCode::CycleDetected
        | ErrorCode::DependencyNotFound
        | ErrorCode::HasDependents
        | ErrorCode::SelfDependency
        | ErrorCode::DuplicateDependency
        | ErrorCode::IdCollision => McpErrorCode::ToolExecutionError,
        // Database / config / IO → internal
        _ => McpErrorCode::InternalError,
    };

    let mut data = json!({
        "error_type": structured.code.as_str(),
        "recoverable": structured.retryable,
        "message": structured.message,
    });

    if let Some(hint) = &structured.hint {
        data["hint"] = json!(hint);
    }
    if let Some(ctx) = &structured.context
        && let Some(similar) = ctx.get("similar_ids")
    {
        data["suggestions"] = similar.clone();
    }

    // Add contextual suggested_tool_calls based on error type
    match structured.code {
        ErrorCode::IssueNotFound | ErrorCode::AmbiguousId => {
            data["suggested_tool_calls"] = json!([{"tool": "list_issues", "arguments": {}}]);
        }
        ErrorCode::CycleDetected => {
            // Suggest list_issues so the agent can discover IDs and
            // inspect the dependency graph.  We can't suggest tools
            // that require an `id` param because we don't know which
            // ID is relevant from the error alone.
            data["suggested_tool_calls"] = json!([
                {"tool": "list_issues", "arguments": {}}
            ]);
        }
        ErrorCode::InvalidStatus => {
            data["available_options"] = json!([
                "open",
                "in_progress",
                "blocked",
                "deferred",
                "draft",
                "closed",
                "pinned"
            ]);
            data["fix_hint"] = json!("Aliases also accepted: wip, todo, done, stuck, later, hold");
        }
        ErrorCode::InvalidPriority => {
            data["available_options"] = json!(["critical", "high", "medium", "low", "backlog"]);
            data["fix_hint"] =
                json!("Aliases also accepted: urgent, important, normal, minor, someday");
        }
        _ => {
            data["discovery_hint"] =
                json!("Use list_issues tool or beads://labels resource to find valid values");
        }
    }

    McpError::with_data(mcp_code, structured.message, data)
}

/// Build a structured "issue not found" `McpError` with fuzzy ID suggestions
/// and `suggested_tool_calls` pointing to the best next action.
fn issue_not_found_err(storage: &SqliteStorage, id: &str) -> McpError {
    let all_ids = storage.get_all_ids().unwrap_or_default();
    let structured = StructuredError::issue_not_found(id, &all_ids);

    let mut data = json!({
        "error_type": "ISSUE_NOT_FOUND",
        "recoverable": true,
        "message": structured.message,
        "discovery_hint": "Use list_issues to find valid issue IDs",
    });

    if let Some(hint) = &structured.hint {
        data["hint"] = json!(hint);
    }

    // Build suggested_tool_calls based on whether we have fuzzy matches
    let mut suggested_calls = Vec::new();
    if let Some(ctx) = &structured.context
        && let Some(similar) = ctx.get("similar_ids")
    {
        data["suggestions"] = similar.clone();
        // If exactly one match, suggest show_issue directly
        if let Some(arr) = similar.as_array()
            && arr.len() == 1
            && let Some(suggested_id) = arr[0].as_str()
        {
            suggested_calls.push(json!({
                "tool": "show_issue",
                "arguments": {"id": suggested_id}
            }));
        }
    }
    suggested_calls.push(json!({"tool": "list_issues", "arguments": {}}));
    data["suggested_tool_calls"] = json!(suggested_calls);

    McpError::with_data(McpErrorCode::ToolExecutionError, structured.message, data)
}

fn open(state: &BeadsState) -> McpResult<SqliteStorage> {
    state.open_storage().map_err(beads_to_mcp)
}

// ---------------------------------------------------------------------------
// Input coercion helpers — Forgive by Default
// ---------------------------------------------------------------------------

/// Parse a status string with coercion. Returns the status and an optional
/// warning if the input was auto-corrected (e.g. "wip" → "in_progress").
fn parse_status(s: &str) -> McpResult<(Status, Option<String>)> {
    let lower = s.to_lowercase();
    let (status, canonical) = match lower.as_str() {
        "open" | "new" | "todo" => (Status::Open, "open"),
        "in_progress" | "in-progress" | "inprogress" | "wip" | "working" | "active" | "started" => {
            (Status::InProgress, "in_progress")
        }
        "blocked" | "stuck" | "waiting" => (Status::Blocked, "blocked"),
        "deferred" | "later" | "postponed" | "backlogged" => (Status::Deferred, "deferred"),
        "draft" => (Status::Draft, "draft"),
        "closed" | "done" | "completed" | "resolved" | "fixed" | "wontfix" | "cancelled" => {
            (Status::Closed, "closed")
        }
        "pinned" | "sticky" | "hold" | "on_hold" | "on-hold" => (Status::Pinned, "pinned"),
        _ => {
            return Err(McpError::with_data(
                McpErrorCode::InvalidParams,
                format!("Unknown status '{s}'"),
                json!({
                    "error_type": "INVALID_STATUS",
                    "provided": s,
                    "recoverable": true,
                    "available_options": ["open", "in_progress", "blocked", "deferred", "draft", "closed", "pinned"],
                    "aliases": {
                        "open": ["new", "todo"],
                        "in_progress": ["wip", "working", "active", "started"],
                        "blocked": ["stuck", "waiting"],
                        "deferred": ["later", "postponed"],
                        "closed": ["done", "completed", "resolved", "fixed"],
                        "pinned": ["hold", "sticky", "on_hold"]
                    },
                    "fix_hint": "Use one of the available options or their aliases"
                }),
            ));
        }
    };

    let warning = (lower != canonical).then(|| format!("'{s}' interpreted as '{canonical}'"));

    Ok((status, warning))
}

/// Parse a priority string with coercion.
fn parse_priority(s: &str) -> McpResult<(Priority, Option<String>)> {
    let lower = s.to_lowercase();
    let (priority, canonical) = match lower.as_str() {
        "critical" | "p0" | "0" | "urgent" | "asap" | "emergency" => {
            (Priority::CRITICAL, "critical")
        }
        "high" | "p1" | "1" | "important" => (Priority::HIGH, "high"),
        "medium" | "p2" | "2" | "normal" | "default" | "mid" => (Priority::MEDIUM, "medium"),
        "low" | "p3" | "3" | "minor" | "trivial" | "nice_to_have" | "nice-to-have" => {
            (Priority::LOW, "low")
        }
        "backlog" | "p4" | "4" | "someday" | "eventually" | "whenever" => {
            (Priority::BACKLOG, "backlog")
        }
        _ => {
            return Err(McpError::with_data(
                McpErrorCode::InvalidParams,
                format!("Unknown priority '{s}'"),
                json!({
                    "error_type": "INVALID_PRIORITY",
                    "provided": s,
                    "recoverable": true,
                    "available_options": ["critical", "high", "medium", "low", "backlog"],
                    "aliases": {
                        "critical": ["p0", "urgent", "asap", "emergency"],
                        "high": ["p1", "important"],
                        "medium": ["p2", "normal", "default"],
                        "low": ["p3", "minor", "trivial"],
                        "backlog": ["p4", "someday", "eventually"]
                    },
                    "fix_hint": "Use one of the available options or their aliases"
                }),
            ));
        }
    };

    // Canonical names and numeric aliases don't need a coercion warning
    let warning = (lower != canonical && !lower.starts_with('p') && lower.parse::<u8>().is_err())
        .then(|| format!("'{s}' interpreted as '{canonical}'"));

    Ok((priority, warning))
}

/// Parse an issue type string with coercion.
fn parse_issue_type(s: &str) -> (IssueType, Option<String>) {
    let lower = s.to_lowercase();
    let (issue_type, canonical) = match lower.as_str() {
        "task" | "issue" => (IssueType::Task, "task"),
        "bug" | "bugfix" | "defect" | "regression" => (IssueType::Bug, "bug"),
        "feature" | "feat" | "enhancement" | "story" | "request" => (IssueType::Feature, "feature"),
        "epic" => (IssueType::Epic, "epic"),
        "chore" | "maintenance" | "cleanup" | "refactor" | "tech_debt" | "tech-debt" => {
            (IssueType::Chore, "chore")
        }
        "docs" | "documentation" | "doc" => (IssueType::Docs, "docs"),
        "question" | "q" | "help" => (IssueType::Question, "question"),
        other => return (IssueType::Custom(other.to_string()), None),
    };

    let warning = (lower != canonical).then(|| format!("'{s}' interpreted as '{canonical}'"));

    (issue_type, warning)
}

/// Validate and coerce a dependency type string. Dependency types use
/// kebab-case internally ("parent-child", "waits-for") but agents often
/// pass underscores or abbreviated forms.
fn parse_dep_type(s: &str) -> McpResult<(String, Option<String>)> {
    let lower = s.to_lowercase();
    let (canonical, alias) = match lower.as_str() {
        "blocks" | "block" | "blocking" => ("blocks", true),
        "related" => ("related", false),
        "parent-child" | "parent_child" | "parentchild" | "parent" | "child" => {
            ("parent-child", true)
        }
        "waits-for" | "waits_for" | "waitfor" | "waitsfor" | "waiting" => ("waits-for", true),
        "duplicates" | "duplicate" | "dupe" | "dup" => ("duplicates", true),
        "supersedes" | "supersede" | "replaces" => ("supersedes", true),
        "caused-by" | "caused_by" | "causedby" | "root_cause" | "root-cause" => ("caused-by", true),
        "conditional-blocks" | "conditional_blocks" | "conditionalblocks" => {
            ("conditional-blocks", true)
        }
        "discovered-from" | "discovered_from" | "discoveredfrom" => ("discovered-from", true),
        "replies-to" | "replies_to" | "repliesto" | "reply" => ("replies-to", true),
        "relates-to" | "relates_to" | "relatesto" => ("relates-to", true),
        _ => {
            return Err(McpError::with_data(
                McpErrorCode::InvalidParams,
                format!("Unknown dependency type '{s}'"),
                json!({
                    "error_type": "INVALID_DEP_TYPE",
                    "provided": s,
                    "recoverable": true,
                    "available_options": [
                        "blocks", "related", "parent-child", "waits-for",
                        "duplicates", "supersedes", "caused-by",
                        "conditional-blocks", "discovered-from", "replies-to", "relates-to"
                    ],
                    "fix_hint": "Dependency types use kebab-case (e.g., 'parent-child', not 'parent_child')"
                }),
            ));
        }
    };

    let warning =
        (alias && lower != canonical).then(|| format!("'{s}' interpreted as '{canonical}'"));

    Ok((canonical.to_string(), warning))
}

/// Parse an ISO 8601 timestamp with coercion (Z → +00:00, slashes → dashes).
fn parse_timestamp(s: &str) -> McpResult<(chrono::DateTime<chrono::Utc>, Option<String>)> {
    let mut normalized = s.trim().to_string();
    let mut coerced = false;

    // Slashes → dashes
    if normalized.contains('/') {
        normalized = normalized.replace('/', "-");
        coerced = true;
    }

    // Try parsing as-is first (handles Z suffix, full ISO 8601)
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&normalized) {
        let warning = coerced.then(|| format!("'{s}' normalized to '{normalized}'"));
        return Ok((dt.with_timezone(&chrono::Utc), warning));
    }

    // Try with appended Z for bare timestamps (no timezone indicator).
    // We check for timezone patterns near the end, not substring matches,
    // because "-0" appears in date portions (e.g. months 01-09).
    let has_tz_suffix = normalized.contains('Z')
        || normalized.contains('+')
        || normalized.ends_with('z')
        || normalized
            .rfind('-')
            .is_some_and(|pos| pos > 10 && normalized[pos..].contains(':'));
    if !has_tz_suffix {
        let with_tz = format!("{normalized}Z");
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&with_tz) {
            return Ok((
                dt.with_timezone(&chrono::Utc),
                Some(format!("'{s}' interpreted as UTC (missing timezone)")),
            ));
        }
    }

    // Try parsing date-only (YYYY-MM-DD → start of day UTC)
    if let Ok(date) = chrono::NaiveDate::parse_from_str(&normalized, "%Y-%m-%d") {
        let dt = date.and_hms_opt(0, 0, 0).unwrap().and_utc();
        return Ok((
            dt,
            Some(format!("'{s}' interpreted as '{}'", dt.to_rfc3339())),
        ));
    }

    Err(McpError::with_data(
        McpErrorCode::InvalidParams,
        format!("Cannot parse timestamp '{s}'"),
        json!({
            "error_type": "INVALID_TIMESTAMP",
            "provided": s,
            "recoverable": true,
            "expected_format": "ISO 8601 (e.g., '2026-03-15T10:00:00Z' or '2026-03-15')",
            "common_mistakes": [
                "Missing timezone — add Z or +00:00",
                "Using slashes — use dashes (2026-03-15 not 2026/03/15)",
                "Date-only works: '2026-03-15' → start of day UTC"
            ]
        }),
    ))
}

/// Sanitize a search query for FTS5 compatibility.
/// Returns `None` for bare wildcards (meaning "return all").
fn sanitize_search(query: &str) -> Option<String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Strip leading wildcards (FTS5 doesn't support *foo)
    let cleaned = trimmed.trim_start_matches('*');
    // Bare wildcard or dot → return all
    if cleaned.is_empty() || cleaned == "." {
        return None;
    }
    Some(cleaned.to_string())
}

/// Read a nullable string field from JSON args: present-string → `Some(Some(s))`,
/// present-null → `Some(None)`, absent → `None`.
/// Returns an error if the value is present but neither a string nor null.
#[allow(clippy::option_option)]
fn nullable_str(args: &serde_json::Value, key: &str) -> McpResult<Option<Option<String>>> {
    match args.get(key) {
        None => Ok(None),
        Some(v) if v.is_null() => Ok(Some(None)),
        Some(v) => v.as_str().map_or_else(
            || {
                Err(McpError::invalid_params(format!(
                    "'{key}' must be a string or null, got {v}"
                )))
            },
            |s| Ok(Some(Some(s.to_string()))),
        ),
    }
}

/// Parse update fields from JSON args into an `IssueUpdate` + coercion warnings.
/// Extracted to keep `UpdateIssueTool::call` under the line limit.
#[allow(clippy::too_many_lines)]
fn parse_update_fields(
    id: &str,
    args: &serde_json::Value,
) -> McpResult<(IssueUpdate, Vec<String>)> {
    let mut coercions: Vec<String> = Vec::new();
    let mut updates = IssueUpdate::default();

    if let Some(title) = args.get("title").and_then(|v| v.as_str()) {
        if title.is_empty() || title.len() > 500 {
            return Err(McpError::invalid_params("Title must be 1-500 characters"));
        }
        updates.title = Some(title.to_string());
    }
    updates.description = nullable_str(args, "description")?;
    if let Some(s) = args.get("status").and_then(|v| v.as_str()) {
        let (status, warning) = parse_status(s)?;

        // Intercept status→closed: redirect to close_issue
        if status == Status::Closed {
            return Err(McpError::with_data(
                McpErrorCode::ToolExecutionError,
                "To close an issue, use close_issue which properly records close metadata",
                json!({
                    "error_type": "USE_CLOSE_ISSUE",
                    "recoverable": true,
                    "hint": "close_issue records close_reason and closed_at timestamp for proper audit trail",
                    "suggested_tool_calls": [{
                        "tool": "close_issue",
                        "arguments": {"id": id}
                    }]
                }),
            ));
        }

        if let Some(w) = warning {
            coercions.push(w);
        }
        updates.status = Some(status);
    }
    if let Some(p) = args.get("priority").and_then(|v| v.as_str()) {
        let (priority, warning) = parse_priority(p)?;
        if let Some(w) = warning {
            coercions.push(w);
        }
        updates.priority = Some(priority);
    }
    if let Some(t) = args.get("type").and_then(|v| v.as_str()) {
        let (issue_type, warning) = parse_issue_type(t);
        if let Some(w) = warning {
            coercions.push(w);
        }
        updates.issue_type = Some(issue_type);
    }
    updates.assignee = nullable_str(args, "assignee")?;
    updates.owner = nullable_str(args, "owner")?;
    if let Some(v) = args.get("due_at") {
        if v.is_null() {
            updates.due_at = Some(None);
        } else if let Some(s) = v.as_str() {
            let (dt, warning) = parse_timestamp(s)?;
            if let Some(w) = warning {
                coercions.push(format!("due_at: {w}"));
            }
            updates.due_at = Some(Some(dt));
        } else {
            return Err(McpError::invalid_params(format!(
                "'due_at' must be an ISO 8601 string or null, got {v}"
            )));
        }
    }
    if let Some(v) = args.get("defer_until") {
        if v.is_null() {
            updates.defer_until = Some(None);
        } else if let Some(s) = v.as_str() {
            let (dt, warning) = parse_timestamp(s)?;
            if let Some(w) = warning {
                coercions.push(format!("defer_until: {w}"));
            }
            updates.defer_until = Some(Some(dt));
        } else {
            return Err(McpError::invalid_params(format!(
                "'defer_until' must be an ISO 8601 string or null, got {v}"
            )));
        }
    }
    if let Some(v) = args.get("estimated_minutes") {
        if v.is_null() {
            updates.estimated_minutes = Some(None);
        } else if let Some(n) = v.as_i64() {
            let mins = i32::try_from(n)
                .map_err(|_| McpError::invalid_params("estimated_minutes must fit in i32"))?;
            updates.estimated_minutes = Some(Some(mins));
        } else if let Some(s) = v.as_str() {
            // Forgive by Default: coerce string → integer
            let mins: i32 = s.parse().map_err(|_| {
                McpError::invalid_params(format!(
                    "'estimated_minutes' must be an integer, got string '{s}'"
                ))
            })?;
            coercions.push(format!(
                "estimated_minutes: string '{s}' coerced to integer {mins}"
            ));
            updates.estimated_minutes = Some(Some(mins));
        } else {
            return Err(McpError::invalid_params(format!(
                "'estimated_minutes' must be an integer or null, got {v}"
            )));
        }
    }
    updates.external_ref = nullable_str(args, "external_ref")?;

    Ok((updates, coercions))
}

/// Serialize an issue to JSON for output.
fn issue_json(issue: &Issue) -> serde_json::Value {
    serde_json::to_value(issue).unwrap_or_else(|_| json!({"id": issue.id, "title": issue.title}))
}

// ---------------------------------------------------------------------------
// 1. list_issues
// ---------------------------------------------------------------------------

/// Build `ListFilters` from the JSON arguments, collecting coercion warnings.
fn build_list_filters(
    args: &serde_json::Value,
    coercions: &mut Vec<String>,
) -> McpResult<ListFilters> {
    let statuses = args
        .get("status")
        .and_then(|v| v.as_str())
        .map(|s| {
            s.split(',')
                .map(|p| {
                    let (status, warning) = parse_status(p.trim())?;
                    if let Some(w) = warning {
                        coercions.push(w);
                    }
                    Ok(status)
                })
                .collect::<McpResult<Vec<_>>>()
        })
        .transpose()?;

    let types = args.get("type").and_then(|v| v.as_str()).map(|s| {
        s.split(',')
            .map(|p| {
                let (t, warning) = parse_issue_type(p.trim());
                if let Some(w) = warning {
                    coercions.push(w);
                }
                t
            })
            .collect::<Vec<_>>()
    });

    let priorities = args
        .get("priority")
        .and_then(|v| v.as_str())
        .map(|s| {
            s.split(',')
                .map(|p| {
                    let (prio, warning) = parse_priority(p.trim())?;
                    if let Some(w) = warning {
                        coercions.push(w);
                    }
                    Ok(prio)
                })
                .collect::<McpResult<Vec<_>>>()
        })
        .transpose()?;

    let labels = args.get("labels").and_then(|v| v.as_str()).map(|s| {
        s.split(',')
            .map(|l| l.trim().to_string())
            .collect::<Vec<_>>()
    });

    let include_closed = args
        .get("include_closed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let raw_limit = args
        .get("limit")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(50);
    let limit = Some(raw_limit.min(500) as usize);

    let sort = args.get("sort").and_then(|v| v.as_str()).map(String::from);

    let title_contains = args.get("title").and_then(|v| v.as_str()).map(String::from);

    // Forgive by Default: if the status filter explicitly includes Closed or
    // Deferred, automatically enable the corresponding include flag so the
    // query doesn't contradict itself (the default exclusion filters would
    // otherwise produce zero results).
    let include_closed = include_closed
        || statuses
            .as_ref()
            .is_some_and(|s| s.contains(&Status::Closed));
    let include_deferred = statuses
        .as_ref()
        .is_some_and(|s| s.contains(&Status::Deferred));

    Ok(ListFilters {
        statuses,
        types,
        priorities,
        assignee: args
            .get("assignee")
            .and_then(|v| v.as_str())
            .map(String::from),
        include_closed,
        include_deferred,
        limit,
        sort,
        labels,
        title_contains,
        ..ListFilters::default()
    })
}

pub struct ListIssuesTool(Arc<BeadsState>);
impl ListIssuesTool {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ToolHandler for ListIssuesTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "list_issues".into(),
            description: Some(
                "List issues matching filters. Returns JSON array of issues.\n\n\
                 Discovery: Use beads://labels resource to see valid label values.\n\
                 When to use: Exploring the backlog, finding issues by status/type/priority/label.\n\
                 NOT for: Getting full details on one issue — use show_issue instead.\n\
                 Do: Start broad (no filters) then narrow down. Use limit to cap output.\n\
                 Don't: Fetch all issues when you only need a count — use project_overview.\n\
                 Note: 'search' (full-text) and 'title' (substring) can be combined but 'search' is preferred for discovery.\n\
                 Inputs auto-corrected: 'wip' → in_progress, 'urgent' → critical, etc.\n\
                 Idempotency: Safe to retry; read-only."
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "description": "Filter by status (comma-separated). Values: open, in_progress, blocked, deferred, draft, closed, pinned. Aliases accepted: wip, todo, done, stuck, later, hold."
                    },
                    "type": {
                        "type": "string",
                        "description": "Filter by issue type: task, bug, feature, epic, chore, docs, question. Aliases: feat, defect, enhancement, refactor, doc."
                    },
                    "priority": {
                        "type": "string",
                        "description": "Filter by priority: critical, high, medium, low, backlog. Aliases: urgent, important, normal, minor, someday."
                    },
                    "assignee": {
                        "type": "string",
                        "description": "Filter by assignee name"
                    },
                    "labels": {
                        "type": "string",
                        "description": "Filter by labels (comma-separated, AND logic). See beads://labels for valid values."
                    },
                    "title": {
                        "type": "string",
                        "description": "Filter by title substring (case-insensitive). For full-text search use 'search' instead."
                    },
                    "search": {
                        "type": "string",
                        "description": "Full-text search query against title/description. Leading wildcards are stripped."
                    },
                    "include_closed": {
                        "type": "boolean",
                        "description": "Include closed issues (default false)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max issues to return (default 50, max 500)"
                    },
                    "sort": {
                        "type": "string",
                        "description": "Sort field: priority, created, updated, title (default: updated)"
                    }
                },
                "additionalProperties": false
            }),
            output_schema: None,
            icon: None,
            version: None,
            tags: vec![],
            annotations: Some(ToolAnnotations {
                read_only: Some(true),
                destructive: Some(false),
                idempotent: Some(true),
                open_world_hint: None,
            }),
        }
    }

    fn call(&self, _ctx: &McpContext, args: serde_json::Value) -> McpResult<Vec<Content>> {
        let storage = open(&self.0)?;
        let mut coercions: Vec<String> = Vec::new();
        let filters = build_list_filters(&args, &mut coercions)?;

        let search_query = args.get("search").and_then(|v| v.as_str());

        let issues = if let Some(raw_q) = search_query {
            if let Some(q) = sanitize_search(raw_q) {
                storage.search_issues(&q, &filters).map_err(beads_to_mcp)?
            } else {
                // Bare wildcard — fall back to list (no search filter)
                coercions.push(format!(
                    "search '{raw_q}' was a bare wildcard, returning all"
                ));
                storage.list_issues(&filters).map_err(beads_to_mcp)?
            }
        } else {
            storage.list_issues(&filters).map_err(beads_to_mcp)?
        };

        let mut result = json!({
            "count": issues.len(),
            "issues": issues.iter().map(issue_json).collect::<Vec<_>>(),
        });

        // Contextual next_actions
        if issues.is_empty() {
            result["next_actions"] = json!([
                "No issues matched. Try broadening filters or removing some.",
                "Use project_overview to see what's in the tracker"
            ]);
        } else {
            result["next_actions"] = json!([
                "Use show_issue with an issue ID for full details",
                "Narrow results with additional filters"
            ]);
        }

        if !coercions.is_empty() {
            result["coercions"] = json!(coercions);
        }

        Ok(vec![Content::text(result.to_string())])
    }
}

// ---------------------------------------------------------------------------
// 2. show_issue
// ---------------------------------------------------------------------------

pub struct ShowIssueTool(Arc<BeadsState>);
impl ShowIssueTool {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ToolHandler for ShowIssueTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "show_issue".into(),
            description: Some(
                "Get full details for a single issue including comments, dependencies, and events.\n\n\
                 Discovery: Get IDs from list_issues or project_overview.\n\
                 When to use: You have an issue ID and need complete context.\n\
                 NOT for: Browsing multiple issues — use list_issues instead.\n\
                 Do: Request specific issue IDs you already know.\n\
                 Don't: Guess IDs — use list_issues to discover them first.\n\
                 Common mistakes: Using placeholder IDs ('YOUR_ID') — these are detected.\n\
                 Idempotency: Safe to retry; read-only."
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Issue ID (e.g. 'br-1a2b3c'). MUST be an exact ID from list_issues. Placeholder values are rejected."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            output_schema: None,
            icon: None,
            version: None,
            tags: vec![],
            annotations: Some(ToolAnnotations {
                read_only: Some(true),
                destructive: Some(false),
                idempotent: Some(true),
                open_world_hint: None,
            }),
        }
    }

    fn call(&self, _ctx: &McpContext, args: serde_json::Value) -> McpResult<Vec<Content>> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::invalid_params("'id' is required"))?;

        // Placeholder detection before any DB work
        if let Some(err) = detect_placeholder(id) {
            return Err(err);
        }

        let storage = open(&self.0)?;

        let details = storage
            .get_issue_details(id, true, true, 20)
            .map_err(beads_to_mcp)?
            .ok_or_else(|| issue_not_found_err(&storage, id))?;

        let mut result = serde_json::to_value(&details.issue).unwrap_or_default();
        if let Some(obj) = result.as_object_mut() {
            obj.insert("labels".into(), json!(details.labels));
            obj.insert("comments".into(), json!(details.comments));
            obj.insert(
                "dependencies".into(),
                json!(
                    details
                        .dependencies
                        .iter()
                        .map(|d| {
                            json!({
                                "id": d.id,
                                "title": d.title,
                                "status": d.status,
                                "dep_type": d.dep_type
                            })
                        })
                        .collect::<Vec<_>>()
                ),
            );
            obj.insert(
                "dependents".into(),
                json!(
                    details
                        .dependents
                        .iter()
                        .map(|d| {
                            json!({
                                "id": d.id,
                                "title": d.title,
                                "status": d.status,
                                "dep_type": d.dep_type
                            })
                        })
                        .collect::<Vec<_>>()
                ),
            );
            if let Some(parent) = &details.parent {
                obj.insert("parent".into(), json!(parent));
            }
            obj.insert(
                "recent_events".into(),
                json!(
                    details
                        .events
                        .iter()
                        .take(10)
                        .map(|e| {
                            json!({
                                "type": e.event_type,
                                "actor": e.actor,
                                "old_value": e.old_value,
                                "new_value": e.new_value,
                                "created_at": e.created_at
                            })
                        })
                        .collect::<Vec<_>>()
                ),
            );

            // Contextual next_actions based on issue state
            let mut actions: Vec<String> = Vec::new();
            if details.issue.status == Status::Blocked {
                actions.push(
                    "This issue is blocked. Use manage_dependencies action 'list' to see blockers."
                        .into(),
                );
            }
            if details.issue.assignee.is_none() && details.issue.status != Status::Closed {
                actions.push("No assignee — consider assigning with update_issue.".into());
            }
            if details.issue.status == Status::Closed {
                actions.push("This issue is closed. Use list_issues to find open work.".into());
            } else {
                actions.push("Use update_issue to modify fields or add a comment.".into());
                actions.push("Use manage_dependencies to link to other issues.".into());
            }
            obj.insert("next_actions".into(), json!(actions));
        }

        Ok(vec![Content::text(result.to_string())])
    }
}

// ---------------------------------------------------------------------------
// 3. create_issue
// ---------------------------------------------------------------------------

pub struct CreateIssueTool(Arc<BeadsState>);
impl CreateIssueTool {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ToolHandler for CreateIssueTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "create_issue".into(),
            description: Some(
                "Create a new issue. Returns the created issue with its ID.\n\n\
                 Discovery: See beads://schema for valid types/priorities, beads://labels for labels.\n\
                 When to use: Recording a new bug, feature, task, or work item.\n\
                 NOT for: Updating existing issues — use update_issue instead.\n\
                 Do: Provide a clear title (1-500 chars). Search with list_issues first to avoid dupes.\n\
                 Don't: Create duplicate issues — search first.\n\
                 Inputs auto-corrected: 'urgent' → critical, 'feat' → feature, etc.\n\
                 Idempotency: NOT idempotent — each call creates a new issue."
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Issue title (1-500 chars). REQUIRED."
                    },
                    "description": {
                        "type": "string",
                        "description": "Detailed description of the issue"
                    },
                    "type": {
                        "type": "string",
                        "description": "Issue type: task (default), bug, feature, epic, chore, docs, question. Aliases: feat, defect, enhancement, refactor, doc."
                    },
                    "priority": {
                        "type": "string",
                        "description": "Priority: critical, high, medium (default), low, backlog. Aliases: urgent, important, normal, minor, someday."
                    },
                    "assignee": {
                        "type": "string",
                        "description": "Assign to a user"
                    },
                    "labels": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Labels to attach. See beads://labels for existing labels."
                    },
                    "parent": {
                        "type": "string",
                        "description": "Parent issue ID to create as sub-issue. Creates a parent-child dependency automatically."
                    }
                },
                "required": ["title"],
                "additionalProperties": false
            }),
            output_schema: None,
            icon: None,
            version: None,
            tags: vec![],
            annotations: Some(ToolAnnotations {
                read_only: None,
                destructive: Some(false),
                idempotent: Some(false),
                open_world_hint: None,
            }),
        }
    }

    fn call(&self, _ctx: &McpContext, args: serde_json::Value) -> McpResult<Vec<Content>> {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::invalid_params("'title' is required"))?;

        if title.is_empty() || title.len() > 500 {
            return Err(McpError::invalid_params("Title must be 1-500 characters"));
        }

        let mut coercions: Vec<String> = Vec::new();

        let (issue_type, type_warning) = args
            .get("type")
            .and_then(|v| v.as_str())
            .map_or((IssueType::Task, None), parse_issue_type);
        if let Some(w) = type_warning {
            coercions.push(w);
        }

        let (priority, prio_warning) = args
            .get("priority")
            .and_then(|v| v.as_str())
            .map(parse_priority)
            .transpose()?
            .unwrap_or((Priority::MEDIUM, None));
        if let Some(w) = prio_warning {
            coercions.push(w);
        }

        let mut storage = open(&self.0)?;

        let now = chrono::Utc::now();
        let prefix = self.0.issue_prefix.as_deref().unwrap_or("br");
        let id_gen =
            crate::util::id::IdGenerator::new(crate::util::id::IdConfig::with_prefix(prefix));
        let id = id_gen.generate(title, None, Some(&self.0.actor), now, 0, |candidate| {
            storage.id_exists(candidate).unwrap_or(false)
        });

        let issue = Issue {
            id: id.clone(),
            title: title.to_string(),
            description: args
                .get("description")
                .and_then(|v| v.as_str())
                .map(String::from),
            status: Status::Open,
            priority,
            issue_type,
            assignee: args
                .get("assignee")
                .and_then(|v| v.as_str())
                .map(String::from),
            created_by: Some(self.0.actor.clone()),
            created_at: now,
            updated_at: now,
            ..Issue::default()
        };

        // Validate parent exists BEFORE creating the issue to avoid orphans
        let parent_id = args
            .get("parent")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        if let Some(ref pid) = parent_id {
            require_valid_issue(&storage, pid)?;
        }

        storage
            .create_issue(&issue, &self.0.actor)
            .map_err(beads_to_mcp)?;

        let mut warnings: Vec<String> = Vec::new();

        // Attach labels
        if let Some(labels) = args.get("labels").and_then(|v| v.as_array()) {
            for label_val in labels {
                if let Some(label) = label_val.as_str()
                    && let Err(e) = storage.add_label(&id, label, &self.0.actor)
                {
                    warnings.push(format!("failed to add label '{label}': {e}"));
                }
            }
        }

        // Create parent-child dependency (parent already validated above)
        let mut parent_linked = false;
        if let Some(ref pid) = parent_id {
            if let Err(e) = storage.add_dependency(&id, pid, "parent-child", &self.0.actor) {
                warnings.push(format!("failed to link parent '{pid}': {e}"));
            } else {
                parent_linked = true;
            }
        }

        let mut result = json!({
            "id": id,
            "title": issue.title,
            "status": "open",
            "priority": issue.priority,
            "type": issue.issue_type,
            "next_actions": [
                "Use update_issue to add details or change fields",
                "Use manage_dependencies to link to other issues"
            ]
        });

        if parent_linked {
            result["parent"] = json!(parent_id);
        }

        if !coercions.is_empty() {
            result["coercions"] = json!(coercions);
        }

        if !warnings.is_empty() {
            result["warnings"] = json!(warnings);
        }

        Ok(vec![Content::text(result.to_string())])
    }
}

// ---------------------------------------------------------------------------
// 4. update_issue
// ---------------------------------------------------------------------------

pub struct UpdateIssueTool(Arc<BeadsState>);
impl UpdateIssueTool {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ToolHandler for UpdateIssueTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "update_issue".into(),
            description: Some(
                "Update fields on an existing issue. Only provided fields are changed.\n\n\
                 Discovery: Get IDs from list_issues. See beads://schema for valid values.\n\
                 When to use: Changing status, priority, assignee, adding comments.\n\
                 NOT for: Closing issues — use close_issue for proper close tracking.\n\
                 Do: Provide only the fields you want to change.\n\
                 Don't: Set status to 'closed' — you'll be redirected to close_issue.\n\
                 Inputs auto-corrected: 'wip' → in_progress, 'urgent' → critical, etc.\n\
                 Idempotency: Safe to retry with the same values."
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Issue ID to update. REQUIRED. Must be a real ID from list_issues."
                    },
                    "title": {
                        "type": "string",
                        "description": "New title (1-500 chars)"
                    },
                    "description": {
                        "type": ["string", "null"],
                        "description": "New description (null to clear)"
                    },
                    "status": {
                        "type": "string",
                        "description": "New status: open, in_progress, blocked, deferred, draft, pinned. NOT 'closed' — use close_issue instead. Aliases accepted."
                    },
                    "priority": {
                        "type": "string",
                        "description": "New priority: critical, high, medium, low, backlog. Aliases: urgent, important, normal, minor, someday."
                    },
                    "type": {
                        "type": "string",
                        "description": "New issue type: task, bug, feature, epic, chore, docs, question. Aliases accepted."
                    },
                    "assignee": {
                        "type": ["string", "null"],
                        "description": "New assignee (null to unassign)"
                    },
                    "owner": {
                        "type": ["string", "null"],
                        "description": "New owner (null to clear). Owner is the person responsible, assignee does the work."
                    },
                    "due_at": {
                        "type": ["string", "null"],
                        "description": "Due date (ISO 8601 or 'YYYY-MM-DD'). Null to clear. Auto-coerced: slashes → dashes, missing timezone → UTC."
                    },
                    "defer_until": {
                        "type": ["string", "null"],
                        "description": "Defer until date (ISO 8601 or 'YYYY-MM-DD'). Null to clear. Issue will be deferred until this date."
                    },
                    "estimated_minutes": {
                        "type": ["integer", "null"],
                        "description": "Estimated effort in minutes. Null to clear."
                    },
                    "external_ref": {
                        "type": ["string", "null"],
                        "description": "External tracker reference (e.g. GitHub issue URL). Null to clear."
                    },
                    "labels_add": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Labels to add. See beads://labels for existing labels."
                    },
                    "labels_remove": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Labels to remove"
                    },
                    "comment": {
                        "type": "string",
                        "description": "Add a comment to the issue (appended after field updates)"
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            output_schema: None,
            icon: None,
            version: None,
            tags: vec![],
            annotations: Some(ToolAnnotations {
                read_only: None,
                destructive: Some(false),
                idempotent: Some(true),
                open_world_hint: None,
            }),
        }
    }

    fn call(&self, _ctx: &McpContext, args: serde_json::Value) -> McpResult<Vec<Content>> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::invalid_params("'id' is required"))?;

        let (updates, coercions) = parse_update_fields(id, &args)?;

        let mut storage = open(&self.0)?;

        // Validate ID exists before attempting update (placeholder + existence check)
        require_valid_issue(&storage, id)?;

        // Detect whether we have field changes vs label/comment side-effects.
        // If only side-effects (labels_add, labels_remove, comment), skip the
        // update_issue call which would fail with NothingToDo.
        let has_field_updates = UPDATE_FIELD_KEYS.iter().any(|k| args.get(k).is_some());
        let has_side_effects = args
            .get("labels_add")
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty())
            || args
                .get("labels_remove")
                .and_then(|v| v.as_array())
                .is_some_and(|a| !a.is_empty())
            || args
                .get("comment")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty());

        let issue = if has_field_updates {
            storage
                .update_issue(id, &updates, &self.0.actor)
                .map_err(beads_to_mcp)?
        } else if has_side_effects {
            // No field changes but labels/comments to process — read current state
            storage
                .get_issue_details(id, false, false, 0)
                .map_err(beads_to_mcp)?
                .ok_or_else(|| issue_not_found_err(&storage, id))?
                .issue
        } else {
            return Err(McpError::with_data(
                McpErrorCode::ToolExecutionError,
                "No changes specified",
                json!({
                    "error_type": "NOTHING_TO_DO",
                    "recoverable": true,
                    "hint": "Provide at least one field to update, a label operation, or a comment",
                    "suggested_tool_calls": [
                        {"tool": "show_issue", "arguments": {"id": id}}
                    ]
                }),
            ));
        };

        let mut warnings: Vec<String> = Vec::new();

        // Handle label mutations
        if let Some(labels) = args.get("labels_add").and_then(|v| v.as_array()) {
            for label_val in labels {
                if let Some(label) = label_val.as_str()
                    && let Err(e) = storage.add_label(id, label, &self.0.actor)
                {
                    warnings.push(format!("failed to add label '{label}': {e}"));
                }
            }
        }
        if let Some(labels) = args.get("labels_remove").and_then(|v| v.as_array()) {
            for label_val in labels {
                if let Some(label) = label_val.as_str()
                    && let Err(e) = storage.remove_label(id, label, &self.0.actor)
                {
                    warnings.push(format!("failed to remove label '{label}': {e}"));
                }
            }
        }

        // Add comment if provided
        if let Some(comment) = args.get("comment").and_then(|v| v.as_str())
            && !comment.is_empty()
            && let Err(e) = storage.add_comment(id, &self.0.actor, comment)
        {
            warnings.push(format!("failed to add comment: {e}"));
        }

        let mut result = json!({
            "id": issue.id,
            "title": issue.title,
            "status": issue.status,
            "priority": issue.priority,
            "updated_at": issue.updated_at,
        });

        if !coercions.is_empty() {
            result["coercions"] = json!(coercions);
        }

        if !warnings.is_empty() {
            result["warnings"] = json!(warnings);
        }

        Ok(vec![Content::text(result.to_string())])
    }
}

// ---------------------------------------------------------------------------
// 5. close_issue
// ---------------------------------------------------------------------------

pub struct CloseIssueTool(Arc<BeadsState>);
impl CloseIssueTool {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ToolHandler for CloseIssueTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "close_issue".into(),
            description: Some(
                "Close an issue with a reason. Sets status to Closed and records close metadata.\n\n\
                 Discovery: Get IDs from list_issues.\n\
                 When to use: Completing, cancelling, or resolving an issue.\n\
                 NOT for: Changing status to anything other than closed — use update_issue.\n\
                 Do: Provide a close_reason explaining why.\n\
                 Don't: Close issues without checking open blockers first.\n\
                 Idempotency: Safe to retry — closing an already-closed issue is a no-op."
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Issue ID to close. REQUIRED. Must be a real ID from list_issues."
                    },
                    "reason": {
                        "type": "string",
                        "description": "Why this issue is being closed (e.g. 'completed', 'wontfix', 'duplicate')"
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            output_schema: None,
            icon: None,
            version: None,
            tags: vec![],
            annotations: Some(ToolAnnotations {
                read_only: None,
                destructive: Some(false),
                idempotent: Some(true),
                open_world_hint: None,
            }),
        }
    }

    fn call(&self, _ctx: &McpContext, args: serde_json::Value) -> McpResult<Vec<Content>> {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::invalid_params("'id' is required"))?;

        let reason = args
            .get("reason")
            .and_then(|v| v.as_str())
            .map(String::from);

        let mut storage = open(&self.0)?;

        // Validate ID exists (with placeholder detection + fuzzy suggestions)
        require_valid_issue(&storage, id)?;

        // Idempotency: if already closed, return existing state without error
        if let Some(details) = storage
            .get_issue_details(id, false, false, 0)
            .map_err(beads_to_mcp)?
            && details.issue.status == Status::Closed
        {
            return Ok(vec![Content::text(
                json!({
                    "id": details.issue.id,
                    "title": details.issue.title,
                    "status": "closed",
                    "closed_at": details.issue.closed_at,
                    "close_reason": details.issue.close_reason,
                    "already_closed": true,
                    "next_actions": ["Issue was already closed. Use list_issues to find open work."]
                })
                .to_string(),
            )]);
        }

        let now = chrono::Utc::now();
        let close_update = IssueUpdate {
            status: Some(Status::Closed),
            closed_at: Some(Some(now)),
            close_reason: Some(reason.clone()),
            ..IssueUpdate::default()
        };

        let issue = storage
            .update_issue(id, &close_update, &self.0.actor)
            .map_err(beads_to_mcp)?;

        let mut result = json!({
            "id": issue.id,
            "title": issue.title,
            "status": "closed",
            "closed_at": now,
            "close_reason": reason,
        });

        // Check for blockers this issue had (warn about closing a blocked issue)
        let our_blockers = storage.get_blockers(id).unwrap_or_default();
        if !our_blockers.is_empty() {
            result["warning"] = json!(format!(
                "This issue was blocked by {} issue(s): {}. Consider whether those blockers are still relevant.",
                our_blockers.len(),
                our_blockers.join(", ")
            ));
        }

        // Check what this issue was blocking (now potentially unblocked)
        let dependents = storage.get_dependents(id).unwrap_or_default();
        if dependents.is_empty() {
            result["next_actions"] = json!(["Issue closed successfully."]);
        } else {
            result["unblocked_candidates"] = json!(dependents);
            result["next_actions"] = json!([
                format!(
                    "{} dependent issue(s) may now be unblocked: {}",
                    dependents.len(),
                    dependents.join(", ")
                ),
                "Use show_issue on these to check if they're now ready for work"
            ]);
        }

        Ok(vec![Content::text(result.to_string())])
    }
}

// ---------------------------------------------------------------------------
// 6. manage_dependencies
// ---------------------------------------------------------------------------

/// Handle the "add" action for `manage_dependencies`.
fn dep_add(
    storage: &mut SqliteStorage,
    actor: &str,
    id: &str,
    args: &serde_json::Value,
) -> McpResult<Vec<Content>> {
    let depends_on = args
        .get("depends_on")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::invalid_params("'depends_on' is required for action 'add'"))?;

    // Validate target ID exists too
    require_valid_issue(storage, depends_on)?;

    let dep_type_raw = args
        .get("dep_type")
        .and_then(|v| v.as_str())
        .unwrap_or("blocks");
    let (dep_type_str, dep_coercion) = parse_dep_type(dep_type_raw)?;

    // Check for cycles with structured error
    if storage
        .would_create_cycle(id, depends_on, true)
        .map_err(beads_to_mcp)?
    {
        return Err(McpError::with_data(
            McpErrorCode::ToolExecutionError,
            format!("Adding dependency {id} -> {depends_on} would create a cycle"),
            json!({
                "error_type": "CYCLE_DETECTED",
                "recoverable": false,
                "from": id,
                "to": depends_on,
                "hint": "Circular dependencies are not allowed. Check the existing dependency graph.",
                "suggested_tool_calls": [
                    {"tool": "manage_dependencies", "arguments": {"action": "list", "id": id}},
                    {"tool": "manage_dependencies", "arguments": {"action": "list", "id": depends_on}}
                ]
            }),
        ));
    }

    let added = storage
        .add_dependency(id, depends_on, &dep_type_str, actor)
        .map_err(beads_to_mcp)?;

    let mut result = json!({
        "added": added,
        "from": id,
        "to": depends_on,
        "dep_type": dep_type_str,
    });
    if let Some(w) = dep_coercion {
        result["coercion"] = json!(w);
    }

    Ok(vec![Content::text(result.to_string())])
}

/// Handle the "remove" action for `manage_dependencies`.
fn dep_remove(
    storage: &mut SqliteStorage,
    actor: &str,
    id: &str,
    args: &serde_json::Value,
) -> McpResult<Vec<Content>> {
    let depends_on = args
        .get("depends_on")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::invalid_params("'depends_on' is required for action 'remove'"))?;

    // Validate target ID (placeholder check only — it might have been deleted)
    if let Some(err) = detect_placeholder(depends_on) {
        return Err(err);
    }

    let removed = storage
        .remove_dependency(id, depends_on, actor)
        .map_err(beads_to_mcp)?;

    Ok(vec![Content::text(
        json!({
            "removed": removed,
            "from": id,
            "to": depends_on,
        })
        .to_string(),
    )])
}

pub struct ManageDependenciesTool(Arc<BeadsState>);
impl ManageDependenciesTool {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ToolHandler for ManageDependenciesTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "manage_dependencies".into(),
            description: Some(
                "Add, remove, or list dependencies between issues.\n\n\
                 Discovery: Get issue IDs from list_issues. See beads://schema for dep types.\n\
                 When to use: Linking related issues, establishing blocking relationships.\n\
                 NOT for: Viewing blocked issues overview — use beads://issues/blocked resource.\n\
                 Do: Use 'list' action first to see existing deps before modifying.\n\
                 Don't: Create circular deps — the system will reject them with guidance.\n\
                 Common mistakes: Swapping source/target for 'blocks' type; using placeholder IDs.\n\
                 Both IDs are validated before any operation."
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["add", "remove", "list"],
                        "description": "Action to perform. REQUIRED."
                    },
                    "id": {
                        "type": "string",
                        "description": "Source issue ID. REQUIRED. Must be a real ID from list_issues."
                    },
                    "depends_on": {
                        "type": "string",
                        "description": "Target issue ID (required for add/remove). Must be a real ID."
                    },
                    "dep_type": {
                        "type": "string",
                        "description": "Dependency type: blocks (default), related, parent-child, waits-for, duplicates, supersedes, caused-by. Aliases auto-corrected (e.g. 'parent_child' → 'parent-child').",
                        "default": "blocks"
                    }
                },
                "required": ["action", "id"],
                "additionalProperties": false
            }),
            output_schema: None,
            icon: None,
            version: None,
            tags: vec![],
            annotations: Some(ToolAnnotations {
                read_only: None,
                destructive: Some(false),
                idempotent: None,
                open_world_hint: Some(
                    "list action is read-only; add is idempotent; remove is not".into(),
                ),
            }),
        }
    }

    fn call(&self, _ctx: &McpContext, args: serde_json::Value) -> McpResult<Vec<Content>> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::with_data(
                    McpErrorCode::InvalidParams,
                    "'action' is required",
                    json!({
                        "error_type": "REQUIRED_FIELD",
                        "available_options": ["add", "remove", "list"],
                        "fix_hint": "Provide action: 'list' to view, 'add' to create, 'remove' to delete"
                    }),
                )
            })?;

        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError::invalid_params("'id' is required"))?;

        let mut storage = open(&self.0)?;

        // Validate source ID for all actions
        require_valid_issue(&storage, id)?;

        match action {
            "list" => {
                let deps = storage.get_dependencies_full(id).map_err(beads_to_mcp)?;
                let dependents = storage.get_dependents(id).map_err(beads_to_mcp)?;

                Ok(vec![Content::text(
                    json!({
                        "id": id,
                        "depends_on": deps.iter().map(|d| {
                            json!({
                                "id": d.depends_on_id,
                                "dep_type": d.dep_type.to_string(),
                            })
                        }).collect::<Vec<_>>(),
                        "depended_on_by": dependents,
                    })
                    .to_string(),
                )])
            }
            "add" => dep_add(&mut storage, &self.0.actor, id, &args),
            "remove" => dep_remove(&mut storage, &self.0.actor, id, &args),
            other => Err(McpError::with_data(
                McpErrorCode::InvalidParams,
                format!("Unknown action '{other}'"),
                json!({
                    "error_type": "INVALID_ARGUMENT",
                    "provided": other,
                    "available_options": ["add", "remove", "list"],
                    "fix_hint": "Use 'list' to view dependencies, 'add' to create, 'remove' to delete"
                }),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// 7. project_overview
// ---------------------------------------------------------------------------

pub struct ProjectOverviewTool(Arc<BeadsState>);
impl ProjectOverviewTool {
    pub fn new(state: Arc<BeadsState>) -> Self {
        Self(state)
    }
}

impl ToolHandler for ProjectOverviewTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "project_overview".into(),
            description: Some(
                "Get a high-level project summary: counts by status, top labels, blocked/ready work.\n\n\
                 When to use: Starting a session, getting oriented, understanding project health.\n\
                 NOT for: Detailed filtering — use list_issues with filters instead.\n\
                 Do: Call this first when you connect to understand the project state.\n\
                 Don't: Call repeatedly — the data doesn't change unless you mutate issues.\n\
                 Idempotency: Safe to retry; read-only."
                    .into(),
            ),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            output_schema: None,
            icon: None,
            version: None,
            tags: vec![],
            annotations: Some(ToolAnnotations {
                read_only: Some(true),
                destructive: Some(false),
                idempotent: Some(true),
                open_world_hint: None,
            }),
        }
    }

    fn call(&self, _ctx: &McpContext, args: serde_json::Value) -> McpResult<Vec<Content>> {
        let _ = args;
        let storage = open(&self.0)?;

        let total = storage.count_all_issues().map_err(beads_to_mcp)?;
        let active = storage.count_active_issues().map_err(beads_to_mcp)?;
        let labels = storage
            .get_unique_labels_with_counts()
            .map_err(beads_to_mcp)?;
        let blocked = storage.get_blocked_issues().map_err(beads_to_mcp)?;
        let dirty = storage.get_dirty_issue_count().map_err(beads_to_mcp)?;

        let ready_filters = ReadyFilters::default();
        let ready = storage
            .get_ready_issues(&ready_filters, ReadySortPolicy::Hybrid)
            .map_err(beads_to_mcp)?;

        // In-progress and deferred counts
        let in_progress_filters = ListFilters {
            statuses: Some(vec![Status::InProgress]),
            include_closed: false,
            limit: Some(50),
            ..ListFilters::default()
        };
        let in_progress = storage
            .list_issues(&in_progress_filters)
            .map_err(beads_to_mcp)?;

        let deferred_filters = ListFilters {
            statuses: Some(vec![Status::Deferred]),
            include_closed: false,
            include_deferred: true,
            limit: Some(50),
            ..ListFilters::default()
        };
        let deferred = storage
            .list_issues(&deferred_filters)
            .map_err(beads_to_mcp)?;

        let prefix = self.0.issue_prefix.as_deref().unwrap_or("br");

        let result = json!({
            "project": {
                "beads_dir": self.0.beads_dir.display().to_string(),
                "issue_prefix": prefix,
            },
            "counts": {
                "total": total,
                "active": active,
                "blocked": blocked.len(),
                "ready": ready.len(),
                "in_progress": in_progress.len(),
                "deferred": deferred.len(),
                "dirty_unsaved": dirty,
            },
            "top_labels": labels.iter().take(15).map(|(name, count)| {
                json!({"label": name, "count": count})
            }).collect::<Vec<_>>(),
            "blocked_issues": blocked.iter().take(10).map(|(issue, blockers)| {
                json!({
                    "id": issue.id,
                    "title": issue.title,
                    "blocked_by": blockers,
                })
            }).collect::<Vec<_>>(),
            "ready_issues": ready.iter().take(10).map(|issue| {
                json!({
                    "id": issue.id,
                    "title": issue.title,
                    "priority": issue.priority,
                    "type": issue.issue_type,
                })
            }).collect::<Vec<_>>(),
            "discovery": {
                "resources": [
                    "beads://schema — valid field values, aliases, and bead anatomy guidance",
                    "beads://labels — all labels with counts",
                    "beads://issues/ready — actionable work",
                    "beads://issues/blocked — stuck items with blockers",
                    "beads://issues/bottlenecks — highest-impact blockers (bv-style)",
                    "beads://graph/health — dependency graph health metrics",
                    "beads://issues/in_progress — current work",
                    "beads://issues/deferred — deferred items",
                    "beads://events/recent — latest audit trail",
                    "beads://project/info — project metadata"
                ],
                "prompts": [
                    "triage — guided backlog triage workflow",
                    "status_report — project status report generation",
                    "plan_next_work — graph-aware work planning (bottlenecks, quick wins)",
                    "polish_backlog — review issue quality and dependency health"
                ]
            },
            "next_actions": [
                "Use list_issues to explore specific subsets",
                "Use show_issue to dig into a specific issue",
                "Use create_issue to add new work items"
            ]
        });

        Ok(vec![Content::text(result.to_string())])
    }
}
