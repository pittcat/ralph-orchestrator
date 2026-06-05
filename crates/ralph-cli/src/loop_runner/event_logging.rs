use super::*;

/// Logs events parsed from output to the event history file.
///
/// When an event has no subscriber (orphan), also logs an `event.orphaned`
/// system event to help Ralph understand the misconfiguration.
pub fn log_events_from_output(
    logger: &mut EventLogger,
    iteration: u32,
    hat_id: &HatId,
    output: &str,
    registry: &ralph_core::HatRegistry,
    enabled: bool,
) {
    if !enabled {
        return;
    }

    let parser = EventParser::new();
    let events = parser.parse(output);

    for event in events {
        // Determine which hat will be triggered by this event
        let triggered = registry.find_by_trigger(event.topic.as_str());

        // Per spec: Log "Published {topic} -> triggers {hat}" at DEBUG level
        if let Some(triggered_hat) = triggered {
            debug!("Published {} -> triggers {}", event.topic, triggered_hat);
        } else {
            debug!(
                "Published {} -> no hat triggered (orphan event)",
                event.topic
            );

            // Emit event.orphaned system event so Ralph sees the problem
            // Collect valid events (all hat subscriptions except wildcards)
            let valid_events: Vec<String> = registry
                .all()
                .flat_map(|hat| hat.subscriptions.iter())
                .map(|t| t.as_str().to_string())
                .filter(|t| t != "*")
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();

            warn!(
                topic = %event.topic,
                source = %hat_id.as_str(),
                valid_events = ?valid_events,
                "Event has no subscriber - logging event.orphaned"
            );

            let orphan_event = Event::new(
                "event.orphaned",
                format!(
                    "Event '{}' has no subscriber hat. Valid events to publish: {:?}",
                    event.topic, valid_events
                ),
            )
            .with_source(hat_id.clone());

            let orphan_record =
                EventRecord::new(iteration, "loop", &orphan_event, None::<&HatId>, None);
            if let Err(e) = logger.log(&orphan_record) {
                warn!("Failed to log event.orphaned: {}", e);
            }
        }

        let phase = Some(registry.current_phase().to_string());
        let record = EventRecord::new(iteration, hat_id.to_string(), &event, triggered, phase);

        if let Err(e) = logger.log(&record) {
            warn!("Failed to log event {}: {}", event.topic, e);
        }
    }
}

pub fn log_accepted_events(
    logger: &mut EventLogger,
    iteration: u32,
    hat_id: &HatId,
    events: &[Event],
    registry: &ralph_core::HatRegistry,
) {
    for event in events {
        let triggered = registry.find_by_trigger(event.topic.as_str());
        if triggered.is_none() && !registry.has_subscriber(event.topic.as_str()) {
            let mut valid_events: Vec<_> = registry
                .all()
                .flat_map(|hat| hat.subscriptions.iter())
                .filter(|topic| topic.as_str() != "*")
                .map(|topic| topic.to_string())
                .collect();
            valid_events.sort();
            valid_events.dedup();

            let orphan_event = Event::new(
                "event.orphaned",
                format!(
                    "Event '{}' has no subscriber hat. Valid events to publish: {:?}",
                    event.topic, valid_events
                ),
            )
            .with_source(hat_id.clone());
            let orphan_record =
                EventRecord::new(iteration, "loop", &orphan_event, None::<&HatId>, None);
            if let Err(e) = logger.log(&orphan_record) {
                warn!("Failed to log event.orphaned: {}", e);
            }
        }

        let phase = Some(registry.current_phase().to_string());
        let record = EventRecord::new(iteration, hat_id.to_string(), event, triggered, phase);
        if let Err(e) = logger.log(&record) {
            warn!("Failed to log accepted event {}: {}", event.topic, e);
        }
    }
}

/// Logs the loop.terminate system event to the event history.
///
/// Per spec: loop.terminate is an observer-only event published on loop exit.
pub fn log_terminate_event(
    logger: &mut EventLogger,
    iteration: u32,
    event: &Event,
    phase: Option<String>,
) {
    // loop.terminate is published by the orchestrator, not a hat
    // No hat can trigger on it (it's observer-only)
    let record = EventRecord::new(iteration, "loop", event, None::<&HatId>, phase);

    if let Err(e) = logger.log(&record) {
        warn!("Failed to log loop.terminate event: {}", e);
    }
}
