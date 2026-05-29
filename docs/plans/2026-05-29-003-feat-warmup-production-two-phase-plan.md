---
title: feat: Warmup/Production 两阶段循环支持
date: 2026-05-29
status: active
origin: docs/brainstorms/2026-05-29-harness-calibration-loop-requirements.md
type: feat
---

# Warmup/Production 两阶段循环实现计划

## 问题概述

AutoResearch 循环缺少显式的 Warmup（校准）和 Production（正式）阶段划分。Harness Hat 的修复后移到实验循环中消耗实验轮次，Harness 专用能力（Repair Budget、状态机、版本追踪）在修复期间完全闲置。

详见需求文档 `docs/brainstorms/2026-05-29-harness-calibration-loop-requirements.md`（R1-R13, KD1-KD5, F1-F2）。

---

## 范围边界

### 范围内
- Ralph 上游：EventLoopConfig 增加 phase 支持、HatConfig 增加阶段感知触发、事件分发阶段过滤
- **PhaseWatcher（Ralph loop_runner 内部）**：在 `experiment.evaluated` 后自动检查退出条件并触发过渡
- **Agent scratchpad 阶段注入**：Ralph 构建 agent context 时自动注入当前阶段信息
- **Event `_phase` metadata**：每行 events JSONL 携带发生时的阶段
- pre_run_smoke.py：增加 `--check-only` 标志 + JSON 结构化输出
- **check_exit_conditions.py（新）**：确定性退出条件检查，供 PhaseWatcher 调用
- generate_autoresearch.py：warmup 配置生成、support scripts 复制
- hat-contracts.yml：experiment.attacked 的 `n` 字段；phase-aware triggers 合约
- hat-harness.md：Warmup 全开放模式与 Production 标准模式双行为（pre_run_smoke "修复后验证"而非"每轮执行"）
- hat-evaluator.md：Warmup 期间 attack_target 路由绕过
- validate_config.py / audit_config.py / runtime_audit.py：阶段感知校验
- 过渡脚本：transition_warmup_to_production.py（幂等 6 步过渡，含 Step 0 drain）
- **stop_on_exit 支持**：配置选项 `stop_on_exit: true` + CLI `--warmup-only` 标志
- **启动阶段检测**：loop_runner 启动时检测 phase.json 中的 `warmup_completed` 标记，跳过 warmup
- Review Skill 同步（finding_detector.py contract 映射 + combox warmup 适配 + finding 严重度调制）
- Report Skill 同步（analyzer.py 阶段感知、collector.py 阶段字段）

### 范围外（推迟）
- 跨循环 Warmup 历史对比分析
- 分布式/多 Agent 场景下的阶段同步
- Warmup 阶段的 UI/日志差异化展示

---

## 关键技术决策

### KTD1. Ralph 层阶段感知（上游修改）

**决定**：Ralph 上游需要改动以原生支持阶段概念，而不是纯 Skill 层约定。

改动点：
1. **EventLoopConfig** — 增加 `phase_config: Option<PhaseConfig>`，包含 `initial: Phase`（warmup|production）和可选的 `transition_event`
2. **HatConfig** — 增加 `phase_triggers: Option<HashMap<String, Vec<String>>>`，按阶段名映射触发事件列表
3. **HatRegistry** — `subscribers()` / `get_for_topic()` 检查当前阶段，使用 `phase_triggers` 覆盖 `triggers`
4. **Phase 状态持久化** — 在 loop state 中记录当前阶段，写入 `.ralph/agent/phase.json`
5. **Phase 转换** — 监听 `phase.transition` 事件，解析 payload 中的目标阶段，动态更新订阅匹配

**为什么不是纯 Skill 层**：Hat 指令中写 if/else 分支不可靠，且与 Ralph 的触发机制解耦——修改后的 Ralph 在 isolated 模式下能正确路由事件，不需要 Agent 的 Hat 指令去"猜"当前阶段。

### KTD2. `n` 字段统一

`attack_target` → `n`，匹配现有代码（`hat-harness.md` 第 25 行、`runtime_audit.py` 中的 `n` 查找）。在 `hat-contracts.yml` 中将 `n` 加入 `experiment.attacked` 的 `optional_fields`。

### KTD3. 过渡前 Drain 当前实验

退出条件满足后，不立即执行过渡。等待当前 task_key 的完整实验链（`experiment.planned` → `experiment.evaluated` / `experiment.blocked`）完成后才开始过渡。防止半截实验污染数据。

### KTD4. 过渡脚本幂等

`transition_warmup_to_production.py` 每步执行前写 checkpoint，崩溃后重跑跳过已完成步骤。不做事务性保证（Ralph 无事务机制），但 checkpoint 使恢复非模糊。

### KTD5. stop_on_exit：PhaseWatcher 层优雅停止

**决定**：Warmup 退出后停止循环的能力由 Ralph 的 PhaseWatcher 直接处理，不是过渡脚本的责任，也不是通用事件机制。

- `stop_on_exit: true`（配置）或 `--warmup-only`（CLI）控制此行为
- PhaseWatcher 在退出条件满足后判断此标志→执行完整过渡脚本→标记 phase.json 为 `production` + `warmup_completed: true`→优雅停止循环
- 下次启动时检测 phase.json 中的 `warmup_completed` 标记，跳过 warmup，直接以 produciton 模式启动
- 默认 `stop_on_exit: false`（向后兼容，现有行为不变）
- 优先级：CLI `--warmup-only` > 配置 `stop_on_exit` > 默认 false
- 客户端重跑 Warmup 方式：手动删除 `warmup_completed` 标记或传递 `--force-warmup`

**为什么不是过渡脚本层停止**：过渡脚本是 Python 子进程，它无法控制 Ralph 主循环的生命周期。由 PhaseWatcher（Ralph loop_runner 内部）判断并控制循环退出是唯一干净的实现路径。

---

## 高层面技术设计

### Phase 感知的 Hat 触发流程

```
Ralph 启动 → 读取 event_loop.phase_config.initial → 写入 .ralph/agent/phase.json (phase: warmup)
  → HatRegistry::subscribers() 检查当前 phase
  → Harness Hat 在 warmup 阶段订阅 ["experiment.start"] 而非 ["harness.blocked"]

每轮迭代:
  experiment.start 发布 → EventBus 路由到 subscribers → Harness Hat (warmup phase) 收到事件
  → Harness Hat 检查修复 → 发布 harness.initialized / harness.repaired / harness.blocked

退出条件满足时:
  Transition 脚本执行 → 最后发布 phase.transition {to: production}
  → Ralph 接收事件 → 更新 phase.json → 更新 HatRegistry 订阅
  → 后续事件按 production 阶段路由
```

### Ralph 配置结构变化

```yaml
# EventLoopConfig 新增
event_loop:
  phase_config:
    initial: warmup           # warmup | production
    transition_event: phase.transition  # 触发阶段切换的事件 topic

# HatConfig 新增 phase_triggers
hats:
  harness:
    triggers: []              # 当 phase_triggers 存在时作为 fallback
    phase_triggers:
      warmup: ["experiment.start"]
      production: ["harness.blocked"]
    publishes: [...]          # 不变

# WarmupConfig 新增 stop_on_exit
event_loop:
  phase_config:
    initial: warmup           # warmup | production
    transition_event: phase.transition  # 触发阶段切换的事件 topic
  warmup_config:
    min_iterations: 10
    max_iterations: 30
    exit_quiet_rounds: 3
    stop_on_exit: false       # NEW — 默认 false，true 时 warmup 完成后停止

# 生成的 autoresearch.yml 顶层结构
event_loop:
  ...
  phase_config:
    initial: warmup
    transition_event: phase.transition
  warmup_config:
    min_iterations: 10
    max_iterations: 30
    exit_quiet_rounds: 3
    stop_on_exit: false       # --warmup-only 时生成 true

hats:
  harness:
    triggers: []
    phase_triggers:
      warmup: ["experiment.start"]
      production: ["harness.blocked"]
    ...
```

### PhaseWatcher 自动检查序列

```
Ralph loop_runner 内部 PhaseWatcher (Rust):
  监听每次事件发布完成
  如果 event.topic == "experiment.evaluated" && current_phase == warmup:
    → spawn 子进程 check_exit_conditions.py --project-root ... --output-json ...
    → 读取子进程 JSON 输出
    → exit 码 0（全部条件满足）:
        → 判断: stop_on_exit == true? 
        → 如果 true（Warmup Only 模式）:
            → spawn 子进程 transition_warmup_to_production.py --project-root ... --stop
            → 等待子进程完成
            → 设置 phase = production + warmup_completed = true
            → 更新 phase.json + HatRegistry
            → 发布 warmup.complete {phase: production, reason: "warmup_completed"}
            → 优雅停止循环（break event loop，不报错退出）
        → 如果 false（标准两阶段模式）:
            → spawn 子进程 transition_warmup_to_production.py --project-root ...
            → 等待子进程完成
            → 设置 phase = production
            → 更新 phase.json + HatRegistry
            → 发布 harness.initialized {phase: production}
            → 继续下一轮循环（进入 Production 阶段）
    → exit 码 1（有未满足条件）:
        → 日志记录哪些条件未满足
        → 继续下一轮循环（不阻塞）
    → exit 码 42（需要 drain）:
        → 日志记录"有进行中的实验，等待下一轮"
        → 继续下一轮循环
```

### Phase 转换序列（完整事件流）

```
标准两阶段模式（stop_on_exit: false）:
  experiment.evaluated 发布 → PhaseWatcher 捕获
    → exit 0 → 过渡脚本执行:
        0. 检查 drain 状态（events JSONL 有无未闭合链）
        1. 标记 superseded (扫描 events JSONL, 更新 measurement_status)
        2. 保留方向记录 (从 strategy_state.json 提取 task_key/hypothesis/decision)
        3. 重建 baseline (运行 baseline 命令)
        4. 重置 strategy_state.json (清空 directions, 恢复 _original_initial_state)
        5. 递增 harness-version.json 主版本
    → 发布 phase.transition {to: production, version: 2}
    → Ralph EventBus 接收 → 更新 phase.json → 更新 HatRegistry 订阅
    → 发布 harness.initialized {phase: production, harness_version: 2}
    → Production 阶段开始

Warmup Only 模式（stop_on_exit: true）:
  experiment.evaluated 发布 → PhaseWatcher 捕获
    → exit 0 → 判断 stop_on_exit == true
    → 过渡脚本以 --stop 执行（5 步完成后不发布 phase.transition）:
        0. 检查 drain 状态
        1. 标记 superseded
        2. 保留方向记录
        3. 重建 baseline
        4. 重置 strategy_state.json
        5. 递增 harness-version.json 主版本
        6. (代替 phase.transition) 写入 phase.json {phase: "production", warmup_completed: true}
    → 发布 warmup.complete {phase: production, reason: "warmup_completed"}
    → PhaseWatcher 收到脚本完成 → 优雅停止循环

下次启动 (继承 warmup_completed):
  Ralph 启动 → 读取 phase.json
    → 发现 phase == "production" && warmup_completed == true
    → 跳过 warmup 初始化
    → 以 phase_config.initial = production 模式启动
    → 发布 harness.initialized {phase: production, harness_version: 2}
    → Production 阶段开始
```

### 文件变化总览

```
universal-autoresearch/             ralph-orchestrator/
  skills/                            crates/
    uni-autoresearch/                  ralph-core/src/
      scripts/                           config.rs      # EventLoopConfig + HatConfig 扩展 + WarmupConfig.stop_on_exit
        generate_autoresearch.py ✅      event_loop/
        pre_run_smoke.py        ✅         mod.rs        # HatRegistry 阶段感知 + PhaseWatcher
        check_exit_conditions.py NEW    hat_registry.rs # subscribers() 阶段过滤
        transition_warmup_to_          ralph-proto/src/
          production.py        NEW        event_bus.rs   # publish() 阶段过滤 + _phase metadata
      assets/                        ralph-cli/src/
        hat-contracts.yml     ✅         loop_runner.rs # PhaseWatcher + phase持久化 + scratchpad注入
      references/                                      #   + stop_on_exit 分支 (U4)
        hat-harness.md        ✅                      #   + 启动阶段检测 (U14)
        hat-evaluator.md       ✅
    uni-autoresearch-review/
      SKILL.md                ✅
      scripts/
        finding_detector.py   ✅
    uni-autoresearch-report/
      scripts/
        analyzer.py           ✅
        collector.py          ✅
  tests/ (regression fixtures)
    run_finding_regression.py ✅
```

---

## 实现单元

### U1. Ralph: EventLoopConfig 增加 phase_config

**Goal**: 让 Ralph 原生支持阶段配置，不再静默忽略阶段相关字段

**Requirements**: R2（event_loop 支持 warmup 对象）

**Dependencies**: 无（Ralph 最底层）

**Files**:
- `crates/ralph-core/src/config.rs` — 修改 `EventLoopConfig`
- 新增 `PhaseConfig` struct：`Phase` enum（Warmup/Production）

**Approach**:
1. 在 `config.rs` 定义 `Phase` enum：`#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]`，值 `Warmup`（序列化 `"warmup"`）和 `Production`（序列化 `"production"`）
2. 定义 `PhaseConfig` struct：
   ```rust
   pub struct PhaseConfig {
       pub initial: Phase,
       #[serde(default = "default_transition_event")]
       pub transition_event: String,  // 默认 "phase.transition"
   }
   fn default_transition_event() -> String { "phase.transition".into() }
   ```
3. `EventLoopConfig` 增加 `pub phase_config: Option<PhaseConfig>`
4. 在 `EventLoopConfig::validate()` 中（如有）增加 phase 一致性检查：warmup 时必须有 `enable_harness_extensions`（skill 层保证，可选）

**Test scenarios**:
- 解析含 phase_config 的 YAML，验证 Phase 枚举正确反序列化
- 解析不含 phase_config 的 YAML，验证 `phase_config` 为 `None`，向后兼容
- 验证 `transition_event` 有合理默认值

**Verification**: 单元测试覆盖 YAML 序列化/反序列化、向后兼容

### U2. Ralph: HatConfig 增加 phase_triggers

**Goal**: 支持 Hat 在不同阶段有不同的触发事件

**Requirements**: R4（Warmup 自动激活）、R11（Production 标准触发）

**Dependencies**: U1（需要 Phase 类型）

**Files**:
- `crates/ralph-core/src/config.rs` — 修改 `HatConfig`

**Approach**:
1. `HatConfig` 增加 `pub phase_triggers: Option<HashMap<String, Vec<String>>>`
2. `trigger_topics()` 方法扩展：如果 `phase_triggers` 存在，返回所有阶段的 topics 并集（用于注册时的完整 topic 集合，实际过滤在 dispatch 时做）
3. 或者新增方法 `triggers_for_phase(phase: &Phase) -> Vec<Topic>`，返回当前阶段的触发 topics
4. 向后兼容：`phase_triggers` 为 `None` 时退化为 `triggers` 当前行为

**Test scenarios**:
- 解析含 phase_triggers 的 YAML，验证结构正确
- 解析不含 phase_triggers 的 YAML，验证向后兼容
- 验证 `trigger_topics()` 在两种情况下都返回完整的 Topic 集合
- 验证 `triggers_for_phase()` 正确提取对应阶段的 trigger list 或回退

**Verification**: 单元测试覆盖两种配置格式的解析和 trigger 提取

### U3. Ralph: HatRegistry 和 EventBus 阶段感知分发

**Goal**: 按当前阶段过滤 Hat 事件订阅

**Requirements**: R4、R11

**Dependencies**: U1（phase 状态）、U2（phase_triggers 配置）

**Files**:
- `crates/ralph-core/src/hat_registry.rs` — 修改 `subscribers()` / `get_for_topic()`
- `crates/ralph-proto/src/event_bus.rs` — 修改 `publish()`，增加 `_phase` metadata
- `crates/ralph-core/src/event_loop/mod.rs` — 增加阶段状态管理

**Approach**:
1. `HatRegistry` 增加方法 `set_phase(phase: Phase)`，更新当前阶段
2. `get_for_topic()` 在返回匹配 hat 前，调用 `hat.triggers_for_phase(current_phase)` 校验 topic 是否在当前阶段的触发列表中
3. `EventBus` 增加 `current_phase: Phase` 字段，`publish()` 在路由时检查阶段匹配
4. **`_phase` metadata 注入**：`EventBus::publish()` 在发布事件时自动注入 `_phase` 字段到 event metadata：
   ```rust
   // EventBus::publish() 内部
   event.metadata.insert("_phase".into(), self.current_phase.to_string());
   ```
   此字段写入 events JSONL，使 analyzer 能区分 warmup 和 production 阶段的事件（不依赖时间戳推断）
5. 当前阶段由 `loop_runner` 在启动时从 `phase_config.initial` 初始化，通过 `HatRegistry::set_phase()` 传播

**Test scenarios**:
- warmup 阶段：experiment.start → Harness Hat 收到；harness.blocked → 不被路由到 Harness Hat
- production 阶段：harness.blocked → Harness Hat 收到；experiment.start → Harness Hat 不收到
- 动态切换阶段后，新事件的订阅匹配立即更新
- 向后兼容：无 phase_triggers 的 hat 在所有阶段行为一致

**Verification**: 集成测试验证事件在不同阶段正确路由到不同 hats

### U4. Ralph: PhaseWatcher 自动检查 + 阶段持久化 + Scratchpad 注入

**Goal**: Ralph loop_runner 在 warmup 阶段自动监听 `experiment.evaluated`，调用退出条件检查脚本，条件满足时自动触发过渡。同时负责阶段持久化和将阶段信息注入 agent scratchpad。

**Requirements**: R8（退出条件自动化检查）、R9（过渡自动执行）、R10（过渡后发布 harness.initialized）

**Dependencies**: U3（EventBus 阶段感知），U5.5（check_exit_conditions.py）

**Files**:
- `crates/ralph-cli/src/loop_runner.rs` — PhaseWatcher 主逻辑、阶段持久化、scratchpad 注入
- `crates/ralph-core/src/event_loop/mod.rs` — PhaseWatcher 入口钩子
- `.ralph/agent/phase.json` — 运行时阶段指示文件

**Approach**:

#### 4a. Phase 状态持久化（原 U4 内容）

1. 启动时从 `EventLoopConfig.phase_config.initial` 初始化阶段
2. 写入 `.ralph/agent/phase.json`：`{"phase": "warmup", "changed_at": "2026-05-29T10:00:00Z"}`
3. 恢复时优先读取 `phase.json`，不存在时回退到配置的 `initial`
4. `loop_runner` 监听 `phase.transition` 事件：
   - 解析 payload `{to: "production", reason: "..."}`
   - 更新 `HatRegistry` 当前阶段
   - 更新 `phase.json`
   - 发布 `harness.initialized {phase: production, harness_version: <new>}`
5. **`warmup_completed` 字段**（新增，用于 stop_on_exit 场景）：
   - 当过渡脚本以 `--stop` 运行时，写入 `phase.json` 增加 `warmup_completed: true`
   - 正常两阶段过渡（无 `--stop`）时不写入此字段
   - 启动检测逻辑（U14）据此判断是否跳过 warmup

6. **phase.json 完整结构**：
   ```json
   {
     "phase": "production",
     "changed_at": "2026-05-29T10:00:00Z",
     "warmup_completed": true    // 仅 stop_on_exit 场景存在
   }
   ```

#### 4b. PhaseWatcher（新增核心机制）

在 `loop_runner.rs` 的事件循环中，每次事件发布完成后插入检查钩子：

```
// loop_runner.rs — 事件循环主逻辑
loop {
    let event = next_event()?;
    EventBus::publish(&event)?;

    // === PhaseWatcher 钩子 ===
    if self.current_phase == Phase::Warmup && event.topic == "experiment.evaluated" {
        // 条件 1: 非强制模式且当前为 warmup 阶段
        // 条件 2: 刚发布的是 experiment.evaluated（一轮评估完成）

        let exit_result = Self::run_check_exit_conditions(&self.project_root, &self.config)?;
        // 调用: python3 check_exit_conditions.py
        //   --project-root <dir>
        //   --output-json .ralph/agent/exit-check-result.json

        match exit_result.overall {
            "pass" => {
                // 判断是否需要在过渡后停止
                let stop_on_exit = exit_result.stop_on_exit
                    || self.cli_args.warmup_only
                    || false;

                if stop_on_exit {
                    // Warmup Only 模式：过渡后优雅停止
                    Self::run_transition_script(&self.project_root, &["--stop"])?;
                    
                    // 标记 warmup_completed
                    self.set_phase_with_completion(Phase::Production, true);
                    
                    // 发布完成事件
                    EventBus::publish(Event::new("warmup.complete", json!({
                        "phase": "production",
                        "reason": "warmup_completed",
                        "harness_version": exit_result.harness_version + 1,
                    })))?;
                    
                    // 优雅停止循环
                    break;  // loop 结束，Ralph 正常退出
                } else {
                    // 标准两阶段模式：过渡后进入 Production
                    Self::run_transition_script(&self.project_root, &[])?;
                    
                    // 更新阶段
                    self.set_phase(Phase::Production);
                    
                    // 发布确认事件
                    EventBus::publish(Event::new("harness.initialized", json!({
                        "phase": "production",
                        "harness_version": exit_result.harness_version + 1,
                    })))?;
                }
            }
            "drain" => {
                // 有未闭合实验 → 日志记录，等待下一轮
                log::info!("PhaseWatcher: drain needed (exit 42), waiting for next round");
            }
            "fail" => {
                // 条件未满足 → 日志记录哪些条件不满足
                log::info!("PhaseWatcher: exit conditions not met, continuing warmup");
                for check in &exit_result.checks {
                    if check.result == "fail" {
                        log::info!("  - {}: {}", check.name, check.detail);
                    }
                }
            }
        }
    }
}
```

**PhaseWatcher 和现有机制的关系**：
- PhaseWatcher 是 **loop_runner 内部的 Rust 逻辑**，不是 Hat 也不是 Hook
- 它运行在事件循环的主线程中，同步等待子进程完成
- 子进程退出后，Ralph 根据退出码决定下步动作
- 它不阻塞正常事件流——只在 `experiment.evaluated` 后插入一次检查

**check_exit_conditions.py 子进程通信协议**：

```
输入: CLI 参数 --project-root / --config-path / --output-json
配置读取: 启动时读取 autoresearch.yml 中 event_loop.warmup_config.stop_on_exit 的值
输出 JSON（写入 --output-json 指定的文件）:
{
  "overall": "pass | fail | drain",
  "stop_on_exit": true,         // 从配置文件读取，PhaseWatcher 决策用
  "checks": [
    {"name": "min_iterations_reached", "result": "pass", "current": 12, "required": 10},
    {"name": "pre_run_smoke_all_pass", "result": "pass"},
    {"name": "no_recent_harness_finding", "result": "fail", "detail": "2 new findings in last 3 rounds"},
    {"name": "no_open_p0", "result": "pass"},
    {"name": "within_max_iterations", "result": "pass", "current": 12, "max": 30}
  ],
  "harness_version": 1,
  "drain_info": null
}

或者 drain 需要时:
{
  "overall": "drain",
  "drain_info": {
    "pending_task_key": "nuttx-exp-5",
    "last_event": "experiment.planned",
    "published_at": "2026-05-29T10:30:00Z"
  },
  "checks": []
}
```

退出码映射：
- 0 = pass（全部条件满足，可以过渡）
- 1 = fail（有未满足条件，继续 warmup）
- 42 = drain（有未闭合实验链，等待下一轮）

#### 4c. 阶段信息注入 Agent Scratchpad

在 `loop_runner` 构建 agent context（scratchpad）时，自动注入当前阶段文件内容：

```
// loop_runner.rs — build_agent_context() 中
let phase_path = project_root.join(".ralph/agent/phase.json");
if phase_path.exists() {
    let phase_content = std::fs::read_to_string(&phase_path)?;
    scratchpad.content.push_str("\n## Current Phase\n");
    scratchpad.content.push_str(&phase_content);
    scratchpad.content.push_str("\n");
}
```

所有 Hat 的指令中统一通过 "检查 scratchpad 中的 Current Phase 节" 来获取当前阶段，而非自行 `cat phase.json`。

**为什么不是 Hat 自读**：LLM agent 可能忘记读文件、读错路径、或解析错误。Ralph 注入 scratchpad 是确定性的——agent 的指令只需说"查看 Current Phase 部分即可"。

**Test scenarios**:

**持久化测试**：
- 正常启动：phase.json 写入正确，内容包含 phase + changed_at
- 恢复启动：从 phase.json 读取阶段，不覆盖（不以 config.initial 为准）
- 无 phase.json + 无 phase_config：回退兼容，默认无阶段概念
- 收到 `phase.transition` 事件：阶段切换，订阅更新，新事件按新阶段路由
- 收到无效 `phase.transition` payload（如 unknown phase）：优雅降级，只日志警告

**PhaseWatcher 测试**：
- `experiment.evaluated` 发布 + warmup 阶段 + 退出条件全满足 + stop_on_exit=false → 自动执行过渡脚本，进入 Production
- `experiment.evaluated` 发布 + warmup 阶段 + 退出条件全满足 + stop_on_exit=true → 自动执行过渡脚本（--stop），发布 warmup.complete，优雅停止循环
- `experiment.evaluated` 发布 + warmup 阶段 + 条件未满足 → 日志记录，继续循环
- `experiment.evaluated` 发布 + production 阶段 → PhaseWatcher 跳过（不检查）
- 非 `experiment.evaluated` 事件 → PhaseWatcher 跳过
- check_exit_conditions.py 返回 drain（退出码 42）→ 日志记录，不阻塞
- check_exit_conditions.py 崩溃/超时 → PhaseWatcher 记录错误，继续循环（不阻塞）
- 过渡脚本被调用后成功完成（无 --stop）→ phase 更新为 production，后续事件按新阶段路由
- **过渡脚本被调用后成功完成（有 --stop）→ phase.json 写入 warmup_completed=true → 循环 break**
- **CLI `--warmup-only` 覆盖配置中的 stop_on_exit=false**

**Scratchpad 注入测试**：
- phase.json 存在 → scratchpad 包含 "## Current Phase" 节
- phase.json 不存在 → scratchpad 不包含该节（不回退到 config.initial）
- 阶段切换后 → 新 agent 实例的 scratchpad 反映最新阶段

**Verification**: 集成测试覆盖 PhaseWatcher 完整链路；单元测试覆盖 phase 持久化；日志检查验证 PhaseWatcher 决策

### U5. pre_run_smoke.py: 增加 --check-only 标志

**Goal**: 支持无副作用的检查模式，输出结构化 JSON 供退出条件评估

**Requirements**: R1（pre_run_smoke hook）、R7（Warmup 每轮检查）、R8（退出条件中的 pre_run_smoke 通过判定）

**Dependencies**: 无

**Files**:
- `skills/uni-autoresearch/scripts/pre_run_smoke.py`

**Approach**:
1. 增加 `--check-only` 标志：只检查不修复，等同于当前 `--repair` 的反向
2. 增加 `--output-json` 可选参数：输出结构化 JSON 到指定文件
3. JSON 输出结构：
   ```json
   {
     "status": "pass | fail | warn",
     "checks": [
       {"name": "required_files", "result": "pass", "detail": ""},
       {"name": "harness_readiness", "result": "warn", "detail": "..."},
       {"name": "proof_gate", "result": "fail", "detail": "..."}
     ],
     "summary": {"pass": 5, "warn": 1, "fail": 0}
   }
   ```
4. 退出码：
   - 0 = all pass (无 FAIL)
   - 1 = 有 FAIL（任意 check 为 FAIL）
5. 新增 `Checker` 类或重构现有函数，使每个 check 返回结构化 result 而非仅 print

**Test scenarios**:
- `--check-only` 无 JSON 输出：标准输出显示，退出码 0
- `--check-only --output-json result.json`：JSON 文件写入正确
- 含 FAIL 的 check（如缺失必需文件）：JSON 中 status=fail，退出码 1
- 仅 WARN、无 FAIL：JSON 中 status=warn，退出码 0
- 向后兼容：无 `--check-only` 时行为不变

**Verification**: `python3 pre_run_smoke.py --check-only --output-json /tmp/test.json` 检查 JSON

### U5.5. 退出条件检查脚本: check_exit_conditions.py (NEW)

**Goal**: 确定性地检查 warmup 退出条件（R8 五项），输出结构化 JSON 供 PhaseWatcher（U4）消费。是 Gap 1+3 中"Python 脚本层"的实现。

**Relationships**:
- 被调用者：U4 PhaseWatcher（Ralph loop_runner 在 `experiment.evaluated` 后 spawn 子进程）
- 内部调用：本脚本内部调用 `pre_run_smoke.py --check-only` 获取检查结果
- 触发时机：PhaseWatcher → check_exit_conditions.py → （条件满足）→ transition_warmup_to_production.py

**Requirements**: R8（5 项退出条件）

**Dependencies**: U5（pre_run_smoke --check-only）

**Files**:
- `skills/uni-autoresearch/scripts/check_exit_conditions.py` NEW
- 在 `generate_autoresearch.py` 的 `_copy_support_scripts()` 中添加

**Approach**:

1. **脚本入口参数**：
   ```
   --project-root <dir>      # 目标项目根目录
   --config-path <path>      # autoresearch.yml 路径（用于读取 warmup_config.stop_on_exit 等配置）
   --output-json <path>      # 结构化结果输出路径
   ```

2. **5 项退出条件检查逻辑**（严格执行 R8）：

   ```
   Check 1: MIN_ITERATIONS_REACHED
     读取 events JSONL，统计已完成的 experiment.evaluated 数量
     比较 vs warmup_config.min_iterations (默认 10)
     条件: count >= min_iterations
     失败时: result=fail, detail="当前 {n} 轮，需要 ≥ {min} 轮"

   Check 2: PRE_RUN_SMOKE_PASS
     内部调用: python3 pre_run_smoke.py --project-root <dir> --check-only --output-json <tmp>
     读取输出的 JSON
     条件: summary.fail == 0（允许 WARN，不允许 FAIL）
     失败时: result=fail, detail="pre_run_smoke 有 {n} 个 FAIL 检查"

   Check 3: NO_RECENT_HARNESS_FINDING
     扫描 events JSONL 最近 exit_quiet_rounds 轮内的 harness.* 事件
     检查是否有新增值的 harness finding（通过事件 payload 中的 finding 字段）
     条件: 最近 N 轮内没有新 finding（N = warmup_config.exit_quiet_rounds）
     失败时: result=fail, detail="最近 {n} 轮内有 {m} 个新 harness finding"

   Check 4: NO_OPEN_P0
     读取 .ralph/agent/open_tasks.json 或 events JSONL 中的 finding state
     条件: 无 severity=P0 且 status=open 的 finding
     失败时: result=fail, detail="存在 {n} 个 open P0 finding"

   Check 5: WITHIN_MAX_ITERATIONS
     统计已完成轮次
     比较 vs warmup_config.max_iterations (默认 30)
     条件: count <= max_iterations
     失败时:
       result=fail（触发 F2 超限处理）
       detail="已运行 {n} 轮，超过上限 {max}。需要用户决策"
   ```

3. **额外 drain 检测**（检查前执行）：
   ```
   扫描 events JSONL 最后 10 条事件
   如果存在 experiment.planned 但无对应 experiment.evaluated/blocked
     → overall="drain", 不执行 5 项检查
     → 输出 drain_info: {pending_task_key, last_event, published_at}
   ```

4. **JSON 输出结构**（与 PhaseWatcher 通信）：
   ```json
   {
     "overall": "pass | fail | drain",
     "stop_on_exit": true,       // NEW — 从配置 warmup_config.stop_on_exit 读取
     "checks": [
       {"name": "min_iterations_reached", "result": "pass", "current": 12, "required": 10},
       {"name": "pre_run_smoke_all_pass", "result": "pass"},
       {"name": "no_recent_harness_finding", "result": "fail", "detail": "2 new findings in last 3 rounds"},
       {"name": "no_open_p0", "result": "pass"},
       {"name": "within_max_iterations", "result": "pass", "current": 12, "max": 30}
     ],
     "harness_version": 1,
     "drain_info": null
   }
   ```

5. **退出码**：
   - 0 = pass（overall=pass）
   - 1 = fail（overall=fail）
   - 42 = drain（overall=drain）

6. **幂等与安全**：
   - 只读操作（不修改任何文件）
   - 可重复调用，无副作用
   - pre_run_smoke --check-only 本身也是只读
   - 解析失败/文件缺失时输出 JSON result=fail + detail，不崩溃

**Test scenarios**:
- 5 项全满足 → overall=pass, exit 0
- stop_on_exit=true → JSON 输出中 stop_on_exit=true
- stop_on_exit 缺失 → JSON 输出中 stop_on_exit=false（向后兼容）
- pre_run_smoke 有 FAIL → overall=fail, exit 1, Check 2 的 detail 包含具体 FAIL 名称
- 最近 2 轮有新 finding → overall=fail, exit 1
- 超过 max_iterations → overall=fail, exit 1, detail 提示需要用户决策
- 有未闭合 experiment.planned → overall=drain, exit 42, drain_info 包含 task_key
- events JSONL 为空（新项目第 1 轮）→ Check 1 fail，其他 skip 或 pass
- 配置文件缺失 → 友好错误 JSON，exit 1

**Verification**: `python3 check_exit_conditions.py --project-root /tmp/test --output-json /tmp/result.json && cat /tmp/result.json | jq`

### U6. 过渡脚本: transition_warmup_to_production.py (更新)

**Goal**: 实现 R9 + R10 过渡，支持幂等 checkpoint 恢复和 Step 0 drain 检测

**Requirements**: R9（过渡 5 步）、R10（发布 harness.initialized）、KTD3（drain 优先）

**Dependencies**: 无（独立脚本，被 PhaseWatcher 调用）

**Requirements**: R9（过渡 5 步）、R10（发布 harness.initialized）

**Dependencies**: U5.5（被 PhaseWatcher 调用，或独立使用）

**Files**:
- `skills/uni-autoresearch/scripts/transition_warmup_to_production.py` NEW
- 在 `generate_autoresearch.py` 的 `_copy_support_scripts()` 中添加

**Approach**:

1. **脚本入口参数**：
   ```
   --project-root <dir>        # 目标项目根目录
   --config-path <path>        # autoresearch.yml 路径
   --force                     # 跳过 Step 0 drain 检查（用于 F2 强制过渡）
   --stop                      # NEW：过渡完成后写入 warmup_completed 标记，不发布 phase.transition
   --check-exit-only           # 只检查退出条件不执行（辅助调试，与 U5.5 同协议）
   --output-json <path>        # --check-exit-only 时的输出路径
   ```

2. **Step 0: DRAIN_EXPERIMENT**（新增，KTD3 实现）：
   ```
   Step 0: DRAIN_EXPERIMENT
     适用条件: --force 未指定时才执行
     读取 events JSONL 最后 20 条事件
     查找最近的 experiment.planned → 是否有对应 experiment.evaluated 或 experiment.blocked
     如果存在未闭合链:
       → 写入 checkpoint: {"step": "DRAIN_EXPERIMENT", "done": false, "drain_info": {task_key, last_event}}
       → 退出码 42（告诉调用者"再等一轮"）
       → 不执行后续步骤
     如果全部闭合:
       → 写 checkpoint: {"step": "DRAIN_EXPERIMENT", "done": true}
       → 继续 Step 1
   ```
   退出码 42 的含义：ASCII 42 是 `*`（通配符）——"现在不是时候，等等再来"。PhaseWatcher 收到 42 后日志记录"正在 drain"，下一轮 `experiment.evaluated` 后重试。

3. **5 步过渡**（每一步写入 checkpoint 支持幂等恢复）：

   **Step 1: MARK_SUPERSEDED**
   ```
   限定范围: 只标记当前 loop session 内的 task_key
     （用 session start 时间戳或 loop ID 过滤 events JSONL）
   扫描 events JSONL，找到当前 session 内的所有 task_key
   更新 measurement-contract.json 中对应实验的:
     measurement_status: "superseded"
     superseded_by: "warmup_v{N}"
   写 checkpoint: {"step": "MARK_SUPERSEDED", "done": true}
   ```
   **范围限定理由**：events JSONL 可能含旧 loop 事件（恢复场景），不限定会误标非 warmup 数据。

   **Step 2: RETAIN_DIRECTIONS**
   ```
   从 strategy_state.json 提取所有 directions（task_key, hypothesis, decision, last_result）
   写入 .ralph/agent/warmup-directions.json（方向探索记录，只读不参与贝叶斯计算）
   写 checkpoint: {"step": "RETAIN_DIRECTIONS", "done": true}
   ```

   **Step 3: REBUILD_BASELINE**
   ```
   删除当前 baseline（调用 baseline 清理命令）
   重新运行 baseline 生成命令
   写 checkpoint: {"step": "REBUILD_BASELINE", "done": true}
   ```

   **Step 4: RESET_BAYESIAN**
   ```
   读取 strategy_state.json 的 _original_initial_state
   重置 directions 为空对象，meta 保留
   将 _original_initial_state 复制为顶层
   写 checkpoint: {"step": "RESET_BAYESIAN", "done": true}
   ```

   **Step 5: BUMP_VERSION**
   ```
   读取 harness-version.json → 递增 major 版本号（1→2）
   写 checkpoint: {"step": "BUMP_VERSION", "done": true}
   ```

4. **幂等逻辑**：启动时读 `.ralph/agent/transition-checkpoint.json`，跳过已完成的 step

5. **全部完成后**（无 `--stop`）：
   - 发布 `phase.transition {to: production, version: <new>}`
   - 删除 checkpoint 文件

6. **全部完成后**（有 `--stop`）：
   - 写入 `phase.json`：
     ```json
     {"phase": "production", "changed_at": "2026-05-29T10:00:00Z", "warmup_completed": true}
     ```
   - **不发布** `phase.transition`（Ralph 即将停止，无需事件分发）
   - 删除 checkpoint 文件
   - 退出码 0

**Test scenarios**:
- 完整过渡（无 --stop）：Step 0 通过 → 5 步全部执行 → phase.transition 发布
- **完整过渡（有 --stop）**：Step 0 通过 → 5 步全部执行 → phase.json 写入 warmup_completed → 不发布 phase.transition → 退出码 0
- **有未闭合实验**：Step 0 退出码 42，不执行后续步骤
- 中断恢复：在 Step 3 中断 → 重跑后从 Step 4 恢复
- 空 warmup（无实验数据）：Step 0 通过（无未闭合链），Step 1 无可标记任务，其他正常执行
- `--force` 模式：跳过 Step 0 drain，直接执行 5 步（F2 强制过渡用）
- `--check-exit-only`：执行 Step 0 检查，输出 JSON 结果，退出码 0/1/42，不修改文件
- 目标文件不存在（如 strategy_state.json 缺失）：报错退出，不清除 checkpoint
- Step 1 范围限定：events JSONL 含旧 loop 数据时，只标记当前 session 的任务

**Verification**: 用 `--force` 模式对测试项目执行完整过渡，检查每个 step 的输出和 checkpoint 文件

### U7. generate_autoresearch.py: Warmup 配置生成

**Goal**: 生成器支持生成包含 warmup 板块的配置

**Requirements**: R1（pre_flight 门禁）、R2（warmup 配置对象）、R3（默认值）、R7（修后验证 + 退出检查时调用 pre_run_smoke）

**R1 实现说明**：需求文档 R1 要求 preflight_extensions 增加 pre_run_smoke hook。但根据实际设计（Gap 2 裁定），pre_run_smoke 不在 preflight 中每轮跑，而是：
1. 修复后由 Harness Hat 指令调用（验证修复有效）
2. 退出条件检查时由 check_exit_conditions.py 内部调用（获取当前状态）
因此 preflight_extensions 中不包含 pre_run_smoke hook。R1 的实现方式与需求文​​档不同，但满足 R7 的"修复效果可验证"意图。

**Dependencies**: U5（pre_run_smoke --check-only）、U6（transition 脚本）

**Files**:
- `skills/uni-autoresearch/scripts/generate_autoresearch.py`

**Approach**:

1. **_build_event_loop()** 增加 warmup 配置（含 `stop_on_exit`）：
   ```python
   def _build_event_loop(answers):
       # ... 现有逻辑 ...
       if answers.get("enable_harness_extensions", False):
           warmup_config = {
               "min_iterations": 10,
               "max_iterations": 30,
               "exit_quiet_rounds": 3,
               "stop_on_exit": False,
           }
           # --warmup-only CLI 标志覆盖
           if answers.get("warmup_only", False):
               warmup_config["stop_on_exit"] = True
           
           event_loop["phase_config"] = {
               "initial": "warmup",
               "transition_event": "phase.transition"
           }
           event_loop["warmup_config"] = warmup_config
       return event_loop
   ```

2. **_build_preflight_extensions()** **不包含** pre_run_smoke hook（与需求文档 R1 不同）：
   ```python
   # preflight_extensions 只包含 validate_config + audit_config
   # pre_run_smoke 不由 preflight 调用，而是：
   #   - 修复后由 Harness Hat 指令调用
   #   - 退出条件检查时由 check_exit_conditions.py 内部调用
   #
   # 理由：preflight 只跑一次，但 Harness 修复可能发生在循环运行中。
   # 修复后的验证需要"按需跑"而不是"提前跑"。
   ```

3. **_build_hats()** 中修改 Harness Hat 的 triggers 为阶段感知：
   ```python
   if answers.get("enable_harness_extensions", False):
       harness_hat["triggers"] = []
       harness_hat["phase_triggers"] = {
           "warmup": ["experiment.start"],
           "production": ["harness.blocked"]
       }
   else:
       harness_hat["triggers"] = ["harness.blocked"]
   ```

4. **_copy_support_scripts()** 增加三个脚本：
   - `transition_warmup_to_production.py` — 过渡执行
   - `check_exit_conditions.py` — 退出条件检查（PhaseWatcher 调用）
   - `pre_run_smoke.py` — 已存在，仍复制（被 check_exit_conditions 内部调用）

5. **CLI 参数**：
   - 增加 `--phase` 可选项，覆盖默认的 `initial` phase
   - 增加 `--warmup-only` 标志，等价于设置 `warmup_config.stop_on_exit: true`
   - `--phase production` 与 `--warmup-only` 互斥（若同时指定，`--warmup-only` 忽略）

**Test scenarios**:
- `enable_harness_extensions=true`：生成含 phase_config、warmup_config（含 stop_on_exit=false）、phase_triggers 的 YAML
- `enable_harness_extensions=true --warmup-only`：生成含 stop_on_exit=true 的 warmup_config
- `enable_harness_extensions=false`：不生成 warmup 相关配置，向后兼容
- 生成的 Harness Hat 包含 `triggers: []` + `phase_triggers`（启用时）
- **preflight_extensions 不含 pre_run_smoke hook**（validate_config + audit_config 仅两个）
- transition/check_exit_conditions/pre_run_smoke 三个脚本都被复制到 support_scripts
- `--phase production` 时 phase_config.initial 为 production（跳过 warmup）
- `--phase production --warmup-only`：互斥，--warmup-only 被忽略，production 优先

**Verification**: `python3 generate_autoresearch.py --input tests/fixtures/sample-answers.json --output-dir ./out`，检查输出的 YAML

### U8. hat-contracts.yml: 更新实验事件合约

**Goal**: 合约反映阶段感知事件结构和 attack_target 字段

**Requirements**: R6（attack_target 路由）、相关依赖

**Dependencies**: 无（文档性修改）

**Files**:
- `skills/uni-autoresearch/assets/hat-contracts.yml`

**Approach**:

1. `experiment.attacked` 的 `optional_fields` 增加 `n`：
   ```yaml
   experiment.attacked:
     required_fields:
       - task_key
     optional_fields:
       - n              # 攻击目标: harness|measurement|state|content|other
       - novel_finding
       - high_risk_finding
     producer: red_team_attacker
   ```

2. Harness Hat 合约增加 `phase_triggers` 记录：
   ```yaml
   harness:
     triggers: []
     phase_triggers:
       warmup: ["experiment.start"]
       production: ["harness.blocked"]
     publishes: [...]
     # 其余不变
   ```

3. `event_protocol` 中新增 `phase.transition` 作为 guarded topic

**Test scenarios**: YAML 解析验证

**Verification**: `python3 -c "import yaml; yaml.safe_load(open('skills/uni-autoresearch/assets/hat-contracts.yml'))"`

### U9. hat-harness.md: Warmup/Production 双模式指令

**Goal**: Harness Hat 指令明确描述 Warmup 和 Production 阶段的不同行为

**Requirements**: R4（Warmup 自动激活）、R5（不限 Repair Budget）、R6（攻击路由）、R7（每轮 pre_run_smoke）、R11（Production 标准触发）、R12（Repair Budget 生效）

**Dependencies**: U7（生成器输出）、U5（pre_run_smoke）

**Files**:
- `skills/uni-autoresearch/references/hat-harness.md`

**Approach**:

1. 增加 "阶段模式" 一节，说明：
   - 当前阶段从 `.ralph/agent/phase.json` 读取
   - Warmup 阶段行为与 Production 阶段行为的差异

2. Warmup 模式：
   - 激活：`experiment.start`（不再等待 `harness.blocked`）
   - Repair Budget：`per_activation_max_attempts` 和 `per_finding_max_attempts` 无限（或极大值）
   - 攻击检测：检测到 `n=harness|measurement|state` 时自动进入修复
   - **修复后验证**（替代原"每轮执行"）：Harness Hat 完成修复后调用 `pre_run_smoke.py --check-only` 验证修复效果。无修复时不跑，不做无意义轮询。
   - 退出条件检查：由 **PhaseWatcher（Ralph loop_runner）** 在 `experiment.evaluated` 后自动执行，非 Harness Hat 职责。Harness Hat 不需要自己检查退出条件。

3. Production 模式：
   - 激活：仅响应 `harness.blocked`
   - Repair Budget 正常生效
   - 攻击检测：`n=harness|measurement|state` 仍应路由到 Harness Hat
   - 内容质量的 finding 路由到 Evaluator

4. 增加退出条件检查的指令指引：
   - 何时检查（每次 `experiment.evaluated` 后）
   - 检查哪些条件（R8 五项）
   - 条件满足后的动作（调用过渡脚本）

5. 过渡指引：
   - Warmup 退出 → 调用 `transition_warmup_to_production.py`
   - **stop_on_exit 模式**：如果配置了 `stop_on_exit: true` 或使用了 `--warmup-only`，Warmup 完成后不会自动进入 Production，而是完成过渡后停止循环。下次启动时自动跳过 Warmup 直接进入 Production
   - 超限处理 → 发布 `harness.blocked` + 等待用户决策

**Test scenarios**: 文档审查，无自动化测试

**Verification**: 人工审查

### U10. hat-evaluator.md: Warmup 阶段评估路由

**Goal**: Evaluator 在 Warmup 阶段识别 attack_target 并路由到 Harness Hat

**Requirements**: R6（attack_target 绕过 Evaluator）、R13（Production 下区分路由）

**Dependencies**: U8（合约更新）

**Files**:
- `skills/uni-autoresearch/references/hat-evaluator.md`

**Approach**:

1. 在 Evaluator 的校准处理节中增加：
   - 读取 `experiment.attacked` 中的 `n` 字段
   - Warmup 阶段：`n=harness|measurement|state` → 静默跳过 Evaluator 评估，路由到 Harness Hat
   - Production 阶段：`n=harness|measurement|state` → 仍路由到 Harness Hat；`n=content|other` → 正常评估

2. `recommended_next_action` 增加 `harness_routing` 值（可选，用于标记校准中的特殊路由）

**Test scenarios**: 文档审查

**Verification**: 人工审查

### U11. validate_config / audit_config: 阶段感知校验

**Goal**: 验证器能识别和校验 warmup 配置的一致性

**Requirements**: R2、R3（warmup 配置结构和条件）

**Dependencies**: U7（生成器输出）

**Files**:
- `skills/uni-autoresearch/scripts/validate_config.py`
- `skills/uni-autoresearch/scripts/audit_config.py`
- `skills/uni-autoresearch/scripts/runtime_audit.py`

**Approach**:

1. **validate_config.py**：
   - 检查 `event_loop.warmup_config` 的字段完整性（min_iterations、max_iterations、exit_quiet_rounds）
   - `stop_on_exit` 是可选的 bool 字段，不存在时不报错（默认为 false）
   - `warmup_config` 存在时 → `phase_config.initial` 必须为 warmup
   - `warmup_config` 存在时 → Harness Hat 必须有 `phase_triggers` 无 `triggers`（或空 triggers）
   - `warmup_config` 存在时 → `enable_harness_extensions` 必须为 true（在生成时已保证，但运行时再检查一次）
   - `stop_on_exit: true` 且 `phase_config.initial = warmup` 无 production 阶段配置 → WARN（warmup 结束后不会进入 Production）

2. **audit_config.py**：
   - warmup 拓扑闭合检查：phase.transition 事件能否到达 Harness Hat
   - warmup 事件链检查：experiment.start → Harness Hat 是否在 warmup 阶段可达
   - production 事件链检查：harness.blocked → Harness Hat 是否在 production 阶段可达
   - 过渡检查：phase.transition 是否能从 warmup 触发 production

3. **runtime_audit.py**：
   - 增加 warmup 阶段相关的运行时检查：
     - warmup 期间 Harness Hat 是否被正确激活
     - warmup 退出条件是否在满足后触发过渡
     - warmup 超限后是否正确发布 harness.blocked

**Test scenarios**:
- 含 warmup_config 的合法配置 → PASS
- 含 warmup_config 但 Harness Hat 无 phase_triggers → FAIL
- 不含 warmup_config 但含 phase_triggers → WARN（phase_triggers 无意义）
- 不含 warmup_config 的旧配置 → 向后兼容 PASS
- 拓扑闭合：warmup 链和 production 链分别可达

**Verification**: `python3 validate_config.py --config ...` 和 `python3 audit_config.py --config-yml ...`

### U12. Review Skill 同步（含 Combox 适配）

**Goal**: Review Skill 能检测 warmup/production 阶段的合约一致性；**combox（阻断追问交互问题流）** 在 warmup 阶段能提出适配的问题、抑制不恰当的问题、自动调低与 warmup 目标一致的 finding 严重度。

**Dependencies**: U8（合约更新）

**Files**:
- `skills/uni-autoresearch-review/scripts/finding_detector.py`
- `skills/uni-autoresearch-review/SKILL.md`

**Approach**:

#### 12a. finding_detector.py — 新增阶段感知检测器

1. `check_contract_consistency()` 增加 `phase_triggers` 检查：合约中定义的 phase_triggers 是否与 YAML 配置一致
2. `check_terminal_authority_drift()` 增加阶段感知：warmup 阶段的 LOOP_COMPLETE 发布授权是否合理
3. **新增 `check_warmup_exit_conditions()`**：验证 warmup 退出条件合理性
   - `exit_quiet_rounds` ≤ 1 → HIGH blocking（误判退出风险，单轮无 finding 可能是随机波动）
   - `min_iterations` > `max_iterations` → HIGH blocking（永远无法退出）
   - `min_iterations` 过小（< 5）→ MEDIUM advisory（可能还没修完就退出了）
   - `exit_quiet_rounds` 缺失或 `min_iterations` 缺失 → HIGH blocking
4. **新增 `check_warmup_transition_path()`**：验证过渡路径完整性
   - warmup 配置存在但 `event_loop.phase_config.transition_event` 不存在 → HIGH blocking（warmup 完了无处可去）
   - warmup 配置存在但 Harness Hat 没有 `production` 阶段的 phase_triggers → MEDIUM（可能遗漏 production 配置）
   - 过渡脚本 `transition_warmup_to_production.py` 不在 support_scripts 中 → HIGH blocking（无法自动过渡）
5. **新增 `check_phase_config_completeness()`**：验证阶段配置完整性
   - `enable_harness_extensions` 为 true 但无 warmup 配置 → HIGH blocking（浪费了 Harness 增强能力，且无法享受两阶段好处）
   - warmup 配置存在但 `phase_triggers` 中 warmup 阶段的 triggers 为空 → HIGH blocking（warmup 模式下 Harness Hat 收不到事件）

#### 12b. finding_detector.py — 阶段感知严重度调制

在 finding 聚合和阻断判定阶段，增加阶段感知的严重度降级逻辑：

6. **新增 `_modulate_findings_by_phase()`** 方法（在 `_build_gate_aggregate()` 之前调用）：
   - 读取 `.ralph/agent/phase.json` 或配置中的 `phase_config.initial`
   - 若当前为 warmup 阶段，对以下 finding 类型自动降一级严重度（HIGH→MEDIUM, MEDIUM→WARN, WARN→INFO）：
     - `review-hardening-exit-missing` — warmup 本身就是 hardening
     - 所有 harness 稳定性相关的 finding — warmup 期间 harness 不稳定是预期行为
     - `runtime-payload-contract-invalid` — warmup 早期重试多是正常的
   - 标记被降级的 finding 的 `phase_downgraded: true`，记录原严重度到 `original_severity`
   - 降级仅在 warmup 阶段生效；production 阶段不做调制

#### 12c. Combox（阻断追问）暖机适配

在 `_enrich_blocking_fields()` 和 `_GRILL_QUESTION_DEFAULTS` 中增加 warmup 相关条目：

7. **`_GRILL_QUESTION_DEFAULTS` 新增 warmup 类别**：
   ```python
   "warmup-exit-conditions": "warmup 退出条件看起来可能不合理：{detail}。"
                            " 你预期在多少轮内完成 harness 校准？"
                            " 当前配置的 min_iterations 和 exit_quiet_rounds 是否符合你的实际情况？",
   "warmup-transition-path": "warmup 配置存在但没有完整的过渡路径。"
                             " warmup 结束后循环如何切换到 production？"
                             " 缺少 transition_event 或过渡脚本可能会导致循环卡在 warmup 阶段。",
   "warmup-production-plan": "warmup 阶段结束后，production 阶段的实验目标是什么？"
                             " 当前配置了 warmup 但没有 production 阶段的 phase_triggers，"
                             " 请确认：warmup 退出后你希望 Harness Hat 进入什么行为模式？",
   "warmup-exit-forever": "warmup 配置的 max_iterations={n} 且 min_iterations={m}，"
                          " 如果到了 max_iterations 仍未满足退出条件，你希望："
                          " (A) 强制过渡到 production（接受已知风险），"
                          " (B) 继续 warmup 等待用户介入，"
                          " (C) 终止循环？",
   ```

8. **`_BLOCKING_REASON_DEFAULTS` 新增对应条目**：
   ```python
   "warmup-exit-conditions": "warmup 退出条件配置不合理，可能导致循环无法顺利切换到 production",
   "warmup-transition-path": "warmup 缺少过渡路径，循环无法从校准阶段自动进入正式实验",
   "warmup-production-plan": "warmup 没有对应的 production 阶段配置，两阶段模式不完整",
   "warmup-exit-forever": "warmup 同时达到 max_iterations 且未满足退出条件时无处理策略",
   ```

9. **`_GRILL_DEFAULT_DEFAULTS` 新增安全兜底**：
   ```python
   "warmup-exit-conditions": "接受当前配置，但如果到达 max_iterations 仍未满足条件则标记为 known limitation",
   "warmup-transition-path": "补充 transition_event: phase.transition 并确保过渡脚本存在",
   "warmup-production-plan": "补充 Harness Hat 的 production 阶段 phase_triggers: [harness.blocked]",
   "warmup-exit-forever": "强制过渡到 production 并记录已知 limitation 到 measurement-contract.json",
   ```

10. **`_GRILL_OPTIONS_DEFAULTS` 中的 warmup 选项增强**：每个 warmup 问题除了标准的 `repair` / `accept_risk` 外，增加 warmup 特有的第三个选项 `continue_warmup`（延续 warmup 观察），选项描述描述"继续运行观察，达到 max_iterations 后自动触发超限处理"。

11. **交互式 combox 的阶段感知呈现**：当存在 warmup 相关的 blocking finding 时，combox 输出应在现有模板基础上增加前置说明：
    ```
    ⚠️ 检测到 warmup 配置问题：循环将从 warmup 阶段开始，
    以下问题涉及 warmup→production 过渡路径，请确认。
    ```
    此说明在 production-only（无 warmup）配置时不显示。

#### 12d. SKILL.md — finding 类型和 combox 文档更新

12. **在 contract finding 类型列表增加**：
    - `phase-config-missing-warmup`
    - `phase-triggers-inconsistent`
    - `warmup-exit-conditions-unreasonable`
    - `warmup-transition-path-missing`
    - `warmup-phase-incomplete`

13. **在 blocking 判定阶段（Phase 3）的 combox 说明中增加暖机标注**：
    补充说明：当检测到 warmup 配置时，combox 会自动：
    - 对 harness 相关 finding 降一级严重度
    - 加入 warmup 特定的追问问题
    - 在 grill question 标题前增加 `[Warmup]` 前缀标识

**Files（补充）**：
- `skills/uni-autoresearch-review/tests/run_finding_regression.py` — 更新 fixtures
- `skills/uni-autoresearch-review/tests/fixtures/` — 新增 warmup 配置的测试夹具

**Test scenarios**:
- 含 phase_triggers 的配置 → finding_detector 不报合约不一致
- 无 phase_triggers 但有 warmup_config → 报警告
- experiment.attacked 缺少 n 字段 → 不报错（可选字段）
- warmup `exit_quiet_rounds=1` → HIGH blocking，combox 输出问题
- warmup `min_iterations > max_iterations` → HIGH blocking
- warmup 配置完整且合理 → finding_detector 无 warmup 相关 finding
- warmup 阶段 + harness 稳定性 finding → 自动降一级 + `phase_downgraded` 标记
- production-only 配置 → combox 不显示 warmup 前置说明
- **regression test fixtures 覆盖**：新增 5 种 warmup finding 类型在 `tests/fixtures/` 中有对应输入，`run_finding_regression.py` 能通过断言（每个 blocking finding 必须包含 `grill_question`/`blocking_reason`/`grill_default`）

**Verification**: `python3 finding_detector.py --config ... --hat-contracts ...`；人工审查 combox 输出；`python3 tests/run_finding_regression.py` 全通过

### U13. Report Skill 同步

**Goal**: Report Skill 能识别和展示阶段信息

**Dependencies**: U6（过渡脚本、phase.json）

**Files**:
- `skills/uni-autoresearch-report/scripts/analyzer.py`
- `skills/uni-autoresearch-report/scripts/collector.py`

**Approach**:

1. **collector.py**：
   - `CollectedRalphData` 增加 `phase` 字段（从 `.ralph/agent/phase.json` 读取）
   - 防御性读取：文件不存在时默认 None

2. **analyzer.py**：
   - 在 `HarnessSummary` 中增加 phase 追踪（当前阶段、阶段转换时间）
   - `_analyze_harness()` 增加阶段事件统计（warmup 阶段事件 vs production 阶段事件）
   - 报告阶段转换时间线和对应的事件数
   - 在 Manager Brief 或执行摘要中展示当前阶段

**Test scenarios**:
- phase.json 存在且阶段为 warmup → 报告中显示 "当前阶段：Warmup"
- phase.json 不存在 → 降级行为，不报错
- 含阶段转换的记录 → 报告中显示转换时间和前后事件数对比

**Verification**: 运行 analyzer.py 检查输出

### U14. Ralph: 启动阶段检测（warmup_completed 跳过）

**Goal**: Ralph 启动时检测 phase.json 中的 `warmup_completed` 标记，如存在则跳过 warmup 初始化，直接以 production 模式启动。

**Requirements**: KTD5（完整过渡 + 标记 phase）

**Dependencies**: U4（phase 持久化）、U6（过渡脚本写入 warmup_completed）

**Files**:
- `crates/ralph-cli/src/loop_runner.rs` — 启动时阶段初始化逻辑

**Approach**:

1. **启动时阶段解析逻辑修改**（替换原有"从 config.initial 初始化"的简单逻辑）：
   ```
   // loop_runner.rs — build_initial_phase()
   let phase_path = project_root.join(".ralph/agent/phase.json");
   
   if phase_path.exists() {
       // 优先读取已持久化的 stage
       let phase_data: PhaseData = serde_json::from_slice(&fs::read(&phase_path)?)?;
       
       if phase_data.warmup_completed == Some(true) && phase_data.phase == "production" {
           // Warmup 已完成的标记 → 跳过 warmup，直接 production
           log::info!("Phase: warmup previously completed, starting in production mode");
           return (Phase::Production, true);  // (phase, skip_warmup_init)
       }
       
       if phase_data.phase == "warmup" {
           // 未完成的 warmup → 从 warmup 开始（恢复场景）
           return (Phase::Warmup, false);
       }
       
       if phase_data.phase == "production" {
           // 正常的 production（非 warmup 场景，之前的正常转换）
           return (Phase::Production, false);
       }
   }
   
   // phase.json 不存在 → 从配置初始化
   let initial = config.phase_config
       .as_ref()
       .map(|pc| pc.initial.clone())
       .unwrap_or(Phase::Production);
   log::info!("Phase: initializing from config as {:?}", initial);
   (initial, initial == Phase::Warmup)
   ```

2. **启动初始化序列**（获得 `(phase, skip_warmup_init)` 后）：
   ```rust
   let (phase, skip_warmup_init) = Self::build_initial_phase(&project_root, &config)?;
   
   if skip_warmup_init {
       // Warmup 标记已完成 → 不写入 phase.json（已存在），不触发 warmup preflight
       // 直接以 production 模式进入事件循环
       self.current_phase = Phase::Production;
       self.hat_registry.set_phase(Phase::Production);
       
       // 发布 production 模式下的 harness.initialized
       EventBus::publish(Event::new("harness.initialized", json!({
           "phase": "production",
           "harness_version": ...,
           "warmup_completed": true,
       })))?;
   } else {
       // 标准启动：写入 phase.json，触发 warmup preflight
       self.set_phase(phase);
       // ... 现有启动逻辑
   }
   ```

3. **强制重跑 Warmup**：
   - CLI 增加 `--force-warmup` 标志
   - 当指定时，忽略 phase.json 中的 warmup_completed，强制以 warmup 模式启动
   - 不删除 phase.json，仅跳过 warmup_completed 检查

**Test scenarios**:
- phase.json 不存在 → 从 config.initial 初始化（回退兼容）
- phase.json 含 `warmup_completed: true` → 跳过 warmup，以 production 启动，不覆盖 phase.json
- phase.json 含 `phase: production` 但无 warmup_completed → 以 production 启动（正常 Production 场景）
- phase.json 含 `phase: warmup` 且无 warmup_completed → 以 warmup 启动（恢复场景）
- `--force-warmup` 标志 → 忽略 warmup_completed，强制以 warmup 模式启动
- 跳过 warmup 启动后 → 发布 harness.initialized 带 `warmup_completed: true` 标记
- skip_warmup_init 时 → 不检查 config.phase_config.initial（phase.json 为准）

**Verification**: 集成测试覆盖启动阶段检测的各种 phase.json 内容场景；`--force-warmup` 覆盖强制 warmup 重跑
