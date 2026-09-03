//! Plan 2026-09-01-2102 Unit 5: 修复 Memory Auto-Injection 的 Hat 可见性.
//!
//! `prompt_injection::inject_memories_and_tools_skill` 调用 `store.load()`
//! 时会拉取 **所有** memory（含其它 hat 的 Private 条目），而不是用
//! `store.load_visible(Some(hat_id.as_str()))` 过滤到「shared + 当前
//! hat 自己的 private」。这造成了跨 hat 的私有记忆泄漏——hat-A 在拉
//! 自己的 prompt 时能看到 hat-B 写的 Private 内容。
//!
//! 本文件固化修复行为（RED → GREEN）：
//! - `cross_hat_leak_red_to_green`：未修复时 hat-B 的私有条目会出现在
//!   hat-A 的 prefix；修复后必须不可见。
//! - `only_shared_no_private_leak`：只有 shared 记忆存在时，注入路径正常。
//! - `inject_disabled_no_op`：`memories.enabled=false` 时 prefix 不变。
//! - `budget_applied_after_visibility_filter`：可见到内容应当进入预算；
//!   不可见的 hat-B 内容 **不计入预算**，修复后必须从 prefix 中消失。
//!
//! 关键依赖：
//! - `MarkdownMemoryStore::load_visible(caller_hat_id)` 是「按 hat 过滤」
//!   的 SSOT；`Memory::is_visible_to(caller_hat_id)` 决定单条可见性。
//! - 预算（`memories.budget`）作用于「过滤后」集合，而非全量集合。

use super::*;
use crate::memory::{Memory, MemoryType, MemoryVisibility};
use crate::memory_store::MarkdownMemoryStore;

/// 共享 / 私有记忆的内容标记：用于在 prefix 内做字符串断言。
const SHARED_CONTENT: &str = "VISIBILITY_FIX_SHARED_TEXT";
const HAT_A_PRIVATE_CONTENT: &str = "VISIBILITY_FIX_HAT_A_PRIVATE_TEXT";
const HAT_B_PRIVATE_CONTENT: &str = "VISIBILITY_FIX_HAT_B_PRIVATE_TEXT";

/// 在临时 workspace 中写入 3 条记忆（共享 + hat-A 私有 + hat-B 私有），
/// 并以指定 hat 触发 `inject_memories_and_tools_skill`，返回最终 prefix。
fn inject_with_three_memories(
    hat_id: &str,
    budget: usize,
    enabled: bool,
    inject: InjectMode,
) -> String {
    let temp_root = tempfile::tempdir().expect("create tempdir for memory_visibility test");
    let workspace_root = temp_root.path().to_path_buf();

    let mut config = common::minimal_isolated_config(enabled, false);
    // minimal_isolated_config 默认 `inject: Auto`；测试可选关闭。
    config.memories.enabled = enabled;
    config.memories.inject = inject;
    config.memories.budget = budget;
    config.core.workspace_root = workspace_root.clone();

    let store = MarkdownMemoryStore::with_default_path(&workspace_root);
    store
        .append(&Memory::new_with_owner(
            MemoryType::Pattern,
            SHARED_CONTENT.to_string(),
            vec![],
            None,
            MemoryVisibility::Shared,
        ))
        .expect("append shared");
    store
        .append(&Memory::new_with_owner(
            MemoryType::Pattern,
            HAT_A_PRIVATE_CONTENT.to_string(),
            vec![],
            Some("hat-A".to_string()),
            MemoryVisibility::Private,
        ))
        .expect("append hat-A private");
    store
        .append(&Memory::new_with_owner(
            MemoryType::Pattern,
            HAT_B_PRIVATE_CONTENT.to_string(),
            vec![],
            Some("hat-B".to_string()),
            MemoryVisibility::Private,
        ))
        .expect("append hat-B private");

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U5 memory visibility leak test");
    let mut prefix = String::new();
    event_loop.inject_memories_and_tools_skill(&mut prefix, &HatId::new(hat_id));
    // temp_root 在这里 Drop,无需手工清理。
    prefix
}

/// Red → Green：未修复时（`store.load()`）hat-A 的 prompt 会看到
/// hat-B 的私有记忆；修复后（`store.load_visible(Some("hat-A"))`）则不会。
#[test]
fn cross_hat_leak_red_to_green() {
    let prefix = inject_with_three_memories("hat-A", 0, true, InjectMode::Auto);
    assert!(
        prefix.contains(SHARED_CONTENT),
        "shared memory must appear in hat-A's prompt; prefix={prefix}"
    );
    assert!(
        prefix.contains(HAT_A_PRIVATE_CONTENT),
        "hat-A's own private memory must appear in hat-A's prompt; prefix={prefix}"
    );
    assert!(
        !prefix.contains(HAT_B_PRIVATE_CONTENT),
        "hat-B's private memory MUST NOT leak into hat-A's prompt (auto-injection hat visibility); \
         prefix={prefix}"
    );
}

/// 仅含共享条目时，注入路径必须按原契约工作（无私有内容、无 panic）。
#[test]
fn only_shared_no_private_leak() {
    let temp_root = tempfile::tempdir().expect("create tempdir for only-shared test");
    let workspace_root = temp_root.path().to_path_buf();

    let mut config = common::minimal_isolated_config(true, false);
    config.core.workspace_root = workspace_root.clone();

    let store = MarkdownMemoryStore::with_default_path(&workspace_root);
    store
        .append(&Memory::new_with_owner(
            MemoryType::Pattern,
            "VISIBILITY_FIX_LONE_SHARED_TEXT".to_string(),
            vec![],
            None,
            MemoryVisibility::Shared,
        ))
        .expect("append shared");

    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U5 only-shared test");
    let mut prefix = String::new();
    event_loop.inject_memories_and_tools_skill(&mut prefix, &HatId::new("hat-A"));

    assert!(
        prefix.contains("VISIBILITY_FIX_LONE_SHARED_TEXT"),
        "shared-only workspace must inject the lone shared memory; prefix={prefix}"
    );
    assert!(
        !prefix.contains(HAT_B_PRIVATE_CONTENT),
        "no hat-B private content in shared-only workspace; prefix={prefix}"
    );
}

/// `memories.enabled=false`（或 `inject==None`）时,记忆数据路径必须
/// 完全跳过——prefix 里不应出现任何 store 里的记忆内容（共享/私有都
/// 不行）。函数本身还会注入 ralph-tools skill,所以这里断言的是
/// 「记忆内容字符串不出现」,而不是整个 prefix 为空。
#[test]
fn inject_disabled_no_op() {
    let prefix_disabled = inject_with_three_memories("hat-A", 0, false, InjectMode::Auto);
    assert!(
        !prefix_disabled.contains(SHARED_CONTENT),
        "memories.enabled=false must skip memory data injection (no shared content); \
         got prefix={prefix_disabled}"
    );
    assert!(
        !prefix_disabled.contains(HAT_A_PRIVATE_CONTENT),
        "memories.enabled=false must skip memory data injection (no hat-A private content); \
         got prefix={prefix_disabled}"
    );
    assert!(
        !prefix_disabled.contains(HAT_B_PRIVATE_CONTENT),
        "memories.enabled=false must skip memory data injection (no hat-B private content); \
         got prefix={prefix_disabled}"
    );

    let prefix_none = inject_with_three_memories("hat-A", 0, true, InjectMode::None);
    assert!(
        !prefix_none.contains(SHARED_CONTENT),
        "InjectMode::None must skip memory data injection; got prefix={prefix_none}"
    );
    assert!(
        !prefix_none.contains(HAT_B_PRIVATE_CONTENT),
        "InjectMode::None must skip memory data injection (no leak); got prefix={prefix_none}"
    );
}

/// 预算在可见性过滤 **之后** 生效：
/// - 可见到 (shared + hat-A private) 集合需要完整渲染（不超出预算）；
/// - 不可见的 hat-B private 内容 **既不在 prefix 中，也不挤占预算**。
///
/// 选择 budget 使得「可见集合 ≈ 350 字符」能完整保留（≈ 87 tokens × 4 = 348）；
/// 「可见集合 + hat-B private ≈ 450 字符」会触发 truncate。当 fix 前用 `load()`
/// 时，prefix 会因 truncate 把 hat-A 私有内容裁掉并出现 hat-B 的尾巴；
/// fix 后用 `load_visible(Some("hat-A"))`，预算按过滤后集合应用，hat-A
/// 私有保留，hat-B 私有彻底不可见。
#[test]
fn budget_applied_after_visibility_filter() {
    let prefix = inject_with_three_memories("hat-A", 100, true, InjectMode::Auto);

    // 核心断言：可见性过滤发生在预算计算之前。
    assert!(
        !prefix.contains(HAT_B_PRIVATE_CONTENT),
        "hat-B's private memory MUST NOT appear in hat-A's prompt after visibility filter, \
         regardless of budget; prefix={prefix}"
    );
    assert!(
        prefix.contains(HAT_A_PRIVATE_CONTENT),
        "hat-A's private memory MUST survive the budget after the visibility filter is applied \
         first; prefix={prefix}"
    );
    assert!(
        prefix.contains(SHARED_CONTENT),
        "shared memory MUST survive the budget; prefix={prefix}"
    );

    // 反向断言：prefix 不能比「可见集合完整渲染」长得多（说明预算没
    // 按全量集合算）——如果按全量算，prefix 末尾会有 truncate 提示并
    // 丢失 hat-A 私有尾部。
    assert!(
        !prefix.contains("<!-- truncated:"),
        "with the fix, the visible (filtered) set fits in budget and no truncation notice \
         should be appended; prefix={prefix}"
    );
}
