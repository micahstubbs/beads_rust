use assert_cmd::Command;
use tempfile::TempDir;

#[test]
fn test_create_parent_standin() {
    let temp = TempDir::new().unwrap();
    let beads_dir = temp.path().join(".beads");
    
    // Initialize the beads directory
    let mut cmd = Command::cargo_bin("br").unwrap();
    cmd.arg("init")
        .env("BEADS_DIR", &beads_dir)
        .assert()
        .success();

    let file_path = temp.path().join("issues.md");
    std::fs::write(&file_path, r#"
## My Epic
### ID
epic1
### Type
epic

## My Task
### Parent
epic1
"#).unwrap();

    let mut cmd = Command::cargo_bin("br").unwrap();
    cmd.arg("create")
        .arg("-f")
        .arg(&file_path)
        .env("BEADS_DIR", &beads_dir)
        .assert()
        .success();
}
