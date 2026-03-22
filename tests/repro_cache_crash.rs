mod common;

use beads_rust::model::{Issue, IssueType, Priority, Status};
use beads_rust::storage::SqliteStorage;
use chrono::Utc;
use common::cli::{BrWorkspace, extract_json_payload, run_br};
use std::fs;

fn create_issue_id(workspace: &BrWorkspace, title: &str, label: &str) -> String {
    let create = run_br(workspace, ["--json", "create", title, "-t", "task"], label);
    assert!(create.status.success(), "create failed: {}", create.stderr);

    let created_issue: serde_json::Value =
        serde_json::from_str(&extract_json_payload(&create.stdout)).expect("parse create json");
    created_issue["id"]
        .as_str()
        .expect("create json should include issue id")
        .to_string()
}

fn make_issue(id: &str, title: &str) -> Issue {
    Issue {
        id: id.to_string(),
        title: title.to_string(),
        status: Status::Open,
        priority: Priority::MEDIUM,
        issue_type: IssueType::Task,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        content_hash: None,
        description: None,
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
fn test_rebuild_blocked_cache_crash_with_multiple_parents() {
    let mut storage = SqliteStorage::open_memory().unwrap();

    // Create blockers D and E (open status)
    storage
        .create_issue(&make_issue("bd-D", "Blocker D"), "test")
        .unwrap();
    storage
        .create_issue(&make_issue("bd-E", "Blocker E"), "test")
        .unwrap();

    // Create parents B and C
    storage
        .create_issue(&make_issue("bd-B", "Parent B"), "test")
        .unwrap();
    storage
        .create_issue(&make_issue("bd-C", "Parent C"), "test")
        .unwrap();

    // Create child A
    storage
        .create_issue(&make_issue("bd-A", "Child A"), "test")
        .unwrap();

    // Make B blocked by D
    storage
        .add_dependency("bd-B", "bd-D", "blocks", "test")
        .unwrap();
    storage
        .rebuild_blocked_cache(true)
        .expect("incremental rebuild after B -> D should not crash");

    // Make C blocked by E
    storage
        .add_dependency("bd-C", "bd-E", "blocks", "test")
        .unwrap();
    storage
        .rebuild_blocked_cache(true)
        .expect("incremental rebuild after C -> E should not crash");

    // Make A child of B AND C (diamond dependency / multiple inheritance).
    // This intentionally stress-tests repeated incremental blocked-cache rebuilds
    // around the same mutations that `br dep add` triggers through storage-owned
    // cache invalidation.
    storage
        .add_dependency("bd-A", "bd-B", "parent-child", "test")
        .unwrap();
    storage
        .rebuild_blocked_cache(true)
        .expect("incremental rebuild after A -> B should not crash");
    storage
        .add_dependency("bd-A", "bd-C", "parent-child", "test")
        .unwrap();

    storage
        .rebuild_blocked_cache(true)
        .expect("incremental rebuild after A -> C should not crash");

    assert!(storage.is_blocked("bd-A").unwrap(), "A should be blocked");
    println!("Test finished successfully");
}

#[test]
fn test_rebuild_blocked_cache_is_idempotent_when_rows_already_exist() {
    let mut storage = SqliteStorage::open_memory().unwrap();

    storage
        .create_issue(&make_issue("bd-root", "Root"), "test")
        .unwrap();
    storage
        .create_issue(&make_issue("bd-parent", "Parent"), "test")
        .unwrap();
    storage
        .create_issue(&make_issue("bd-child", "Child"), "test")
        .unwrap();

    storage
        .add_dependency("bd-parent", "bd-root", "blocks", "test")
        .unwrap();
    storage
        .add_dependency("bd-child", "bd-parent", "parent-child", "test")
        .unwrap();

    assert!(storage.is_blocked("bd-parent").unwrap());
    assert!(storage.is_blocked("bd-child").unwrap());

    for _ in 0..64 {
        storage
            .rebuild_blocked_cache(true)
            .expect("rebuilding an already-populated blocked cache must stay idempotent");
    }

    assert!(storage.is_blocked("bd-parent").unwrap());
    assert!(storage.is_blocked("bd-child").unwrap());
}

#[test]
fn repro_dep_add_parent_child_succeeds_db_backed_after_blocked_cache_exists() {
    let workspace = BrWorkspace::new();

    let init = run_br(&workspace, ["init"], "init");
    assert!(init.status.success(), "init failed: {}", init.stderr);

    let root_id = create_issue_id(&workspace, "Root blocker", "create_root");
    let parent_id = create_issue_id(&workspace, "Parent issue", "create_parent");
    let child_id = create_issue_id(&workspace, "Child issue", "create_child");

    let add_blocker = run_br(
        &workspace,
        [
            "dep", "add", &parent_id, &root_id, "--type", "blocks", "--json",
        ],
        "dep_add_blocker_db",
    );
    assert!(
        add_blocker.status.success(),
        "db-backed dep add (blocks) failed: {}",
        add_blocker.stderr
    );

    let add_parent_child = run_br(
        &workspace,
        [
            "dep",
            "add",
            &child_id,
            &parent_id,
            "--type",
            "parent-child",
            "--json",
        ],
        "dep_add_parent_child_db",
    );
    assert!(
        add_parent_child.status.success(),
        "db-backed dep add (parent-child) failed: {}",
        add_parent_child.stderr
    );

    let payload: serde_json::Value =
        serde_json::from_str(&extract_json_payload(&add_parent_child.stdout))
            .expect("parse dep add json");
    assert_eq!(payload["status"], "ok");
    assert_eq!(payload["action"], "added");
}

#[test]
fn repro_dep_add_parent_child_succeeds_no_db_after_blocked_cache_exists() {
    let workspace = BrWorkspace::new();

    let init = run_br(&workspace, ["init"], "init");
    assert!(init.status.success(), "init failed: {}", init.stderr);

    let root_id = create_issue_id(&workspace, "Root blocker", "create_root");
    let parent_id = create_issue_id(&workspace, "Parent issue", "create_parent");
    let child_id = create_issue_id(&workspace, "Child issue", "create_child");

    let flush = run_br(&workspace, ["sync", "--flush-only"], "flush");
    assert!(flush.status.success(), "flush failed: {}", flush.stderr);

    let add_blocker = run_br(
        &workspace,
        [
            "dep", "add", &parent_id, &root_id, "--type", "blocks", "--no-db", "--json",
        ],
        "dep_add_blocker_no_db",
    );
    assert!(
        add_blocker.status.success(),
        "no-db dep add (blocks) failed: {}",
        add_blocker.stderr
    );

    let add_parent_child = run_br(
        &workspace,
        [
            "dep",
            "add",
            &child_id,
            &parent_id,
            "--type",
            "parent-child",
            "--no-db",
            "--json",
        ],
        "dep_add_parent_child_no_db",
    );
    assert!(
        add_parent_child.status.success(),
        "no-db dep add (parent-child) failed: {}",
        add_parent_child.stderr
    );

    let jsonl_path = workspace.root.join(".beads").join("issues.jsonl");
    let child_record = fs::read_to_string(&jsonl_path)
        .expect("read issues.jsonl")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("parse issue json"))
        .find(|record| record["id"].as_str() == Some(child_id.as_str()))
        .expect("child issue record in issues.jsonl");
    let dependencies = child_record["dependencies"]
        .as_array()
        .expect("jsonl issue should include dependencies array");
    assert!(dependencies.iter().any(|dependency| {
        dependency["depends_on_id"].as_str() == Some(parent_id.as_str())
            && dependency["type"].as_str() == Some("parent-child")
    }));
}
