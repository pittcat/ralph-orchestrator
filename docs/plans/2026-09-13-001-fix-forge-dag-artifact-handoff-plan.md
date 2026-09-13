---
title: 修复 DAG 模式 hat 交接证据链(artifact_refs 真实化)
type: fix
date: 2026-09-13
topic: forge-dag-artifact-handoff
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: docs/brainstorms/2026-09-11-001-feat-hat-handoff-state-requirements.md (GAP-03)
execution: code
---

# 修复 DAG 模式 hat 交接证据链(artifact_refs 真实化)

## 0. 计划状态

**READY** —— 所有实施关键决策置信度 ≥ 0.85,均有直接源码证据。

- **代码库基线**:工作区 HEAD(含未提交的 `docs/brainstorms/2026-09-11-001-feat-hat-handoff-state-requirements.md`);DAG 执行面最新 anchor 为 plan `2026-09-09-0917-fix-forge-dag-p1-closure-plan`。
- **调查范围**:`crates/ralph-cli/src/loop_runner/dag_scheduler/{spawn.rs(全文 3728 行),job_context.rs(全文),worktree.rs(全文)}`、`crates/ralph-core/src/supervisor/{migrations.rs,migrations/v18.sql,dag_store.rs,dag_store_rusqlite.rs,dag_store_memory.rs}`、`crates/ralph-core/src/{artifact_canonicalizer.rs,workspace_mutation_guard.rs}`、`crates/ralph-core/data/ralph-tools.md:30-50`、`crates/ralph-core/tests/ralph_tools_doc_drift.rs`、`presets/en/parallel-forge.yml`、`presets/schemas/parallel-forge.yml`、`crates/ralph-cli/tests/integration_dag_scheduler.rs(全文)`、`crates/ralph-e2e/src/scenarios/parallel_forge.rs`。
- **已执行的验证**:全文阅读 + 定向 grep(`RALPH_DAG_ARTIFACT_REFS` 全部读写点、`unit_report_path|review_report_path|verification_log_path` 全部命中、migration 注册点)。
- **尚未执行的验证**:未运行任何测试(计划阶段不改代码);所有"现有测试不受影响"的结论来自全文阅读的点名式核对(见 E13),U2 的 Acceptance Red 会首先实证这一点。
- **阻塞项**:无。

## 1. 功能目标

- **业务目标**:DAG 模式(`parallel-forge`)下,execute → review → verify → fix 的 hat 交接证据从"装饰性占位"变为"真实、可验证、对下游可见"。对应 brainstorm GAP-03(P0)。
- **用户/调用方**:(a) DAG runtime 自身(spawn 边界);(b) 下游 hat 子进程(reviewer / verifier / fixer agent);(c) preset 作者与 operator(文档与真实行为对齐)。
- **当前行为**(全部有源码证据,见 §2.2):
  1. Review/Verify/Fix 的 `JobContext.artifact_refs` 指向**从不存在的文件**(`prior_event_ref` 生成 `<stage>.events.jsonl`,实际文件命名为 `{job_id}.events.jsonl`,spawn.rs:1719-1720 vs :284);
  2. digest 是字面量 `"runtime-accepted"` / `"runtime-correction"`(spawn.rs:1725/1739/1749),`validate_context` 只查非空(job_context.rs:154-165),且 spawn 边界**只处理 `MissingFields`、`DigestMismatch` 被静默忽略**(spawn.rs:1774-1780);
  3. 注入子进程的 `RALPH_DAG_ARTIFACT_REFS` 只含 `verified_execution_plan_path` 一项——:1824 的局部变量**遮蔽**了 :1702 构建的 typed map,上游 stage 证据从未进 env;
  4. `ralph-tools.md:48` 宣称该 env 是"上游交付物引用列表",与实际行为不符(documented drift)。
- **目标行为**:
  1. 每个 stage 完成(accepted,以及 review 的 rejected)时,runtime 把 payload 中声明的业务 artifact(`*_path` 字段)以 `路径 + 真实 SHA-256` 落库到新表 `dag_stage_artifacts`(migration v24);
  2. 下游 stage spawn 前,runtime 从 store 取上游 artifact,重新计算文件摘要并比对:**缺失 / 文件不存在 / digest 漂移 → fail-closed 拒 spawn**(journaled `failed` terminal + `forge.unit.execution_failed`,`FailureClass::ContractViolation`);
  3. `RALPH_DAG_ARTIFACT_REFS` 注入**经过校验的完整 typed map**;job prompt 列出上游证据清单;
  4. 文档(ralph-tools.md 等)与行为一致。
- **行为差异**:下游 hat 第一次能拿到"上游证据在哪、内容指纹是什么"的真实答案;上游证据在两次 stage 之间被篡改/删除时,runtime 在 spawn 前拒绝启动下游 job(现状:照常启动)。
- **本次范围**:DAG 模式 spawn 边界 + 完成记录 + env/prompt 注入 + 文档同步。
- **非目标**(明确不做):wave / dag_shadow 模式(代码路径不经过 spawn.rs 的 Dag 门);Integrate stage(SpawnKind 无 Integrate 变体,integration.rs 另行处理);consumer 侧(hat 内)digest 校验工具;`ralph_tools_doc_drift.rs` 12-key 清单缺 `RALPH_DAG_EXPECTED_HEAD` 的既有盲区(记录为残留,不在本计划修);memory 系统增强(brainstorm GAP-01/02/04-08,后续独立 plan)。
- **输入**:上游 stage 已接受事件的 payload(`*_path` 字段,repo-relative);unit worktree 文件系统状态。
- **输出**:`dag_stage_artifacts` 行;下游 job 的 env / prompt;失败时的 journaled terminal + 合成失败事件。
- **状态变化**:新增 v24 表;`handle_completion` 增加 fail-soft 证据记录;`spawn_job` 增加 fail-closed 消费校验。
- **错误语义**:记录侧 fail-soft(单字段跳过 + warn,不改变已发生的事件接受,与 `record_accepted_evidence` 的既有哲学一致,spawn.rs:1374-1381);消费侧 fail-closed(spawn 前拒绝,永不启动拿不到合法证据的 job)。
- **兼容性要求**:旧 `dag.db`(user_version ≤ 23)经 migration runner 自动升级,已有行保留(遵循 v12→v13 / v13→v14 差分升级测试模式);wave/dag_shadow 行为零变化;`ralph emit` / preset schema / CLI 参数不变。
- **性能要求**:每次 completion 多 ≤N 次文件读取+哈希(N = payload 中 `*_path` 字段数,实际 ≤2);spawn 前每个引用一次重哈希;文件均为 KB 级 markdown/yml,可忽略。
- **安全要求**:agent 控制的 payload 路径必须过形状校验(仅相对路径、拒绝 `..`、解析后不逃逸 unit worktree),记录侧与消费侧双重校验(防御纵深,对齐 U26 `ensure_events_file_in_workspace` 的既有防御模式,spawn.rs:261-309)。
- **已知约束**:测试入口必须走 nextest(HARD RULE 1/2);集成测试 spawn `ralph` 必须 scrub hat env(HARD RULE 5);新增 migration 必须同步 `CURRENT_VERSION` 与硬编码版本断言;ralph-core 的 `supervisor-db` feature 默认关、ralph-cli 默认开。
- **已确认假设**(均有证据):unit worktree 路径确定性可推导(`<workspace>/.ralph/worktrees/<loop_id>-<unit_id>`,worktree.rs:201-202);payload `*_path` 字段相对 unit worktree(preset schema field_docs:"path repo-relative",presets/schemas/parallel-forge.yml:214-218);Fix 是 execute 的业务重跑(`resolve_stage_base` 对 Fix 取 prior="execute",spawn.rs:1317;固化测试 `u3_fix_admission_reads_prior_execute_evidence`,spawn.rs:3465)。
- **待验证假设**:无(全部已在调查阶段闭环)。

## 2. 代码库现状与证据

### 2.1 当前实现入口

```
EventLoop tick → DagSchedulerRuntime::observe_unit_event_dag (spawn.rs:1043)
  → driver.observe_accepted → queue_spawn(PendingSpawn) (spawn.rs:1135)
  → spawn_job (spawn.rs:1521)
      → journal.reserve_job (D20 fencing)
      → resolve_stage_base → UnitWorktree::acquire
      → ensure_events_file_in_workspace
      → 构建 artifact_refs (:1702-1755,占位逻辑)  ← 修改点 A
      → validate_context (:1774,只处理 MissingFields)  ← 修改点 B
      → build_job_prompt (:1983)                        ← 修改点 C(U3)
      → env 注入(:1824-1836 遮蔽 typed map)            ← 修改点 D(U3)
子进程退出 → drain_completions → handle_completion (spawn.rs:685)
  → record_accepted_evidence (:816-818,仅 accepted)     ← 修改点 E(U1,扩展为 artifact 记录)
```

数据边界:durable store = `<workspace>/.ralph/dag.db`(rusqlite,migrations v1-v23,migrations.rs:121 `CURRENT_VERSION = 23`);测试另有内存实现 `dag_store_memory.rs` + `attach_in_memory_stores`(mod.rs:297-300);store 契约测试套件在 `dag_store::contract_tests`(spawn.rs:3309 注释引用)。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | spawn.rs:1719-1755 | Review/Verify/Fix 的 artifact_refs 指向 `{stage}.events.jsonl`,digest 为字面量占位符 | 修改点 A:整段替换为 store 查询结果 | 高(全文阅读) |
| E2 | spawn.rs:284 vs :1719 | 实际事件文件命名 `{job_id}.events.jsonl`(`dag-U1-execute-a1.events.jsonl`),占位路径 `execute.events.jsonl` 永不存在 | 证明现状是"断链"而非"可用旧路径",无兼容负担 | 高 |
| E3 | spawn.rs:1824-1836 | env 的 `artifact_refs` 局部变量遮蔽 typed map,只注入 plan 一项 | 修改点 D:序列化 `context.artifact_refs` 全量 | 高 |
| E4 | job_context.rs:154-165 + spawn.rs:1774-1780 | DigestMismatch 只查非空且被 spawn 静默忽略 | 修改点 B:非 Ok 一律 fail_job | 高 |
| E5 | spawn.rs:816-818 + :1382-1418 | 证据记录只覆盖 accepted terminal,fail-soft | 修改点 E:artifact 记录扩展到 review-rejected,保持 fail-soft | 高 |
| E6 | presets/schemas/parallel-forge.yml:184-218, 259-295, 297-331 | `forge.unit.executed/reviewed/verified` 的 required_fields 分别含 `unit_report_path`/`review_report_path`/`verification_log_path`,field_docs 明确 "Must exist before emit" + "path repo-relative" | artifact 来源规则:payload 中非空 `*_path` 字符串字段 | 高 |
| E7 | worktree.rs:201-202 | worktree 路径 = `<repo>/.ralph/worktrees/<loop_id>-<unit_id>`,纯拼接、无随机性 | completion 时无需 acquire 即可解析 artifact 绝对路径;抽取共享 helper 防漂移 | 高 |
| E8 | ralph_core::workspace_mutation_guard::sha256_hex(bytes) (:211,pub) | 已有 pub 字节级 SHA-256 helper(64 位小写 hex) | digest 计算复用,不新增依赖、不重复实现 | 高 |
| E9 | artifact_canonicalizer.rs:177-188 + parallel_forge_handoff.rs:486-506 | plan artifact 的 digest 是 canonical YAML 字节哈希,与文件原始字节哈希语义不同 | plan 条目维持现状不动;新 artifact digest 用语义清晰的原始字节哈希,两者不混用 | 高 |
| E10 | migrations.rs:121, :646/:712/:741, :714-760, :779-848 | 新表 migration 模式:v19-23 纯 `CREATE TABLE IF NOT EXISTS` + `migrations()` 追加 + `CURRENT_VERSION` 递增;测试硬编码 23(:723-726) | U1 必须同步 4 处 + 更新版本断言测试 | 高 |
| E11 | spawn.rs 测试模块全文(:2066-3728) | fixture 完备:`dag_fixture`(真实 canonical digest 注册)、`real_job_exec_context`(sh backend 写 `$RALPH_EVENTS_FILE`,cwd=worktree)、`init_git_fixture`、`u3_runtime`(in-memory stores)、`register_plan_for_test` | 全部新测试可复用现有 fixture,不需要新测试框架 | 高 |
| E12 | spawn.rs:2202, :2234 | 现有 sh fixture 的 executor payload 为 `"{}"`(无 `*_path` 字段) | 该 fixture 只驱动 Execute spawn,不触发 Review spawn;U2 的 fail-closed 不影响它(实证见 U2 §16) | 高 |
| E13 | spawn.rs 测试模块全文点名核对 | **没有任何现有测试通过 spawn_job 启动 Review/Verify/Fix job**:`real_dag_executor_canary`(:2402)只 spawn Execute;`review_rejection_beyond_budget`(:2657)走 pipeline Blocked 合成失败;`verify_terminal_releases_slot`(:2356)/`post_acceptance_cleanup`(:2718)不触发 follow-up spawn;u3_* 只调 `resolve_stage_base` | U2 的 fail-closed 对现有单测零回归面(待 Acceptance Red 实证) | 高 |
| E14 | integration_dag_scheduler.rs:525 `dag_cli_completes_real_pipeline` | 该"E2E"实为 CLI 平面冒烟(preset check / hats validate / inspect),不驱动 spawn_job;真 DAG E2E 在 ralph-e2e cassette 场景 | 集成回归面 = 该文件全量 + ralph-e2e mock | 高 |
| E15 | ralph-e2e/src/scenarios/parallel_forge.rs:146-157, :308-322 | e2e 场景经 cassette/bus.publish 注入事件,不经 spawn_job 真实子进程 | 本计划不触碰其行为;最终门禁跑一遍确认 | 高 |
| E16 | ralph-tools.md:30-50 + ralph_tools_doc_drift.rs:40-53 | env 表 13 行;drift 测试锁 12 个 key 存在性(不锁描述文案) | U3 改描述文案不破 drift 测试;key 集合不变 | 高 |
| E17 | grep `RALPH_DAG_ARTIFACT_REFS` 全仓 | 除注入点外无任何 Rust 消费方;preset yml / hat instructions 零引用;仅文档类引用(ralph-tools.md、capability_inventory.rs:377、skills/ralph-preset-{author,review}/references/) | U3 改 env 内容无行为消费者回归;文档引用需核对准确性 | 高 |
| E18 | spawn.rs:1126-1133 + follow_up_kind (:513-523) | Fix 的 feedback 取自 payload `review_report_path`;rejected review 的 payload 在 handle_completion :784 可得 | review-rejected 也必须记录 artifact(否则 Fix 拿不到证据) | 高 |
| E19 | spawn.rs:1317 + u3_fix_admission_reads_prior_execute_evidence (:3465) | Fix 在 base 解析上等同 execute 重跑(prior="execute") | D8:Fix 完成的 artifact 归到业务语义 stage "execute",否则 fix 重写 completion 文件后 Review 永远 digest mismatch | 高 |

### 2.3 受影响范围

- **生产模块**:`crates/ralph-core/src/supervisor/{migrations.rs, migrations/v24.sql(新增), dag_store.rs, dag_store_rusqlite.rs, dag_store_memory.rs}`;`crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs`;`crates/ralph-cli/src/loop_runner/dag_scheduler/worktree.rs`(仅抽取路径 helper,行为不变)。
- **测试模块**:上述各文件内联 tests;`dag_store::contract_tests`;无新增测试文件。
- **配置 / CLI / API / UI**:不变。
- **数据**:`dag.db` 新增表 `dag_stage_artifacts`(v24);旧库自动迁移。
- **文档**:`crates/ralph-core/data/ralph-tools.md`(env 表描述);`skills/ralph-preset-author/references/agent-native-model.md` 与 `skills/ralph-preset-review/references/{agent-native-model,commands,finding-rubric}.md`(核对 ARTIFACT_REFS 描述,仅在有失实描述时修订);AGENTS.md/CLAUDE.md(JobContext 字段清单不变,实现后反向核对)。
- **调用方**:`DagSchedulerStore` trait 的全部实现者(rusqlite + memory,编译器兜底);`spawn_job` 内部调用点(单文件内)。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| D1 | artifact_refs 指向什么 | (a) 上游 job 的 events.jsonl;(b) payload `*_path` 声明的业务 artifact | **(b)** | E6(schema 已有 "must exist before emit" 契约)、AGENTS.md artifact-first 规则、E2(events 文件命名证明现状即断链) | events.jsonl 是 runtime 内部 ledger,HARD RULE 4.5 禁止 hat 读 ledger;且 hatch 消费它需要解析 JSONL 再找 payload,间接一层 | 0.90 |
| D2 | 存哪里 | (a) 新表 `dag_stage_artifacts`(v24);(b) ALTER `dag_stage_evidence` 加列 | **(a)** | E10(新表模式简单、无 column_probe;v18 表语义是 per-stage 单证据行,与 per-field 多行不匹配) | ALTER 需 column_probe 路径且污染 evidence 表语义 | 0.90 |
| D3 | digest 算法 | (a) 原始字节 SHA-256;(b) canonical YAML 哈希 | **(a)** | E8(现成 pub helper)、E9(canonical 语义专属 plan handoff,混用会制造两套"digest"含义) | (b) 只适用于 plan YAML,markdown 报告无 canonical 化必要 | 0.90 |
| D4 | 记录侧严格性 | (a) fail-soft 跳过+warn;(b) fail-closed 拒绝接受事件 | **(a)** | E5(既有 `record_accepted_evidence` 同哲学;事件已被 merge 进 ledger,此时 fail-closed 会改变接受语义、回归面大) | (b) 会把"agent 谎报路径"从下游 spawn 门禁提前到接受门禁,是更大的行为变更,留给后续 plan | 0.85 |
| D5 | 消费侧严格性 | (a) spawn 前 fail-closed;(b) 缺失时降级为占位 | **(a)** | brainstorm GAP-03 需求;E13(现有测试零回归面);job_context required_fields 本就声明这些键必填 | (b) 等于保留现状漏洞 | 0.90 |
| D6 | 记录哪些 terminal | (a) 仅 accepted;(b) accepted + review-rejected | **(b)** | E18(Fix 的证据来自 rejected review 的 report) | (a) 会让 Fix spawn 永远找不到 review artifact | 0.90 |
| D7 | Fix 的 artifact 归到哪个 stage | (a) 记为 "fix";(b) 记为 "execute" | **(b)** | E19(Fix 是 execute 的业务重跑;Review 永远查 "execute" 的最新行;记为 fix 会导致 fix 后 Review 拿旧 digest 必然 mismatch) | (a) 制造必然漂移,见 §7 U2 风险 R3 | 0.90 |
| D8 | 路径形态(存储 vs 注入) | 存储 repo-relative,注入时解析为绝对路径 | **同左** | E6(repo-relative 是 schema 契约)、E7(worktree 可确定性解析);绝对路径对子进程无歧义(cwd=worktree 但 abs 恒可用) | 存绝对路径会把 host 路径烤进 durable store,工作区搬迁即腐坏 | 0.85 |
| D9 | env 内容 | 序列化 validate 后的 `context.artifact_refs` 全量 | **同左** | E3(遮蔽是 bug 本身);E17(无消费者依赖旧形状) | 保留单条等于不修 | 0.90 |
| D10 | prompt 是否列出证据 | build_job_prompt 增加只读证据清单块 | **同左** | E17(hat 当前完全无从得知上游证据位置;env 可编程但 prompt 才是 agent 的第一视野) | 不加则 env 修复对 agent 几乎不可发现 | 0.85 |

全部 ≥ 0.85,无 BLOCKED 决策。

## 4. BDD 行为规格

Feature: DAG stage 交接证据记录(U1)

  Background:
    Given 一个 Dag scheduler_mode 的 loop
    And plan "pf-test" 已注册并有 verified base commit

  Scenario: accepted execute 完成后 artifact 落库
    Given executor job 在 unit worktree 写了 ".ralph/forge/pf-test/units/U1-completion.md"
    When 该 job 的完成事件 payload 含 unit_report_path 指向该文件
    Then dag_stage_artifacts 出现 (pf-test, U1, execute, attempt, unit_report_path) 行
    And digest 等于该文件字节的 SHA-256
    And artifact_path 保持 repo-relative 原文

  Scenario: review REJECTED 的 report 也落库
    When review job 完成且 verdict=REJECTED,payload 含 review_report_path
    Then dag_stage_artifacts 出现 stage="review" 的 review_report_path 行

  Scenario: fix 完成的 artifact 归入 execute 语义 stage
    When fix job 完成,payload 含 unit_report_path
    Then 落库行的 stage 为 "execute"(不是 "fix")
    And 查询 execute 的 latest artifacts 返回该行

  Scenario: 声明了路径但文件不存在
    When 完成事件 payload 的 *_path 指向不存在文件
    Then 该字段不落库,记录 warn
    And 事件接受与 merge 流程不受影响

  Scenario: 恶意路径被拒记录
    When payload 的 *_path 为绝对路径或含 ".." 逃逸
    Then 该字段不落库,记录 warn

  Scenario: payload 无 *_path 字段
    When 完成事件 payload 为 "{}" 
    Then 不落任何行,不报错

  Scenario: 旧库升级
    Given 一个 user_version=23 的 dag.db,已有业务行
    When migration runner 执行
    Then dag_stage_artifacts 表存在,旧表行数不变,user_version=24

Feature: DAG spawn 前证据校验(U2)

  Scenario: 证据齐全时 review 正常启动
    Given store 中有 execute 的 unit_report_path artifact 且文件与 digest 一致
    When runtime spawn Review job
    Then job 启动,JobContext.artifact_refs 含 executor_completion_artifact_path/digest
    And path 为解析后的绝对路径,digest 为记录值

  Scenario: 上游 artifact 被篡改 → 拒 spawn
    Given store 中有 artifact 记录
    When 文件内容在记录后被修改
    Then Review spawn 被拒,unit 得 journaled failed terminal
    And 合成 forge.unit.execution_failed,failure_class=contract_violation,reason 含字段名

  Scenario: 上游 artifact 文件被删除 → 拒 spawn
    When 记录的文件不存在
    Then 与篡改场景同样 fail-closed

  Scenario: 上游从未记录 artifact → 拒 spawn
    Given store 中无 execute artifact 行
    When runtime spawn Review job
    Then 同样 fail-closed(不再用占位路径放行)

  Scenario: Fix spawn 读取 rejected review 的 report
    Given review(rejected)的 review_report_path 已落库
    When runtime spawn Fix job
    Then fix_failure_fingerprint 与 correction_digest 指向该 report 的真实路径与 digest

  Scenario: Execute 不受影响
    When runtime spawn Execute job
    Then artifact_refs 仍只含 verified_execution_plan_path/digest(值不变)

Feature: 交接证据对子进程可见(U3)

  Scenario: env 携带全量引用
    Given Review spawn 通过校验
    When 子进程启动
    Then RALPH_DAG_ARTIFACT_REFS 解析为 JSON,含 executor_completion_artifact_path 等全部 key
    And 每个 key 的 {path, digest} 与 validate_context 校验的 typed map 逐项一致

  Scenario: prompt 列出证据清单
    When build_job_prompt 生成 Review prompt
    Then prompt 含上游证据块(字段名 + 绝对路径 + digest)

  Scenario: 文档与行为一致
    When 阅读 ralph-tools.md 的 DAG env 表
    Then RALPH_DAG_ARTIFACT_REFS 的描述与真实注入内容一致
    And ralph_tools_doc_drift 测试保持绿

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 层级 | 风险补充 | 需要 E2E |
|---|---|---|---|---|---|
| accepted execute 落库 | store 查询返回行,digest == sha256(文件字节) | spawn.rs tests 新增(真实 git fixture + sh backend 写文件) | 集成(in-process runtime) | 无 | 否 |
| review REJECTED 落库 | 同上,verdict=REJECTED 路径 | spawn.rs tests(直接驱动 handle_completion 等价路径) | 单元/集成 | 无 | 否 |
| fix 归入 execute | latest_stage_artifacts("execute") 返回 fix 产物 | spawn.rs tests | 集成 | 无 | 否 |
| 文件不存在/逃逸/空 payload | 零行落库 + warn;流程不断 | spawn.rs tests | 单元 | 安全形状校验(对齐 U26 模式) | 否 |
| 旧库升级 | v23 fixture 库升级后行保留 | migrations.rs tests(仿 :1071/:1156 模式) | 单元 | 无 | 否 |
| 证据齐全正常启动 | spawn 成功进入 active_jobs;refs 内容正确 | spawn.rs tests(全链路:execute→drain→observe→review spawn) | 集成 | 无 | 否 |
| 篡改/删除/未记录 → 拒 spawn | merge_queue 得 ContractViolation 失败事件;active_jobs 空;journal terminal=failed | spawn.rs tests(负路径不产生子进程,断言确定) | 集成 | 无 | 否 |
| Fix 证据映射 | fix spawn 的 refs 指向 review report | spawn.rs tests | 集成 | 无 | 否 |
| Execute 不受影响 | Execute refs 与现状逐项一致 | spawn.rs tests | 单元 | Characterization(先 pin 现状再改) | 否 |
| env 全量 | sh fixture 子进程内 grep env JSON key;缺 key 则 exit≠0、无成功事件 | spawn.rs tests(stage 分支 sh backend) | 集成(真子进程) | 无 | 否 |
| prompt 证据块 | build_job_prompt 输出含字段名+路径 | spawn.rs tests(纯函数断言) | 单元 | 无 | 否 |
| 文档一致 | drift 测试 + 人工 diff 核对 | ralph_tools_doc_drift | 结构测试 | 无 | 否 |

层级选择理由:失败路径全部确定性地停在 spawn 前(无子进程),用 in-process runtime 测试即可;成功路径的 env 可见性必须真起子进程(sh fixture)才算验证——这是本计划唯一需要真实进程的点,复用 E11 既有基建,不升级为大 E2E。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成测试 | Evidence |
|---|---|---|---|---|---|---|
| R1 | completion 时 artifact 证据落库(含 rejected review、fix→execute 归并) | U1 场景 1/2/3 | `dag_artifacts_recorded_on_accepted_completion` 等(spawn.rs tests) | contract_tests 新增 record/latest 用例;migrations 测试 | 同验收测试(in-process 真实 store) | E5/E6/E8/E18/E19 |
| R2 | 恶意/缺失路径不入库 | U1 场景 4/5/6 | `dag_artifacts_reject_unsafe_or_missing_paths` | 路径形状校验纯函数测试 | — | E6(安全约束) |
| R3 | v24 迁移与旧库升级 | U1 场景 7 | `migrations_apply_v24_creates_stage_artifacts` + 升级保行测试 | 同左 | — | E10 |
| R4 | spawn 消费真实 refs 并 fail-closed | U2 场景 1/2/3/4/5 | `dag_spawn_fails_closed_on_artifact_drift_or_absence`、`dag_review_spawn_carries_real_artifact_refs` 等 | validate_context 非 Ok 全路由测试 | 全链路 spawn 测试 | E1/E2/E4/E5 |
| R5 | Execute 零变化 | U2 场景 6 | `dag_execute_artifact_refs_unchanged`(characterization) | 同左 | — | E13 |
| R6 | env/prompt 全量可见 | U3 场景 1/2 | `dag_child_env_carries_full_artifact_refs`(真子进程)、`dag_job_prompt_lists_artifact_refs` | prompt 纯函数测试 | 真子进程测试 | E3/E17 |
| R7 | 文档同步 | U3 场景 3 | `ralph_tools_doc_drift` 全量 + capability_inventory 测试 | — | — | E16/E17 |

## 7. 严格串行开发单元

```
Unit 1(证据落库 + v24 迁移)
  ↓ 完成全部测试、重构和回归
Unit 2(spawn 消费校验 + fail-closed)
  ↓ 完成全部测试、重构和回归
Unit 3(env/prompt 可见性 + 文档同步)
```

---

### Unit 1:stage 完成时业务 artifact 证据落库(migration v24 + store + handle_completion 接线)

**1. Unit 目标**:一个 accepted(或 review-rejected)的 DAG job 完成后,其 payload 声明的业务 artifact 以 repo-relative 路径 + 真实 SHA-256 写入 `dag_stage_artifacts`,可按 (plan, unit, stage) 查最新 attempt 的行。

**2. 对应需求与 Scenario**:R1/R2/R3;U1 全部 7 个 Scenario;D1/D2/D3/D4/D6/D7/D8;E5/E6/E7/E8/E10/E18/E19。

**3. 外部可观察结果**:`dag.db` 升级后出现 `dag_stage_artifacts` 表;完成事件处理后表中出现对应行(store API 可查)。无 CLI/事件形状变化。

**4. 当前行为基线**:表现状 = 无该表、无该记录动作(E5:只有 `dag_stage_evidence` 的 accepted 证据)。该能力全新,无旧行为需 pin;但需 pin 既有行为不变:① `migrations_apply_v20_v23_creates_4_new_tables` 等现有迁移测试在版本推进后语义不变(仅版本号与表清单更新);② `handle_completion` 既有分支(空 payload、无 evidence)不产生任何新行——由"payload 无 *_path 字段"场景覆盖。

**5. 输入与输出**:
- 输入:完成 job 的 identity(plan_key/unit_id/stage/attempt)+ complete_payload 后的 payload Value。
- 输出:写入 `dag_stage_artifacts` 的行;每字段一条。
- 错误:文件不存在/读取失败/路径形状非法 → 该字段跳过 + warn!(fail-soft,D4);store 写失败 → warn!(同 E5 哲学)。
- 副作用:仅新表写;不改 `dag_stage_evidence` 写入路径。
- 不变量:事件接受语义不变;既有表行不变;fix 的 artifact 行 stage="execute"(D7)。

**6. 修改位置**(均已确认存在,除标记"新增"外):
- `crates/ralph-core/src/supervisor/migrations/v24.sql`(**新增**):`CREATE TABLE IF NOT EXISTS dag_stage_artifacts(plan_key TEXT NOT NULL, unit_key TEXT NOT NULL, stage TEXT NOT NULL, attempt INTEGER NOT NULL, field_name TEXT NOT NULL, artifact_path TEXT NOT NULL, artifact_digest TEXT NOT NULL, recorded_at_ms INTEGER NOT NULL, PRIMARY KEY(plan_key, unit_key, stage, attempt, field_name))`。
- `crates/ralph-core/src/supervisor/migrations.rs`:`CURRENT_VERSION` 23→24(:121);`migrations()` 数组尾部(:671-675 后)追加 v24 条目(无 column_probe);文件头版本说明注释同步;测试 `migrations_apply_v20_v23...`(:714-760)的版本断言与 `required_tables_exist_after_run`(:779-848)表清单同步。
- `crates/ralph-core/src/supervisor/dag_store.rs`:新增 `StageArtifactRecord` struct(字段同表列);trait `DagSchedulerStore` 新增 `record_stage_artifacts(&self, records: &[StageArtifactRecord])` 与 `latest_stage_artifacts(&self, plan_key, unit_key, stage) -> DagStoreResult<Vec<StageArtifactRecord>>`(latest = max attempt 的全部 field 行);`contract_tests` 扩展。
- `crates/ralph-core/src/supervisor/dag_store_rusqlite.rs`:两方法的 INSERT OR REPLACE / SELECT 实现(仿 :771/:875 既有方法模式)。
- `crates/ralph-core/src/supervisor/dag_store_memory.rs`:内存实现(BTreeMap key 即主键)。
- `crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs`:`handle_completion`(:784-818 区)在 accepted 与 review-rejected 两个分支调用新的 `record_stage_artifacts_from_payload(&identity, &payload)`(fail-soft);新增该私有方法:遍历 payload object 的非空字符串 `*_path` 字段 → 形状校验(相对、无 `..`)→ 解析 `<workspace>/.ralph/worktrees/<loop_id>-<unit_id>/<rel>`(loop_id 取自 exec,E7)→ 读文件算 `ralph_core::workspace_mutation_guard::sha256_hex`(E8)→ 组行(Stage 归并:Fix→"execute",D7)→ store 写。
- `crates/ralph-cli/src/loop_runner/dag_scheduler/worktree.rs`:抽取 `pub(crate) fn worktree_path_for(repo_root, loop_id, unit_id) -> PathBuf`(:201-202 拼接逻辑上移),`acquire` 改调它;**不改任何行为**。
- 明确不修改:`record_accepted_evidence` / `resolve_stage_base` / 任何 preset 文件。

**7. 可依赖能力**:E8 的 pub sha256 helper;E11 的全部 fixture;`attach_in_memory_stores` / `register_plan_for_test`(mod.rs:297-306);`journal()` / `ensure_stores()`。

**8. 禁止依赖的未来能力**:不得在本 Unit 读 `latest_stage_artifacts` 来构建 spawn 的 artifact_refs(U2);不得改 env/prompt(U3);不得在 spawn 中增加任何 fail-closed 分支。

**9. 验收测试**:
- `dag_artifacts_recorded_on_accepted_completion`(spawn.rs tests,集成):git fixture + 新 sh backend(execute 分支:worktree 内 `mkdir -p .ralph/forge/pf-test/units && printf … > U1-completion.md`,再 emit `{"topic":"forge.unit.executed","payload":"{\"unit_report_path\":\".ralph/forge/pf-test/units/U1-completion.md\"}"}`)→ maybe_spawn_executor + drain → 断言 store `latest_stage_artifacts("pf-test","U1","execute")` 返回一行,path 为 repo-relative 原文,digest == 对文件字节手动 sha256。
- `dag_artifacts_record_fix_under_execute_stage`:驱动 fix 身份的完成(直接构造 JobIdentity stage="fix" 走 handle_completion 等价路径或直接调新方法),断言行 stage="execute"。
- `dag_artifacts_record_rejected_review_report`:review + REJECTED verdict 完成 → stage="review" 行存在。
- `dag_artifacts_reject_unsafe_or_missing_paths`:payload 分别带 `/etc/passwd`、`../escape.md`、不存在文件 → 零行 + 无 panic;以及 payload `{}` → 零行。
- store contract 用例:record→latest 返回;高 attempt 覆盖语义(latest 只返回 max attempt 行);空查询返回空 Vec。rusqlite + memory 两后端都跑(contract_tests 模式)。
- 迁移:`migrations_apply_v24_creates_stage_artifacts`(断言表存在 + CURRENT_VERSION==24);升级保行测试(仿 :1071 模式:v23 库预置行 → run → 行在、新表在)。
- 运行命令:`cargo nextest run -p ralph-core --features supervisor-db -- migrations dag_store`;`cargo nextest run -p ralph-cli --bin ralph -- dag_artifacts`。

**10. Acceptance Red**:
- 先写 `dag_artifacts_recorded_on_accepted_completion` 并运行:Red 形态 = **编译失败**(`latest_stage_artifacts` / `record_stage_artifacts` 不存在)——这是新 API 引入的预期 Red 形态,如实记录;随后补最小 trait+impl 使编译通过但断言行不存在(store 无写入接线)→ 真实断言失败 → 接 `handle_completion` 后转绿。
- 迁移测试 Red:`assert dag_stage_artifacts exists` 在 v24 落地前失败(table 不存在),属真实行为失败。
- 无效 Red 排除:fixture 的 sh 脚本写错路径、git 未 init——先用现有 `real_dag_executor_canary` 的同构 fixture 验证基建可用。

**11. 单元测试拆分**:
- 路径形状校验纯函数:合法相对路径 Ok;绝对路径/`..`/空串 Err。
- `*_path` 字段提取:混合 payload(含非 path 字段、非字符串 path 字段、空字符串)只提取合法项。
- stage 归并映射:Fix→"execute",其余恒等。
- store 两后端:record/latest/覆盖/空。
- 迁移:v24 建表 + CURRENT_VERSION。
- 不允许 mock 的真实行为:文件 sha256 必须真读临时文件;store 必须用真实 rusqlite(tempfile)与真实内存实现,不用 mock store。

**12. Red → Green → Refactor 顺序**:
1. 迁移测试 Red(表不存在)→ v24.sql + 注册 + CURRENT_VERSION → Green;
2. contract_tests Red(方法不存在,编译 Red → 补空实现后断言失败)→ 两后端实现 → Green;
3. 形状校验/字段提取/stage 归并纯函数 Red → 实现 → Green;
4. `dag_artifacts_recorded_on_accepted_completion` Red(无接线)→ handle_completion 接线 → Green;
5. rejected-review / fix 归并 / 恶意路径用例逐个 Red→Green;
6. Refactor:worktree 路径 helper 抽取(行为不变,全部既有 worktree 测试保持绿)。

**13. 最小实现范围**:v24 表 + trait 两方法 + 两后端 + spawn 记录接线 + worktree helper 抽取。必须处理:文件读失败、形状非法、store 写失败(全部 fail-soft warn)。必须保持:accepted 证据既有写入不变;空 payload 零副作用。**不实现**:任何 spawn 消费、任何 env/prompt 变化、rejected review 之外的非 accepted terminal 记录。

**14. 集成验证**:`dag_artifacts_recorded_on_accepted_completion` 本身就是集成级(真 git worktree + 真 sh 子进程 + 真 rusqlite store)。另跑 `cargo nextest run -p ralph-cli --bin ralph -- dag_scheduler` 确认既有 DAG 测试全绿(特别是 canary 与 recovery 两个真子进程测试——它们的 payload 为 `{}`,应零新行)。

**15. 风险驱动测试**:路径形状校验已有专项(安全,对齐 U26);迁移升级保行(持久化);并发:单 runtime 单线程 drain,无并发写同一行的路径,**不加**并发测试(无风险依据)。

**16. 回归范围**:
- `cargo nextest run -p ralph-core --features supervisor-db`(migrations + dag_store + 全部 core;迁移版本号变更是全局面);
- `cargo nextest run -p ralph-cli --bin ralph -- dag`(spawn/driver/jobs/integration 相邻模块);
- `cargo nextest run -p ralph-cli --test integration_dag_scheduler`(E14:CLI 平面);
- 理由:改动集中在 dag store 与 spawn.rs;ralph-core 无 supervisor-db 默认特性,必须显式带 feature 跑(E10 附注)。
- `cargo clippy --all-targets` + `cargo fmt --check`。

**17. 预期文件变更**:

| 位置 | 变更类型 | 原因 | Evidence |
|---|---|---|---|
| crates/ralph-core/src/supervisor/migrations/v24.sql | 新增 | 新表 | E10 |
| crates/ralph-core/src/supervisor/migrations.rs | 修改 | 注册+版本+测试 | E10 |
| crates/ralph-core/src/supervisor/dag_store.rs | 修改 | 新 record struct + trait 方法 + contract 测试 | D2 |
| crates/ralph-core/src/supervisor/dag_store_rusqlite.rs | 修改 | rusqlite 实现 | D2 |
| crates/ralph-core/src/supervisor/dag_store_memory.rs | 修改 | 内存实现 | D2 |
| crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs | 修改 | 记录接线 + 新测试 | E5/E18/E19 |
| crates/ralph-cli/src/loop_runner/dag_scheduler/worktree.rs | 修改 | 路径 helper 抽取(行为不变) | E7 |

**18. 完成标准**:本 Unit 全部验收/单元/集成测试绿;上述回归全绿;clippy/fmt 绿;无跳过测试、无削弱断言;未实现 U2/U3 内容;v23 旧库升级测试绿;可独立提交。

**19. 停止条件**:`DagSchedulerStore` 出现第三个未知实现者(编译器暴露)→ 停下评估;payload `*_path` 在真实 preset 中不是 worktree-relative(与 E6 冲突)→ 停下重查;worktree 路径在 acquire 之外有第二套拼接(与 E7 冲突)→ 停下;迁移测试暴露 v19-23 之外的历史包袱 → 停下。

**20. 风险与注意事项**:
- R1(fixture 真实性):sh backend 在 PTY 下的 env/cwd 假设——先用既有 canary fixture 同构验证,失败即停(§19)。
- R2(macOS 路径):TempDir `/var` vs `/private/var` 漂移——所有路径比较前 canonicalize(既有测试 :417-425 的同构处理)。
- R3:record 发生在 drain 时,worktree 仍在(无任何清理逻辑,E 调查确认),风险低;剩余风险:operator 手工删 worktree 会留下悬空行——由 U2 的 spawn fail-closed 兜底检出,正是设计意图。

---

### Unit 2:spawn 前消费真实证据并 fail-closed

**1. Unit 目标**:Review/Verify/Fix spawn 时,`JobContext.artifact_refs` 由 store 中真实 artifact 构建(绝对路径 + 真实 digest),并经文件存在性 + digest 复核;任何缺失/漂移/逃逸在 spawn 前 fail-closed。

**2. 对应需求与 Scenario**:R4/R5;U2 全部 6 个 Scenario;D5/D8;E1/E2/E4/E13/E18/E19。

**3. 外部可观察结果**:证据漂移/缺失时,下游 job 不再启动;unit 收到 journaled `failed` terminal + `forge.unit.execution_failed`(failure_class="contract_violation",reason 含缺失字段名)。证据齐全时行为与现状等价(但 refs 内容为真)。

**4. 当前行为基线**:E1/E2(占位路径永不存在、占位 digest)、E4(DigestMismatch 被忽略)。Characterization 先行:`dag_execute_artifact_refs_unchanged` pin 住 Execute 的 refs 内容(plan path + canonical digest)在重构前后逐项一致;Execute 分支逻辑不在本 Unit 改。

**5. 输入与输出**:
- 输入:PendingSpawn(kind/plan_key/unit_key/attempt/feedback)+ store 中 U1 写入的行 + worktree 文件系统。
- 输出:成功 → context.artifact_refs(真实)+ 继续既有 spawn 流程;失败 → fail_job(ContractViolation) + return(不启动子进程)。
- 映射规则(固定,无 Executor 决策空间):
  - Review:`executor_completion_artifact_path` + `executor_completion_artifact_digest` ← latest("execute") 的 `unit_report_path` 行(两 key 指向同一 ArtifactRef,沿用现状的双 key 形状以满足 required_fields);
  - Verify:`executor_completion_artifact_path` ← latest("execute") 的 `unit_report_path`;`reviewer_completion_artifact_path` ← latest("review") 的 `review_report_path`;
  - Fix:`fix_failure_fingerprint` + `correction_digest` ← latest("review") 的 `review_report_path`(即 rejected review 的驳回报告,E18);
  - Execute:不变。
- 错误:行缺失 / 文件不存在 / digest 不等 / 解析后逃逸 worktree → fail-closed,reason 命名具体字段与性质。
- 不变量:占位字符串 `"runtime-accepted"`/`"runtime-correction"` 与 `prior_event_ref` 闭包从代码中消失;`validate_context` 的任何非 Ok 结果都路由到 fail_job(修复 E4)。

**6. 修改位置**:
- `spawn.rs:1719-1755`(修改点 A):删除 `prior_event_ref` 与整个占位 match;替换为按上表从 `ensure_stores()` 读取 + 解析 + 复核的构建逻辑(worktree 已在 :1652 acquire,顺序天然满足)。注意 `ensure_stores` 失败 = 无法验证 = fail-closed(与 spawn 无 journal 拒启动的既有姿态一致,:1578-1581)。
- `spawn.rs:1774-1780`(修改点 B):`validate_context` 改为匹配全部非 Ok 变体(MissingFields + DigestMismatch)→ fail_job(ContractViolation)。
- `job_context.rs`:不改(校验语义不变)。
- 明确不修改:env 注入段(U3)、`build_job_prompt`(U3)、integration.rs。

**7. 可依赖能力**:Unit 1 的 store 行与查询 API;E13 结论(现有测试无 Review/Verify/Fix spawn);fixture 基建。

**8. 禁止依赖的未来能力**:不得改 env 序列化内容(U3);不得加 prompt 块(U3);不得实现 Integrate stage 的任何 refs。

**9. 验收测试**(全部 spawn.rs tests):
- `dag_review_spawn_carries_real_artifact_refs`(集成):git fixture + stage 分支 sh backend(execute:写 completion + emit 带 unit_report_path;review:emit `forge.unit.reviewed` verdict=ACCEPTED 带 review_report_path 并先写该文件)→ 驱动 execute→drain→observe_unit_event_dag→review spawn→drain → 断言 review 成功事件进入 merge_queue( spawn 未被拒),且 store 出现 review 行。
- `dag_spawn_fails_closed_on_artifact_drift`:seed 记录后篡改 worktree 中文件 → 直接调 `spawn_job(PendingSpawn{kind:Review,…})` → 断言 active_jobs 空、merge_queue 一条 execution_failed、payload 含 "contract_violation" 与字段名;journal terminal=failed。
- `dag_spawn_fails_closed_on_artifact_absent`:无记录 → 同上断言。
- `dag_spawn_fails_closed_on_artifact_deleted`:有记录但文件删除 → 同上。
- `dag_fix_spawn_reads_rejected_review_report`:seed review 行 → spawn Fix → 不被拒;refs 指向 review report。
- `dag_execute_artifact_refs_unchanged`(characterization):Execute spawn 的 refs 只含 plan 两 key、值与 plan 记录一致。
- `validate_context` 非 Ok 路由:构造 digest 为空的 refs(单测层)→ fail_job 被调用(经 merge_queue 断言)。
- 运行命令:`cargo nextest run -p ralph-cli --bin ralph -- dag_`。

**10. Acceptance Red**:`dag_spawn_fails_closed_on_artifact_absent` 先写先跑——现状下 Review spawn 会用占位路径**正常通过校验并尝试启动**,断言"active_jobs 空 + ContractViolation 事件"失败,且失败原因正是占位放行(可观察:merge_queue 空或出现非预期 spawn 行为);这是真实行为 Red。注意:现状下该 spawn 会走到 `spawn_pty_job`(默认 backend 可能失败转而 fail_job SpawnFailed)——断言必须区分 failure_class,确保 Red 的失败信息可辨认(不是 ContractViolation)。

**11. 单元测试拆分**:
- refs 构建映射纯逻辑(给定 store 行集合 → 期望 key 集合):Review/Verify/Fix 三态 + 缺行报错命名字段。
- digest 复核:内容一致 Ok;改一字节 mismatch;文件消失 missing。
- Escape 复核:store 中被注入 `../` 行(模拟腐坏库)→ 拒。
- 不允许 mock:复核必须真读文件;store 用真实内存实现。

**12. Red → Green → Refactor 顺序**:
1. characterization(Execute pin)先绿;
2. `dag_spawn_fails_closed_on_artifact_absent` Red → 实现 store 查询 + 缺行 fail-closed → Green;
3. drift/deleted Red → 实现复核 → Green;
4. fix 映射 Red → Green;
5. validate_context 全路由 Red → Green;
6. 全链路正向 Red(review spawn 成功且 refs 为真)→ Green;
7. Refactor:删除占位代码与 `prior_event_ref`,确认无残留引用(grep "runtime-accepted")。

**13. 最小实现范围**:修改点 A+B + 上述测试。必须保持:Execute 路径逐字节等价;`feedback` prompt 行行为不变(它由 observe_unit_event_dag 传入,U2 不动);wave/dag_shadow 零接触。**不实现**:env 序列化变更、prompt 变更、文档变更。

**14. 集成验证**:全链路正向测试即集成验证;另跑 E14 集成文件 + `cargo run -p ralph-e2e -- --mock`(E15:确认 cassette 场景不受影响——预期零影响,因其不经 spawn_job)。

**15. 风险驱动测试**:Fault injection(文件删除/篡改)已内建为验收;并发:同 unit stage 串行(DAG pipeline 保证),跨 unit 行主键隔离,不加并发测试;State-machine:fix→execute 归并后的 latest 语义已在 U1 contract 覆盖,此处补一个"fix 后 review 重跑取到新 digest"的链式用例(R3 防护)。

**16. 回归范围**:
- `cargo nextest run -p ralph-cli --bin ralph -- dag`(重点:E13 点名过的全部现有测试必须原样绿——它们是"零回归面"结论的实证);
- `cargo nextest run -p ralph-core --features supervisor-db`;
- `cargo nextest run -p ralph-cli --test integration_dag_scheduler`;
- `cargo run -p ralph-e2e -- --mock`;
- `cargo clippy --all-targets` + `cargo fmt --check`。
- 若任何 E13 点名测试在本 Unit 变红,即触发 §19 停止条件(说明回归面结论错误)。

**17. 预期文件变更**:

| 位置 | 变更类型 | 原因 | Evidence |
|---|---|---|---|
| crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs | 修改 | 修改点 A+B + 全部新测试 | E1/E4 |

**18. 完成标准**:同 U1 标准 + `"runtime-accepted"`/`"runtime-correction"`/`prior_event_ref` 全仓 grep 零残留;E13 点名测试全绿;characterization pin 绿。

**19. 停止条件**:E13 结论被证伪(任一既有测试因 fail-closed 变红)→ 停下重新评估影响面;真实 preset 流程中 reviewer 需要的证据不在 `unit_report_path`(与 E6/D1 冲突)→ 停下;`ensure_stores` 在 spawn 期的可用性与假设不符 → 停下。

**20. 风险与注意事项**:
- R1(行为收紧):真实运行中 agent 不写 artifact 文件时,Review spawn 将被拒、unit 进入 correction/失败通道而非静默放行。这是**预期门禁**(preset instructions 本就要求写文件 + `test -f` 自检,presets/en/parallel-forge.yml:802-807);检测方式 = execution_failed 事件的 reason;缓解 = reason 文案必须指名缺哪个字段、期望哪份文件。
- R2:fix 归并若漏掉(D7 实现错误)会造成"fix 后 review 必然 drift 拒 spawn"死循环——由 §15 的链式用例钉死。
- R3(残留):dag_shadow/wave 无此校验,属范围外;`dag_stage_artifacts` 无清理策略(行随 DB 生命周期),量小,记录为已知残留。

---

### Unit 3:交接证据对子进程可见(env 全量 + prompt 块 + 文档同步)

**1. Unit 目标**:DAG 子进程的 `RALPH_DAG_ARTIFACT_REFS` 携带经校验的完整 typed map;job prompt 列出上游证据清单;文档与行为一致。

**2. 对应需求与 Scenario**:R6/R7;U3 全部 3 个 Scenario;D9/D10;E3/E16/E17。

**3. 外部可观察结果**:子进程 env 的 JSON 含当前 stage 全部 artifact 引用;prompt 中可见证据清单;ralph-tools.md 描述为真。

**4. 当前行为基线**:E3(遮蔽,只注入 plan 一项)、E16(文档描述失实)。无消费者依赖旧形状(E17),无需 characterization pin 旧内容;需 pin:13 个 env key 集合不变(drift 测试锁定项不动)。

**5. 输入与输出**:输入 = U2 产出的 `context.artifact_refs`;输出 = env JSON(`{key: {path, digest}}`)+ prompt 证据块。不变量:env key 集合 13 个不变;D23 env 契约(clear_env + allowlist + overlay,:1816-1823)不变。

**6. 修改位置**:
- `spawn.rs:1824-1836`(修改点 D):删除遮蔽段,`RALPH_DAG_ARTIFACT_REFS` 改为序列化 `context.artifact_refs`(env 构建点已在 validate 之后,:1774 → :1837,顺序天然满足)。
- `spawn.rs:1983-2052`(修改点 C):`build_job_prompt` 增加参数 `artifact_refs: &BTreeMap<String, ArtifactRef>`;在 DAG JOB CONTEXT 块尾部追加证据清单(字段名 + 绝对路径 + digest;Execute 只有 plan 两项也照列)。既有调用点 :1781 同步;既有测试 `job_prompt_surfaces_unit_path_policy`(:~3290)同步更新签名。
- `crates/ralph-core/data/ralph-tools.md:48`:ARTIFACT_REFS 行描述修订为真实语义(内容 = 本 stage 的上游交付物引用;path = 绝对路径;digest = 接受时文件字节 SHA-256、spawn 前 runtime 已复核)。
- `skills/ralph-preset-author/references/agent-native-model.md` 与 `skills/ralph-preset-review/references/` 下引用 ARTIFACT_REFS 的文件(E17):核对描述,失实才改;capability anchor token 不变。
- AGENTS.md/CLAUDE.md:实现后反向核对 :142 段(预期无需改,字段清单未变)。

**7. 可依赖能力**:U2 的真实 refs;E16 结论(drift 测试不锁描述文案)。

**8. 禁止依赖的未来能力**:不得新增 env key;不得改 hat instructions(preset 文本演进是另一个话题);不得修 drift 测试 12-key 盲区(残留记录)。

**9. 验收测试**:
- `dag_child_env_carries_full_artifact_refs`(真子进程):stage 分支 sh backend 的 review 分支首行执行 `printf '%s' "$RALPH_DAG_ARTIFACT_REFS" | grep -q executor_completion_artifact_path || exit 42`(exit≠0 → 无成功事件);驱动到 review spawn → 断言 merge_queue 得 `forge.unit.reviewed`(证明 env 含 key)且 review 行落库。
- `dag_job_prompt_lists_artifact_refs`(纯函数):构造含两条 refs 的 map → prompt 含字段名与路径;空 map(理论上不发生,Execute 有 plan 项)→ 块仍渲染 plan 项。
- 文档:`cargo nextest run -p ralph-core --test ralph_tools_doc_drift` + `cargo nextest run -p ralph-core -- capability_inventory` 全绿;`cargo build -p ralph-cli && bash scripts/check-cli-doc-drift.sh`。
- 运行命令:同上 + `cargo nextest run -p ralph-cli --bin ralph -- dag_`。

**10. Acceptance Red**:`dag_child_env_carries_full_artifact_refs` 在 U2 完成后、U3 实现前运行:子进程 grep 不到 key → exit 42 → 无 `forge.unit.reviewed` → 断言失败;失败原因 = env 缺 key(而非校验拒 spawn——该校验已在 U2 变绿),属真实行为 Red。

**11. 单元测试拆分**:序列化形状(BTreeMap 序确定 → JSON key 序确定,可做精确断言);prompt 块渲染;空 digest 防御(类型上不可能,不写)。不允许 mock:env 断言必须经真实子进程(env 传递是本行为本体)。

**12. Red → Green → Refactor 顺序**:
1. env 测试 Red → 修改点 D → Green;
2. prompt 测试 Red → 修改点 C(含既有 prompt 测试签名更新)→ Green;
3. 文档修订 → drift/capability 测试 Green;
4. Refactor:确认 `serde_json` 序列化与 `ArtifactRef` 的字段名(path/digest)即 env 契约,补一行结构注释。

**13. 最小实现范围**:修改点 C+D + 文档。必须保持:13 key 集合、D23 契约、既有 prompt 各段落文本(只允许追加证据块)。**不实现**:新 env key、preset instructions 修改、drift 盲区修复。

**14. 集成验证**:U2 全链路测试重跑(env 变更后仍绿);`cargo nextest run -p ralph-cli --test integration_dag_scheduler`。

**15. 风险驱动测试**:序列化 Round-trip(key 序确定性)已内建;无需更多。

**16. 回归范围**:`cargo nextest run -p ralph-cli --bin ralph -- dag`;`cargo nextest run -p ralph-core --features supervisor-db`;`cargo nextest run -p ralph-core --test ralph_tools_doc_drift`;`cargo nextest run -p ralph-core -- capability_inventory`;`bash scripts/check-cli-doc-drift.sh`;若改了 skills/ 下文件 → 其自带的 anchor 一致性由 capability_inventory 测试覆盖(同命令)。

**17. 预期文件变更**:

| 位置 | 变更类型 | 原因 | Evidence |
|---|---|---|---|
| crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs | 修改 | 修改点 C+D + 测试 | E3 |
| crates/ralph-core/data/ralph-tools.md | 修改 | 描述失实修复 | E16 |
| skills/ralph-preset-{author,review}/references/*.md | 条件修改 | 仅当描述失实 | E17 |

**18. 完成标准**:同前 + drift/capability/check-cli-doc-drift 三项静态门禁全绿 + 文档与行为逐句一致(人工核对)。

**19. 停止条件**:drift 测试锁的 key 集合必须变更才能过(说明误改了 key)→ 停下;skills 引用文件的内容与预期不符(E17 描述过时)→ 停下重查;prompt 追加破坏既有 prompt 断言且无法以纯追加解决 → 停下。

**20. 风险与注意事项**:
- R1:prompt 变长(每 job 多 ≤5 行)——token 影响可忽略,不量化测试。
- R2:env JSON 含绝对路径——属 runtime 内部注入(D23 overlay),不进 ledger、不进 inspect 输出(E14 的 sanitization 面不涉及 env),无泄漏面。
- R3(残留):`ralph_tools_doc_drift.rs` 12-key 清单缺 `EXPECTED_HEAD` 的既有盲区不在本计划修;记入 plan 残留说明。

## 8. Unit 串行依赖图

```
Unit 1(落库)→ Unit 2(消费校验)→ Unit 3(可见性)
```

- U2 依赖 U1 的 `record_stage_artifacts` / `latest_stage_artifacts` 与已落库的行;无 U1 则 U2 的 spawn 查询无数据源。不可交换。
- U3 依赖 U2 的"refs 为真"——U3 的 env Red 要求 spawn 先能通过校验,否则子进程根本不会启动,grep 断言无从谈起。不可交换。
- U1 独立完成且不改变任何 spawn 行为(纯增量写入),可独立提交回滚;U2 独立完成(占位删除 + fail-closed,env 仍是旧形状但无人消费,E17);U3 纯可见性。

## 9. 执行命令清单

| 命令 | 时机 | 目的 | 预期 | 失败可否继续 |
|---|---|---|---|---|
| `cargo nextest run -p ralph-core --features supervisor-db -- migrations` | U1 每步 | v24 迁移 + 版本断言 | 绿 | 否 |
| `cargo nextest run -p ralph-core --features supervisor-db -- dag_store` | U1 每步 | store 契约两后端 | 绿 | 否 |
| `cargo nextest run -p ralph-cli --bin ralph -- dag_artifacts` | U1 每步 | 记录接线 | Red→Green | 预期 Red 时可继续 TDD 循环 |
| `cargo nextest run -p ralph-cli --bin ralph -- dag` | U1/U2/U3 收尾 | spawn/driver 相邻全量 | 绿 | 否 |
| `cargo nextest run -p ralph-core --features supervisor-db` | 每 Unit 收尾 | core 全量(feature 显式) | 绿 | 否 |
| `cargo nextest run -p ralph-cli --test integration_dag_scheduler` | U2/U3 收尾 | CLI 平面回归(E14) | 绿 | 否 |
| `cargo run -p ralph-e2e -- --mock` | U2 收尾 | cassette 场景不受影响(E15) | 绿 | 否 |
| `cargo nextest run -p ralph-core --test ralph_tools_doc_drift` | U3 | 文档结构 | 绿 | 否 |
| `cargo nextest run -p ralph-core -- capability_inventory` | U3 | skill anchor 一致 | 绿 | 否 |
| `cargo build -p ralph-cli && bash scripts/check-cli-doc-drift.sh` | U3 | CLI flag 文档漂移 | 绿 | 否 |
| `cargo clippy --all-targets` / `cargo fmt --check` | 每 Unit 收尾 | Lint/格式 | 绿 | 否 |
| `./scripts/run-tests.sh` | 最终门禁 | 全量(nextest 两阶段 + doctest) | 绿 | 否 |

注意:全部测试入口为 nextest 系列(HARD RULE 1);本计划新增测试均不 spawn `ralph` 二进制(in-process runtime 或 sh fixture 子进程,D23 clear_env 保证不继承宿主 RALPH_*),集成测试文件沿用 `common::ralph_bin()` 既有 scrub(HARD RULE 5 已满足)。

## 10. 最终质量门禁

- 三个 Feature 的全部 Scenario 有对应测试且绿;R1-R7 每条至少一个可执行测试(§6 矩阵);
- 无新增跳过/`.only`/削弱断言;无 Snapshot/Golden 更新(本计划不涉及);
- 占位字符串 `"runtime-accepted"` / `"runtime-correction"` / `prior_event_ref` 全仓零残留(grep 实证);
- E13 点名既有测试全部原样绿(零回归面实证);
- `./scripts/run-tests.sh` 全绿;clippy/fmt 绿;
- v23 旧库升级测试绿;D23 env 契约测试(既有)绿;
- 文档(ralph-tools.md)与行为一致;AGENTS.md/CLAUDE.md 反向核对完成;
- 决策置信度未因实现发现跌破 0.85;无未处理 BLOCKED;
- 三个 Unit 各自完整 TDD 闭合并按序完成、各自可独立提交。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 每 Unit 落到具体文件/行号/测试名 |
| Executor 是否仍需做关键设计决策 | 否 | D1-D10 全部定案;映射表逐 key 固定(U2 §5) |
| 所有文件和接口是否有代码库证据 | 是 | E1-E19;新增文件均已标注"新增" |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | §3 最低 0.85(D4/D8/D10) |
| 是否存在未处理的低置信度假设 | 否 | §1 待验证假设:无 |
| 每个 Unit 是否只有一个可观察行为 | 是 | 落库 / 消费校验 / 可见性 |
| 每个 Unit 是否可以独立验证 | 是 | §7 各 Unit §14/§16 |
| 每个 Unit 是否有真实 Red | 是 | 各 §10(含新 API 编译 Red 的如实标注) |
| 每个 Unit 是否包含回归范围 | 是 | 各 §16 |
| 是否存在未来 Unit 依赖 | 否 | §8 依赖均指向前置已完成 Unit |
| 是否存在泛化任务描述 | 否 | 修改点 A-E 均带行号锚 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | §6 矩阵 |
| 所有关键决策是否有 Evidence | 是 | §3 支持证据列 |
| 计划是否可以严格串行执行 | 是 | §8 |
