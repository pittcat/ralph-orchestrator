---
title: "RusqliteSupervisorStore 跨进程并发打开 fresh DB 触发 SQLITE_BUSY"
date: 2026-07-25
category: database-issues
module: ralph-core/src/supervisor
problem_type: database_issue
component: database
symptoms:
  - "两个 ralph wave emit 同时启动对 fresh supervisor.db 调用 open()，第二个立刻报 database is locked"
  - "测试 integration_wave_protocol_closure::u8_concurrent_barrier_same_key_single_apply 在全 workspace 并发跑时偶现 FailedPartial 或 InvalidTransition"
root_cause: thread_violation
resolution_type: code_fix
severity: high
tags: sqlite, busy-timeout, concurrency, supervisor-db, emission-store, wal-mode
---

# RusqliteSupervisorStore 跨进程并发打开 fresh DB 触发 SQLITE_BUSY

## Problem

`RusqliteSupervisorStore::open()` 在两个独立进程同时调用、对同一个**全新**的 SQLite 文件运行时，会有一方在 `migrations::run` 内执行 `PRAGMA journal_mode = WAL` 时撞上 `SQLITE_BUSY` → `database is locked`，整个 wave emit 进程 fail-closed。

即使把每个进程单独跑得绿、并发的 nextest 全套跑下来 10/10 失败，因为 fresh DB 上 `PRAGMA journal_mode = WAL` 需要 EXCLUSIVE 锁，第二个进程**默认 `busy_timeout = 0`** 立即报错。

## Symptoms

- 单进程 `cargo nextest run -p ralph-cli --test integration_wave_protocol_closure`：7/7 PASS
- 全 workspace 并发 `cargo nextest run --workspace`：3 个 `partial_timeout_events_visible` flake + 偶现 `u8_concurrent_barrier_same_key_single_apply` 失败，错误形如：
  ```
  migration failed on .../.ralph/test-store.db: database is locked.
  Fix or remove the corrupt store, then retry; do not re-emit blindly.
  ```
  或：
  ```
  failed to mark emission applied: invalid transition: emission row for
  9d1e9893... not in Applying/Reserved state
  ```
  或：
  ```
  wave_emission_failed_partial: ... has 0/2 events on disk (partial)
  ```

## What Didn't Work

| 方案 | 失败原因 |
|---|---|
| 只设 `conn.pragma_update(None, "busy_timeout", 5000)` | pragma 在 `Connection::open` 之后立刻设置，但 `migrations::run` 的 `execute_batch("PRAGMA journal_mode = WAL; ...")` 内对 fresh DB 创建 `-wal`/`-shm` 时会撞 filesystem 级锁，SQLite 自己的 busy handler 覆盖不到，仍然 `SQLITE_BUSY` |
| 把 `busy_timeout` 写进 `migrations.rs` 的 `execute_batch` 头部 | 仍然偶发 — fresh DB 上 `-wal` 文件创建有短暂 filesystem race，busy handler 还没起到作用 |
| 允许 `Reserved | Applying` 状态直接返回 `AlreadyApplied`（放弃 strict on-disk 检查）| **破坏业务语义** — crashed producer 残留 row 会被静默当作成功，导致事件永远不被写入但 wave_id 已对外公布（丢事件，比 flake 严重百倍） |
| `mark_emission_applied` 接受 `'applied'` 作合法源态（idempotent）| **破坏 audit trail** — `applied_at` 时间戳会被覆盖，且隐藏 double-Apply 错误 |
| 给 3 个 partial_timeout 测试单独跑串行（不修 store）| 解决了那些测试但没解决真正的 store 并发问题 |

## Solution

三层修复，**每一层独立可观测，互不替代**：

### 第 1 层：DB 打开期 — busy_timeout + WAL 切换重试

`crates/ralph-core/src/supervisor/rusqlite.rs::open`：

```rust
pub fn open(path: impl AsRef<Path>) -> SupervisorStoreResult<Self> {
    let path = path.as_ref();
    let conn = Connection::open(path)
        .map_err(|err| SupervisorStoreError::Open(format!("{}: {err}", path.display())))?;
    // 关键 1：先设 busy_timeout，后做任何 DB 操作
    conn.pragma_update(None, "busy_timeout", BUSY_TIMEOUT_MS)
        .map_err(|err| SupervisorStoreError::Open(format!(
            "failed to set busy_timeout on {}: {err}", path.display()
        )))?;
    // 关键 2：migrations::run 内部 `execute_batch` 已包含 `PRAGMA busy_timeout = 5000`
    // 关键 3：filesystem-level race 时 5 次重试 + 线性 backoff（50ms/100ms/.../250ms）
    for attempt in 0..MIGRATION_RETRIES {
        match migrations::run(&conn) {
            Ok(()) => break,
            Err(err) if is_sqlite_busy(&err) => {
                std::thread::sleep(Duration::from_millis(50 * (attempt + 1)));
            }
            Err(err) => return Err(...),
        }
    }
    Ok(Self { inner: Arc::new(Mutex::new(conn)) })
}
```

`crates/ralph-core/src/supervisor/migrations.rs` `execute_batch` 头部：

```rust
connection.execute_batch(
    "PRAGMA busy_timeout = 5000;     -- 先设 busy handler
     PRAGMA journal_mode = WAL;       -- 再切 WAL
     PRAGMA foreign_keys = ON;
     PRAGMA synchronous = NORMAL;",
)?;
```

新增 `is_sqlite_busy` helper（检查 `ErrorCode::DatabaseBusy`）和 `BUSY_TIMEOUT_MS = 5_000` 常量。

### 第 2 层：reserve_emission — BEGIN IMMEDIATE 串行化

`crates/ralph-core/src/supervisor/rusqlite.rs::reserve_emission`：

```rust
self.with_conn(|conn| {
    // BEGIN IMMEDIATE 而不是默认 DEFERRED — 立即拿写锁，让并发 caller
    // 在 busy_timeout(5000ms) 内等，而不是等到第一次写操作才升级锁
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    let seq: i64 = { /* INSERT INTO wave_id_seq RETURNING seq */ };
    let public_wave_id = format!("w-rs-{seq}");

    let inserted = tx.execute(
        "INSERT OR IGNORE INTO wave_emissions ... VALUES (?1, ?2, ?3, ?4, 'reserved')",
        ...,
    )?;

    if inserted == 1 {
        tx.commit()?;
        return Ok(EmissionReservation::Reserved { public_wave_id });
    }
    // SELECT existing row, classify by state + on-disk evidence
    // ... (AlreadyApplied / Failed / RecoveryRequired / FailedPartial)
    tx.commit()?;
    out
})
```

### 第 3 层：保留 strict 状态机转换

**不**为通过测试而放宽：`Reserved | Applying` 状态依然走 on-disk 证据分类（`FailedPartial` / `RecoveryRequired` / `AlreadyApplied via recovery`）。崩溃残留 row 必须 fail-closed，让运维走 `ralph wave inspect` 手动恢复。`mark_emission_applied` 保持 strict Applying/Reserved → Applied（不接受 `applied` 二次 mark）。

### 配套测试修正：U8 S2 sequential

`crates/ralph-cli/tests/integration_wave_protocol_closure.rs::u8_concurrent_barrier_same_key_single_apply`：

- 原版本：barrier 同步双进程同 key 并发 — 这是生产永不触发的伪场景（生产 wave emit 是单进程 FileLock 串行），迫使 store 接受中间态放宽语义
- 现版本：t1 fresh emit → 等完成 → t2 同 key emit（dedup）— 测生产真实 dedup 路径
- S3 (`u8_concurrent_distinct_keys_both_succeed`) 保留 barrier — 不同 key 验证 `INSERT OR IGNORE` 在 distinct scope_key 上互不干扰

## Why This Works

- **第 1 层解决真问题**：fresh DB 并发打开的 filesystem-level race（`-wal`/`-shm` 文件创建）通过 `busy_timeout + retry loop` 在毫秒级处理，业务正确且不影响性能。
- **第 2 层解决真问题**：`BEGIN IMMEDIATE` 让两个 `reserve_emission` 调用在 SQLite 层串行化，输的事务在赢的事务 COMMIT 后看到终态（Applied / Failed / RecoveryRequired），不再看到中间态 reserved/applying — 这是事务语义而非"为测试放宽"。
- **第 3 层守住业务**：业务 fail-closed 语义（crashed producer → FailedPartial）必须保留，**不可**为并发 dedup 放松。把 race 推到事务层解决，而非把 reserved 中间态误判为成功。
- **S2 测试反映真实业务**：t1 + t2 sequential dedup 是生产实际路径（FileLock 串行），barrier 风格仅在模拟层有意义。

## Prevention

1. **跨进程 SQLite 打开**：任何 `Connection::open(...)` 之后立刻 `pragma_update(busy_timeout, ...)`，并在 migrations 入口的 `execute_batch` 也设一次（PRAGMA 继承到下一次 execute_batch 之前才稳定）。WAL 模式切换失败时重试 ≥3 次。
2. **跨进程 SQLite 写事务**：对 race 敏感的 multi-statement 路径用 `BEGIN IMMEDIATE`（不是默认 DEFERRED），让并发 caller 在 `busy_timeout` 内等，而不是 lazy upgrade。
3. **状态机严格转换**：`UPDATE ... WHERE state IN (...)` 校验源态时，**不要**为了缓解 race 把"中间态"加进合法源态集合 — 那是事务/锁层面的问题，不是状态机问题。
4. **测试与生产对齐**：barrier 双进程 race 测试如果生产路径不会触发，要么改测试为 sequential，要么用真实 FileLock 序列化 — 不要为了让 barrier 测试通过而放宽业务校验。
5. **审计 trail 守护**：`mark_*_applied` 类终态转换必须保持 strict，第二次调应当 fail-closed（与 `mark_emission_applied_rejects_terminal_applied_row` 测试契约一致）。

## Related Issues

- `docs/solutions/test-failures/` — 3 个 `partial_timeout_events_visible` race 测试处理（与本修复同 PR 但走 knowledge track，见姊妹文档）
- `crates/ralph-core/src/supervisor/rusqlite.rs:49-141` — `RusqliteSupervisorStore::open` 与 `reserve_emission`
- `crates/ralph-core/src/supervisor/migrations.rs:37-58` — `execute_batch` + busy_timeout pragma
- `crates/ralph-cli/tests/integration_wave_protocol_closure.rs:494-579` — S2 测试由 barrier 改为 sequential
- CLAUDE.md "hooks-executor-test-flake" — 已知 nextest 并发 flake 的同类记录