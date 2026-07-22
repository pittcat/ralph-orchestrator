---
title: Ralph CLI 测试全并发安全化 - Plan
date: 2026-07-16
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-brainstorm
execution: code
origin: docs/plans/2026-07-16-005-refactor-ralph-cli-parallel-tests-plan.md
---

# Ralph CLI 测试全并发安全化 - Plan

## Goal Capsule

- **目标：** 让 `ralph-cli` 在 nextest 默认并发下稳定绿，并移除整包 `cli-serial`，缩短本地包级/全量反馈墙钟。
- **产品权威：** 本地开发者为主受益方；完成定义是「拿掉串行闸 + 并行稳定绿」，不设秒数 KPI。
- **无回归硬约束：** 只改测试基础设施与必要的 `#[cfg(test)]` 钩子；禁止 ignore/删测/弱化断言换绿；禁止借机改生产对外行为。
- **执行方式：** Unit 1 → Unit N 严格串行；每 Unit 内 TDD（Red → Green → Refactor）+ 范围回归后才能进入下一 Unit。

## Product Contract

### Summary

改造 `ralph-cli` 测试地基：固定 sleep → 事件驱动等待；fake-PATH / mock-ACP 等 process-global 夹具 → per-test 隔离；最后移除 `.config/nextest.toml` 对 `package(ralph-cli)` 的 `cli-serial` 整包强制串行。速度是结果，不是验收指标。

### Requirements（保留 brainstorm R1–R7）

- R1. mock ACP / fake-PATH 等跨测试共享状态必须 per-test 隔离，nextest 多进程并发下确定。
- R2. 固定 sleep 等待必须改为就绪信号或有界轮询；禁止加长 sleep 赌绿。
- R3. 完成后移除整包 `cli-serial`，`ralph-cli` 走默认并发。
- R4. 不得 ignore/删除/跳过/弱化既有回归护栏。
- R5. 不得借机改变生产对外行为；若测出生产缺陷，修复须单独可辨识。
- R6. 本地与 CI 共用同一并行 nextest 配置均须稳定绿。
- R7. 同步更新把「ralph-cli 必须整包串行」写成硬规则的文档。

---

### 1. 功能目标

**业务目标**

- 本地开发者跑 `ralph-cli` / 全量基线时，不再被「整包强制串行」拖慢。
- 在 nextest 默认并发下，`ralph-cli` 测试集稳定通过（无系统性 PoisonError / 夹具串台 / 时序 flake）。

**本次范围**

- `crates/ralph-cli` 测试代码与 `#[cfg(test)]` 夹具钩子。
- `.config/nextest.toml` 的 `cli-serial` 整包 override。
- `AGENTS.md` / `CLAUDE.md`、`docs/solutions/developer-experience/ralph-cli-loop-runner-tests-must-run-serial.md`、相关注释中的串行硬规则表述。

**非目标**

- 不为速度砍测试规模，或改「日常 targeted / 合入前全量」工作流。
- 不专项优化其他已并发 crate、doctest、e2e、web。
- 不设墙钟 KPI（例如必须压到 N 分钟）。
- 不把「先缩小串行 filter（方案 A）」作为最终态；最终态是整包可安全并发。

**已知约束和假设**

- 现行承重墙：`.config/nextest.toml` 将 `package(ralph-cli)` 划入 `cli-serial`（`max-threads = 1`）。
- 文档记载根因：4 个 process-global 量（`MOCK_ACP_EXECUTIONS` / `MOCK_ACP_EXECUTION_SERIAL` / `FAKE_PATH_BACKEND_SERIAL` / `FAKE_PATH_BACKEND_BIN`）+ 固定 sleep 在并行负载下 flake。
- 代码锚点（规划时已核实，实现前再扫一遍）：
  - `crates/ralph-cli/src/loop_runner/tests/fake_path.rs` — fake-PATH static + `install_fake_path_backends`
  - `crates/ralph-cli/src/loop_runner/wave/acp_mock.rs` — `MOCK_ACP_*` static
  - `crates/ralph-cli/src/loop_runner/tests/common.rs` — `install_mock_acp_executions`
  - `crates/ralph-cli/src/loop_runner/tests/{legacy,hooks,hard_gate}.rs` 与 `crates/ralph-cli/tests/agent_doc_sync_integration.rs` — 仍含 `thread::sleep`
  - `merge_queue` 生产路径注释已说明曾用 500ms sleep，部分已改为同步 wait（实现时以现存 sleep 清单为准，勿假设已清零）
- **假设（U1 必须验证）：** nextest 默认 process-per-test 下，Mutex 跨测共享不再成立；整包串行更可能由（a）sleep 在高并行 CPU 抢占下 flake、（b）仍存在的跨进程共享宿主资源、（c）历史「整包连坐」策略共同造成。U1 用并行试跑列出真实失败，禁止凭记忆只改 Mutex 表面。
- **假设：** `MOCK_ACP_EXECUTIONS` 队列当前可能几乎只被 test helper 写入/清空、生产 wave 路径未必再 pop（U1/U4 必须用引用扫描确认）；若已死代码，删除须有 differential 证明，不得 silently 留下 SERIAL 锁继续连坐。
- 测试入口硬约束：验证一律 `cargo nextest run`（`ralph-cli` 包级）；禁止裸 `cargo test -p ralph-cli` 作为完成声明。

---

### 2. BDD 行为规格

```gherkin
Feature: ralph-cli 测试可在 nextest 默认并发下安全运行
  作为本地开发者
  我想让 ralph-cli 测试不再整包强制串行
  以便在不削弱回归护栏的前提下缩短反馈时间

  Background:
    Given 仓库使用 cargo-nextest
    And 存在 .config/nextest.toml

  # —— 正常流程 ——
  Scenario: S1 移除整包 cli-serial 后 ralph-cli 全量并发通过
    Given package(ralph-cli) 不再被划入 cli-serial 强制串行
    When 执行 `cargo nextest run -p ralph-cli --bin ralph`
    Then 全部测试通过
    And 输出中不出现系统性 PoisonError 或 fake-PATH/mock 夹具串台失败

  Scenario: S2 含 ralph-cli 的 workspace 基线在并行配置下通过
    Given cli-serial 整包约束已移除
    When 执行 `./scripts/run-tests.sh`（或 CI 等价的 ci-rust-gate 测试段）
    Then nextest 阶段通过
    And 无因 ralph-cli 并发引入的新增失败

  # —— 夹具隔离 ——
  Scenario: S3 两个 fake-PATH 测试并行不串台
    Given 两个测试各自调用 install_fake_path_backends（或其后继 API）安装不同可执行文件
    When 它们在 nextest 默认并发下被同时调度
    Then 各自读到的 backend bin 目录与可执行内容互不污染
    And 任一测试 panic 不得导致另一测试因锁中毒失败

  Scenario: S4 mock ACP（或其后继）安装在并发下互不抢队列
    Given 两个测试各自安装不同的 mock ACP execution 序列
    When 它们在 nextest 默认并发下同时运行
    Then 各自消费的执行结果只来自本测试安装的序列
    And 不出现队列交叉或空队列误用

  # —— 时序 / 边界 ——
  Scenario: S5 原依赖固定 sleep 的测试在高并行负载下仍稳定
    Given 某测试曾用固定 sleep 等待文件/子进程/事件就绪
    And 该等待已改为有界事件驱动等待（轮询就绪或同步 join）
    When 在 nextest 高并行（默认 test-threads）下连续跑该测试子集多次
    Then 每次均通过
    And 失败时错误为超时/断言语义失败，而非「睡不够」类偶发

  Scenario: S6 事件驱动等待在超时边界失败可诊断
    Given 就绪条件在超时内不会满足
    When 运行使用该等待原语的测试（或针对等待 helper 的单元测试）
    Then 测试以明确超时原因失败
    And 不得无限挂起

  # —— 非法 / 禁止的「假绿」——
  Scenario: S7 禁止用 ignore/删测/弱断言换并行绿
    Given 改造前后的 ralph-cli 测试集合可对比
    When 审查为并行而改动的测试
    Then 不存在新增的 #[ignore]、无故删除的护栏测试、或明显弱化的断言
    And wave / fake-PATH / loop_runner 生产路径护栏仍存在

  # —— 状态 / 契约 ——
  Scenario: S8 开发者文档不再要求整包串行
    Given cli-serial 整包约束已移除且并行稳定绿
    When 阅读 AGENTS.md/CLAUDE.md Build & Test 与相关 solutions 文档
    Then 不再把「ralph-cli 必须整包 cli-serial」写成现行硬规则
    And 仍保留 nextest 入口与「禁止裸 cargo test 当默认」的正确指引（若仍适用则改写原因）

  # —— 失败恢复 ——
  Scenario: S9 并行试跑暴露的残留共享资源必须在放行前清零
    Given U1 并行试跑清单中存在失败用例
    When 后续 Unit 声称完成
    Then 清单中每条失败要么已修复并有复跑证据，要么有书面证明「非夹具/sleep 根因且已另开缺陷」且不阻塞移除 cli-serial
```

---

### 3. 验收与测试策略

| Scenario | 验收条件 | 推荐测试层级 | 是否需要 E2E |
| -------- | ---- | ------ | -------- |
| S1 | `package(ralph-cli)` 无整包 cli-serial；`cargo nextest run -p ralph-cli --bin ralph` 全绿 | 包级集成（nextest 全量） | 否（包级全量即本需求的「外层门禁」） |
| S2 | `./scripts/run-tests.sh` nextest 段绿 | workspace 基线 | 是（基线级，非浏览器 E2E） |
| S3 | 两测并行不串台；无 PoisonError 连坐 | 集成 + 并发重复跑 | 否 |
| S4 | mock 序列不交叉 | 集成 + 并发重复跑 | 否 |
| S5 | 原 sleep 测在高并行下 N 次通过（建议 N≥3） | 集成 + 并发压力 | 否 |
| S6 | 超时失败信息明确、不挂死 | 单元（wait helper） | 否 |
| S7 | 无 ignore/删测/弱断言；护栏仍在 | Characterization / diff 审查 + 测试计数对比 | 否 |
| S8 | 文档硬规则已改写 | 文档审查（可脚本 rg） | 否 |
| S9 | U1 失败清单清零或书面豁免 | 回归清单核对 | 否 |

**风险驱动加测（按需，不机械全上）**

- Characterization：U1 对现有串行绿做基线快照（测试名列表 + 通过）。
- Differential：夹具 API 替换前后，同一批 wave/fake-PATH 测试断言不变。
- Concurrency：移除 cli-serial 后对 `ralph-cli` 连续 nextest ≥3 次。
- 不默认上 Mutation / Fuzz / 浏览器 E2E（本需求是测试基建）。

---

### 4. 需求—测试追踪矩阵

| 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E/基线 |
| -- | -------- | ---- | ---- | ------- | --- |
| R1 | S3, S4, S9 | 并发夹具子集 nextest | 夹具 guard 生命周期单测（若抽出） | loop_runner wave/fake-PATH 既有测 | — |
| R2 | S5, S6 | 原 sleep 测点改写后的 nextest 子集 ×N | wait helper 超时/就绪单测 | late_events / hooks / merge 相关既有测 | — |
| R3 | S1, S2 | nextest.toml 断言/人工检查 + 全包 nextest | — | — | run-tests.sh |
| R4 | S7 | 测试集合 diff + 禁止 ignore 审查 | — | 既有护栏仍执行 | — |
| R5 | S7 | 生产 diff 审查（应近似为空或仅 cfg(test)） | — | 既有行为断言不变 | — |
| R6 | S1, S2, S5 | 本地并发 ×N；CI 同配置 | — | — | CI test job |
| R7 | S8 | rg 文档关键字审查 | — | — | — |

---

### 5. 严格串行开发单元

> 执行铁律：Unit N 的实现、测试、重构、回归全部完成并满足完成标准后，才允许开始 Unit N+1。禁止并行开发多个 Unit。  
> 每个 Unit 内必须走：写/启用验收测 → Red（正确原因）→ 最小单测 Red/Green/Refactor → 集成 → 回归 → 关闭。  
> 禁止：删断言、ignore、`.only`、无解释改 golden、Mock 掉待验行为、只跑局部就宣称完成。

---

#### Unit 1 — 并行失败表征与罪犯清单（Characterization）

- **Unit 目标：** 在不改生产逻辑、不提前移除 cli-serial 的前提下，建立「串行基线绿」证据，并用**受控并行试跑**列出真实失败清单，作为后续 Unit 的唯一事实源。
- **对应 Scenario：** S7（基线）, S9（清单）
- **外部可观察结果：**
  - 产出 `docs/plans/scratch/` 或 Unit 笔记中的：**串行通过证明**、**并行失败用例列表**（测试名 + 失败症状分类：Poison / 夹具串台 / 时序 / 其他）、**sleep 与全局 static 引用清单**。
  - 不修改 `.config/nextest.toml` 的默认 profile（试跑可用临时 override / 一次性命令，用完还原，不提交「半放开」）。
- **输入与输出：**
  - 输入：现行 `cli-serial` 配置；`crates/ralph-cli` 测试树。
  - 输出：罪犯清单文档（测试名级）；`rg` 汇总的 sleep / `FAKE_PATH_*` / `MOCK_ACP_*` 引用表。
- **可依赖的已完成能力：** 现有 nextest、`scripts/run-tests.sh`、现行串行配置。
- **明确禁止依赖的未来能力：** 不得依赖 U2+ 的 wait helper、新夹具 API、或已移除的 cli-serial。
- **验收测试：**
  - 串行：`cargo nextest run -p ralph-cli --bin ralph` 全绿（记录）。
  - 并行试跑：临时去掉整包 serial（或等价 `max-threads` 放开）跑同一命令，导出失败列表；试跑配置不入库。
- **需要拆分的单元测试：** 无（本 Unit 是表征，不新增生产行为）。
- **Red 预期失败原因：** 并行试跑**预期**出现失败或 flake；若意外全绿，记录「并行已绿」——后续 Unit 仍须完成 sleep/夹具根治与文档，但可缩短 U3–U5，**不得**跳过 S6/S7/R4。
- **最小实现范围：** 仅清单与引用扫描；可加一次性脚本但不作为产品功能提交。禁止「先改两个测试再说」。
- **集成验证：** 串行全包绿证明工作区干净。
- **回归范围：** 无代码回归；确保临时配置已还原。
- **完成标准：** 清单落盘；串行基线绿；static/sleep 引用表完整；S9 清单作为后续 Unit 的关闭依据。
- **风险与注意事项：** 并行试跑可能长时间占用 CPU——用 nextest 过滤子集迭代；勿在试跑中「顺手修」导致清单失真。

---

#### Unit 2 — 事件驱动等待原语（ATDD + TDD）

- **Unit 目标：** 提供可复用的测试侧等待原语（有界轮询/就绪谓词），覆盖「就绪通过」与「超时失败可诊断」，供后续替换固定 sleep。
- **对应 Scenario：** S6；为 S5 铺路
- **外部可观察结果：** 测试代码可调用统一 helper（例如 `wait_until(timeout, predicate)` / `wait_file_contains`）；超时返回明确错误；单测证明行为。
- **输入与输出：**
  - 输入：超时、谓词或路径条件。
  - 输出：`Ok(())` 或带超时信息的 `Err`；绝不无限阻塞。
- **可依赖的已完成能力：** U1 清单（优先服务清单中的时序类失败）。
- **明确禁止依赖的未来能力：** 不改 fake-PATH/MOCK_ACP API；不移除 cli-serial。
- **验收测试：** 新增 helper 的单元测试（S6）：谓词立即真 → 立刻成功；谓词始终假 → 在超时边界失败且含超时语义。
- **需要拆分的单元测试：**
  1. 就绪立即返回
  2. 超时失败
  3. （可选）谓词中途变真 → 成功且耗时 < timeout
- **Red 预期失败原因：** helper 尚未实现或仍用固定 sleep 伪装。
- **最小实现范围：** 仅测试支持模块（建议落在 `crates/ralph-cli/src/loop_runner/tests/` 或既有 `test_support`），**不改**业务生产路径。优先复用仓库已有 wait/poll 模式（实现前搜索），避免第二套原语。
- **集成验证：** helper 单测全绿；串行 `ralph-cli` 全包仍绿（未迁移旧测前不应变红）。
- **回归范围：** `cargo nextest run -p ralph-cli --bin ralph`（串行配置下）。
- **完成标准：** S6 满足；API 文档注释写清「禁止用加长 sleep 替代」。
- **风险与注意事项：** 轮询间隔过大变相 sleep；过小空转吵 CPU——选小间隔 + 总超时。

---

#### Unit 3 — 消除测试中的固定 sleep（按 U1 清单）

- **Unit 目标：** 将 U1 清单中「时序类」及全包 `thread::sleep` 测试等待改为 Unit 2 原语或同步 join/子进程 wait；并行负载下稳定。
- **对应 Scenario：** S5, S9（时序项）
- **外部可观察结果：** 目标测试文件中不再用固定 sleep 等待副作用就绪（允许模拟「延迟写入」的 producer 线程内短 sleep，但断言侧必须事件驱动）。
- **输入与输出：** 改写后的测试；行为断言与改前一致（differential）。
- **可依赖的已完成能力：** U1 清单、U2 helper。
- **明确禁止依赖的未来能力：** 不移除 cli-serial；不要求先完成夹具去 static（若某测同时依赖两者，本 Unit 只改等待侧）。
- **验收测试：**
  - 对每个改写测：串行下仍通过。
  - 对时序子集：临时并行 ×≥3 通过（配置不入库）。
- **需要拆分的单元测试：** 无新业务单测；若抽出文件轮询封装则补单测。
- **Red 预期失败原因：** 先把断言侧 sleep 删掉或改为极短超时 → 应因「未等就绪」失败；再接入 wait helper 变绿。
- **最小实现范围：** 按 U1 引用表逐文件改：`legacy.rs` / `hooks.rs` / `hard_gate.rs` / `agent_doc_sync_integration.rs` 等；`merge_queue` 若仍有测试侧 sleep 一并处理。生产代码仅当测试证明必须把「同步 wait」从测迁移到 API 时才动，且须单独说明（倾向保持生产已有同步 wait）。
- **集成验证：** 受影响测试名过滤的 nextest；然后串行全包。
- **回归范围：** `cargo nextest run -p ralph-cli --bin ralph`。
- **完成标准：** U1 时序类条目清零或仅剩「producer 模拟延迟」类；断言侧无固定 sleep 赌就绪；S5 证据留存。
- **风险与注意事项：** `recover_late_events*` 类测故意延迟写事件——保留 writer 延迟，断言侧用 wait/已有 poll；勿把业务超时常数改到不合理放大来藏 flake。

---

#### Unit 4 — fake-PATH per-test 隔离（去掉 process-global bin 单例）

- **Unit 目标：** 消除 `FAKE_PATH_BACKEND_SERIAL` / `FAKE_PATH_BACKEND_BIN` 跨测试共享语义；每个测试通过 guard/返回值持有自己的 bin 目录；并发下不串台、不中毒连坐。
- **对应 Scenario：** S3, S9（夹具类）
- **外部可观察结果：**
  - `install_fake_path_backends`（或替换 API）不再依赖进程级 `FAKE_PATH_BACKEND_BIN` 单槽。
  - `read_fake_path_backend_bin()` 全局读要么删除，要么改为从当前 guard/参数传入（调用点在 `tests/wave.rs` 等）。
- **输入与输出：** 测试传入 backends 列表 → 得到隔离的 bin 目录与可执行文件；Drop 清理本测文件。
- **可依赖的已完成能力：** U1（fake-PATH 相关失败项）、U2/U3（避免夹具问题与 sleep 问题缠绕）。
- **明确禁止依赖的未来能力：** 不移除 cli-serial；不阻塞在 MOCK_ACP（U5）。
- **验收测试：**
  - 既有 wave/fake-PATH 测试全部保持通过（differential）。
  - 新增或改编：两测并行安装不同 backend 名称/内容不串台（可用 nextest 两次进程级测 + 文档化并发证明；或同一进程内两 guard 若设计允许）。
  - panic 路径：一测 panic 不得毒死另一测（nextest 进程隔离下主要防「残留文件/固定路径」；若仍支持 `cargo test` 同进程，须无 PoisonError 连坐）。
- **需要拆分的单元测试：** guard Drop 清理；重复 install 同一测内行为（若有定义）。
- **Red 预期失败原因：** 先写「两隔离目录内容不同」验收测，在旧单槽 API 下失败或无法表达。
- **最小实现范围：** `tests/fake_path.rs` + 所有 `install_fake_path_backends` / `read_fake_path_backend_bin` 调用点。**默认不改**非 test 生产后端解析逻辑，除非现有生产代码错误地依赖 test static（实现前确认）。
- **集成验证：** `cargo nextest run -p ralph-cli --bin ralph -E 'test(wave) | test(fake)'`（过滤表达式以实现时 nextest 语法为准）+ 全包串行。
- **回归范围：** 全包 `ralph-cli` nextest。
- **完成标准：** 两 static 移除或降为非共享；S3 满足；U1 中 fake-PATH 类失败项清零。
- **风险与注意事项：** 旧注释曾要求「Mutex 字面形式不动」——本需求**显式废止**该约束；以并发安全为准。改 API 时保持测试可读性，避免巨型新框架。

---

#### Unit 5 — mock ACP 夹具隔离或确认删除死代码

- **Unit 目标：** 消除 `MOCK_ACP_EXECUTIONS` / `MOCK_ACP_EXECUTION_SERIAL` 的跨测试共享连坐；若队列已无生产消费方，则在 differential 证明下删除死代码与 SERIAL 锁，而不是留空壳。
- **对应 Scenario：** S4, S9
- **外部可观察结果：** 测试不再持有进程级 SERIAL 锁跨整个用例；若保留 mock 队列，则 per-test 注入（例如线程局部、guard 内队列、或显式参数传到 `#[cfg(test)]` 钩子）。
- **输入与输出：** 依 U1 引用扫描二选一：
  - **路径 A（仍有消费方）：** per-test 队列注入 + 生产 `#[cfg(test)]` 钩子改读当前测队列。
  - **路径 B（无消费方）：** 删除 static/install helper/无用测试安装调用；wave 测试改走现行真实依赖（多半是 fake-PATH）。
- **可依赖的已完成能力：** U1 扫描、U4（降低多夹具耦合）。
- **明确禁止依赖的未来能力：** 不移除 cli-serial（留给 U6）。
- **验收测试：**
  - 路径 A：S4 并发不交叉。
  - 路径 B：删除后既有 wave 测仍绿；全包测试计数不允许「静默少跑」（对比 U1 基线名录）。
- **需要拆分的单元测试：** 路径 A 下队列 FIFO/清空/Drop 清理单测。
- **Red 预期失败原因：** 路径 A 先写交叉污染验收；路径 B 先删消费侧编译失败或行为红，再删净。
- **最小实现范围：** `wave/acp_mock.rs`、`tests/common.rs` install helper、`tests/wave.rs` 等调用点；仅当存在真实 pop 钩子时改 `#[cfg(test)]` 生产旁路。
- **集成验证：** wave 相关 nextest + 全包串行。
- **回归范围：** 全包 `ralph-cli`。
- **完成标准：** 无进程级 MOCK_ACP SERIAL 连坐；S4 或路径 B 的名录不丢测；U1 mock 类条目清零。
- **风险与注意事项：** 勿在未证明死代码时盲目删除；路径 B 必须名录对比。

---

#### Unit 6 — 移除整包 cli-serial 并锁定并发稳定

- **Unit 目标：** 删除 `.config/nextest.toml` 中 `package(ralph-cli) → cli-serial` 的整包 override；在默认并发下稳定绿；清零 U1 清单。
- **对应 Scenario：** S1, S2, S5, S9
- **外部可观察结果：** nextest.toml 不再整包串行 `ralph-cli`；`cargo nextest run -p ralph-cli --bin ralph` 默认并发连续 ≥3 次通过；`./scripts/run-tests.sh` nextest 段通过。
- **输入与输出：** 配置变更；并发跑日志证据。
- **可依赖的已完成能力：** U2–U5 全部完成；U1 清单仅剩已修复项。
- **明确禁止依赖的未来能力：** 不把文档改写（U7）当作本 Unit 完成的一部分（可同 PR，但关闭顺序上本 Unit 先证明绿）。
- **验收测试：**
  - 配置：`rg`/`cat` 证明无整包 ralph-cli → cli-serial override（允许保留空 `test-groups` 定义作将来窄用，但不得再整包套用）。
  - 并发：`cargo nextest run -p ralph-cli --bin ralph` ×≥3。
  - 基线：`./scripts/run-tests.sh`（或文档等价入口）。
- **需要拆分的单元测试：** 可选：小测试或脚本断言 nextest 配置不含整包 filter（若仓库已有 presets/config 测试模式可跟随；**禁止**为配置加脆弱全文 snapshot）。
- **Red 预期失败原因：** 先移除 override → 若 U2–U5 未完成应出现 U1 类失败；完成后应变绿。
- **最小实现范围：** `.config/nextest.toml`；若 `integration_agent_reference.rs` 等注释写死 cli-serial，可在本 Unit 或 U7 改，但以「配置已放开」为准。
- **集成验证：** 全包并发 ×3；workspace run-tests。
- **回归范围：** `./scripts/run-tests.sh`；关注 CI 同配置（R6）。
- **完成标准：** S1/S2/S5/S9 满足；无新增 ignore；测试名录相对 U1 基线无护栏丢失（R4）。
- **风险与注意事项：** 若仅本地绿、CI flake，本 Unit **未完成**（R6）。发现新共享资源（端口、固定路径）必须当场修，不得「先合再看」。

---

#### Unit 7 — 文档与硬规则同步（无回归契约收口）

- **Unit 目标：** 同步 AGENTS.md/CLAUDE.md（保持二者一致）、solutions 文档、过时注释，使「现行规则」与并发现实一致，并明确保留的 nextest 入口纪律。
- **对应 Scenario：** S8
- **外部可观察结果：** 文档不再要求整包 cli-serial；说明已通过 per-test 隔离 + 事件驱动等待放开并发；仍强调用 nextest、禁止以裸 `cargo test -p ralph-cli` 作为默认入口（若同进程螺纹并行仍非一等公民，写清现状）。
- **输入与输出：** 文档 diff；`cp CLAUDE.md AGENTS.md`（或等价同步）。
- **可依赖的已完成能力：** U6 已绿。
- **明确禁止依赖的未来能力：** 无。
- **验收测试：**
  - `rg -n 'cli-serial|整包串行|必须单线程' AGENTS.md CLAUDE.md docs/solutions/developer-experience/ralph-cli-loop-runner-tests-must-run-serial.md` 人工审阅：无过时强制表述。
  - solutions 文首改为「历史原因 / 已修复」或归档指引，避免新人按旧法整包串行。
- **需要拆分的单元测试：** 无。
- **Red 预期失败原因：** 先写「文档不得再匹配旧硬规则句子」的检查清单，改前失败。
- **最小实现范围：** 仅文档与注释；不改测试逻辑。
- **集成验证：** 无。
- **回归范围：** 再跑一次 `cargo nextest run -p ralph-cli --bin ralph` 确认文档 PR 无夹带代码回退。
- **完成标准：** S8 满足；CLAUDE.md ↔ AGENTS.md 一致。
- **风险与注意事项：** 不要删掉「为何曾经串行」的历史知识——改成过去时 + 指向本计划。

---

### 6. 最终质量门禁

合并/宣布完成前必须全部满足：

- [ ] 所有计划内 Scenario S1–S9 有证据通过（日志、清单勾销或文档审查记录）
- [ ] Unit 1→7 按序关闭；无跳 Unit
- [ ] `cargo nextest run -p ralph-cli --bin ralph` 在**无整包 cli-serial**下连续 ≥3 次通过
- [ ] `./scripts/run-tests.sh` nextest 段通过（含 ralph-cli）
- [ ] 无新增 `#[ignore]` / 删除护栏测 / 弱化断言（相对 U1 基线名录）
- [ ] 生产对外行为无「只为并行」的夹带修改；若有生产修复，单独说明
- [ ] `cargo fmt` / `cargo clippy`（工作区惯例）通过
- [ ] `AGENTS.md` 与 `CLAUDE.md` 已同步；solutions 硬规则已改写
- [ ] CI 同 nextest 配置下无系统性 flake（R6）
- [ ] **未验证 / 剩余风险（必须显式写出）：**
  - 裸 `cargo test -p ralph-cli`（同进程多线程）是否作为一等公民支持——本计划默认 **nextest 为唯一一等入口**；若 U4/U5 已做到同进程安全，可在文档中降级风险，否则保持「不要用裸 cargo test 跑 ralph-cli」
  - 墙钟加速幅度未设 KPI，完成不承诺具体分钟数
  - 若 U1 发现非夹具/sleep 的宿主级共享（固定端口等），须在门禁备注中列出已修项

---

### Executor 速查

| 顺序 | Unit | 一句话 |
| --- | --- | --- |
| 1 | 表征 | 串行基线 + 并行罪犯清单 + sleep/static 引用表 |
| 2 | Wait helper | 有界事件驱动等待 + S6 单测 |
| 3 | 去 sleep | 断言侧等待改 helper；时序子集并行 ×N |
| 4 | fake-PATH | 去 process-global bin；S3 |
| 5 | mock ACP | 隔离或删死代码；S4 / 名录不丢 |
| 6 | 去 cli-serial | 配置移除 + 并发 ×3 + run-tests |
| 7 | 文档 | HARD RULE / solutions 改写；CLAUDE↔AGENTS 同步 |

**测试命令纪律：** 一律 `cargo nextest run -p ralph-cli --bin ralph`（及过滤子集）；最终 `./scripts/run-tests.sh`。禁止用裸 `cargo test -p ralph-cli` 宣布完成。
