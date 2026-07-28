# Red Team Attack Preset 设计规范

> **定位**：一个完全独立、实验驱动、只读代码树、只交付修复计划的 Red Team Preset。  
> **核心输入**：一个或多个开发计划。  
> **核心输出**：经过 Git 溯源、Patch 重建、真实实验、证据硬门禁和独立审查的零回归修复计划。  
> **执行边界**：本 Preset 不修改生产代码、不修改正式测试、不提交代码、不自动调用 Coding Agent。  
> **人工门禁**：最终计划必须由操作者明确确认后，才能进入编码阶段。

---

## 1. 目标

该 Preset 面向以下场景：

- 一个或多个开发计划已经被执行；
- 这些计划可能在同一分支中串行完成，也可能通过 merge、squash、rebase、cherry-pick 等方式进入当前代码树；
- 原开发 Worktree、临时分支和执行现场可能已经不存在；
- 操作者只提供开发计划，不提供具体提交列表、Diff、审计报告或既有分析产物；
- Preset 必须自行从 Git 历史中定位实现提交、重建 Patch、分析当前最终代码树，并通过真实实验验证潜在问题；
- 最终只输出一份不产生无关改动、以零回归为硬约束的修复计划或开发计划。

该 Preset 的核心流程：

```text
多个开发计划
    ↓
锁定当前目标代码树
    ↓
从 Git 历史反向定位计划实现提交
    ↓
重建每计划 Patch 和 Combined Patch
    ↓
建立计划声明与真实实现的追踪矩阵
    ↓
识别跨计划攻击面
    ↓
设计可执行实验
    ↓
实际运行控制组与攻击组
    ↓
保存原始证据并进行硬阈值评分
    ↓
限定真实影响范围
    ↓
只为全部达标 Finding 生成修复计划
    ↓
独立审查
    ↓
等待人工确认
```

---

## 2. 独立性原则

本 Preset 是完全独立的流程，不以前置审计、合并收敛流程或历史分析产物作为运行条件。

### 2.1 必填输入

唯一必填业务输入：

```yaml
plans:
  - docs/plans/plan-a.md
  - docs/plans/plan-b.md
  - docs/plans/plan-c.md
```

### 2.2 可选输入

```yaml
target_branch: optional
target_commit: optional
verification_commands: optional
allowed_test_environments: optional
forbidden_external_targets: optional
```

未提供目标 commit 时：

```text
target_commit = 当前 HEAD
```

未提供目标 branch 时：

```text
target_branch = 当前分支
```

### 2.3 明确不依赖

以下内容全部不是必要条件：

- 原开发 Worktree
- 原开发分支
- 原始执行 Agent 的上下文
- 手工提供的 commit 列表
- 手工提供的 Diff
- 手工提供的 Finding
- 历史 CI 报告

即使以上内容全部不存在，本 Preset 也必须能够独立运行。

---

## 3. 运行原则

### 3.1 实验优先

本 Preset 不是“代码阅读型 Red Team”，而是：

```text
计划溯源
+ Patch 重建
+ 实验执行
+ 原始证据保存
+ 硬阈值筛选
+ 零回归修复规划
```

硬规则：

> 没有实际执行实验，就不能形成正式 Finding。  
> 没有控制组和攻击组，就不能形成正式 Finding。  
> 没有可复查原始证据，就不能形成正式 Finding。  
> 四项指标任意一项低于阈值时，必须先按失败指标进行定向 Retry；只有重试预算耗尽后仍不达标，才从最终修复计划中淘汰。

### 3.2 不修改代码

本 Preset 在整个运行期间禁止：

- 修改生产代码；
- 修改正式测试代码；
- 修改 tracked 配置；
- 修改开发计划；
- 执行 `git add`；
- 执行 `git commit`；
- 执行 `git merge`；
- 执行 `git rebase`；
- 执行 `git cherry-pick`；
- 执行 `git reset --hard`；
- 应用修复 Patch；
- 启动 Coding Agent 执行修复；
- 自动将计划交给执行器。

允许在以下 gitignored 目录中生成实验辅助物：

```text
.ralph/red-team/repros/
.ralph/red-team/evidence/
.ralph/red-team/sandbox/
.ralph/red-team/logs/
.ralph/red-team/patches/
```

允许生成：

- 临时 Shell/Python 脚本；
- 临时测试输入；
- 临时配置副本；
- HTTP 请求文件；
- 浏览器自动化步骤；
- 并发执行脚本；
- 损坏数据样本；
- 旧格式样本；
- 故障注入脚本；
- 实验日志、截图和状态快照。

### 3.3 Confirm Before Code

```text
A generated PLAN.md is not authorization to modify code.
```

任何以下情况都必须先写入 `.ralph/red-team/QUESTIONS.md`，并等待操作者确认：

- 产品行为存在多种合法选择；
- 需要修改公共接口；
- 需要修改数据格式；
- 需要执行迁移；
- 需要改变默认行为；
- 需要扩大作用范围；
- 需要接受风险；
- 需要架构取舍；
- 需要兼容性取舍；
- 需要删除旧能力；
- 需要调用 Coding Agent。


### 3.4 指标、阈值与 Retry 总览

本 Preset 使用三层门禁，所有门禁均采用“单项达标”，禁止平均分和高分补偿低分。

| 层级 | 指标 | 默认阈值 | 用途 |
|---|---|---:|---|
| Git 溯源 | Commit Match Confidence | 85 | 判断开发计划与候选提交的归属是否可靠 |
| Patch 重建 | Patch Attribution Coverage | 90 | 判断目标变更是否完成归属分析 |
| Patch 重建 | Critical Claim Traceability | 100 | 判断关键计划声明能否追踪到 Commit、Diff、当前实现与测试 |
| Finding | Confidence | 85 | 判断问题是否真实存在，而非环境噪声或错误归因 |
| Finding | Evidence Coverage | 85 | 判断必要证据义务是否覆盖充分 |
| Finding | Verifiability | 90 | 判断独立 Agent 能否稳定、机器可判定地复现 |
| Finding | Impact Certainty | 85 | 判断影响边界、传播路径及不受影响范围是否明确 |

P0/P1 Finding 的四项 Finding 指标全部提高到 `90`。

阈值筛选采用短板门禁：

```text
qualified
=
每一项指标都达到自身阈值
AND
全部 mandatory evidence 已满足
AND
二元执行门禁全部通过
```

不存在以下逻辑：

```text
四项平均分达标
某一项高分补偿另一项低分
因为 Severity 高而降低证据门槛
人工主观覆盖机器门禁
```

未达阈值时不是立即淘汰，而是进入定向 Retry：

```text
首次评估未达标
    ↓
识别失败指标和缺失证据
    ↓
生成 Retry Delta（本轮必须新增什么）
    ↓
重新设计或扩展实验
    ↓
重新执行控制组与攻击组
    ↓
重新评分
    ↓
达标 → 继续
仍未达标且有预算 → 再次 Retry
预算耗尽 → 最终淘汰
```

默认重试预算：

```yaml
retry_policy:
  default_max_retries: 2   # 初次执行之外，最多再执行 2 轮
  p0_p1_max_retries: 3     # P0/P1 初次执行之外，最多再执行 3 轮
  require_retry_delta: true
  blind_rerun_forbidden: true
  score_must_be_recomputed_from_raw_evidence: true
```

每次 Retry 必须有实质变化，例如增加重复次数、收紧控制变量、补充状态快照、构造独立环境、增加重启验证或补充边界实验。只重复相同命令而没有新增证据，不计作有效 Retry。

---

## 4. Hat 数量与流程

建议使用 **10 个 Hat**。

```text
01 Target Locker
        ↓
02 Plan Resolver ↺            # Commit 匹配不足时深化 Git 搜索
        ↓
03 Patch Reconstructor ↺      # Patch 归属不足时扩大/校正重建
        ↓
04 Attack Surface Mapper
        ↓
05 Experiment Designer ←──────────────┐
        ↓                              │
06 Experiment Runner                  │
        ↓                              │
07 Evidence Gate ── RETRY_REQUIRED ───┘
        ↓ QUALIFIED
08 Impact Boundary ── RETRY_REQUIRED ─→ 05/06
        ↓ IMPACT_QUALIFIED
09 Repair Planner
        ↓
10 Independent Reviewer
```

使用 10 个 Hat 的原因：

- Git 提交归属识别与 Patch 重建必须分离；
- 攻击面识别与实验设计必须分离；
- 实验执行与证据评分必须分离；
- 影响范围分析不能由发现问题的 Hat 自行批准；
- 修复计划必须接受独立审查；
- 避免“自己发现、自己证明、自己批准、自己规划”的偏差。

---

# 5. Hat 01：Target Locker

## 5.1 职责

锁定本轮 Red Team 的攻击目标，并证明整个实验期间代码树没有变化。

## 5.2 必须实际执行

```bash
git rev-parse --show-toplevel
git branch --show-current
git rev-parse HEAD
git rev-parse HEAD^{tree}
git status --porcelain=v1
git remote -v
git log -1 --date=iso --format=fuller
git diff --check
```

检测是否存在未完成操作：

```bash
git rev-parse -q --verify MERGE_HEAD
git rev-parse -q --verify REBASE_HEAD
git rev-parse -q --verify CHERRY_PICK_HEAD
```

计算开发计划内容哈希：

```bash
sha256sum <plan-path>
```

## 5.3 锁定内容

```yaml
target_repository:
target_branch:
target_head:
target_tree:
working_tree_status:
plan_file_hashes:
started_at:
```

## 5.4 运行期不变量

每个后续 Hat 和每次实验开始、结束时都必须检查：

```bash
test "$(git rev-parse HEAD)" = "<locked-head>"
test "$(git rev-parse HEAD^{tree})" = "<locked-tree>"
git diff --quiet
git diff --cached --quiet
```

出现以下任一情况时终止：

```text
TARGET_HEAD_CHANGED
TARGET_TREE_CHANGED
UNEXPECTED_TRACKED_MODIFICATION
MERGE_IN_PROGRESS
REBASE_IN_PROGRESS
CHERRY_PICK_IN_PROGRESS
```

## 5.5 产物

```text
.ralph/red-team/01-target-lock.md
```

---

# 6. Hat 02：Plan Resolver

## 6.1 职责

根据开发计划内容，从 Git 历史中反向寻找对应的真实实现提交。

不允许只根据提交时间或作者进行猜测。

## 6.2 提取计划指纹

每个计划至少提取：

```yaml
plan_fingerprint:
  plan_path:
  plan_hash:
  title:
  plan_id:
  date:
  units:
  scenarios:
  acceptance_criteria:
  expected_files:
  expected_symbols:
  expected_tests:
  commands:
  cli_flags:
  config_keys:
  state_fields:
  data_formats:
  migration_names:
  explicit_commit_ids:
  change_ids:
  issue_ids:
```

## 6.3 必须实际执行的 Git 搜索

```bash
git log --all --decorate --date=iso --name-status
git log --first-parent --date=iso
git log --merges --date=iso
git log --all --grep="<plan-id>"
git log --all --grep="<plan-title>"
git log --all -- "<planned-path>"
git log --all -S"<config-key>"
git log --all -S"<symbol>"
git log --all -G"<regex>"
git blame <suspected-file>
```

对每个候选提交：

```bash
git show --stat --summary <sha>
git show --name-status <sha>
git show --format=fuller <sha>
git diff-tree --root --no-commit-id --name-status -r <sha>
```

处理 cherry-pick、rebase、squash：

```bash
git show <sha> | git patch-id --stable
```

## 6.4 匹配证据

### 硬证据

- commit message 中出现 plan ID；
- commit message 中出现计划路径；
- commit message 中出现明确 Change-Id；
- 提交新增了计划声明的唯一文件；
- 提交新增了计划声明的唯一符号；
- 提交新增了计划声明的唯一配置字段；
- 提交新增了计划声明的唯一 CLI 参数；
- 提交新增了计划声明的验收测试；
- patch-id 与已知提交一致。

### 软证据

- 时间接近；
- 作者一致；
- commit message 语义相似；
- 修改模块相同；
- 提交顺序符合计划 Unit 顺序；
- 位于其他已确认提交之间。

### 硬规则

至少必须存在一项硬证据。

以下情况禁止单独作为匹配依据：

```text
只有时间接近
只有作者相同
只有提交信息相似
只有修改模块相同
只有提交顺序相近
```

## 6.5 Commit 匹配阈值

```yaml
commit_resolution_gate:
  commit_match_confidence_min: 85
  require_hard_evidence: true
  allow_time_only_match: false
  allow_author_only_match: false
  allow_message_similarity_only: false
```

建议评分：

| 证据 | 分值 |
|---|---:|
| 明确 plan ID、计划路径或 Change-Id | 35 |
| 核心文件路径匹配 | 20 |
| 唯一符号、配置或 CLI 匹配 | 15 |
| 验收测试匹配 | 15 |
| 行为语义匹配 | 5 |
| 提交顺序一致 | 5 |
| 时间范围一致 | 5 |

评分达到 85 仍必须满足硬证据规则。

## 6.6 未达阈值的 Git 溯源 Retry

首次 `commit_match_confidence < 85` 时，不得立即猜测归属，也不得直接进入 Patch 重建。Plan Resolver 必须生成 `resolution_retry_delta`，说明当前缺少哪些证据，并执行定向深化：

- 扩大 `git log --all` 的时间和路径范围；
- 使用更多计划中的唯一符号、配置项、测试名执行 `-S` / `-G` 搜索；
- 检查 merge、squash、rebase、cherry-pick 和 patch-id 等价关系；
- 检查后续覆盖提交以及同一提交同时服务多个计划的情况；
- 对候选提交执行逐文件、逐 hunk 对照，而不是只比较 commit message；
- 重新计算分数并保留新增证据路径。

默认最多 Retry 2 轮。每一轮必须新增硬证据或明确排除候选；无新增信息的重复搜索不计作有效 Retry。

状态流：

```text
CANDIDATE_MATCH
→ SCORE_BELOW_THRESHOLD
→ RESOLUTION_RETRY_REQUIRED
→ RESCORED
→ RESOLVED | RESOLUTION_RETRY_REQUIRED | PLAN_UNRESOLVED_AFTER_RETRY
```

重试预算耗尽后仍低于 85：

```text
PLAN_UNRESOLVED_AFTER_RETRY
```

该计划才会被排除：

- 不进入 Patch 重建；
- 不进入攻击；
- 不进入最终修复计划；
- 不允许通过人工猜测自动补齐。

所有计划均在 Retry 耗尽后无法解析时：

```text
REJECTED_NO_RESOLVED_PLAN
```

## 6.7 产物

```text
.ralph/red-team/02-plan-resolution.md
.ralph/red-team/commits/PLAN-NNN.md
```

---

# 7. Hat 03：Patch Reconstructor

## 7.1 职责

基于已经确认的提交集合，重建每个计划的真实变更，并生成当前最终代码树的有效合并 Patch。

## 7.2 原始提交 Patch

每个匹配 commit 单独保存：

```bash
git format-patch -1 --stdout --binary --full-index <sha>
```

输出示例：

```text
.ralph/red-team/patches/PLAN-001/
├── 001-<sha>.patch
├── 002-<sha>.patch
└── 003-<sha>.patch
```

## 7.3 每计划 Patch Series

按真实提交顺序组合：

```text
.ralph/red-team/patches/PLAN-001.series.patch
```

禁止对不连续提交直接使用宽泛的 `base..tip`，避免混入其他计划提交。

## 7.4 Combined Current Patch

从所有已解析计划中最早实现提交的第一父提交，到锁定 HEAD：

```bash
git diff \
  --binary \
  --full-index \
  --find-renames \
  --find-copies \
  <global-baseline>..<locked-head>
```

输出：

```text
.ralph/red-team/patches/combined-current.patch
```

## 7.5 变更归属分类

每个 hunk 必须标记：

```text
PLAN_OWNED
SHARED_BY_MULTIPLE_PLANS
OVERRIDDEN_LATER
UNATTRIBUTED
RELATED_NEIGHBOR_CHANGE
```

## 7.6 Claim 追踪矩阵

必须建立：

```text
开发计划声明
→ 匹配 commit
→ Patch hunk
→ 当前最终代码
→ 调用者
→ 测试
→ 当前实现状态
```

状态至少包括：

```text
IMPLEMENTED
PARTIALLY_IMPLEMENTED
MISSING
IMPLEMENTED_DIFFERENTLY
OVERRIDDEN_BY_LATER_COMMIT
UNPLANNED_CHANGE
DEAD_CODE
UNTESTED
UNRESOLVED
```

## 7.7 Patch 门禁

```yaml
patch_gate:
  patch_attribution_coverage_min: 90
  critical_claim_traceability_min: 100
  unresolved_critical_hunks_allowed: 0
```

关键声明包括：

- 公共接口；
- 数据格式；
- 持久化状态；
- 状态迁移；
- 默认配置；
- 权限和安全边界；
- 删除和 cleanup；
- 并发控制；
- 对外可见行为。

首次未达到 Patch 门禁时不得直接进入实验阶段，也不得立即淘汰。Patch Reconstructor 必须执行定向 Retry：

- `Patch Attribution Coverage < 90`：扩展候选提交集合、检查后续覆盖、拆分共享提交、逐 hunk 重新归属；
- `Critical Claim Traceability < 100`：回到计划声明，补查 Commit、Diff、当前实现、调用者与测试之间的缺口；
- 存在关键 unresolved hunk：追查 blame、父提交、merge parent、patch-id 和历史重写关系。

默认最多 Retry 2 轮。每轮必须生成新的 Patch、追踪矩阵差异或排除证据。重试预算耗尽仍不达标时，相关计划标记为 `PATCH_UNRESOLVED_AFTER_RETRY`，不得进入实验阶段。

## 7.8 产物

```text
.ralph/red-team/03-patch-reconstruction.md
.ralph/red-team/patches/**
```

---

# 8. Hat 04：Attack Surface Mapper

## 8.1 职责

基于开发计划、提交、Patch、当前最终代码树和未修改调用者，识别必须动手验证的攻击面。

## 8.2 攻击对象

攻击对象不只是 Diff 修改行，还包括：

- 修改函数的调用者；
- 状态的读写双方；
- 成功路径与失败路径；
- 创建与删除路径；
- 启动与关闭路径；
- 新格式与旧读取方；
- 配置开启与关闭路径；
- 多个计划之间的交叉点；
- Patch 没修改但本应同步修改的地方；
- 后续提交覆盖前面计划的地方。

## 8.3 强制攻击维度

### 契约攻击

- 调用者是否继续使用旧语义；
- 新错误是否被上层吞掉；
- 新字段是否被旧消费者忽略；
- 枚举或状态值是否漂移；
- 返回成功时状态是否真正成功。

### 状态攻击

- 内存与持久化是否一致；
- 是否存在双重状态权威；
- 部分写入后是否暴露半完成状态；
- 删除后是否遗留引用；
- 重启后状态是否恢复。

### 生命周期攻击

- 首次启动；
- 重复启动；
- 正常关闭；
- 强制终止；
- 崩溃重启；
- 升级；
- 降级；
- 卸载；
- 清理。

### 配置攻击

- 默认配置；
- 功能关闭；
- 非法组合；
- 环境变量、配置文件和 CLI 优先级；
- 旧配置兼容；
- 缺失配置。

### 并发与幂等攻击

- 双击；
- 双提交；
- 两个进程并发操作；
- 重试；
- 超时后再次请求；
- 调度器重复派发；
- cleanup 与执行并发。

### 外部系统攻击

- 网络断开；
- 4xx/5xx；
- 响应超时；
- 响应损坏；
- 重复响应；
- 浏览器刷新；
- 浏览器回退；
- Session 过期；
- Cookie、本地存储和服务端状态不一致。

## 8.4 攻击面记录

```yaml
surface_id: RTS-001
plans:
commits:
patch_hunks:
affected_modules:
target_claim:
target_invariant:
attack_dimensions:
```

## 8.5 产物

```text
.ralph/red-team/04-attack-surface.md
```

---

# 9. Hat 05：Experiment Designer

## 9.1 职责

将攻击面转化成可实际执行、可重复、可判定的实验。

禁止使用模糊描述：

```text
测试一下并发是否安全
检查一下重启是否正常
看看页面会不会出问题
```

必须设计成具体实验。

## 9.2 标准实验结构

```yaml
experiment_id: RTE-001
surface_id: RTS-001

hypothesis:
  "两个并发请求使用相同 operation_id 时可能创建两条记录"

invariant:
  "相同 operation_id 在持久化层最多存在一条有效记录"

environment:
  required_services:
  required_devices:
  required_credentials:
  isolation_method:

setup_commands:
control_commands:
attack_commands:
state_inspection_commands:
cleanup_commands:

expected_control:
expected_attack_safe_result:
failure_oracle:
repeat_policy:
mandatory_evidence:
```

## 9.3 控制组强制要求

每个实验都必须包含：

- 控制组；
- 攻击组；
- 可判定的不变量；
- 状态检查；
- 重复策略；
- 清理策略。

示例：

### 控制组

```text
顺序提交两个不同 operation_id
```

### 攻击组

```text
并发提交两个相同 operation_id
```

控制组用于证明：

- 环境正常；
- 服务正常；
- 凭证正常；
- 功能正常；
- 失败不是环境问题。

没有控制组的实验，不得形成正式 Finding。

## 9.4 产物

```text
.ralph/red-team/05-experiment-plan.md
```

---

# 10. Hat 06：Experiment Runner

## 10.1 职责

实际执行实验。

这是 self-loop Hat，一次 activation 只执行一个实验。

```text
选择实验
→ 检查锁定 HEAD
→ 准备隔离环境
→ 记录初始状态
→ 执行控制组
→ 恢复初始状态
→ 执行攻击组
→ 检查状态与副作用
→ 按要求重复
→ 重启后复查
→ 清理
→ 验证 tracked tree 未变化
→ 保存原始证据
```

## 10.2 允许执行的操作

### Git 和静态工具

```text
git log
git show
git diff
git blame
git grep
rg
grep
find
nm
objdump
readelf
依赖图工具
静态分析器
```

但仅靠静态阅读不能形成行为 Finding。

### 构建和已有测试

允许：

- 构建整个项目；
- 构建受影响模块；
- 运行已有单元测试；
- 运行已有集成测试；
- 运行已有系统测试；
- 运行已有 lint；
- 运行 format-check；
- 运行 static check；
- 运行已有 benchmark；
- 运行已有 fuzz target。

### CLI 和程序运行

允许：

- 启动程序；
- 执行命令；
- 重复执行命令；
- 传入非法参数；
- 传入边界参数；
- 并发执行命令；
- 终止本实验启动的程序；
- 超时后重新执行；
- 比较执行前后状态。

### 文件与持久化实验

仅允许在隔离目录、临时数据或副本中：

- 创建文件；
- 删除临时文件；
- 修改临时文件权限；
- 设置只读；
- 构造损坏数据；
- 截断临时文件；
- 模拟旧版本格式；
- 占用测试锁；
- 创建残留临时文件；
- 模拟写入失败；
- 检查 crash 后数据。

禁止对真实用户数据和生产数据做破坏性实验。

### 进程和资源实验

允许对本实验启动的进程：

- 正常停止；
- 强制终止；
- 超时终止；
- 模拟子进程异常退出；
- 模拟端口占用；
- 模拟文件锁竞争；
- 设置 CPU 或内存限制；
- 模拟网络中断；
- 重启测试服务。

绝对禁止：

```text
终止 ralph
终止 ralph-orchestrator
终止非本实验创建的未知进程
```

### HTTP/API 实验

允许：

- GET/POST/PUT/DELETE 测试请求；
- 重复请求；
- 并发请求；
- 错误 token；
- 过期 token；
- 超时；
- 断网恢复；
- 错误响应；
- 部分响应；
- 重放请求；
- 请求乱序。

默认只能作用于：

- 本地服务；
- 测试环境；
- Staging；
- Mock server；
- 临时容器；
- 操作者明确授权的非生产系统。

如果只发现生产 URL：

- 禁止执行有副作用攻击；
- 只允许安全只读检查；
- 或标记为不可安全执行。

### 网页操作

允许：

- 打开页面；
- 使用测试账户登录；
- 点击按钮；
- 提交表单；
- 连续双击；
- 重复提交；
- 刷新；
- 浏览器后退；
- 关闭页面再打开；
- 清理 Cookie；
- 清理 LocalStorage；
- 使用新浏览器会话；
- 模拟 Session 过期；
- 检查 Network 请求；
- 检查响应状态；
- 对比页面状态与服务端状态。

网页实验必须保存：

- 操作步骤；
- 页面地址；
- 浏览器和版本；
- 测试账户类型；
- 截图；
- Network 请求和响应；
- Console 错误；
- 页面状态；
- 服务端状态；
- 刷新或重新登录后的状态。

禁止：

- 真实支付；
- 真实下单；
- 向大量真实用户发送通知；
- 发送真实短信或邮件；
- 删除生产数据；
- 修改生产权限。

## 10.3 重复次数门禁

### 确定性功能实验

```yaml
control:
  passed: 2/2

attack:
  reproduced: 2/2
```

### 故障恢复和重启实验

```yaml
control:
  passed: 3/3

attack:
  reproduced: 3/3
```

### 可控并发实验

当时序可以通过 barrier、锁或现有测试钩子稳定控制：

```yaml
attack:
  reproduced: 3/3
```

### 概率型竞态实验

无法稳定控制时序时：

```yaml
attempts_min: 30
failures_min: 3
control_failures_allowed: 0
```

30 次只出现 1 次时，可以保留实验记录，但通常无法达到 Confidence 阈值。

### 网页实验

至少：

```text
同一浏览器会话重复 2 次
全新浏览器会话或清理存储后重复 1 次
```

涉及刷新、回退、Session 恢复时，必须验证服务端最终状态。

## 10.4 实验后代码树验证

```bash
git status --porcelain=v1
git diff --exit-code
git diff --cached --exit-code
```

如果 tracked tree 被污染：

```text
EXPERIMENT_INVALID_CODE_MUTATION
```

该实验结果作废。

## 10.5 每个实验必须保存的证据

```yaml
experiment_evidence:
  experiment_id:
  target_head:
  plan_ids:
  commit_ids:
  patch_hunks:

  environment:
    os:
    runtime_versions:
    dependency_versions:
    browser_version:
    service_versions:
    feature_flags:
    config_hashes:

  setup:
    commands:
    exit_codes:
    stdout_paths:
    stderr_paths:

  control:
    exact_commands:
    expected:
    actual:
    state_before:
    state_after:
    passed:

  attack:
    exact_commands:
    expected_safe_behavior:
    actual:
    invariant_violation:
    state_before:
    state_after:
    side_effects:

  repetitions:
    attempts:
    failures:
    reproduction_rate:

  recovery:
    restart_result:
    retry_result:
    persistent_state:

  cleanup:
    commands:
    resources_remaining:
    tracked_tree_clean:

  evidence_paths:
    logs:
    screenshots:
    http_traces:
    state_dumps:
```

一段 Agent 自述不能作为证据。

## 10.6 产物

```text
.ralph/red-team/experiments/RTE-NNN.md
.ralph/red-team/evidence/RTE-NNN/**
.ralph/red-team/repros/RTE-NNN/**
```

---

# 11. Hat 07：Evidence Gate

## 11.1 职责

独立审核实验是否真实执行、证据是否充分，并使用硬阈值筛选。

不接受 Experiment Runner 的主观结论，只读取原始证据。

## 11.2 二元执行门禁

```yaml
execution_gate:
  experiment_actually_executed: true
  target_head_matches: true
  control_case_passed: true
  attack_case_completed: true
  concrete_invariant_defined: true
  actual_state_inspected: true
  repeat_requirement_met: true
  cleanup_verified: true
  tracked_tree_unchanged: true
  raw_evidence_present: true
```

二元门禁失败时先分类：

### 可 Retry

以下失败通常可以通过补实验修复：

- 控制组未充分验证；
- 攻击组未完整执行；
- 状态检查不足；
- 重复次数不足；
- 原始证据缺失但实验可以安全重跑；
- cleanup 验证不完整；
- 不变量定义不够可判定。

这些情况输出 `EXECUTION_RETRY_REQUIRED`，回到 Experiment Designer，补齐实验设计后重新执行。

### 不可直接 Retry

以下情况使本轮实验无效：

- 目标 HEAD 已变化；
- tracked tree 被修改且无法确认来源；
- 实验触碰未授权生产环境或真实数据；
- 无法安全清理实验副作用；
- 实验执行违反禁止修改代码或 Git 历史的约束。

这些情况必须先停止实验并恢复安全状态。未恢复前不得继续评分；安全恢复后只能创建新的实验轮次，旧证据不得复用为正式证据。

## 11.3 四项指标

所有指标范围：

```text
0–100
```

禁止平均、禁止加权补偿。

### 11.3.1 Confidence

回答：

> 这个问题真实存在的确定程度是多少？

建议构成：

| 子项 | 权重 |
|---|---:|
| 重复稳定性 | 35 |
| 控制组与攻击组区分度 | 25 |
| 替代原因排除程度 | 20 |
| 多来源证据一致性 | 20 |

默认阈值：

```yaml
confidence_min: 85
```

### 11.3.2 Evidence Coverage

回答：

> 为证明该问题所需的证据义务覆盖了多少？

计算：

```text
Evidence Coverage
=
已完成的必要证据义务权重
÷
全部必要证据义务权重
× 100
```

可能的证据义务：

- 输入；
- 输出；
- 退出码；
- 内存状态；
- 持久化状态；
- 外部副作用；
- 控制组；
- 攻击组；
- 重试；
- 重启；
- cleanup；
- 并发；
- 兼容性；
- 网络请求；
- 页面状态；
- 服务端状态；
- 环境独立性。

默认阈值：

```yaml
evidence_coverage_min: 85
```

即使分数达到 85，只要缺少 mandatory evidence，仍不能通过；应先进入 `EVIDENCE_RETRY_REQUIRED` 补齐证据，只有 Retry 预算耗尽后仍缺失才最终淘汰。

### 11.3.3 Verifiability

回答：

> 一个没有参与实验设计和执行的 Agent，能否独立、稳定、机器可判定地复现？

建议构成：

| 子项 | 权重 |
|---|---:|
| 步骤精确完整 | 25 |
| 输入和环境可以准备 | 20 |
| 结果可以机器判定 | 25 |
| 可在隔离环境重复 | 20 |
| 独立复跑成功 | 10 |

默认阈值：

```yaml
verifiability_min: 90
```

### 11.3.4 Impact Certainty

回答：

> 对问题影响边界的确定程度是多少？

它不等于 Severity。

建议构成：

| 子项 | 权重 |
|---|---:|
| 直接受影响模块明确 | 25 |
| 状态传播路径明确 | 25 |
| 生命周期和配置影响明确 | 20 |
| 兼容性影响明确 | 15 |
| 不受影响边界明确 | 15 |

默认阈值：

```yaml
impact_certainty_min: 85
```

## 11.4 默认 Finding 门禁

```yaml
finding_gate:
  confidence_min: 85
  evidence_coverage_min: 85
  verifiability_min: 90
  impact_certainty_min: 85

  aggregation: none
  qualification_requires_all_thresholds: true
  retry_on_any_below_threshold: true
  retry_on_missing_mandatory_evidence: true
  discard_only_after_retry_exhausted: true
```

示例：

```yaml
confidence: 96
evidence_coverage: 92
verifiability: 89
impact_certainty: 94
```

首次评估结果：

```text
RETRY_REQUIRED_LOW_VERIFIABILITY
```

Experiment Designer 必须针对 Verifiability 生成 Retry Delta，例如固定环境、脚本化步骤、增加机器判定 oracle，并由独立执行者复跑。只有 Retry 预算耗尽后仍低于 90，才转为：

```text
REJECTED_AFTER_RETRY_LOW_VERIFIABILITY
```

其他高分不能补偿低分。

## 11.5 P0/P1 阈值

```yaml
severity_thresholds:
  P0:
    confidence_min: 90
    evidence_coverage_min: 90
    verifiability_min: 90
    impact_certainty_min: 90

  P1:
    confidence_min: 90
    evidence_coverage_min: 90
    verifiability_min: 90
    impact_certainty_min: 90

  P2:
    confidence_min: 85
    evidence_coverage_min: 85
    verifiability_min: 90
    impact_certainty_min: 85

  P3:
    confidence_min: 85
    evidence_coverage_min: 85
    verifiability_min: 90
    impact_certainty_min: 85
```

严重度越高，证据要求不能降低。

## 11.6 未达阈值的定向 Retry 与最终淘汰

首次评分低于阈值时，Evidence Gate 不创建正式 Finding，而是生成：

```yaml
retry_required:
  experiment_id:
  attempt:
  failed_metrics:
  current_scores:
  required_thresholds:
  missing_mandatory_evidence:
  retry_delta:
  next_owner: Experiment Designer
```

### 按失败指标路由

| 未达标指标 | 必须采取的 Retry 动作 |
|---|---|
| Confidence | 增加有效重复次数；增加控制组；收紧变量；排除环境、依赖、随机性和错误归因；增加独立证据源 |
| Evidence Coverage | 补齐缺失的 mandatory evidence；增加状态、持久化、副作用、重试、重启、cleanup、兼容性或服务端证据 |
| Verifiability | 将步骤脚本化；固定输入和环境；增加机器判定 oracle；降低人工判断；由未参与设计的执行者独立复跑 |
| Impact Certainty | 增加调用者、消费者、配置、生命周期、兼容性和不受影响范围的边界实验；必要时转由 Impact Boundary 补实验 |

### Retry 约束

```yaml
retry_policy:
  default_max_retries: 2
  p0_p1_max_retries: 3
  retry_same_commands_without_delta: forbidden
  production_code_change_during_retry: forbidden
  formal_test_change_during_retry: forbidden
  require_new_raw_evidence: true
  require_full_rescore: true
```

Retry 次数不包含首次实验。例如 `default_max_retries: 2` 表示最多执行：

```text
首次实验 + Retry 1 + Retry 2
```

每次 Retry 后四项指标必须全部重新计算，不能沿用上一轮高分，也不能只重算失败项。

### 通过条件

```text
所有二元门禁通过
AND
所有 mandatory evidence 已满足
AND
Confidence 达标
AND
Evidence Coverage 达标
AND
Verifiability 达标
AND
Impact Certainty 达标
```

### Retry 耗尽后的最终淘汰

只有重试预算耗尽后仍不达标，才执行最终淘汰：

```text
REJECTED_AFTER_RETRY_NOT_EXECUTED
REJECTED_AFTER_RETRY_CONTROL_FAILED
REJECTED_AFTER_RETRY_NOT_REPRODUCIBLE
REJECTED_AFTER_RETRY_LOW_CONFIDENCE
REJECTED_AFTER_RETRY_LOW_EVIDENCE
REJECTED_AFTER_RETRY_LOW_VERIFIABILITY
REJECTED_AFTER_RETRY_UNCERTAIN_IMPACT
REJECTED_AFTER_RETRY_DIRTY_WORKTREE
REJECTED_AFTER_RETRY_MISSING_MANDATORY_EVIDENCE
```

最终淘汰的候选问题：

- 不创建正式 Finding；
- 不进入 `PLAN.md`；
- 不生成修复 Unit；
- 不成为 Accepted Risk；
- 不交给 Coding Agent。

只保留完整的尝试历史、每轮评分、Retry Delta 和最终拒绝原因：

```yaml
eligible_for_repair_plan: false
discarded_from_final_plan: true
retry_exhausted: true
```

## 11.7 产物

```text
.ralph/red-team/07-evidence-board.md
```

---

# 12. Hat 08：Impact Boundary

## 12.1 职责

只处理已经通过实验和前三项证据门禁的候选问题，并继续通过真实命令和测试限定影响范围。

## 12.2 必须回答

- 最早错误层在哪里；
- 哪个不变量被破坏；
- 哪个模块拥有修复责任；
- 哪些调用者受到影响；
- 哪些下游消费者受到影响；
- 哪些持久化状态受到影响；
- 哪些配置组合受到影响；
- 是否影响默认关闭模式；
- 是否影响重启与恢复；
- 是否影响升级和降级；
- 是否影响兼容性；
- 哪些模块确认不受影响；
- 最小安全修复边界是什么。

## 12.3 必须继续动手验证

根据问题类型选择：

- 运行直接受影响组件测试；
- 运行上游调用者测试；
- 运行下游消费者测试；
- 运行 feature-off 场景；
- 运行旧配置场景；
- 运行旧数据或旧格式场景；
- 重启后验证；
- cleanup 后验证；
- 重复执行后验证；
- 并发入口验证；
- 网页刷新和重新登录验证；
- API 调用者兼容验证。

## 12.4 正向与负向边界

不仅要证明哪里坏了，还要证明：

```text
哪些模块确定受到影响
哪些模块确定不受影响
哪些配置受到影响
哪些配置不受影响
哪些数据需要迁移
哪些数据不需要迁移
```

## 12.5 最终门禁与 Impact Retry

Impact Certainty 必须在边界实验完成后重新评分，而不是沿用 Evidence Gate 的初步分数。

首次低于阈值时输出 `IMPACT_RETRY_REQUIRED`，并明确需要补充的边界实验，例如：

- 未确定上游调用者：运行调用者测试或实际调用命令；
- 未确定下游传播：检查持久化、消息、文件、页面或服务端最终状态；
- 未确定配置边界：补默认、关闭、非法组合和优先级实验；
- 未确定兼容性：补旧数据、旧配置、升级或降级实验；
- 未确定不受影响范围：运行负向边界和对照模块验证。

Impact Retry 与 Finding Retry 共享预算：默认 2 轮，P0/P1 最多 3 轮。每轮必须产生新的边界证据并重新计算全部四项指标。

重试后达到阈值，才创建正式 Finding：

```text
.ralph/red-team/findings/RTF-NNN.md
```

重试预算耗尽仍低于阈值：标记 `REJECTED_AFTER_RETRY_UNCERTAIN_IMPACT`，不得进入修复计划。

## 12.6 Finding 标准字段

```yaml
finding_id:
title:
severity:
status: EVIDENCE_GATED
plans:
commits:
patch_hunks:
experiment_id:
broken_invariant:
root_failure_layer:
affected_modules:
unaffected_modules:
compatibility_impact:
state_impact:
lifecycle_impact:
configuration_impact:

metrics:
  confidence:
  evidence_coverage:
  verifiability:
  impact_certainty:

raw_evidence_paths:
retry_history:
final_qualified_attempt:
minimal_safe_fix_boundary:
eligible_for_repair_plan: true
```

## 12.7 产物

```text
.ralph/red-team/08-impact-boundary.md
.ralph/red-team/findings/RTF-NNN.md
```

---

# 13. Hat 09：Repair Planner

## 13.1 职责

只为全部通过阈值的正式 Finding 生成修复计划。

不编码、不修改测试、不提交代码。

## 13.2 最终输出

```text
.ralph/red-team/PLAN.md
```

## 13.3 每个 Unit 必须绑定

```yaml
source:
  finding_id:
  experiment_id:
  plan_ids:
  commits:
  patch_hunks:

metrics:
  confidence:
  evidence_coverage:
  verifiability:
  impact_certainty:

broken_invariant:
minimal_fix_locus:
allowed_files:
forbidden_scope:
existing_behavior_to_preserve:
red_test_source:
regression_matrix:
rollback_condition:
```

## 13.4 计划方法论

最终计划必须融合：

- Spec-First；
- BDD；
- ATDD；
- Outside-In；
- TDD；
- Regression；
- Clean Environment；
- Compatibility；
- Rollback。

## 13.5 每个 Unit 的执行顺序

```text
读取原始实验
→ 将临时 reproducer 转为正式失败测试
→ 确认 Red
→ 最小生产修复
→ 确认 Green
→ 局部重构
→ 组件回归
→ 集成回归
→ 跨计划回归
→ 全量回归
```

## 13.6 默认严格串行

```text
Unit 0：锁定修复前基线
Unit 1：RTF-001 — Red → Green → Refactor → Regression
Unit 2：RTF-002 — Red → Green → Refactor → Regression
Unit 3：RTF-003 — Red → Green → Refactor → Regression
Final Regression
Clean Environment
Independent Review
```

前一个 Unit 的实现、测试、重构和回归未全部完成时，不得开始下一个 Unit。

## 13.7 修复计划禁止事项

- 禁止无关重构；
- 禁止顺手修复其他 Finding；
- 禁止删除失败测试；
- 禁止弱化失败断言；
- 禁止 catch-and-ignore；
- 禁止返回假成功；
- 禁止改变公共行为来规避问题；
- 禁止扩大公共接口；
- 禁止通过默认关闭功能掩盖缺陷；
- 禁止使用不达标 Finding 生成 Unit。

## 13.8 零回归约束

该计划不能在执行前声称绝对不会产生回归，但必须通过以下机制把零回归作为硬验收目标：

```text
先锁定现有行为
先生成失败测试
限制最小修复范围
禁止无关重构
逐 Finding 回归
跨计划集成回归
全量测试
静态检查
干净环境重建
独立最终审查
```

---

# 14. Hat 10：Independent Reviewer

## 14.1 职责

独立审查最终 `PLAN.md`，不能重新解释低分实验，也不能将被拒绝项重新加入计划。

## 14.2 必须检查

- 每个 Finding 是否有真实实验；
- 控制组是否通过；
- 攻击组是否稳定复现；
- 原始证据是否存在；
- 四项指标是否全部达标；
- Commit 匹配是否达到 85；
- Patch 归属覆盖率是否达到 90；
- 关键 Claim 是否 100% 可追踪；
- 每个修复 Unit 是否最小化；
- 是否先 Red 再 Green；
- 是否保护现有行为；
- 是否覆盖调用者和消费者；
- 是否覆盖失败恢复；
- 是否覆盖并发与重复执行；
- 是否覆盖兼容性；
- 是否覆盖 clean environment；
- 是否存在无证据架构决策；
- 是否混入任何被淘汰实验；
- 是否要求未经用户确认的范围扩张。

## 14.3 最终结论

只能是：

```text
PLAN_READY
PLAN_REJECTED
```

禁止使用：

```text
PLAN_READY_WITH_LOW_CONFIDENCE
```

因为低于阈值的项已经被淘汰。

## 14.4 产物

```text
.ralph/red-team/10-independent-review.md
.ralph/red-team/REPORT.md
.ralph/red-team/QUESTIONS.md
```

---

# 15. 最终产物目录

```text
.ralph/red-team/
├── 01-target-lock.md
├── 02-plan-resolution.md
├── 03-patch-reconstruction.md
├── 04-attack-surface.md
├── 05-experiment-plan.md
├── 07-evidence-board.md
├── 07-retry-board.md
├── 08-impact-boundary.md
├── 10-independent-review.md
├── PLAN.md
├── REPORT.md
├── QUESTIONS.md
├── commits/
│   ├── PLAN-001.md
│   └── PLAN-002.md
├── patches/
│   ├── PLAN-001.series.patch
│   ├── PLAN-002.series.patch
│   └── combined-current.patch
├── retries/
│   └── RTE-001-attempt-02.md
├── experiments/
│   └── RTE-001.md
├── evidence/
│   └── RTE-001/
│       ├── control.stdout
│       ├── control.stderr
│       ├── attack.stdout
│       ├── attack.stderr
│       ├── state-before.txt
│       ├── state-after.txt
│       ├── http-trace.json
│       └── screenshot.png
├── findings/
│   └── RTF-001.md
├── repros/
│   └── RTE-001/
├── sandbox/
└── logs/
```

---

# 16. 推荐的核心配置

```yaml
preset_independence:
  required_input:
    - development_plan_paths


plan_resolution:
  discover_commits_from_git_log: true
  commit_match_confidence_min: 85
  require_hard_evidence: true
  allow_time_only_match: false
  allow_author_only_match: false
  allow_message_similarity_only: false

patch_reconstruction:
  generate_per_plan_patches: true
  generate_combined_patch: true
  patch_attribution_coverage_min: 90
  critical_claim_traceability_min: 100
  unresolved_critical_hunks_allowed: 0

hands_on_validation:
  required: true
  static_reasoning_only_is_sufficient: false
  experiment_required_for_finding: true
  control_case_required: true
  attack_case_required: true
  repeat_required: true
  state_inspection_required: true
  cleanup_verification_required: true
  raw_evidence_required: true

code_mutation:
  production_code: forbidden
  formal_tests: forbidden
  tracked_config: forbidden
  git_history: forbidden
  temporary_repro_under_ralph: allowed

finding_gate:
  confidence_min: 85
  evidence_coverage_min: 85
  verifiability_min: 90
  impact_certainty_min: 85
  aggregation: none
  qualification_requires_all_thresholds: true
  retry_on_any_below_threshold: true
  retry_on_missing_mandatory_evidence: true
  discard_only_after_retry_exhausted: true
  allow_rejected_findings_in_plan: false
  allow_low_confidence_accepted_risk: false

severity_overrides:
  P0:
    confidence_min: 90
    evidence_coverage_min: 90
    verifiability_min: 90
    impact_certainty_min: 90

  P1:
    confidence_min: 90
    evidence_coverage_min: 90
    verifiability_min: 90
    impact_certainty_min: 90


retry_policy:
  default_max_retries: 2
  p0_p1_max_retries: 3
  shared_budget_across_evidence_and_impact: true
  require_retry_delta: true
  blind_rerun_forbidden: true
  require_new_raw_evidence: true
  recompute_all_metrics_after_each_retry: true
  discard_only_after_retry_exhausted: true

confirm_before_code:
  execution_authorized: false
  production_edit_allowed: false
  formal_test_edit_allowed: false
  commit_allowed: false
  coding_agent_handoff_allowed: false
  human_confirmation_required: true
```

---

# 17. 最终完成条件

只有以下条件全部满足，Preset 才能输出 `PLAN_READY`：

- 所有输入开发计划已读取并计算哈希；
- 目标 branch、HEAD 和 tree 已锁定；
- 至少一个计划的 Commit 匹配达到阈值；
- 每个进入攻击范围的计划至少存在一项硬匹配证据；
- Patch 归属覆盖率至少为 90；
- 关键 Claim 追踪率为 100%；
- 每个正式 Finding 都有实际执行实验；
- 每个正式 Finding 都有控制组；
- 每个正式 Finding 都有攻击组；
- 每个正式 Finding 都有明确不变量；
- 每个正式 Finding 都满足重复次数要求；
- 每个正式 Finding 都保留原始证据；
- 每个正式 Finding 的四项指标全部达到阈值；
- 每个首次未达标但最终进入计划的 Finding 都有完整 Retry 历史和 Retry Delta；
- 每轮 Retry 都产生了新的原始证据并重新计算全部四项指标；
- 所有最终淘汰项均确认已耗尽允许的 Retry 预算；
- 每个正式 Finding 的影响边界已通过实际命令或测试确认；
- `PLAN.md` 不包含任何被拒绝实验；
- `PLAN.md` 不包含任何无证据修复要求；
- `PLAN.md` 不包含无关重构；
- `PLAN.md` 包含 Red → Green → Refactor → Regression；
- `PLAN.md` 包含全量回归和干净环境验证；
- 独立 Reviewer 给出 `PLAN_READY`；
- 当前 tracked tree 与锁定时一致；
- 没有修改生产代码；
- 没有修改正式测试；
- 没有创建 commit；
- 没有自动进入编码阶段。

---

# 18. 最终交付语义

Preset 最终可输出：

```text
DELIVERABLE_PATH: .ralph/red-team/PLAN.md
REPORT_PATH: .ralph/red-team/REPORT.md
QUESTIONS_PATH: .ralph/red-team/QUESTIONS.md
EXECUTION_AUTHORIZED: false
CONFIRMATION_REQUIRED: true
```

最终交付只代表：

> Red Team 已根据开发计划，从 Git 历史中重建真实实现，执行了真实攻击实验，过滤掉全部低证据和低确定性问题，并生成了一份可以交给 Coding Agent 执行的修复计划。

最终交付不代表：

- 已经修复代码；
- 已经修改测试；
- 已经提交 commit；
- 已经授权 Coding Agent 执行；
- 已经保证未经执行的计划绝对不会产生回归。

编码必须等待操作者显式确认。
