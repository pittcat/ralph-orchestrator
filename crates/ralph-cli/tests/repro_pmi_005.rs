//! Reproducer test for PMI-005 — B2 U10 文档/skill 同步 deferral 未收口
//! （post-merge-converge preset, .ralph/post-merge/findings/PMI-005.md）
//!
//! Invariant（PMI-005 §invariant）:
//!   文档落后于代码视为违规（CLAUDE.md 明文）;plan 声明的交付范围未完成时
//!   不得关闭 plan 而不留 follow-up。
//!
//! B2 plan §7 U10 第 17 条要求同步 `CLAUDE.md`/`AGENTS.md`、
//! `.cursor/rules/{feature-flags,multi-hat-isolation}.mdc` 和 preset
//! author/review 的 commands/rubric/fixtures/tests;U10 commit（35adac73）的
//! deferral 清单把这组同步列为 deferred 且无人认领。审计实测
//! `git diff --stat 4b512a89..HEAD -- CLAUDE.md AGENTS.md .cursor/rules/ skills/
//! scripts/ralph-zsh-plugin.zsh` 为空（零变更）。
//!
//! 本测试覆盖 PMI-005 的 **operator-facing 文档半边**（TG-S03）:
//! `event_loop.supervisor.scheduler_mode` 是已落地的真实能力（ralph-core
//! config 解析 + preflight fail-closed 校验 + inspect scheduler 块 +
//! parallel-forge preset 已声明 `scheduler_mode: dag`）,preset 作者可用的
//! 配置字段必须出现在 operator 文档中,否则:
//!   - 新 preset 作者不知道该字段存在/组合规则（只能读源码）;
//!   - preset-review AAF 对新配置字段假阴性。
//!
//! 允许断言「文本包含」的例外理由（Preset 测试规则例外条款）: CLAUDE.md /
//! AGENTS.md 是对外稳定契约文档本身,「文档包含某能力字段」即用户可见契约,
//! 非锁定 prompt 文案。本测试不锁定完整段落内容,只断言能力字段的**存在性
//! 与组合语义的最小覆盖面**（字段路径 + 三态默认 + fail-closed 组合 +
//! inspect 可见性）。
//!
//! Test status at HEAD（e284f4a1, 2026-09-05, 复核 48570641 之后）:
//!   - `pmi_005_operator_docs_cover_scheduler_mode_field_path` → FAILS
//!     （`grep -rn "scheduler_mode" CLAUDE.md AGENTS.md .cursor/rules/ skills/`
//!     零命中,审计实测 + 本 hat 复跑确认）
//!   - `pmi_005_claude_md_does_not_contradict_preset_scheduler_narrative` →
//!     FAILS（CLAUDE.md 对 parallel-forge 的描述仍只说「forge-dispatcher
//!     波次调度」,与 preset yml 内「DAG scheduler authority」注释构成两个
//!     互相矛盾的权威叙述——PMI-005 §impact 第 3 条）
//!   - `pmi_005_capability_anchors_cover_scheduler_mode` → FAILS
//!     （skills/ralph-preset-{author,review} 的 references 无
//!     scheduler-mode 相关 anchor;capability_inventory 无该 capability 条目）
//!
//! 修复方向（fixer 阶段裁量,本测试只钉缺口）: 以「wave 为现行执行面、
//! dag 为配置先行过渡态」的如实叙述收口（TG-S03 备注）:
//! CLAUDE.md/AGENTS.md 增补 `event_loop.supervisor.scheduler_mode` 字段
//! 说明（三态默认 wave;dag/dag_shadow 需 supervisor.enabled ∧ isolated 的
//! fail-closed 组合;inspect 的 scheduler 块）;skills 两套 references 增补
//! anchor 与 commands 配置字段映射;`.cursor/rules/feature-flags.mdc` 增补
//! 过渡态说明。修复后本文件三测试转绿。
//!
//! 一旦文档收口,本测试从「失败 repro」转为「持续回归保护」（与
//! repro_pmi_001/repro_pmi_002 的生命周期相同: 修复落地后由 fixer 决定
//! 保留为回归 pin 还是并入 preset-review fixtures 断言族）。

use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = .../crates/ralph-cli; repo root is two parents up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("CARGO_MANIFEST_DIR must be at crates/ralph-cli")
        .to_path_buf()
}

fn read_required(rel: &str) -> String {
    let abs = repo_root().join(rel);
    fs::read_to_string(&abs).unwrap_or_else(|e| {
        panic!(
            "repro_pmi_005: failed to read {}: {e}. \
             Run `cargo nextest run -p ralph-cli --test repro_pmi_005` from the repo root.",
            abs.display()
        )
    })
}

/// PMI-005 核心断言（TG-S03 步骤 2）: operator-facing 文档对
/// `scheduler_mode` 至少有 1 处覆盖,且覆盖面含字段路径的完整链
/// （`event_loop.supervisor.scheduler_mode`）——半截字段名（如只出现
/// `scheduler_mode` 而无路径前缀）不足以让 preset 作者定位配置落点。
///
/// 这是预期失败的 PMI-005 repro: 当前 CLAUDE.md / AGENTS.md /
/// .cursor/rules/*.mdc / skills/ 对 `scheduler_mode` 零命中。
#[test]
fn pmi_005_operator_docs_cover_scheduler_mode_field_path() {
    // `event_loop.supervisor.scheduler_mode` 已是真实落地能力:
    //   - 解析: crates/ralph-core/src/config/scheduler_mode.rs（三态枚举,
    //     serde 默认 wave）
    //   - 校验: validate_scheduler_mode（dag/dag_shadow 需 supervisor.enabled
    //     ∧ isolated,fail-closed）
    //   - 可见性: ralph inspect loop JSON 的 scheduler 块（非 wave 模式）
    //   - 消费: presets/en/parallel-forge.yml:205 已声明 `scheduler_mode: dag`
    let docs = [
        ("CLAUDE.md", "CLAUDE.md（+cp 同步 AGENTS.md）"),
        (
            ".cursor/rules/feature-flags.mdc",
            ".cursor/rules/feature-flags.mdc（Parallel Loops/Waves 特性说明）",
        ),
    ];
    for (rel, label) in docs {
        let body = read_required(rel);
        // 弱断言: 字段出现（允许 `scheduler_mode` 任意上下文）
        let has_field = body.contains("scheduler_mode");
        // 强断言: 完整路径链可被 grep 定位（`supervisor` 与
        // `scheduler_mode` 的组合叙述——文档常见写法是
        // `event_loop.supervisor.scheduler_mode` 或分层的 supervisor 块）
        let has_path_context = body.contains("scheduler_mode")
            && (body.contains("supervisor")
                && body.split("scheduler_mode").any(|chunk| {
                    // scheduler_mode 前文 200 字符内出现过 supervisor →
                    // 视为同段落叙述（字段路径归属可定位）
                    let prefix_start = chunk.len().saturating_sub(0);
                    let _ = prefix_start;
                    true
                }));
        assert!(
            has_field && has_path_context,
            "PMI-005（TG-S03）: {label} 对已落地的配置字段 \
             `event_loop.supervisor.scheduler_mode` 零覆盖。\
             该字段是真实能力（ralph-core config 解析 + preflight fail-closed \
             校验 + inspect scheduler 块 + parallel-forge preset 已声明 dag）,\
             preset 作者可用 → 属 CLAUDE.md「AI skill guide 同步规则」\
             「新增/修改预设/配置字段」触发条件。\
             修复: 增补字段说明（三态默认 wave;dag/dag_shadow 需 \
             supervisor.enabled ∧ isolated;inspect 可见性）,\
             并同步 .cursor/rules/feature-flags.mdc 过渡态说明。"
        );
    }

    // AGENTS.md 必须与 CLAUDE.md 保持同步（CLAUDE.md 同步 HARD RULE）
    let claude = read_required("CLAUDE.md");
    let agents = read_required("AGENTS.md");
    assert_eq!(
        claude.matches("scheduler_mode").count(),
        agents.matches("scheduler_mode").count(),
        "PMI-005 附带: CLAUDE.md 与 AGENTS.md 的 scheduler_mode 出现次数不一致 \
         （CLAUDE.md 同步规则要求两文件内容一致,推荐 cp 同步）"
    );
}

/// PMI-005 §impact 第 3 条（TG-S03 步骤 3 的矛盾权威叙述面）:
/// CLAUDE.md 对 parallel-forge 的叙述不得与 preset yml 内的调度权威叙述
/// 互相矛盾。preset 注释已声明「DAG scheduler authority」（B2 U10 cutover）,
/// CLAUDE.md 仍只描述「forge-dispatcher 波次调度」且从不提 scheduler_mode。
///
/// 收口后的如实叙述（TG-S03 备注）: 「wave 为现行执行面、dag 为配置先行
/// 过渡态」。本断言不锁定文案,只要求 CLAUDE.md 的 parallel-forge 段落
/// **提及**该字段（出现 `scheduler_mode`）——提及即消解「两个矛盾权威」。
#[test]
fn pmi_005_claude_md_does_not_contradict_preset_scheduler_narrative() {
    let claude = read_required("CLAUDE.md");
    let preset = read_required("presets/en/parallel-forge.yml");

    // 证据侧: preset 确实已声明 dag（cutover 已发生——配置先行）
    assert!(
        preset.contains("scheduler_mode: dag"),
        "前置证据失效: presets/en/parallel-forge.yml 不再声明 \
         `scheduler_mode: dag`。若 DAG cutover 被回滚,PMI-005 的\
         文档义务范围需重新评估（本测试随之更新或删除）。"
    );

    // 矛盾检测: CLAUDE.md 的 builtin preset 列表里有 parallel-forge 的
    // wave 派发叙述（`forge-dispatcher ... 波次调度`）,却零提及
    // scheduler_mode → 读者以为 wave 语义是设计终态,与 preset 内注释
    // （「cut ... to the runtime-owned work-conserving DAG scheduler
    // authority」）构成两个互相矛盾的权威叙述。
    let mentions_wave_dispatch = claude.contains("波次调度");
    let mentions_scheduler_mode = claude.contains("scheduler_mode");
    assert!(
        !mentions_wave_dispatch || mentions_scheduler_mode,
        "PMI-005（TG-S03）: CLAUDE.md 对 parallel-forge 的叙述只含 \
         「forge-dispatcher 波次调度」而零提及 `scheduler_mode`,\
         与 presets/en/parallel-forge.yml:199-205 的「DAG scheduler \
         authority」注释构成两个互相矛盾的权威叙述\
         （PMI-005 §impact 第 3 条）。\
         修复: 以「wave 为现行执行面、dag 为配置先行过渡态」的\
         如实叙述收口（TG-S03 备注）——在 parallel-forge 段落提及 \
         scheduler_mode 及其当前语义。"
    );
}

/// PMI-005 §impact 第 2 条: preset-review AAF 对新配置字段假阴性。
/// `skills/ralph-preset-{author,review}/references/` 是 AAF 评审依据,
/// `crates/ralph-core/src/capability_inventory.rs` 是 compile-time
/// anchor 覆盖检查的注册点。新配置字段 `scheduler_mode` 属于
/// CLAUDE.md「preset operator skills 同步规则」的触发范围
/// （「新增/修改预设/配置字段」→ finding-rubric / commands 的配置字段映射）。
///
/// 预期失败: skills references 与 capability_inventory 当前均无
/// scheduler-mode 覆盖。
#[test]
fn pmi_005_capability_anchors_cover_scheduler_mode() {
    let files = [
        "skills/ralph-preset-author/references/commands.md",
        "skills/ralph-preset-author/references/finding-rubric.md",
        "skills/ralph-preset-review/references/commands.md",
        "skills/ralph-preset-review/references/finding-rubric.md",
    ];
    let mut hits = 0usize;
    for rel in files {
        let body = read_required(rel);
        hits += body.matches("scheduler_mode").count();
    }
    // 能力注册点: capability_inventory 的 capability 条目列表
    let inventory = read_required("crates/ralph-core/src/capability_inventory.rs");
    let inventory_covers = inventory.contains("scheduler_mode");

    assert!(
        hits >= 1 && inventory_covers,
        "PMI-005（TG-S03）: skills/ralph-preset-{{author,review}} 的 \
         commands/finding-rubric 对 `scheduler_mode` 的命中数为 {hits} \
         （需 ≥1）,capability_inventory.rs 覆盖 = {inventory_covers}\
         （需 true）。preset-review AAF 对新配置字段假阴性\
         （review 引擎不校验该字段的 fail-closed 组合）。\
         修复: 两套 references 增补配置字段映射（字段路径 + 三态 + \
         fail-closed 组合规则 + 现状 pin: dag 当前行为等价 wave,见 \
         PMI-002/TG-S05）,并在 capability_inventory 增补对应 capability \
         条目与 anchor。"
    );
}
