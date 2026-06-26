//! Shared abstraction over the CLI args that drive profile overlay
//! activation.
//!
//! `ralph run` and `ralph inspect profiles` both take a
//! `--profile ...` / `--no-default-profiles` pair, and both need to
//! resolve them to the same ordered `Vec<ProfileSpec>`. Before this
//! module existed each command re-implemented the body inline, with
//! the bug-prone consequence that `ralph inspect profiles` could
//! silently disagree with `ralph run` about activation order.
//!
//! The [`ProfileArgs`] trait lets both surfaces share
//! [`collect_active_profile_specs`]. Each command supplies a thin
//! `impl ProfileArgs for ItsArgs { ... }` that exposes the two fields;
//! the helper itself does the spec-merge logic in exactly one place.
//!
//! [`collect_active_profile_specs`]: crate::commands::profile_args::collect_active_profile_specs

use ralph_core::config::profiles::ProfileSpec;
use ralph_core::profiles::ProfilesError;
use ralph_core::RalphConfig;

/// CLI args that drive profile overlay activation.
///
/// Implemented by both [`crate::commands::run::RunArgs`] and
/// [`crate::commands::inspect::InspectProfilesArgs`]. The shape is
/// intentionally narrow — only the two fields the activation logic
/// needs — so neither caller has to fabricate a synthetic arg struct
/// just to reuse the helper.
pub trait ProfileArgs {
    /// Raw `--profile` values, in argv order. Empty when the flag was
    /// not passed.
    fn profile_specs(&self) -> &[String];
    /// `true` when `--no-default-profiles` was supplied; suppresses
    /// `config.profiles.default` but leaves CLI flags in effect.
    fn no_default_profiles(&self) -> bool;
}

/// Merge `config.profiles.default` (unless suppressed) with the
/// caller-supplied CLI flags, returning the activation-order list of
/// [`ProfileSpec`]s.
///
/// Activation order (per plan 2026-06-25-002 R10):
///
/// 1. `config.profiles.default` (operator-supplied ralph.yml
///    defaults) — only included when `args.no_default_profiles()` is
///    `false`.
/// 2. Each entry in `args.profile_specs()` (CLI `--profile` flags),
///    in argv order.
///
/// Both lists are validated via [`ralph_core::profiles::parse_profile_spec`];
/// the first malformed entry (from either source) is surfaced as a
/// [`ProfilesError::InvalidSpec`] carrying the original literal. That
/// lets `ralph run` and `ralph inspect profiles` report the offending
/// token verbatim.
///
/// This helper is **pure**: it does not touch the filesystem and
/// never resolves `<profile>/<preset>/<hat>.md` paths. U4 owns the
/// apply step; U5 owns the inspect-side resolution that reuses this
/// same helper for spec collection.
pub(crate) fn collect_active_profile_specs<A: ProfileArgs>(
    config: &RalphConfig,
    args: &A,
) -> Result<Vec<ProfileSpec>, ProfilesError> {
    let mut specs = Vec::new();
    if !args.no_default_profiles() {
        // `ProfilesConfig::default` already filters empty entries, so
        // we can blindly clone the operator-supplied list. Cloning
        // keeps `config` borrowed immutably, which matches the helper
        // signature.
        specs.extend(config.profiles.default.iter().cloned());
    }
    for raw in args.profile_specs() {
        specs.push(ralph_core::profiles::parse_profile_spec(raw)?);
    }
    Ok(specs)
}

#[cfg(test)]
mod tests {
    //! Cross-surface activation-order parity: the two impls of
    //! [`ProfileArgs`] in `commands::run` and `commands::inspect`
    //! must produce the same `Vec<ProfileSpec>` for the same inputs.
    //! If they ever diverge, the inspect-preview falls out of sync
    //! with `ralph run` and operators chase phantom warnings.

    use super::*;
    use ralph_core::config::profiles::ProfileScope;

    struct StubArgs {
        specs: Vec<String>,
        no_default: bool,
    }

    impl ProfileArgs for StubArgs {
        fn profile_specs(&self) -> &[String] {
            &self.specs
        }
        fn no_default_profiles(&self) -> bool {
            self.no_default
        }
    }

    fn stub(specs: &[&str], no_default: bool) -> StubArgs {
        StubArgs {
            specs: specs.iter().map(|s| s.to_string()).collect(),
            no_default,
        }
    }

    fn empty_config() -> RalphConfig {
        RalphConfig::default()
    }

    fn config_with_defaults(specs: Vec<ProfileSpec>) -> RalphConfig {
        let mut cfg = RalphConfig::default();
        cfg.profiles.default = specs;
        cfg
    }

    #[test]
    fn defaults_then_cli_order() {
        let cfg = config_with_defaults(vec![ProfileSpec {
            scope: ProfileScope::Repo,
            name: "base".to_string(),
        }]);
        let args = stub(&["user:extra"], false);
        let active = collect_active_profile_specs(&cfg, &args).unwrap();
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].to_string(), "repo:base");
        assert_eq!(active[1].to_string(), "user:extra");
    }

    #[test]
    fn no_default_profiles_skips_only_defaults() {
        let cfg = config_with_defaults(vec![ProfileSpec {
            scope: ProfileScope::Repo,
            name: "base".to_string(),
        }]);
        let args = stub(&["user:extra"], true);
        let active = collect_active_profile_specs(&cfg, &args).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].to_string(), "user:extra");
    }

    #[test]
    fn empty_inputs_yield_empty_list() {
        let cfg = empty_config();
        let args = stub(&[], false);
        let active = collect_active_profile_specs(&cfg, &args).unwrap();
        assert!(active.is_empty());
    }

    #[test]
    fn malformed_cli_spec_surfaces_invalid_spec_error() {
        let cfg = empty_config();
        let args = stub(&["bad-spec"], false);
        let err = collect_active_profile_specs(&cfg, &args).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("bad-spec"), "got: {msg}");
    }
}