use crate::models::{AppConfig, DecisionAction, DevicePref, FlowConfig, SwitchDecision};
use std::collections::HashSet;

#[cfg(test)]
use crate::models::EndpointFlow;

pub fn decide_switch(
    config: &AppConfig,
    available_render: &HashSet<String>,
    available_capture: &HashSet<String>,
) -> SwitchDecision {
    if !config.autoswitch_enabled {
        return SwitchDecision {
            reason: "Autoswitching is disabled".to_string(),
            actions: Vec::new(),
        };
    }

    let mut actions = Vec::new();
    let mut chosen = Vec::new();

    if let Some(pref) = top_available(&config.output, available_render) {
        actions.push(DecisionAction::SetRenderDefault {
            endpoint_id: pref.id.clone(),
        });
        chosen.push(format!("output -> {}", pref.name));
    }

    if let Some(pref) = top_available(&config.input, available_capture) {
        actions.push(DecisionAction::SetCaptureDefault {
            endpoint_id: pref.id.clone(),
        });
        chosen.push(format!("input -> {}", pref.name));
    }

    let reason = if chosen.is_empty() {
        "No available device matched the priority lists".to_string()
    } else {
        format!(
            "Selected highest-priority available device ({})",
            chosen.join(", ")
        )
    };

    SwitchDecision { reason, actions }
}

/// First non-excluded entry, in priority order, whose endpoint is currently available.
fn top_available<'a>(flow: &'a FlowConfig, available: &HashSet<String>) -> Option<&'a DevicePref> {
    flow.priorities
        .iter()
        .filter(|pref| !pref.excluded)
        .find(|pref| available.contains(&pref.id))
}

#[cfg(test)]
pub fn flow_for_action(action: &DecisionAction) -> EndpointFlow {
    match action {
        DecisionAction::SetRenderDefault { .. } => EndpointFlow::Render,
        DecisionAction::SetCaptureDefault { .. } => EndpointFlow::Capture,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pref(id: &str, excluded: bool) -> DevicePref {
        DevicePref {
            id: id.to_string(),
            name: id.to_string(),
            excluded,
        }
    }

    fn config() -> AppConfig {
        AppConfig {
            autoswitch_enabled: true,
            output: FlowConfig {
                priorities: vec![pref("headset-out", false), pref("speaker-out", false)],
            },
            input: FlowConfig {
                priorities: vec![pref("headset-mic", false), pref("fallback-mic", false)],
            },
            chatmix: Default::default(),
        }
    }

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn selects_highest_priority_available_device() {
        let render = set(&["headset-out", "speaker-out"]);
        let capture = set(&["headset-mic", "fallback-mic"]);
        let decision = decide_switch(&config(), &render, &capture);
        assert_eq!(
            decision.actions,
            vec![
                DecisionAction::SetRenderDefault {
                    endpoint_id: "headset-out".into()
                },
                DecisionAction::SetCaptureDefault {
                    endpoint_id: "headset-mic".into()
                }
            ]
        );
    }

    #[test]
    fn falls_through_when_top_priority_is_unavailable() {
        // Headset (top priority) is not available, e.g. disconnected SteelSeries.
        let render = set(&["speaker-out"]);
        let capture = set(&["fallback-mic"]);
        let decision = decide_switch(&config(), &render, &capture);
        assert_eq!(
            decision.actions,
            vec![
                DecisionAction::SetRenderDefault {
                    endpoint_id: "speaker-out".into()
                },
                DecisionAction::SetCaptureDefault {
                    endpoint_id: "fallback-mic".into()
                }
            ]
        );
    }

    #[test]
    fn excluded_devices_are_never_selected() {
        let mut config = config();
        config.output.priorities[0].excluded = true;
        let render = set(&["headset-out", "speaker-out"]);
        let decision = decide_switch(&config, &render, &HashSet::new());
        assert_eq!(
            decision.actions,
            vec![DecisionAction::SetRenderDefault {
                endpoint_id: "speaker-out".into()
            }]
        );
    }

    #[test]
    fn no_action_when_nothing_available() {
        let decision = decide_switch(&config(), &HashSet::new(), &HashSet::new());
        assert!(decision.actions.is_empty());
    }

    #[test]
    fn disabled_autoswitch_has_no_actions() {
        let mut config = config();
        config.autoswitch_enabled = false;
        let render = set(&["headset-out"]);
        assert!(decide_switch(&config, &render, &HashSet::new())
            .actions
            .is_empty());
    }

    #[test]
    fn each_flow_is_independent() {
        // Output available, input has nothing available.
        let render = set(&["headset-out"]);
        let decision = decide_switch(&config(), &render, &HashSet::new());
        assert_eq!(decision.actions.len(), 1);
        assert_eq!(flow_for_action(&decision.actions[0]), EndpointFlow::Render);
    }
}
