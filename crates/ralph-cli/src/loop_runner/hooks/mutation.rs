use super::super::*;

pub const HOOK_MUTATION_PAYLOAD_METADATA_KEY: &str = "metadata";
pub const HOOK_MUTATION_METADATA_NAMESPACE_KEY: &str = "hook_metadata";

#[derive(Debug, Clone, PartialEq)]
pub enum HookMutationParseOutcome {
    Disabled,
    Parsed {
        namespaced_metadata: serde_json::Map<String, serde_json::Value>,
    },
    Invalid(HookMutationParseError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookMutationParseError {
    InvalidJson { message: String },
    InvalidSchema { message: String },
}

pub fn format_hook_mutation_parse_error(error: &HookMutationParseError) -> String {
    match error {
        HookMutationParseError::InvalidJson { message }
        | HookMutationParseError::InvalidSchema { message } => message.clone(),
    }
}

pub fn parse_hook_mutation_stdout(
    mutate: &HookMutationConfig,
    hook_name: &str,
    stdout: &str,
) -> HookMutationParseOutcome {
    if !mutate.enabled {
        return HookMutationParseOutcome::Disabled;
    }

    let parsed = match serde_json::from_str::<serde_json::Value>(stdout.trim()) {
        Ok(parsed) => parsed,
        Err(error) => {
            return HookMutationParseOutcome::Invalid(HookMutationParseError::InvalidJson {
                message: format!("mutation stdout is not valid JSON: {error}"),
            });
        }
    };

    let Some(payload_object) = parsed.as_object() else {
        return HookMutationParseOutcome::Invalid(HookMutationParseError::InvalidSchema {
            message: "mutation payload must be a JSON object".to_string(),
        });
    };

    if payload_object.len() != 1 || !payload_object.contains_key(HOOK_MUTATION_PAYLOAD_METADATA_KEY)
    {
        let keys = payload_object.keys().cloned().collect::<Vec<_>>();
        return HookMutationParseOutcome::Invalid(HookMutationParseError::InvalidSchema {
            message: format!(
                "mutation payload supports only '{{\"{HOOK_MUTATION_PAYLOAD_METADATA_KEY}\": {{...}}}}'; found keys: {keys:?}"
            ),
        });
    }

    let Some(metadata) = payload_object
        .get(HOOK_MUTATION_PAYLOAD_METADATA_KEY)
        .and_then(serde_json::Value::as_object)
        .cloned()
    else {
        return HookMutationParseOutcome::Invalid(HookMutationParseError::InvalidSchema {
            message: "mutation payload key 'metadata' must contain a JSON object".to_string(),
        });
    };

    let mut namespaced_metadata = serde_json::Map::new();
    if let Err(error) = merge_hook_metadata_namespace(&mut namespaced_metadata, hook_name, metadata)
    {
        return HookMutationParseOutcome::Invalid(error);
    }

    HookMutationParseOutcome::Parsed {
        namespaced_metadata,
    }
}

pub fn merge_hook_metadata_namespace(
    accumulated_metadata: &mut serde_json::Map<String, serde_json::Value>,
    hook_name: &str,
    metadata: serde_json::Map<String, serde_json::Value>,
) -> std::result::Result<(), HookMutationParseError> {
    if hook_name.trim().is_empty() {
        return Err(HookMutationParseError::InvalidSchema {
            message: "hook metadata namespace requires non-empty hook name".to_string(),
        });
    }

    let namespace = accumulated_metadata
        .entry(HOOK_MUTATION_METADATA_NAMESPACE_KEY.to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));

    let Some(namespace_object) = namespace.as_object_mut() else {
        return Err(HookMutationParseError::InvalidSchema {
            message: format!(
                "metadata namespace '{HOOK_MUTATION_METADATA_NAMESPACE_KEY}' must be a JSON object"
            ),
        });
    };

    namespace_object.insert(hook_name.to_string(), serde_json::Value::Object(metadata));
    Ok(())
}

pub fn merge_namespaced_hook_metadata(
    accumulated_metadata: &mut serde_json::Map<String, serde_json::Value>,
    namespaced_metadata: &serde_json::Map<String, serde_json::Value>,
) -> std::result::Result<(), HookMutationParseError> {
    let Some(namespace_object) = namespaced_metadata
        .get(HOOK_MUTATION_METADATA_NAMESPACE_KEY)
        .and_then(serde_json::Value::as_object)
    else {
        return Err(HookMutationParseError::InvalidSchema {
            message: format!(
                "parsed mutation metadata must contain object key '{HOOK_MUTATION_METADATA_NAMESPACE_KEY}'"
            ),
        });
    };

    for (hook_name, metadata_value) in namespace_object {
        let Some(metadata_object) = metadata_value.as_object().cloned() else {
            return Err(HookMutationParseError::InvalidSchema {
                message: format!(
                    "parsed metadata entry for hook '{hook_name}' must be a JSON object"
                ),
            });
        };

        merge_hook_metadata_namespace(accumulated_metadata, hook_name, metadata_object)?;
    }

    Ok(())
}

pub fn merge_accumulated_hook_metadata_from_outcomes(
    accumulated_hook_metadata: &mut serde_json::Map<String, serde_json::Value>,
    outcomes: &[HookDispatchOutcome],
) {
    for outcome in outcomes {
        let HookMutationParseOutcome::Parsed {
            namespaced_metadata,
        } = &outcome.mutation_parse_outcome
        else {
            continue;
        };

        if let Err(error) =
            merge_namespaced_hook_metadata(accumulated_hook_metadata, namespaced_metadata)
        {
            warn!(
                phase_event = %outcome.phase_event,
                hook_name = %outcome.hook_name,
                error = ?error,
                "Failed to merge parsed hook mutation metadata; ignoring mutation output"
            );
        }
    }
}

pub fn mutation_parse_failure(
    mutation_parse_outcome: &HookMutationParseOutcome,
) -> Option<HookDispatchFailure> {
    let HookMutationParseOutcome::Invalid(error) = mutation_parse_outcome else {
        return None;
    };

    Some(HookDispatchFailure::InvalidMutationOutput {
        message: format_hook_mutation_parse_error(error),
    })
}
