Implement dev plan:docs/plans/2026-07-04-001-feat-opac-isolated-agent-discipline-plan.md,必须要确保每个单元都要完成,从git log，以及开发计划里面确认进度

### Note

注意每个角色要各司其职,不能越权

### Worktree 复用检查（先检查 + 模糊匹配,严禁盲目创建）

执行本 plan 前**必须先跑** `git worktree list`,对结果做**模糊匹配**——只要 worktree 路径或分支名中包含本 plan 的 basename(如 `2026-06-25-002-feat-profiles-for-preset-role-tuning-plan`)就算匹配(命名约定:`<plan-basename>-<adjective-noun>`,允许后半段随机词不同)。

- **有匹配** → 直接 `cd` 进去复用,**绝对不要创建**
- **无匹配** → 才按上述命名约定(`<plan-basename>-<adjective-noun>`)创建新 worktree

**严禁**:看到 `git worktree list` 列表里有名字相近的就跳过检查,或凭印象判断"应该没有"直接 `EnterWorktree`/`git worktree add` 创建——**检查必须真跑、模糊匹配必须真做**。
