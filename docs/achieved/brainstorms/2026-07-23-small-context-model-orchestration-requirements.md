---
date: 2026-07-23
topic: small-context-model-orchestration
status: problem-frame
note: 本文只记录现状、需求与困境，不包含方案设计。
related:
  - docs/achieved/brainstorms/2026-06-25-profiles-for-preset-role-tuning-requirements.md
  - crates/ralph-core/data/ralph-tools.md
  - presets/en/ce-executor-pipeline.yml
---

# 小上下文模型下的编排：需求与困境（问题框）

## 1. 一句话

Ralph 编排在「大上下文、付费云模型」上已经能跑通复杂 pipeline；换成「小上下文、速度快的本地模型」时，同样的注入密度会把模型搞乱。目标是：**整条 pipeline 仍能在小模型上跑，且验收标准不降级**——只压缩输入，不放松门禁。本文只把现状、需求、困境写清楚，**不写方案**。

## 2. 目前项目是怎么样的（与本问题相关的事实）

### 2.1 编排定位

- Ralph 是薄协调层：preset 定义多 hat 拓扑，event loop 按 topic 调度，agent 通过 `ralph emit` / `ralph tools task` 等 runtime API 交接。
- 主路径 preset（如 `ce-executor-pipeline`）是多 hat、长链路：plan-review → executor → test-stabilizer → 多维 review → fix → alignment → reporter 等。
- `execution_mode: isolated` 下，每个 hat activation 是独立进程；看不到其它 hat 的 history，主要靠注入的 instructions + skills + 本轮 trigger/payload 工作。

### 2.2 每次 hat 激活时，模型实际吃到什么

典型 prompt 组装大致是：

1. **Hat `instructions`（及 `extra_instructions` / profile 追加片段）**  
   Preset YAML 里每个 hat 有一段（往往很长）职责说明与约束。主路径 preset 文件本身可达数千行量级（含全部 hats + event_policy 等）。
2. **Auto-inject skills**（`crates/ralph-core/data/ralph-tools*.md`）  
   - `ralph-tools.md` 在 tasks/memories 开启时常驻注入。  
   - `ralph-tools-tasks.md` / `ralph-tools-memories.md` 等按配置自动注入。  
   - 另有 emit / wave / precheck / recovery / opac / cmdref 等可按需 `skill load`。  
   - 仅 `ralph-tools*.md` 合计已约 **1600+ 行**；再叠加 hat instructions，单次 activation 的「规则文本」就很厚。
3. **本轮业务上下文**  
   plan、代码、trigger payload、任务列表等（本问题里运营方认为**主因不是这块**，见下）。
4. **Backend 把上述内容交给具体 agent CLI**  
   已支持多种 backend（Claude / Codex / Gemini / OpenCode / **Pi** / Trae / Cursor Agent 等）。Pi 通过 `pi -p --mode json --no-session`（及可选 `--provider` / `--model`）接入；大 prompt 时会走临时文件路径，但**那只解决「命令行长度」问题，不解决「模型上下文装不下 / 规则过密」问题**。

### 2.3 已有、且与「风格/注入」相邻的能力（事实，不是方案）

- **Profiles**：可对指定 preset/hat 追加 markdown 片段（风格叠加），不改编排拓扑。
- **Per-hat backend**：配置上可为不同 hat 指定不同 backend/args（例如不同模型）。
- **Skill 可见性**：skill 可按 hat / backend 过滤可见与自动注入。
- **Hard rule**：注入给 agent 的 skill 文档必须可执行、去计划化、禁止泄漏 runtime 内部细节——这会推高「写清楚」的篇幅，与「小上下文要短」存在张力。

### 2.4 运营方当前的使用画像（对话中已对齐）

| 维度 | 大上下文模型（已购/云） | 小上下文本地模型 |
|------|-------------------------|------------------|
| 上下文 | 很大（可达百万级量级） | 小；塞复杂规则易乱 |
| 速度 | 相对慢 | 相对快 |
| 与 Ralph | 复杂 pipeline + 厚注入仍可工作 | 同样注入密度易失控 |

## 3. 需求（已澄清部分）

以下条目来自 2026-07-23 讨论对齐，**尚未进入方案设计**。

### 3.1 目标范围

- **N1.** 希望**整条 pipeline**都能在小上下文模型上跑通，而不是只把小模型用在个别「简单 hat」。
- **N2.** 质量底线与现有大模型路径**同等可靠**：同样的验收门禁、同样的产出标准。允许「更短、更窄」的输入，**不允许「更糙」的标准**。
- **N3.** 现阶段**不优先改动编排机制本身**（event loop、hat 拓扑、emit 契约、isolated 语义等）。问题首先被定义为「注入内容相对模型能力过密」，而不是「调度逻辑错误」。

### 3.2 问题归因（运营方判断）

- **N4.** 小模型乱套的主因是：**注入的 skill / hat instructions 太长、规则太多**。  
  - 不是（当前判断下的主因）「单 hat 职责过重」。  
  - 不是（当前判断下的主因）「plan/代码一次塞太多」。

### 3.3 切换方式（讨论过但未拍板）

曾讨论过「精简注入如何切换」（手动 profile / 按 backend 档位自动 / 双份 instructions 等），**运营方要求先停在问题框，不展开方案**。此处仅记录：存在「同一套编排、不同注入密度」的诉求，切换手段未定。

### 3.4 开放问题：Pi Code Agent 插件路径

运营方当前使用 **Pi** 作为 agent 入口，并主动提出一个**尚未回答的问题**：

- **Q-Pi.** 这件事是否更适合（或部分适合）在 **Pi Code Agent 侧用插件 / extension** 完成，而不是（或不仅是）在 Ralph 编排侧消化？

需要后续单独澄清的子问题包括（仍非方案，只是问题清单）：

- Pi 插件能改的是「进模型前的上下文」还是「工具/会话行为」？与 Ralph 已注入的 instructions/skills 如何叠床架屋？
- 若压缩发生在 Pi 侧，Ralph 侧的 skill HARD RULE（agent 必须知道的命令/约束）如何保证不丢、不漂？
- 「同等可靠」的门禁仍在 Ralph（tests / emit / policy-check）——Pi 插件是否只负责瘦身，验收仍完全由 Ralph 背锅？
- 维护边界：preset 升级 vs Pi 插件升级，哪边是注入内容的事实源？

**本文不对 Q-Pi 做结论。**

## 4. 困境（为什么难）

### 4.1 核心张力

```text
大模型路径：厚 instructions + 厚 skills  → 行为可控、门禁可过
小模型路径：同样厚度                 → 上下文挤爆 / 注意力被规则淹没 → 行为乱套
目标：整条 pipeline + 同等可靠 + 尽量不改编排机制
```

同一套「为可靠而写厚」的注入，在小上下文上变成失败模式。

### 4.2 具体困境点

1. **可靠 vs 简短**  
   Skill/instructions 变厚，是为了减少协议违规、emit 猜字段、越权。变薄又可能回到「agent 不知道下一步该干什么」——与 N2「不降可靠性」直接冲突。

2. **一套内容服务两类模型**  
   Preset 与 `ralph-tools*.md` 当前基本是**单一事实源、单一密度**。没有「按模型上下文档位」的正式产品面；profiles 是「追加风格」，不是「替换为精简契约」的成熟叙事。

3. **编排想薄，注入却厚**  
   项目信条是「编排是薄协调层，agent 要聪明」。落地时为了让 isolated hat 可执行，又必须把规则写进 prompt。小模型场景下，这条路径的成本显性化了。

4. **责任边界不清（Ralph vs Pi）**  
   Ralph 负责：调度、注入、门禁、事件契约。  
   Pi 负责：具体会话、工具调用、本地/远程模型。  
   「瘦身」落在哪一侧、事实源在哪、失败时谁先修——目前没有共识（见 Q-Pi）。

5. **成功标准难量化（尚未定义）**  
   「同等可靠」在口头上清楚，但尚未落到可测指标，例如：同一 plan 在小模型档是否必须通过同一套 nextest / 同一套终态事件 / 同一套 review 维度。没有指标就无法判断「瘦了但没糙」。

6. **主因已点名，但未度量**  
   运营方判断主因是 skill/instructions 密度；尚未做「单次 activation 各段 token 占比」一类的基线测量。困境是：方向有了，证据粒度还不够支撑拍板。

## 5. 非目标（当前对话中的边界）

- 不在本文设计「怎么改」。
- 不默认要 fork 一整套新编排引擎。
- 不把问题表述成「本地模型能力差所以放弃 pipeline」——需求明确是要跑通且不降标准。
- 不把 Q-Pi 预先否决或采纳。

## 6. 对话中已锁定的决策摘要

| 项 | 结论 |
|----|------|
| 小模型角色 | 整条 pipeline 可跑（非仅简单 hat） |
| 质量 | 与大模型路径同等可靠；只压缩输入，不降门禁 |
| 主因（判断） | skill / hat instructions 过长、规则过密 |
| 编排机制 | 不优先改 event/hat 拓扑 |
| 方案 | **刻意未写**；含 Pi 插件可能性仅作开放问题 |

## 7. 下一步（仅流程，非方案）

1. 运营方审阅本文：需求 / 现状 / 困境是否写对。  
2. 单独澄清 **Q-Pi**（Ralph 侧瘦身 vs Pi 插件 vs 两者分工）是否进入后续设计范围。  
3. 若问题框确认无误，再另开设计讨论（仍可先做注入体积基线，再谈手段）。
