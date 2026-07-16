// U2a: fake-PATH 后端安装 helper + 2 个 `FAKE_PATH_BACKEND_*` private `static` Mutex。
//
// 从原 `loop_runner/tests.rs:593-660` 行迁出。
// - `FAKE_PATH_BACKEND_SERIAL: LazyLock<Mutex<()>>` (#[cfg(unix)], private `static`)
// - `FAKE_PATH_BACKEND_BIN: LazyLock<Mutex<Option<PathBuf>>>` (#[cfg(unix)], private `static`)
//
// Nextest runs each test in its own process, so these fixture guards and
// their temporary backend directory are isolated across tests. The static
// values remain an implementation detail of this test fixture; callers use
// the guard and accessor APIs below.

use std::path::Path;

#[cfg(unix)]
pub(super) fn write_fake_executable(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    let script = format!("#!/bin/sh\n{}\n", body);
    std::fs::write(&path, script).expect("write script");
    let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod");
    path
}

#[cfg(unix)]
static FAKE_PATH_BACKEND_SERIAL: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

#[cfg(unix)]
static FAKE_PATH_BACKEND_BIN: std::sync::LazyLock<std::sync::Mutex<Option<std::path::PathBuf>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

// sibling 模块(legacy / 后续 wave)需要读 bin 目录,走 `read_fake_path_backend_bin()` 访问器
// 间接访问。
#[cfg(unix)]
pub(super) fn read_fake_path_backend_bin() -> Option<std::path::PathBuf> {
    FAKE_PATH_BACKEND_BIN
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
}

#[cfg(unix)]
pub(super) struct FakePathBackendsGuard {
    _guard: std::sync::MutexGuard<'static, ()>,
    _temp_dir: tempfile::TempDir,
    installed_paths: Vec<std::path::PathBuf>,
}

#[cfg(unix)]
impl Drop for FakePathBackendsGuard {
    fn drop(&mut self) {
        for path in &self.installed_paths {
            let _ = std::fs::remove_file(path);
        }
        *FAKE_PATH_BACKEND_BIN
            .lock()
            .expect("fake PATH backend bin lock") = None;
    }
}

#[cfg(unix)]
pub(super) fn install_fake_path_backends(backends: &[(&str, &str)]) -> FakePathBackendsGuard {
    let guard = FAKE_PATH_BACKEND_SERIAL
        .lock()
        .expect("fake PATH backend serial lock");
    let temp_dir = tempfile::tempdir().expect("fake backend temp dir");
    let bin_dir = temp_dir.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("fake backend bin dir");

    let mut installed_paths = Vec::with_capacity(backends.len());
    for (name, body) in backends {
        let path = bin_dir.join(name);
        assert!(
            !path.exists(),
            "expected fake backend slot to be free: {}",
            path.display()
        );
        installed_paths.push(write_fake_executable(&bin_dir, name, body));
    }
    *FAKE_PATH_BACKEND_BIN
        .lock()
        .expect("fake PATH backend bin lock") = Some(bin_dir.clone());

    FakePathBackendsGuard {
        _guard: guard,
        _temp_dir: temp_dir,
        installed_paths,
    }
}
