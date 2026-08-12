---
date: 2026-07-25
topic: ralph-e2e-bootstrap-cross-repo-intent
status: problem-frame
note: 本文只记录现状、需求与困境，不包含方案设计。
related:
  - docs/plans/2026-07-24-006-feat-ralph-e2e-bootstrap-skill-plan.md
  - skills/ralph-e2e-bootstrap/SKILL.md
  - skills/ralph-e2e-bootstrap/scripts/plan_diff.py
  - skills/ralph-e2e-bootstrap/scripts/sandbox_suite.py
  - skills/ralph-e2e-bootstrap/references/interaction.md
  - skills/tests/test_plan_diff_edge_cases.py
---

# ralph-e2e-bootstrap 跨仓 plan 场景：需求与困境（问题框）

## 1. 一句话

`ralph-e2e-bootstrap` 当前对 **plan 与 target sandbox 在同一 git 仓** 是 happy path;对 **plan 在 ralph-orchestrator 仓、target sandbox 是另一产品仓** 的合法 cross-repo 场景,`scripts/plan_diff.py` 误报 `scope_drift` / `plan_stale`,caller 只能强行喂 `repo_root=plan_repo_root` 才能拿正确 verdict。本文只把现状、需求、困境写清楚——**不写方案**。

## 2. 目前 skill 是怎么样的（与本问题相关的事实）

### 2.1 skill 的边界(006 plan 与 SKILL.md 已落地)

- **Inputs**:caller 供应 (1) development plan 路径 (2) E2E sandbox 目录。
- **Outputs**:`<sandbox>/ralph.<stem>.yml` + `<sandbox>/PROMPT.<stem>.md` + 静态门 + 启动命令。
- **静态门**(R7):`capability` → `preset check --strict` → `preflight --strict` → `ralph run --dry-run`。
- **Handoff**:`static_only: true` + `not_live_run` clause(R10),不做 live loop。
- **No mutation**(R14):不写 `crates/`,不写 `presets/`,不写 caller plan 文件。
- **No Rust / CLI 改动**:本次实现要落地的所有逻辑都必须装在 `skills/ralph-e2e-bootstrap/**` + `skills/tests/**` 里。

### 2.2 U2 plan × diff audit 的现状实现

`scripts/plan_diff.py` 的 `run_audit(plan_path, repo_root, diff_provider)`:

- 当 `repo_root` 缺省时,默认回退到 `plan_path.parent`(L246-249)。
- `git diff --name-only HEAD` 在 `repo_root` 跑,得 `diff_paths`。
- `_classify` 把 `diff_paths` 和 `plan_intent_paths` 做 **depth-2 prefix 比较**:
  - 不匹配 → emit `CLARIFY_SCOPE_DRIFT`
  - plan 有 U-IDs 但 diff 空 → emit `CLARIFY_STALE_PLAN`
- `AuditDecision` 当前无 `plan_repo_root` / `diff_repo_root` 字段,只是 `plan_path` + `plan_intent_paths` + `diff_paths`。

### 2.3 U4 sandbox suite 的现状实现

`scripts/sandbox_suite.py` 的 `generate_suite(...)`:

- 把 caller plan 字节哈希后,stage 进 `<sandbox>/docs/plans/<basename>`(R13:源 plan 不动)。
- 写 `ralph.<stem>.yml`(preset-bound config)和 `PROMPT.<stem>.md` 到 sandbox 仓根。
- argv 用 sandbox-relative path(`--plan docs/plans/<basename>`),machine-portable。
- `hats_source` 字段写 `builtin:<preset-name>`(skill 不在 sandbox 新建 preset,只复用 builtin)。

### 2.4 combo-box 决策点(decision-point table)

`SKILL.md` 与 `references/interaction.md` 都列 6 个决策点(S2-S4 + R8/R10/R13):

```
plan_diff_clarify / binary_resolution / preset_gap /
write_conflict / argv_shape / live_run
```

每个决策点配 combo-box 默认推荐(R4/R5/R6/R8/R10/R12/R13,R14 外)。

### 2.5 当前 cross-repo 场景走出的轨迹

已知 caller 形态:plan 在 ralph-orchestrator 仓,target sandbox 是 ralph-supervisor 仓(独立产品仓)。当前跑出的轨迹:

- `run_audit` 默认 `repo_root=plan_path.parent`(= ralph-supervisor/docs/plans/) → `git diff` 在 ralph-orchestrator 仓 cwd 跑 → 拿到 ralph-orchestrator 仓空 diff → emit `CLARIFY_STALE_PLAN`。
- 显式 `repo_root=ralph-supervisor/` → `git diff` 在 ralph-supervisor 仓跑 → 拿到 ralph-supervisor 仓 diff(profiles, sandbox 仓的 preset-bound pair) → depth-2 prefix 比较 plan intent(`crates/`) 对 diff(`PROMPT.*.md` / `ralph.*.yml`) → emit `CLARIFY_SCOPE_DRIFT`。
- 任一情况,skill 都会 raise `plan_diff_clarify` combo-box 给 operator。operator 的"接受 plan intent 继续"选项目前没显式登记的"cross-repo"语义,只能强行选"接受 plan intent"。

## 3. 需求(已澄清部分)

以下条目来自 2026-07-25 对话对齐,**尚未进入方案设计**。

### 3.1 目标范围

- **N1.** skill 在 **plan 与 sandbox 跨 git 仓** 的合法场景下,plan_diff audit 应当**识别**为合法 pattern,而不是 `scope_drift` / `plan_stale` 误报。
- **N2.** plan 与 sandbox 同仓 happy path 行为**不变**;现有 5 个 plan_diff edge case 测试 (T2.1-T2.5) 必须继续绿。
- **N3.** 沙箱产物(preset-bound pair argv)行为**不变**:src `sandbox_suite.py` 已经正确把 plan 字节按 sandbox 仓路径 stage + 用 sandbox-relative argv,这块不需要改。
- **N4.** 不引入 live `ralph run`、不写 `crates` 或 `presets`、不越过 skill 边界做 preset authoring / diagnostics。

### 3.2 误报需要消除的具体形态

- **N5.** `repo_root` 显式指向 sandbox 仓根、plan 在 ralph-orchestrator 仓、sandbox 仓有 diff 时,**不能** emit `CLARIFY_SCOPE_DRIFT`。
- **N6.** `repo_root` 显式指向 sandbox 仓根、plan 在 ralph-orchestrator 仓、sandbox 仓 diff 为空 时,**不能** emit `CLARIFY_STALE_PLAN`(因为 diff 仓不是 plan 的仓,plan 谈不到 stale)。
- **N7.** 跨仓场景需要一种状态码(或显式 audit 字段)让 caller 知道 "plan 与 sandbox 跨 git 仓是事实",便于落库 / 写 handoff evidence。

### 3.3 caller 体验

- **N8.** caller 仍按现有调用形状供给:plan 路径 + sandbox 路径 + 可选 `repo_root`。不应为了处理 cross-repo 强制 caller 多学一组 API。
- **N9.** `AuditDecision` 上应当能拿到 `plan_repo_root` / `diff_repo_root` / `cross_repo` 字段,便于 caller 写日志、写 handoff evidence、做 audit trail。

### 3.4 决策边界

- **N10.** 与 `CLARIFY_*` 已有 5 个状态码同等级,新增状态码(如 `CLARIFY_CROSS_REPO_INTENT`)需要:
  - 在 `references/interaction.md` 决策表里有一行,因为 `SKILL.md` 要求每个决策点带 combo-box(R12)。
  - 同时,这个 combo-box 默认推荐是"接受 plan intent 跑 skill"——因为 cross-repo 是合法 pattern,不是问题。
- **N11.** combo-box 决策点表的 screw-anchor:`SKILL.md` combo-box section 顶部有硬要求"R12 decision points surfaced one at a time"。新增 cross_repo_intent 必须不破这个约束。

### 3.5 既有资产 / 测试

- **N12.** `skills/tests/test_plan_diff_edge_cases.py` 已 5 个用例 (T2.1-T2.5):intent_undeclared / unit_missing / prefix_depth_boundary / mixed_clarify_codes / unicode_path_in_plan。新增 cross-repo 用例至少 2 个:(a) 跨仓 + diff 非空 (b) 跨仓 + diff 空。
- **N13.** `test_e2e_bootstrap_contract.py` / `test_e2e_bootstrap_e2e.py` / `test_install.py` + `test_project_bootstrap_contract.py` (188/188) **必须继续绿**——本次改动不能破 contract 锚。
- **N14.** `SKILL.md` 的 Boundaries / Workflow / Static Gates / Guardrails 段落**不动**;只允许在 `references/interaction.md` 决策表里追加一行 + 在 `scripts/plan_diff.py` 加常量 + 测试里加 fixture。

## 4. 尚未拍板的事(开放问题)

- **O1.** `CLARIFY_CROSS_REPO_INTENT` 是否要进 combo-box 决策点表?目前只在"audit 字段 + 状态码"层面就要,还是必须落在 combo-box 决策表里(显式 R12 决策)?
- **O2.** cross-repo 场景下,默认推荐"接受 plan intent 跑 skill"是直接跳过 combo-box(让 `ok=True` 当下生成 sandbox suite),还是依然 raise combo-box 给 operator 一次确认?
- **O3.** "跨仓"判定粒度:基于磁盘路径的 git toplevel 比较?还是基于 caller 显式声明的 `repo_root` 与 plan_path 的 git toplevel 比较?这是 audit 内部的事,但影响 AuditDecision 字段语义。
- **O4.** `scripts/plan_diff.py` 的 `_git_diff_paths` 失败/超时 → `diff_unavailable=True` 时,跨仓场景该怎么处理?是不是仍然 emit `CLARIFY_DIFF_UNAVAILABLE` 而非 `CLARIFY_CROSS_REPO_INTENT`?
- **O5.** plan-stale 在跨仓场景下的"空 diff"判定:plan 仓本身有可能有 diff(改了 orchestrator 源码),但 sandbox 仓没 diff。plan_stale 是看哪个仓的 diff 状态?

## 5. 运营方当前正在试的 instance

为了把"实测证据"留在 record 里,这一节记录本次对话看到的 1 次真实尝试:

- plan:**这 005 plan**(在 ralph-supervisor 仓物理位置,但内容描述改 ralph-orchestrator 仓源码)
- sandbox:`~/Dev/agent_tools/ralph-supervisor/`(独立产品仓)
- preset:`builtin:ce-executor-supervisor`
- 跑出的轨迹:cross-repo 误报 `scope_drift` / `plan_stale`,caller 只能强行喂 `repo_root` 走默认推荐"接受 plan intent"。

skill 跑出 launch command 后,实测 events.jsonl 等中间产物不在 skill 边界内(它是 `ralph-run-diagnosis` 的边界),本需求不要求扩展到那一侧。

## 6. 范围之外(显式排除)

- **不**让 skill 解析 events.jsonl / hat-channel / supervisor.db(skill 不做 post-run diagnosis)。
- **不**让 skill 在 sandbox 仓新建 preset(`ralph-preset-author` 是那一侧)。
- **不**让 skill 改 ralph 源码 / crates / preset YAML 文件 / 启动参数之外的 argv。
- **不**清理 ralph-supervisor 仓根的前一轮错产物(ralph.ce-executor-supervisor.yml / PROMPT.ce-executor-supervisor.md / .ralph/agent/e2e-bootstrap-handoff.md —— 是上次对话失败记录,git ops 单独走,不在 skill 修复范围)。
- **不**重写 005 plan 本身(005 plan 是 cross-repo 场景的 input,不是 skill 修复对象)。
- **不**触碰 `documents/brainstorms/` 下的其他历史 brainstorm 文件。
