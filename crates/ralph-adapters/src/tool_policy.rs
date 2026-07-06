//! Hat-level tool restrictions bridged into CLI backend spawn args.
//!
//! Today only Claude's `--disallowedTools` flag is merged; other backends
//! either lack per-tool deny flags or use blanket `--yolo` / `--allow-all-tools`.

use crate::CliBackend;
use std::collections::BTreeSet;

const DISALLOWED_TOOLS_PREFIX: &str = "--disallowedTools=";

/// Merge `hat.disallowed_tools` into a Claude backend's `--disallowedTools` arg.
///
/// No-op for non-`claude` commands or empty restriction lists. Existing global
/// disallows (e.g. `TodoWrite`) are preserved; hat tools are unioned in.
pub fn apply_hat_tool_policy(backend: &mut CliBackend, disallowed_tools: &[String]) {
    if backend.command != "claude" || disallowed_tools.is_empty() {
        return;
    }
    merge_claude_disallowed_tools(&mut backend.args, disallowed_tools);
}

fn merge_claude_disallowed_tools(args: &mut Vec<String>, extra: &[String]) {
    let mut merged = BTreeSet::new();
    let mut existing_idx = None;

    for (i, arg) in args.iter().enumerate() {
        if let Some(rest) = arg.strip_prefix(DISALLOWED_TOOLS_PREFIX) {
            existing_idx = Some(i);
            for tool in rest.split(',') {
                let tool = tool.trim();
                if !tool.is_empty() {
                    merged.insert(tool.to_string());
                }
            }
            break;
        }
    }

    for tool in extra {
        let tool = tool.trim();
        if !tool.is_empty() {
            merged.insert(tool.to_string());
        }
    }

    if merged.is_empty() {
        return;
    }

    let new_arg = format!(
        "{}{}",
        DISALLOWED_TOOLS_PREFIX,
        merged.into_iter().collect::<Vec<_>>().join(",")
    );

    if let Some(idx) = existing_idx {
        args[idx] = new_arg;
    } else {
        args.push(new_arg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CliBackend;

    #[test]
    fn merges_hat_edit_into_existing_claude_disallowed_tools() {
        let mut backend = CliBackend::claude();
        apply_hat_tool_policy(&mut backend, &["Edit".to_string()]);

        let flag = backend
            .args
            .iter()
            .find(|a| a.starts_with(DISALLOWED_TOOLS_PREFIX))
            .expect("must have --disallowedTools");
        assert!(flag.contains("Edit"), "flag={flag}");
        assert!(flag.contains("TodoWrite"), "flag={flag}");
    }

    #[test]
    fn non_claude_backend_is_unchanged() {
        let mut backend = CliBackend::codex();
        let before = backend.args.clone();
        apply_hat_tool_policy(&mut backend, &["Edit".to_string()]);
        assert_eq!(backend.args, before);
    }

    #[test]
    fn empty_hat_list_is_noop() {
        let mut backend = CliBackend::claude();
        let before = backend.args.clone();
        apply_hat_tool_policy(&mut backend, &[]);
        assert_eq!(backend.args, before);
    }

    #[test]
    fn deduplicates_overlapping_tools() {
        let mut backend = CliBackend::claude();
        apply_hat_tool_policy(&mut backend, &["Edit".to_string(), "Edit".to_string()]);
        let flag = backend
            .args
            .iter()
            .find(|a| a.starts_with(DISALLOWED_TOOLS_PREFIX))
            .unwrap();
        assert_eq!(flag.matches("Edit").count(), 1, "flag={flag}");
    }
}
