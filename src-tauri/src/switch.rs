//! Autoswitch decision logic: decide which audio endpoints should be default given
//! the headset presence, and apply those decisions to the OS.

use crate::app_state::{push_event, AppState};
use crate::models::{
    AudioEndpoint, DecisionAction, DiagnosticEvent, EndpointFlow, EndpointState, SwitchDecision,
};
use crate::rules::decide_switch;
use std::collections::HashSet;

/// Enumerate the current endpoints, compute which are available given the headset
/// presence, and run the priority rules against the saved config.
pub(crate) fn decide_current(
    state: &AppState,
    headset_connected: bool,
    has_status: bool,
) -> anyhow::Result<SwitchDecision> {
    let endpoints = state.audio.endpoints()?;
    let (render, capture) = available_ids(&endpoints, headset_connected, has_status);
    let config = state.config.lock().clone();
    let mut decision = decide_switch(&config, &render, &capture);
    let original_action_count = decision.actions.len();
    decision.actions.retain(|action| match action {
        DecisionAction::SetRenderDefault { endpoint_id } => {
            !is_default_for_all_roles(&endpoints, endpoint_id, EndpointFlow::Render)
        }
        DecisionAction::SetCaptureDefault { endpoint_id } => {
            !is_default_for_all_roles(&endpoints, endpoint_id, EndpointFlow::Capture)
        }
    });

    if original_action_count > 0 && decision.actions.is_empty() {
        decision.reason = format!("{}; already default", decision.reason);
    }

    Ok(decision)
}

/// An endpoint is available when it is Active and — for presence-tracked devices
/// (SteelSeries dongles) — only when we positively know the headset is connected.
/// When the connection state is unknown we keep tracked devices available so we
/// never switch away from a headset just because HID has not reported yet.
fn available_ids(
    endpoints: &[AudioEndpoint],
    headset_connected: bool,
    has_status: bool,
) -> (HashSet<String>, HashSet<String>) {
    let mut render = HashSet::new();
    let mut capture = HashSet::new();
    for endpoint in endpoints {
        if endpoint.state != EndpointState::Active {
            continue;
        }
        if endpoint.is_presence_tracked && has_status && !headset_connected {
            continue;
        }
        match endpoint.flow {
            EndpointFlow::Render => render.insert(endpoint.id.clone()),
            EndpointFlow::Capture => capture.insert(endpoint.id.clone()),
        };
    }
    (render, capture)
}

pub(crate) fn apply_decision(state: &AppState, decision: &SwitchDecision) -> anyhow::Result<()> {
    for action in &decision.actions {
        match action {
            DecisionAction::SetRenderDefault { endpoint_id } => {
                push_event(
                    state,
                    DiagnosticEvent::info(format!("Setting render default: {endpoint_id}")),
                );
                state.audio.set_default(endpoint_id, EndpointFlow::Render)?;
            }
            DecisionAction::SetCaptureDefault { endpoint_id } => {
                push_event(
                    state,
                    DiagnosticEvent::info(format!("Setting capture default: {endpoint_id}")),
                );
                state.audio.set_default(endpoint_id, EndpointFlow::Capture)?;
            }
        }
    }
    Ok(())
}

fn is_default_for_all_roles(
    endpoints: &[AudioEndpoint],
    endpoint_id: &str,
    flow: EndpointFlow,
) -> bool {
    endpoints
        .iter()
        .find(|endpoint| endpoint.id == endpoint_id && endpoint.flow == flow)
        .map(|endpoint| {
            endpoint.is_default_console
                && endpoint.is_default_multimedia
                && endpoint.is_default_communications
        })
        .unwrap_or(false)
}
