# Spec-First Development Plan

> **模板来源**：parallel-dev-preset.md §10.14。Planner 必须复制本文件到
> `.ralph/forge/<plan-key>/development-plan.md` 并**逐节填满**；不得删减章节标题。
> 无法从仓库确认的内容标记 `executor_must_confirm`，不得伪造。

---

## 1. 功能目标

### 业务目标

<!-- 用户要解决什么问题；价值；谁会受影响 -->

### 本次范围

<!-- 本次明确包含的外部可观察能力（列表） -->

### 非目标

<!-- 明确不在本次处理的内容 -->

### 已知约束

<!-- 兼容、性能、安全、迁移、平台、Feature Flag 等 -->

### 已确认事实

<!-- 已从需求或仓库确认 -->

### 假设

<!-- 合理假设；不得写成事实 -->

### 待确认项

<!-- Executor 执行前必须确认；或 executor_must_confirm -->

---

## 2. 仓库现状与可复用能力

| 类别 | 发现 |
|---|---|
| 相关目录和模块 | |
| 现有入口 | |
| 现有接口 | |
| 现有测试 | |
| 构建和测试命令 | |
| 可复用 Fake / Stub / Fixture | |
| 兼容性约束 | |
| 无法确认的信息 | executor_must_confirm: ... |

---

## 3. BDD 行为规格

<!-- 每个 Feature 使用 Cucumber 风格；每个 Scenario 只描述一个主要外部行为 -->

```gherkin
Feature: <功能名称>
  为了 <业务价值>
  作为 <角色或调用方>
  我希望 <目标能力>

  Scenario: <场景名称>
    Given <初始状态>
    And <其他前置条件>
    When <用户动作或系统事件>
    Then <外部可观察结果>
    And <附加结果>
```

<!-- 复制上述块为每个 Feature；按需覆盖正常/边界/失败/并发等风险场景 -->

---

## 4. 验收与测试策略

| Scenario | 验收条件 | 推荐测试层级 | 测试入口 | 依赖模式 | 是否需要 E2E |
|---|---|---|---|---|---|
| S01 | | unit / component / integration / contract / cli / e2e | | fake / stub / real | 是 / 否 |

---

## 5. Outside-In 能力分解

| Scenario | 外部入口 | 应用层能力 | 领域规则 | 稳定端口 | 基础设施能力 |
|---|---|---|---|---|---|
| S01 | | | | | |

---

## 6. 需求—测试追踪矩阵

| 需求 | Scenario | 验收测试 | 单元测试 | 集成测试 | 契约测试 | E2E |
|---|---|---|---|---|---|---|
| R01 | S01 | | | | | |

---

## 7. Unit 依赖 DAG

### ASCII DAG

```text
U01 公共基础
 ├── U02 ...
 └── U03 ...
```

### 依赖表

| Unit | Depends On | Wave | 执行模式 | 集成顺序 | 并发或串行理由 |
|---|---|---:|---|---:|---|
| U01 | | 1 | parallel | 1 | |

---

## 8. 并发边界与共享资源

| Unit | Allowed Paths | Forbidden Paths | Shared Contracts | Owned Resources |
|---|---|---|---|---|
| U01 | | | | |

### 共享资源所有权（每个资源唯一 Owner）

| 资源 | Owner Unit | Consumers | 规则 |
|---|---|---|---|
| | | | |

---

## 9. 开发单元

<!-- 每个 Unit 一节；字段须与 presets/templates/parallel-forge/unit.template.yml 一致 -->

### Unit U01：<标题>

（从 development-plan 人类可读摘要；机器可读 SSOT 在 execution-plan.yml 的 `units[]`）

- Unit 类型：
- Unit 目标：
- 对应 Scenario：
- 依赖关系：
- 执行安排（wave / parallel_with / integration_order）：
- 允许 / 禁止路径：
- 验收测试与 RED 预期：
- TDD 最小行为拆分（Red → Green → Refactor）：
- 明确不在本 Unit 实现的内容：
- 回归范围：
- 完成标准：
- Commit 建议：
- 风险与注意事项：

<!-- 为每个 Unit 复制 ### Unit Uxx 小节 -->

---

## 10. Execution Waves

> **编号规则**：本节标题「Wave N」与 `execution-plan.yml` 的
> `execution_wave: N` **同号**，且 **N 从 1 起**（无依赖 Unit = Wave 1）。
> 禁止写「Wave 0」——`execution_wave` 不允许 0。

```text
Wave 1：Foundation
  U01

Wave 2：Parallel Behavior Development
  U02
  U03

Wave 3：Integration
  U05

Wave 4：System Verification
  U06
```

### Wave 说明

| Wave | 启动条件 | 可并发 Unit | 等待依赖 | 完成条件 | 下一 Wave 入口 |
|---|---|---|---|---|---|
| 1 | | | | | |

---

## 11. 顺序集成计划

| 集成顺序 | Unit | 前置条件 | Rebase 基线 | 合入后验证 | 失败处理 |
|---:|---|---|---|---|---|
| 1 | U01 | | | | |

---

## 12. 最终质量门禁

- [ ] 所有计划内 Scenario 通过
- [ ] 所有验收测试通过
- [ ] 所有单元测试通过
- [ ] 必要集成 / 契约测试通过
- [ ] 关键 E2E 通过
- [ ] Lint / Typecheck / Build 通过
- [ ] 项目 CI 等价检查通过
- [ ] 无新增失败或跳过测试；无 `.only`
- [ ] 无无解释 Snapshot / Golden 更新
- [ ] 最终 Git 历史：每 Unit 一个原子 Commit、线性
- [ ] 工作目录干净；未验证内容与剩余风险已记录

---

## 13. 最终交付物

- [ ] 生产代码
- [ ] 自动化测试
- [ ] 必要 Fixture / Fake / Stub
- [ ] 必要配置和迁移
- [ ] 必要文档
- [ ] Unit Completion Report（每 Unit）
- [ ] 测试结果记录
- [ ] 最终 Commit 列表
- [ ] 剩余风险说明
