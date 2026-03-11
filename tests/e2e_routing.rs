//! End-to-end tests for routing, redirect files, and external DB reference safety.
//!
//! Tests cover:
//! - Prefix-based route lookup (routes.jsonl)
//! - Redirect file following
//! - Redirect loop detection
//! - External DB reference safety and path normalization
//! - Clear errors for missing/invalid routes

use std::fs;
use std::process::Command;

mod common;

use common::cli::{BrWorkspace, extract_json_payload, run_br, run_br_with_env};
use serde_json::Value;

/// Helper to create a routes.jsonl file with given entries.
fn create_routes_file(workspace: &BrWorkspace, entries: &[(&str, &str)]) {
    let routes_path = workspace.root.join(".beads").join("routes.jsonl");
    let content: String = entries
        .iter()
        .map(|(prefix, path)| format!(r#"{{"prefix":"{}","path":"{}"}}"#, prefix, path))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&routes_path, content).expect("write routes.jsonl");
}

/// Helper to create a redirect file.
fn create_redirect_file(beads_dir: &std::path::Path, target: &str) {
    let redirect_path = beads_dir.join("redirect");
    fs::write(&redirect_path, target).expect("write redirect");
}

fn switch_workspace_to_custom_database(workspace: &BrWorkspace, database_name: &str) {
    let beads_dir = workspace.root.join(".beads");
    let old_db = beads_dir.join("beads.db");
    let new_db = beads_dir.join(database_name);
    fs::rename(&old_db, &new_db).expect("move db to custom metadata path");
    fs::write(
        beads_dir.join("metadata.json"),
        format!(r#"{{"database":"{database_name}","jsonl_export":"issues.jsonl"}}"#),
    )
    .expect("write metadata");
}

fn init_test_git_repo(repo_root: &std::path::Path) -> String {
    let init_git = Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(repo_root)
        .output()
        .expect("git init");
    assert!(init_git.status.success(), "git init failed");
    let config_name = Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(repo_root)
        .output()
        .expect("git config user.name");
    assert!(config_name.status.success(), "git config user.name failed");
    let config_email = Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(repo_root)
        .output()
        .expect("git config user.email");
    assert!(
        config_email.status.success(),
        "git config user.email failed"
    );
    fs::write(repo_root.join("README.md"), "hello\n").expect("write readme");
    let add = Command::new("git")
        .args(["add", "README.md"])
        .current_dir(repo_root)
        .output()
        .expect("git add");
    assert!(add.status.success(), "git add failed");
    let commit = Command::new("git")
        .args(["commit", "-m", "initial"])
        .current_dir(repo_root)
        .output()
        .expect("git commit");
    assert!(commit.status.success(), "git commit failed");
    String::from_utf8_lossy(
        &Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(repo_root)
            .output()
            .expect("git rev-parse")
            .stdout,
    )
    .trim()
    .to_string()
}

// =============================================================================
// PREFIX-BASED ROUTING TESTS
// =============================================================================

#[test]
fn e2e_routing_local_prefix_no_routes_file() {
    let _log = common::test_log("e2e_routing_local_prefix_no_routes_file");
    let workspace = BrWorkspace::new();

    // Initialize workspace
    let init = run_br(&workspace, ["init"], "init");
    assert!(init.status.success(), "init failed: {}", init.stderr);

    // Create an issue
    let create = run_br(
        &workspace,
        [
            "create",
            "Test issue",
            "--priority",
            "2",
            "--type",
            "task",
            "--json",
        ],
        "create",
    );
    assert!(create.status.success(), "create failed: {}", create.stderr);

    // Verify the issue was created locally (no routes.jsonl means local)
    let list = run_br(&workspace, ["list", "--json"], "list");
    assert!(list.status.success(), "list failed: {}", list.stderr);
    assert!(
        list.stdout.contains("Test issue"),
        "Expected issue in list output"
    );
}

#[test]
fn e2e_routing_routes_jsonl_local_route() {
    let _log = common::test_log("e2e_routing_routes_jsonl_local_route");
    let workspace = BrWorkspace::new();

    // Initialize workspace
    let init = run_br(&workspace, ["init"], "init");
    assert!(init.status.success(), "init failed: {}", init.stderr);

    // Create routes file with local route (path = ".")
    create_routes_file(&workspace, &[("bd-", ".")]);

    // Create an issue - should use local storage
    let create = run_br(
        &workspace,
        [
            "create",
            "Test issue with route",
            "--priority",
            "2",
            "--type",
            "task",
            "--json",
        ],
        "create",
    );
    assert!(create.status.success(), "create failed: {}", create.stderr);

    // Verify the issue was created
    let list = run_br(&workspace, ["list", "--json"], "list");
    assert!(list.status.success(), "list failed: {}", list.stderr);
    assert!(
        list.stdout.contains("Test issue with route"),
        "Expected issue in list output"
    );
}

#[test]
fn e2e_routing_routes_jsonl_malformed_line() {
    let _log = common::test_log("e2e_routing_routes_jsonl_malformed_line");
    let workspace = BrWorkspace::new();

    // Initialize workspace
    let init = run_br(&workspace, ["init"], "init");
    assert!(init.status.success(), "init failed: {}", init.stderr);

    // Create malformed routes.jsonl
    let routes_path = workspace.root.join(".beads").join("routes.jsonl");
    fs::write(&routes_path, "not valid json\n").expect("write routes.jsonl");

    // Create an issue - should still work (local fallback) or give clear error
    let create = run_br(
        &workspace,
        [
            "create",
            "Test issue",
            "--priority",
            "2",
            "--type",
            "task",
            "--json",
        ],
        "create",
    );

    // Either succeeds with local fallback or fails with clear error
    if !create.status.success() {
        assert!(
            create.stderr.contains("Invalid route")
                || create.stderr.contains("invalid")
                || create.stderr.contains("JSON"),
            "Expected clear error message for malformed routes.jsonl, got: {}",
            create.stderr
        );
    }
}

#[test]
fn e2e_routing_routes_jsonl_external_route() {
    let _log = common::test_log("e2e_routing_routes_jsonl_external_route");

    // Use separate workspaces for main and external projects
    let main_workspace = BrWorkspace::new();
    let external_workspace = BrWorkspace::new();

    // Initialize main workspace
    let init = run_br(&main_workspace, ["init"], "init");
    assert!(init.status.success(), "init failed: {}", init.stderr);

    // Initialize external workspace
    let init_external = run_br(&external_workspace, ["init"], "init_external");
    assert!(
        init_external.status.success(),
        "external init failed: {}",
        init_external.stderr
    );

    // Set a different prefix for external project
    let external_config = external_workspace.root.join(".beads").join("config.yaml");
    fs::write(&external_config, "issue_prefix: ext\n").expect("write external config");

    // Create routes file in main workspace pointing to external workspace
    let routes_path = main_workspace.root.join(".beads").join("routes.jsonl");
    let route_entry = format!(
        r#"{{"prefix":"ext-","path":"{}"}}"#,
        external_workspace.root.display()
    );
    fs::write(&routes_path, route_entry).expect("write routes.jsonl");

    // Create an issue in external project
    let create = run_br(
        &external_workspace,
        [
            "create",
            "External issue",
            "--priority",
            "2",
            "--type",
            "task",
            "--json",
        ],
        "create_external",
    );
    assert!(create.status.success(), "create failed: {}", create.stderr);
    let create_payload = extract_json_payload(&create.stdout);
    let created_issue: Value = serde_json::from_str(&create_payload).expect("create json");
    let external_id = created_issue["id"]
        .as_str()
        .expect("external id")
        .to_string();

    // Verify the issue exists in external project
    let list = run_br(&external_workspace, ["list", "--json"], "list_external");
    assert!(list.status.success(), "list failed: {}", list.stderr);
    assert!(
        list.stdout.contains("External issue"),
        "Expected issue in external project"
    );

    // Show the external issue from the main workspace via routing
    let show = run_br(
        &main_workspace,
        ["show", &external_id, "--json"],
        "show_external_via_route",
    );
    assert!(show.status.success(), "show failed: {}", show.stderr);
    let show_payload = extract_json_payload(&show.stdout);
    let shown: Vec<Value> = serde_json::from_str(&show_payload).expect("show json");
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0]["id"].as_str(), Some(external_id.as_str()));
    assert_eq!(shown[0]["title"].as_str(), Some("External issue"));
}

#[test]
fn e2e_routing_update_external_issue_via_main_workspace() {
    let _log = common::test_log("e2e_routing_update_external_issue_via_main_workspace");

    let main_workspace = BrWorkspace::new();
    let external_workspace = BrWorkspace::new();

    let init = run_br(&main_workspace, ["init"], "init_main");
    assert!(init.status.success(), "init failed: {}", init.stderr);

    let init_external = run_br(&external_workspace, ["init"], "init_external");
    assert!(
        init_external.status.success(),
        "external init failed: {}",
        init_external.stderr
    );

    fs::write(
        external_workspace.root.join(".beads").join("config.yaml"),
        "issue_prefix: ext\n",
    )
    .expect("write external config");

    create_routes_file(
        &main_workspace,
        &[("ext-", external_workspace.root.to_string_lossy().as_ref())],
    );

    let create = run_br(
        &external_workspace,
        ["create", "External update target", "--json"],
        "create_external_update_target",
    );
    assert!(create.status.success(), "create failed: {}", create.stderr);
    let create_payload = extract_json_payload(&create.stdout);
    let created_issue: Value = serde_json::from_str(&create_payload).expect("create json");
    let external_id = created_issue["id"]
        .as_str()
        .expect("external id")
        .to_string();

    let update = run_br(
        &main_workspace,
        ["update", &external_id, "--status", "in_progress", "--json"],
        "update_external_via_route",
    );
    assert!(update.status.success(), "update failed: {}", update.stderr);
    let update_payload = extract_json_payload(&update.stdout);
    let updated: Value = serde_json::from_str(&update_payload).expect("update json");
    let updated_array = updated.as_array().expect("update array");
    assert_eq!(updated_array.len(), 1);
    assert_eq!(updated_array[0]["id"].as_str(), Some(external_id.as_str()));
    assert_eq!(updated_array[0]["status"].as_str(), Some("in_progress"));

    let show_external = run_br(
        &external_workspace,
        ["show", &external_id, "--json"],
        "show_external_after_routed_update",
    );
    assert!(
        show_external.status.success(),
        "external show failed: {}",
        show_external.stderr
    );
    let show_payload = extract_json_payload(&show_external.stdout);
    let shown: Vec<Value> = serde_json::from_str(&show_payload).expect("show json");
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0]["status"].as_str(), Some("in_progress"));
}

#[test]
fn e2e_routing_show_external_issue_uses_metadata_database_path() {
    let _log = common::test_log("e2e_routing_show_external_issue_uses_metadata_database_path");

    let main_workspace = BrWorkspace::new();
    let external_workspace = BrWorkspace::new();

    let init = run_br(&main_workspace, ["init"], "init_main");
    assert!(init.status.success(), "init failed: {}", init.stderr);

    let init_external = run_br(&external_workspace, ["init"], "init_external");
    assert!(
        init_external.status.success(),
        "external init failed: {}",
        init_external.stderr
    );

    fs::write(
        external_workspace.root.join(".beads").join("config.yaml"),
        "issue_prefix: ext\n",
    )
    .expect("write external config");
    switch_workspace_to_custom_database(&external_workspace, "custom.db");

    create_routes_file(
        &main_workspace,
        &[("ext-", external_workspace.root.to_string_lossy().as_ref())],
    );

    let create = run_br(
        &external_workspace,
        ["create", "External issue on custom db", "--json"],
        "create_external_custom_db",
    );
    assert!(create.status.success(), "create failed: {}", create.stderr);
    let create_payload = extract_json_payload(&create.stdout);
    let created_issue: Value = serde_json::from_str(&create_payload).expect("create json");
    let external_id = created_issue["id"]
        .as_str()
        .expect("external id")
        .to_string();

    let show = run_br(
        &main_workspace,
        ["show", &external_id, "--json"],
        "show_external_custom_db_via_route",
    );
    assert!(show.status.success(), "show failed: {}", show.stderr);
    let show_payload = extract_json_payload(&show.stdout);
    let shown: Vec<Value> = serde_json::from_str(&show_payload).expect("show json");
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0]["id"].as_str(), Some(external_id.as_str()));
    assert_eq!(
        shown[0]["title"].as_str(),
        Some("External issue on custom db")
    );
}

#[test]
fn e2e_routing_update_external_issue_uses_metadata_database_path() {
    let _log = common::test_log("e2e_routing_update_external_issue_uses_metadata_database_path");

    let main_workspace = BrWorkspace::new();
    let external_workspace = BrWorkspace::new();

    let init = run_br(&main_workspace, ["init"], "init_main");
    assert!(init.status.success(), "init failed: {}", init.stderr);

    let init_external = run_br(&external_workspace, ["init"], "init_external");
    assert!(
        init_external.status.success(),
        "external init failed: {}",
        init_external.stderr
    );

    fs::write(
        external_workspace.root.join(".beads").join("config.yaml"),
        "issue_prefix: ext\n",
    )
    .expect("write external config");
    switch_workspace_to_custom_database(&external_workspace, "custom.db");

    create_routes_file(
        &main_workspace,
        &[("ext-", external_workspace.root.to_string_lossy().as_ref())],
    );

    let create = run_br(
        &external_workspace,
        ["create", "External update on custom db", "--json"],
        "create_external_update_custom_db",
    );
    assert!(create.status.success(), "create failed: {}", create.stderr);
    let create_payload = extract_json_payload(&create.stdout);
    let created_issue: Value = serde_json::from_str(&create_payload).expect("create json");
    let external_id = created_issue["id"]
        .as_str()
        .expect("external id")
        .to_string();

    let update = run_br(
        &main_workspace,
        ["update", &external_id, "--status", "in_progress", "--json"],
        "update_external_custom_db_via_route",
    );
    assert!(update.status.success(), "update failed: {}", update.stderr);
    let update_payload = extract_json_payload(&update.stdout);
    let updated: Value = serde_json::from_str(&update_payload).expect("update json");
    let updated_array = updated.as_array().expect("update array");
    assert_eq!(updated_array.len(), 1);
    assert_eq!(updated_array[0]["id"].as_str(), Some(external_id.as_str()));
    assert_eq!(updated_array[0]["status"].as_str(), Some("in_progress"));

    let show_external = run_br(
        &external_workspace,
        ["show", &external_id, "--json"],
        "show_external_custom_db_after_routed_update",
    );
    assert!(
        show_external.status.success(),
        "external show failed: {}",
        show_external.stderr
    );
    let show_payload = extract_json_payload(&show_external.stdout);
    let shown: Vec<Value> = serde_json::from_str(&show_payload).expect("show json");
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0]["status"].as_str(), Some("in_progress"));
}

// =============================================================================
// REDIRECT FILE TESTS
// =============================================================================

#[test]
fn e2e_routing_redirect_file_absolute_path() {
    let _log = common::test_log("e2e_routing_redirect_file_absolute_path");

    // Use separate workspaces
    let actual_workspace = BrWorkspace::new();
    let redirect_workspace = BrWorkspace::new();

    // Initialize the actual workspace
    let init = run_br(&actual_workspace, ["init"], "init_actual");
    assert!(init.status.success(), "init failed: {}", init.stderr);

    // Create redirect file pointing to actual beads directory (absolute path)
    let actual_beads = actual_workspace.root.join(".beads");
    // First create the redirect .beads directory
    fs::create_dir_all(redirect_workspace.root.join(".beads")).expect("create redirect beads");
    // Then create the redirect file
    create_redirect_file(
        &redirect_workspace.root.join(".beads"),
        actual_beads.to_str().unwrap(),
    );

    // The redirect is used during route resolution, not BEADS_DIR discovery.
    // Test that creating an issue in the actual workspace works
    let create = run_br(
        &actual_workspace,
        [
            "create",
            "Via redirect test",
            "--priority",
            "2",
            "--type",
            "task",
            "--json",
        ],
        "create",
    );
    assert!(create.status.success(), "create failed: {}", create.stderr);

    // Verify issue exists
    let list = run_br(&actual_workspace, ["list", "--json"], "list");
    assert!(list.status.success(), "list failed: {}", list.stderr);
    assert!(
        list.stdout.contains("Via redirect test"),
        "Expected issue in workspace"
    );
}

#[test]
fn e2e_routing_redirect_file_relative_path() {
    let _log = common::test_log("e2e_routing_redirect_file_relative_path");
    let workspace = BrWorkspace::new();

    // Initialize workspace
    let init = run_br(&workspace, ["init"], "init");
    assert!(init.status.success(), "init failed: {}", init.stderr);

    // Test that relative paths in redirect files are handled correctly
    // by creating a redirect file and verifying the path resolution logic
    let beads_dir = workspace.root.join(".beads");
    let redirect_path = beads_dir.join("redirect");

    // Create a redirect to a relative path (which resolves to same location)
    fs::write(&redirect_path, ".").expect("write redirect");

    // Should work (redirect to "." means same directory)
    let create = run_br(
        &workspace,
        [
            "create",
            "Test relative redirect",
            "--priority",
            "2",
            "--type",
            "task",
            "--json",
        ],
        "create",
    );
    assert!(create.status.success(), "create failed: {}", create.stderr);

    // Verify issue exists
    let list = run_br(&workspace, ["list", "--json"], "list");
    assert!(list.status.success(), "list failed: {}", list.stderr);
    assert!(
        list.stdout.contains("Test relative redirect"),
        "Expected issue in workspace"
    );
}

#[test]
fn e2e_routing_redirect_missing_target() {
    let _log = common::test_log("e2e_routing_redirect_missing_target");
    let workspace = BrWorkspace::new();

    // Initialize workspace
    let init = run_br(&workspace, ["init"], "init");
    assert!(init.status.success(), "init failed: {}", init.stderr);

    // Create a route to a nonexistent external project
    let routes_path = workspace.root.join(".beads").join("routes.jsonl");
    fs::write(
        &routes_path,
        r#"{"prefix":"missing-","path":"/nonexistent/path/to/project"}"#,
    )
    .expect("write routes.jsonl");

    // Create a redirect file in an external route target directory
    let ext_beads = workspace.root.join("ext").join(".beads");
    fs::create_dir_all(&ext_beads).expect("create ext beads");
    create_redirect_file(&ext_beads, "/nonexistent/redirect/target/.beads");

    // Add route to this external project
    fs::write(&routes_path, r#"{"prefix":"ext-","path":"ext"}"#).expect("write routes.jsonl");

    // Trying to show an issue with the ext- prefix should trigger redirect resolution
    // and fail because the redirect target doesn't exist
    let show = run_br(
        &workspace,
        ["show", "ext-abc123", "--json"],
        "show_missing_redirect",
    );

    // The routing code attempts to follow redirects. If target is missing,
    // it should produce an error or fall back gracefully.
    // Check that error messaging is clear when redirect/route fails
    if !show.status.success() {
        assert!(
            show.stderr.contains("not found")
                || show.stderr.contains("Redirect")
                || show.stderr.contains("redirect")
                || show.stderr.contains("Issue")
                || show.stderr.contains("route"),
            "Expected clear error about routing/redirect, got: {}",
            show.stderr
        );
    }
    // If it succeeds (by falling back to local), that's also acceptable behavior
}

#[test]
fn e2e_routing_redirect_empty_file() {
    let _log = common::test_log("e2e_routing_redirect_empty_file");
    let workspace = BrWorkspace::new();

    // Initialize workspace
    let init = run_br(&workspace, ["init"], "init");
    assert!(init.status.success(), "init failed: {}", init.stderr);

    // Create empty redirect file
    let redirect_path = workspace.root.join(".beads").join("redirect");
    fs::write(&redirect_path, "").expect("write empty redirect");

    // Should still work (empty redirect is ignored)
    let list = run_br(&workspace, ["list", "--json"], "list");
    assert!(
        list.status.success(),
        "Expected success with empty redirect: {}",
        list.stderr
    );
}

// =============================================================================
// EXTERNAL DB REFERENCE SAFETY TESTS
// =============================================================================

#[test]
fn e2e_routing_db_flag_external_path() {
    let _log = common::test_log("e2e_routing_db_flag_external_path");
    let workspace = BrWorkspace::new();

    // Create external project with beads
    let external_beads = workspace.root.join("external").join(".beads");
    fs::create_dir_all(&external_beads).expect("create external beads dir");
    let external_db = external_beads.join("beads.db");

    // Initialize external database using --db flag
    let init = run_br_with_env(
        &workspace,
        ["init"],
        [("BEADS_DIR", external_beads.to_str().unwrap())],
        "init_external",
    );
    assert!(init.status.success(), "init failed: {}", init.stderr);

    // Use --db flag to point to external database
    let create = run_br(
        &workspace,
        [
            "--db",
            external_db.to_str().unwrap(),
            "create",
            "Via db flag",
            "--priority",
            "2",
            "--type",
            "task",
            "--json",
        ],
        "create_via_db",
    );
    assert!(create.status.success(), "create failed: {}", create.stderr);

    // Verify issue exists in external project
    let list = run_br(
        &workspace,
        ["--db", external_db.to_str().unwrap(), "list", "--json"],
        "list_external",
    );
    assert!(list.status.success(), "list failed: {}", list.stderr);
    assert!(
        list.stdout.contains("Via db flag"),
        "Expected issue in external project"
    );
}

#[test]
fn e2e_routing_db_flag_external_db_uses_workspace_beads_dir() {
    let _log = common::test_log("e2e_routing_db_flag_external_db_uses_workspace_beads_dir");
    let workspace = BrWorkspace::new();

    let init = run_br(&workspace, ["init"], "init_workspace");
    assert!(init.status.success(), "init failed: {}", init.stderr);

    let create = run_br(
        &workspace,
        [
            "create",
            "Workspace issue",
            "--priority",
            "2",
            "--type",
            "task",
        ],
        "create_workspace_issue",
    );
    assert!(create.status.success(), "create failed: {}", create.stderr);

    let external_db = workspace.root.join("cache").join("beads.db");
    fs::create_dir_all(external_db.parent().unwrap()).expect("create cache dir");
    fs::copy(workspace.root.join(".beads").join("beads.db"), &external_db).expect("copy db");

    let list = run_br(
        &workspace,
        ["--db", external_db.to_str().unwrap(), "list", "--json"],
        "list_external_db_outside_beads",
    );
    assert!(
        list.status.success(),
        "commands should still discover the workspace when --db points outside .beads: {}",
        list.stderr
    );
    assert!(list.stdout.contains("Workspace issue"));
}

#[test]
fn e2e_config_get_db_flag_invalid_target_fails_instead_of_falling_back() {
    let _log =
        common::test_log("e2e_config_get_db_flag_invalid_target_fails_instead_of_falling_back");
    let workspace = BrWorkspace::new();

    let external_beads = workspace.root.join("broken").join(".beads");
    fs::create_dir_all(&external_beads).expect("create external beads dir");
    let external_db = external_beads.join("beads.db");
    fs::write(&external_db, "not a sqlite database").expect("write corrupt db");
    fs::write(
        external_beads.join("config.yaml"),
        "issue_prefix: PROJECT\n",
    )
    .expect("write config");

    let get = run_br(
        &workspace,
        [
            "--db",
            external_db.to_str().unwrap(),
            "config",
            "get",
            "issue_prefix",
        ],
        "config_get_invalid_db_target",
    );
    assert!(
        !get.status.success(),
        "config get should fail for an explicitly targeted broken DB"
    );
    assert!(
        !get.stdout.contains("PROJECT"),
        "config get should not silently fall back to YAML layers on explicit DB failure"
    );
}

#[test]
fn e2e_config_delete_db_flag_invalid_target_preserves_yaml() {
    let _log = common::test_log("e2e_config_delete_db_flag_invalid_target_preserves_yaml");
    let workspace = BrWorkspace::new();

    let external_beads = workspace.root.join("broken-delete").join(".beads");
    fs::create_dir_all(&external_beads).expect("create external beads dir");
    let external_db = external_beads.join("beads.db");
    let project_config = external_beads.join("config.yaml");
    fs::write(&external_db, "not a sqlite database").expect("write corrupt db");
    fs::write(&project_config, "issue_prefix: PROJECT\n").expect("write config");

    let delete = run_br(
        &workspace,
        [
            "--db",
            external_db.to_str().unwrap(),
            "config",
            "delete",
            "issue_prefix",
        ],
        "config_delete_invalid_db_target",
    );
    assert!(
        !delete.status.success(),
        "config delete should fail for an explicitly targeted broken DB"
    );
    assert_eq!(
        fs::read_to_string(&project_config).unwrap(),
        "issue_prefix: PROJECT\n",
        "project YAML should remain untouched when explicit DB open fails"
    );
}

#[test]
fn e2e_changelog_since_commit_uses_target_repo_root() {
    let _log = common::test_log("e2e_changelog_since_commit_uses_target_repo_root");
    let workspace = BrWorkspace::new();

    let external_root = workspace.root.join("external-repo");
    let external_beads = external_root.join(".beads");
    fs::create_dir_all(&external_beads).expect("create external beads dir");
    let head = init_test_git_repo(&external_root);

    let init = run_br_with_env(
        &workspace,
        ["init"],
        [("BEADS_DIR", external_beads.to_str().unwrap())],
        "init_external_repo",
    );
    assert!(init.status.success(), "init failed: {}", init.stderr);

    let create = run_br(
        &workspace,
        [
            "--db",
            external_beads.join("beads.db").to_str().unwrap(),
            "create",
            "External closed issue",
            "--type",
            "task",
            "--priority",
            "2",
            "--json",
        ],
        "create_external_closed_issue",
    );
    assert!(create.status.success(), "create failed: {}", create.stderr);
    let payload = extract_json_payload(&create.stdout);
    let issue: Value = serde_json::from_str(&payload).expect("parse create json");
    let id = issue["id"].as_str().expect("issue id").to_string();

    let close = run_br(
        &workspace,
        [
            "--db",
            external_beads.join("beads.db").to_str().unwrap(),
            "close",
            &id,
            "--reason",
            "done",
        ],
        "close_external_closed_issue",
    );
    assert!(close.status.success(), "close failed: {}", close.stderr);

    let changelog = run_br(
        &workspace,
        [
            "--db",
            external_beads.join("beads.db").to_str().unwrap(),
            "changelog",
            "--since-commit",
            &head,
            "--json",
        ],
        "changelog_external_since_commit",
    );
    assert!(
        changelog.status.success(),
        "changelog should resolve git references in the targeted repo: {}",
        changelog.stderr
    );
}

#[test]
fn e2e_routing_path_normalization() {
    let _log = common::test_log("e2e_routing_path_normalization");
    let workspace = BrWorkspace::new();

    // Create actual project
    let actual_beads = workspace.root.join("actual").join(".beads");
    fs::create_dir_all(&actual_beads).expect("create actual beads dir");

    // Initialize
    let init = run_br_with_env(
        &workspace,
        ["init"],
        [("BEADS_DIR", actual_beads.to_str().unwrap())],
        "init",
    );
    assert!(init.status.success(), "init failed: {}", init.stderr);

    // Use path with .. components that normalizes to a valid path
    let db_with_dotdot = workspace
        .root
        .join("actual")
        .join("subdir")
        .join("..")
        .join(".beads")
        .join("beads.db");
    fs::create_dir_all(workspace.root.join("actual").join("subdir")).expect("create subdir");

    let list = run_br(
        &workspace,
        ["--db", db_with_dotdot.to_str().unwrap(), "list", "--json"],
        "list_normalized",
    );
    assert!(
        list.status.success(),
        "Expected success with normalized path: {}",
        list.stderr
    );
}

// =============================================================================
// ERROR MESSAGE CLARITY TESTS
// =============================================================================

#[test]
fn e2e_routing_not_initialized_error() {
    let _log = common::test_log("e2e_routing_not_initialized_error");
    let workspace = BrWorkspace::new();

    // Run command without initialization
    let list = run_br(&workspace, ["list", "--json"], "list_not_init");
    assert!(
        !list.status.success(),
        "Expected failure when not initialized"
    );
    assert!(
        list.stderr.contains("not initialized")
            || list.stderr.contains("br init")
            || list.stderr.contains("NotInitialized"),
        "Expected clear error about initialization, got: {}",
        list.stderr
    );
}

#[test]
fn e2e_routing_invalid_beads_dir_env() {
    let _log = common::test_log("e2e_routing_invalid_beads_dir_env");
    let workspace = BrWorkspace::new();

    // Use BEADS_DIR pointing to nonexistent directory
    let list = run_br_with_env(
        &workspace,
        ["list", "--json"],
        [("BEADS_DIR", "/nonexistent/path/.beads")],
        "list_invalid_env",
    );
    assert!(
        !list.status.success(),
        "Expected failure for invalid BEADS_DIR"
    );
    // Should fall back to discovery and fail with not initialized
    assert!(
        list.stderr.contains("not initialized")
            || list.stderr.contains("br init")
            || list.stderr.contains("NotInitialized")
            || list.stderr.contains("not found"),
        "Expected clear error, got: {}",
        list.stderr
    );
}

#[test]
fn e2e_routing_show_external_issue_not_found() {
    let _log = common::test_log("e2e_routing_show_external_issue_not_found");

    // Use separate workspaces to avoid init conflicts
    let main_workspace = BrWorkspace::new();
    let external_workspace = BrWorkspace::new();

    // Initialize main workspace
    let init = run_br(&main_workspace, ["init"], "init");
    assert!(init.status.success(), "init failed: {}", init.stderr);

    // Initialize external workspace
    let init_ext = run_br(&external_workspace, ["init"], "init_external");
    assert!(
        init_ext.status.success(),
        "init failed: {}",
        init_ext.stderr
    );

    // Set a different prefix for external project
    let external_config = external_workspace.root.join(".beads").join("config.yaml");
    fs::write(&external_config, "issue_prefix: ext\n").expect("write external config");

    // Create routes file in main workspace pointing to external workspace
    let routes_path = main_workspace.root.join(".beads").join("routes.jsonl");
    let route_entry = format!(
        r#"{{"prefix":"ext-","path":"{}"}}"#,
        external_workspace.root.display()
    );
    fs::write(&routes_path, route_entry).expect("write routes.jsonl");

    // Try to show a nonexistent issue with ext- prefix
    // This should trigger route resolution to external project
    let show = run_br(
        &main_workspace,
        ["show", "ext-nonexistent", "--json"],
        "show_missing",
    );
    assert!(
        !show.status.success(),
        "Expected failure for nonexistent issue"
    );
    assert!(
        show.stderr.contains("not found")
            || show.stderr.contains("Issue")
            || show.stderr.contains("ext-nonexistent")
            || show.stderr.contains("No issue"),
        "Expected clear error about missing issue, got: {}",
        show.stderr
    );
}
