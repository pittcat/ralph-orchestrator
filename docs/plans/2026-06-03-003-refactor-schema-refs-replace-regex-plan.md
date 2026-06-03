# Plan: 用显式 `schema_refs` 替换 `payload_contract.rs` 的正则提取

> **Scope**: 仅替换静态 payload 字段引用的来源（层面 B）。不改 `event_policy.schemas` 格式（不做层面 A），不改运行时 `event_policy.rs` 的事件 payload 校验语义，不做 Promptfoo，不添加正则 fallback。

---

## 1. 背景与问题

`crates/ralph-core/src/payload_contract.rs` 当前用三个正则从 hat `instructions` 文本中**猜**字段依赖：

- `From event payload: field1, field2`
- `payload MUST include: field1, field2`
- 含 payload 意图行里的反引号字段

**问题：**
1. **不可靠**：正则漏匹配会 fail-open，匹配错会 false positive。
2. **无法区分 topic**：所有提取到的字段都会被归因到当前 trigger topic，但 instructions 里可能引用多个 topic 的字段。
3. **与提示词话术耦合**：改 instructions 文案就可能破坏静态校验。
4. **preset 覆盖脆弱**：builtin preset 依赖隐式文本契约，没有结构化兜底。

**目标：** 给每个 hat 增加显式 `schema_refs` 字段，直接声明“本 hat 会读取哪些 topic 的哪些 payload 字段”，用结构化配置彻底替换正则提取逻辑。

**非目标：**
- 不改变事件发布方的 schema 定义方式。
- 不改变运行时 `EventPolicy` 对 payload 的校验规则。
- 不做旧正则与新 `schema_refs` 的运行时双轨兼容。
- 不把 `schema_refs` 注入 agent prompt；它只服务静态校验和诊断。

---

## 2. 方案设计

### 2.1 `schema_refs` 数据结构

在 `HatConfig` 中新增：

```rust
/// 显式声明本 hat 从各 topic 的 payload 中读取的字段列表。
/// Key = topic 名称，Value = 该 topic 下被引用的字段名列表。
/// 用于替代从 instructions 文本正则提取字段的逻辑。
#[serde(default)]
pub schema_refs: HashMap<String, Vec<String>>,
```

YAML 使用方式：

```yaml
hats:
  coordinator:
    triggers: ["work.start"]
    publishes: ["work.ready", "work.failed"]
    schema_refs:
      work.done:
        - plan_name
        - task_id
        - task_key
        - step
      fix.applied:
        - plan_name
        - task_id
        - task_key
        - step
    instructions: |
      ...
```

**设计决策：**
- `schema_refs` 是 **topic -> fields** 映射，不是 trigger-bound 映射。一个 hat 可以声明任意 topic 的字段依赖，无论该 topic 是否在当前 `triggers` 中。
- `ignore_payload_fields` 暂时保留，作用于 `schema_refs` 中的字段，用于排除已知静态误报或过渡期残留字段。
- 字段去重、空字段过滤在提取函数中完成，不依赖 YAML 作者手工保持唯一。
- topic key 也要 trim；空 topic 直接忽略，避免产生不可诊断的 `(none)` 错误。
- 字段名不做新正则合法性限制。原因是 `EventSchema.required_fields` 已经是字符串集合，静态校验应当和 schema 定义保持同一字段命名能力。

### 2.2 `PayloadFieldRef` 结构体

`PayloadFieldRef` 从“文本提取诊断”转成“结构化字段引用诊断”：

```rust
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PayloadFieldRef {
    pub hat_id: String,
    pub field: String,
    #[serde(default)]
    pub topic: String,
    #[serde(default)]
    pub line: Option<usize>,
    #[serde(default)]
    pub pattern: Option<String>,
    #[serde(default)]
    pub source_excerpt: Option<String>,
}
```

**字段顺序建议：**
- 保持 `hat_id`、`field` 在前，减少现有调试输出变化。
- `topic` 必须存在于新对象中；如果确认没有任何持久化反序列化路径，可以不加 `serde(default)`，但实现前必须用 `rg "PayloadFieldRef" crates tests docs` 确认。若有任何 fixture、JSON、诊断文件反序列化可能性，就必须加 `serde(default)`。
- `line`、`pattern`、`source_excerpt` 改成 `Option`，新结构化来源默认 `None`。

### 2.3 `extract_payload_field_refs` 改造

**当前签名：**

```rust
pub fn extract_payload_field_refs(
    hat_id: &str,
    instructions: &str,
    ignore_fields: &[String],
) -> Vec<PayloadFieldRef>
```

**新签名：**

```rust
pub fn extract_payload_field_refs(
    hat_id: &str,
    schema_refs: &HashMap<String, Vec<String>>,
    ignore_fields: &[String],
) -> Vec<PayloadFieldRef>
```

**新行为：**
1. 构建 `ignore_set`。
2. 遍历 `schema_refs` 的 `(topic, fields)`。
3. 对 topic 执行 `trim()`；空 topic 跳过。
4. 对每个 field 执行 `trim()`；空 field 跳过。
5. 过滤 `ignore_payload_fields` 中的 field。
6. 基于 `(topic, field)` 去重。
7. 生成 `PayloadFieldRef { hat_id, topic, field, line: None, pattern: None, source_excerpt: None }`。
8. 稳定排序：先 `hat_id`，再 `topic`，再 `field`。

**必须删除：**
- `from_payload_regex`
- `must_include_regex`
- `backtick_field_regex`
- `backtick_intent_regex`
- `extract_comma_separated_fields`
- `ExtractionPattern`
- `regex::Regex` import（如果该文件不再使用 Regex）

### 2.4 `validate_payload_contract` 改造

**当前逻辑：**

```text
对每个 registry hat:
  找到 hat_config.instructions 和 ignore_payload_fields
  对每个 subscription topic:
    从 instructions 正则提取 refs
    把所有 refs 归因到当前 subscription topic
    检查该 topic 是否有 schema、field 是否在 required_fields
```

**新逻辑：**

```text
对每个 registry hat:
  找到 hat_config.schema_refs 和 ignore_payload_fields
  refs = extract_payload_field_refs(hat_id, &schema_refs, &ignore_payload_fields)
  按 refs.topic 分组
  对每个 topic:
    查找 source hats
    查找 event_policy.schemas[topic]
    strict=true 且 schema 缺失 -> error
    strict=false 且 schema 缺失 -> warning
    schema 存在但 field 不在 required_fields -> error
    schema 存在且 field 在 required_fields -> pass

对 config.hats 中存在但 registry 未注册的 hat:
  保持现有处理策略；如果当前代码已有 config-only hat 校验，必须同步改为 schema_refs 来源
```

**关键变化：**
- 不再按 `hat.subscriptions` 遍历字段引用；字段引用直接来自 `schema_refs` 的 topic。
- 不再读取 `instructions` 文本。
- 校验结果中的 `topic` 直接来自 `schema_refs` key。
- wildcard topic 不再通过 subscription 特判跳过；如果 `schema_refs` 里出现 `work.*`，应当按普通 topic 处理并因为缺少 schema 报错或 warning。计划不鼓励在 `schema_refs` 中写 wildcard。
- `source_hats_by_topic` 构建逻辑保留，用于错误消息。

### 2.5 `PayloadContractError` 与输出格式

`PayloadContractError` 目前已经有 `instructions_line: Option<usize>`、`pattern: Option<String>`、`source_excerpt: Option<String>`。实现时要确认所有构造点都继续填 `None` 或旧测试需要的 `Some`。

输出格式要求：
- `hats validate` 和 `preset_validator` 不能打印 `line=None pattern=None` 这种噪声作为主诊断。
- 推荐格式：
  - 有文本来源时：`line=<n> pattern=<pattern>`
  - 无文本来源时：`source=schema_refs`
- 错误必须包含 `hat`、`topic`、`field`、`source_hats`、`schema_defined_in`。

### 2.6 `ignore_payload_fields` 的兼容边界

`ignore_payload_fields` 继续生效，但语义从“忽略正则误提取字段”变为“忽略结构化字段引用中的已知例外”。这不是 runtime schema 放行机制。

实现和文档都要明确：
- `ignore_payload_fields` 只影响静态 payload contract validator。
- `ignore_payload_fields` 不影响 `event_policy.rs` 的运行时 payload 校验。
- 如果某字段确实被 hat 读取，优先更新 schema，而不是长期依赖 ignore。

---

## 3. 实施步骤

### Phase 0: 变更前基线与调用点清点

**目标：** 在动代码前固定回归基线，避免实现后无法判断是计划导致还是现有状态导致。

#### Step 0.1: 检查工作区状态

```bash
rtk git status --short
```

要求：
- 记录已有未提交文件，不能误改或回滚用户已有变更。
- 本计划文件自身修改可以存在，但源码实现前要知道基线。

#### Step 0.2: 全量搜索相关调用点

```bash
rtk grep "PayloadFieldRef|extract_payload_field_refs|validate_payload_contract|PayloadContractError|instructions_line|source_excerpt|pattern" crates/ralph-core/src crates/ralph-cli/src
rtk grep "HatConfig \\{" crates/ralph-core/src crates/ralph-cli/src
rtk grep "ignore_payload_fields|From event payload|payload MUST include|schema_refs" crates/ralph-core/src crates/ralph-cli/src docs presets crates/ralph-core/data
```

必须把结果归入实现清单。当前已知需要关注：
- `crates/ralph-core/src/config.rs`
- `crates/ralph-core/src/payload_contract.rs`
- `crates/ralph-core/src/preset_validator.rs`
- `crates/ralph-cli/src/hats.rs`
- `crates/ralph-cli/src/doctor.rs`
- `crates/ralph-cli/src/loop_runner.rs`
- `crates/ralph-core/src/event_loop/tests.rs`
- `crates/ralph-core/src/preflight.rs`
- `crates/ralph-core/src/wave_prompt.rs`
- 所有 `presets/**/*.yml`
- `docs/guide/payload-contracts.md`
- `presets/COLLECTION.md`
- `presets/schemas/*.yml` 中涉及 payload contract 的注释
- `AGENTS.md` 和 `CLAUDE.md`
- `crates/ralph-core/data/*.md`

#### Step 0.3: 记录旧提取结果基线

在修改 `payload_contract.rs` 前，用一次性脚本或临时 Rust test 记录旧正则对 builtin preset 的字段提取结果。

基线记录内容：
- preset 名称
- hat id
- trigger topic
- 正则提取字段
- line/pattern/source_excerpt
- strict validation 的 errors/warnings

保存位置建议：
- 临时文件放在系统临时目录，例如 `$TMPDIR/ralph-schema-refs-baseline.json`，不提交。
- 如果需要提交长期测试 fixture，只能放到合适的 `tests/fixtures/` 并明确它是测试资产，不是临时输出。

注意：旧基线不是新实现的“错误数上限”。新 `schema_refs` 可以比旧正则更准确，也可能暴露旧正则漏掉的真实缺口。基线用于防止遗漏旧字段，不用于要求新 errors 是旧 errors 的子集。

---

### Phase 1: 数据结构 + Rust 核心改造

**目标：** Rust 核心改完后，workspace 编译通过，`payload_contract` 单测和相关 CLI/validator 输出测试通过。

#### Step 1.1: 改 `config.rs`，新增 `schema_refs`

- **文件**: `crates/ralph-core/src/config.rs`
- **位置**: `HatConfig` 结构体，建议放在 `instructions`/`extra_instructions` 后或 `ignore_payload_fields` 前。
- **修改**:
  - 新增 `#[serde(default)] pub schema_refs: HashMap<String, Vec<String>>`
  - 更新字段注释，明确它服务静态 payload contract validator。
  - 在 `impl Default for HatConfig` 中初始化 `schema_refs: HashMap::new()`

**必须验证：**
- `serde_yaml` 读取没有 `schema_refs` 的旧 YAML 时仍然成功。
- `serde_yaml` 读取含 `schema_refs` 的 YAML 时字段完整进入 `HatConfig`。
- `HashMap` 已在 `config.rs` 当前 import 范围内可用；若已有 `HashMap` import，不重复添加。

#### Step 1.2: 全量修复 `HatConfig` struct literal

新增字段后，所有没有 `..Default::default()` 的 `HatConfig { ... }` 都可能编译失败。

执行：

```bash
rtk grep "HatConfig \\{" crates/ralph-core/src crates/ralph-cli/src
```

当前已知位置必须逐一处理：
- `crates/ralph-cli/src/doctor.rs`
- `crates/ralph-cli/src/loop_runner.rs`
- `crates/ralph-core/src/event_loop/tests.rs`
- `crates/ralph-core/src/preflight.rs`
- `crates/ralph-core/src/wave_prompt.rs`

处理原则：
- 生产代码里可读性优先；如果字段很多且已有全部字段显式列出，补 `schema_refs: HashMap::new()` 或 `schema_refs: Default::default()`。
- 测试代码优先改成 `..Default::default()`，减少未来字段新增的维护成本。
- 不要顺手重构无关测试逻辑。

#### Step 1.3: 改 `payload_contract.rs`，替换提取函数

- **文件**: `crates/ralph-core/src/payload_contract.rs`
- **修改**:
  1. `PayloadFieldRef` 新增 `topic: String`。
  2. `PayloadFieldRef.line`、`pattern`、`source_excerpt` 改为 `Option`，并按 2.2 决策添加 `serde(default)`。
  3. `extract_payload_field_refs` 改为接收 `schema_refs`。
  4. 删除所有正则提取代码和相关 helper/enum/import。
  5. 去重 key 从 `(hat_id, field)` 改为 `(topic, field)` 或 `(hat_id, topic, field)`；单 hat 内 `(topic, field)` 已足够，但排序仍包含 `hat_id`。
  6. 空 topic、空 field、trim 后空字符串都过滤。
  7. 返回值稳定排序为 `(hat_id, topic, field)`。

#### Step 1.4: 改 `validate_payload_contract`

- **文件**: `crates/ralph-core/src/payload_contract.rs`
- **修改**:
  1. 从 `hat_config.schema_refs` 获取 refs，不再从 `instructions` 获取 refs。
  2. 不再按 `hat.subscriptions` 遍历来套字段。
  3. 按 refs 的 topic 分组校验。
  4. 保留 `source_hats_by_topic` 构建逻辑。
  5. 保留 strict/default 对 schema 缺失的 error/warning 行为。
  6. 保留 field 不在 `required_fields` 时的 `FieldMissingFromSchema` 行为。
  7. 确认 config-only hats 如果现有代码覆盖，也使用同一 schema_refs 逻辑。

**不得引入的回归：**
- hatless/solo 模式仍应 pass。
- 没有 `event_policy` 的配置仍应按现有语义处理 schema 缺失。
- `default_publishes` 仍应进入 source hats 计算。
- registry hat publishes 仍应进入 source hats 计算。
- wildcard subscription 不应再影响 schema_refs 校验结果。

#### Step 1.5: 改错误格式化和 CLI 输出

涉及文件：
- `crates/ralph-core/src/preset_validator.rs`
- `crates/ralph-cli/src/hats.rs`

要求：
- 输出里必须可见 `hat`、`topic`、`field`。
- 没有 line/pattern/source_excerpt 时显示 `source=schema_refs`，不要把 `None` 当主要信息打印出来。
- 旧测试中人工构造 `Some(line/pattern/source_excerpt)` 的情况仍可读。
- 全局搜索 `.unwrap()`，确认没有对这些 Option 做不安全 unwrap：

```bash
rtk grep "instructions_line.*unwrap|pattern.*unwrap|source_excerpt.*unwrap|\\.line.*unwrap" crates/ralph-core/src crates/ralph-cli/src
```

#### Step 1.6: 配置解析测试

新增或改写配置层测试，覆盖：
- 旧 YAML 没有 `schema_refs` 时 `HatConfig.schema_refs` 为空。
- YAML 中有单 topic 多字段时能读入。
- YAML 中有多 topic 多字段时能读入。
- `schema_refs` 和 `ignore_payload_fields` 可以同时存在。
- 中文 preset YAML 中 `schema_refs` 字段解析不受 instructions 语言影响。

#### Step 1.7: Phase 1 编译与定向测试

```bash
rtk cargo check -p ralph-core
rtk cargo check -p ralph-cli
rtk cargo test -p ralph-core payload_contract
rtk cargo test -p ralph-core preset_validator
rtk cargo test -p ralph-cli hats
```

如果 `ralph-cli hats` 没有定向测试，至少运行 `rtk cargo test -p ralph-cli` 或覆盖该输出路径的现有测试。

---

### Phase 2: 给所有 preset YAML 添加 `schema_refs`

**目标：** 所有 builtin preset 都有完整结构化字段引用，`ralph hats validate --strict` 对 builtin 集合通过。

#### Step 2.1: 枚举真实 preset 清单

不要依赖“37 个”这个数字。实现前必须从仓库真实清单枚举：

```bash
rtk find "presets/**/*.yml"
rtk sed -n '1,220p' presets/index.json
```

清单应按实际 builtin manifest 分组：
- `presets/en/*.yml`
- `presets/zh/*.yml`
- `presets/extras/*.yml`
- `presets/minimal/*.yml`
- `presets/schemas/*.yml`
- 其他 manifest 引用的 preset 文件

验收标准里使用“manifest 中所有 builtin preset”，而不是硬编码 37。

#### Step 2.2: 编写一次性提取/辅助脚本

脚本用途：辅助生成初稿，不作为最终真相。

脚本要求：
1. 加载 YAML，保留 hat id、triggers、publishes、instructions。
2. 用旧三类规则提取字段：
   - `From event payload: ...`
   - `payload MUST include: ...`
   - payload 意图行中的反引号字段
3. 输出候选项时必须包含 line、pattern、source_excerpt。
4. 不自动写回文件，先输出 diff 或 JSON。
5. 默认 topic 只能标记为 `TODO_REVIEW_TOPIC` 或候选 trigger；不能静默归入第一个 trigger。
6. 生成一份“需要人工确认”的列表，特别标出：
   - hat 有多个 trigger
   - instructions 同时提到多个 topic
   - line 里有 `based on trigger`、`depending on trigger`、`work.done` 这类 topic 提示
   - 字段名在多个 topic schema 中都存在

脚本保存策略：
- 如果只是开发辅助，放 `/tmp`，不提交。
- 如果它能长期帮助维护 `schema_refs`，放 `scripts/`，并在文档中说明用途。

#### Step 2.3: 人工填写 `schema_refs`

优先级：
1. `presets/en/ce-executor.yml`
2. `presets/en/code-assist.yml`
3. `presets/en/pdd-to-code-assist.yml`
4. `presets/en/review.yml`
5. `presets/en/research.yml`
6. `presets/en/debug.yml`
7. `presets/en/autoresearch.yml`
8. `presets/en/hatless-baseline.yml`
9. `presets/en/merge-loop.yml`
10. `presets/zh/*.yml`
11. `presets/extras/*.yml`
12. `presets/minimal/*.yml`
13. `presets/schemas/*.yml` 和 deprecated/reference copies

填写原则：
- 只填 hat 实际会读取的字段。
- topic 必须是字段真实来源 topic，而不是 hat 的 trigger topic。
- 如果 instructions 说“根据 trigger 读取不同字段”，必须按每个 trigger topic 分开写。
- 如果 hat 不读取 payload 字段，省略 `schema_refs`；不要写空 map 造成视觉噪声。
- 如果字段确实被读取但 schema 缺失，优先补 schema 的 `required_fields`，不要用 `ignore_payload_fields` 掩盖。
- `ignore_payload_fields` 只用于确认不是真字段依赖的残留项。

#### Step 2.4: 每个 preset 的本地验证节奏

每改完一个 preset：

```bash
rtk cargo run -p ralph-cli -- hats validate -H builtin:<preset> --strict
```

如果失败：
- 先判断是 `schema_refs` topic/field 写错，还是 `event_policy.schemas` 缺失真实 required field。
- 如果是真实读取字段但 schema 缺字段，补 schema。
- 如果是误引用，删 `schema_refs` 或加入 `ignore_payload_fields`，并在 preset 旁边保留最小注释说明。

#### Step 2.5: 全 builtin 自动化验证

新增一个脚本或测试，自动从 `presets/index.json` 枚举 builtin preset 并执行 strict validate。

可选实现：
- Rust 集成测试：直接加载 manifest 和 preset，调用 validator。
- Shell 脚本：`scripts/validate-builtin-presets.sh`，循环执行 `cargo run -p ralph-cli -- hats validate -H builtin:<name> --strict`。

验收要求：
- 不允许只手动跑 3 个示例 preset。
- 输出必须列出失败 preset 名称。
- CI 可运行；如果脚本太慢，至少 Rust 测试必须覆盖所有 manifest preset。

---

### Phase 3: 文档、skill 和注释更新

**目标：** 所有面向 agent/维护者的说明都不再教授旧正则契约，`crates/ralph-core/data` 的源码引用不漂移。

#### Step 3.1: 更新用户文档

必须更新：
- `docs/guide/payload-contracts.md`

更新内容：
- `Extractor Behaviour` 改为 `schema_refs` 行为。
- 删除或迁移旧的 `From event payload` / `payload MUST include` 正则说明。
- 增加 YAML 示例。
- 说明 `schema_refs` 的 topic 是字段来源 topic，不要求在 `triggers` 中。
- 说明 `ignore_payload_fields` 的新语义和限制。
- 增加“新增 hat 时如何维护 payload contract”的 checklist。

#### Step 3.2: 更新 preset 维护文档和 schema 注释

必须检查并按需更新：
- `presets/COLLECTION.md`
- `presets/schemas/*.yml`
- 所有 preset 中解释 payload contract 的注释

当前已知旧表述：
- `presets/COLLECTION.md` 中有 `From event payload` 检查项。
- `presets/schemas/code-assist.yml` 中有 “mirrors the explicit payload MUST include” 注释。

#### Step 3.3: 更新 agent 指令文档

必须检查：
- `AGENTS.md`
- `CLAUDE.md`

如果新增或修改任何内容：
- 两个文件必须保持完全一致。
- 修改后执行：

```bash
rtk diff AGENTS.md CLAUDE.md
diff -u AGENTS.md CLAUDE.md
```

`diff -u` 必须无输出。

#### Step 3.4: 更新 `crates/ralph-core/data` skill 文档并反向验证

必须检查：

```bash
rtk grep "payload_contract|From event payload|payload MUST include|schema_refs|\\.rs:[0-9]+-[0-9]+" crates/ralph-core/data/*.md
```

处理规则：
- 如果 `crates/ralph-core/data/*.md` 中有旧正则说明，改成 `schema_refs`。
- 如果有源码行号引用，如 `xxx.rs:NN-MM`，必须用 `sed -n 'NN,MMp' <file>` 逐条复核。
- 如果源码改动导致行号漂移，必须同步修正文档。
- 如果没有相关引用，也要在最终实现总结中写明“已检查 `crates/ralph-core/data/*.md`，无相关行号引用需要更新”。

如果改动涉及 `ralph tools` 子命令语法或 skill 文档引用的命令行为，还必须跑对应 `ralph <cmd> --help` 或 skill 中列出的冒烟命令。本计划本身不改 `ralph tools` 子命令，预期只需要文档引用反向验证。

---

### Phase 4: 回归验证与清理

#### Step 4.1: 格式化和静态检查

```bash
rtk cargo fmt --check
rtk cargo clippy --workspace --exclude ralph-e2e --all-targets -- -D warnings
```

如果仓库现有 clippy 基线无法 `-D warnings` 通过，记录现有失败并至少运行：

```bash
rtk cargo clippy --workspace --exclude ralph-e2e --all-targets
```

#### Step 4.2: workspace 编译

```bash
rtk cargo check --workspace --exclude ralph-e2e
rtk cargo build --workspace --exclude ralph-e2e
```

#### Step 4.3: 测试

优先使用仓库推荐入口：

```bash
rtk ./scripts/run-tests.sh
```

如果 `cargo-nextest` 不可用，脚本会 fallback。若脚本本身失败，再按失败范围定向补跑。

必须额外确认：

```bash
rtk cargo test -p ralph-core payload_contract
rtk cargo test -p ralph-core smoke_runner
rtk cargo test -p ralph-core scenarios
rtk cargo run -p ralph-e2e -- --mock
```

#### Step 4.4: preset strict validate 全量通过

运行 Phase 2.5 的全量 builtin preset 验证脚本或测试。

最低要求：
- manifest 中每个 builtin preset 都跑到。
- 每个失败都必须修复，不能在验收里标为 known failure。

#### Step 4.5: 无临时文件

```bash
rtk git status --short
rtk ls "$TMPDIR" | rtk grep "ralph-schema-refs-"
```

要求：
- 不提交 `/tmp` 输出。
- 不提交一次性脚本，除非已经改造成长期维护脚本并有文档。
- 不提交 cargo 生成物、日志、临时 baseline JSON。

---

## 4. 文件修改清单

| # | 文件 | 修改类型 | 说明 |
|---|------|---------|------|
| 1 | `crates/ralph-core/src/config.rs` | 新增字段 | `HatConfig` 新增 `schema_refs: HashMap<String, Vec<String>>` |
| 2 | `crates/ralph-core/src/config.rs` | 修改 | `impl Default for HatConfig` 初始化 `schema_refs` |
| 3 | `crates/ralph-core/src/payload_contract.rs` | 重写 | `PayloadFieldRef`、`extract_payload_field_refs`、`validate_payload_contract` |
| 4 | `crates/ralph-core/src/payload_contract.rs` | 删除 | `ExtractionPattern`、`extract_comma_separated_fields`、所有 Regex 提取逻辑 |
| 5 | `crates/ralph-core/src/payload_contract.rs` | 重写 | 单元测试和 YAML fixture 集成测试 |
| 6 | `crates/ralph-core/src/preset_validator.rs` | 适配 | payload contract 错误格式化处理 `schema_refs` 来源 |
| 7 | `crates/ralph-cli/src/hats.rs` | 适配 | CLI 输出处理 `schema_refs` 来源 |
| 8 | `crates/ralph-cli/src/doctor.rs` | 补全 | 硬编码 `HatConfig` 实例补字段或改 `..Default::default()` |
| 9 | `crates/ralph-cli/src/loop_runner.rs` | 补全 | 硬编码 `HatConfig` 实例补字段或改 `..Default::default()` |
| 10 | `crates/ralph-core/src/event_loop/tests.rs` | 补全 | 测试中的 `HatConfig` 构造适配 |
| 11 | `crates/ralph-core/src/preflight.rs` | 补全 | 测试或 helper 中的 `HatConfig` 构造适配 |
| 12 | `crates/ralph-core/src/wave_prompt.rs` | 补全 | 测试 helper 中的 `HatConfig` 构造适配 |
| 13 | `presets/en/*.yml` | 新增 | 各 hat 按真实字段依赖添加 `schema_refs` |
| 14 | `presets/zh/*.yml` | 新增 | 中文 preset 同步 `schema_refs` |
| 15 | `presets/extras/*.yml` | 新增 | extras preset 同步 `schema_refs` |
| 16 | `presets/minimal/*.yml` | 新增 | minimal preset 同步 `schema_refs` |
| 17 | `presets/schemas/*.yml` | 更新 | reference/deprecated schema 注释同步新契约 |
| 18 | `presets/index.json` | 检查 | 确认全量 builtin validate 枚举来源；通常不改 |
| 19 | `scripts/validate-builtin-presets.sh` 或 Rust 测试 | 新增/可选 | 自动枚举所有 builtin preset 做 strict validate |
| 20 | `docs/guide/payload-contracts.md` | 更新 | 文档更新为 `schema_refs` |
| 21 | `presets/COLLECTION.md` | 更新 | preset 维护 checklist 移除旧正则契约 |
| 22 | `AGENTS.md` | 检查/更新 | 如有相关描述则更新 |
| 23 | `CLAUDE.md` | 检查/更新 | 与 `AGENTS.md` 保持完全一致 |
| 24 | `crates/ralph-core/data/*.md` | 检查/更新 | skill 文档和源码行号引用反向验证 |

---

## 5. 测试策略

### 5.1 `extract_payload_field_refs` 单元测试

文件：`crates/ralph-core/src/payload_contract.rs`

| 测试名 | 输入 | 期望 |
|---|---|---|
| `extract_empty_schema_refs_returns_empty` | `{}` | 空 Vec |
| `extract_single_topic_single_field` | `{"work.ready": ["task_id"]}` | 1 个 ref，topic=`work.ready` |
| `extract_single_topic_multiple_fields` | `{"work.ready": ["task_id", "plan_name"]}` | 2 个 ref |
| `extract_multiple_topics` | `{"work.ready": ["task_id"], "work.done": ["step"]}` | topic 分别正确 |
| `extract_preserves_same_field_across_topics` | `{"a": ["task_id"], "b": ["task_id"]}` | 2 个 ref，不跨 topic 去重 |
| `extract_deduplicates_same_topic_field` | `{"work.ready": ["task_id", "task_id"]}` | 1 个 ref |
| `extract_trims_fields` | `{"work.ready": [" task_id ", "\tplan_name\n"]}` | 输出 trim 后字段 |
| `extract_filters_empty_fields` | `{"work.ready": ["task_id", "", "   "]}` | 只保留 `task_id` |
| `extract_filters_empty_topics` | `{"": ["task_id"], "   ": ["x"], "work.ready": ["task_id"]}` | 只保留 `work.ready` |
| `extract_ignore_fields_filters_exact_field` | ignore `plan_name` | `plan_name` 不出现 |
| `extract_ignore_fields_does_not_cross_trim_bug` | schema field `" plan_name "` + ignore `plan_name` | trim 后被忽略 |
| `extract_ignore_fields_same_name_all_topics` | topic a/b 都有 `task_id` + ignore `task_id` | 两个 topic 都过滤 |
| `extract_stable_sort_by_hat_topic_field` | 乱序 HashMap/fields | 输出稳定排序 |
| `extract_sets_structured_source_fields_to_none` | 任意 schema_refs | `line/pattern/source_excerpt` 都是 `None` |
| `extract_does_not_read_instructions_text` | instructions 中含旧模式但 schema_refs 空（如果保留测试 helper） | 空 Vec，证明不再正则提取 |

### 5.2 `PayloadFieldRef` serde 测试

文件：`crates/ralph-core/src/payload_contract.rs` 或合适的 serde 测试模块

| 测试名 | 输入 | 期望 |
|---|---|---|
| `payload_field_ref_serializes_topic` | 新 ref | JSON/YAML 包含 `topic` |
| `payload_field_ref_deserializes_with_optional_source_fields_missing` | 缺 `line/pattern/source_excerpt` | 反序列化成功，字段为 `None` |
| `payload_field_ref_deserializes_old_shape_if_supported` | 只有 `hat_id/field/line/pattern/source_excerpt` | 如果决定兼容旧诊断，反序列化成功且 `topic=""`；如果不兼容，删除此测试并在实现说明中写明无持久化路径 |

### 5.3 `validate_payload_contract` 单元/集成测试

文件：`crates/ralph-core/src/payload_contract.rs`

复用或改造现有 YAML fixture helper。测试 YAML 不再依赖 instructions 文案。

| 测试名 | 场景 | 期望 |
|---|---|---|
| `validator_empty_hats_passes` | 无 hats | pass |
| `validator_hat_without_schema_refs_passes` | hat 有 triggers 但无 schema_refs | pass |
| `validator_schema_refs_empty_map_passes` | `schema_refs: {}` | pass |
| `validator_missing_schema_strict_is_error` | strict=true，topic 无 schema | `SchemaMissingForRequiredTopic` error |
| `validator_missing_schema_default_is_warning` | strict=false，topic 无 schema | warning，不是 error |
| `validator_field_not_in_required_fields_is_error` | schema 存在但缺 field | `FieldMissingFromSchema` |
| `validator_all_fields_in_required_fields_passes` | schema required_fields 覆盖全部 refs | pass |
| `validator_multiple_topics_one_missing_schema` | 一个 topic 有 schema，一个没有 | 只对缺失 topic 报错/警告 |
| `validator_multiple_topics_one_missing_field` | 一个 topic 缺 field | 错误指向正确 topic |
| `validator_same_field_name_different_topic_uses_each_schema` | topic a/b 都读 `id` | 分别查各自 schema |
| `validator_ref_topic_need_not_be_subscription` | hat trigger=`work.start`，schema_refs=`work.done` | 校验 `work.done` |
| `validator_wildcard_subscription_does_not_skip_schema_refs` | trigger=`work.*`，schema_refs=`work.done` | 校验 `work.done` |
| `validator_schema_refs_wildcard_topic_is_not_special` | schema_refs=`work.*` | 按 `work.*` 查 schema，缺失则报错/警告 |
| `validator_default_publishes_in_source_hats` | source hat 通过 `default_publishes` 发布 | error source_hats 包含该 hat |
| `validator_registry_publishes_in_source_hats` | registry publishes 提供 source | error source_hats 包含 registry hat |
| `validator_multiple_source_hats_listed_sorted_deduped` | 多个 hats publish 同 topic | source_hats 稳定排序且去重 |
| `validator_ignore_payload_fields_excludes_error` | schema_refs 有缺失字段但 ignore | 不报该字段 |
| `validator_ignore_payload_fields_does_not_hide_other_fields` | ignore 一个字段，另一个缺 schema | 仍报另一个字段 |
| `validator_error_contains_hat_topic_field` | 任意 error | 错误结构含 hat/topic/field |
| `validator_error_source_is_schema_refs` | 新结构化来源 | line/pattern/source_excerpt 为 None，格式化显示 `source=schema_refs` |
| `validator_config_only_hat_uses_schema_refs` | config.hats 中有 hat 但 registry 没注册（如现有逻辑支持） | 行为与现有 config-only 策略一致，只改字段来源 |
| `validator_no_event_policy_strict_reports_schema_missing` | 无 event_policy + strict | 缺 schema error |
| `validator_no_event_policy_default_warns` | 无 event_policy + strict=false | warning |

### 5.4 `HatConfig` 配置解析测试

文件：`crates/ralph-core/src/config.rs` 现有 config 测试模块或新增测试模块

| 测试名 | 场景 | 期望 |
|---|---|---|
| `hat_config_default_schema_refs_empty` | `HatConfig::default()` | `schema_refs.is_empty()` |
| `hat_config_deserializes_without_schema_refs` | 旧 YAML | 成功且空 map |
| `hat_config_deserializes_schema_refs` | YAML 含 schema_refs | map 内容正确 |
| `hat_config_deserializes_schema_refs_with_ignore_fields` | 同时有 `schema_refs` 和 `ignore_payload_fields` | 两者都正确 |
| `ralph_config_deserializes_preset_with_schema_refs` | 最小完整 `RalphConfig` YAML | `config.hats[hat].schema_refs` 正确 |

### 5.5 CLI/格式化测试

文件：`crates/ralph-cli/src/hats.rs` 或现有 CLI snapshot/command tests

| 测试名 | 场景 | 期望 |
|---|---|---|
| `hats_validate_displays_schema_refs_source` | schema_refs 缺 field | 输出包含 `source=schema_refs` |
| `hats_validate_does_not_display_none_noise` | line/pattern/source_excerpt 为 None | 输出不含 `None` / `null` 主诊断噪声 |
| `preset_validator_formats_structured_error` | 调用 `format_payload_contract_error` | 包含 kind/hat/topic/field/schema/source |
| `preset_validator_formats_legacy_source_when_present` | 手工构造 Some line/pattern | 仍显示 line/pattern |

如果当前项目没有 CLI snapshot 体系，至少用 Rust 单测覆盖 formatter，外加手动命令冒烟：

```bash
rtk cargo run -p ralph-cli -- hats validate -H builtin:code-assist --strict
```

### 5.6 Preset 覆盖测试

| 测试名/脚本 | 场景 | 期望 |
|---|---|---|
| `all_builtin_presets_have_valid_schema_refs` | 枚举 `presets/index.json` 中所有 builtin | strict validate 全部通过 |
| `all_preset_yaml_parses` | 枚举 `presets/**/*.yml` | YAML 解析全部成功 |
| `schema_refs_topics_have_schema_or_are_intentionally_ignored` | 每个 schema_refs topic | strict validate 不报 schema missing |
| `schema_refs_fields_are_required_fields` | 每个 schema_refs field | 出现在对应 schema required_fields |
| `no_old_regex_contract_required_for_validation` | 删除/忽略 instructions 中旧短语也能 validate | validate 只依赖 schema_refs |

### 5.7 文档和 skill 反向验证测试

手动但必须执行：

```bash
rtk grep "From event payload|payload MUST include|Extractor Behaviour|schema_refs|payload_contract" docs presets crates/ralph-core/data/*.md AGENTS.md CLAUDE.md
rtk grep "\\.rs:[0-9]+-[0-9]+" crates/ralph-core/data/*.md
```

对每个 `xxx.rs:NN-MM`：

```bash
rtk sed -n 'NN,MMp' <file>
```

期望：
- 没有旧正则契约作为当前推荐方式出现。
- `crates/ralph-core/data/*.md` 的源码引用仍指向正确代码。
- `AGENTS.md` 与 `CLAUDE.md` 完全一致。

### 5.8 全量回归测试命令

最终必须跑：

```bash
rtk cargo fmt --check
rtk cargo check --workspace --exclude ralph-e2e
rtk cargo build --workspace --exclude ralph-e2e
rtk ./scripts/run-tests.sh
rtk cargo test -p ralph-core payload_contract
rtk cargo test -p ralph-core smoke_runner
rtk cargo test -p ralph-core scenarios
rtk cargo run -p ralph-e2e -- --mock
```

推荐额外跑：

```bash
rtk cargo clippy --workspace --exclude ralph-e2e --all-targets
rtk npm run test
```

`npm run test` 只有在 web/dashboard 相关文件被间接触碰或 workspace test 时间允许时才必须；本计划不预期触碰 web 代码。

---

## 6. 新旧对比验证口径

旧正则基线用于“防遗漏”，不是用于限制新实现不能报更多错。

必须检查：
- 每个旧正则提取出的 `(preset, hat, inferred_topic, field)` 都被人工确认：
  - 要么进入正确的 `schema_refs` topic；
  - 要么明确判定为旧正则 false positive，并不写入；
  - 要么判定为旧正则无法判断 topic，人工根据 instructions/schema 填到正确 topic。
- 新增的 `schema_refs` 字段必须能在 instructions 或 hat 行为中找到读取依据。
- 如果新 `schema_refs` 发现旧正则没发现的真实依赖，这是允许且期望的；必须补 schema 并让 strict validate 通过。

对比产物建议包含：

| preset | hat | old field | old inferred topic | new topic | decision |
|---|---|---|---|---|---|
| `code-assist` | `reviewer` | `task_id` | `review.ready` | `review.ready` | kept |
| `ce-executor` | `coordinator` | `plan_name` | ambiguous | `work.done` | moved to real source topic |
| `x` | `y` | `debug_only` | `z` | n/a | false positive |

---

## 7. 风险与缓解

| 风险 | 可能性 | 影响 | 缓解措施 |
|---|---:|---:|---|
| `schema_refs` 填写不完整，导致静态校验漏报 | 中 | 高 | 旧正则基线 + 人工 review + 所有 builtin strict validate + 文档 checklist |
| `schema_refs` 填错 topic，导致误报或漏报 | 高 | 高 | 每个多 trigger hat 必须人工确认 topic；测试覆盖 ref topic 不等于 trigger |
| 新增 `HatConfig` 字段导致 struct literal 编译失败 | 高 | 中 | `rg "HatConfig \\{"` 全量修复；workspace check |
| `PayloadFieldRef.topic` serde 不兼容旧诊断/fixture | 低 | 中 | 先搜索持久化路径；需要兼容则加 `serde(default)` 并补 serde 测试 |
| `line/pattern/source_excerpt` Option 输出变丑或 panic | 中 | 中 | formatter 测试；全局搜索 unwrap；输出 `source=schema_refs` |
| 删除正则后 instructions 中旧契约文案不再被校验 | 高 | 中 | 所有 preset 必须补 schema_refs；文档说明旧文案不再驱动校验 |
| 旧正则发现的字段在迁移中被漏掉 | 中 | 高 | 变更前基线记录；迁移决策表逐项处理 |
| `ignore_payload_fields` 被误用来绕过真实 schema 缺口 | 中 | 中 | 文档约束；测试只允许忽略已确认 false positive；review 时逐项检查 |
| `schema_refs` wildcard topic 语义不清 | 低 | 中 | 文档禁止；测试证明按普通 topic 处理 |
| preset YAML 大量编辑引入语法错误 | 中 | 中 | 每个 preset 改完 validate；全量 YAML parse test |
| `AGENTS.md`/`CLAUDE.md` 不一致 | 中 | 低 | 修改后 `diff -u AGENTS.md CLAUDE.md` 必须无输出 |
| `crates/ralph-core/data` skill 文档行号漂移 | 中 | 中 | grep `.rs:NN-MM`，逐条 `sed -n` 反向验证 |
| 一次性脚本或 baseline 文件误提交 | 低 | 低 | 最终 `git status` 清理；临时文件放 `/tmp` |

---

## 8. 回滚方案

- 所有修改在一个 feature branch 上完成。
- 建议分阶段提交：
  1. Rust 核心和测试。
  2. preset YAML 迁移。
  3. 文档、skill、反向验证脚本/测试。
- 如果 Phase 2 发现大量 preset 迁移问题，不能只回滚 YAML 并保留 Rust 核心合入，因为没有 `schema_refs` 会让静态校验失去覆盖。可以在本地回退 Phase 2 继续修，但最终合入必须包含完整 preset 迁移。
- 如果方案根本不成立，使用 git revert 回滚整个 branch。不添加正则 fallback。

---

## 9. 验收标准

- [ ] `rtk git status --short` 已确认没有无关源码改动或临时文件。
- [ ] `HatConfig.schema_refs` 已添加并有默认值。
- [ ] 所有 `HatConfig { ... }` struct literal 已修复或改为 `..Default::default()`。
- [ ] `payload_contract.rs` 不再使用 Regex 提取 instructions 字段。
- [ ] `extract_payload_field_refs` 只从 `schema_refs` 提取字段。
- [ ] `validate_payload_contract` 按 ref topic 校验，不再按 subscription topic 归因字段。
- [ ] `PayloadContractError` 输出包含 `hat/topic/field/source_hats/schema`，无 `None` 噪声。
- [ ] 配置解析测试覆盖旧 YAML 和新 `schema_refs` YAML。
- [ ] `rtk cargo test -p ralph-core payload_contract` 通过。
- [ ] `rtk cargo test -p ralph-core preset_validator` 通过。
- [ ] `rtk cargo test -p ralph-cli` 或等效 CLI formatter 测试通过。
- [ ] manifest 中所有 builtin preset 都通过 `hats validate --strict`。
- [ ] `presets/**/*.yml` 全部 YAML parse 通过。
- [ ] 旧正则基线中的每个字段都有迁移决策记录。
- [ ] `docs/guide/payload-contracts.md` 已更新为 `schema_refs`。
- [ ] `presets/COLLECTION.md` 和 `presets/schemas/*.yml` 的旧正则表述已检查并更新。
- [ ] `AGENTS.md` 和 `CLAUDE.md` 已检查；如果修改过，`diff -u AGENTS.md CLAUDE.md` 无输出。
- [ ] `crates/ralph-core/data/*.md` 已检查；所有 `.rs:NN-MM` 源码引用已用 `sed -n` 反向验证。
- [ ] `rtk cargo fmt --check` 通过。
- [ ] `rtk cargo check --workspace --exclude ralph-e2e` 通过。
- [ ] `rtk cargo build --workspace --exclude ralph-e2e` 通过。
- [ ] `rtk ./scripts/run-tests.sh` 通过，或记录 nextest fallback 后的等效 `cargo test` 通过。
- [ ] `rtk cargo test -p ralph-core smoke_runner` 通过。
- [ ] `rtk cargo test -p ralph-core scenarios` 通过。
- [ ] `rtk cargo run -p ralph-e2e -- --mock` 通过。

---

## 10. 工作量估算

| 阶段 | 预估时间 | 说明 |
|---|---:|---|
| Phase 0: 基线与清点 | 1-2 小时 | 搜索调用点、记录旧正则输出、确认 preset manifest |
| Phase 1: Rust 核心改造 | 5-8 小时 | config、payload_contract、formatter、struct literal、定向测试 |
| Phase 2: preset 迁移 | 6-12 小时 | 取决于多 trigger hat 数量；`ce-executor` 最耗时 |
| Phase 3: 文档/skill 更新 | 2-4 小时 | guide、preset 文档、AGENTS/CLAUDE、data 反向验证 |
| Phase 4: 全量验证与修正 | 2-5 小时 | workspace test、smoke、BDD、e2e mock、preset 全量 validate |
| **总计** | **2-3.5 天** | 包含 review 修正和验证时间 |

---

## 11. 实施者注意事项

- 这次改动的核心风险不是 Rust 代码复杂，而是 preset 迁移漏字段或错 topic。
- 不要把旧正则逻辑留作 fallback；这会继续让 instructions 文案成为隐式契约。
- 不要为了让 strict validate 通过而滥用 `ignore_payload_fields`。
- 不要只跑几个代表性 preset；必须从 manifest 自动枚举。
- 不要提交临时 baseline、一次性输出、cargo 生成物或日志。
- 所有面向人类的文档和总结按仓库要求使用中文。
