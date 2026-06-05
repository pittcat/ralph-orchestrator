use super::*;

pub fn enforce_payload_contract_gate(config: &ralph_core::RalphConfig) -> Result<()> {
    let registry = ralph_core::HatRegistry::from_runtime_config(config);
    let result = validate_payload_contract(config, &registry, true);
    if result.is_valid() {
        // Surface any non-fatal warnings on stderr so users notice them.
        for w in &result.warnings {
            eprintln!("[payload-contract] warning: {}", w);
        }
        return Ok(());
    }
    let mut msg = String::from(
        "Payload contract gate failed. The preset's hat topology references \
         payload fields that are not covered by the configured schemas.\n\n\
         Errors:\n",
    );
    for err in &result.errors {
        msg.push_str(&format!(
            "  - [{}] hat={} topic={} field={} source_hats=[{}] schema={} line={:?}\n    {}\n",
            match err.kind {
                ralph_core::payload_contract::PayloadContractErrorKind::FieldMissingFromSchema =>
                    "FieldMissingFromSchema",
                ralph_core::payload_contract::PayloadContractErrorKind::SchemaMissingForRequiredTopic =>
                    "SchemaMissingForRequiredTopic",
            },
            err.hat_id,
            err.topic,
            err.field.as_deref().unwrap_or("(none)"),
            err.source_hats.join(", "),
            err.schema_defined_in,
            err.instructions_line,
            err.message,
        ));
    }
    msg.push_str(
        "\nFix by adding the missing fields to `event_policy.schemas.<topic>.required_fields`,\n\
         adding a `schema_file`, or removing the payload reference from the hat instructions.\n\
         Run `ralph hats validate` for a top-level view.\n",
    );
    bail!(msg);
}

/// U6: write a payload contract violation report to
/// `.ralph/diagnostics/payload-contract-error-{timestamp}.json`.
///
/// Returns the file path of the written report (even if the write failed,
/// the path is still useful for the user). The diagnostic must include:
/// error_type, timestamp, topic, field, source_hat[], target_hat,
/// schema_defined_in, downstream_reference, upstream_reference, fix_hint.
pub fn write_payload_contract_violation_report(
    diagnostics_dir: &std::path::Path,
    violation: &ralph_core::payload_contract::PayloadContractViolation,
) -> std::path::PathBuf {
    use std::io::Write as _;
    let stamp = violation.timestamp.replace(':', "-").replace('.', "-");
    let path = diagnostics_dir.join(format!("payload-contract-error-{}.json", stamp));
    let body = match serde_json::to_string_pretty(violation) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[payload-contract] failed to serialize violation: {}", e);
            return path;
        }
    };
    if let Err(e) = std::fs::create_dir_all(diagnostics_dir) {
        eprintln!(
            "[payload-contract] failed to create diagnostics dir {}: {}",
            diagnostics_dir.display(),
            e
        );
        eprintln!("[PAYLOAD CONTRACT VIOLATION] {}", violation.topic);
        eprintln!("  field: {:?}", violation.field);
        eprintln!("  source_hats: {:?}", violation.source_hat);
        eprintln!("  target_hats: {:?}", violation.target_hat);
        eprintln!("  fix_hint: {}", violation.fix_hint);
        return path;
    }
    match std::fs::File::create(&path).and_then(|mut f| f.write_all(body.as_bytes())) {
        Ok(()) => {
            eprintln!(
                "[PAYLOAD CONTRACT VIOLATION] Loop paused. Diagnostic written to {}",
                path.display()
            );
        }
        Err(e) => {
            eprintln!(
                "[payload-contract] failed to write diagnostic to {}: {}",
                path.display(),
                e
            );
            // Non-regression: must still surface the violation summary.
            eprintln!("[PAYLOAD CONTRACT VIOLATION] {}", violation.topic);
            eprintln!("  field: {:?}", violation.field);
            eprintln!("  source_hats: {:?}", violation.source_hat);
            eprintln!("  target_hats: {:?}", violation.target_hat);
            eprintln!("  fix_hint: {}", violation.fix_hint);
        }
    }
    path
}
