//! Tests for the `append_runtime_config_block` helper that exposes the
//! runtime-resolved `event_loop.*` values (e.g. `max_residuals`) to the
//! hat prompt via a `## RUNTIME CONFIG` block.

#[test]
fn append_runtime_config_block_includes_max_residuals() {
    use crate::event_loop::append_runtime_config_block;
    let prompt = append_runtime_config_block("base".to_string(), 8);
    assert!(
        prompt.contains("## RUNTIME CONFIG"),
        "missing ## RUNTIME CONFIG block"
    );
    assert!(
        prompt.contains("max_residuals: 8"),
        "max_residuals value not visible to hat"
    );
}

#[test]
fn append_runtime_config_block_reflects_custom_value() {
    use crate::event_loop::append_runtime_config_block;
    let prompt = append_runtime_config_block("base".to_string(), 5);
    assert!(prompt.contains("max_residuals: 5"));
}

#[test]
fn append_runtime_config_block_preserves_existing_prompt() {
    use crate::event_loop::append_runtime_config_block;
    let prompt = append_runtime_config_block("hello world".to_string(), 8);
    assert!(prompt.starts_with("hello world"));
    assert!(prompt.contains("max_residuals: 8"));
}
