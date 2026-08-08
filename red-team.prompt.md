# Red Team Attack

请对以下已完成开发计划及其对应实现执行实验驱动的 Red Team 审查。

## 输入

plans:
  - path/to/completed-plan.md
  # - path/to/another-completed-plan.md

# 可选：指定目标分支；省略时使用当前分支。
# target_branch: main

# 可选：固定审查提交；省略时锁定启动时的 HEAD。
# target_commit: <full-commit-sha>

# 可选：显式 scope base SHA（40-char Git SHA）。
# 若提供，plan-resolver 直接从此 SHA 计算 patch，不读取 merge_batch boundary。
# 若省略，plan-resolver 从 target_lock artifact 推导 scope base。
# scope_base: abc1234def5678901234567890abcdef1234567890

# 可选：merge-batch boundary 文件路径。
# 若提供，plan-resolver 在混历史场景下可参考 boundary；direct-target 可省略。
# merge_boundary_path: .ralph/merge/boundary.json

# 可选：项目允许执行的验证命令。
# verification_commands:
#   - <build-command>
#   - <lint-command>
#   - <test-command>

allowed_test_environments:
  - local
  - mock
  - test

forbidden_external_targets:
  - production
  - real customer data
  - real payment or ordering systems
  - real notification recipients
  - shared infrastructure without explicit authorization
  - third-party services requiring write operations

## 任务

1. 从 Git 历史中独立定位每份计划对应的实现提交。
2. 重建目标 patch，验证计划条目与实际代码改动之间的归属关系。
3. 分析功能契约、状态转换、生命周期、配置组合、并发、幂等、恢复和兼容性方面的攻击面。
4. 为关键风险设计并实际执行控制组与攻击组实验。
5. 保存完整、可复现、机器可检查的原始证据。
6. 对确认的问题圈定最小影响边界。
7. 生成零回归修复计划，但不要修改、暂存或提交项目代码。

## 实验要求

每项正式实验必须包含：

- 明确的不变式和失败判据；
- 能正常通过的控制组；
- 只改变关键攻击条件的攻击组；
- 实际执行的命令、参数、输出和退出状态；
- 对文件、进程、数据库、网络响应或事件等真实状态的检查；
- 达到 preset 要求的重复次数；
- 原始证据和可复现步骤；
- 清理操作及清理结果；
- 实验前后 tracked tree 未变化的证明。

优先覆盖：

- 默认配置与显式配置；
- 功能开启、关闭和部分配置；
- 空输入、边界值、畸形输入和超大输入；
- 中断、超时、重启和恢复；
- 重复提交、重复消费、重放和幂等；
- 并发、乱序和部分失败；
- 旧数据、旧配置、升级和降级；
- CLI、API、schema、runtime、配置和文档之间的契约漂移；
- 表面成功但内部状态失败的路径；
- 绕过校验、权限或完成门禁的路径；
- 多份计划组合后的跨功能回归。

## 安全边界

- 只允许在授权的本地、mock、test 或明确授权的非生产环境执行实验。
- 禁止访问或修改生产环境、真实用户数据、真实支付、真实订单、真实通知接收者或未经授权的共享基础设施。
- 禁止执行破坏性、不可逆或扩大影响面的操作。
- 禁止修改生产代码、正式测试、tracked 配置或 Git 历史。
- 禁止执行 `git add`、`git commit`、`git merge`、`git rebase`、`git cherry-pick`、`git reset --hard` 或强制推送。
- 所有实验文件、fixture、证据、复现材料和报告只能写入 `.ralph/red-team/`。
- 每项实验结束后必须验证 tracked tree 未发生变化。
- 不得删除既有证据来掩盖失败。
- 不得终止或干扰 Ralph 及其父进程。

## 证据与结论标准

只有同时满足以下条件的问题才能成为正式 finding：

- 攻击实验已经实际执行；
- 控制组通过；
- 攻击组完成并触发明确的不变式失败；
- 检查了真实运行状态；
- 满足重复性要求；
- 原始证据完整；
- 实验清理成功；
- tracked tree 保持不变；
- 可以证明问题属于目标计划对应的实现 patch；
- 影响范围和最小安全修复边界清晰。

以下内容不得作为正式 finding：

- 仅有静态推测，没有实际实验；
- 控制组失败；
- 缺少真实状态检查或原始证据；
- 无法稳定复现；
- 无法归因到目标 patch；
- 影响边界不清楚；
- 需要扩大公共接口或进行无关重构的修复建议。

证据不足时，将问题写入待调查清单，不要猜测结论。

## 最终交付

生成以下文件：

- `.ralph/red-team/REPORT.md`
- `.ralph/red-team/PLAN.md`
- `.ralph/red-team/QUESTIONS.md`

`PLAN.md` 仅供人工审阅，不授权自动修改、暂存、提交或合并代码。
