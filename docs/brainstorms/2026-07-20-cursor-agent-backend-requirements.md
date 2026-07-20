---
date: 2026-07-20
topic: cursor-agent-backend
---

# Cursor Agent Backend（`agent`）

## Problem Frame

Ralph 已支持多个 CLI agent backend（`claude`、`gemini`、`codex`、`opencode`、`pi`、`traecli`），但还不能把 [Cursor Headless CLI](https://cursor.com/docs/cli/headless)（`agent -p`）当作一等公民后端使用。目标是在 Ralph loop 里提供一条 **可观测的 headless Cursor 执行路径**：无人值守改代码，并能看到助手文本与工具调用（对标 Claude backend 的文本/工具可见性子集，而非完整功能平替）。

## Requirements

**Backend 注册与发现**
- R1. 新增可配置 backend 名 `agent`，对应 Cursor CLI 二进制 `agent`。
- R2. 用户可通过显式选择使用该 backend（例如 `ralph run -b agent` / `ralph run --backend agent` 或等价配置）。
- R3. `auto` 检测应识别 PATH 上的 `agent`，但优先级低于现有已支持的后端；有更优先后端可用时不抢占。
- R3a. `agent` 进入与现有 backends 一致的 CLI allowlist 与 shell 补全暴露面。

**Headless 执行契约**
- R4. 默认调用形态为 print/headless：`-p` / `--print`。
- R5. v1 **固定**附带 `--force` 与 `--trust`（工厂默认写死，不可通过 backend args 关闭），以便自动化 loop 能直接改文件且不卡在 workspace trust 提示。
- R6. 默认输出格式为 `stream-json`，使 runtime 能消费增量文本与工具事件（而非仅最终纯文本）。

**可观测性（Claude 平替）**
- R7. Ralph 应解析 Cursor `stream-json` 事件，至少把助手文本与工具开始/完成事件映射到现有 stream 回调语义（文本可见、工具调用可见）。
- R8. Cursor CLI 进程非零退出时，Ralph 应按现有 backend 失败路径处理，并尽量保留/透传 stderr 诊断信息。

**鉴权**
- R9. 默认沿用本机已有 Cursor 登录态（例如先前执行过 `agent login`）；缺少 `CURSOR_API_KEY` 时 Ralph **不**硬失败预检。
- R10. 鉴权失败由 Cursor CLI 自身报错；Ralph 不额外发明一套鉴权流程。

## Success Criteria

- 在已安装并已登录 Cursor CLI 的机器上，用 `-b agent` 跑一次典型 Ralph hat/loop，能实际改文件并完成（不因缺 `--force`/`--trust` 卡住）。
- 运行过程中能看到助手文本与至少一类工具调用的开始/完成信息（映射到现有 stream 回调：文本可见、工具可见；未知 `stream-json` 字段忽略；不要求与 Claude 事件分类一一对应）。
- `-b auto` 在同时存在 `claude` 与 `agent` 时仍优先选现有更高优先级后端；仅当更高优先级后端不可用且 `agent` 在 PATH 时才选中 `agent`。
- 未设置 `CURSOR_API_KEY` 但本机已 `agent login` 的环境可以启动；Ralph 不因“缺 API key”单独拒绝。

## Scope Boundaries

**v1 包含**
- Headless/`--print` 路径作为唯一支持的执行模式
- 默认 `--force` + `--trust` + `--output-format stream-json`
- Backend 名 `agent`、auto 检测（低优先级）、与现有 CLI allowlist / 补全的一致暴露

**v1 不包含**
- `agent acp` / ACP 执行路径
- Cursor session resume / `--continue`
- Cursor 自带的 `--worktree` / worktree setup
- 把 Ralph 的 model 配置透传为 `--model`
- Interactive 模式工厂（类似其它后端的 `*_interactive`）
- 强制要求或管理 `CURSOR_API_KEY` 的预检流程
- Sandbox / MCP approve 等 Cursor 高级开关的一等配置面

## Key Decisions

- **目标定位 = 可观测 headless Cursor 路径**：需要 `stream-json` 与工具可见性；不是 text-only MVP，也不是完整 Claude 功能平替。
- **配置名 = `agent`**：与 Cursor CLI 二进制名一致。
- **默认且固定 `--force` + `--trust`**：无人值守 loop 优先；v1 工厂写死，不允许用 backend args 关掉。
- **auto 纳入但靠后**：有 `agent` 可用，但不改变现有用户对 `claude` 等后端的默认偏好。
- **PATH 同名误检：v1 不做二次校验**：信任用户 PATH 上的 `agent` 即为 Cursor CLI；规划仅可评估零成本防护，不改变该基线除非另开范围。
- **鉴权信任本机 login**：不把 API key 预检做成硬门禁；v1 目标环境是「开发者本机已 `agent login`」，不以 CI/无桌面 runner 为成功前提。
- **v1 只做 headless**：把 ACP / resume / Cursor worktree / model 透传明确推迟。
- **可观测性上限**：助手文本 + tool 开始/完成；未知 `stream-json` 字段忽略；不要求与 Claude 事件分类一一对应。定位表述为「可观测的 headless Cursor 执行路径（对标 Claude 的文本/工具可见性子集）」，而非完整 Claude 功能平替。

## Dependencies / Assumptions

- 目标环境已安装 Cursor CLI，且命令名为 `agent`（与官方安装文档一致）。
- Cursor `--output-format stream-json` 的事件形状以官方文档为准（`system` / `assistant` / `tool_call` / `result` 等）；规划阶段需对照文档与实测样本确认解析边界。
- 仓库当前无 Cursor agent backend；现有后端通过 `CliBackend` 工厂 + 可选专用 stream 解析扩展（已有 `claude` / `pi` / `traecli` 先例）。
- v1 服务对象是本机已 `agent login` 的开发者；不以 CI/无桌面 runner 为成功前提（`CURSOR_API_KEY` 若存在可透传，但不做硬门禁）。
- 已知残余风险（接受）：固定 `--force`+`--trust` 且 v1 不做 Cursor sandbox 配置面、PATH 不做二次校验；依赖用户保证二进制可信，并优先在隔离工作区（如 Ralph worktree）运行。

## Alternatives Considered

- **Text-only 薄封装**：上线快，但达不到文本/工具可观测性目标，否决为 v1 主路径。
- **复用 Claude StreamJson 解析器**：Cursor 事件 schema 不同，误判/丢事件风险高，否决。
- **配置名 `cursor` / `cursor-agent`**：更不易撞名，但与二进制不一致；用户选择与 CLI 对齐的 `agent`。
- **`--force`/`--trust` 可覆盖关闭**：否决；v1 固定写死以保障无人值守落盘。

## Outstanding Questions

### Resolve Before Planning

（无）

### Deferred to Planning

- [Affects R7][Needs research] Cursor `stream-json` 与现有 `pi`/`traecli`/`claude` 解析器在事件字段上的具体差异；哪些工具类型必须映射、哪些可忽略未知字段。
- [Affects R3][Technical] `auto` 优先级列表中 `agent` 的精确插入位置；PATH 同名误检的零成本防护是否值得做（v1 基线：不做二次校验，见 Key Decisions）。
- [Affects R6][Technical] 是否默认追加 `--stream-partial-output`（字符级增量），或接受工具间消息级增量即可。
- [Affects R7][Technical] 工具可见性验收以 PtyExecutor/TUI 观测路径为准，还是 headless `CliExecutor` 路径也必须同级解析。
- [Affects R4][Needs research] 大 prompt（30KB+ / temp-file 间接提示）下 `agent -p` 是否会卡住；PromptMode 与超时策略。

## Next Steps

-> 开发计划已就绪：`docs/plans/2026-07-20-001-feat-cursor-agent-backend-plan.md`（按 Unit 1→6 串行执行）
