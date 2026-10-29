mod common;

#[cfg(unix)]
mod web_monitor_integration {
    use std::fs;
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn write_executable(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        let script = format!("#!/bin/sh\n{}\n", body);
        fs::write(&path, script).expect("write script");
        let mut perms = fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod");
        path
    }

    /// `ralph web --monitor` spawns only the monitor process with the
    /// dashboard port/static-dir env and prints the Dashboard URL once
    /// the monitor reports "listening". Fake npm makes the spawned
    /// `run start` exit shortly after announcing readiness, which lets
    /// the launcher observe the child-exit path and terminate cleanly.
    #[test]
    fn test_web_monitor_spawns_single_process_and_prints_url() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path();

        let monitor_dir = workspace.join("backend/ralph-monitor");
        fs::create_dir_all(&monitor_dir).expect("monitor dir");
        fs::write(monitor_dir.join("package.json"), "{}\n").expect("pkg");

        let bin_dir = workspace.join("bin");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        write_executable(
            &bin_dir,
            "npm",
            "if [ \"$1\" = \"--version\" ]; then echo 9.6.0; exit 0; fi\n\
if [ \"$1\" = \"run\" ] && [ \"$2\" = \"start\" ]; then\n\
  env | grep -E '^RALPH_MONITOR_(PORT|STATIC_DIR)=' >&2\n\
  echo \"ralph-monitor listening on http://127.0.0.1:$RALPH_MONITOR_PORT\"\n\
  sleep 0.1\n\
  exit 0\n\
fi\n\
exit 1",
        );

        let original_path = std::env::var("PATH").unwrap_or_default();
        let path = format!("{}:{}", bin_dir.display(), original_path);

        let port = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(l) => {
                let p = l.local_addr().expect("addr").port();
                drop(l);
                p
            }
            Err(_) => return,
        };

        let output = crate::common::ralph_bin()
            .current_dir(workspace)
            .env("PATH", &path)
            .args([
                "web",
                "--workspace",
                workspace.to_str().expect("workspace"),
                "--monitor",
                "--no-open",
                "--backend-port",
                &port.to_string(),
            ])
            .output()
            .expect("execute ralph web --monitor");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "ralph web --monitor failed: {}\n{}",
            stdout,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout.contains("Starting Ralph monitor"), "{stdout}");
        assert!(
            stdout.contains(&format!("Dashboard: http://localhost:{port}")),
            "{stdout}"
        );
        assert!(stdout.contains("Monitor exited"), "{stdout}");
        // The monitor process must receive the port + static dir env.
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("RALPH_MONITOR_PORT={port}")),
            "{stderr}"
        );
        assert!(stderr.contains("RALPH_MONITOR_STATIC_DIR="), "{stderr}");
    }

    /// Bare `ralph web` defaults to monitor mode — the `--monitor`
    /// flag is kept only for backwards compatibility.
    #[test]
    fn test_web_defaults_to_monitor() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path();
        let monitor_dir = workspace.join("backend/ralph-monitor");
        fs::create_dir_all(&monitor_dir).expect("monitor dir");
        fs::write(monitor_dir.join("package.json"), "{}\n").expect("pkg");

        let bin_dir = workspace.join("bin");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        write_executable(
            &bin_dir,
            "npm",
            "if [ \"$1\" = \"--version\" ]; then echo 9.6.0; exit 0; fi\n\
if [ \"$1\" = \"run\" ] && [ \"$2\" = \"start\" ]; then\n\
  echo \"listening\"\n\
  exit 0\n\
fi\n\
exit 1",
        );
        let original_path = std::env::var("PATH").unwrap_or_default();
        let path = format!("{}:{}", bin_dir.display(), original_path);

        let port = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(l) => {
                let p = l.local_addr().expect("addr").port();
                drop(l);
                p
            }
            Err(_) => return,
        };

        let output = crate::common::ralph_bin()
            .current_dir(workspace)
            .env("PATH", &path)
            .args([
                "web",
                "--workspace",
                workspace.to_str().expect("workspace"),
                "--no-open",
                "--backend-port",
                &port.to_string(),
            ])
            .output()
            .expect("execute ralph web");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{stdout}");
        assert!(stdout.contains("Starting Ralph monitor"), "{stdout}");
    }

    /// `--frontend-port` is meaningless in monitor mode — a notice is
    /// printed and startup proceeds.
    #[test]
    fn test_web_monitor_ignores_frontend_port() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path();
        let monitor_dir = workspace.join("backend/ralph-monitor");
        fs::create_dir_all(&monitor_dir).expect("monitor dir");
        fs::write(monitor_dir.join("package.json"), "{}\n").expect("pkg");

        let bin_dir = workspace.join("bin");
        fs::create_dir_all(&bin_dir).expect("bin dir");
        write_executable(
            &bin_dir,
            "npm",
            "if [ \"$1\" = \"--version\" ]; then echo 9.6.0; exit 0; fi\n\
if [ \"$1\" = \"run\" ] && [ \"$2\" = \"start\" ]; then\n\
  echo \"listening\"\n\
  exit 0\n\
fi\n\
exit 1",
        );
        let original_path = std::env::var("PATH").unwrap_or_default();
        let path = format!("{}:{}", bin_dir.display(), original_path);

        let port = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(l) => {
                let p = l.local_addr().expect("addr").port();
                drop(l);
                p
            }
            Err(_) => return,
        };

        let output = crate::common::ralph_bin()
            .current_dir(workspace)
            .env("PATH", &path)
            .args([
                "web",
                "--workspace",
                workspace.to_str().expect("workspace"),
                "--monitor",
                "--no-open",
                "--backend-port",
                &port.to_string(),
                "--frontend-port",
                "9999",
            ])
            .output()
            .expect("execute ralph web --monitor");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{stdout}");
        assert!(
            stdout.contains("--frontend-port is ignored in monitor mode"),
            "{stdout}"
        );
    }
}
