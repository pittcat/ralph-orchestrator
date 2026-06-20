//! 2026-06-18-002 plan: Isolated 模式 hat→hat roadmap handoff.
//!
//! 在 `execution_mode: isolated` 下提供一个**可选**的机制,让上游
//! hat 通过 `ralph tools handoff prepare` 获得确定性
//! `handoff_path` + 五段式 skeleton,填好后随宏观边 emit 一同
//! 提交;机制层校验结构、路径、R15 topic 约束,下游
//! `build_prompt` 注入 `## HAT HANDOFF` 块供 agent 快速导航。
//!
//! 关键设计:
//!
//! - **默认关闭**(`HatHandoffConfig::default().enabled == false`),
//!   仅当 `execution_mode == isolated && enabled == true` 时
//!   生效。
//! - **宏观边** = 唯一消费者 topic ∧ 非自环 ∧ 非豁免。
//! - **fail-closed** 注入:文件缺失/不可读 → 不注入 + diagnostic。
//! - **seq 分配**:`prepare` 返回的 `handoff_path` 由
//!   `LoopState.hat_handoff_seq + 1` 决定;gate accept 后递增。
//! - **拒收清理**:gate 拒收时调 `HandoffTracker::cancel_pending`
//!   抹掉已在 policy accept 时记录的 phantom pending。
//!
//! Plan reference: `docs/plans/2026-06-18-002-feat-isolated-hat-handoff-plan.md`
//! (R1-R19, KTD-1..18).

pub mod allocator;
pub mod emit_instructions;
pub mod gate;
pub mod inject;
pub mod macro_edges;
pub mod payload;
pub mod publishes_check;
pub mod validator;

use serde::{Deserialize, Serialize};

/// 默认注入块最大字节数(KTD-7)。截断时必须保留完整 `## next`。
pub const DEFAULT_HAT_HANDOFF_MAX_BYTES: usize = 2048;

/// handoff 文件相对 repo 根的存放目录。
pub const HAT_HANDOFF_DIR: &str = ".ralph/agent/hat-handoff";

/// 默认豁免 topic 列表(KTD-2)。`review.dimension.*` 是微观边,
/// 不需要 roadmap handoff。
pub const DEFAULT_EXEMPT_TOPICS: &[&str] = &[
    "review.dimension.ready",
    "review.dimension.done",
    "review.dimension.failed",
];

/// 2026-06-18-002 plan U1: hat→hat roadmap handoff 配置。
///
/// 挂在 `EventLoopConfig.hat_handoff` 字段上,默认 `enabled: false`。
/// 仅当 `execution_mode == isolated && enabled == true` 时,
/// 宏观边校验/注入才会生效。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HatHandoffConfig {
    /// Master switch。默认 `false`;开启 + isolated 才生效。
    #[serde(default)]
    pub enabled: bool,

    /// 额外强制 handoff 的 topic 列表(绕过 wildcard 隐藏)。
    /// 配合 `exempt_topics` 使用,可精细控制宏观边集合。
    #[serde(default)]
    pub macro_topics: Vec<String>,

    /// 额外豁免的 topic(从默认豁免集合基础上叠加)。
    /// 这两个集合都不影响 `DEFAULT_EXEMPT_TOPICS` 的内置微观边。
    #[serde(default)]
    pub exempt_topics: Vec<String>,

    /// 注入块最大字节数。超长截断但**完整保留 `## next`**(KTD-7)。
    #[serde(default = "default_hat_handoff_max_bytes")]
    pub max_bytes: usize,

    /// Artifact 校验规则 (R21, plan 2026-06-20-001)。
    /// `required_sections` 期望 `## next` 之前的标题段数;
    /// `require_next_marker` 强制存在 `## next`。
    /// 当 SSOT 协议节 (`hat_handoff.artifact:`) 通过 `build.rs`
    /// merge 进 `event_loop.hat_handoff`,注入器 (`inject.rs`)
    /// 在 fail-closed 路径上会调用 `ArtifactRule::validate`,
    /// 不通过则阻断 handoff 注入并记录 diagnostic。
    #[serde(default)]
    pub artifact: ArtifactRule,

    /// Linter 子节 (R8, R22)。当 `linter.auto_prepare_on_macro_edge`
    /// 为 `true`,lint 阶段会在 payload 缺 `handoff_path` 时
    /// **同步** 调用 prepare (写 artifact + 注入 path),而不是
    /// 仅提示 agent。Operator 通过 schemas 文件 (`presets/schemas/
    /// ce-executor-serial.yml`) 的 `hat_handoff.linter` 节启用。
    #[serde(default)]
    pub linter: LinterRule,
}

/// Artifact shape requirements (R21). Defaults to a permissive
/// shape — `required_sections = 0` and `require_next_marker =
/// false` — so existing presets do not regress.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactRule {
    /// Required number of `## section` headings before the
    /// `## next` marker. Plan SSOT pins 5 for `ce-executor-serial`.
    #[serde(default)]
    pub required_sections: u32,

    /// Whether `## next` marker MUST be present.
    #[serde(default)]
    pub require_next_marker: bool,
}

impl ArtifactRule {
    /// Validate a handoff artifact's markdown body. Returns the
    /// first failing rule as `Err(String)` so the engine can
    /// surface it through LintResumeHint (U4) or runtime gate (U2).
    pub fn validate(&self, content: &str) -> Result<(), String> {
        if self.required_sections > 0 {
            let mut headings = 0u32;
            for line in content.lines() {
                if line.starts_with("## ") {
                    if line.starts_with("## next") {
                        break;
                    }
                    headings += 1;
                }
            }
            if headings < self.required_sections {
                return Err(format!(
                    "hat_handoff.artifact: expected at least {} `## section` headings before `## next`, found {}",
                    self.required_sections, headings
                ));
            }
        }
        if self.require_next_marker && !content.contains("## next") {
            return Err(
                "hat_handoff.artifact: required `## next` marker not found in artifact body"
                    .to_string(),
            );
        }
        Ok(())
    }
}

/// Linter-side configuration (R8, R22).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinterRule {
    /// When `true`, the linter synchronously prepares a handoff
    /// artifact on macro edges if the payload lacks
    /// `handoff_path`. Defaults to `false` to preserve legacy
    /// behaviour — `ce-executor-serial` opts in via SSOT.
    #[serde(default)]
    pub auto_prepare_on_macro_edge: bool,
}

fn default_hat_handoff_max_bytes() -> usize {
    DEFAULT_HAT_HANDOFF_MAX_BYTES
}

impl Default for HatHandoffConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            macro_topics: Vec::new(),
            exempt_topics: Vec::new(),
            max_bytes: default_hat_handoff_max_bytes(),
            artifact: ArtifactRule::default(),
            linter: LinterRule::default(),
        }
    }
}

impl HatHandoffConfig {
    /// 当前 topic 是否在任何豁免集合内。
    pub fn is_exempt(&self, topic: &str) -> bool {
        DEFAULT_EXEMPT_TOPICS.contains(&topic) || self.exempt_topics.iter().any(|t| t == topic)
    }

    /// 当前 topic 是否在 `macro_topics` 显式列表内(绕过 wildcard)。
    pub fn is_explicit_macro(&self, topic: &str) -> bool {
        self.macro_topics.iter().any(|t| t == topic)
    }
}
