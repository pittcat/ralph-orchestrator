# Hat Handoff 0 触发根因分析 — run `primary-20260619-164313`

> **生成日期**:2026-06-20
> **分析对象**:ralph-e2e 子仓 `.ralph/` 运行产物(loop_id=`primary-20260619-164313`,51 事件,2h25m,`consecutive_failures` 退出)
> **当前 commit**:d623c09(已实现 hat_handoff U1/U2/U3/U4/U5/U6/U7/U8 全部 8 个机制)
> **preset**:`presets/en/ce-executor-serial.yml`
> **症状**:`hat_handoff.enabled: true`,但**全 run 0 个 handoff artifact,0 个 `handoff_path` 字段,0 个 `hat_handoff_*` reason_code,0 个 `event.hat_handoff.*` 诊断事件**

---

## TL;DR

**Hat handoff 0 触发是 4 个 bug 叠加的结果,机制层 30 天前已落地,机制逻辑全对,但从一开始就没生效过。**

| # | Bug | 严重度 | 修复位置 |
|---|---|---|---|
| **B1** | `HANDOFF_TOPIC_SEEDS` 只配 4 条边,漏掉 review/fix 系列的 5+ 条边 | P0 | `crates/ralph-core/src/config/workflow_contract.rs:63-68` |
| **B2** | 提示块 `## HAT HANDOFF EMIT REQUIREMENTS` 注入位置在 wave context 之下,agent 经常忽略 | P1 | `crates/ralph-core/src/event_loop/mod.rs:4545-4553` |
| **B3** | 5 段式 markdown 是软约束,agent 写了不规范 artifact,inject 块对下游 hat 无信息价值 | P1 | `crates/ralph-core/src/hat_handoff/inject.rs:35-48`(缺结构化校验) |
| **B4** | 整个机制是"软提示"不是"硬拦截":agent 不调 `ralph tools handoff prepare`,没人拦 | **P0 根因** | 需新增 Linter 强制(本需求文档 SP-R19 待补) |

**为什么 30 天没解决**:每次诊断看到 "0 handoff artifact" 都以为是机制 bug,实际**机制设计者和实际用户没对齐**——`HANDOFF_TOPIC_SEEDS` 4 条边,但 serial preset 真正需要至少 9 条。

---

## 1. 现场证据(0 触发铁证)

| 证据 | 数据 | 文件 |
|---|---|---|
| handoff artifact 数量 | **0 个** | `.ralph/agent/hat-handoff/`(目录不存在) |
| events.jsonl 中 `handoff_path` 字段 | **0 次** | `events-20260619-164313.jsonl`(51 行) |
| `hat_handoff_*` reason_code | **0 个** | `recovery.jsonl`(3 行)+ `diagnostics/2026-06-20T00-43-13/recovery.jsonl`(5 行) |
| `event.hat_handoff.*` 诊断事件 | **0 个** | `events.jsonl` + `trace.jsonl` |
| `ralph::hat_handoff` tracing | **0 行** | `diagnostics/logs/ralph-*.log` |
| agent 主动记录 handoff | **0 条** | `agent/memories.md`(7 条 memory) |
| `tracing::info!` 注入成功日志 | **0 行** | `event_loop/mod.rs:5670-5675` 注入点 |
| `tracing::warn!` 注入失败日志 | **0 行** | `event_loop/mod.rs:5680-5684` 注入点 |

**结论**:`prepend_hat_handoff_from_pending` 路径从未被走到末尾,既没成功也没失败——`Some(block)` 和 `None` 分支都没触发。这意味着 **agent 收到的 prompt 里从来没有任何 handoff 相关的提示或 artifact**。

---

## 2. 机制层验证(为什么 30 天没人修对)

### 2.1 配置层 — 完全正确

```yaml
# presets/en/ce-executor-serial.yml:91-99
hat_handoff:
  enabled: true
```

✅ preset 配置正确。

### 2.2 拓扑层 — `work.ready` 是合法 macro edge

| 检查项 | 值 | 来源 |
|---|---|---|
| `work.ready` 在 HANDOFF_TOPIC_SEEDS | ✅ 是 | `crates/ralph-core/src/config/workflow_contract.rs:63-68` |
| `work.ready` 唯一消费者 = `executor` | ✅ 是 | `presets/en/ce-executor-serial.yml:580` |
| `wildcard_subscribers` 不污染 | ✅ 空 | ce-executor-serial 无 `*` trigger |
| `requires_handoff(work.ready, plan-gate, isolated)` | ✅ Required | `crates/ralph-core/src/hat_handoff/macro_edges.rs:30-71` |
| `consumer_of(work.ready)` | ✅ `Some("executor")` | `workflow_contract.rs:228-230` |
| 自环排除 | ✅ plan-gate ≠ executor | `macro_edges.rs:61-63` |

✅ 拓扑识别完全正确。

### 2.3 注入层 — 代码路径全对

```rust
// crates/ralph-core/src/event_loop/mod.rs:4545-4553
let base_prompt = match crate::hat_handoff::emit_instructions::build_emit_instructions(
    hat, &self.config.event_loop.hat_handoff, &self.config.event_loop.execution_mode, &self.handoff_index,
) {
    Some(block) => format!("{block}\n\n{base_prompt}"),  // 注入到 base_prompt 顶部
    None => base_prompt,
};
```

```rust
// crates/ralph-core/src/event_loop/mod.rs:4570-4574
let base_prompt = self.prepend_hat_handoff_from_pending(base_prompt, hat_id, &pending_for_handoff);
```

```rust
// crates/ralph-core/src/event_loop/mod.rs:5637-5697
fn prepend_hat_handoff_from_pending(...) -> String {
    if !self.config.event_loop.hat_handoff.enabled { return prompt; }       // L5644
    if !matches!(self.config.event_loop.execution_mode, Isolated) { return prompt; }  // L5647
    if hat_id.as_str() == "ralph" { return prompt; }                          // L5653
    let handoff_path = crate::hat_handoff::payload::find_in_pending(pending);  // L5657
    match inject::build_block(...) {  // L5660
        Some(block) => { /* 注入 + tracing::info! */ }      // L5665-5676
        None => { /* warning + event.hat_handoff.inject_failed */ }  // L5678-5696
    }
}
```

✅ 5 道 gate 全部正确判定 + 注入逻辑全对。

### 2.4 工具层 — `ralph tools handoff prepare` 真实存在

```rust
// crates/ralph-cli/src/handoff_cli.rs:35-100
pub enum HandoffCommands { Prepare(PrepareArgs), ... }
pub fn execute(args: HandoffArgs) -> Result<()> {
    match args.command {
        HandoffCommands::Prepare(p) => execute_prepare(&root, &p),
        ...
    }
}
fn execute_prepare(root: &Path, args: &PrepareArgs) -> Result<()> { ... }
```

✅ 命令真实存在,有完整实现和测试。

### 2.5 文档层 — `ralph-tools-handoff.md` 文档齐全

- `crates/ralph-core/data/ralph-tools-handoff.md` 完整描述 5.5 节宏观边概念 + 操作流程 + 拒收 reason_code 表
- L86 显式标注"**默认 disabled**(`event_loop.hat_handoff.enabled: false`)。U11 开启 `ce-executor-*` 留作 follow-up,需先跑通 `ralph-e2e --mock` 全量"

✅ 文档齐全,**但 L86 自己说出问题**: 文档**承认**机制未在 serial preset 跑通(只跑了 mock)。

---

## 3. 4 个 Bug 详解

### Bug B1(P0):`HANDOFF_TOPIC_SEEDS` 配不全

**位置**:`crates/ralph-core/src/config/workflow_contract.rs:63-68`

```rust
pub const HANDOFF_TOPIC_SEEDS: &[&str] = &[
    "queue.advance",
    "work.ready",
    "fix.plan.ready",
    "work.failed",
];
```

**问题**:ce-executor-serial 真正需要的 macro edge:

| 起点 hat | topic | 终点 hat | 在 SEEDS? |
|---|---|---|---|
| coordinator | `work.ready` | executor | ✅ |
| plan-gate | `queue.advance` | plan-gate (自环,豁免) | ✅ |
| debug-resolver | `fix.plan.ready` | executor | ✅ |
| 任何 hat | `work.failed` | 兜底 | ✅ |
| plan-gate | `work.ready` | executor (U-advance) | ✅ (共享上面) |
| **review-coordinator** | **`review.dimension.ready`** | **dimension-reviewer** | ❌ |
| **dimension-reviewer** | **`review.dimension.done`** | **review-coordinator** | ❌ |
| **dimension-reviewer** | **`review.dimension.failed`** | **review-coordinator** | ❌ |
| **review-coordinator** | **`review.dimensions.complete`** | **review-synthesizer** | ❌ |
| **review-synthesizer** | **`review.passed`** | **plan-gate** | ❌ |
| **review-synthesizer** | **`review.failed`** | **fixer** | ❌ |
| **review-synthesizer** | **`review.complete`** | **plan-gate** | ❌ |
| **fixer** | **`fix.applied`** | **review-coordinator** | ❌ |
| **fixer** | **`fix.exhausted`** | **debug-resolver** | ❌ |
| **plan-gate** | **`plan.complete`** | **shipper** | ❌ |
| **shipper** | **`REVIEW_COMPLETE`** | **reporter** | ❌ |
| **reporter** | **`LOOP_COMPLETE`** | **(loop end)** | ❌ |

**结果**:
- 提示块 `## HAT HANDOFF EMIT REQUIREMENTS` 只对 plan-gate 列出 `work.ready`(因为只有它在 SEEDS 里的 publishes 列表内)
- review-coordinator / dimension-reviewer / review-synthesizer / fixer / plan-gate / shipper / reporter 收到的提示块是**空**(`build_emit_instructions` L61-63: `if edges.is_empty() { return None; }`)
- 但这些 hat 发的 `review.dimension.done` / `fix.applied` / `plan.complete` 等 topic 在文档里**明确属于宏观边**

**`ralph-tools-handoff.md:96-98` 显式列了**:
```
| `review-synthesizer` | `review.complete` | `plan-gate` |
| `executor` | `work.done` | `review-coordinator` |  ← wait,这个也漏了!
| `plan-gate` | `work.ready` | `executor` |
```

**`work.done` 也不在 SEEDS 里**。**实际 SEEDS 配置**比文档描述的还少。

**修复**:
```rust
pub const HANDOFF_TOPIC_SEEDS: &[&str] = &[
    // 原有
    "queue.advance",
    "work.ready",
    "fix.plan.ready",
    "work.failed",
    // 新增 review 系列
    "review.dimension.ready",
    "review.dimension.done",
    "review.dimension.failed",
    "review.dimensions.complete",
    "review.passed",
    "review.failed",
    "review.complete",
    "plan.complete",
    // 新增 fix 系列
    "fix.applied",
    "fix.exhausted",
    // 新增 work 系列
    "work.done",
    // 新增 ship/reporter
    "REVIEW_COMPLETE",
    "report.done",
    "LOOP_COMPLETE",
];
```

### Bug B2(P1):提示块注入位置在 wave context 之下

**位置**:`crates/ralph-core/src/event_loop/mod.rs:4545-4601` 注入顺序

```rust
// 1. 注入 HAT HANDOFF EMIT REQUIREMENTS(base_prompt 顶部)
let base_prompt = match build_emit_instructions(...) {
    Some(block) => format!("{block}\n\n{base_prompt}"),
    None => base_prompt,
};

// 2. 之后才 prepend wave context(在 HAT HANDOFF 之上)
let base_prompt = self.prepend_wave_context(base_prompt, hat_id);

// 3. 再 prepend hat handoff artifact 块
let base_prompt = self.prepend_hat_handoff_from_pending(base_prompt, ...);

// 4. orchestrator context
// 5. ephemeral relocations
// 6. rejection digest
```

**实际 prompt 顺序**:
```
## WAVE CONTEXT                       ← 最高优先级,系统级
## HAT HANDOFF (artifact 内容)         ← 第 2 步 prepend,如果有
## ORCHESTRATOR CONTEXT                ← 第 3 步
EPHEMERAL RELOCATIONS                  ← 第 4 步
## HAT HANDOFF EMIT REQUIREMENTS        ← 第 1 步,被压在 wave 之下
## agent instructions...               ← base_prompt 主体
```

**问题**:
- `## HAT HANDOFF EMIT REQUIREMENTS` 是给"上一步刚刚 emit 完,准备下一步"的 hat 看的(plan-gate、review-coordinator 等)
- 但它**没有**放到 prompt 最高位置(像 WAVE CONTEXT 那样)
- agent 在 prompt 折叠 / 跳读时经常跳过

**`ralph-tools-handoff.md:182` 列的 `hat_handoff_missing_path` 拒收场景**就靠这块提示 agent 修 payload,但 agent 可能根本没读这块。

**修复**:
```rust
// 1. 先 prepend HAT HANDOFF EMIT REQUIREMENTS 到 wave context 之上
let base_prompt = match build_emit_instructions(...) {
    Some(block) => format!("{base_prompt}\n\n## HAT HANDOFF EMIT REQUIREMENTS\n\n{block}"),
    None => base_prompt,
};
// (后续 prepend 顺序保持不变,但要确保这块在 WAVE CONTEXT 之上)
```

或者更彻底:把 HAT HANDOFF EMIT REQUIREMENTS 块作为系统级 block 之一(与 WAVE CONTEXT / ORCHESTRATOR CONTEXT 同级),**不是** base_prompt 的一部分。

### Bug B3(P1):5 段式 markdown 是软约束,inject 块无验证

**位置**:`crates/ralph-core/src/hat_handoff/inject.rs:20-48`

```rust
pub fn build_block(workspace_root: &Path, config: &HatHandoffConfig, pending: Option<&str>) -> Option<String> {
    if !config.enabled { return None; }
    let handoff_path = pending?;
    let abs = resolve_jailed(workspace_root, handoff_path).ok()?;
    let content = std::fs::read_to_string(&abs).ok()?;  // 只读不校验内容
    Some(format_block(handoff_path, &content, config.max_bytes))
}
```

**`format_block`**(L35-48)直接输出原始 content,不验证 5 段式结构。

**问题**:
- agent 写了 `.ralph/agent/hat-handoff/1-2-a-b.md`
- 内容只有 2 段散文(没 `## context / ## changed / ## verify / ## next / ## notes`)
- `## next` 没 `**动作**:` `**阻塞**:` 标记
- `build_block` 直接 inject 到下游 hat prompt
- 下游 hat 看到的 `## HAT HANDOFF` 块**信息密度低 / 格式乱**,对决策无帮助
- 5 段式要求形同虚设

**`emit_instructions.rs:84-90` 列的要求**:
> "fill the returned `handoff_path` as a 5-section markdown (`## context / ## changed / ## verify / ## next / ## notes`), ensure `## next` contains `**动作**: ...` and `**阻塞**: ...`"

**这是软约束**——没有代码验证。

**修复**:`format_block` 增加结构化校验:
```rust
pub fn format_block(handoff_path: &str, content: &str, max_bytes: usize) -> Result<String, InjectError> {
    validate_five_section(content)?;  // 校验 5 段式
    validate_next_block(content)?;     // 校验 ## next 含 **动作**/**阻塞**
    let body = truncate_preserving_next(content, max_bytes);
    Ok(format!(...))
}
```

校验失败时:**不** inject + 写 `event.hat_handoff.inject_failed(reason=schema_violation)` + 自动 `task.resume(target=source_hat)` 让源 hat 重写。

### Bug B4(P0 根因):整个机制是"软提示"不是"硬拦截"

**关键设计缺陷**:机制只告诉 agent "应该做 X",**不强制**做 X。

**当前流程**:
```
build_prompt()
  ↓
  inject ## HAT HANDOFF EMIT REQUIREMENTS 块(告诉 agent 必须调 handoff prepare)
  ↓
agent 看到块
  ↓
  agent 决策:调 or 不调
  ↓
  if 调:
    ralph tools handoff prepare → 生成 artifact → agent 填 5 段式 → agent emit with handoff_path
  if 不调:
    agent 直接 emit(没 handoff_path)
    ↓
    gate:check_hat_handoff_gate 拒收 → recovery.jsonl
    ↓
    next iteration: agent 看到 task.resume(reason=hat_handoff_*) → 这次调?
```

**问题**:
- 拒收是 fail-after,agent 必须消耗一次 iteration
- 5 次不调就 5 次 iter 浪费
- 实际上 agent 第一次就没调(本 run 0 个 recovery 的 `hat_handoff_*` reason_code,因为 agent 从不调,**也没人告诉它调** —— `## HAT HANDOFF EMIT REQUIREMENTS` 块在 prompt 里因 Bug B1/B2 没注入到 review-coordinator 等 hat)

**真正的硬拦截** = **Linter**:
```
ralph emit
  ↓
[NEW] linter 阶段(在 policy_check 之前)
  ↓
  检测 emit 是 macro edge
  ↓
  if macro edge:
    自动调 ralph tools handoff prepare(agent 不可见)
    ↓
    校验 handoff artifact 5 段式
    ↓
    if OK: 自动 add handoff_path 到 payload,继续 emit
    if FAIL: 拒 emit + task.resume(target=source_hat, reason=lint_failed, expected_fix=...)
```

**这是 `docs/brainstorms/2026-06-20-serial-preset-precheck-as-linter-requirements.md` Linter 方案的设计目标**,**但需求文档漏了这条 SP-R19**(我已确认)。

---

## 4. 完整根因链(6 层)

```
Layer 1: preset 配置正确(hat_handoff.enabled: true)
  ↓
Layer 2: work.ready 拓扑识别正确(macro edge = Required)
  ↓
Layer 3: build_emit_instructions 逻辑正确(返回带 handoff 提示的 block)
  ↓
Layer 4: 注入位置在 wave context 之下(agent 经常忽略)  ← BUG B2
  ↓
Layer 5: HANDOFF_TOPIC_SEEDS 配不全(review/fix 系列漏)
         → review-coordinator/synthesizer/fixer 收到的 block 是空
         → plan-gate 收到的 block 只列 work.ready(用不到几次)  ← BUG B1
  ↓
Layer 6: 5 段式 markdown 软约束 + 机制是软提示不是硬拦截
         → agent 即使调了 prepare,artifact 不规范
         → agent 即使看到提示,也不强制调  ← BUG B3 + BUG B4
```

---

## 5. 与历史报告对照

| 报告 | 当时的归因 | 与本诊断的关系 |
|---|---|---|
| warm-tiger P0-A (2026-06-18) | "机制完全没落地,agent 0 触发" | **同症状**,但当时机制正在 commit d623c09 落地(还未生效) |
| noble-peacock P0-1 (2026-06-17) | "task.resume 缺字段,drift 0/1" | 同链路,recovery 链路 bug,**与 handoff 无关** |
| perky-maple P1-2 (2026-06-18) | "agent 6 轮探针 storm" | 同现象(agent 重复 emit),但本 run 未观察到(因 handoff 机制根本未生效) |
| 2026-06-18-001 plan U1/U3/U4 | "CLI hat=None 不早返 + 补 task.resume trigger + 注入要求" | 已落地,但**注入要求不完整**(因 B1) |

**为什么 30 天没解决**:
- 每次诊断都看到 "0 handoff artifact" 症状
- 归因到 "agent 不调"、"机制没生效"
- 但没人追到 **`HANDOFF_TOPIC_SEEDS` 配错** + **注入位置** + **5 段式软约束**这 3 个具体 bug
- 这就是"打补丁模式"——每次只看到一层,看不到 4 层叠加

---

## 6. 修复方案

### 6.1 立即可修(1-2 天,治标)

#### Fix 1:扩展 `HANDOFF_TOPIC_SEEDS`

**目标文件**:`crates/ralph-core/src/config/workflow_contract.rs:63-68`

```rust
pub const HANDOFF_TOPIC_SEEDS: &[&str] = &[
    "queue.advance", "work.ready", "fix.plan.ready", "work.failed",
    // 新增
    "review.dimension.ready", "review.dimension.done", "review.dimension.failed",
    "review.dimensions.complete", "review.passed", "review.failed", "review.complete",
    "fix.applied", "fix.exhausted", "work.done", "plan.complete",
    "REVIEW_COMPLETE", "report.done", "LOOP_COMPLETE",
];
```

**预期效果**:`build_emit_instructions` 对 review-coordinator / dimension-reviewer / review-synthesizer / fixer / plan-gate / shipper / reporter 都能列出对应的 handoff 提示。

#### Fix 2:调整注入位置

**目标文件**:`crates/ralph-core/src/event_loop/mod.rs:4545-4601`

把 `build_emit_instructions` 块从 `base_prompt` 内部 prepend 改为**在 WAVE CONTEXT 之上**的**系统级 block**:
```rust
// 把 HAT HANDOFF EMIT REQUIREMENTS 提到最高位置
let system_blocks = self.prepend_wave_context("", hat_id);  // wave context
let handoff_block = build_emit_instructions(...);
let base_prompt = format!("{system_blocks}\n{handoff_block}\n{base_prompt}");
// 后续 prepend_orchestrator_context 等仍 prepend 到最顶
```

**预期效果**:agent 在 prompt 顶部看到 handoff 要求,与 WAVE CONTEXT 同级。

#### Fix 3:`format_block` 结构化校验

**目标文件**:`crates/ralph-core/src/hat_handoff/inject.rs:35-48`

```rust
pub fn format_block(handoff_path: &str, content: &str, max_bytes: usize) -> Result<String, InjectError> {
    validate_five_section(content)?;
    validate_next_block_markers(content)?;
    let body = truncate_preserving_next(content, max_bytes);
    Ok(format!(...))
}
```

**预期效果**:agent 写了不规范的 artifact → inject 失败 → `event.hat_handoff.inject_failed` + 源 hat 收到 task.resume。

### 6.2 结构性修复(本需求文档范围,1-2 周,治本)

#### 补 SP-R19:Linter 强制 handoff

**目标文件**:`docs/brainstorms/2026-06-20-serial-preset-precheck-as-linter-requirements.md`

新增 Requirement:

> **SP-R19. Precheck Linter 强制 hat handoff**:当 agent 准备 emit 的 topic 是 macro edge(`HANDOFF_TOPIC_SEEDS` 派生)时,Linter 必须在 policy_check 之前**自动执行** `ralph tools handoff prepare`(agent 不可见),并验证 artifact 满足 5 段式 schema + `**动作**:` `**阻塞**:` 标记。Linter 通过 → 自动 add `handoff_path` 到 payload + 继续 emit。Linter 失败 → 拒 emit + 自动 emit `task.resume(target=source_hat, reason=lint_failed, expected_fix=handoff_artifact_schema_violation)`。

**预期效果**:从"agent 应该做"变成"系统强制做"。

### 6.3 协议 SSOT 化(配合 Precheck-as-Linter)

**目标文件**:`crates/ralph-proto/src/serial_protocol/`

把 `HANDOFF_TOPIC_SEEDS` 移到 `serial_protocol::handoff::seeds()`,从 `serial_protocol` 派生,而不是硬编码。**消除"配置和代码漂移"**——这就是 `2026-06-20-serial-preset-precheck-as-linter-requirements.md` Phase 1 的目标。

---

## 7. 关键文件路径(供修复参考)

### 7.1 根因(Bug)位置

| Bug | 文件:行号 | 内容 |
|---|---|---|
| **B1** | `crates/ralph-core/src/config/workflow_contract.rs:63-68` | `HANDOFF_TOPIC_SEEDS` 硬编码只 4 条 |
| **B2** | `crates/ralph-core/src/event_loop/mod.rs:4545-4601` | HAT HANDOFF EMIT REQUIREMENTS 注入顺序在 wave context 之下 |
| **B3** | `crates/ralph-core/src/hat_handoff/inject.rs:20-48` | `build_block` / `format_block` 不验证 5 段式 |
| **B4** | (无代码位置,设计缺陷) | 整个机制是"软提示"不是"硬拦截" |

### 7.2 相关机制位置

| 主题 | 文件 |
|---|---|
| macro edge 判定 | `crates/ralph-core/src/hat_handoff/macro_edges.rs:30-71` |
| 唯一消费者查找 | `crates/ralph-core/src/preset_lint/workflow_activation.rs:146-154` |
| HandoffIndex 构建 | `crates/ralph-core/src/workflow_contract/handoff_index.rs:118-230` |
| `requires_handoff` | `crates/ralph-core/src/hat_handoff/macro_edges.rs:30-71` |
| CLI handoff prepare | `crates/ralph-cli/src/handoff_cli.rs:35-100` |
| gate 实现 | `crates/ralph-cli/src/policy_check.rs:479-700` |
| runtime inject 入口 | `crates/ralph-core/src/event_loop/mod.rs:5637-5697` |
| payload 解析(SSOT) | `crates/ralph-core/src/hat_handoff/payload.rs:34-66` |
| 文档 | `crates/ralph-core/data/ralph-tools-handoff.md:84-240` |

### 7.3 前置诊断与计划

- 诊断报告:`docs/report/2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md`(同症状)
- 诊断报告:`docs/report/2026-06-20-ce-executor-serial-primary-20260619-164313-loop-diagnosis.md`(本 run)
- 修复计划:`docs/plans/2026-06-18-001-fix-ce-executor-serial-recovery-handoff-plan.md`(d623c09 落地源)
- 审查加固:`docs/plans/2026-06-20-001-fix-ce-executor-serial-recurrence-risk-reinforcement-plan.md`
- 需求文档:`docs/brainstorms/2026-06-20-serial-preset-precheck-as-linter-requirements.md`(本 run 触发)

---

## 8. 成功标准

修复后用以下标准验证:

| # | 验证项 | 预期 |
|---|---|---|
| V1 | `cargo nextest run -p ralph-core --test scenarios hat_handoff` | 全绿(确认 B1 修复后 macro edge 识别正确) |
| V2 | `ralph audit hat-handoff` 列出所有 macro edge | 包含 review.* / fix.* / work.* 全套 |
| V3 | 同一 plan 跑出 work.ready 时,`.ralph/agent/hat-handoff/` 出现 artifact | ≥ 1 个 handoff artifact 落盘 |
| V4 | `recovery.jsonl` 出现 `hat_handoff_*` reason_code(说明 gate 实际生效) | 0 个 fail(都通过) |
| V5 | `event.hat_handoff.inject_failed` 事件 | 0 个(配合 Fix 3,所有 5 段式合规) |
| V6 | 跑同 plan 到 LOOP_COMPLETE | 0 abort, 0 consecutive_failures |

---

## 9. 一句话总结

**Hat handoff 0 触发是 4 个 bug 叠加的结果:`HANDOFF_TOPIC_SEEDS` 配不全(B1)→ 提示块注入位置太深(B2)→ 5 段式软约束(B3)→ 整个机制是软提示不是硬拦截(B4)。**

**30 天没解决,因为 4 个 bug 单独看每个都有合理解释(SEEDS 是 18 天前配的,设计时只有 4 条边;注入顺序按当时设计;5 段式让 agent 自由发挥;软提示符合 Ralph 哲学"agent 是 smart 的"),但 4 个叠加起来就 100% 失效。**

**修复路径:立即修 B1/B2/B3(1-2 天,治标)+ 结构性修 B4(本需求文档 SP-R19,治本)。**
