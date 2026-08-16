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

# Dynamic workflow verification.
# review REQUIRES actual verify report evidence; static-pass-only is not
# sufficient. Success, failure/block, and no-output/abnormal-output
# scenarios are required; add recovery/terminal-closure cases when
# applicable. Failure to run verify, missing scenario file, or any
# non-pass scenario blocks review (see `references/finding-rubric.md`).
ralph preset verify -H <path|builtin:name> --scenario <scenario.yml> --format json
ralph preset verify -H <path|builtin:name> --scenario <scenario.yml> --format human

# Workspace preset_lint 子集
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint
```

`--strict`：Warn 级 finding 也视为失败。JSON 输出供 review 报告 Mechanical Lint 节摘录。

scope topic 的 `payload_consistency` 规则还会检查 finding
`preset.payload_consistency_scope_positive_assertion`：不要用
`exists:true` / `non_empty:true` 表达合法字段存在，也不要用会命中合法值的正向 threshold 谓词；这些检查由 scope guard 和 schema 负责。规则只能表达同一 payload 内的非法矛盾（Hit = 拒绝）。

## Schema / emit 验证

```bash
# 某 topic 的 payload 字段 SSOT（**只验 shape**）
ralph emit --schema <topic> -H <path|builtin:name>

# 写盘前策略预检（OPAC Precheck；**shape + 拓扑 ownership**，不验字段可见性 / 值源 / 语义）
ralph emit --policy-check --output json <topic> '<payload>' -H <path|builtin:name>

# envelope 层 triggered 拓扑预检（仅用于已确认的跨 hat 直达例外）
ralph emit --policy-check --output json --triggered <different-hat-id> <topic> '<payload>' -H <path|builtin:name>

# emit ↔ hat 通道桥接校验（runtime 视角）
ralph tools task verify-emit-bridge ...
```

`--triggered` 是事件的目标 hat，不是来源 hat。普通业务 handoff 必须省略它，让 isolated runtime / CLI 按 topic 的唯一消费者自动推导；但若 schema 声明 `EventSchema.required_target_hat`，必须显式指定该字段声明的目标（包括允许终态 reporter 的合法 self-target）。目标必须与 schema 和 `policy-check` 一致。只有确需跨 hat 直达、且 author notes 记录目标、原因和拓扑证据时才允许使用；其它 self-target 仍禁止。详见 `crates/ralph-core/data/ralph-tools-emit.md`「Envelope 校验」段。
`verify-emit-bridge` 的完整参数见 `crates/ralph-core/data/ralph-tools-tasks.md` 的 OPAC Precheck 段；本 reference 只保留入口名，避免复制 runtime command table。

**policy-check feedback 审核点**：JSON 输出中的 error item 可能包含 `field`、`reason_code`、`expected`、`actual`、`field_description`、`suggested_payload_shape`、`suggested_command`、`payload_index`、`gate`、`referenced_fields`。这些字段只说明 runtime 如何拒收和建议 agent 怎么修 shape；review 仍要检查 `field_description` 是否来自正确的 `field_docs`，`suggested_payload_shape` 是否没有伪造业务事实，batch 错误是否保留 `payload_index`。当 `reason_code` 是 `semantic_gate_violation` 时，`gate` 携带触发的 gate 标识（如 `payload_consistency:<rule_id>`），`referenced_fields` 列出该规则 `when` 谓词声明的所有 payload 字段路径——review 须确认 `field` 没有被用来承载 gate ID、agent 能只靠结构化字段完成定位。

**review 含义**：上面四条命令只能排除 shape / 拓扑 ownership 类问题，并观察 policy-check 的 agent-facing 修复面。**字段可见性 / 值源 / 身份 / 语义 / 下游消费 必须由 review 从 activated-hat 视角独立审**。详见 `references/agent-native-model.md`「Payload 审计模型」段。

**`candidate_emit.next_hat_candidates` 三态**：`ralph inspect prompt --hat <id> --format json --topic ... --payload ...` 的 JSON 输出中 `candidate_emit.next_hat_candidates` 字段有三种形状（由 `kind` 标签区分）：
- `{"kind":"verified","hats":["<hat_id>",...]}` — 所有订阅者都是 config 中已注册的 hat，或当前 topic 没有订阅者（空路由被视为可验证的空集合）。
- `{"kind":"unverified"}` — topology 证据不可得 / 无法从 config 验证路由。
- `{"kind":"mixed","entries":[{"hat_id":"...","verified":true|false},...]}` — 混合态，部分订阅者不在 config 中。review 需注意 `entries` 中 `verified: false` 的 hat 在运行时无法路由，应确认它们不是业务拓扑的一部分。

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

# 场景化激活预览
ralph -c <preset>.yml inspect prompt --hat <hat_id> --format json \
    --trigger <TOPIC> --source-hat <hat_id> --payload '<JSON>' \
    [--iteration N] [--wave-context <JSON>] [--orchestrator-context <JSON>] \
    [--correction <JSON>] [--scratchpad <true|false>] [--tasks-enabled <true|false>] \
    [--memories-enabled <true|false>]

**`source_hat_known` 语义：**
当 `--source-hat` 提供的 hat id 在 config `hats` 映射中存在时，`trigger_context_injected.source_hat_known` 为 `true`；存在但不在 config 中时为 `false`；未提供 `--source-hat` 时该字段不序列化（`skip_serializing_if`）。这使得 reviewer 可以区分"已知 topology 成员"和"任意 Unicode ID"（不允许凭外观拒绝，也不混淆 matched_hints）。

# 候选 emit 干跑评估（Unit 2）
ralph -c <preset>.yml inspect prompt --hat <hat_id> --format json \
    --topic <TOPIC> --payload '<JSON>' [--triggered <hat_id>]

**失败停机条件：**
- `--topic` 必须是当前 hat 的 `publishes` 列表成员或 `default_publishes` 回退值，否则返回 `policy_decision: reject` + gate=`topic_publishes`（reason_code=`hat_does_not_publish_topic`）。
- `--triggered` 如果提供，必须是 config 中已注册的 hat id，否则返回 `policy_decision: reject` + gate=`triggered_not_in_topology`（reason_code=`triggered_hat_not_in_config`）。
- 省略 `--triggered` 为合法路径（降级到普通 emit 评估，不校验 triggered 拓扑）。

# Capability inventory（Unit 3）
ralph capability inventory --format {human|json}
```

外仓（无 `crates/ralph-core/data/`）同样可用——内容来自当前 ralph 二进制内嵌（`SkillRegistry::include_str!`）；报告须注明来源。详细规程见 [`prompt-visibility.md`](prompt-visibility.md)；audit 规程见 [`agent-skill-audit.md`](agent-skill-audit.md)。

## Preset 脚手架（author 拓扑阶段）

```bash
ralph preset list
ralph preset show <template>
ralph preset new <template> --output .ralph/hats/my.yml
ralph preset diff --file <path>   # 与 template 基线对比
```

## Builtin artifact 模板落盘（binary-only）

某些 preset（如 `parallel-forge`）的 **fill-in artifact 模板**（development-plan / unit / manager-report 等）在编译期嵌入 `ralph` 二进制。部署机没有源码树时，hat 必须先 materialize，再 `cp` 到业务路径填写：

```bash
ralph preset materialize-artifacts parallel-forge --plan-key <plan-key>
# 默认写出：.ralph/forge/<plan-key>/templates/

ralph preset materialize-artifacts builtin:parallel-forge --plan-key <plan-key> --dest /tmp/templates
```

与 `ralph preset new` 不同：`new` 生成 **preset YAML 脚手架**；`materialize-artifacts` 抽出 **运行时填写模板**。幂等可重复执行。

> **机制通用性**：当前 runtime 仅内嵌 `parallel-forge` 的模板目录（`presets/templates/parallel-forge/`）。若新 preset 需要采用同样的模板文件机制压缩 hat instructions，需同步扩展 `crates/ralph-cli/src/builtin_artifact_templates.rs` 与 `build.rs` 的模板内嵌列表，或改用本地文件路径方案。
## 合入前升级（非默认）

```bash
./scripts/run-tests.sh
cargo nextest run -p ralph-core --test scenarios
scripts/check-cli-doc-drift.sh --strict
```

## Lint 失败时 review 行为

机械 lint 失败时 **仍继续** AAF 评审；Executive Summary 须标注 lint 通过/失败及 Error 计数。

## Capability inventory

```bash
# 列出 preset-facing capability 清单（read-only；AAF 评审前必读）
ralph capability inventory --format json
ralph capability inventory --format human
```

JSON 输出结构：
```json
{
  "version": "capability_inventory/v1",
  "capabilities": [
    {
      "id": "wave-emit",
      "trigger_signal": "execution_model == wave | supervisor+wave",
      "applies_when": "preset uses ralph wave emit / ralph wave verify",
      "evidence_sources": ["skills/.../finding-rubric.md", "crates/.../ralph-tools-wave.md"],
      "recommended_evidence_level": "runtime",
      "source": "binary_embedded"
    }
  ]
}
```

**用途**：AAF 评审前，对照此清单确认 preset 作者已理解所有依赖的 runtime capability，并检查 `covered_in_author_review`（静态审文档 vs 运行时验证）。

## Capability coverage

<!-- anchor: wave-emit -->
<!-- anchor: supervisor-emit -->
<!-- anchor: task-id-live -->
<!-- anchor: artifact-first -->
<!-- anchor: payload-consistency -->
<!-- anchor: trigger-context -->
<!-- anchor: key-stage-event-gate --><!-- anchor: evidence-bound --><!-- anchor: scope-handoff -->

## Wave 子命令

| 命令 | 说明 |
|------|------|
| `ralph wave emit <TOPIC> --payloads-stdin` | 将多个 payload 作为 wave 事件发射；必须先 `wave verify` 再 `emit` |
| `ralph wave verify <TOPIC> --payloads-stdin` | OPAC Precheck：校验 payload 符合 event policy，写入一次性 ticket |
| `ralph wave inspect <WAVE_ID>` | 公开只读 Confirm：查询 wave 在 supervisor store 的登记状态（phase / counts） |
| `ralph wave redrive --wave-id <ID> [--slots <LIST>]` | 为已关闭 wave 的失败 slot 创建子 attempt wave；仅限 operator 在 loop 外手动干预 |

## Scope handoff contract（merge-batch / post-merge-converge / red-team-attack presets）

```bash
# scope topic 的 policy-check（强制，不可被 --unsafe-no-policy-check 绕过）
ralph emit --policy-check <scope-topic> '<payload>' -H <path|builtin:name>

# 校验 scope topic payload 字段（只验 shape）
ralph emit --schema <scope-topic> -H <path|builtin:name>
```

**Scope topics**：`merge.integrated` / `merge.stabilized` / `postmerge.changemap.ready` / `redteam.plan.resolved`。

**强制门禁**：scope handoff guard 对这些 topic 是**强制不可绕过**的，`--unsafe-no-policy-check` 不能跳过。指令必须要求 agent 先 `--policy-check` 预检，通过后再真实 emit。

**Manifest 路径规则**：`scope_manifest_path` 必须落在 `.ralph/{merge,post-merge,red-team}/` 下，且文件必须在 emit 前已落盘可读。

**Digest 计算**：`scope_digest` 是 canonical JSON（排除 `scope_digest` 字段本身）的 SHA-256 64-char hex。

**Threshold gate**：`overall_confidence >= 90` 且 `critical_unknown_count == 0` 且 `proceed == true` 时 scope 才能标记为 resolved。

**边界**：reviewer 在审涉及 scope topic 的 hat 时，必须验证 `instructions` 中明确要求 agent 先写 manifest、再 policy-check、再真实 emit。
