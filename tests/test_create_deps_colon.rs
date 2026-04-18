use assert_cmd::Command;
use tempfile::TempDir;

#[test]
fn test_create_deps_colon_title() {
    let temp = TempDir::new().unwrap();
    let beads_dir = temp.path().join(".beads");
    
    // Initialize the beads directory
    let mut cmd = Command::cargo_bin("br").unwrap();
    cmd.arg("init")
        .env("BEADS_DIR", &beads_dir)
        .assert()
        .success();

    let mut cmd = Command::cargo_bin("br").unwrap();
    cmd.arg("create")
        .arg("Task: With colon")
        .env("BEADS_DIR", &beads_dir)
        .assert()
        .success();

    let file_path = temp.path().join("issues.md");
    std::fs::write(&file_path, r#"
## My Task
[depends_on: Task: With colon]
"#).unwrap();

    let mut cmd = Command::cargo_bin("br").unwrap();
    cmd.arg("create")
        .arg("-f")
        .arg(&file_path)
        .env("BEADS_DIR", &beads_dir)
        .assert()
        .success();

    // Use br search to find the ID of My Task
    let mut cmd = Command::cargo_bin("br").unwrap();
    let output = cmd.arg("search")
        .arg("My Task")
        .arg("--json")
        .env("BEADS_DIR", &beads_dir)
        .output()
        .unwrap();
    
    println!("SEARCH OUTPUT:\n{}", String::from_utf8_lossy(&output.stdout));
    
    // Just dump all issues
    let mut cmd = Command::cargo_bin("br").unwrap();
    let output = cmd.arg("list")
        .arg("--json")
        .env("BEADS_DIR", &beads_dir)
        .output()
        .unwrap();
        
    println!("LIST OUTPUT:\n{}", String::from_utf8_lossy(&output.stdout));
}
