---
title: Builtin artifact templates must materialize from the binary
date: 2026-07-27
category: developer-experience
module: ralph-cli
problem_type: binary-deploy-missing-source-templates
tags: [parallel-forge, preset, materialize-artifacts, binary-install, tdd, bdd]
---

# Builtin artifact 模板必须从二进制落盘

## 问题

`parallel-forge` 要求 planner / executor / reporter **先复制固定模板再填写**（含 Spec-First、BDD Scenario、TDD Red→Green→Refactor 章节）。模板源文件在开发仓库的 `presets/templates/parallel-forge/`。

若 hat instructions 直接写 `cp presets/templates/...`，则 **只安装 `ralph` 二进制、没有源码树的部署机** 上第一步就会失败。

## 解法

与 builtin preset YAML 相同：编译期嵌入 + 运行时写出。

1. `crates/ralph-cli/build.rs` 复制 `presets/templates/parallel-forge/*` → `$OUT_DIR/artifact-templates/`
2. `builtin_artifact_templates.rs` 用 `include_str!` 嵌入
3. CLI：`ralph preset materialize-artifacts parallel-forge --plan-key <key>`
4. 默认输出：`.ralph/forge/<plan-key>/templates/`
5. Hat 再 `cp` 到业务 artifact 路径并填写

## 与 `ralph preset new` 的区别

| 命令 | 产出 |
|---|---|
| `preset new` | 作者用的 **preset YAML 脚手架** |
| `preset materialize-artifacts` | 运行时 **fill-in 文档/YAML 模板** |

## 验证闭环

```bash
cargo nextest run -p ralph-cli --bin ralph -- materialize
cargo nextest run -p ralph-cli --test integration_preset_materialize_artifacts
ralph preset materialize-artifacts parallel-forge --plan-key demo
test -f .ralph/forge/demo/templates/development-plan.template.md
```

单元测试断言嵌入内容仍含 BDD / TDD 标记；集成测试覆盖 happy path、`--dest`、`builtin:` 前缀、非法 plan-key、幂等覆盖、help 文案。
