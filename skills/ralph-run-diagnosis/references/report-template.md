# 诊断报告模板

落盘：`docs/report/YYYY-MM-DD-<preset-basename>-<loop_id>-diagnosis.md`

参考样板：`docs/report/2026-07-04-ce-executor-serial-primary-20260704-115242-diagnosis.md`

---

```markdown
---
title: <preset> Loop `<loop_id>` 运行链路诊断报告
date: YYYY-MM-DD
type: diagnosis
loop_id: <loop_id>
preset: builtin:<name> 或 <preset_file>
run_dir: <repo-relative run_dir>
status: <一句话健康度>
---

# <preset> Loop `<loop_id>` 运行链路诊断报告

> **生成时间**: ...
> **诊断对象**: `<run_dir>/.ralph/`（loop_id=..., 启动 → 终止）
> **对照 preset**: `<preset_file>` + `presets/schemas/<name>.yml`
> **执行方式**: 4 sub-agent 并行（流程还原 / 历史 / 对账 / 归因）→ 汇总
> **Diagnostics 模式**: FULL | MINIMAL | LOGS_ONLY | DISABLED
> **报告仓库**: `ralph-orchestrator` 主仓（非 run_dir）
> **Tier C 根**: （从 preset+schema 解析）

---

## 0. 产物盘点（Phase 0 必附）

| Tier | 路径 | 存在 | 行数 | 备注 |
|------|------|------|------|------|
| S | events（current-events 解析） | | | |
| … | | | | |

**盲区 / OPAC 置信度上限**：（如 LOGS_ONLY → ≤50）

---

## 1. 结论摘要

### 1.1 健康度

- **判定**: （健康 / 部分偏离 / 死锁 / 假闭环 silent-success / …）
- **P0 / P1 / P2 数量**:
- **历史复发**: 是/否 — 第 N 次 — 引用 `docs/report/...`

### 1.2 强制四问（debug.md）

| # | 问题 | 答案 | 一句证据 |
|---|------|------|----------|
| Q1 | 整体执行与 OPAC 是否合规？ | ✅/❌/⚠️ | ... |
| Q2 | 基座机制是否正常生效？ | ✅/❌/⚠️ | ... |
| Q3 | 编排是否合理、正常运行？ | ✅/❌/⚠️ | ... |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | ... | ... |

### 1.3 根因一句话

...

---

## 2. 执行链路对比图

（粘贴 Agent A 输出：拓扑表 + 时间轴 + mermaid）

---

## 3. 历史问题上下文

（粘贴 Agent B：关联度表 + 复发对照）

---

## 4. 证据清单

| ID | 描述 | 文件:行号 / event# | 严重度 |
|----|------|-------------------|--------|
| DEV-001 | ... | ... | P0 |

### 4.1 OPAC 逐 hat 审计表

（Agent C）

---

## 5. 问题归因表

| 优先级 | 问题 | 根因分类 | 证据 | 历史关联 |
|--------|------|----------|------|----------|
| P0 | ... | mechanism / preset / agent / compound | DEV-00x | 高/中/低 |

---

## 6. 修复建议

### 6.1 短期（operator workaround）

### 6.2 中期（preset / schema / instructions）

### 6.3 长期（机制 / 底座）

每条：目标文件或机制 | 具体改动 | 预期效果
```

---

## 质量门槛

- §1 四问 **不可省略**。
- 每条 P0 至少一条 DEV 证据 ID。
- `compound` 归因须写贡献比例或主次。
- 路径一律 **repo-relative**。
