//! State file injection for prompts.
//!
//! Automatically injects external structured state files (JSON/JSONL) into
//! the agent prompt, similar to how scratchpad content is injected.

use crate::config::{StateFileFormat, StateFilesConfig};
use std::path::Path;

/// Injects state files into the prompt.
pub fn inject_state_files(
    prompt: String,
    config: &StateFilesConfig,
    workspace_root: &Path,
) -> String {
    if !config.enabled {
        return prompt;
    }

    let mut blocks = Vec::new();

    if let Some(ref preamble) = config.inject_preamble {
        blocks.push(preamble.clone());
    }

    for file in &config.files {
        let resolved_path = workspace_root.join(&file.path);
        match read_state_file(&resolved_path, file.char_budget, file.tail_lines) {
            Ok(content) => {
                let format_attr = match file.format {
                    StateFileFormat::Json => "json",
                    StateFileFormat::Jsonl => "jsonl",
                };
                blocks.push(format!(
                    "<state-file name=\"{}\" format=\"{}\">\n{}\n</state-file>",
                    file.path, format_attr, content
                ));
            }
            Err(e) => {
                eprintln!("[state_files] warning: {}: {}", file.path, e);
                blocks.push(format!(
                    "<state-file name=\"{}\" format=\"{}\"></state-file>",
                    file.path,
                    match file.format {
                        StateFileFormat::Json => "json",
                        StateFileFormat::Jsonl => "jsonl",
                    }
                ));
            }
        }
    }

    if blocks.is_empty() {
        return prompt;
    }

    let injected = blocks.join("\n\n");
    format!("{}\n\n{}", injected, prompt)
}

fn read_state_file(
    path: &Path,
    char_budget: Option<usize>,
    tail_lines: Option<usize>,
) -> std::io::Result<String> {
    let content = std::fs::read_to_string(path)?;

    let content = if let Some(lines) = tail_lines {
        content
            .lines()
            .rev()
            .take(lines)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        content
    };

    let content = if let Some(budget) = char_budget {
        let char_count = content.chars().count();
        if char_count > budget {
            // Keep tail (most recent), similar to scratchpad truncation
            let skip_chars = char_count - budget;
            let start = content
                .char_indices()
                .nth(skip_chars)
                .map(|(idx, _)| idx)
                .unwrap_or(content.len());
            let start = crate::text::floor_char_boundary(&content, start);
            // Find line boundary
            let line_start = content[start..].find('\n').map_or(start, |n| start + n + 1);
            format!(
                "<!-- earlier content truncated ({} chars omitted) -->\n{}",
                skip_chars,
                &content[line_start..]
            )
        } else {
            content
        }
    } else {
        content
    };

    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StateFileEntry;
    use std::io::Write;

    fn make_config(files: Vec<StateFileEntry>) -> StateFilesConfig {
        StateFilesConfig {
            enabled: true,
            inject_preamble: None,
            files,
        }
    }

    #[test]
    fn test_json_file_happy_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("state.json");
        std::fs::write(&file_path, r#"{"key": "value"}"#).unwrap();

        let config = make_config(vec![StateFileEntry {
            path: "state.json".to_string(),
            format: StateFileFormat::Json,
            char_budget: None,
            tail_lines: None,
        }]);

        let result = inject_state_files("prompt here".to_string(), &config, temp_dir.path());
        assert!(result.contains("<state-file name=\"state.json\" format=\"json\">"));
        assert!(result.contains(r#"{"key": "value"}"#));
        assert!(result.contains("</state-file>"));
        assert!(result.ends_with("prompt here"));
    }

    #[test]
    fn test_jsonl_tail_lines() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("events.jsonl");
        let mut file = std::fs::File::create(&file_path).unwrap();
        writeln!(file, r#"{{"event": "first"}}"#).unwrap();
        writeln!(file, r#"{{"event": "second"}}"#).unwrap();
        writeln!(file, r#"{{"event": "third"}}"#).unwrap();

        let config = make_config(vec![StateFileEntry {
            path: "events.jsonl".to_string(),
            format: StateFileFormat::Jsonl,
            char_budget: None,
            tail_lines: Some(2),
        }]);

        let result = inject_state_files("prompt".to_string(), &config, temp_dir.path());
        assert!(result.contains(r#"{"event": "second"}"#));
        assert!(result.contains(r#"{"event": "third"}"#));
        assert!(!result.contains(r#"{"event": "first"}"#));
        assert!(result.ends_with("prompt"));
    }

    #[test]
    fn test_char_budget_truncation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("large.json");
        // Create content larger than budget
        let content = "line1\nline2\nline3\nline4\nline5";
        std::fs::write(&file_path, content).unwrap();

        let config = make_config(vec![StateFileEntry {
            path: "large.json".to_string(),
            format: StateFileFormat::Json,
            char_budget: Some(20),
            tail_lines: None,
        }]);

        let result = inject_state_files("prompt".to_string(), &config, temp_dir.path());
        assert!(result.contains("<!-- earlier content truncated"));
        assert!(result.contains("line4"));
        assert!(result.contains("line5"));
        assert!(!result.contains("line1"));
        assert!(result.ends_with("prompt"));
    }

    #[test]
    fn test_missing_file_injects_empty_block() {
        let temp_dir = tempfile::tempdir().unwrap();

        let config = make_config(vec![StateFileEntry {
            path: "missing.json".to_string(),
            format: StateFileFormat::Json,
            char_budget: None,
            tail_lines: None,
        }]);

        let result = inject_state_files("prompt".to_string(), &config, temp_dir.path());
        assert!(result.contains("<state-file name=\"missing.json\" format=\"json\"></state-file>"));
        assert!(result.ends_with("prompt"));
    }

    #[test]
    fn test_injection_order() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("state.json");
        std::fs::write(&file_path, r#"{"key": "value"}"#).unwrap();

        let config = StateFilesConfig {
            enabled: true,
            inject_preamble: Some("Preamble text".to_string()),
            files: vec![StateFileEntry {
                path: "state.json".to_string(),
                format: StateFileFormat::Json,
                char_budget: None,
                tail_lines: None,
            }],
        };

        let result = inject_state_files("original prompt".to_string(), &config, temp_dir.path());
        // Order should be: preamble, state-file block, original prompt
        let preamble_pos = result.find("Preamble text").unwrap();
        let state_file_pos = result.find("<state-file").unwrap();
        let prompt_pos = result.find("original prompt").unwrap();
        assert!(preamble_pos < state_file_pos);
        assert!(state_file_pos < prompt_pos);
    }

    #[test]
    fn test_disabled_returns_prompt_unchanged() {
        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("state.json");
        std::fs::write(&file_path, r#"{"key": "value"}"#).unwrap();

        let config = StateFilesConfig {
            enabled: false,
            inject_preamble: None,
            files: vec![StateFileEntry {
                path: "state.json".to_string(),
                format: StateFileFormat::Json,
                char_budget: None,
                tail_lines: None,
            }],
        };

        let result = inject_state_files("prompt".to_string(), &config, temp_dir.path());
        assert_eq!(result, "prompt");
    }

    #[test]
    fn test_no_files_returns_prompt_unchanged() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = make_config(vec![]);

        let result = inject_state_files("prompt".to_string(), &config, temp_dir.path());
        assert_eq!(result, "prompt");
    }
}
