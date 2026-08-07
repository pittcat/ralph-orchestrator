use super::super::*;
use crate::config::RalphConfig;

#[test]
fn config_without_mechanism_uses_hat_only_emit_pipeline() {
    let config = RalphConfig::default();
    let (pipeline, step_totals, _authority) = build_stage_pipeline_from_config(&config);
    assert!(step_totals.is_empty());
    assert_eq!(
        pipeline.names(),
        vec!["RepairDispatch", "EmitSchemaGate", "VerdictGate"]
    );
}
