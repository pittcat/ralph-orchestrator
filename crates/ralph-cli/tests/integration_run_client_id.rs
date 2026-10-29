//! Integration tests for `ralph run --client-request-id` (Plan5-U1
//! D4): the id lands in the run manifest so the monitor can reconcile
//! launches across crash windows; the same test asserts the additive
//! `actions[]` capability block on `inspect run`.

mod common;

use serde_json::Value;
use std::fs;
use std::process::Output;
use tempfile::TempDir;

fn write_fixture(temp_path: &std::path::Path, home_path: &std::path::Path) {
    fs::create_dir_all(home_path.join(".ralph")).expect("user config dir");
    fs::write(
        home_path.join(".ralph/config.yml"),
        "cli:\n  backend: custom\n",
    )
    .expect("user config");

    let backend = temp_path.join("backend.sh");
    fs::write(
        &backend,
        format!(
            "#!/bin/sh\ncat >/dev/null\n\"{}\" emit LOOP_COMPLETE done\n",
            env!("CARGO_BIN_EXE_ralph")
        ),
    )
    .expect("backend script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = fs::metadata(&backend).unwrap().permissions();
        p.set_mode(0o755);
        fs::set_permissions(&backend, p).unwrap();
    }

    fs::write(
        temp_path.join("ralph.yml"),
        r#"
cli:
  backend: custom
  command: "./backend.sh"
  prompt_mode: stdin
event_loop:
  max_iterations: 3
  max_runtime_seconds: 30
topic_format_whitelist:
  - LOOP_COMPLETE
tasks:
  enabled: false
"#,
    )
    .expect("local config");
}

fn run(temp_path: &std::path::Path, home: &std::path::Path, args: &[&str]) -> Output {
    common::ralph_bin()
        .args(args)
        .current_dir(temp_path)
        .env("HOME", home)
        .output()
        .expect("ralph")
}

fn manifests(temp_path: &std::path::Path) -> Vec<Value> {
    let dir = temp_path.join(".ralph/runs");
    let mut out = Vec::new();
    for e in fs::read_dir(&dir).expect("runs dir") {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "json")
            && p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with(".manifest.json")
        {
            out.push(serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap());
        }
    }
    out
}

#[test]
fn test_client_request_id_lands_in_manifest_and_actions_present() {
    let temp = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    write_fixture(temp.path(), home.path());

    let out = run(
        temp.path(),
        home.path(),
        &[
            "run",
            "--no-tui",
            "-p",
            "x",
            "--client-request-id",
            "mon-req-0001",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let ms = manifests(temp.path());
    assert_eq!(ms.len(), 1);
    assert_eq!(ms[0]["client_request_id"], "mon-req-0001");

    // inspect run declares actions[] for the monitor menu.
    let inspect = run(
        temp.path(),
        home.path(),
        &["inspect", "run", "--format", "json"],
    );
    assert!(inspect.status.success());
    let json: Value = serde_json::from_slice(&inspect.stdout).unwrap();
    let actions = json["runs"][0]["actions"].as_array().expect("actions[]");
    let ids: Vec<&str> = actions.iter().map(|a| a["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["stop", "retry", "merge", "emit"]);
    // The run is exited: stop unavailable with a reason; revisions
    // present on every decl.
    let stop = &actions[0];
    assert_eq!(stop["available"], false);
    assert!(stop["reason"].is_string());
    for a in actions {
        assert_eq!(a["revision"].as_str().unwrap().len(), 64);
    }
    let emit = actions.iter().find(|a| a["id"] == "emit").unwrap();
    assert_eq!(
        emit["input_schema"]["topics"],
        serde_json::json!(["loop.cancel", "task.resume"])
    );
}

#[test]
fn test_client_request_id_rejects_bad_shape() {
    let temp = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    write_fixture(temp.path(), home.path());

    for bad in ["short", "has spaces!!", &"x".repeat(65)] {
        let out = run(
            temp.path(),
            home.path(),
            &["run", "--no-tui", "-p", "x", "--client-request-id", bad],
        );
        assert!(!out.status.success(), "expected rejection for {bad:?}");
    }
}
