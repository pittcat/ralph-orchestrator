# Preset Validation Commands

## Preset 路径写法

| 类型 | `-H` 示例 |
|---|---|
| Builtin | `builtin:debug` |
| Repo 内 YAML | `presets/en/debug.yml` |
| Local | `.ralph/hats/my-workflow.yml` |

可加 `-c ralph.yml` 指定 core config（local preset 常用）。Custom 项目配置文件名同样支持：`-c myapp.yml` 或 `RALPH_CONFIG=myapp.yml` 都会走 discovery SSOT（`ConfigSource::File` → `$RALPH_CONFIG` → `ralph.yml` / `ralph.yaml`），agent-facing tool (`ralph tools task`、`ralph emit` 等) 都会读到同一份。Runner 启动 hat 子进程时会自动注入 `RALPH_CONFIG`，AAF 评审不再要求把 custom 文件 symlink 成 `ralph.yml`。

## 机械门禁（review 默认）

```bash
# Preset runtime contract（config + lint + topology + orphan + payload）
ralph preset check -H <path|builtin:name> --strict
ralph preset check -H <path|builtin:name> --strict --format json

# Workspace preset_lint 子集
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint
```

`--strict`：Warn 级 finding 也视为失败。JSON 输出供 review 报告 Mechanical Lint 节摘录。

## Schema / emit 验证

```bash
# 某 topic 的 payload 字段 SSOT（**只验 shape**）
ralph emit --schema <topic> -H <path|builtin:name>

# 写盘前策略预检（OPAC Precheck；**shape + 拓扑 ownership**，不验字段可见性 / 值源 / 语义）
ralph emit --policy-check --output json <topic> '<payload>' -H <path|builtin:name>

# envelope 层 triggered 拓扑预检（与 payload schema 分开）
ralph emit --policy-check --output json --triggered <hat-id> <topic> '<payload>' -H <path|builtin:name>

# emit ↔ hat 通道桥接校验（runtime 视角）
ralph tools task verify-emit-bridge ...
```

`--triggered` 必须是 preset `hats[]` 里声明的 hat id，否则 `triggered_not_in_topology`（apply 与 `--policy-check` 均 enforce）。缺省 `--triggered` 允许。ralph-control / orchestrator diagnostic topic 跳过 topology check。详见 `crates/ralph-core/data/ralph-tools-emit.md`「Envelope 校验」段。
`verify-emit-bridge` 的完整参数见 `crates/ralph-core/data/ralph-tools-tasks.md` 的 OPAC Precheck 段；本 reference 只保留入口名，避免复制 runtime command table。

**policy-check feedback 审核点**：JSON 输出中的 error item 可能包含 `field`、`reason_code`、`expected`、`actual`、`field_description`、`suggested_payload_shape`、`suggested_command`、`payload_index`、`gate`、`referenced_fields`。这些字段只说明 runtime 如何拒收和建议 agent 怎么修 shape；review 仍要检查 `field_description` 是否来自正确的 `field_docs`，`suggested_payload_shape` 是否没有伪造业务事实，batch 错误是否保留 `payload_index`。当 `reason_code` 是 `semantic_gate_violation` 时，`gate` 携带触发的 gate 标识（如 `payload_consistency:<rule_id>`），`referenced_fields` 列出该规则 `when` 谓词声明的所有 payload 字段路径——review 须确认 `field` 没有被用来承载 gate ID、agent 能只靠结构化字段完成定位。

**review 含义**：上面四条命令只能排除 shape / 拓扑 ownership 类问题，并观察 policy-check 的 agent-facing 修复面。**字段可见性 / 值源 / 身份 / 语义 / 下游消费 必须由 review 从 activated-hat 视角独立审**。详见 `references/agent-native-model.md`「Payload 审计模型」段。

**Trigger Context（preset `event_policy.schemas.<topic>.trigger_context`）命令边界**：`ralph preset check --strict` 与 `ralph emit --schema <topic>` 只能验证 `trigger_context.summary_fields` 字段引用、`routing_hints[*].conditions[*].{op, value}` 形状、`label` 唯一性、以及 `trigger_context` 与下游 hat `triggers` / `subscribes_to` 的拓扑消费方关系（lint ID 见 `references/finding-rubric.md`）。**它们不能证明 hint `guidance` 与下游 hat 实际决策分支语义一致，也不能验证 hat `instructions` 是否仍在复制 hint 条件值**——这两项是 review 必须独立审的软性 AAF / payload-content 缺口。

## Hat 检查（local / 路径 preset）

```bash
ralph hats validate -c ralph.yml -H <hats.yml>
ralph hats show -c ralph.yml -H <hats.yml> <hat_id>
ralph hats graph -c ralph.yml -H <hats.yml> --format mermaid
```

`ralph hats show` 可看单 hat 有效配置；**不是**完整 isolated prompt dump。

## Prompt 可见性（author / review / diagnose 三规程必用）

```bash
# 默认 human 输出：块清单 + skill 表
ralph -c <preset>.yml inspect prompt --hat <id>

# JSON（合同 / fixture / lint 自动化 SSOT）
ralph -c <preset>.yml inspect prompt --hat <id> --format json

# --full：JSON 返回真实 prompt_body，human 打印完整 body（不 suppressed）
ralph -c <preset>.yml inspect prompt --hat <id> --format json --full
ralph -c <preset>.yml inspect prompt --hat <id> --format human --full
```

外仓（无 `crates/ralph-core/data/`）同样可用——内容来自当前 ralph 二进制内嵌（`SkillRegistry::include_str!`）；报告须注明来源。详细规程见 [`prompt-visibility.md`](prompt-visibility.md)；audit 规程见 [`agent-skill-audit.md`](agent-skill-audit.md)。

## Preset 脚手架（author 拓扑阶段）

```bash
ralph preset list
ralph preset show <template>
ralph preset new <template> --output .ralph/hats/my.yml
ralph preset diff --file <path>   # 与 template 基线对比
```

## 合入前升级（非默认）

```bash
./scripts/run-tests.sh
cargo nextest run -p ralph-core --test scenarios
scripts/check-cli-doc-drift.sh --strict
```

## Lint 失败时 review 行为

机械 lint 失败时 **仍继续** AAF 评审；Executive Summary 须标注 lint 通过/失败及 Error 计数。

## Wave 子命令

| 命令 | 说明 |
|------|------|
| `ralph wave emit <TOPIC> --payloads-stdin` | 将多个 payload 作为 wave 事件发射；必须先 `wave verify` 再 `emit` |
| `ralph wave verify <TOPIC> --payloads-stdin` | OPAC Precheck：校验 payload 符合 event policy，写入一次性 ticket |
| `ralph wave inspect <WAVE_ID>` | 公开只读 Confirm：查询 wave 在 supervisor store 的登记状态（phase / counts） |
| `ralph wave redrive --wave-id <ID> [--slots <LIST>]` | 为已关闭 wave 的失败 slot 创建子 attempt wave；仅限 operator 在 loop 外手动干预 |
