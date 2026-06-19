//! Regression gate enforced on every eval run.
//!
//! From `flint-performance.mdc` and design doc §20:
//!
//! * win rate >= 50% vs the stored baseline
//! * directional conciseness pass rate >= 95%
//! * no per-domain relevance score below 0.7
//!
//! The gate returns structured findings — the CLI prints them and exits 1
//! if any are violated. CI parses the JSON output and fails the PR.

use serde::{Deserialize, Serialize};

use crate::bank::Domain;
use crate::report::{Report, VariantSummary};
use crate::runner::PromptVariant;

/// Thresholds — single source of truth so the rule and the test match.
const WIN_RATE_FLOOR: f32 = 0.50;
const CONCISENESS_FLOOR: f32 = 0.95;
const DOMAIN_RELEVANCE_FLOOR: f32 = 0.70;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateOutcome {
    pub passed: bool,
    pub violations: Vec<Violation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Violation {
    Conciseness {
        variant: PromptVariant,
        pass_rate: f32,
    },
    DomainRelevance {
        variant: PromptVariant,
        domain: Domain,
        relevance: f32,
    },
    WinRate {
        variant: PromptVariant,
        win_rate: f32,
    },
}

/// Evaluate a freshly-generated report against an optional baseline.
///
/// `baseline` may be `None` for the first run — only the absolute gates
/// (conciseness, domain relevance) are checked in that case.
pub fn evaluate(report: &Report, baseline: Option<&Report>) -> GateOutcome {
    let mut violations = Vec::new();

    for (variant, summary) in &report.variants {
        check_conciseness(*variant, summary, &mut violations);
        check_domain_relevance(*variant, summary, &mut violations);

        if let Some(base) = baseline {
            if let Some(base_summary) = base.variants.get(variant) {
                check_win_rate(*variant, summary, base_summary, &mut violations);
            }
        }
    }

    GateOutcome {
        passed: violations.is_empty(),
        violations,
    }
}

fn check_conciseness(variant: PromptVariant, summary: &VariantSummary, out: &mut Vec<Violation>) {
    if summary.overall.directional_conciseness_pass_rate < CONCISENESS_FLOOR {
        out.push(Violation::Conciseness {
            variant,
            pass_rate: summary.overall.directional_conciseness_pass_rate,
        });
    }
}

fn check_domain_relevance(
    variant: PromptVariant,
    summary: &VariantSummary,
    out: &mut Vec<Violation>,
) {
    for (domain, d) in &summary.by_domain {
        // Skip domains where no question had a judge score — counted as N/A.
        if d.mean_relevance == 0.0 {
            continue;
        }
        if d.mean_relevance < DOMAIN_RELEVANCE_FLOOR {
            out.push(Violation::DomainRelevance {
                variant,
                domain: *domain,
                relevance: d.mean_relevance,
            });
        }
    }
}

/// Win rate against the baseline is approximated by comparing per-variant
/// mean relevance. A variant "wins" if its relevance is >= baseline's; the
/// gate fails when the win rate across compared variants falls below 50%.
fn check_win_rate(
    variant: PromptVariant,
    summary: &VariantSummary,
    base_summary: &VariantSummary,
    out: &mut Vec<Violation>,
) {
    let mut comparable = 0;
    let mut wins = 0;
    for (domain, d) in &summary.by_domain {
        if let Some(b) = base_summary.by_domain.get(domain) {
            comparable += 1;
            if d.mean_relevance >= b.mean_relevance {
                wins += 1;
            }
        }
    }
    if comparable == 0 {
        return;
    }
    let win_rate = wins as f32 / comparable as f32;
    if win_rate < WIN_RATE_FLOOR {
        out.push(Violation::WinRate { variant, win_rate });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::DomainSummary;
    use std::collections::BTreeMap;

    fn summary_with(overall_conciseness: f32, domains: Vec<(Domain, f32)>) -> VariantSummary {
        let mut by_domain = BTreeMap::new();
        for (d, rel) in domains {
            by_domain.insert(
                d,
                DomainSummary {
                    questions: 10,
                    directional_conciseness_pass_rate: overall_conciseness,
                    mean_relevance: rel,
                    mean_grounding: 0.7,
                    mean_ttft_ms: 500.0,
                    mean_stream_ms: 6000.0,
                    errors: 0,
                },
            );
        }
        VariantSummary {
            overall: DomainSummary {
                questions: 10,
                directional_conciseness_pass_rate: overall_conciseness,
                mean_relevance: 0.8,
                mean_grounding: 0.7,
                mean_ttft_ms: 500.0,
                mean_stream_ms: 6000.0,
                errors: 0,
            },
            by_domain,
        }
    }

    fn report_with(variant: PromptVariant, vs: VariantSummary) -> Report {
        let mut variants = BTreeMap::new();
        variants.insert(variant, vs);
        Report {
            run_id: "r".into(),
            prompt_version: "v".into(),
            variants,
        }
    }

    #[test]
    fn passes_when_all_floors_met_and_no_baseline() {
        let r = report_with(
            PromptVariant::Gpt,
            summary_with(0.96, vec![(Domain::SoftwareEngineering, 0.85)]),
        );
        let outcome = evaluate(&r, None);
        assert!(outcome.passed);
    }

    #[test]
    fn flags_conciseness_floor_breach() {
        let r = report_with(
            PromptVariant::Gpt,
            summary_with(0.5, vec![(Domain::SoftwareEngineering, 0.85)]),
        );
        let outcome = evaluate(&r, None);
        assert!(!outcome.passed);
        assert!(matches!(
            outcome.violations[0],
            Violation::Conciseness { .. }
        ));
    }

    #[test]
    fn flags_domain_relevance_below_floor() {
        let r = report_with(
            PromptVariant::Gpt,
            summary_with(0.96, vec![(Domain::SoftwareEngineering, 0.5)]),
        );
        let outcome = evaluate(&r, None);
        assert!(!outcome.passed);
        assert!(matches!(
            outcome.violations[0],
            Violation::DomainRelevance { .. }
        ));
    }

    #[test]
    fn flags_win_rate_below_50_against_baseline() {
        let current = report_with(
            PromptVariant::Gpt,
            summary_with(
                0.96,
                vec![
                    (Domain::SoftwareEngineering, 0.6),
                    (Domain::ProductManagement, 0.65),
                ],
            ),
        );
        let baseline = report_with(
            PromptVariant::Gpt,
            summary_with(
                0.96,
                vec![
                    (Domain::SoftwareEngineering, 0.9),
                    (Domain::ProductManagement, 0.9),
                ],
            ),
        );
        let outcome = evaluate(&current, Some(&baseline));
        assert!(!outcome.passed);
        assert!(outcome
            .violations
            .iter()
            .any(|v| matches!(v, Violation::WinRate { .. })));
    }
}
