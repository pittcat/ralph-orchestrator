//! `profiles` configuration block — operator-supplied defaults for runtime
//! profile overlays.
//!
//! This module is the deserialization surface for the v1 `profiles:`
//! section of `ralph.yml`. It does NOT load profile directories, walk
//! preset subdirectories, or merge fragments into hat instructions —
//! those responsibilities live in `crate::profiles` (U2). The contract
//! here is narrow:
//!
//! - [`ProfileScope`] is a strict enum (`Repo` | `User`) so unknown
//!   scopes fail at parse time instead of silently turning into
//!   nothing.
//! - [`ProfileSpec`] pairs a scope with a non-empty name. Both halves
//!   are validated together in [`ProfileSpec::deserialize`] so a
//!   `repo:` spec (no name) is rejected at config load.
//! - [`ProfilesConfig`] holds the parsed list of default specs. It
//!   derives `Default` so omitting `profiles:` from `ralph.yml`
//!   continues to parse.
//!
//! Custom deserializer for `default`: the field accepts **either** a
//! YAML string (`"repo:strict, user:my-style"`) **or** a YAML sequence
//! (`[repo:strict, user:my-style]`). Whitespace around each comma-
//! separated entry is trimmed. The two shapes are equivalent — choose
//! whichever reads more cleanly in your `ralph.yml`.

use serde::{Deserialize, Deserializer, Serialize};

/// Where to look up a named profile directory at runtime.
///
/// - `Repo`: resolved by U2's [`crate::profiles::resolve_profile_dir`] to
///   `<workspace_root>/ralph-profiles/<name>/`.
/// - `User`: resolved to `$XDG_CONFIG_HOME/ralph/profiles/<name>/` (or
///   `~/.config/ralph/profiles/<name>/` when `XDG_CONFIG_HOME` is unset).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ProfileScope {
    /// Profile stored inside the project repository (shared via git).
    Repo,
    /// Profile stored under the user's XDG config dir (per-developer).
    User,
}

impl std::fmt::Display for ProfileScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Repo => f.write_str("repo"),
            Self::User => f.write_str("user"),
        }
    }
}

/// A single `(scope, name)` reference to a profile directory.
///
/// Built from a `<scope>:<name>` literal — see
/// [`ProfileSpec::deserialize`] for the parsing rules and the failure
/// modes (unknown scope, empty name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Hash)]
pub struct ProfileSpec {
    /// Which directory tree the profile lives under.
    pub scope: ProfileScope,
    /// Non-empty directory name under the scope root.
    pub name: String,
}

impl std::fmt::Display for ProfileSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.scope, self.name)
    }
}

impl ProfileSpec {
    /// Parse a single `<scope>:<name>` literal into a [`ProfileSpec`].
    ///
    /// Used by both the custom deserializer below and the U2 resolver.
    /// Public so U2 can reuse the exact same error format without
    /// round-tripping through YAML.
    pub fn parse_str(s: &str) -> Result<Self, ProfileSpecParseError> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return Err(ProfileSpecParseError::Empty);
        }
        let (scope, name) = trimmed
            .split_once(':')
            .ok_or_else(|| ProfileSpecParseError::MissingColon(trimmed.to_string()))?;
        let scope = match scope {
            "repo" => ProfileScope::Repo,
            "user" => ProfileScope::User,
            other => {
                return Err(ProfileSpecParseError::UnknownScope {
                    spec: trimmed.to_string(),
                    scope: other.to_string(),
                });
            }
        };
        let name = name.trim();
        if name.is_empty() {
            return Err(ProfileSpecParseError::EmptyName(trimmed.to_string()));
        }
        Ok(Self {
            scope,
            name: name.to_string(),
        })
    }
}

/// Errors surfaced when parsing a single `<scope>:<name>` literal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileSpecParseError {
    /// The input was empty or whitespace-only.
    Empty,
    /// The input had no `:` separator (e.g. `repo_strict`).
    MissingColon(String),
    /// The scope half was something other than `repo` or `user`.
    UnknownScope { spec: String, scope: String },
    /// The name half was empty (e.g. `repo:`).
    EmptyName(String),
}

impl std::fmt::Display for ProfileSpecParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("profile spec is empty"),
            Self::MissingColon(s) => write!(
                f,
                "invalid profile spec {s:?}: must be \"<scope>:<name>\" with scope 'repo' or 'user'"
            ),
            Self::UnknownScope { spec, scope } => write!(
                f,
                "invalid profile spec {spec:?}: unknown scope {scope:?}; expected 'repo' or 'user'"
            ),
            Self::EmptyName(s) => write!(
                f,
                "invalid profile spec {s:?}: profile name cannot be empty"
            ),
        }
    }
}

impl std::error::Error for ProfileSpecParseError {}

impl<'de> Deserialize<'de> for ProfileSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse_str(&raw).map_err(serde::de::Error::custom)
    }
}

/// Top-level `profiles:` block in `ralph.yml`.
///
/// Today only carries the operator-supplied defaults; future fields
/// (e.g. `disabled`, `paths`) will be added here without changing the
/// public surface.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfilesConfig {
    /// Profiles that activate by default when `ralph run` starts. The
    /// CLI's `--profile` flags are applied after these, and
    /// `--no-default-profiles` clears this list entirely (U3 / U4).
    #[serde(default, deserialize_with = "deserialize_profile_specs")]
    pub default: Vec<ProfileSpec>,
}

/// Accept either a single comma-separated string or a YAML sequence.
/// Empty entries (e.g. `"repo:strict, , user:my"`) and surrounding
/// whitespace are tolerated — the deserializer skips blanks and trims
/// each spec before handing it to [`ProfileSpec::parse_str`].
fn deserialize_profile_specs<'de, D>(deserializer: D) -> Result<Vec<ProfileSpec>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::{Error, SeqAccess, Visitor};
    use std::fmt;

    struct ProfileSpecsVisitor;

    impl<'de> Visitor<'de> for ProfileSpecsVisitor {
        type Value = Vec<ProfileSpec>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(
                "a string (\"repo:x, user:y\"), a sequence of strings, or a sequence of {scope, name} objects",
            )
        }

        fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
            parse_comma_separated(v).map_err(Error::custom)
        }

        fn visit_string<E: Error>(self, v: String) -> Result<Self::Value, E> {
            self.visit_str(&v)
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut out = Vec::new();
            // Look ahead at the first element to decide whether this is
            // a sequence of strings or a sequence of already-structured
            // {scope, name} objects. The two shapes have different YAML
            // representations and only differ at the element type.
            if let Some(first) = seq.next_element::<serde_yaml::Value>()? {
                match first {
                    serde_yaml::Value::String(s) => {
                        // Sequence of strings — split each entry on `:`.
                        let trimmed = s.trim();
                        if !trimmed.is_empty() {
                            out.push(ProfileSpec::parse_str(trimmed).map_err(Error::custom)?);
                        }
                        while let Some(next) = seq.next_element::<String>()? {
                            let trimmed = next.trim();
                            if trimmed.is_empty() {
                                continue;
                            }
                            out.push(ProfileSpec::parse_str(trimmed).map_err(Error::custom)?);
                        }
                    }
                    serde_yaml::Value::Mapping(_) => {
                        // Sequence of already-structured specs — extract
                        // scope + name from each `{scope, name}` mapping
                        // directly. This is the round-trip path emitted by
                        // `serde_yaml::to_value`. We can't delegate to
                        // `ProfileSpec::deserialize` (it expects a string),
                        // so we read the two fields out manually.
                        out.push(spec_from_mapping(first).map_err(Error::custom)?);
                        while let Some(next) = seq.next_element::<serde_yaml::Value>()? {
                            out.push(spec_from_mapping(next).map_err(Error::custom)?);
                        }
                    }
                    other => {
                        return Err(Error::invalid_type(
                            serde::de::Unexpected::Other(&format!("{other:?}")),
                            &"a string or {scope, name} mapping",
                        ));
                    }
                }
            }
            Ok(out)
        }
    }

    deserializer.deserialize_any(ProfileSpecsVisitor)
}

fn parse_comma_separated(s: &str) -> Result<Vec<ProfileSpec>, ProfileSpecParseError> {
    let mut out = Vec::new();
    for part in s.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        out.push(ProfileSpec::parse_str(trimmed)?);
    }
    Ok(out)
}

/// Extract a [`ProfileSpec`] from a `{scope, name}` YAML mapping.
///
/// Used by the round-trip path in `deserialize_profile_specs`, where
/// `serde_yaml::to_value(&ProfilesConfig { default: [...] })` produces a
/// sequence of mapping nodes. `ProfileSpec`'s `Deserialize` impl parses
/// a single string (`<scope>:<name>`), so we can't reuse it here.
fn spec_from_mapping(value: serde_yaml::Value) -> Result<ProfileSpec, ProfileSpecParseError> {
    use serde_yaml::Value;
    let mapping = match value {
        Value::Mapping(m) => m,
        other => {
            return Err(ProfileSpecParseError::MissingColon(format!(
                "expected {{scope, name}} mapping, got {other:?}"
            )));
        }
    };
    let scope = mapping
        .get(Value::String("scope".to_string()))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ProfileSpecParseError::MissingColon("missing or non-string `scope`".to_string())
        })?;
    let name = mapping
        .get(Value::String("name".to_string()))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ProfileSpecParseError::MissingColon("missing or non-string `name`".to_string())
        })?;
    let scope = match scope {
        "repo" => ProfileScope::Repo,
        "user" => ProfileScope::User,
        other => {
            return Err(ProfileSpecParseError::UnknownScope {
                spec: format!("{scope}:{name}"),
                scope: other.to_string(),
            });
        }
    };
    if name.is_empty() {
        return Err(ProfileSpecParseError::EmptyName(format!("{scope}:")));
    }
    Ok(ProfileSpec {
        scope,
        name: name.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_repo_spec() {
        let spec = ProfileSpec::parse_str("repo:strict").unwrap();
        assert_eq!(spec.scope, ProfileScope::Repo);
        assert_eq!(spec.name, "strict");
    }

    #[test]
    fn parse_user_spec_with_hyphens() {
        let spec = ProfileSpec::parse_str("user:my-style").unwrap();
        assert_eq!(spec.scope, ProfileScope::User);
        assert_eq!(spec.name, "my-style");
    }

    #[test]
    fn parse_trims_surrounding_whitespace() {
        let spec = ProfileSpec::parse_str("  user:my-style  ").unwrap();
        assert_eq!(spec.name, "my-style");
    }

    #[test]
    fn parse_rejects_empty() {
        assert!(matches!(
            ProfileSpec::parse_str("").unwrap_err(),
            ProfileSpecParseError::Empty
        ));
        assert!(matches!(
            ProfileSpec::parse_str("   ").unwrap_err(),
            ProfileSpecParseError::Empty
        ));
    }

    #[test]
    fn parse_rejects_missing_colon() {
        let err = ProfileSpec::parse_str("repo_strict").unwrap_err();
        assert!(matches!(err, ProfileSpecParseError::MissingColon(_)));
    }

    #[test]
    fn parse_rejects_unknown_scope() {
        let err = ProfileSpec::parse_str("team:strict").unwrap_err();
        assert!(matches!(
            err,
            ProfileSpecParseError::UnknownScope { ref scope, .. } if scope == "team"
        ));
    }

    #[test]
    fn parse_rejects_empty_name() {
        let err = ProfileSpec::parse_str("repo:").unwrap_err();
        assert!(matches!(err, ProfileSpecParseError::EmptyName(_)));
    }

    #[test]
    fn deserialize_comma_string_skips_blank_entries() {
        let yaml = r#"
default: "repo:strict, , user:my-style"
"#;
        let cfg: ProfilesConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.default.len(), 2);
        assert_eq!(cfg.default[0].name, "strict");
        assert_eq!(cfg.default[1].name, "my-style");
    }

    #[test]
    fn deserialize_sequence_skips_blank_entries() {
        let yaml = r#"
default:
  - repo:strict
  - ""
  - user:my-style
"#;
        let cfg: ProfilesConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.default.len(), 2);
        assert_eq!(cfg.default[0].name, "strict");
        assert_eq!(cfg.default[1].name, "my-style");
    }

    #[test]
    fn deserialize_default_is_empty_list() {
        let cfg = ProfilesConfig::default();
        assert!(cfg.default.is_empty());
    }

    #[test]
    fn display_renders_lowercase_scope() {
        assert_eq!(ProfileScope::Repo.to_string(), "repo");
        assert_eq!(ProfileScope::User.to_string(), "user");
    }
}
