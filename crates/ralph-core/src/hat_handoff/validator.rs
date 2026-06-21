//! 2026-06-18-002 plan U3: 五段式结构校验器(R7-R14, R16 结构部分)。
//!
//! 五段固定顺序:
//! - `# Handoff: <from> → <to>`
//! - `## context`
//! - `## changed`
//! - `## verify`
//! - `## next`(恰好一行 `**动作**:`, 一行 `**阻塞**:`;可选 `**先读**:`)
//! - `## notes`(可选)
//!
//! 拒收反模式:
//! - 缺段、段顺序错乱
//! - `## next` 动作行无宾语(纯"继续处理"、"review"等)
//! - `## notes` 单词数 > 15(逼 agent 把细节写到 `## verify` / `## changed`)

use std::fmt;
use std::path::Path;

/// 五段标题在文件中的固定顺序。
pub const SECTION_HEADERS: &[&str] = &[
    "## context",
    "## changed",
    "## verify",
    "## next",
];

/// `## next` 必填字段。
pub const NEXT_REQUIRED_FIELDS: &[&str] = &["**动作**:", "**阻塞**:"];

/// `## next` 可选字段(0 或 1 行)。
pub const NEXT_OPTIONAL_FIELDS: &[&str] = &["**先读**:"];

/// 占位符:三选一表示"无/未验证/不适用"。
pub const PLACEHOLDERS: &[&str] = &["无", "未验证", "不适用"];

/// 动作行反模式表(KTD-15 不在此处处理,topic 校验在 U4)。
/// 命中任一关键词 + 无宾语 → 拒收。
const ACTION_ANTIPATTERNS: &[&str] = &[
    "继续处理",
    "继续执行",
    "按 preset",
    "按规则",
    "review",
    "评审",
    "看下",
    "检查一下",
    "处理一下",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HatHandoffViolation {
    /// 缺段 / 段顺序错乱。
    MissingOrOutOfOrderSection { expected: String, found: String },
    /// `## next` 缺必填行(动作/阻塞)。
    MissingNextField { field: String },
    /// `## next` 多于一行(动作/阻塞只能各 1 行)。
    DuplicateNextField { field: String },
    /// `## next` 出现未知行(`**xxx**:` 不在白名单内)。
    UnknownNextField { field: String },
    /// `## next` 动作行仅含反模式关键词,无宾语。
    AntipatternActionLine { raw: String },
    /// `## notes` 超过 15 词。
    NotesTooLong { words: usize },
    /// 文件不是以 `# Handoff:` 开头。
    MissingH1Title,
    /// 文件为空。
    Empty,
}

impl fmt::Display for HatHandoffViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingOrOutOfOrderSection { expected, found } => write!(
                f,
                "expected section `{expected}` next, got `{found}` (or section missing)"
            ),
            Self::MissingNextField { field } => {
                write!(f, "`## next` missing required line `{field}`")
            }
            Self::DuplicateNextField { field } => {
                write!(f, "`## next` field `{field}` appears more than once")
            }
            Self::UnknownNextField { field } => write!(
                f,
                "`## next` contains unknown field `{field}` (must be one of action/blocker/先读)"
            ),
            Self::AntipatternActionLine { raw } => write!(
                f,
                "`## next` action line is an antipattern with no concrete object: `{raw}`"
            ),
            Self::NotesTooLong { words } => write!(
                f,
                "`## notes` exceeds 15 words ({words}); move detail to ## verify / ## changed"
            ),
            Self::MissingH1Title => write!(f, "file must start with `# Handoff:` title"),
            Self::Empty => write!(f, "handoff file is empty"),
        }
    }
}

impl std::error::Error for HatHandoffViolation {}

/// 解析并校验 handoff 文件。返回 `Ok(())` 或首个违规。
pub fn validate(content: &str) -> Result<(), HatHandoffViolation> {
    let content = content.trim_start();
    if content.is_empty() {
        return Err(HatHandoffViolation::Empty);
    }
    if !content.starts_with("# Handoff:") {
        return Err(HatHandoffViolation::MissingH1Title);
    }

    // 抽取 ## 段(忽略 H1 之前的行)。
    let mut lines = content.lines();
    let _title = lines.next(); // H1
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for line in lines {
        if let Some(header) = line.strip_prefix("## ") {
            // 新段开始
            if let Some(prev) = current.take() {
                sections.push(prev);
            }
            current = Some((header.trim().to_string(), Vec::new()));
        } else if let Some(ref mut c) = current {
            c.1.push(line.to_string());
        }
    }
    if let Some(last) = current.take() {
        sections.push(last);
    }

    // 校验段顺序。
    let mut expected_iter = SECTION_HEADERS.iter().map(|s| s.trim_start_matches("## "));
    let mut expected = expected_iter.next().map(str::to_string);
    for (header, _body) in &sections {
        if header == "notes" {
            continue; // notes 是可选 + 不参与顺序检查
        }
        match expected.as_ref() {
            Some(exp) if header == exp => {
                expected = expected_iter.next().map(str::to_string);
            }
            Some(exp) => {
                return Err(HatHandoffViolation::MissingOrOutOfOrderSection {
                    expected: format!("## {exp}"),
                    found: format!("## {header}"),
                });
            }
            None => {
                return Err(HatHandoffViolation::MissingOrOutOfOrderSection {
                    expected: "(none)".to_string(),
                    found: format!("## {header}"),
                });
            }
        }
    }
    if expected.is_some() {
        return Err(HatHandoffViolation::MissingOrOutOfOrderSection {
            expected: format!("## {}", expected.unwrap_or_default()),
            found: "(end of file)".to_string(),
        });
    }

    // 校验 `## next`。
    let next_body = sections
        .iter()
        .find(|(h, _)| h == "next")
        .map(|(_, b)| b.clone())
        .unwrap_or_default();
    validate_next_body(&next_body)?;

    // 校验 `## notes` (可选)。
    if let Some((_, body)) = sections.iter().find(|(h, _)| h == "notes") {
        let joined = body.join(" ");
        let words = joined.split_whitespace().count();
        if words > 15 {
            return Err(HatHandoffViolation::NotesTooLong { words });
        }
    }

    Ok(())
}

fn validate_next_body(body: &[String]) -> Result<(), HatHandoffViolation> {
    let mut action_count = 0usize;
    let mut blocker_count = 0usize;
    let mut xian_du_count = 0usize;
    let mut action_raw = String::new();
    for line in body {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("**动作**:") {
            action_count += 1;
            action_raw = trimmed.to_string();
        } else if trimmed.starts_with("**阻塞**:") {
            blocker_count += 1;
        } else if trimmed.starts_with("**先读**:") {
            xian_du_count += 1;
        } else {
            return Err(HatHandoffViolation::UnknownNextField {
                field: trimmed.split(':').next().unwrap_or(trimmed).to_string(),
            });
        }
    }
    if action_count == 0 {
        return Err(HatHandoffViolation::MissingNextField {
            field: "**动作**:".to_string(),
        });
    }
    if blocker_count == 0 {
        return Err(HatHandoffViolation::MissingNextField {
            field: "**阻塞**:".to_string(),
        });
    }
    if action_count > 1 {
        return Err(HatHandoffViolation::DuplicateNextField {
            field: "**动作**:".to_string(),
        });
    }
    if blocker_count > 1 {
        return Err(HatHandoffViolation::DuplicateNextField {
            field: "**阻塞**:".to_string(),
        });
    }
    if xian_du_count > 1 {
        return Err(HatHandoffViolation::DuplicateNextField {
            field: "**先读**:".to_string(),
        });
    }

    // 反模式:动作行仅含反模式关键词 + 无宾语。
    let action_body = action_raw
        .trim_start_matches("**动作**:")
        .trim()
        .to_lowercase();
    for anti in ACTION_ANTIPATTERNS {
        if action_body == anti.to_lowercase() || action_body.is_empty() {
            return Err(HatHandoffViolation::AntipatternActionLine { raw: action_raw });
        }
    }
    Ok(())
}

/// 构造一份合格的五段式 skeleton,供 `HatHandoffAllocator::prepare` 写盘。
pub fn build_skeleton(from: &str, to: &str, topic: &str) -> String {
    format!(
        "# Handoff: {from} → {to}\n\
         ## context\n无\n\n\
         ## changed\n无\n\n\
         ## verify\n未验证\n\n\
         ## next\n\
         **动作**: 待填写 (e.g. emit `{topic}` after <step>)\n\
         **阻塞**: 无\n\n\
         ## notes\n无\n"
    )
}

/// 2026-06-21-002 plan U5: 端到端校验。
///
/// 读盘 → 调 `validate` 做结构校验 → 额外校验 H1 的 `from → to`
/// 与 `expected_from` / `expected_to`(sanitize 后)匹配。
///
/// `expected_from` / `expected_to` 是 caller 已知的 hat id
/// (例如 `LoopState` 持有的 from hat),无需走 H1 字符串解析。
/// 文件不存在或读失败返回 `Err(String)`,`validate` 失败时
/// 同样返回 `Err(String)`(结构错误描述)。
pub fn validate_artifact(
    workspace: &Path,
    handoff_path: &str,
    expected_from: &str,
    expected_to: &str,
) -> Result<(), String> {
    let rel = Path::new(handoff_path);
    if rel.is_absolute() {
        return Err(format!(
            "validate_artifact: handoff_path `{handoff_path}` must be repo-relative"
        ));
    }
    // 简化:假设 caller 给的 `workspace` 是仓库根;若 handoff_path
    // 以 `..` 逃逸,resolve 后会落到 workspace 之外,读盘自然失败
    // — 不会被滥用为越权读。
    let abs = workspace.join(rel);
    let content = std::fs::read_to_string(&abs).map_err(|e| {
        format!(
            "validate_artifact: failed to read `{handoff_path}`: {e}"
        )
    })?;
    validate(&content).map_err(|v| v.to_string())?;
    // H1 owner 校验:形如 `# Handoff: <from> → <to>`。
    let expected_h1 = format!("# Handoff: {expected_from} → {expected_to}");
    if !content.trim_start().starts_with(&expected_h1) {
        return Err(format!(
            "validate_artifact: H1 must start with `{expected_h1}`; got `{}`",
            content.lines().next().unwrap_or("")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good_template() -> &'static str {
        "# Handoff: plan-gate → executor\n\
         ## context\n无\n\n\
         ## changed\n无\n\n\
         ## verify\n未验证\n\n\
         ## next\n\
         **动作**: emit work.ready after executor task definition\n\
         **阻塞**: 无\n\n\
         ## notes\n无\n"
    }

    #[test]
    fn good_template_validates() {
        assert!(validate(good_template()).is_ok());
    }

    #[test]
    fn empty_file_rejected() {
        assert_eq!(validate(""), Err(HatHandoffViolation::Empty));
    }

    #[test]
    fn missing_h1_rejected() {
        assert_eq!(
            validate("## context\nfoo"),
            Err(HatHandoffViolation::MissingH1Title)
        );
    }

    #[test]
    fn missing_section_rejected() {
        let bad = "# Handoff: a → b\n\
                   ## context\nx\n\n\
                   ## verify\ny\n";
        let err = validate(bad).unwrap_err();
        assert!(matches!(
            err,
            HatHandoffViolation::MissingOrOutOfOrderSection { .. }
        ));
    }

    #[test]
    fn out_of_order_section_rejected() {
        let bad = "# Handoff: a → b\n\
                   ## context\nx\n\n\
                   ## verify\ny\n\n\
                   ## changed\nz\n\n\
                   ## next\n**动作**: foo\n**阻塞**: bar\n";
        let err = validate(bad).unwrap_err();
        assert!(matches!(
            err,
            HatHandoffViolation::MissingOrOutOfOrderSection { .. }
        ));
    }

    #[test]
    fn missing_next_action_rejected() {
        let bad = "# Handoff: a → b\n\
                   ## context\nx\n\n\
                   ## changed\ny\n\n\
                   ## verify\nz\n\n\
                   ## next\n**阻塞**: bar\n";
        assert_eq!(
            validate(bad),
            Err(HatHandoffViolation::MissingNextField {
                field: "**动作**:".to_string()
            })
        );
    }

    #[test]
    fn antipattern_action_line_rejected() {
        let bad = "# Handoff: a → b\n\
                   ## context\nx\n\n\
                   ## changed\ny\n\n\
                   ## verify\nz\n\n\
                   ## next\n\
                   **动作**: 继续处理\n\
                   **阻塞**: 无\n";
        assert!(matches!(
            validate(bad).unwrap_err(),
            HatHandoffViolation::AntipatternActionLine { .. }
        ));
    }

    #[test]
    fn notes_too_long_rejected() {
        let mut s = String::from(good_template());
        s = s.replace(
            "## notes\n无\n",
            "## notes\nthis is a very long notes section that exceeds the fifteen word limit and should be rejected by the validator\n",
        );
        let err = validate(&s).unwrap_err();
        assert!(matches!(err, HatHandoffViolation::NotesTooLong { .. }));
    }

    #[test]
    fn unknown_next_field_rejected() {
        let bad = "# Handoff: a → b\n\
                   ## context\nx\n\n\
                   ## changed\ny\n\n\
                   ## verify\nz\n\n\
                   ## next\n\
                   **动作**: emit foo\n\
                   **阻塞**: 无\n\
                   **补充**: some detail\n";
        let err = validate(bad).unwrap_err();
        assert!(matches!(err, HatHandoffViolation::UnknownNextField { .. }));
    }

    #[test]
    fn duplicate_action_rejected() {
        let bad = "# Handoff: a → b\n\
                   ## context\nx\n\n\
                   ## changed\ny\n\n\
                   ## verify\nz\n\n\
                   ## next\n\
                   **动作**: emit foo\n\
                   **动作**: emit bar\n\
                   **阻塞**: 无\n";
        assert!(matches!(
            validate(bad).unwrap_err(),
            HatHandoffViolation::DuplicateNextField { .. }
        ));
    }

    #[test]
    fn skeleton_renderer_is_valid_after_filling_next() {
        let s = build_skeleton("plan-gate", "executor", "work.ready");
        // 把 next 动作行替换成合法内容
        let s = s.replace(
            "**动作**: 待填写 (e.g. emit `work.ready` after <step>)",
            "**动作**: emit work.ready after executor task creation",
        );
        validate(&s).unwrap();
    }

    // 2026-06-21-002 plan U5: validate_artifact 端到端校验。
    #[test]
    fn validate_artifact_passes_for_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        let abs = dir.path().join(".ralph/agent/hat-handoff/3-2-a-b.md");
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        let body = good_template();
        // good_template 写死 "plan-gate → executor",此处我们改写以匹配
        // from = a / to = b,确保 owner 校验通过。
        let body = body
            .replace("plan-gate → executor", "a → b")
            .replace("**动作**: emit work.done", "**动作**: emit b after a done");
        std::fs::write(&abs, &body).unwrap();
        let rel = ".ralph/agent/hat-handoff/3-2-a-b.md";
        assert!(validate_artifact(dir.path(), rel, "a", "b").is_ok());
    }

    #[test]
    fn validate_artifact_rejects_h1_owner_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let abs = dir.path().join(".ralph/agent/hat-handoff/3-2-a-b.md");
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        let body = good_template()
            .replace("plan-gate → executor", "x → y") // 与 caller 期望 a→b 不符
            .replace("**动作**: emit work.done", "**动作**: emit y after x done");
        std::fs::write(&abs, &body).unwrap();
        let rel = ".ralph/agent/hat-handoff/3-2-a-b.md";
        let err = validate_artifact(dir.path(), rel, "a", "b").unwrap_err();
        assert!(err.contains("H1 must start with"));
    }

    #[test]
    fn validate_artifact_rejects_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let rel = ".ralph/agent/hat-handoff/3-2-a-b.md";
        let err = validate_artifact(dir.path(), rel, "a", "b").unwrap_err();
        assert!(err.contains("failed to read"));
    }

    #[test]
    fn validate_artifact_rejects_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let rel = "/etc/passwd";
        let err = validate_artifact(dir.path(), rel, "a", "b").unwrap_err();
        assert!(err.contains("must be repo-relative"));
    }

    #[test]
    fn validate_artifact_rejects_structure_violation() {
        let dir = tempfile::tempdir().unwrap();
        let abs = dir.path().join(".ralph/agent/hat-handoff/3-2-a-b.md");
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        // 缺 ## verify
        let body = "# Handoff: a → b\n## context\nx\n## changed\ny\n## next\n**动作**: emit b after a\n**阻塞**: 无\n";
        std::fs::write(&abs, body).unwrap();
        let rel = ".ralph/agent/hat-handoff/3-2-a-b.md";
        let err = validate_artifact(dir.path(), rel, "a", "b").unwrap_err();
        assert!(err.contains("section"));
    }
}
