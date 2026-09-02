//! Deterministic visual-contract checks and serializable benchmark envelope.

use serde::Serialize;

use super::theme;

#[derive(Debug, Serialize, PartialEq)]
#[allow(dead_code)]
pub struct VisualMetrics {
    pub viewport: [u32; 2],
    pub theme: &'static str,
    pub transition_budget_ms: u64,
    pub idle_repaint_requested: bool,
}

#[allow(dead_code)]
pub fn reference_metrics(width: u32, height: u32, dark: bool) -> VisualMetrics {
    VisualMetrics {
        viewport: [width, height],
        theme: if dark { "dark" } else { "light" },
        transition_budget_ms: theme::TRANSITION_MS,
        idle_repaint_requested: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_sizes_and_metrics_are_stable_json() {
        for (width, height) in [(400, 300), (1280, 800)] {
            let value = serde_json::to_value(reference_metrics(width, height, false)).unwrap();
            assert_eq!(value["viewport"], serde_json::json!([width, height]));
            assert_eq!(value["idle_repaint_requested"], false);
            assert!(value["transition_budget_ms"].as_u64().unwrap() <= 180);
        }
    }
}
