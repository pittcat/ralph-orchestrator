---
name: ralph-tools-precheck
description: 事件发射 LLM 关卡（event_loop.precheck）— loop 内 agent 对 .proposed/.rejected topic 的行为规范
metadata:
  internal: true
---

# Precheck Gates（事件发射 LLM 关卡）

> **仅当 preset 声明了 `event_loop.precheck` 时才需要本 skill。** 出厂 builtin preset 默认不带 precheck；若你的 prompt / 指令从未提到 `.proposed` / `.rejected` / `precheck-<X>`，不要加载本文件。
>
> **这不是 CLI 命令。** 没有 `ralph precheck`。本机制由 preset 里的 `event_loop.precheck` 配置驱动；**脱糖** = runtime 在加载配置时把原 topic `<X>` 改写成 `<X>.proposed`，并合成名为 `precheck-<X>` 的 gate hat。
>
> **与 `ralph emit --policy-check` 无关** — 后者是 CLI dry-run 确定性 schema/ownership 预检（**不写盘**）；本特性是 loop 里多一轮 LLM-as-judge hat。
>
> **opt-in**：未声明 `event_loop.precheck` 时一切行为与未启用时等价。
>
> Preset 作者配置 walkthrough：`docs/guide/precheck-gates.md`（人类文档，不是 agent 必读输入）。

---

## 何时加载本 skill

| 场景 | 操作 |
|------|------|
| loop 里出现 `<X>.proposed` / `<X>.rejected` / `precheck-<X>` hat | `ralph tools skill load ralph-tools-precheck` |
| 你是 producer，preset 已对某 topic 启用 precheck | 读下方「producer 行为」 |
| 你是下游 consumer，订阅的是 bare `<X>` | **无变化** — 脱糖后你仍只处理真 `<X>` |
| 要配置/启用 precheck | 读 `docs/guide/precheck-gates.md`（人类文档），不要猜 YAML |
| preset **没有** `event_loop.precheck` | **不要加载**本 skill |

---

## 关键术语（首次出现）

| 术语 | 含义 |
|------|------|
| **脱糖** | runtime 按 `event_loop.precheck.rules` 改写 publishes/triggers，并插入 gate hat 的配置变换 |
| **`retry_budget`** | producer 被 `<X>.rejected` 打回后，runtime 允许再试的次数上限（默认 `3`） |
| **`on_exhausted`** | `retry_budget` 用尽后 runtime 发出的终态动作（默认 `plan.blocked(reason=precheck_failed)`） |
| **`on_fail.target`** | `<X>.rejected` 后 `task.resume` 打回的目标 hat / 步骤 |

---

## 脱糖后的事件流（agent 视角）

对每条 `rules.<X>`：

1. 原 producer 的 `publishes`/`terminal_events` 里的 `<X>` 被改写为 `<X>.proposed`；若 producer 声明了 `default_publishes: <X>`（沉默时 runtime 兜底注入的 topic），同样改写为 `<X>.proposed` —— 兜底注入也必须过 gate，不会绕过审计直达下游
2. 合成 hat `precheck-<X>`：`triggers=[<X>.proposed]`，`publishes=[<X>, <X>.rejected]`
3. producer 发 `<X>.proposed` → gate hat 激活一轮 → 过则发 `<X>`，不过则发 `<X>.rejected`
4. `<X>.rejected` → runtime 自动 `task.resume(target=on_fail.target)`；`retry_budget`（再试上限）耗尽 → 执行 `on_exhausted`（默认 `plan.blocked(reason=precheck_failed)`）

---

## Topic 速查：你应该做什么

| Topic | 谁发 | agent 应该做什么 |
|-------|------|------------------|
| `<X>.proposed` | producer（脱糖后） | **producer**：照常 `ralph emit <X> ...` — CLI/解析层会落到 `.proposed`。**其他 hat**：不要订阅、不要处理 |
| `<X>` | `precheck-<X>` gate | **下游 consumer**：与未启用 precheck 时相同，正常处理 |
| `<X>.rejected` | `precheck-<X>` gate | **任何 hat 都不要处理** — runtime 已注入 `task.resume` 打回 `on_fail.target` |
| `precheck-<X>` 轮次 | 调度器 | **gate hat**：按 instructions 二选一 emit `<X>` 或 `<X>.rejected`，禁止沉默或双发 |

---

## Producer 行为

- 仍按原 topic 名 emit（例如 `ralph emit review.complete ...`）。脱糖后实际写入的是 `review.complete.proposed`。
- 收到 `task.resume`（precheck 打回）后：读 payload 里的 `failed_checks` / `reason`，针对检查点修改产物，**再 emit 同一业务 topic**；不要手 emit `*.proposed`。
- `retry_budget`（再试上限）默认 3：连续被拒会打回 producer；耗尽后 loop 发 `plan.blocked(reason=precheck_failed)`，**不要**自己重开 loop 或绕过 gate。

---

## Gate hat（`precheck-<X>`）行为

- 一轮 activation **必须且只能** emit `<X>`（过）或 `<X>.rejected`（不过）。
- `<X>.rejected` payload 须含 `failed_checks` 与 `reason`（runtime 在脱糖时自动注入 schema）。
- 只做 **主观 checklist**（读产物、判断是否有实质内容）；git/test/task 等机械检查归 `execution_contracts` / `event_policy`，不要塞进 checklist。

---

## 紧急关停

```bash
RALPH_PRECHECK_MODE=off ralph run ...
```

环境变量为 `off` 时，即使 YAML 里 `precheck.enabled: true` 也**不脱糖、不跑 gate**。

---

## 反模式

- ❌ 手 emit `<X>.proposed` 绕过 producer 改写 — origin guard 会拒收
- ❌ 下游 hat 订阅 `<X>.proposed` 或 `<X>.rejected` — 拓扑错误；consumer 只订 `<X>`
- ❌ 把 git diff / test pass / task 关闭等塞进 `precheck.rules.<X>.prompt` — 用 execution_contracts / event_policy
- ❌ 看到 `<X>.rejected` 后自己再 emit bare `<X>` — 等 `task.resume` 打回 producer 重做
- ❌ 与 `ralph emit --policy-check` 混淆 — policy 失败在正式 emit 前（`--policy-check` 本身不写盘）；precheck 失败在 gate hat 轮次
- ❌ 收到 protocol correction 后跳过 `--policy-check` 直接真实 emit — precheck 失败则本 activation 不得继续写盘 emit
- ❌ 同类 protocol violation 无限自由 retry — 第二次同类违规 runtime 阻塞 loop（见 `ralph-tools-recovery-directives` Correction 优先级）

---

## 配置形状（仅供识别，作者请读 guide）

```yaml
event_loop:
  precheck:
    enabled: true
    rules:
      review.complete:
        prompt:
          - "findings 有实质内容，不是 placeholder"
        on_fail:
          target: review-synthesizer
          retry_budget: 3
          on_exhausted: "plan.blocked(reason=precheck_failed)"
          reason: "subjective checklist failed"
```
