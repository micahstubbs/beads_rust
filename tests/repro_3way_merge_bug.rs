//! Reproduction of 3-way merge data loss bug.
//!
//! This test demonstrates that changes to labels, dependencies, or comments
//! are lost during a 3-way merge if the "significant" content hash (title, description, etc.)
//! remains unchanged on both sides.

mod common;

use common::cli::{BrWorkspace, run_br};
use std::fs;

fn create_issue_id(workspace: &BrWorkspace, title: &str, label: &str) -> String {
    let create = run_br(workspace, ["--json", "create", title, "-t", "task"], label);
    assert!(create.status.success(), "create failed: {}", create.stderr);

    let created_issue: serde_json::Value =
        serde_json::from_str(&create.stdout).expect("parse create json");
    created_issue["id"]
        .as_str()
        .expect("create json should include issue id")
        .to_string()
}

#[test]
fn repro_3way_merge_data_loss() {
    let workspace = BrWorkspace::new();

    // 1. Initialize beads
    let init = run_br(&workspace, ["init"], "init");
    assert!(init.status.success(), "init failed");

    // 2. Create an issue
    let issue_id = create_issue_id(&workspace, "Test issue", "create");

    // 3. Sync to JSONL (creates base snapshot)
    let sync1 = run_br(&workspace, ["sync", "--flush-only"], "sync1");
    assert!(sync1.status.success(), "sync1 failed");

    // 4. Modify labels LOCALLY (Left side of merge)
    let label_local = run_br(
        &workspace,
        ["label", "add", &issue_id, "local-tag"],
        "label_local",
    );
    assert!(
        label_local.status.success(),
        "label_local failed: {}",
        label_local.stderr
    );

    // 5. Modify description EXTERNALLY (Right side of merge)
    // We simulate this by directly editing the JSONL.
    let jsonl_path = workspace.root.join(".beads").join("issues.jsonl");
    let jsonl_content = fs::read_to_string(&jsonl_path).expect("read jsonl");
    let mut issue: serde_json::Value =
        serde_json::from_str(jsonl_content.trim()).expect("parse jsonl");

    // Change description - this WILL change the content_hash
    issue["description"] = serde_json::Value::String("External description".to_string());

    let modified_jsonl = serde_json::to_string(&issue).expect("serialize modified issue");
    fs::write(&jsonl_path, format!("{}\n", modified_jsonl)).expect("write modified jsonl");

    // 6. Run 3-way merge
    // At this point:
    // Base: labels=[], desc=""
    // Left (DB): labels=["local-tag"], desc=""  -> Hash matches Base! (labels excluded from hash)
    // Right (JSONL): labels=[], desc="External description" -> Hash differs from Base.

    // In the CURRENT implementation of merge_issue:
    // left_changed = (l.hash != b.hash) = (H1 != H1) = false
    // right_changed = (r.hash != b.hash) = (H2 != H1) = true
    // (false, true) => Keep(r)
    // Result: Issue has desc="External description" but labels=[] (LOCAL TAG LOST!)

    let merge = run_br(&workspace, ["sync", "--merge", "--force"], "merge");
    assert!(merge.status.success(), "merge failed: {}", merge.stderr);

    // 7. Verify result
    let show = run_br(&workspace, ["show", &issue_id, "--json"], "show");
    assert!(show.status.success(), "show failed");
    let final_issue_list: serde_json::Value =
        serde_json::from_str(&show.stdout).expect("parse final issue list");
    let final_issue = &final_issue_list[0];

    let labels = final_issue["labels"]
        .as_array()
        .expect("labels should be array");
    let has_local_tag = labels.iter().any(|v| v.as_str() == Some("local-tag"));

    assert!(
        has_local_tag,
        "DATA LOSS: Local tag 'local-tag' was lost during 3-way merge!\n\
         Final labels: {:?}\n\
         Final description: {}",
        labels, final_issue["description"]
    );
}

#[test]
fn repro_merge_tolerates_base_only_deleted_issue_absent_from_db() {
    let workspace = BrWorkspace::new();

    let init = run_br(&workspace, ["init"], "init");
    assert!(init.status.success(), "init failed: {}", init.stderr);

    let issue_id = create_issue_id(&workspace, "Current issue", "create");

    let flush = run_br(&workspace, ["sync", "--flush-only"], "flush");
    assert!(flush.status.success(), "flush failed: {}", flush.stderr);

    let jsonl_path = workspace.root.join(".beads").join("issues.jsonl");
    let issue_json = fs::read_to_string(&jsonl_path).expect("read issues jsonl");
    let mut phantom_issue: serde_json::Value =
        serde_json::from_str(issue_json.trim()).expect("parse current issue json");
    phantom_issue["id"] = serde_json::Value::String("second-9u2".to_string());
    phantom_issue["external_ref"] = serde_json::Value::Null;

    let base_snapshot_path = workspace.root.join(".beads").join("beads.base.jsonl");
    fs::write(
        &base_snapshot_path,
        format!(
            "{}\n",
            serde_json::to_string(&phantom_issue).expect("serialize phantom issue")
        ),
    )
    .expect("write base snapshot");

    let merge = run_br(&workspace, ["sync", "--merge"], "merge");
    assert!(merge.status.success(), "merge failed: {}", merge.stderr);

    let show = run_br(&workspace, ["show", &issue_id, "--json"], "show");
    assert!(show.status.success(), "show failed: {}", show.stderr);

    let final_issue_list: serde_json::Value =
        serde_json::from_str(&show.stdout).expect("parse final issue list");
    let final_issue = &final_issue_list[0];
    assert_eq!(final_issue["id"].as_str(), Some(issue_id.as_str()));

    let merged_jsonl = fs::read_to_string(&jsonl_path).expect("read merged jsonl");
    assert!(
        !merged_jsonl.contains("second-9u2"),
        "phantom base-only issue should not survive merge"
    );
}
