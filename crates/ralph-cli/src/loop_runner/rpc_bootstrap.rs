//! Maps loop-runner recovery state onto the frontend RPC bootstrap contract.

use ralph_core::event_loop::rejection::ManifestResumeRecovery;
use ralph_proto::json_rpc::LoopBootstrap;

pub(super) fn loop_bootstrap(
    resume: bool,
    manifest_recovery: Option<&ManifestResumeRecovery>,
) -> LoopBootstrap {
    if resume {
        LoopBootstrap::Continue
    } else if let Some(recovery) = manifest_recovery {
        LoopBootstrap::ManifestResume {
            target_hat: recovery.target_hat.as_str().to_string(),
            original_trigger_topic: recovery.original_trigger_topic.clone(),
        }
    } else {
        LoopBootstrap::Fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_proto::HatId;

    fn recovery() -> ManifestResumeRecovery {
        ManifestResumeRecovery {
            target_hat: HatId::new("forge-dispatcher"),
            payload: "{}".to_string(),
            original_trigger_topic: "forge.worktrees.ready".to_string(),
            original_trigger_payload: None,
        }
    }

    #[test]
    fn manifest_recovery_is_exposed_to_rpc_frontends() {
        assert_eq!(
            loop_bootstrap(false, Some(&recovery())),
            LoopBootstrap::ManifestResume {
                target_hat: "forge-dispatcher".to_string(),
                original_trigger_topic: "forge.worktrees.ready".to_string(),
            }
        );
    }

    #[test]
    fn continue_takes_precedence_over_reuse_manifest() {
        assert_eq!(
            loop_bootstrap(true, Some(&recovery())),
            LoopBootstrap::Continue
        );
    }

    #[test]
    fn absent_recovery_is_fresh() {
        assert_eq!(loop_bootstrap(false, None), LoopBootstrap::Fresh);
    }
}
