//! Supervisor Outside-In E2E tests.
//!
//! ## Historical context
//!
//! 2026-07-23-001 plan U9 established the original exec fan-in E2E
//! (`supervisor_primary_path_exec_wave_completes_with_schema_payload`).
//! That test only covers the exec wave → fan-in segment and is now
//! retained as **staged evidence** — it proves the dispatcher, SQLite
//! store, per-slot worktree and fan-in are wired, but does NOT prove
//! the full chain from `work.ready` to `LOOP_COMPLETE`.
//!
//! ## 2026-07-23-002 plan U6/U7 — full-chain Outside-In
//!
//! Product contract (`ce-executor-supervisor`): even with zero blocking
//! findings, `fix-task-planner` still emits a Fix wave (no-op / small
//! batch). "Non-blocking" ≠ skip Fix topology.
//!
//! - `supervisor_full_chain_non_blocking_review_reaches_loop_complete`:
//!   verdict=pass → 1 no-op Fix slot → `LOOP_COMPLETE` + R13 cleanup.
//! - `supervisor_full_chain_blocking_review_reaches_loop_complete`:
//!   `RALPH_E2E_BLOCKING=1` → 2 Fix slots → same terminal assertions.
//! - `supervisor_fault_path_one_slot_failure_no_loop_complete`:
//!   forced Exec slot failure → no successful `LOOP_COMPLETE`.
//!
//! 本文件不修改 `presets/en/`、preset schema 或运行时状态文件。

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::time::{Duration, Instant};

use ralph_core::supervisor::SupervisorStore;
use serde_json::Value;
use tempfile::TempDir;

mod common;

/// 单条测试的整体时间预算(看门狗,防 CI 挂死)。
/// 正常路径 exec 5 PTY spawn,exec 5/sleep 0.6s × 5 slots + 路由调度,典型 < 5s。
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

/// exec worker 的并发探测睡眠:足够长让 5 个 slot 在 cap=4
/// 下产生可观察的重叠窗口。
const PROBE_SLEEP: &str = "0.6";

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .expect("spawn git");
    assert!(status.success(), "git {args:?} failed");
}

/// 初始化 temp git 仓库:plan.md(5 unit)+ fake backend 脚本,
/// 全部进初始 commit(worktree 会 checkout 该树)。
fn init_repo(repo: &Path, backend_script: &str) {
    git(repo, &["init", "-q"]);
    git(repo, &["config", "user.name", "U9 E2E"]);
    git(repo, &["config", "user.email", "u9-e2e@test"]);
    std::fs::write(
        repo.join("plan.md"),
        "# E2E plan\n\n- U1: unit one\n- U2: unit two\n- U3: unit three\n- U4: unit four\n- U5: unit five\n",
    )
    .expect("write plan.md");
    std::fs::write(repo.join("fake-backend.sh"), backend_script).expect("write backend script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(repo.join("fake-backend.sh"))
            .expect("metadata")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(repo.join("fake-backend.sh"), perms).expect("chmod");
    }
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-q", "-m", "init"]);
}

fn write_project_config(repo: &Path) {
    // 绝对路径(backend 脚本可能 cwd = slot worktree,不能用相对路径)
    let backend = repo.join("fake-backend.sh");
    std::fs::write(
        repo.join("ralph.yml"),
        format!(
            r#"cli:
  backend: custom
  command: "{}"
  prompt_mode: stdin
event_loop:
  max_iterations: 30
  max_runtime_seconds: 120
"#,
            backend.display()
        ),
    )
    .expect("write ralph.yml");
}

/// fake backend:按 `$RALPH_WAVE_WORKER` / `$RALPH_WAVE_KIND`(wave
/// worker)或 `$RALPH_CURRENT_HAT`(hat activation)分支,直接向
/// `$RALPH_EVENTS_FILE` 写事件(等价于 `ralph emit` 的落盘形状)。
/// 关键:不写未来时间戳(event_reader 拒绝 future_timestamp 窗口外的事件)。
fn fake_backend_script(probe_sleep: &str) -> String {
    format!(
        r##"#!/bin/sh
# U9 E2E fake backend for builtin:ce-executor-supervisor.
# 每个 activation 只写该 hat 允许的唯一业务事件(isolated 单事件预算)。
cat >/dev/null 2>&1 || true
EF="$RALPH_EVENTS_FILE"
M="${{RALPH_E2E_MARKERS:-/tmp/u9-markers-$$}}"
mkdir -p "$M" 2>/dev/null || true
# 真实 UTC 时间戳:event_reader 拒绝 future_timestamp 窗口外的事件。
TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
once() {{
  if [ -f "$M/$1.done" ]; then return 1; fi
  touch "$M/$1.done"
  return 0
}}

# ── wave worker(dispatcher 以 PTY spawn,cwd = slot worktree)──────
if [ "$RALPH_WAVE_WORKER" = "1" ]; then
  case "$RALPH_WAVE_KIND" in
    exec)
      IDX="$RALPH_WAVE_INDEX"
      if [ -n "$RALPH_E2E_FAIL_SLOT" ] && [ "$IDX" = "$RALPH_E2E_FAIL_SLOT" ]; then
        echo "u9-e2e: forced exec slot $IDX failure" >&2
        exit 1
      fi
      if [ -n "$RALPH_E2E_CONC" ]; then
        mkdir -p "$RALPH_E2E_CONC"
        touch "$RALPH_E2E_CONC/active-$IDX"
        ls "$RALPH_E2E_CONC" 2>/dev/null | grep -c '^active-' > "$RALPH_E2E_CONC/seen-$IDX" || true
        sleep {probe_sleep}
        rm -f "$RALPH_E2E_CONC/active-$IDX"
      fi
      cat >> "$EF" <<EOF
{{"topic":"exec.unit.done","payload":"{{\\"wave_id\\":\\"$RALPH_WAVE_ID\\",\\"slot_index\\":$RALPH_WAVE_INDEX,\\"content_hash\\":\\"h-$RALPH_WAVE_INDEX\\",\\"unit\\":\\"u$RALPH_WAVE_INDEX\\"}}","ts":"$TS","hat":"worker","wave_id":"$RALPH_WAVE_ID","wave_index":$RALPH_WAVE_INDEX,"wave_total":5}}
EOF
      ;;
  esac
  exit 0
fi

case "$RALPH_CURRENT_HAT" in
  coordinator)
    if once coordinator-ready; then
      cat >> "$EF" <<EOF
{{"topic":"work.ready","payload":"{{\\"plan_name\\":\\"e2e-plan\\",\\"plan_path\\":\\"plan.md\\",\\"task_id\\":\\"t-1\\",\\"task_key\\":\\"plan:e2e:u1\\",\\"step\\":\\"step-01\\",\\"complexity\\":\\"small\\"}}","ts":"$TS","hat":"coordinator"}}
EOF
    fi
    ;;
  task-planner)
    if once task-planner; then
      i=0
      while [ $i -lt 5 ]; do
        cat >> "$EF" <<EOF
{{"topic":"exec.unit.ready","payload":"{{\\"wave_id\\":\\"w-exec-e2e\\",\\"slot_index\\":$i,\\"plan_name\\":\\"e2e-plan\\",\\"unit\\":\\"u$i\\"}}","ts":"$TS","hat":"task-planner","wave_id":"w-exec-e2e","wave_index":$i,"wave_total":5}}
EOF
        i=$((i + 1))
      done
    fi
    ;;
esac
exit 0
"##,
        probe_sleep = probe_sleep
    )
}

struct E2eEnv {
    #[allow(dead_code)]
    repo_holder: TempDir,
    #[allow(dead_code)]
    home_holder: TempDir,
    repo: PathBuf,
    home: PathBuf,
    markers: PathBuf,
}

fn setup_env(backend_script: &str) -> E2eEnv {
    let repo_holder = TempDir::new().expect("temp repo dir");
    let home_holder = TempDir::new().expect("temp home dir");
    let repo = repo_holder
        .path()
        .canonicalize()
        .expect("canonicalize repo");
    let home = home_holder.path().to_path_buf();
    let markers = home.join("markers");
    init_repo(&repo, backend_script);
    write_project_config(&repo);
    E2eEnv {
        repo_holder,
        home_holder,
        repo,
        home,
        markers,
    }
}

fn spawn_run(env: &E2eEnv, extra_env: &[(&str, String)]) -> Child {
    let mut cmd = common::ralph_bin();
    cmd.current_dir(&env.repo)
        .env("HOME", &env.home)
        .env("XDG_CONFIG_HOME", env.home.join(".config"))
        .env("XDG_DATA_HOME", env.home.join(".data"))
        .env("RALPH_E2E_MARKERS", &env.markers)
        .args([
            "run",
            "--no-tui",
            "--skip-preflight",
            "-H",
            "builtin:ce-executor-supervisor",
            "-P",
            "plan.md",
            "--max-iterations",
            "30",
        ]);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    cmd.spawn().expect("spawn ralph run")
}

fn wait_bounded(mut child: Child) -> (Output, std::process::ExitStatus) {
    let start = Instant::now();
    loop {
        match child.try_wait().expect("wait status") {
            Some(status) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut s) = child.stdout.take() {
                    use std::io::Read;
                    let _ = s.read_to_string(&mut stdout);
                }
                if let Some(mut s) = child.stderr.take() {
                    use std::io::Read;
                    let _ = s.read_to_string(&mut stderr);
                }
                let output = Output {
                    status,
                    stdout: stdout.into_bytes(),
                    stderr: stderr.into_bytes(),
                };
                return (output, status);
            }
            None => {
                if start.elapsed() > RUN_TIMEOUT {
                    let _ = child.kill();
                    panic!("ralph run exceeded {RUN_TIMEOUT:?}; killed");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// 读取主 ledger。
/// 主 ledger 用时间戳命名(`events-YYYY-MM-DDTHHMMSS.jsonl`),随每次
/// `ralph run` 创建一组。本测试取所有 `events-*.jsonl` 的合并去重。
fn read_ledger(repo: &Path) -> Vec<Value> {
    // 主 ledger 用时间戳命名(events-YYYY-MM-DDTHHMMSS.jsonl),
    // 而不是固定的 events.jsonl。本测试取所有 events-*.jsonl
    // + ledger.jsonl(若存在)的并集,按行追加去重。
    let ralph_dir = repo.join(".ralph");
    let mut lines: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&ralph_dir) {
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_file()
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("events-") && n.ends_with(".jsonl"))
                        .unwrap_or(false)
            })
            .collect();
        paths.sort();
        for p in paths {
            if let Ok(content) = std::fs::read_to_string(&p) {
                lines.extend(content.lines().map(String::from));
            }
        }
    }
    collected_into_events(repo, &ralph_dir, lines)
}

fn collected_into_events(repo: &Path, ralph_dir: &Path, lines: Vec<String>) -> Vec<Value> {
    let collected: Vec<String> = lines.into_iter().filter(|l| !l.trim().is_empty()).collect();
    if collected.is_empty() {
        eprintln!(
            "[E2E debug] .ralph contents at {}: {:?}",
            ralph_dir.display(),
            std::fs::read_dir(ralph_dir)
                .map(|d| { d.flatten().map(|e| e.file_name()).collect::<Vec<_>>() })
                .ok()
        );
        panic!("main ledger missing at {}/.ralph", repo.display());
    }
    collected
        .into_iter()
        .map(|l| {
            serde_json::from_str::<Value>(&l)
                .unwrap_or_else(|e| panic!("ledger line not JSON: {e}: {l}"))
        })
        .collect()
}

fn build_ralph_debug(repo: &Path, stderr_text: &str) -> String {
    let ralph_dir = repo.join(".ralph");
    let mut out = stderr_text.to_string();
    if let Ok(entries) = std::fs::read_dir(&ralph_dir) {
        out.push_str("\n[E2E debug] .ralph contents:\n");
        for e in entries.flatten() {
            out.push_str(&format!("  - {}\n", e.file_name().to_string_lossy()));
        }
    }
    out
}

fn events_with_topic<'a>(ledger: &'a [Value], topic: &str) -> Vec<&'a Value> {
    ledger
        .iter()
        .filter(|v| v.get("topic").and_then(|t| t.as_str()) == Some(topic))
        .collect()
}

/// 探测 supervisor store 里的所有 wave(库分配 id `w-<seq>`,
/// 从 1 开始连续;逐个探测到第一个 NotFound 为止)。
fn store_snapshots(
    store: &ralph_core::supervisor::RusqliteSupervisorStore,
) -> Vec<ralph_core::supervisor::WaveSnapshot> {
    let mut out = Vec::new();
    for seq in 1..32u32 {
        match store.fan_in_status(&format!("w-{seq}")) {
            Ok(snap) => out.push(snap),
            Err(_) => break,
        }
    }
    out
}

/// 验收 #1+#2+#3+#4 关键证据链(happy path):
/// builtin preset + fake backend + 5-unit plan → exec wave 经
/// dispatcher/SQLite/worktree/fan-in:
/// - 5 个 slot 全部 dispatched,业务事件按 slot index 排序进主 ledger;
/// - `exec.wave.complete` 经 production fan-in 注入并携带
///   schema-compliant payload;
/// - 真实 `git worktree add` 创建 5 条独立 worktree,均落在 temp repo 内;
/// - supervisor.db 含 5 个 exec slot (Completed / phase=Done /
///   merged_to_events=true)。
#[test]
fn supervisor_primary_path_exec_wave_completes_with_schema_payload() {
    let env = setup_env(&fake_backend_script(PROBE_SLEEP));
    let conc_dir = env.home.join("conc");
    let child = spawn_run(&env, &[("RALPH_E2E_CONC", conc_dir.display().to_string())]);
    let (output, status) = wait_bounded(child);
    let stderr_text = String::from_utf8_lossy(&output.stderr);
    assert!(
        status.code().is_some(),
        "ralph run must have an exit code.\n\
         [stderr]\n{}",
        build_ralph_debug(&env.repo, &stderr_text)
    );

    // ── 1. 真实生产 binding:supervisor.db 存在 ─────────────────────
    let db_path = env.repo.join(".ralph").join("supervisor.db");
    assert!(
        db_path.exists(),
        "production SQLite store must exist at {}\n\
         [stderr]\n{}",
        db_path.display(),
        build_ralph_debug(&env.repo, &stderr_text)
    );
    let store =
        ralph_core::supervisor::RusqliteSupervisorStore::open(&db_path).expect("open store");
    let snapshots = store_snapshots(&store);
    use ralph_core::supervisor::{WaveKind, WavePhase};
    let exec_snap = snapshots
        .iter()
        .find(|s| matches!(s.kind, WaveKind::Exec))
        .expect("exec wave must be persisted in the supervisor store");
    assert_eq!(exec_snap.expected_total, 5, "exec wave has 5 slots");

    // ── 2. 业务事件进主 ledger,按 slot index 排序 ───────────────
    let ledger = read_ledger(&env.repo);
    let exec_done = events_with_topic(&ledger, "exec.unit.done");
    assert_eq!(
        exec_done.len(),
        5,
        "5 de-duplicated exec.unit.done business events must land in main ledger"
    );
    for (pos, ev) in exec_done.iter().enumerate() {
        let payload_str = ledger_payload_string(ev);
        let parsed: Value =
            serde_json::from_str(&payload_str).expect("exec.unit.done payload parse");
        assert_eq!(
            parsed.get("slot_index").and_then(|s| s.as_u64()),
            Some(pos as u64),
            "ledger business event at position {pos} must be slot {pos} (sorted by slot index)"
        );
        assert_eq!(
            parsed.get("unit").and_then(|u| u.as_str()),
            Some(format!("u{pos}").as_str()),
            "slot {pos} payload identity"
        );
    }

    // ── 3. exec.wave.complete:fan-in 注入 schema-compliant 协调事件 ─
    let completes = events_with_topic(&ledger, "exec.wave.complete");
    assert_eq!(
        completes.len(),
        1,
        "exactly one exec.wave.complete (system-injected fan-in coord event)"
    );
    let coord = completes[0];
    assert_eq!(
        coord.get("system_injected").and_then(|v| v.as_bool()),
        Some(true),
        "exec.wave.complete must be system_injected (KTD-6)"
    );
    let coord_payload: Value =
        serde_json::from_str(&ledger_payload_string(coord)).expect("coord payload parse");
    // Schema required_fields:wave_id, completed_slots, merge_root_event_id
    assert_eq!(
        coord_payload
            .get("completed_slots")
            .and_then(|v| v.as_u64()),
        Some(5),
        "U9 closure: exec.wave.complete payload must carry completed_slots=5 \
         to satisfy presets/schemas/ce-executor-supervisor.yml required_fields"
    );
    assert!(
        coord_payload
            .get("merge_root_event_id")
            .and_then(|v| v.as_str())
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "exec.wave.complete payload must carry a non-empty merge_root_event_id"
    );
    // 5 个 success_slots,每个 slot_index/branch/worktree_path 配对
    let success_slots = coord_payload
        .get("success_slots")
        .and_then(|s| s.as_array())
        .expect("payload.success_slots must be an array");
    assert_eq!(
        success_slots.len(),
        5,
        "success_slots must list all 5 slots"
    );
    let mut payload_worktree_paths: Vec<String> = Vec::with_capacity(5);
    for (i, slot) in success_slots.iter().enumerate() {
        assert_eq!(
            slot.get("slot_index").and_then(|v| v.as_u64()),
            Some(i as u64),
            "success_slots[{i}].slot_index"
        );
        let branch = slot
            .get("branch")
            .and_then(|b| b.as_str())
            .expect("success_slots[i].branch");
        assert!(
            branch.ends_with(&format!("-exec-{i}")),
            "slot {i} branch must encode the slot index: {branch}"
        );
        let wt = slot
            .get("worktree_path")
            .and_then(|w| w.as_str())
            .expect("success_slots[i].worktree_path");
        payload_worktree_paths.push(wt.to_string());
    }
    let mut payload_dedup = payload_worktree_paths.clone();
    payload_dedup.sort();
    payload_dedup.dedup();
    assert_eq!(
        payload_dedup.len(),
        5,
        "5 pairwise-distinct worktree_path values in the fan-in payload"
    );

    // ── 4. SQLite 生产存储:5 slot / phase 收尾 / merged_to_events=true ─
    assert!(
        exec_snap.completed_count + exec_snap.failed_count >= 5,
        "5 slots must have terminal status: completed={} failed={}",
        exec_snap.completed_count,
        exec_snap.failed_count
    );
    assert!(
        matches!(
            exec_snap.phase,
            WavePhase::Done | WavePhase::Integrate | WavePhase::Collect
        ),
        "exec wave reached the dispatch lifecycle, got {}",
        exec_snap.phase
    );
    assert!(
        exec_snap.merged_to_events,
        "merged_to_events must be set (idempotent fan-in contract)"
    );
    use ralph_core::supervisor::SlotStatus;
    assert!(
        exec_snap.slots.iter().all(|(_, st)| matches!(
            st,
            SlotStatus::Completed | SlotStatus::Failed | SlotStatus::Cancelled
        )),
        "every exec slot is in a terminal status: {:?}",
        exec_snap.slots
    );

    // ── 5. 真实 worktrees:5 个 exec slot 有 distinct 路径,全部落在 temp repo 内 ─
    let exec_resources = store
        .list_worktree_paths(&exec_snap.wave_id)
        .expect("exec resources");
    assert_eq!(exec_resources.len(), 5, "5 exec slot resource rows");
    let mut db_paths: Vec<String> = exec_resources
        .iter()
        .map(|r| {
            r.worktree_path
                .clone()
                .expect("exec slot must own a worktree")
        })
        .collect();
    db_paths.sort();
    let mut db_dedup = db_paths.clone();
    db_dedup.dedup();
    assert_eq!(db_dedup.len(), 5, "5 distinct worktree paths in DB");
    assert_eq!(
        db_dedup, payload_dedup,
        "payload worktree_paths must equal DB slot resource rows"
    );
    for p in &db_dedup {
        let path = Path::new(p);
        // R13 / KTD8: runner terminal finalizer removes slot worktrees
        // on every loop exit. Mid-chain evidence is the store/payload
        // binding (distinct paths under the temp repo), not a live
        // directory after termination.
        assert_ne!(path, env.repo.as_path(), "slot worktree ≠ main workspace");
        assert!(
            path.starts_with(&env.repo),
            "slot worktree stays inside temp repo: {p}"
        );
        assert!(
            !path.exists(),
            "R13 cleanup must remove slot worktree after loop exit: {p}"
        );
    }

    // ── 6. cap=4:并发探测:同时活跃的 exec worker ≤ 4,且 ≥ 2 ────
    let mut max_seen: u32 = 0;
    if let Ok(entries) = std::fs::read_dir(&conc_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(_) = name.strip_prefix("seen-") {
                let n: u32 = std::fs::read_to_string(entry.path())
                    .unwrap_or_default()
                    .trim()
                    .parse()
                    .unwrap_or(0);
                max_seen = max_seen.max(n);
            }
        }
    }
    assert!(
        max_seen <= 4,
        "supervisor cap=4 violated: {max_seen} exec workers ran concurrently"
    );
    assert!(
        max_seen >= 2,
        "expected observable parallelism (>=2 concurrent exec workers), saw {max_seen}; \
         the dispatcher may have serialized the wave"
    );
}

/// EventPolicy payload 字段是 JSON-as-string(payload 是 EventPolicy
/// 校验的内部 format)。读取时按字符串解析,值嵌入在 JSONL 的顶层
/// payload 字段中;对于 `worker`-stamped events(如 `exec.unit.done`
/// 在 dispatcher's fan-in sink 重排后),顶层 payload 是 JSON object
/// 而不是 string。两种都要 handle。
fn ledger_payload_string(ev: &Value) -> String {
    match ev.get("payload") {
        None => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(_)) | Some(Value::Array(_)) => {
            // The fan-in sink normalizes `*.wave.complete` / `*.wave.failed`
            // 业务 payload 为 JSON object(this happens after U6 — see the
            // queue/serve boundary). Serialize it back to a JSON string
            // to match the (later) EventPolicy required-fields reader.
            serde_json::to_string(ev.get("payload").expect("payload")).expect("serialize payload")
        }
        Some(other) => other.to_string(),
    }
}

// ===========================================================================
// 2026-07-23-002 plan U6 — full-chain Red tests
// ===========================================================================

/// 全链 fake backend:驱动 16-hat supervisor preset 从 `plan.ready` 到
/// `LOOP_COMPLETE`。每个 hat activation 只写该 hat 允许的唯一业务事件
/// (isolated 单事件预算)。wave worker 通过 `$RALPH_EVENTS_FILE` 写
/// per-worker 事件,由 dispatcher 收集做 fan-in。
///
/// 覆盖的 hat 链:
/// coordinator → task-planner → worker(×5) → exec-integrator →
/// review-coordinator → review-batch-worker(×6) → review-synthesizer →
/// fix-task-planner → fix-worker(×1) → fix-integrator → alignment →
/// shipper(fall-through) → reporter → LOOP_COMPLETE
///
/// fault path(由 `RALPH_E2E_FAIL_SLOT` 触发):
/// worker slot fail → exec.wave.failed(system-injected) →
/// exec-failure-handler → work.failed → fixer → fix.exhausted →
/// coordinator(第二次激活,被 `once` 阻止)→ loop stall
fn full_chain_fake_backend_script(probe_sleep: &str) -> String {
    format!(
        r##"#!/bin/sh
# U6 full-chain fake backend for builtin:ce-executor-supervisor.
# Drives the complete 16-hat chain from plan.ready to LOOP_COMPLETE.
cat >/dev/null 2>&1 || true
EF="$RALPH_EVENTS_FILE"
M="${{RALPH_E2E_MARKERS:-/tmp/u6-markers-$$}}"
mkdir -p "$M" 2>/dev/null || true
TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
once() {{
  if [ -f "$M/$1.done" ]; then return 1; fi
  touch "$M/$1.done"
  return 0
}}

# ── wave worker(dispatcher PTY spawn)──────────────────────────────
if [ "$RALPH_WAVE_WORKER" = "1" ]; then
  # Determine wave kind:
  #   exec/fix: RALPH_WAVE_KIND set by supervisor_bridge
  #   review: RALPH_WAVE_KIND NOT set (bridge returns None for Review),
  #           but RALPH_WAVE_DIMENSION is set by dispatcher
  KIND="$RALPH_WAVE_KIND"
  if [ -z "$KIND" ] && [ -n "$RALPH_WAVE_DIMENSION" ]; then
    KIND="review"
  fi
  case "$KIND" in
    exec)
      IDX="$RALPH_WAVE_INDEX"
      if [ -n "$RALPH_E2E_FAIL_SLOT" ] && [ "$IDX" = "$RALPH_E2E_FAIL_SLOT" ]; then
        echo "u6-e2e: forced exec slot $IDX failure" >&2
        exit 1
      fi
      if [ -n "$RALPH_E2E_CONC" ]; then
        mkdir -p "$RALPH_E2E_CONC"
        touch "$RALPH_E2E_CONC/active-$IDX"
        ls "$RALPH_E2E_CONC" 2>/dev/null | grep -c '^active-' > "$RALPH_E2E_CONC/seen-$IDX" || true
        sleep {probe_sleep}
        rm -f "$RALPH_E2E_CONC/active-$IDX"
      fi
      # Write a nonce into the worktree and mirror to markers (R13 deletes wt)
      if [ -n "$RALPH_WAVE_WORKTREE_PATH" ] && [ -d "$RALPH_WAVE_WORKTREE_PATH" ]; then
        NONCE="nonce-slot-$IDX-$(date +%s%N)"
        echo "$NONCE" > "$RALPH_WAVE_WORKTREE_PATH/.e2e-nonce-$IDX"
        echo "$NONCE" > "$M/nonce-$IDX"
      fi
      cat >> "$EF" <<EOF
{{"topic":"exec.unit.done","payload":"{{\\"wave_id\\":\\"$RALPH_WAVE_ID\\",\\"slot_index\\":$IDX,\\"content_hash\\":\\"h-$IDX\\",\\"unit\\":\\"u$IDX\\"}}","ts":"$TS","hat":"worker","wave_id":"$RALPH_WAVE_ID","wave_index":$IDX,"wave_total":5}}
EOF
      ;;
    fix)
      IDX="$RALPH_WAVE_INDEX"
      cat >> "$EF" <<EOF
{{"topic":"fix.unit.done","payload":"{{\\"wave_id\\":\\"$RALPH_WAVE_ID\\",\\"slot_index\\":$IDX,\\"content_hash\\":\\"fh-$IDX\\"}}","ts":"$TS","hat":"fix-worker","wave_id":"$RALPH_WAVE_ID","wave_index":$IDX}}
EOF
      ;;
    review)
      IDX="$RALPH_WAVE_INDEX"
      DIM="$RALPH_WAVE_DIMENSION"
      cat >> "$EF" <<EOF
{{"topic":"review.unit.done","payload":"{{\\"wave_id\\":\\"$RALPH_WAVE_ID\\",\\"dimension\\":\\"$DIM\\",\\"findings\\":[]}}","ts":"$TS","hat":"review-batch-worker","wave_id":"$RALPH_WAVE_ID","wave_index":$IDX}}
EOF
      ;;
  esac
  exit 0
fi

# ── hat activation(non-wave-worker)────────────────────────────────
# Debug: log every hat activation
echo "[fake-backend] hat=$RALPH_CURRENT_HAT ts=$TS" >> "$M/activations.log"
case "$RALPH_CURRENT_HAT" in
  coordinator)
    # On plan.ready: emit work.ready
    # On fix.exhausted (fault path): would emit plan.blocked, but
    # plan.blocked is NOT in coordinator's publishes list → origin guard
    # rejects it. The `once` guard also prevents re-entry.
    if once coordinator-ready; then
      cat >> "$EF" <<EOF
{{"topic":"work.ready","payload":"{{\\"plan_name\\":\\"e2e-plan\\",\\"plan_path\\":\\"plan.md\\",\\"task_id\\":\\"t-1\\",\\"task_key\\":\\"plan:e2e:u1\\",\\"step\\":\\"step-01\\",\\"complexity\\":\\"small\\"}}","ts":"$TS","hat":"coordinator"}}
EOF
    fi
    ;;
  task-planner)
    if once task-planner; then
      i=0
      while [ $i -lt 5 ]; do
        cat >> "$EF" <<EOF
{{"topic":"exec.unit.ready","payload":"{{\\"wave_id\\":\\"w-exec-e2e\\",\\"slot_index\\":$i,\\"plan_name\\":\\"e2e-plan\\",\\"unit\\":\\"u$i\\"}}","ts":"$TS","hat":"task-planner","wave_id":"w-exec-e2e","wave_index":$i,"wave_total":5}}
EOF
        i=$((i + 1))
      done
    fi
    ;;
  exec-integrator)
    if once exec-integrator; then
      cat >> "$EF" <<EOF
{{"topic":"work.done","payload":"{{\\"plan_name\\":\\"e2e-plan\\",\\"plan_path\\":\\"plan.md\\",\\"task_id\\":\\"t-1\\",\\"task_key\\":\\"plan:e2e:u1\\",\\"step\\":\\"step-01\\",\\"commit_count\\":5,\\"changed_lines\\":100}}","ts":"$TS","hat":"exec-integrator"}}
EOF
    fi
    ;;
  exec-failure-handler)
    if once exec-failure-handler; then
      cat >> "$EF" <<EOF
{{"topic":"work.failed","payload":"{{\\"plan_name\\":\\"e2e-plan\\",\\"task_id\\":\\"t-1\\",\\"task_key\\":\\"plan:e2e:u1\\",\\"step\\":\\"step-01\\",\\"reason\\":\\"exec_slot_failed\\"}}","ts":"$TS","hat":"exec-failure-handler"}}
EOF
    fi
    ;;
  review-coordinator)
    if once review-coordinator; then
      i=0
      while [ $i -lt 6 ]; do
        case $i in
          0) DIM="correctness";;
          1) DIM="testing";;
          2) DIM="maintainability";;
          3) DIM="project-standards";;
          4) DIM="goal-alignment";;
          5) DIM="adversarial";;
        esac
        cat >> "$EF" <<EOF
{{"topic":"review.unit.ready","payload":"{{\\"wave_id\\":\\"w-review-e2e\\",\\"slot_index\\":$i,\\"dimension\\":\\"$DIM\\",\\"plan_name\\":\\"e2e-plan\\",\\"task_id\\":\\"t-1\\",\\"task_key\\":\\"plan:e2e:u1\\"}}","ts":"$TS","hat":"review-coordinator","wave_id":"w-review-e2e","wave_index":$i,"wave_total":6}}
EOF
        i=$((i + 1))
      done
    fi
    ;;
  review-synthesizer)
    if once review-synthesizer; then
      cat >> "$EF" <<EOF
{{"topic":"review.complete","payload":"{{\\"wave_id\\":\\"w-review-e2e\\",\\"completed_dimensions\\":[\\"correctness\\",\\"testing\\",\\"maintainability\\",\\"project-standards\\",\\"goal-alignment\\",\\"adversarial\\"],\\"aggregate_timeout\\":600,\\"plan_name\\":\\"e2e-plan\\",\\"task_id\\":\\"t-1\\",\\"task_key\\":\\"plan:e2e:u1\\",\\"step\\":\\"step-01\\",\\"verdict\\":\\"pass\\"}}","ts":"$TS","hat":"review-synthesizer"}}
EOF
    fi
    ;;
  fix-task-planner)
    if once fix-task-planner; then
      # Product: always activates after review.complete.
      # Default (non-blocking): 1 no-op unit. RALPH_E2E_BLOCKING=1: 2 units.
      if [ "$RALPH_E2E_BLOCKING" = "1" ]; then
        i=0
        while [ $i -lt 2 ]; do
          cat >> "$EF" <<EOF
{{"topic":"fix.unit.ready","payload":"{{\\"wave_id\\":\\"w-fix-e2e\\",\\"slot_index\\":$i,\\"plan_name\\":\\"e2e-plan\\",\\"task_id\\":\\"t-1\\",\\"task_key\\":\\"plan:e2e:u1\\",\\"unit_scope\\":\\"fix-$i\\"}}","ts":"$TS","hat":"fix-task-planner","wave_id":"w-fix-e2e","wave_index":$i,"wave_total":2}}
EOF
          i=$((i + 1))
        done
      else
        cat >> "$EF" <<EOF
{{"topic":"fix.unit.ready","payload":"{{\\"wave_id\\":\\"w-fix-e2e\\",\\"slot_index\\":0,\\"plan_name\\":\\"e2e-plan\\",\\"task_id\\":\\"t-1\\",\\"task_key\\":\\"plan:e2e:u1\\",\\"unit_scope\\":\\"no-op\\"}}","ts":"$TS","hat":"fix-task-planner","wave_id":"w-fix-e2e","wave_index":0,"wave_total":1}}
EOF
      fi
    fi
    ;;
  fix-integrator)
    if once fix-integrator; then
      cat >> "$EF" <<EOF
{{"topic":"fix.done","payload":"{{\\"plan_name\\":\\"e2e-plan\\",\\"plan_path\\":\\"plan.md\\",\\"executor_head_sha\\":\\"abc123\\",\\"fix_plan_file\\":\\"\\",\\"review_verdict\\":\\"pass\\",\\"fixes_applied\\":1,\\"fixes_skipped\\":0}}","ts":"$TS","hat":"fix-integrator"}}
EOF
    fi
    ;;
  alignment)
    if once alignment; then
      cat >> "$EF" <<EOF
{{"topic":"plan.complete","payload":"{{\\"plan_name\\":\\"e2e-plan\\",\\"completed_steps\\":[\\"u1\\",\\"u2\\",\\"u3\\",\\"u4\\",\\"u5\\"],\\"task_id\\":\\"t-1\\",\\"task_key\\":\\"plan:e2e:u1\\",\\"step\\":\\"complete\\",\\"verdict\\":\\"pass\\",\\"final_findings_count\\":0,\\"fix_round\\":0}}","ts":"$TS","hat":"alignment"}}
EOF
    fi
    ;;
  reporter)
    if once reporter; then
      cat >> "$EF" <<EOF
{{"topic":"LOOP_COMPLETE","payload":"{{\\"reason\\":\\"e2e full-chain complete\\"}}","ts":"$TS","hat":"reporter"}}
EOF
    fi
    ;;
  fixer)
    # Fault path only: activated by work.failed
    if once fixer; then
      cat >> "$EF" <<EOF
{{"topic":"fix.exhausted","payload":"{{\\"plan_name\\":\\"e2e-plan\\",\\"fix_round\\":0,\\"task_id\\":\\"t-1\\",\\"task_key\\":\\"plan:e2e:u1\\",\\"step\\":\\"step-01\\",\\"reason\\":\\"steward_escalation\\"}}","ts":"$TS","hat":"fixer"}}
EOF
    fi
    ;;
esac
exit 0
"##,
        probe_sleep = probe_sleep
    )
}

/// 全链超时预算:16-hat 链 + 3 个 wave(5 exec + 6 review + 1 fix)
/// 每个wave 需 PTY spawn + fan-in,典型 < 60s,留余量到 120s。
const FULL_CHAIN_TIMEOUT: Duration = Duration::from_secs(120);

fn wait_bounded_with(mut child: Child, timeout: Duration) -> (Output, std::process::ExitStatus) {
    let start = Instant::now();
    loop {
        match child.try_wait().expect("wait status") {
            Some(status) => {
                let mut stdout = String::new();
                let mut stderr = String::new();
                if let Some(mut s) = child.stdout.take() {
                    use std::io::Read;
                    let _ = s.read_to_string(&mut stdout);
                }
                if let Some(mut s) = child.stderr.take() {
                    use std::io::Read;
                    let _ = s.read_to_string(&mut stderr);
                }
                let output = Output {
                    status,
                    stdout: stdout.into_bytes(),
                    stderr: stderr.into_bytes(),
                };
                return (output, status);
            }
            None => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    panic!("ralph run exceeded {timeout:?}; killed");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// Shared happy-path assertions for full-chain Outside-In tests.
fn assert_full_chain_happy(
    env: &E2eEnv,
    status: &std::process::ExitStatus,
    expected_fix_slots: u32,
) {
    let ledger = read_ledger(&env.repo);
    assert!(
        status.success(),
        "full-chain must exit successfully\n{}",
        build_ralph_debug(&env.repo, "")
    );
    assert_eq!(
        events_with_topic(&ledger, "work.done").len(),
        1,
        "exactly one work.done"
    );
    assert_eq!(
        events_with_topic(&ledger, "plan.complete").len(),
        1,
        "exactly one plan.complete"
    );
    assert_eq!(
        events_with_topic(&ledger, "LOOP_COMPLETE").len(),
        1,
        "exactly one LOOP_COMPLETE"
    );
    assert!(
        events_with_topic(&ledger, "plan.blocked").is_empty(),
        "no plan.blocked"
    );
    assert!(
        events_with_topic(&ledger, "loop_stale").is_empty(),
        "no loop_stale"
    );

    // Exec nonce artifacts mirrored before R13 cleanup
    for i in 0..5 {
        let nonce = env.markers.join(format!("nonce-{i}"));
        assert!(
            nonce.exists(),
            "exec slot {i} must mirror worktree nonce to markers (causal artifact)"
        );
        let body = std::fs::read_to_string(&nonce).expect("read nonce");
        assert!(
            body.contains(&format!("nonce-slot-{i}-")),
            "nonce marker content for slot {i}: {body:?}"
        );
    }

    let db_path = env.repo.join(".ralph").join("supervisor.db");
    assert!(db_path.exists(), "supervisor.db must exist");
    let store =
        ralph_core::supervisor::RusqliteSupervisorStore::open(&db_path).expect("open store");
    let snapshots = store_snapshots(&store);
    use ralph_core::supervisor::{SlotStatus, WaveKind, WavePhase};

    let exec_snap = snapshots
        .iter()
        .find(|s| matches!(s.kind, WaveKind::Exec))
        .expect("exec wave in store");
    assert_eq!(exec_snap.completed_count, 5, "all 5 exec slots completed");
    assert_eq!(exec_snap.failed_count, 0, "no failed exec slots");
    assert!(
        matches!(exec_snap.phase, WavePhase::Done),
        "exec wave phase must be Done, got {:?}",
        exec_snap.phase
    );
    assert!(exec_snap.merged_to_events, "exec wave merged_to_events");
    assert!(
        exec_snap
            .slots
            .iter()
            .all(|(_, st)| matches!(st, SlotStatus::Completed)),
        "all exec slots Completed"
    );

    // Review slots are shared-readonly (no writable worktree binding)
    if let Some(review_snap) = snapshots
        .iter()
        .find(|s| matches!(s.kind, WaveKind::Review))
    {
        let review_resources = store
            .list_worktree_paths(&review_snap.wave_id)
            .expect("review resources");
        for r in &review_resources {
            assert!(
                r.worktree_path.is_none(),
                "review slot must be shared-readonly (no worktree), got {:?}",
                r.worktree_path
            );
        }
    }

    let fix_snap = snapshots
        .iter()
        .find(|s| matches!(s.kind, WaveKind::Fix))
        .expect("fix wave in store");
    assert_eq!(
        fix_snap.completed_count, expected_fix_slots,
        "expected {expected_fix_slots} fix slots completed, got {}",
        fix_snap.completed_count
    );
    assert!(
        matches!(fix_snap.phase, WavePhase::Done),
        "fix wave phase must be Done, got {:?}",
        fix_snap.phase
    );

    // R13 cleanup: exec + fix worktrees removed
    for snap in [&exec_snap, &fix_snap] {
        let resources = store.list_worktree_paths(&snap.wave_id).expect("resources");
        for r in &resources {
            if let Some(ref wt) = r.worktree_path {
                assert!(
                    !Path::new(wt).exists(),
                    "R13 cleanup violation: worktree still present: {wt}"
                );
            }
        }
    }
}

/// Non-blocking happy path: zero blocking findings → 1 no-op Fix slot → LOOP_COMPLETE.
#[test]
fn supervisor_full_chain_non_blocking_review_reaches_loop_complete() {
    let env = setup_env(&full_chain_fake_backend_script(PROBE_SLEEP));
    let child = spawn_run(&env, &[]);
    let (_output, status) = wait_bounded_with(child, FULL_CHAIN_TIMEOUT);
    assert_full_chain_happy(&env, &status, 1);
}

/// Blocking happy path: `RALPH_E2E_BLOCKING=1` → 2 Fix slots → LOOP_COMPLETE.
#[test]
fn supervisor_full_chain_blocking_review_reaches_loop_complete() {
    let env = setup_env(&full_chain_fake_backend_script(PROBE_SLEEP));
    let child = spawn_run(&env, &[("RALPH_E2E_BLOCKING", "1".to_string())]);
    let (_output, status) = wait_bounded_with(child, FULL_CHAIN_TIMEOUT);
    assert_full_chain_happy(&env, &status, 2);
}

/// U6 Red test: fault path — 1 个 Exec slot 失败,不应产生 LOOP_COMPLETE。
///
/// 强制 slot 0 失败。断言:
/// - 主 ledger 无 `LOOP_COMPLETE`
/// - 进程不以 success 退出(AE5 / R14)
///
/// **U6 当前状态**:本测试在 U6 阶段可能通过,但**不是因为正确的 fault
/// terminal**——而是因为 happy path 同一路由缺口(exec-integrator 未被
/// 激活)也阻止了 LOOP_COMPLETE 产生。当 U7 修复 happy path 路由后,本
/// 测试将自然变 Red,迫使 U7 同时闭合 fault terminal(plan.blocked 或
/// 明确失败退出)。这是 U6→U7 的预期渐进验证流。
///
/// 预期 Red 来源(U7 修复):
/// 1. happy path 路由修复后,slot failure 必须产生明确 fault terminal
/// 2. `plan.blocked` 需加入 coordinator 的 `publishes` 或走其他终态出口
#[test]
fn supervisor_fault_path_one_slot_failure_no_loop_complete() {
    let env = setup_env(&full_chain_fake_backend_script(PROBE_SLEEP));
    let child = spawn_run(&env, &[("RALPH_E2E_FAIL_SLOT", "0".to_string())]);
    let (output, status) = wait_bounded_with(child, FULL_CHAIN_TIMEOUT);
    let stderr_text = String::from_utf8_lossy(&output.stderr);

    let ledger = read_ledger(&env.repo);

    // ── 1. 无 LOOP_COMPLETE ─────────────────────────────────────────
    let loop_complete = events_with_topic(&ledger, "LOOP_COMPLETE");
    assert!(
        loop_complete.is_empty(),
        "fault path must NOT produce LOOP_COMPLETE, got {} events.\n\
         [exit code: {:?}]\n[stderr]\n{}",
        loop_complete.len(),
        status.code(),
        stderr_text
    );

    // ── 2. 进程不以 success 退出 ────────────────────────────────────
    // Either non-zero exit or timeout kill is acceptable.
    assert!(
        !status.success(),
        "fault path must NOT exit with success status.\n\
         [exit code: {:?}]\n[stderr]\n{}",
        status.code(),
        stderr_text
    );
}

/// U7 smoke: `plan.complete` must activate reporter and produce LOOP_COMPLETE.
///
/// Guards the isolated-mode ralph drain bug (build_prompt coordinator path
/// stealing multi-consumer pending) without the heavy diagnostic dump.
#[test]
fn plan_complete_activates_reporter_with_loop_complete() {
    let env = setup_env(&full_chain_fake_backend_script(PROBE_SLEEP));
    let child = spawn_run(&env, &[]);
    let (_output, status) = wait_bounded_with(child, FULL_CHAIN_TIMEOUT);
    assert!(status.success(), "routing smoke must exit successfully");

    let ledger = read_ledger(&env.repo);
    assert_eq!(
        events_with_topic(&ledger, "LOOP_COMPLETE").len(),
        1,
        "reporter must emit exactly one LOOP_COMPLETE"
    );
    assert!(
        env.markers.join("reporter.done").exists(),
        "reporter hat must activate after plan.complete"
    );
}
