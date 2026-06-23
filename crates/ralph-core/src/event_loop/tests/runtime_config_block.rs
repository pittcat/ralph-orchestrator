//! Tests for the `append_runtime_config_block` helper that exposes the
//! runtime-resolved `event_loop.*` values (e.g. `max_fix_rounds`) to the
//! hat prompt via a `## RUNTIME CONFIG` block.

#[test]
fn append_runtime_config_block_includes_max_fix_rounds() {
    use crate::event_loop::append_runtime_config_block;
    let prompt = append_runtime_config_block("base".to_string(), 1);
    assert!(
        prompt.contains("## RUNTIME CONFIG"),
        "missing ## RUNTIME CONFIG block"
    );
    assert!(
        prompt.contains("max_fix_rounds: 1"),
        "max_fix_rounds value not visible to hat"
    );
}

#[test]
fn append_runtime_config_block_reflects_custom_value() {
    use crate::event_loop::append_runtime_config_block;
    let prompt = append_runtime_config_block("base".to_string(), 7);
    assert!(prompt.contains("max_fix_rounds: 7"));
}

#[test]
fn append_runtime_config_block_preserves_existing_prompt() {
    use crate::event_loop::append_runtime_config_block;
    let prompt = append_runtime_config_block("hello world".to_string(), 3);
    assert!(prompt.starts_with("hello world"));
    assert!(prompt.contains("max_fix_rounds: 3"));
}