# Precheck Gates（事件发射 LLM 关卡）

在关键 topic 进入下游之前，插入一轮 **LLM-as-judge** 检查。Preset 作者在 YAML 里声明 checklist；runtime 自动脱糖、合成 gate hat，无需手写 `precheck-*` hat 或 `.proposed`/`.rejected` 拓扑。

> 实施计划：`docs/plans/2026-07-02-004-feat-event-emit-precheck-prompt-gate-plan.md`  
> Loop 内 agent 行为：`ralph tools skill load ralph-tools-precheck`

## 这是什么（不是什么）

| | **Precheck Gates（本文）** | **CLI emit policy precheck** |
|---|---|---|
| 配置 | `event_loop.precheck` | `event_policy`、hat `publishes`、schema |
| 触发 | producer emit 后，gate hat 一轮 | `ralph emit` 写盘前 |
| 判断 | LLM checklist（主观） | 确定性字段/ownership/schema |
| CLI 命令 | **无** | `ralph emit --policy-check` |

**没有** `ralph precheck` 子命令。启用方式是在 preset / `ralph.yml` 里写配置块。

## 适用场景

- 主观质量门：如「review findings 是否有实质内容」「plan 是否回答了全部 open questions」
- 只想守 **少数关键终态 topic**（`review.complete`、`plan.complete` 等），接受每守一次多一轮 LLM 的成本

**不适合**（请用现有确定性门）：

- git diff / commit 证据 → `execution_contracts`
- 必填字段 / 白名单 → `event_policy.schemas`
- step 允许集 → `mechanism.flow` / `flow_step_scope`

## 工作原理（脱糖）

配置加载并 `normalize()` 后，对每条 `rules.<X>`：

1. 所有 `publishes` 或 `terminal_events` 含 `<X>` 的 hat → 改写为发 `<X>.proposed`
2. 插入合成 hat `precheck-<X>`，消费 `<X>.proposed`，发 `<X>`（过）或 `<X>.rejected`（不过）
3. 订阅 bare `<X>` 的下游 hat **不变** — 它们只在 gate 通过后看到真事件
4. `<X>.rejected` → `task.resume(target=on_fail.target)`；连续 `retry_budget` 次仍不过 → `on_exhausted`

```
producer --emit--> X.proposed --> precheck-X (LLM) --+--> X --> downstream
                                                    +--> X.rejected --> task.resume --> producer
```

> **Producer 视角（2026-07-29-006 计划）**：在 `event_loop.precheck` 启用且当前 hat 已是 `<X>.proposed` 的 producer 时，**`ralph emit <X>` 在 CLI 进入第一个 topic-dependent gate 之前会被透明改写为 `<X>.proposed`**。`ralph emit <X>.proposed` 也是合法且幂等的（不会被改写成 `<X>.proposed.proposed`）。显式 `<X>.proposed` 不会因「手 emit」被 origin guard 拒收。`<X>.proposed` 的 schema 继承 guarded `<X>` 的 `payload` + `required_fields`，所以缺字段的 emit 在写盘前就被拒。

## 快速启用（本地 preset）

在 hat YAML 的 `event_loop` 段加入：

```yaml
event_loop:
  execution_mode: isolated   # 若 hat 数 ≥4 或加 gate 后超过 coordinator 上限，必须 isolated
  precheck:
    enabled: true
    rules:
      work.done:
        prompt:
          - "实现与 plan 目标一致，不是空壳或 placeholder"
          - "变更范围与当前 step 匹配"
        on_fail:
          target: executor
          retry_budget: 3
          on_exhausted: "plan.blocked(reason=precheck_failed)"
          reason: "work.done failed subjective checklist"
          # 2026-08-17-1841 U1/U3 — 可选; common 总是显示,
          # by_check key 必须是 1-based 提示索引(字符串形式)
          recovery_guidance:
            common:
              - "重读 artifact 修根因,不要复制失败 payload"
              - "修复后重跑 ralph emit <topic> --policy-check"
            by_check:
              "1":
                - "实现必须能对照 plan 目标逐项验证"
```

## 2026-08-17-1841 计划新增：可配置 recovery guidance

precheck rule 与 payload-consistency rule 现在都接受可选 `recovery_guidance` 块（详见「2026-08-17-1841 计划」节）。两类 rule 的字段语义不同：

| 字段 | precheck | payload_consistency |
|---|---|---|
| `common` | 总是显示（含 synthetic rejection） | 总是显示（含 synthetic 不存在的假设） |
| `by_check` key | `1..=prompt.len()` 的 1-based 索引（字符串） | rule 的稳定 `id`（必等于 rule `id`） |
| 选择语义 | 按 gate 上报的 `failed_checks` 筛选（unmatched 不渲染） | 命中当前 rule 时显示整张 `by_check`（first-hit order） |

lint 严格门禁（preset_lint `recovery_guidance` 模块）：

- `preset.recovery_guidance_unknown_check` — precheck key 非 1-based 正整数 / 越界；consistency key ≠ rule id（致命：lint 直接 fail-close，strict mode 启动拒绝）
- `preset.recovery_guidance_empty_item` — `common[]` 或 `by_check[*][]` 任一 item 为空字符串
- `preset.recovery_guidance_unsafe_item` — item 超 1024 UTF-8 bytes / 含 ANSI escape / C0/C1 控制字符 / 零宽字符（mirrors `payload_consistency_unsafe_message`）

renderer 与 CLI JSON 同源：semantic guidance 永远只是 prose，不替代 payload；`Suggested payload` / `Suggested command` / `Expected payload` 字段不新增（plan D4 明确禁止）；agent 必须先 `ralph emit <topic> --policy-check` 通过，再正式 emit。

字段说明：

| 字段 | 必填 | 说明 |
|------|------|------|
| `enabled` | 是 | `true` 且 `rules` 非空才脱糖 |
| `rules.<X>` | 是 | 被守 topic，如 `review.complete` |
| `prompt` | 推荐 | checklist 字符串列表，渲染进 gate hat instructions |
| `on_fail.target` | 是 | 打回哪个 hat（接 `task.resume`） |
| `on_fail.retry_budget` | 否 | 默认 `3`；连续拒绝此次数后升级终态 |
| `on_fail.on_exhausted` | 否 | 默认空；典型值 `plan.blocked(reason=precheck_failed)` |
| `on_fail.reason` | 否 | 写在 `X.rejected` 与打回 prompt 里的短原因 |

### Schema SSOT 作者（`presets/schemas/*.yml`）

若 preset 从 schema 文件生成，可在 schema 顶层写 `precheck:`，`build.rs` 会 merge 到 `event_loop.precheck`（与 `execution_contracts` 同路径）。示例占位见 `presets/schemas/ce-executor-pipeline.yml` 尾部注释。

启用 precheck 后，runtime 会自动为 `<X>.proposed` / `<X>.rejected` 注入 `event_policy.schemas` 条目（`inject_precheck_event_schemas`）。若 preset 已有 `<X>` schema，不会被覆盖。

若使用 `plan.blocked(reason=precheck_failed)`，须在 schema 的 `plan.blocked.allowed_values.reason` 白名单中包含 `precheck_failed`（`ce-executor-pipeline` schema 已包含）。

## 验证清单

```bash
# 1. 静态 lint（在脱糖后的图上检查，含合成 gate hat）
ralph preset check -H .ralph/hats/my-flow.yml --strict

# 2. 或跑 crate 内 preset_lint 测试子集
cargo nextest run -p ralph-core -- preset_lint
cargo nextest run -p ralph-cli --bin ralph -- preset_lint

# 3. precheck 单元 + BDD
cargo nextest run -p ralph-core -- precheck
cargo nextest run -p ralph-core --test scenarios -- precheck
```

BDD 参考场景：

- `crates/ralph-core/tests/scenarios/2026-07-02-precheck-gate-pass.yml` — gate 通过
- `crates/ralph-core/tests/scenarios/2026-07-02-precheck-gate-exhaust.yml` — 预算耗尽 → `plan.blocked`

## Multi-hat / isolated 注意

每个 `rules.<X>` 会 **合成一个 gate hat**，计入 hat 总数：

- coordinator 模式已弃用（2026-06-18 起 isolated 为唯一支持模式）；文档保留此条仅为历史对照
- **isolated 模式：无 hat 数上限**，但每个 `rules.<X>` 会合成一个 gate hat，脱糖后仍需通过 `preset_lint` 校验
- 若 hat 数 ≥4 或加 gate 后拓扑不合法：须 `event_loop.execution_mode: isolated`

`preset_lint` 的 `multi_hat` / `workflow_activation` 在 **normalize 后的图**上运行；配置错会在 `ralph preset check` 报错，而不是 runtime 静默失败。

## 紧急关停（不改 YAML）

```bash
RALPH_PRECHECK_MODE=off ralph run -H .ralph/hats/my-flow.yml -p "..."
```

`off` 时跳过脱糖与 runtime gate，用于紧急止血。

## 零回归保证

- 所有出厂 **builtin preset 默认不带** `precheck`
- 未声明 `precheck` 的 preset：`normalize()` 前后 hat 拓扑等价
- 现有 loop 不启用本块则行为不变

## 成本与权衡

- 每个被守 topic、每次 producer 尝试发射，最多多 **一轮 LLM**（gate hat）
- 只守关键 topic；checklist 保持短、可判定
- gate 误判：有误拒（有界重试 + 终态）和误过（下游确定性门兜底）两侧缓解

## 相关文档

- [Preset Authoring](preset-authoring.md) — 创建与 lint preset
- [Execution Contracts](execution-contracts.md) — 机械 `work.done` 门（与 precheck 互补）
- [Payload Contracts](payload-contracts.md) — schema / `event_policy`
- [Runtime Diagnosis](runtime-diagnosis.md) — `plan.blocked` / recovery 诊断
