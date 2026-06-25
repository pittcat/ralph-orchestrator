
Implement dev plan: docs/plans/2026-06-25-001-feat-ce-executor-serial-5dim-coordinator-amendments-plan.md


### Note

注意每个角色要各司其职,不能越权

### Worktree 复用检查（先确认再创建）

执行本 plan 前**先跑** `git worktree list`,确认是否已有与本 plan 同名的 worktree(命名约定:`<plan-basename>-<adjective-noun>`,例如 `2026-06-25-001-feat-ce-executor-serial-5dim-coordinator-amendments-plan-lucky-reed`)。**已有则直接复用,不要盲目新开**;只有确认不存在匹配 worktree 时,才按上述命名约定创建新 worktree。
