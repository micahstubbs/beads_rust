mod common;
use common::cli::{BrWorkspace, extract_json_payload, run_br};
use std::fs;

#[test]
fn test_markdown_import() {
    let workspace = BrWorkspace::new();

    // Initialize
    let output = run_br(&workspace, ["init"], "init");
    assert!(output.status.success(), "init failed");

    // Create markdown file
    let md_path = workspace.root.join("issues.md");
    // We use content_safe below. The logic validation of dependencies is commented out
    // because we can't easily refer to new issue IDs in markdown import without placeholders.

    let content_safe = r"## First Issue
### Priority
1
### Labels
bug, frontend

## Second Issue
Implicit description here.

### Type
feature
";

    fs::write(&md_path, content_safe).expect("write md");

    // Run create --file
    let output = run_br(&workspace, ["create", "--file", "issues.md"], "create_md");
    println!("stdout:\n{}", output.stdout);
    println!("stderr:\n{}", output.stderr);
    assert!(output.status.success(), "create --file failed");

    assert!(output.stdout.contains("✓ Created 2 issues from issues.md:"));
    assert!(
        output
            .stdout
            .lines()
            .any(|line| line.starts_with("  ") && line.contains(": First Issue")),
        "expected indented created-issue line in stdout: {}",
        output.stdout
    );

    // Verify list
    let output = run_br(&workspace, ["list"], "list");
    assert!(output.status.success());
    assert!(output.stdout.contains("First Issue"));
    assert!(output.stdout.contains("Second Issue"));
    assert!(output.stdout.contains("P1]")); // Priority 1 (format: [● P1])

    // Verify labels on First Issue using JSON output
    let output = run_br(&workspace, ["list", "--json"], "list_json");
    assert!(output.status.success());

    assert!(output.stdout.contains(r#""title": "First Issue"#));
    assert!(output.stdout.contains(r#""labels": ["#));
    assert!(output.stdout.contains(r#""bug"#));
    assert!(output.stdout.contains(r#""frontend"#));
}

#[test]
fn test_markdown_import_json_output() {
    let workspace = BrWorkspace::new();

    let output = run_br(&workspace, ["init"], "init_json");
    assert!(output.status.success(), "init failed");

    let md_path = workspace.root.join("issues.md");
    let content = r"## One
### Type
task

## Two
### Type
bug
";
    fs::write(&md_path, content).expect("write md");

    let output = run_br(
        &workspace,
        ["create", "--file", "issues.md", "--json"],
        "create_json",
    );
    assert!(output.status.success(), "create --file --json failed");

    let payload = extract_json_payload(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&payload).expect("json parse");
    let array = json.as_array().expect("json array");
    assert_eq!(array.len(), 2);
    assert!(payload.contains("\"One\""));
    assert!(payload.contains("\"Two\""));
}

#[test]
fn test_markdown_import_implicit_description_keeps_first_non_empty_line_only() {
    let workspace = BrWorkspace::new();

    let output = run_br(&workspace, ["init"], "init_implicit_description");
    assert!(output.status.success(), "init failed");

    let md_path = workspace.root.join("issues.md");
    let content = r"## Implicit Description Issue
First line becomes description
This line should be ignored

### Type
task
";
    fs::write(&md_path, content).expect("write md");

    let output = run_br(
        &workspace,
        ["create", "--file", "issues.md", "--json"],
        "create_implicit_description_json",
    );
    assert!(output.status.success(), "create --file --json failed");

    let payload = extract_json_payload(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&payload).expect("json parse");
    let issues = json.as_array().expect("json array");
    assert_eq!(issues.len(), 1);
    assert_eq!(
        issues[0]["description"].as_str(),
        Some("First line becomes description")
    );
}

#[test]
fn test_markdown_import_rejects_dry_run() {
    let workspace = BrWorkspace::new();

    let output = run_br(&workspace, ["init"], "init_dry_run");
    assert!(output.status.success(), "init failed");

    let md_path = workspace.root.join("issues.md");
    let content = r"## DryRun Issue
### Type
task
";
    fs::write(&md_path, content).expect("write md");

    let output = run_br(
        &workspace,
        ["create", "--file", "issues.md", "--dry-run"],
        "create_dry_run",
    );
    assert!(!output.status.success(), "dry-run should fail with --file");
    assert!(
        output
            .stderr
            .contains("--dry-run is not supported with --file")
    );
}

#[test]
fn test_markdown_import_rejects_title_argument() {
    let workspace = BrWorkspace::new();

    let output = run_br(&workspace, ["init"], "init_title_arg");
    assert!(output.status.success(), "init failed");

    let md_path = workspace.root.join("issues.md");
    let content = r"## Bulk Issue
### Type
task
";
    fs::write(&md_path, content).expect("write md");

    let output = run_br(
        &workspace,
        ["create", "SingleTitle", "--file", "issues.md"],
        "create_title_arg",
    );
    assert!(
        !output.status.success(),
        "title argument should fail with --file"
    );
    assert!(
        output
            .stderr
            .contains("cannot be combined with title arguments")
    );
}

#[test]
fn test_markdown_import_rejects_parent_argument() {
    let workspace = BrWorkspace::new();

    let output = run_br(&workspace, ["init"], "init_parent_arg");
    assert!(output.status.success(), "init failed");

    let parent = run_br(&workspace, ["create", "Parent issue"], "create_parent");
    assert!(
        parent.status.success(),
        "create parent failed: {}",
        parent.stderr
    );

    let parent_id = parent
        .stdout
        .lines()
        .next()
        .unwrap_or("")
        .strip_prefix("✓ Created ")
        .and_then(|rest| rest.split(':').next())
        .unwrap_or("")
        .trim()
        .to_string();
    assert!(!parent_id.is_empty(), "expected parent issue id");

    let md_path = workspace.root.join("issues.md");
    let content = r"## Child from import
### Type
task
";
    fs::write(&md_path, content).expect("write md");

    let output = run_br(
        &workspace,
        ["create", "--file", "issues.md", "--parent", &parent_id],
        "create_parent_arg",
    );
    assert!(!output.status.success(), "--parent should fail with --file");
    assert!(
        output
            .stderr
            .contains("--parent is not supported with --file")
    );
}

#[test]
fn test_markdown_import_rejects_external_ref_argument() {
    let workspace = BrWorkspace::new();

    let output = run_br(&workspace, ["init"], "init_external_ref_arg");
    assert!(output.status.success(), "init failed");

    let md_path = workspace.root.join("issues.md");
    let content = r"## Imported issue
### Type
task
";
    fs::write(&md_path, content).expect("write md");

    let output = run_br(
        &workspace,
        [
            "create",
            "--file",
            "issues.md",
            "--external-ref",
            "JIRA-123",
        ],
        "create_external_ref_arg",
    );
    assert!(
        !output.status.success(),
        "--external-ref should fail with --file"
    );
    assert!(
        output
            .stderr
            .contains("--external-ref is not supported with --file")
    );
}

#[test]
fn test_markdown_import_rejects_non_empty_file_without_issue_headers() {
    let workspace = BrWorkspace::new();

    let output = run_br(&workspace, ["init"], "init_no_headers");
    assert!(output.status.success(), "init failed");

    let md_path = workspace.root.join("issues.md");
    let content = r"### Description
This file has content but no issue headers.
";
    fs::write(&md_path, content).expect("write md");

    let output = run_br(
        &workspace,
        ["create", "--file", "issues.md"],
        "create_no_headers",
    );
    assert!(
        !output.status.success(),
        "non-empty file without issue headers should fail"
    );
    assert!(
        output
            .stderr
            .contains("no issues found; expected '## Title' headers")
    );
}

#[test]
fn test_markdown_import_dependency_bullets_do_not_create_marker_dependency() {
    let workspace = BrWorkspace::new();

    let init = run_br(&workspace, ["init"], "init_bullet_deps");
    assert!(init.status.success(), "init failed");

    let blocker = run_br(
        &workspace,
        ["create", "Blocker for markdown import", "--json"],
        "create_blocker_json",
    );
    assert!(
        blocker.status.success(),
        "create blocker failed: {}",
        blocker.stderr
    );
    let blocker_payload = extract_json_payload(&blocker.stdout);
    let blocker_json: serde_json::Value =
        serde_json::from_str(&blocker_payload).expect("blocker json");
    let blocker_id = blocker_json["id"].as_str().expect("blocker id").to_string();

    let md_path = workspace.root.join("issues.md");
    let content =
        format!("## Imported issue\n### Dependencies\n- {blocker_id}\n- [ ] external:github#123\n");
    fs::write(&md_path, content).expect("write md");

    let output = run_br(
        &workspace,
        ["create", "--file", "issues.md", "--json"],
        "create_bullet_deps_json",
    );
    assert!(
        output.status.success(),
        "create --file --json failed: {}",
        output.stderr
    );

    let payload = extract_json_payload(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&payload).expect("json parse");
    let issues = json.as_array().expect("json array");
    assert_eq!(issues.len(), 1);

    let dependencies = issues[0]["dependencies"]
        .as_array()
        .expect("dependencies array");
    assert_eq!(dependencies.len(), 2);
    assert!(
        dependencies
            .iter()
            .any(|dep| dep["depends_on_id"].as_str() == Some(blocker_id.as_str()))
    );
    assert!(
        dependencies
            .iter()
            .any(|dep| dep["depends_on_id"].as_str() == Some("external:github#123"))
    );
    assert!(
        dependencies
            .iter()
            .all(|dep| dep["depends_on_id"].as_str() != Some("-"))
    );
}

#[test]
fn test_markdown_import_invalid_dependency_warns() {
    let workspace = BrWorkspace::new();

    let output = run_br(&workspace, ["init"], "init_invalid_dep");
    assert!(output.status.success(), "init failed");

    let md_path = workspace.root.join("issues.md");
    let content = r"## Issue With Bad Dep
### Dependencies
invalid-type:bd-123
";
    fs::write(&md_path, content).expect("write md");

    let output = run_br(
        &workspace,
        ["create", "--file", "issues.md"],
        "create_bad_dep",
    );
    assert!(
        output.status.success(),
        "create should succeed with warnings"
    );
    assert!(
        output
            .stderr
            .contains("warning: skipping invalid dependency type"),
        "expected warning for invalid dependency type"
    );
}

#[test]
fn test_markdown_import_whitespace_separated_typed_dependencies() {
    let workspace = BrWorkspace::new();

    let output = run_br(&workspace, ["init"], "init_whitespace_typed_deps");
    assert!(output.status.success(), "init failed");

    let blocker = run_br(
        &workspace,
        ["create", "Whitespace dependency blocker", "--json"],
        "create_whitespace_dep_blocker_json",
    );
    assert!(
        blocker.status.success(),
        "create blocker failed: {}",
        blocker.stderr
    );
    let blocker_payload = extract_json_payload(&blocker.stdout);
    let blocker_json: serde_json::Value =
        serde_json::from_str(&blocker_payload).expect("blocker json");
    let blocker_id = blocker_json["id"].as_str().expect("blocker id").to_string();

    let md_path = workspace.root.join("issues.md");
    let content =
        format!("## Imported issue\n### Dependencies\nblocks: {blocker_id} external:github#123\n");
    fs::write(&md_path, content).expect("write md");

    let output = run_br(
        &workspace,
        ["create", "--file", "issues.md", "--json"],
        "create_whitespace_typed_deps_json",
    );
    assert!(
        output.status.success(),
        "create --file --json failed: {}",
        output.stderr
    );

    let payload = extract_json_payload(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&payload).expect("json parse");
    let issues = json.as_array().expect("json array");
    assert_eq!(issues.len(), 1);

    let dependencies = issues[0]["dependencies"]
        .as_array()
        .expect("dependencies array");
    assert_eq!(dependencies.len(), 2);
    assert!(
        dependencies
            .iter()
            .any(|dep| dep["depends_on_id"].as_str() == Some(blocker_id.as_str()))
    );
    assert!(
        dependencies
            .iter()
            .any(|dep| dep["depends_on_id"].as_str() == Some("external:github#123"))
    );
}
