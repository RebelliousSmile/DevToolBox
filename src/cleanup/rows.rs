//! Per-module aggregation of plan candidates.
//!
//! A walking module (npm cache, pip cache…) yields many candidates; the
//! view shows one row per module. No module name is ever hardcoded — rows
//! echo whatever names the script emits, so the same code serves the
//! Windows and Linux module sets.

use super::model::CleanupPlan;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ModuleRow {
    pub module: String,
    pub level: String,
    /// Sum of the *measured* candidates, mirroring the script's `sum_known`:
    /// `None` only when nothing was measurable, never a lying zero.
    pub estimated: Option<u64>,
    /// `true` when at least one candidate was non-measurable, so the sum is
    /// a floor (« ≥ taille »), matching the script's `unpriced_modules`.
    pub partially_measured: bool,
    pub candidate_count: usize,
    pub paths: Vec<String>,
    pub needs_network: bool,
}

/// Group candidates by module, sorted by size descending then module name.
/// Unmeasured rows (`estimated: None`) sort last.
pub fn module_rows(plan: &CleanupPlan) -> Vec<ModuleRow> {
    let mut rows: Vec<ModuleRow> = Vec::new();
    let mut by_module: HashMap<&str, usize> = HashMap::new();
    for candidate in &plan.candidates {
        let index = *by_module.entry(&candidate.module).or_insert_with(|| {
            rows.push(ModuleRow {
                module: candidate.module.clone(),
                level: candidate.level.clone(),
                estimated: None,
                partially_measured: false,
                candidate_count: 0,
                paths: Vec::new(),
                needs_network: false,
            });
            rows.len() - 1
        });
        let row = &mut rows[index];
        row.candidate_count += 1;
        row.needs_network |= candidate.needs_network;
        if let Some(path) = &candidate.path {
            row.paths.push(path.clone());
        }
        match candidate.estimated_bytes {
            Some(bytes) => *row.estimated.get_or_insert(0) += bytes,
            None => row.partially_measured = true,
        }
    }
    rows.sort_by(|a, b| {
        b.estimated
            .unwrap_or(0)
            .cmp(&a.estimated.unwrap_or(0))
            .then_with(|| a.module.cmp(&b.module))
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::model::Candidate;

    fn candidate(module: &str, bytes: Option<u64>) -> Candidate {
        Candidate {
            module: module.to_string(),
            path: Some(format!("C:\\{module}\\entry")),
            label: module.to_string(),
            estimated_bytes: bytes,
            level: "safe".to_string(),
            needs_network: false,
        }
    }

    fn plan(candidates: Vec<Candidate>) -> CleanupPlan {
        CleanupPlan {
            level: "moderate".to_string(),
            apply: false,
            candidates,
            total_estimated_bytes: None,
            unpriced_modules: Vec::new(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn multi_candidate_module_sums_known_sizes_and_collects_paths() {
        let rows = module_rows(&plan(vec![
            candidate("npm-cache", Some(100)),
            candidate("npm-cache", Some(200)),
        ]));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].estimated, Some(300));
        assert_eq!(rows[0].candidate_count, 2);
        assert_eq!(rows[0].paths.len(), 2);
        assert!(!rows[0].partially_measured);
    }

    #[test]
    fn unmeasured_candidate_raises_the_partial_flag_without_zeroing_the_sum() {
        let rows = module_rows(&plan(vec![
            candidate("uv-cache", Some(500)),
            candidate("uv-cache", None),
        ]));
        assert_eq!(rows[0].estimated, Some(500));
        assert!(rows[0].partially_measured);
    }

    #[test]
    fn fully_unmeasured_module_stays_none_never_zero() {
        let rows = module_rows(&plan(vec![candidate("recycle-bin", None)]));
        assert_eq!(rows[0].estimated, None);
        assert!(rows[0].partially_measured);
    }

    #[test]
    fn rows_sort_by_size_descending_then_name_with_unmeasured_last() {
        let rows = module_rows(&plan(vec![
            candidate("small", Some(10)),
            candidate("unknown", None),
            candidate("big", Some(1000)),
            candidate("alpha-ten", Some(10)),
        ]));
        let order: Vec<&str> = rows.iter().map(|row| row.module.as_str()).collect();
        assert_eq!(order, ["big", "alpha-ten", "small", "unknown"]);
    }

    #[test]
    fn needs_network_propagates_from_any_candidate() {
        let mut networked = candidate("pnpm-store", Some(1));
        networked.needs_network = true;
        let rows = module_rows(&plan(vec![candidate("pnpm-store", Some(1)), networked]));
        assert!(rows[0].needs_network);
    }
}
