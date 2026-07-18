# 上下文审计契约

> 这份文档写给 `ralph-project-bootstrap` skill 的实施者：审计阶段如何在不读任何未来配置、不调用 backend 的前提下确认目标项目根与输入。

## 触发条件

operator 调用 skill 时，必须先通过上下文审计。任何持久写盘、`ralph emit`、`ralph run` 都在审计通过之后才允许发生。

## audit 阶段要做什么

1. 解析 cwd 与潜在的 VCS 根（`.git` 父目录）。
2. 从 cwd 向上收集 `AGENTS.md` / `CLAUDE.md` 的可见 scope。
3. 在 VCS 根与最近的 AGENTS/CLAUDE scope 冲突时，立即阻塞并返回 `root_ambiguous`。
4. 在没有 VCS 根且存在多个 AGENTS scope 时，按相同规则阻塞。
5. 在 root 唯一时，继续校验 preset 与 plan/task 输入。
6. 输入校验失败或 root 解析失败时返回 `AuditDecision(blocking=True)`，**不**调用 helper 写盘。

## 命令与动作

- 不发出 `ralph` 子命令、不写任何 owned 文件、不动 `.ralph`。
- 不复制 hat instructions、不预生成 `ralph.pipeline.yml`。
- 输出 `AuditDecision` 结构体：`root`、`inputs_ok`、`facts`、`issues`、`blocking`、`notes`。

## 关键字段从哪里取得

- `root` 解析：`audit_project_root` 走 VCS 根优先，缺则走最近的 AGENTS/CLAUDE scope，否则使用 cwd。
- `inputs_ok`：`audit_inputs` 校验 `preset` 与 `plan_path`。
- `facts`：`collect_project_facts` 通过文件存在性判断技术栈（Rust/Node/Python/unknown）。
- `issues`：审计发现的可枚举阻塞，全部带 `code` + `message` + `paths`。
- `notes`：附加可观察说明（如未发现任何可证实命令）。

## 失败停止条件

- `AuditDecision.blocking == True` 时，后续阶段（U3 写盘、U4 配置、U5 CLI 验证、U6 smoke、U7 handoff）必须停止并把决策直接转交 operator。
- 任何持久写盘之前必须再次调用 `audit_project_root` 复核：cwd 改变、VCS 根变化、或 AGENTS scope 漂移都重新阻塞。
- 输入路径相对性由 `_paths.is_safe_relative` 校验：绝对路径与 `..` 转义都视为非法。

## 实现位置

- `scripts/audit.py`：`run_audit` 与具体分类逻辑
- `scripts/_paths.py`：路径规范化
- `scripts/_fixtures.py`：单元测试用的 project 构造器