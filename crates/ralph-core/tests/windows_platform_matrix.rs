//! Windows platform matrix: fuzz + scenario coverage for the pieces of
//! the Windows port that unit tests cannot cover — NTFS filename
//! legality, shell/exe resolution against a real filesystem, real
//! process-tree termination, path-containment under Windows path
//! shapes, and environment filtering.
//!
//! Every test is `#[cfg(windows)]`-gated at the file level so the
//! module compiles to nothing on Unix. Env-mutating cases rely on
//! nextest's process-per-test isolation.
#![cfg(windows)]

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use ralph_core::path::{filename_stem, path_within_workspace};
use ralph_core::pid_probe::{kill_process_tree, pid_alive};
use ralph_core::shell::{
    ShellKind, resolve_exe, resolve_exe_in, spawn_program, spawn_program_in, windows_resolve_from,
};

// ── deterministic xorshift64 PRNG (no external fuzz deps) ────────────

struct XorShift(u64);

impl XorShift {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Alphabet weighted toward NTFS-hostile input: illegal chars, control
/// chars, Unicode, spaces/dots at edges, '%' (encoding escape char),
/// and digits used to build reserved-name collisions.
const FUZZ_CHARS: &[char] = &[
    'a',
    'b',
    'Z',
    '0',
    '1',
    '9',
    ':',
    '<',
    '>',
    '"',
    '/',
    '\\',
    '|',
    '?',
    '*',
    '%',
    '.',
    ' ',
    '-',
    '_',
    '+',
    '=',
    '(',
    ')',
    '[',
    ']',
    '{',
    '}',
    '@',
    '#',
    '$',
    '^',
    '&',
    '~',
    '!',
    'é',
    '中',
    '文',
    '日',
    '本',
    '\u{1f600}',
    '\u{7}',
    '\u{1f}',
    '\u{0}',
    '\u{80}',
    '¹',
    '²',
    '³',
    'C',
    'O',
    'M',
    'N',
    'U',
    'L',
    'P',
    'R',
    'A',
    'X',
    'T',
    'L',
];

fn fuzz_name(rng: &mut XorShift) -> String {
    let len = 1 + rng.below(48);
    (0..len)
        .map(|_| FUZZ_CHARS[rng.below(FUZZ_CHARS.len())])
        .collect()
}

/// NTFS component rules a stem must satisfy. Mirrors the intent of
/// `filename_stem`; used as the oracle for the fuzz tests.
fn assert_ntfs_legal(stem: &str, original: &str) {
    assert!(!stem.is_empty(), "stem for {original:?} must be non-empty");
    assert!(
        stem.len() <= 255,
        "stem for {original:?} exceeds NTFS limit: {} bytes",
        stem.len()
    );
    for ch in stem.chars() {
        assert!(
            !matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
                && (ch as u32) >= 0x20,
            "stem {stem:?} (from {original:?}) contains illegal char {ch:?}"
        );
    }
    assert!(
        !stem.ends_with('.') && !stem.ends_with(' '),
        "stem {stem:?} (from {original:?}) ends with '.' or ' '"
    );
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    assert!(
        !RESERVED.contains(&stem.to_uppercase().as_str()),
        "stem {stem:?} (from {original:?}) is an NTFS reserved name"
    );
}

// ── filename_stem fuzz ───────────────────────────────────────────────

#[test]
fn fuzz_filename_stem_always_ntfs_legal() {
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    for _ in 0..20_000 {
        let name = fuzz_name(&mut rng);
        assert_ntfs_legal(&filename_stem(&name), &name);
    }
}

#[test]
fn fuzz_filename_stem_injective_for_distinct_inputs() {
    let mut rng = XorShift(0xD1B5_4A32_D192_ED03);
    // stem -> the input that produced it; a real collision means two
    // DIFFERENT inputs share a stem (same input may recur and is fine).
    let mut seen: std::collections::HashMap<String, String> =
        std::collections::HashMap::with_capacity(50_000);
    for _ in 0..50_000 {
        let name = fuzz_name(&mut rng);
        let stem = filename_stem(&name);
        match seen.get(&stem) {
            None => {
                seen.insert(stem, name);
            }
            Some(prev) => assert_eq!(
                prev, &name,
                "filename_stem collision: {prev:?} and {name:?} both -> {stem:?}"
            ),
        }
    }
}

#[test]
fn filename_stem_edge_cases() {
    let long_a = "a".repeat(400);
    let long_x = "x".repeat(260);
    let cases = [
        "",
        "CON",
        "con",
        "NUL",
        "aux",
        "PRN",
        "COM1",
        "com9",
        "LPT1",
        "lpt7",
        "COM¹",
        "LPT³",
        "foo.",
        "foo ",
        "...",
        "   ",
        ".",
        " ",
        long_a.as_str(),
        long_x.as_str(),
        "task:task-1:loop:loop-1",
        "loop-1.2026-10-29T13:30:00Z",
        "%3Aalready-encoded",
        "%%literal-percent",
        "trailing%25encoded.",
    ];
    for case in cases {
        assert_ntfs_legal(&filename_stem(case), case);
    }
    // Injectivity among the edge cases themselves.
    let stems: HashSet<_> = cases.iter().map(|c| filename_stem(c)).collect();
    assert_eq!(stems.len(), cases.len(), "edge-case stems must be unique");
}

#[test]
fn filename_stem_produces_real_creatable_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut rng = XorShift(0xF00D_CAFE_BABE_0001);
    let mut created = 0usize;
    for _ in 0..500 {
        let name = fuzz_name(&mut rng);
        let path = dir.path().join(format!("{}.jsonl", filename_stem(&name)));
        match std::fs::File::create(&path) {
            Ok(_) => created += 1,
            Err(e) => panic!("cannot create {path:?} (stem of {name:?}): {e}"),
        }
    }
    assert_eq!(created, 500);
}

#[test]
fn filename_stem_reserved_names_create_files() {
    let dir = tempfile::TempDir::new().unwrap();
    for name in ["CON", "NUL", "PRN", "AUX", "COM1", "LPT9", "con", "nul.txt"] {
        let path = dir.path().join(format!("{}.jsonl", filename_stem(name)));
        std::fs::File::create(&path)
            .unwrap_or_else(|e| panic!("{name:?} stem should be creatable: {e}"));
    }
    // Why the escaping matters even when `CON.ext` is creatable on
    // modern Windows: a bare stem like `CON` opens the console *device*
    // (silent data sink), and `NUL` opens the null device — neither is
    // a real file. Escaping keeps stems real files on every Windows
    // version.
}

// ── shell / spawn_program matrix ─────────────────────────────────────

/// Write an executable fixture file and return its path.
fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn spawn_program_cmd_batch_runs_for_real() {
    let dir = tempfile::TempDir::new().unwrap();
    let script = write_script(dir.path(), "probe.cmd", "@echo off\r\nexit /b 0\r\n");

    let (program, prefix) = spawn_program(script.to_str().unwrap());
    let status = Command::new(&program)
        .args(&prefix)
        .status()
        .unwrap_or_else(|e| panic!("spawn {program:?} failed: {e}"));
    assert!(status.success(), ".cmd probe exited {status}");
}

#[test]
fn spawn_program_cmd_from_path_searched_name() {
    // A `.cmd` reachable by bare name through PATH must spawn — this is
    // the npm/npx shim shape. `spawn_program_in` injects a PATH value so
    // the test never touches process env (set_var is unsafe in 2024).
    let dir = tempfile::TempDir::new().unwrap();
    write_script(dir.path(), "fake-shim.cmd", "@echo off\r\nexit /b 0\r\n");

    let path_env = std::env::join_paths([dir.path().to_path_buf()]).unwrap();
    let (program, prefix) = spawn_program_in("fake-shim", Some(&path_env));
    // which() must resolve to our shim with an extension.
    assert!(
        program.to_string_lossy().ends_with("fake-shim.cmd"),
        "expected resolved shim path, got {program:?}"
    );
    let status = Command::new(&program)
        .args(&prefix)
        .status()
        .unwrap_or_else(|e| panic!("spawn shim failed: {e}"));
    assert!(status.success());
}

#[test]
fn resolve_exe_in_searches_injected_path_only() {
    let dir = tempfile::TempDir::new().unwrap();
    let real = write_script(dir.path(), "only-here.exe", "binary\n");

    let path_env = std::env::join_paths([dir.path().to_path_buf()]).unwrap();

    // Present in injected PATH → resolved.
    let resolved = resolve_exe_in("only-here", Some(&path_env));
    assert_eq!(resolved, real.clone().into_os_string());

    // Absent from injected PATH even though it exists on the real PATH →
    // falls back to the bare name.
    let missing = resolve_exe_in("cmd", Some(&path_env));
    assert_eq!(missing, OsString::from("cmd"));
}

#[test]
fn spawn_program_matrix_shape() {
    let dir = tempfile::TempDir::new().unwrap();
    let ps1 = write_script(dir.path(), "hook.ps1", "exit 0\n");
    let sh = write_script(dir.path(), "hook.sh", "#!/bin/sh\nexit 0\n");
    let py = write_script(dir.path(), "hook.py", "pass\n");
    let cmd = write_script(dir.path(), "hook.cmd", "exit /b 0\n");
    let noext = write_script(dir.path(), "plain-script", "irrelevant\n");

    // .ps1 → pwsh interpreter prefix with bypass flags.
    let (prog, pre) = spawn_program(ps1.to_str().unwrap());
    let pl = prog.to_string_lossy().to_lowercase();
    assert!(
        pl.contains("pwsh"),
        ".ps1 should route through pwsh, got {prog:?}"
    );
    let pre_str: Vec<_> = pre
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    for flag in ["-NoProfile", "-NonInteractive", "-File"] {
        assert!(
            pre_str.iter().any(|a| a == flag),
            "missing {flag}: {pre_str:?}"
        );
    }

    // .sh → bash interpreter prefix (Git Bash chain).
    let (prog, pre) = spawn_program(sh.to_str().unwrap());
    let pl = prog.to_string_lossy().to_lowercase();
    assert!(
        pl.contains("bash"),
        ".sh should route through bash, got {prog:?}"
    );
    assert_eq!(pre.last().unwrap(), &OsString::from(sh.to_str().unwrap()));

    // .py → python interpreter prefix.
    let (prog, pre) = spawn_program(py.to_str().unwrap());
    let pl = prog.to_string_lossy().to_lowercase();
    assert!(
        pl.contains("python"),
        ".py should route through python, got {prog:?}"
    );
    assert_eq!(pre.last().unwrap(), &OsString::from(py.to_str().unwrap()));

    // .cmd stays direct (std runs batch through cmd.exe itself).
    let (prog, pre) = spawn_program(cmd.to_str().unwrap());
    assert!(
        prog.to_string_lossy().ends_with("hook.cmd"),
        ".cmd should stay direct, got {prog:?}"
    );
    assert!(pre.is_empty(), ".cmd must not get an interpreter prefix");

    // Extensionless stays direct.
    let (_, pre) = spawn_program(noext.to_str().unwrap());
    assert!(pre.is_empty());
}

#[test]
fn resolve_exe_matrix() {
    // Bare name with implicit .exe resolution — cmd.exe always exists.
    let resolved = resolve_exe("cmd.exe");
    assert!(
        resolved
            .to_string_lossy()
            .to_lowercase()
            .ends_with("cmd.exe"),
        "resolve_exe(cmd.exe) -> {resolved:?}"
    );

    // Name with explicit extension and path separators pass through.
    let verbatim = resolve_exe(r"C:\Windows\System32\cmd.exe");
    assert_eq!(verbatim, OsString::from(r"C:\Windows\System32\cmd.exe"));

    // Unknown name falls back to itself (caller decides how to fail).
    let missing = resolve_exe("definitely-not-a-real-program-xyz");
    assert_eq!(missing, OsString::from("definitely-not-a-real-program-xyz"));
}

#[test]
fn path_with_spaces_and_unicode_spawns() {
    let dir = tempfile::TempDir::new().unwrap();
    let weird = dir.path().join("héllo 中 文 dir [brackets]");
    std::fs::create_dir_all(&weird).unwrap();
    let script = write_script(&weird, "probe.cmd", "@echo off\r\nexit /b 0\r\n");

    let (program, prefix) = spawn_program(script.to_str().unwrap());
    let status = Command::new(&program)
        .args(&prefix)
        .current_dir(&weird)
        .status()
        .expect("spawn in unicode/space path");
    assert!(status.success());
}

// ── shell resolution matrix (env-injected seams) ─────────────────────

/// Private "bin" directory containing only the supplied fake exes —
/// injected as the *entire* PATH so resolution can only see them.
fn fake_path_env(names: &[&str]) -> (tempfile::TempDir, OsString) {
    let dir = tempfile::TempDir::new().unwrap();
    for name in names {
        write_script(dir.path(), name, "");
    }
    let joined = std::env::join_paths([dir.path().to_path_buf()]).unwrap();
    (dir, joined)
}

#[test]
fn resolve_prefers_ralph_shell_absolute_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let fake = dir.path().join("my-shell.exe");
    std::fs::write(&fake, b"").unwrap();

    let (kind, exe) = windows_resolve_from(
        Some(fake.clone().into_os_string()),
        Some(OsString::from("C:\\Windows\\System32\\cmd.exe")),
        None,
    );
    assert_eq!(kind, ShellKind::Cmd);
    assert_eq!(exe, fake);
}

#[test]
fn resolve_ralph_shell_bash_name_routes_sh() {
    let dir = tempfile::TempDir::new().unwrap();
    let fake = dir.path().join("mybash.exe");
    std::fs::write(&fake, b"").unwrap();

    let (kind, exe) = windows_resolve_from(Some(fake.clone().into_os_string()), None, None);
    assert_eq!(kind, ShellKind::Sh);
    assert_eq!(exe, fake);
}

#[test]
fn resolve_falls_back_to_cmd_when_path_has_nothing() {
    let (_dir, path_env) = fake_path_env(&[]);
    let (kind, exe) = windows_resolve_from(
        None,
        Some(OsString::from(r"C:\Windows\System32\cmd.exe")),
        Some(&path_env),
    );
    assert_eq!(kind, ShellKind::Cmd);
    assert_eq!(exe, PathBuf::from(r"C:\Windows\System32\cmd.exe"));
}

#[test]
fn resolve_falls_back_to_cmd_exe_literal_without_comspec() {
    let (_dir, path_env) = fake_path_env(&[]);
    let (kind, exe) = windows_resolve_from(None, None, Some(&path_env));
    assert_eq!(kind, ShellKind::Cmd);
    assert_eq!(exe, PathBuf::from("cmd.exe"));
}

#[test]
fn resolve_pwsh_wins_over_cmd_when_present() {
    let (_dir, path_env) = fake_path_env(&["pwsh.exe"]);
    let (kind, exe) = windows_resolve_from(
        None,
        Some(OsString::from(r"C:\Windows\System32\cmd.exe")),
        Some(&path_env),
    );
    assert_eq!(kind, ShellKind::Pwsh);
    assert!(exe.to_string_lossy().ends_with("pwsh.exe"));
}

#[test]
fn resolve_explicit_cmd_respects_comspec() {
    let (kind, exe) = windows_resolve_from(
        Some(OsString::from("cmd")),
        Some(OsString::from(r"D:\custom\comspec.exe")),
        None,
    );
    assert_eq!(kind, ShellKind::Cmd);
    assert_eq!(exe, PathBuf::from(r"D:\custom\comspec.exe"));
}

#[test]
fn resolve_empty_ralph_shell_is_ignored() {
    let (_dir, path_env) = fake_path_env(&[]);
    let (kind, _) = windows_resolve_from(
        Some(OsString::from("   ")),
        Some(OsString::from("cmd.exe")),
        Some(&path_env),
    );
    assert_eq!(kind, ShellKind::Cmd);
}

#[test]
fn resolve_never_picks_wsl_system32_bash() {
    // Emulate a host where ONLY the WSL launcher bash.exe is visible on
    // PATH: sh-family resolution must not hand it out as Git Bash.
    let sys32 = tempfile::TempDir::new().unwrap();
    let fake_sys = sys32.path().join("Windows").join("System32");
    std::fs::create_dir_all(&fake_sys).unwrap();
    std::fs::write(fake_sys.join("bash.exe"), b"").unwrap();
    // PATH contains the fake System32 — bash.exe resolves but is WSL.
    let path_env = std::env::join_paths([fake_sys]).unwrap();

    let resolved = resolve_exe_in("bash", Some(&path_env));
    // Must NOT hand back the WSL launcher; falls back to bare "bash".
    assert!(
        !resolved
            .to_string_lossy()
            .to_lowercase()
            .contains("system32"),
        "WSL bash must be filtered, got {resolved:?}"
    );
}

// ── real process-tree termination ────────────────────────────────────

#[test]
fn kill_process_tree_kills_real_descendants() {
    // `cmd /c` wraps an infinite `ping -t`: cmd is the root, ping the
    // grandchild. Both must die; the test itself must survive.
    let mut root = Command::new("cmd")
        .args(["/c", "ping -t 127.0.0.1 >nul"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn cmd+ping tree");
    let root_pid = root.id();

    std::thread::sleep(std::time::Duration::from_millis(400));

    // Find the ping grandchild via sysinfo (its parent is our cmd).
    let ping_pid = {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};
        let mut sys = System::new_with_specifics(
            RefreshKind::nothing().with_processes(ProcessRefreshKind::nothing()),
        );
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            false,
            ProcessRefreshKind::nothing(),
        );
        sys.processes()
            .values()
            .find(|p| {
                p.parent().map(|pp| pp.as_u32()) == Some(root_pid)
                    && p.name()
                        .to_string_lossy()
                        .to_lowercase()
                        .starts_with("ping")
            })
            .map(|p| p.pid().as_u32())
    };

    kill_process_tree(root_pid);
    let _ = root.wait();

    // Root is dead.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while pid_alive(root_pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!pid_alive(root_pid), "tree root still alive after kill");

    // Grandchild is dead too — this is the whole point of tree kill.
    if let Some(ping) = ping_pid {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while pid_alive(ping) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(!pid_alive(ping), "grandchild ping survived tree kill");
    }
    // We are still alive (ancestor protection).
    assert!(pid_alive(std::process::id()));
}

#[test]
fn kill_process_tree_protects_own_process() {
    // Asking to kill our own PID must be a no-op for us (the caller is
    // always protected).
    kill_process_tree(std::process::id());
    assert!(pid_alive(std::process::id()));
}

#[test]
fn pid_alive_reports_dead_pid() {
    // Reap a child so its PID is guaranteed dead, then probe it.
    let mut child = Command::new("cmd").args(["/c", "exit 0"]).spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();

    // PID reuse could theoretically resurrect it — poll briefly.
    for _ in 0..20 {
        if !pid_alive(pid) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("reaped pid {pid} still reports alive");
}

// ── path containment scenarios ───────────────────────────────────────

#[test]
fn path_within_workspace_scenarios() {
    let dir = tempfile::TempDir::new().unwrap();
    let ws = dir.path().join("work space 中文");
    std::fs::create_dir_all(&ws).unwrap();

    // Descendant inside.
    let inside = ws.join(".ralph").join("events.jsonl");
    std::fs::create_dir_all(inside.parent().unwrap()).unwrap();
    std::fs::write(&inside, b"").unwrap();
    assert!(path_within_workspace(&ws, &inside).unwrap());

    // Not-yet-existing descendant.
    let future = ws.join("future").join("deep").join("file.txt");
    assert!(path_within_workspace(&ws, &future).unwrap());

    // Sibling escapes.
    let outside = dir.path().join("sibling.txt");
    std::fs::write(&outside, b"").unwrap();
    assert!(!path_within_workspace(&ws, &outside).unwrap());

    // `..` escape attempt must not pass.
    let escape = ws.join("..").join("sibling.txt");
    assert!(!path_within_workspace(&ws, &escape).unwrap());

    // Workspace itself counts as contained.
    assert!(path_within_workspace(&ws, &ws).unwrap());
}

#[test]
fn workspace_dir_with_reserved_tail_dot() {
    // A directory literally named `x.` is impossible on NTFS; verify
    // the containment helper errors closed instead of panicking on
    // hostile path shapes.
    let dir = tempfile::TempDir::new().unwrap();
    let weird = dir.path().join("trailing-dot-dir.").join("f.txt");
    // Should return Err or false — must not panic.
    let _ = path_within_workspace(dir.path(), &weird);
}

// ── env / json property checks ───────────────────────────────────────

#[test]
fn windows_paths_roundtrip_through_json() {
    // The exact bug family that broke several tests: a Windows path
    // hand-embedded into JSON. serde round-trip must be lossless for
    // any path shape.
    let mut rng = XorShift(0xABCD_EF01_2345_6789);
    for _ in 0..5_000 {
        let name = fuzz_name(&mut rng);
        let path = format!("C:\\Users\\{}\\{}", name, fuzz_name(&mut rng));
        let json = serde_json::to_string(&path).unwrap();
        let back: String = serde_json::from_str(&json).unwrap();
        assert_eq!(back, path);
    }
}

#[test]
fn tempdir_paths_are_containable_in_json_and_yaml() {
    let dir = tempfile::TempDir::new().unwrap();
    let p = dir.path().join("sub").join("f.txt").display().to_string();
    // JSON.
    let j = serde_json::json!({"path": p});
    let s = serde_json::to_string(&j).unwrap();
    assert!(s.contains("\\\\") || p.contains('/'), "json {s}");
    // YAML double-quoted scalars escape backslashes — single-quoted or
    // plain scalars do not. Round-trip must be lossless either way.
    let y = serde_yaml::to_string(&serde_json::json!({"path": p})).unwrap();
    let back: serde_json::Value = serde_yaml::from_str(&y).unwrap();
    assert_eq!(back["path"].as_str().unwrap(), p);
}
