# implementation-review preset — author notes

> 2026-07-24-003 plan / ralph-preset-author. Six-hat isolated wave preset
> (`builtin:implementation-review`). Read these notes next to the YAML
> when reviewing; the YAML is the contract, the notes are the AAF + payload
> audit + hard-question ledger.

## Preset Intent Confirmation

- **目标:** 新增公开 builtin preset `implementation-review`。操作者提供已
  实现完成的开发计划,preset 冻结该计划对应的 Git 审查范围
  (`first_implementation_commit_sha=C` + `resolved_baseline_sha=C^` +
  binary `review.diff.patch`),以一次 wave 并行执行六个相互独立的只读
  维度 review,汇总 P0–P3 findings,六维综合成功后生成一份可审计、可继续
  执行的 fix plan;阻塞路径生成可审计 block artifact。
- **操作者与启动路径:** operator 通过 `ralph run --plan <plan.md> -H
  builtin:implementation-review` 启动。Runner 把 `review.start` 注入
  scope-preparer。启动路径不依赖 worktree(默认 in-workspace)、不依赖
  supervisor execution model、不创建 branch / worktree slot。
- **输入与事实源:**
  - 操作者:`--plan <plan.md>` 提供的原开发计划(repo-relative path)。
  - Git 证据:`first_implementation_commit_sha` / `resolved_baseline_sha` /
    `review_head_sha` / commit list / changed files / patch digest。
  - 维度产物:六个 worker 各写一份 `.ralph/review/<plan>/dimensions/<dim>.md`。
  - 不读取 `.ralph/events.jsonl` / `.ralph/loops.json` /
    `.ralph/supervisor.db` 作为业务 artifact 接口(R0/visibility rule)。
- **成功条件:** 生成 `LOOP_COMPLETE{result: clean | residual_only |
  fixes_required, artifact_path: .ralph/review/<plan>/fix-plan.md}`。
  `fixes_required` 是合法成功结果(KTD18:发现 P0–P2 不等于 preset 失败)。
- **阻塞条件:** 生成 `LOOP_COMPLETE{result: blocked, artifact_path:
  .ralph/review/<plan>/{scope-blocked|review-blocked|wave-blocked}.md}`。
  阻塞来源:scope 多候选/根 commit/merge 父歧义/Git 对象不可读/相关
  dirty/review 期间 tracked 漂移/wave fan-in 失败/synth 完整性失败。
- **允许的修改范围:**
  - 业务 artifact:`.ralph/review/<plan>/` 下所有文件(scope manifest /
    patch / scope-analysis / review-context / scope-blocked /
    dimensions/*.md / synthesized-review.md / fix-plan.md /
    review-blocked.md / wave-blocked.md / git-state-* evidence)。
  - 审计:`.ralph/agent/decisions.md` 追加行(scope-preparer 唯一允许的
    audit file)。
  - 其它路径:严格只读。tracked source / 计划文件 / 其它 worktree
    metadata 一律不得修改。
- **必须独立执行的评审:** 六维 review worker 之间互不读取彼此结论
  (KTD6 / R7)。每个 worker 只从 trigger payload + scope-manifest.json
  + review.diff.patch + tracked code 读输入;`review-synthesizer` 是唯一
  同时看到六份维度产物的 hat。
- **重要 artifact、生产方与消费者:**
  - `.ralph/review/<plan>/scope-manifest.json`(scope-preparer →
    dispatcher / synthesizer / finalizer,生命周期:保留至 loop
    completion 之后)
  - `.ralph/review/<plan>/review.diff.patch`(scope-preparer → six
    workers,保留至 loop completion)
  - `.ralph/review/<plan>/scope-analysis.md`(scope-preparer → 审计 +
    阻塞时由 reviewer 查,保留)
  - `.ralph/review/<plan>/review-context.md`(scope-preparer → six
    workers,保留)
  - `.ralph/review/<plan>/scope-blocked.md`(scope-preparer → finalizer
    触发 `LOOP_COMPLETE{result: blocked}`,保留)
  - `.ralph/review/<plan>/dispatch-batch/payloads.jsonl`(review-dispatcher
    → 自身 OPAC verify/emit 输入,loop 完成后清理或归档到 dispatch
    artifact 目录)
  - `.ralph/review/<plan>/dimensions/<dim>.md`(六 worker →
    review-synthesizer,保留)
  - `.ralph/review/<plan>/git-state-review-worker-<dim>-{start,end}.txt`
    (六 worker → audit,保留)
  - `.ralph/review/<plan>/synthesized-review.md`(review-synthesizer
    → fix-planner,保留)
  - `.ralph/review/<plan>/review-blocked.md`(review-synthesizer →
    finalizer,保留)
  - `.ralph/review/<plan>/fix-plan.md`(fix-planner → finalizer,
    保留;loop completion 后 operator 可读取并喂给后续 fix executor)
  - `.ralph/review/<plan>/wave-blocked.md`(finalizer 自写 → LOOP_COMPLETE
    引用,保留)
- **execution_model:** wave
  **why:** 六维独立 review 同 topic 批并行 fan-out;不开 supervisor
  (KTD2);runtime 默认 wave 热路径已足够(KTD9);不开 worktree slot。
- **非目标:**
  - 不执行 fix plan(交给 operator 后续 preset)。
  - 不让用户在 baseline 歧义时继续同一 loop(deferred)。
  - 不启用 supervisor execution model、不创建 worktree、不做
    supervisor merge sink。
  - 不新增安全 / 性能第七、第八维。
  - 不审查未提交实现。
  - reviewer 不读取 internal ledger(KTD17/R0)。
- **Author 推导与假设:**
  - 默认 wave 热路径在 `event_loop.supervisor.enabled: false` 时仍
    会 lazy 创建 SupervisorBridge 并打开默认 SupervisorStore;本 preset
    把 store 当 runtime I/O 使用,不暴露为业务 artifact 接口。
  - testing 维度只读 patch 中的测试 diff、覆盖边界、断言强度、缺失
    场景、已有可见测试证据;所有 reviewer 都不得运行测试、构建、
    静态检查(R10)。
  - KTD16:`scope.ready` 的 state projection 是跨 wave 数据面索引;
    synthesizer 从 trigger payload + projection 推导六个 canonical
    artifact paths,再 `Read` 文件验证,不是从 `review.wave.complete`
    payload 索取。
- **用户确认:** 已确认(三个关键问题:execution_model=wave,
  finalizer LOOP_COMPLETE 仅 result+artifact_path,
  阻塞 handoff=scope.blocked+review.blocked+finalizer 自写 wave-blocked.md)。

---

## Hard questions — single-chain-first (强制 5 问)

1. **本 preset 的 unit 拆分能否由 executor 内部 subagent 完成?**
   ✗ + 本 preset 无 executor;unit 拆分由 wave 拓扑(六 dim 共享一个
   review-worker hat, payload dimension 区分 activation)而非
   subagent 完成(KTD8)。合理:review 不产生 code changes,executor
   subagent 拆分不适用。
2. **任何业务 topic 是否超过一个消费者?**
   ✗ + 每个业务 topic 唯一消费者:
   - `scope.ready` → review-dispatcher
   - `scope.blocked` → finalizer
   - `review.unit.ready` → review-worker (six activations)
   - `review.unit.done` → runtime fan-in → `review.wave.complete` → review-synthesizer
   - `review.wave.failed` → finalizer (via `event_filter` routing)
   - `review.synthesized` → fix-planner
   - `review.blocked` → finalizer
   - `fix.plan.ready` → finalizer
   `LOOP_COMPLETE` 不被任何 hat 消费,终态。
3. **fallback 是否可能路由到 success?**
   ✗ + 无 fallback / rescue hat;`finalizer` 只在收到合法 success /
   blocked handoff 后才发 `LOOP_COMPLETE`。没有任何 hat 能把 blocked
   路径降级成 `clean` / `residual_only` / `fixes_required`。
4. **是否有 hat 把 tasks / progress / recovery 当业务事实?**
   ✗ + `tasks.enabled: false`(preset 顶层);六个 hat 都不消费
   `.ralph/tasks.jsonl` 或 `.ralph/agent/progress*`。review-worker
   与 review-synthesizer 通过 `ralph tools task list` 取得 live id
   时也只用于 OPAC 验证,不当作业务事实。
5. **是否有 rescue hat 能改变业务链路?**
   ✗ + 无 rescue hat。finalizer 是 preset 的唯一收尾出口。

**结论:** 五问全 ✓(✓/✗ 按问题号,✗ 即"答案为否,即通过");preset
默认走 single-chain 的理由不再适用(KTD2 已选 wave),其它四问均
不阻塞 wave 模型。

---

## Hard questions — wave fan-out (强制 7 问)

1. **唯一 dispatcher:** ✓ — `review-dispatcher` 是 preset 中**唯一**
   调用 `ralph wave emit` / `ralph wave verify` 的 hat。其它 5 hats
   一律只调 `ralph emit`(单事件)。
2. **worker 禁 `wave emit`:** ✓ — `review-worker` 的 `instructions:`
   显式声明 "Do NOT call `ralph wave emit` / `ralph wave verify`
   (U23 wave ACL — only the dispatcher hat may)"。
3. **`wave verify` → emit:** ✓ — dispatcher Step 4 强制
   `ralph wave verify --payloads-stdin` 在前,verify 通过后再用
   **同一未修改 bytes** 做 `ralph wave emit`;verify/emit 中间不得
   改 payload(否则 fingerprint mismatch 拒)。引用
   `ralph-tools-wave` red box。
4. **Confirm 用 main ledger:** ✓ — `ralph wave emit` 写入主 ledger
   (`current-events` 链);Confirm 必须用 `ralph events --events-source
   main` 验 wave 已登记,**不**用 hat-channel。dispatcher instructions
   没让 agent 自己 Confirm(agent 立即停,Confirm 留给 runtime);不违反。
5. **禁 agent 发协调 topic:** ✓ — grep `presets/en/implementation-review.yml`
   的 `publishes`:`scope-preparer` / `review-dispatcher` / `review-worker` /
   `review-synthesizer` / `fix-planner` / `finalizer` 均不含
   `wave.*` / `exec.wave.*` 等协调 topic。`review.wave.complete` /
   `review.wave.failed` 只在 `event_policy.schemas` 中声明,作为
   runtime-injected 输入 reference,不进任何 hat 的 `publishes`。
6. **batch 失败可定位:** ✓ — dispatcher Step 4 引用
   `ralph-tools-wave`「policy-check 反馈」段;当 `ralph wave verify`
   拒收时,agent 按 JSON 中 `validation_errors[].payload_index` /
   `field` 定位失败 item 并精准修复,**不**整批重发。
7. **emitter cite skill:** ✓ — dispatcher Step 4 显式引用
   `ralph-tools-wave` 与 `ralph-tools-emit` §5 policy-check feedback;
   reviewer (六 worker) 的 Step 5 显式引用 `ralph-tools-emit` §5。

**结论:** 七问全 ✓。

---

## Hard questions — supervisor orchestration (强制 6 问)

> **N/A 规则:** `execution_model = wave`(非 supervisor / supervisor+wave),
> 本段不适用。按 author-checklist「N/A 规则」标 N/A + ≤30 字理由,不得留空。

| 问题 | 标注 | 理由 |
|---|---|---|
| 1 (`supervisor.enabled` + isolated) | **N/A** | execution_model=wave,无 supervisor;`event_loop.supervisor.enabled` 不声明 |
| 2 (禁读 / 写 `supervisor.db` 作业务接口) | **N/A** | 同上 |
| 3 (禁发 coordination topic) | **N/A** | 同上 |
| 4 (unit 状态经 task API / 业务 artifact) | **N/A** | 同上;本 preset `tasks.enabled: false` |
| 5 (timeout / partial 有业务可见出口) | **N/A** | `review-worker.timeout: 900`(每 slot 15min);timeout / partial 经 runtime `review.wave.failed` 注入,finalizer → `LOOP_COMPLETE{result: blocked}` |
| 6 (与 Intent 一致) | **N/A** | Intent.execution_model=wave,YAML 无 `event_loop.supervisor.enabled: true`,一致 |
| 7 (wave consumer `concurrency > 1`) | **N/A** (本问题实际属于 wave fan-out,但与 supervisor 7 同形) | `review-worker.concurrency: 6` ≥ 6 (六槽) ✓,wave detector 不会 `SequentialTarget` 拒 |

> **silent-success 契约 (2026-07-26):** `review-dispatcher` /
> `review-worker` / `fix-planner` / `finalizer` **不得**配置成功态
> `default_publishes`。空 emit 必须走 missing-event hard gate。
> `scope-preparer` / `review-synthesizer` 保留 fail-closed default
> (`scope.blocked` / `review.blocked`)。`scope-preparer` Step 0 必须
> 清理同 plan 的 `dimensions/`、`dispatch-batch/` 与综合/阻塞残留,
> 避免跨 loop 误读。

---

## Hard questions — Artifact-First Handoff (强制 5 问)

1. **每条写入型 hat 是否声明了当前 `.ralph/` 下的 artifact 路径集合,
   且拓扑层没有把这些路径描述为「preset 创建」?**
   ✓ + scope-preparer 声明 scope-manifest.json / review.diff.patch /
   scope-analysis.md / review-context.md / scope-blocked.md;
   review-worker 声明 dimensions/<dim>.md + git-state evidence;
   review-synthesizer 声明 synthesized-review.md / review-blocked.md;
   fix-planner 声明 fix-plan.md;finalizer 自写 wave-blocked.md(仅
   review.wave.failed 路径)。YAML 顶部注释明确"6-hat topology,
   该 hat 写入...",**不**写"preset creates X.md"。
2. **每条 consumer hat 的 instructions 是否要求从路径读完整内容,不是
   依赖 prompt 中的长文本?**
   ✓ + review-worker Step 1/2: `Read <patch_path>` /
   `Read <scope_manifest_path>` / `Read <review_context_path>`;
   review-synthesizer Step 2/3: 同样 `Read` 六个
   dimensions/<dim>.md;fix-planner Step 1: `Read` synthesized-review.md;
   finalizer Step 1: 按 trigger 表读对应 block / fix-plan artifact。
   任何 hat 不得只凭 payload 字符串摘要放行。
3. **每个被传递的完整结果 / 长内容 / 跨 hat 摘要是否都已先落盘,
   event / message 是否只保留短状态、摘要、路径、必要身份与路由字段?**
   ✓ + scope.ready 只携带 plan_name / first_implementation_commit_sha /
   resolved_baseline_sha / review_head_sha / scope_digest / patch_path
   / patch_digest / scope_manifest_path / dirty_verdict — 无完整
   findings / 无长内容 / 无 review-context 全文。
   review.unit.done 只携带 findings_count + findings_file 路径 +
   scope_digest / patch_digest / review_head_sha — 完整 findings
   在 dimensions/<dim>.md。
   review.synthesized 只携带 counts + synthesized_review_file 路径。
   fix.plan.ready 只携带 counts + fix_plan_file 路径。
   LOOP_COMPLETE 只携带 result + artifact_path + scope_digest。
4. **是否有任何 hat 把 `.ralph/events.jsonl` / `.ralph/loops.json` /
   `.ralph/supervisor.db` 当作业务 artifact 接口?**
   ✗ + grep `instructions:` 段与 payload audit 表,**无任何 hat** 要求
   读 / 写这些路径作为业务接口。wave dispatcher 的 Confirm 走
   `ralph events --events-source main`(诊断 / audit,不作为业务
   artifact);supervisor.db 根本没出现。
5. **每条声明「不落盘」的信息是否都标注了简短理由,并按恢复价值、
   审计价值和下游依赖解释,而非只按字符数判断?**
   ✓ + 终态 `LOOP_COMPLETE.result` 是单 token verdict(短状态),
   不落盘例外 — 理由:短暂(activation 内有效)、短小(枚举值)、无
   下游历史依赖(reviewer 业务事实已落在 fix-plan.md / block
   artifact)。`scope_digest` / `patch_digest` 是 digests 不落盘 —
   理由:非业务内容,仅作为 identity 摘要,完整内容已落盘。
   `count` 字段(`p0_count` / `findings_count` 等)不落盘例外 —
   理由:可由 consumer 立即重算,无下游历史依赖(完整 findings 已
   落盘 synthesized-review.md)。

**结论:** 五问全 ✓(问题 4 必须为 ✗,已确认)。

---

## Payload Audit 通用原则

- **Agent 身份唯一性:** `task_id` / `task_key` / `step` 不适用于本 preset
  (`tasks.enabled: false`);但 scope_digest / patch_digest / review_head_sha
  是 preset 的"live identity",均由 scope-preparer 一次性冻结并向下游
  传递;**禁止**任何下游 hat 重算或手写。`commit_count` / `changed_lines`
  等不存在(本 preset 无 executor 提交动作)。
- **Git 状态门禁:** 每个写 artifact 的 hat 在 emit 前都做"Stage A:
  写文件 → 重读 → 重算 digest → policy-check"两阶段 precheck;每个
  只读 hat 都做"Entry Precheck (HEAD / tree / digest / porcelain) +
  Exit Precheck"。
- **Handoff failure emit 规则:** 只读 hat(dispatcher / 六 workers /
  synthesizer / fix-planner / finalizer)任何 precheck 失败都按本 hat
  唯一允许的事件 + `handoff_precheck_failed: true` 发,**不**silent-stop
  (KTD14 / R11)。
- **Emit schema metadata:** 每个 agent-authored emit topic 在
  `event_policy.schemas.<topic>` 都有完整 `field_docs`(`meaning` /
  `source` / `fill_rule`)— 任何 path_field 的 `meaning` 明确写
  「该路径是 artifact 落盘点,值为相对 `.ralph/` 的路径」。
- **Payload examples:** 暂未填具体业务 examples(避免 agent 复制为
  真实路径);`fill_rule` / `examples` 占位用结构 placeholder(如
  `.ralph/<plan>/<file>.md`)。
- **Lifecycle owner:** 见 Intent「重要 artifact、生产方与消费者」段;
  每份重要 artifact 都有 production / consumption / 保留责任。

---

## Hat: scope-preparer

### AAF 五问

- **Q1 使命:** 唯一确定第一个实现 commit `C`,冻结 `C^..HEAD` patch 与
  scope manifest,生成可审计的 `.ralph/review/<plan>/` 业务 artifact;
  成功发 `scope.ready`,任何失败发 `scope.blocked`。一次 activation
  只有一个出口。
- **Q2 输入(Observe 命令 + 期望字段):**
  - `ralph inspect loop --format json` — `plan_name`, `loop_id`。
  - 当前 trigger payload(`review.start`)— `plan_path`, `plan_name`(若有)。
  - `Read` / `cat $plan_path` — 完整 plan bytes。
  - Git 命令(只读):`git log --reverse --format=%H -- <plan_path>`,
    `git log -n 200 --format=%H%s`, `git rev-parse C^`, `git rev-parse HEAD`,
    `git diff --name-only --diff-filter=ACMRT C^..HEAD`,
    `git diff --binary --no-color C^..HEAD`,
    `git log --format=%H C^..HEAD`,
    `git cat-file -t <sha>` / `git cat-file -p <sha>`,
    `git status --porcelain --untracked-files=all -z`,
    `git rev-parse --show-toplevel`,
    `sha256sum <patch>` / `sha256sum <manifest>`。
- **Q3 执行(OPAC 命令序列):** Observe(plan + git + status)→ Precheck(
  写 .ralph/ 下 artifacts → 重读 → 重算 digests → `ralph emit
  --policy-check` 不带 `--policy-check` 之前的同源 policy 检验)→
  Apply(`ralph emit <scope.ready|scope.blocked> --policy-check` →
  drop `--policy-check` → 真 emit)→ Confirm(`ralph events
  --events-source hat-channel`)。
- **Q4 输出(topic + payload 合同):** 见下方 Payload Contract。
- **Q5 交接(emit 字段 → 下游 Observe 路径):**
  - 成功路径:`scope.ready` payload 字段(plan_name / first_implementation_commit_sha
    / resolved_baseline_sha / review_head_sha / scope_digest / patch_path /
    patch_digest / scope_manifest_path / dirty_verdict)→ review-dispatcher
    通过 trigger payload + `## TRIGGER CONTEXT`(preset 声明
    `summary_fields` + `routing_hints`)读 → 同时通过 `Read
    scope_manifest_path` 取得 canonical scope。
  - 阻塞路径:`scope.blocked` payload → finalizer 通过 trigger payload
    读 `block_artifact_path` → `Read scope-blocked.md` 验证
    reason code。

### Payload Contract — scope-preparer

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| `scope.ready` | `plan_name` | string | 当前 trigger (`review.start`) `plan_path` basename | `## TRIGGER CONTEXT` + `ralph inspect loop` | 不涉及 | dispatcher 决定 wave 拓扑 | `field_docs.plan_name.meaning/source/fill_rule` 已声明 | 不落盘·短暂+短小,无下游历史依赖 |
| `scope.ready` | `first_implementation_commit_sha` | string(40 hex) | scope-preparer Step 2 candidate 决议 | 本 hat work 输出(自写 artifact 后回填) | live `git rev-parse` 验证 | dispatcher / worker / synthesizer / finalizer 全部携带 | `field_docs.first_implementation_commit_sha.fill_rule` 禁手写 | 必填 · `C` 落到 `scope-manifest.json`;event 只携带 SHA |
| `scope.ready` | `resolved_baseline_sha` | string(40 hex) | `git rev-parse C^` | 本 hat work 输出 | live `git rev-parse C^` 验证 | 同上 | `field_docs.resolved_baseline_sha.fill_rule` 禁手写 | 必填 · `C^` 落到 `scope-manifest.json`;event 只携带 SHA |
| `scope.ready` | `review_head_sha` | string(40 hex) | `git rev-parse HEAD` at freeze | 本 hat work 输出 | live `git rev-parse HEAD` 验证 | 同上 | `field_docs.review_head_sha.fill_rule` 禁手写 | 必填 · 落到 `scope-manifest.json`;event 只携带 SHA |
| `scope.ready` | `scope_digest` | string(16-64 hex) | scope-preparer Step 5a recompute over canonical manifest | 本 hat work 输出 | 与 manifest 内 `scope_digest` 字段再交叉验证 | dispatcher / worker / synthesizer / finalizer identity check | `field_docs.scope_digest.fill_rule` 禁手写 | 必填 · `scope-manifest.json` 自包含 |
| `scope.ready` | `patch_path` | string | scope-preparer Step 3 Write tool | 本 hat work 输出 | 文件存在可读 | worker / synthesizer / finalizer 读 patch | `field_docs.patch_path.fill_rule` 禁构路径 | 必填 · 路径 `.ralph/review/<plan>/review.diff.patch` |
| `scope.ready` | `patch_digest` | string(64 hex) | scope-preparer Step 3 `sha256sum` | 本 hat work 输出 | 写后重算对齐 | worker / synthesizer / finalizer identity check | `field_docs.patch_digest.fill_rule` 禁手写 | 必填 · digest 落到 `scope-manifest.json` 与 event |
| `scope.ready` | `scope_manifest_path` | string | scope-preparer Step 5a Write tool | 本 hat work 输出 | 文件存在可读 | dispatcher / synthesizer / finalizer 读完整 manifest | `field_docs.scope_manifest_path.fill_rule` 禁构路径 | 必填 · 路径 `.ralph/review/<plan>/scope-manifest.json` |
| `scope.ready` | `dirty_verdict` | enum | scope-preparer Step 4 决议 | 本 hat work 输出 | 与 manifest `dirty_verdict` 字段对齐 | dispatcher 路由(`scope_clean` / `scope_dirty_blocked` hints) | `field_docs.dirty_verdict.fill_rule` 枚举 `clean | blocked` | 不落盘·短暂 verdict,审计已在 `scope-analysis.md` |
| `scope.blocked` | `reason` | enum string | scope-preparer Step 5b/Step 6 决议 | 本 hat work 输出 | 与 block artifact `reason` 字段对齐 | finalizer 路由,reviewer 查阻塞根因 | `field_docs.reason.fill_rule` 枚举 11 个 canonical code | 必填 · 完整 evidence 落到 `scope-blocked.md`;event 只带 short code |
| `scope.blocked` | `block_artifact_path` | string | scope-preparer Step 5b Write tool | 本 hat work 输出 | 文件存在可读 | finalizer 读完整 block evidence | `field_docs.block_artifact_path.fill_rule` 禁构路径 | 必填 · 路径 `.ralph/review/<plan>/scope-blocked.md` |
| `scope.blocked` | `scope_digest` | string(16-64 hex) | 同上 | 本 hat work 输出 | 与 manifest / block artifact 对齐 | identity check | `field_docs.scope_digest.fill_rule` 禁手写 | 必填 |

---

## Hat: review-dispatcher

### AAF 五问

- **Q1 使命:** 唯一 fan-out:从冻结 scope 派发一次六 payload wave,共享
  同一 `wave_id` + idempotency key。本 hat 唯一允许调 `ralph wave emit`
  / `ralph wave verify`;dispatcher **不**做 review。一次 activation
  = 一次 wave。
- **Q2 输入(Observe 命令 + 期望字段):**
  - 当前 trigger payload(`scope.ready`)— 全字段。
  - `Read scope-manifest.json` / `Read review.diff.patch` / `Read
    review-context.md`(Step 1 re-verify)。
  - `git rev-parse HEAD`、`git status --porcelain --untracked-files=all`、
    `sha256sum <patch>`(Step 1 re-verify)。
- **Q3 执行(OPAC 命令序列):** Observe(scope.ready + scope
  artifacts + git)→ Precheck(写 `dispatch-batch/payloads.jsonl` →
  `wc -l == 6` → `sha256sum` 取得 fingerprint → `ralph wave verify
  --payloads-stdin`)→ Apply(`ralph wave emit --payloads-stdin
  --idempotency-key <key>`,payload 与 verify 同源)→ Confirm(
  `ralph events --events-source main` 验 wave_id 已登记;`deduplicated`
  判定)。
- **Q4 输出(topic + payload 合同):** 一次 `review.unit.ready` wave
  emit(`count=6`,`wave_id` 共享);不在 `publishes` 内单独列 `wave.*`。
- **Q5 交接(emit 字段 → 下游 Observe 路径):** 6 个 `review.unit.ready`
  payload 全部携带同一 `scope_digest` / `patch_digest` /
  `review_head_sha` / `first_implementation_commit_sha` /
  `resolved_baseline_sha` / `scope_manifest_path`;`slot_index` /
  `dimension` 唯一;review-worker 通过 trigger payload + `## TRIGGER
  CONTEXT` 读取。

### Payload Contract — review-dispatcher

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| `review.unit.ready` (×6) | `scope_digest` | string(16-64 hex) | dispatcher 从 `scope.ready.scope_digest` 复制 | 本 hat 可见 trigger payload | 与 manifest 对齐 | worker identity check | `field_docs.scope_digest.fill_rule` 禁手写 | 不落盘·本 hat 已持有同一值 |
| `review.unit.ready` (×6) | `patch_path` | string | dispatcher 从 `scope.ready.patch_path` 复制 | 本 hat 可见 | 文件存在可读 | worker `Read` | `field_docs.patch_path.fill_rule` 禁构路径 | 必填 · patch 已由 scope-preparer 落盘 |
| `review.unit.ready` (×6) | `patch_digest` | string(64 hex) | dispatcher 从 `scope.ready.patch_digest` 复制 | 本 hat 可见 | 与 patch 文件 digest 对齐 | worker identity check | `field_docs.patch_digest.fill_rule` 禁手写 | 不落盘·本 hat 持有 |
| `review.unit.ready` (×6) | `scope_manifest_path` | string | dispatcher 从 `scope.ready.scope_manifest_path` 复制 | 本 hat 可见 | 文件存在可读 | worker `Read` | `field_docs.scope_manifest_path.fill_rule` 禁构路径 | 必填 · manifest 已落盘 |
| `review.unit.ready` (×6) | `review_head_sha` / `first_implementation_commit_sha` / `resolved_baseline_sha` | string(40 hex) | dispatcher 从 `scope.ready` 三个字段复制 | 本 hat 可见 | git re-verify | worker identity check | 三个字段 `fill_rule` 禁手写 | 不落盘·本 hat 持有 |
| `review.unit.ready` (×6) | `slot_index` | int 0..5 | dispatcher Step 2 枚举 | 本 hat 枚举产出 | 唯一性 cross-check | worker 路由 / audit | `field_docs.slot_index.fill_rule` 整数 0..5 唯一 | 不落盘·短暂路由 |
| `review.unit.ready` (×6) | `dimension` | enum | dispatcher Step 2 枚举 6 canonical | 本 hat 枚举产出 | 与 `slot_index` 一一对应 | worker 决定维度策略 | `field_docs.dimension.fill_rule` 枚举 6 canonical | 不落盘·短暂路由 |
| `review.unit.ready` (×6) | `idempotency_payload_version` | int ≥1 | dispatcher Step 2 | 本 hat 产出 | dedup key 派生 | runtime dedup 判定 | `field_docs.idempotency_payload_version.fill_rule` 整数 ≥1 | 不落盘·短暂 schema 版本 |
| (dispatcher 自产物,非 emit 字段) | `dispatch-batch/payloads.jsonl` | NDJSON 6 行 | dispatcher Step 2 Write tool | 本 hat 可见 | `wc -l == 6` + `sha256sum` 一致 | verify / emit 同源输入 | 不适用 | 必填 · 路径 `.ralph/review/<plan>/dispatch-batch/payloads.jsonl`;loop 完成后归档或清理 |

---

## Hat: review-worker (六 dim 共用同一 hat id;`concurrency: 6`)

### AAF 五问

- **Q1 使命:** 审查一个维度,写 `.ralph/review/<plan>/dimensions/<dim>.md`,
  发 `review.unit.done`(`findings_count` + `findings_file` +
  `handoff_precheck_failed`)。一次 activation = 一个 dim。一次
  `review.unit.done`。**不**运行测试、构建、静态检查(R10)。
- **Q2 输入(Observe 命令 + 期望字段):**
  - 当前 trigger payload(`review.unit.ready`)— `dimension`,
    `slot_index`, `scope_digest`, `patch_path`, `patch_digest`,
    `scope_manifest_path`, `review_head_sha`,
    `first_implementation_commit_sha`, `resolved_baseline_sha`。
  - `Read scope-manifest.json` / `Read review.diff.patch` /
    `Read review-context.md`。
  - Git 命令(只读):`git rev-parse HEAD`、`git status --porcelain
    --untracked-files=all`(排除 `.ralph/`,排除已知 build/cache
    `.gitignore` paths)、`sha256sum <patch>`。
- **Q3 执行(OPAC 命令序列):** Observe(trigger + scope artifacts +
  tracked code)→ Precheck(Entry Precheck 写 git-state-* evidence →
  `ralph emit --policy-check`)→ Apply(写 dimension artifact → 重读
  → 重算 `findings_count` → `ralph emit review.unit.done` 去掉
  `--policy-check` 真写盘)→ Confirm(`ralph events --events-source
  hat-channel`)。
- **Q4 输出(topic + payload 合同):** 见 Payload Contract。
- **Q5 交接(emit 字段 → 下游 Observe 路径):** `review.unit.done`
  payload 中 `scope_digest` / `patch_digest` / `review_head_sha`
  携带给 runtime;`dimension` / `slot_index` / `findings_count`
  / `findings_file` 给 synthesizer;synthesizer 通过
  `ralph events --events-source main` 收到六个 done,然后
  `Read dimensions/<dim>.md` 验证(KTD16 跨 wave 数据面索引)。

### Payload Contract — review-worker (六 dim)

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| `review.unit.done` | `dimension` | enum | 当前 trigger `dimension` 字段 | `## TRIGGER CONTEXT` | 与 `slot_index` 一一对应 | synthesizer fan-in | `field_docs.dimension.fill_rule` literal copy | 不落盘·短暂路由 |
| `review.unit.done` | `slot_index` | int 0..5 | 当前 trigger `slot_index` | `## TRIGGER CONTEXT` | 同上 | synthesizer fan-in | `field_docs.slot_index.fill_rule` literal copy | 不落盘·短暂路由 |
| `review.unit.done` | `findings_count` | int ≥0 | 本 hat 自写 dimension artifact frontmatter | 本 hat work 输出 | artifact 实际 `findings:` 数组长度对齐 | synthesizer 综合 + finalizer 终态 | `field_docs.findings_count.fill_rule` 禁 fabricate | 不落盘·count 单独存在;完整 findings 落盘 |
| `review.unit.done` | `findings_file` | string | 本 hat Step 3 Write tool | 本 hat work 输出 | 文件存在可读 | synthesizer `Read` 取得完整 findings | `field_docs.findings_file.fill_rule` 禁构路径 | 必填 · 路径 `.ralph/review/<plan>/dimensions/<dimension>.md` |
| `review.unit.done` | `scope_digest` / `patch_digest` / `review_head_sha` / `plan_name` / `plan_path` | string | 当前 trigger payload 复制 | `## TRIGGER CONTEXT` | 与 manifest / patch 对齐 | synthesizer identity check / propagation | 五字段 `fill_rule` 禁手写 | 不落盘·本 hat 已持有 |
| `review.unit.done` | `handoff_precheck_failed` | bool | 本 hat Step 1 / Step 4 precheck 决议 | 本 hat 内部 | 与 git-state evidence 文件一致 | synthesizer fail-close 判定 | `field_docs.handoff_precheck_failed.fill_rule` 默认 false | 不落盘·短暂 verdict,evidence 落到 `git-state-review-worker-<dim>-*.txt` |
| (worker 自产物) | `dimensions/<dim>.md` | markdown | 本 hat Step 3 Write tool | 本 hat 可见 | YAML frontmatter 解析 + 长度对齐 | synthesizer `Read` | 不适用(是 artifact) | 必填 · 完整 findings 必须落盘 |
| (worker 自产物) | `git-state-review-worker-<dim>-{start,end}.txt` | text | 本 hat Step 1 / Step 4 写盘 | 本 hat 可见 | 不适用 | audit;不进 payload | 不适用 | 必填 · 路径 `.ralph/review/<plan>/git-state-review-worker-<dim>-{start,end}.txt` |

---

## Hat: review-synthesizer

### AAF 五问

- **Q1 使命:** 消费 runtime `review.wave.complete`;重验 HEAD/tree/six
  dims/scope digests;dedupe 与 P0–P3 排名;写 `synthesized-review.md`;
  发 `review.synthesized` 或 `review.blocked`。
- **Q2 输入(Observe 命令 + 期望字段):**
  - 当前 trigger(`review.wave.complete`)— `wave_id`,
    `completed_dimensions[]`, `aggregate_timeout`(由
    `build_wave_complete_payload` 构造)。
  - `ralph inspect loop` — `plan_name`, 投影 `scope.ready` 字段。
  - `Read scope-manifest.json` / `Read review.diff.patch` /
    `Read dimensions/<dim>.md`(每 dim 一次)。
  - Git 命令(只读):`git rev-parse HEAD`、`git status --porcelain`
    排除 `.ralph/`、`sha256sum <patch>`。
- **Q3 执行(OPAC 命令序列):** Observe(wave.complete + scope 投影 +
  六个 dimensions 全文)→ Precheck(写 `synthesized-review.md` →
  重读 → 重算 counts → `ralph emit --policy-check`)→ Apply(
  `ralph emit review.synthesized` 去掉 `--policy-check`)→ Confirm(
  `ralph events --events-source hat-channel`)。阻塞路径类似但发
  `review.blocked`,先写 `review-blocked.md`。
- **Q4 输出(topic + payload 合同):** `review.synthesized` payload
  携带 counts + `synthesized_review_file` 路径 + 七个 identity 字段;
  或 `review.blocked` payload 携带 reason + `block_artifact_path`。
- **Q5 交接(emit 字段 → 下游 Observe 路径):** `review.synthesized`
  payload 由 fix-planner 通过 trigger + `## TRIGGER CONTEXT` 读 →
  fix-planner `Read synthesized_review_file` 取得完整内容;
  `review.blocked` 由 finalizer 通过 trigger + `Read block_artifact_path`
  验证。

### Payload Contract — review-synthesizer

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| `review.synthesized` | `synthesized_review_file` | string | 本 hat Step 4 Write tool | 本 hat 可见 | 文件存在可读 | fix-planner `Read` 完整 | `field_docs.synthesized_review_file.fill_rule` 禁构路径 | 必填 · 路径 `.ralph/review/<plan>/synthesized-review.md` |
| `review.synthesized` | `dimensions_covered` | array<string> | 本 hat Step 2 验证六唯一 | `## TRIGGER CONTEXT` (`completed_dimensions` 投影) | length==6 且 unique | fix-planner / finalizer 路由 | `field_docs.dimensions_covered.fill_rule` literal copy from completed_dimensions | 不落盘·短暂路由 |
| `review.synthesized` | `findings_count` / `p0_count` / `p1_count` / `p2_count` / `p3_count` | int ≥0 | 本 hat Step 3 dedupe + rank 后写 artifact,再回填 | 本 hat work 输出 | 与 artifact frontmatter 对齐 | fix-planner 决定 `result`;finalizer 终态 | 五个字段 `fill_rule` 禁手写 | 不落盘·完整 findings 已落盘 synthesized-review.md |
| `review.synthesized` | `handoff_precheck_failed_count` | int ≥0 | 本 hat Step 2 累积 | 本 hat work 输出 | 与六 dimensions 对齐 | fix-planner 路由(`handoff_precheck_failed` hint);finalizer fail-close | `field_docs.handoff_precheck_failed_count.fill_rule` 整数 | 不落盘·短暂 verdict,evidence 在 synthesized-review.md frontmatter |
| `review.synthesized` | seven identity fields | string | 从 `scope.ready` 复制(`scope_digest` / `patch_digest` / `review_head_sha` / `first_implementation_commit_sha` / `resolved_baseline_sha` / `plan_name` / `plan_path`) | `## TRIGGER CONTEXT` 投影 | 与 manifest / patch 对齐 | fix-planner / finalizer identity check | 七个字段 `fill_rule` 禁手写 | 不落盘·本 hat 持有 |
| `review.blocked` | `reason` | enum string | 本 hat Step 6 决议 | 本 hat work 输出 | 与 block artifact `reason` 对齐 | finalizer fail-close | `field_docs.reason.fill_rule` 枚举 5 个 canonical code | 必填 · 完整 evidence 落到 `review-blocked.md` |
| `review.blocked` | `block_artifact_path` | string | 本 hat Step 6 Write tool | 本 hat work 输出 | 文件存在可读 | finalizer `Read` 验证 | `field_docs.block_artifact_path.fill_rule` 禁构路径 | 必填 · 路径 `.ralph/review/<plan>/review-blocked.md` |

---

## Hat: fix-planner

### AAF 五问

- **Q1 使命:** 把 P0–P2 转成 actionable Implementation Units(P3 → residual),
  写 `fix-plan.md`,发 `fix.plan.ready`(`result` enum)。空 findings 仍
  生成结构完整、actionable_unit_count=0 的 fix plan。
- **Q2 输入(Observe 命令 + 期望字段):**
  - 当前 trigger(`review.synthesized`)— 七 identity 字段 +
    `synthesized_review_file` + counts。
  - `Read synthesized_review_file`(Step 1)+ `Read scope-manifest.json`。
- **Q3 执行(OPAC 命令序列):** Observe(trigger + synthesized
  artifact + manifest)→ Precheck(写 fix-plan.md → 重读 →
  `ralph emit --policy-check`)→ Apply(`ralph emit fix.plan.ready`)
  → Confirm(`ralph events --events-source hat-channel`)。
- **Q4 输出(topic + payload 合同):** `fix.plan.ready` payload 携带
  seven identity 字段 + `fix_plan_file` + `result` + three counts。
- **Q5 交接(emit 字段 → 下游 Observe 路径):** finalizer 通过
  trigger + `## TRIGGER CONTEXT` 读 `fix_plan_file` → `Read
  fix-plan.md` 验证 counts + `result` → 发 `LOOP_COMPLETE`。

### Payload Contract — fix-planner

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| `fix.plan.ready` | `fix_plan_file` | string | 本 hat Step 3 Write tool | 本 hat 可见 | 文件存在可读 | finalizer `Read` 完整 | `field_docs.fix_plan_file.fill_rule` 禁构路径 | 必填 · 路径 `.ralph/review/<plan>/fix-plan.md` |
| `fix.plan.ready` | `result` | enum | 本 hat Step 3 artifact frontmatter | 本 hat work 输出 | 与 artifact 对齐 | finalizer 路由 4 hint groups(`clean` / `residual_only` / `fixes_required` 等) | `field_docs.result.fill_rule` 枚举 3 个 success code | 不落盘·短暂 verdict,完整 plan 已落盘 |
| `fix.plan.ready` | `implementation_unit_count` / `actionable_unit_count` / `residual_finding_count` | int ≥0 | 本 hat Step 3 artifact frontmatter | 本 hat work 输出 | 与 artifact body 对齐 | finalizer validation(fall-back to `blocked` on mismatch) | 三个字段 `fill_rule` 禁手写 | 不落盘·完整 plan 已落盘 |
| `fix.plan.ready` | `source_review_file` | string | 当前 trigger `synthesized_review_file` 复制 | `## TRIGGER CONTEXT` | 文件存在可读 | finalizer 反向验证 source review | `field_docs.source_review_file.fill_rule` literal copy | 不落盘·路径已存在 |
| `fix.plan.ready` | seven identity fields | string | 从 `review.synthesized` 复制 | `## TRIGGER CONTEXT` | 与 manifest / patch 对齐 | finalizer identity check | 七个字段 `fill_rule` 禁手写 | 不落盘·本 hat 持有 |

---

## Hat: finalizer

### AAF 五问

- **Q1 使命:** 唯一发布 `LOOP_COMPLETE`;在发之前验证 referenced artifact
  (fix-plan.md / scope-blocked.md / review-blocked.md / wave-blocked.md),
  保证 `result` 与 `artifact_path` 一一对应且 digest 一致。
- **Q2 输入(Observe 命令 + 期望字段):**
  - 当前 trigger 任一(`fix.plan.ready` / `scope.blocked` /
    `review.blocked` / `review.wave.failed`)。
  - 按 trigger 表读对应 artifact:
    `Read fix-plan.md` / `Read scope-blocked.md` /
    `Read review-blocked.md` / 自写 `Read wave-blocked.md`。
  - `Read scope-manifest.json`(任何路径都要对齐 scope_digest)。
- **Q3 执行(OPAC 命令序列):** Observe(trigger + manifest + 对应
  artifact)→ Precheck(可选:写 wave-blocked.md → 重读 →
  `ralph emit --policy-check`)→ Apply(`ralph emit LOOP_COMPLETE`)→
  Confirm(`ralph events --events-source main`)。
- **Q4 输出(topic + payload 合同):** `LOOP_COMPLETE` payload:
  `plan_name` / `plan_path` / `result` / `artifact_path` /
  `scope_digest`。
- **Q5 交接(emit 字段 → 下游 Observe 路径):** runtime / operator
  通过 `ralph events --events-source main` 读 `LOOP_COMPLETE`;
  operator 据 `result` + `artifact_path` 决定下一步(fix executor
  或重新启动)。

### Payload Contract — finalizer

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| `LOOP_COMPLETE` | `result` | enum | 本 hat Step 1 validation | 本 hat 决议 | 与 artifact `result` 对齐(fix-plan)或 reason code 对齐(block) | operator 路由;runtime 终态记录 | `field_docs.result.fill_rule` 枚举 4 个 code | 不落盘·短暂 verdict |
| `LOOP_COMPLETE` | `artifact_path` | string | 本 hat Step 1 / Step 2 决议 | 本 hat 决议 | 文件存在可读 | operator / downstream 读 artifact | `field_docs.artifact_path.fill_rule` 禁构路径 | 必填 · artifact 已由上游 hat 落盘;finalizer 自写 wave-blocked.md |
| `LOOP_COMPLETE` | `scope_digest` | string(16-64 hex) | `Read scope-manifest.json` 取 | 本 hat 可见 | 与 manifest 对齐 | operator / downstream identity check | `field_docs.scope_digest.fill_rule` 禁手写 | 不落盘·digest 已落盘 manifest |
| `LOOP_COMPLETE` | `plan_name` / `plan_path` | string | 当前 trigger 复制 | `## TRIGGER CONTEXT` | 与 manifest 对齐 | operator routing | 两字段 `fill_rule` 禁手写 | 不落盘·本 hat 持有 |
| (finalizer 自产物,仅 review.wave.failed 路径) | `wave-blocked.md` | markdown | finalizer Step 2 Write tool | 本 hat 可见 | YAML frontmatter 解析 | 本 hat Step 3 emit 引用 | 不适用(是 artifact) | 必填 · 路径 `.ralph/review/<plan>/wave-blocked.md` |

---

## 与 lint / scenario 对齐(U6 验收 fixture 思路)

- `ralph preset check -H builtin:implementation-review --strict`:无 error
  (WAC strict,Multi-hat isolation,Ownership,Topic format,Schema parity,
  Trigger context Lints 全部绿色)。
- `ralph emit --policy-check --schema` 对每个 agent-authored topic 给出
  无 missing field 的 shape 反馈;policy-check JSON 不出现
  `validation_errors`。
- EventLoop scenarios(U2 / U3 / U4)在
  `crates/ralph-core/tests/scenarios/implementation_review_{scope,wave,
  fan_in}.yml` 中走 `run_workflow_guard_scenario` 真路径,断言事件序
  / absent events / 终态;**禁止**用 `run_scenario` stub。
- CLI dispatcher integration(U3)通过 `ralph-cli/src/loop_runner/tests/
  wave_supervisor.rs`(或现有归属文件)证明一次六 payload 单 wave_id
  + SharedReadonly slots + runtime fan-in 的真实路径。
- 关键 forbidden 行为:`agent emit review.wave.complete` 必须被 origin
  guard reject(U3 scenario + origin.rs allowlist)。

## 同步清单(U5 / U1 7-point)

1. `crates/ralph-core/src/event_loop/mod.rs` — 无变更(本 preset 不改
   runtime step-close / completion 语义;`completion_promise: LOOP_COMPLETE`
   + `required_events: []` 是 builtin-only)。
2. `crates/ralph-core/src/preset_lint/` — 无新增 finding_id;复用现有
   `preset.multi_hat_requires_isolated` /
   `preset.supervisor_hat_publishes_coord_topic` /
   `preset.wave_agent_emits_coordination_topic` /
   `preset.artifact_uses_internal_ledger` /
   `preset.trigger_context_*` 系列。本 preset 六 hat isolated,所有
   hat `publishes` 不含 `wave.*`,所以无需新 lint。
3. `crates/ralph-core/tests/scenarios/` — 新增
   `implementation_review_scope.yml` / `_wave.yml` / `_fan_in.yml`(U2/
   U3/U4),在 `scenarios.rs` 注册。**必须**用
   `run_workflow_guard_scenario`,不得 `run_scenario`。
4. `crates/ralph-core/src/config/loop_config.rs` +
   `crates/ralph-cli/src/preflight.rs` +
   `crates/ralph-cli/src/config_resolution.rs` — 无变更(本 preset
   不改 core event_loop 字段)。
5. `crates/ralph-cli/src/presets.rs` — 新增
   `EmbeddedPreset { name: "implementation-review", description:
   "...", content: include_str!(".../implementation-review.yml"),
   public: true }`;同步 SSOT 测试。
6. `presets/manifest.yml` `embedded:` 加 `implementation-review`;
   `presets/index.json` `category: "development"`。
7. `CLAUDE.md` / `AGENTS.md` builtin preset 列表加
   `implementation-review`(KTD1 命名,`cp` 同步);
   `.cursor/rules/multi-hat-isolation.mdc` 可见列表同步;
   `scripts/ralph-zsh-plugin.zsh` compadd 加
   `builtin:implementation-review`(`cp` 到
   `~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`)。