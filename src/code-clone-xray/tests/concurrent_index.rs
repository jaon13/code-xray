//! Multi-process OS lockfile serialization and CLI integration tests.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

fn make_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("xray_mp_test_{label}_{nanos}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("mkdir");
    dir
}

#[test]
fn multiprocess_xray_index_build_never_hits_sqlite_busy() {
    let root = make_temp_dir("build");
    let proj_dir = root.join("src").join("Demo.Multi");
    fs::create_dir_all(&proj_dir).expect("mkdir");
    fs::write(
        root.join("Demo.slnx"),
        "<Solution><Project Path=\"src/Demo.Multi/Demo.Multi.csproj\" /></Solution>\n",
    )
    .expect("slnx");
    fs::write(
        proj_dir.join("Demo.Multi.csproj"),
        "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><TargetFramework>net10.0</TargetFramework></PropertyGroup></Project>\n",
    )
    .expect("csproj");
    for i in 0..6 {
        fs::write(
            proj_dir.join(format!("Item{i}.cs")),
            format!("namespace Demo.Multi;\npublic sealed class Item{i} {{ public int Val() => {i}; }}\n"),
        )
        .expect("cs");
    }

    let bin = env!("CARGO_BIN_EXE_xray-index");
    let mut children = Vec::new();
    for _ in 0..4 {
        let child = Command::new(bin)
            .args(["build", "-r"])
            .arg(&root)
            .arg("--no-external")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn xray-index build");
        children.push(child);
    }

    let mut total_parsed = 0usize;
    for child in children {
        let output = child.wait_with_output().expect("wait child");
        assert!(
            output.status.success(),
            "xray-index build failed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("json summary");
        assert_eq!(parsed["totalFiles"], 6);
        total_parsed += parsed["parsedFiles"].as_u64().expect("parsedFiles") as usize;
    }

    assert_eq!(total_parsed, 6);
    assert!(root.join(".xray").join("index.build.lock").is_file());
    assert!(root.join(".xray").join("index.db").is_file());

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn xray_grep_and_index_grep_subcommand_work_with_save_review() {
    let root = make_temp_dir("grep_cli");
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).expect("mkdir");
    fs::write(
        src_dir.join("lib.rs"),
        "pub fn greet_user() -> &'static str {\n    \"GREP_MARKER\"\n}\n",
    )
    .expect("rs");

    let grep_bin = env!("CARGO_BIN_EXE_xray-grep");
    let out1 = Command::new(grep_bin)
        .args(["-r"])
        .arg(&root)
        .args(["-p", "GREP_MARKER", "--save-review"])
        .output()
        .expect("run xray-grep");
    assert!(
        out1.status.success(),
        "xray-grep failed: {}",
        String::from_utf8_lossy(&out1.stderr)
    );
    let val1: serde_json::Value = serde_json::from_slice(&out1.stdout).expect("parse json");
    assert_eq!(val1["totalMatches"], 1);
    let manifest_path = PathBuf::from(val1["reviewManifest"].as_str().expect("manifest"));
    assert!(manifest_path.is_file());

    let index_bin = env!("CARGO_BIN_EXE_xray-index");
    let out2 = Command::new(index_bin)
        .args(["grep", "-r"])
        .arg(&root)
        .args(["-p", "GREP_MARKER", "--format", "markdown"])
        .output()
        .expect("run xray-index grep");
    assert!(
        out2.status.success(),
        "xray-index grep failed: {}",
        String::from_utf8_lossy(&out2.stderr)
    );
    let md = String::from_utf8_lossy(&out2.stdout);
    assert!(md.contains("fn greet_user"));

    let _ = fs::remove_dir_all(&root);
}
