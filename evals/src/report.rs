//! Report aggregation and Markdown rendering.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::bank::Domain;
use crate::error::EvalError;
use crate::runner::{EvalRow, EvalRun, PromptVariant};

/// Aggregated metrics rolled up per domain × variant.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DomainSummary {
    pub questions: usize,
    pub directional_conciseness_pass_rate: f32,
    pub mean_relevance: f32,
    pub mean_grounding: f32,
    pub mean_ttft_ms: f32,
    pub mean_stream_ms: f32,
    pub errors: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VariantSummary {
    pub overall: DomainSummary,
    pub by_domain: BTreeMap<Domain, DomainSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub run_id: String,
    pub prompt_version: String,
    pub variants: BTreeMap<PromptVariant, VariantSummary>,
}

impl Report {
    pub fn from_run(run: &EvalRun) -> Self {
        let mut variants: BTreeMap<PromptVariant, VariantSummary> = BTreeMap::new();
        for variant in collect_variants(&run.rows) {
            let rows: Vec<&EvalRow> = run.rows.iter().filter(|r| r.variant == variant).collect();
            let overall = summarise(&rows);
            let by_domain = rows
                .iter()
                .map(|r| r.domain)
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .map(|d| {
                    let in_domain: Vec<&EvalRow> =
                        rows.iter().copied().filter(|r| r.domain == d).collect();
                    (d, summarise(&in_domain))
                })
                .collect();
            variants.insert(
                variant,
                VariantSummary {
                    overall,
                    by_domain,
                },
            );
        }
        Self {
            run_id: run.run_id.clone(),
            prompt_version: run.prompt_version.clone(),
            variants,
        }
    }

    pub fn write_json(&self, path: &Path) -> Result<(), EvalError> {
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, json)?;
        Ok(())
    }

    pub fn write_markdown(&self, path: &Path) -> Result<(), EvalError> {
        fs::write(path, self.render_markdown())?;
        Ok(())
    }

    pub fn render_markdown(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let _ = writeln!(out, "# Flint eval report\n");
        let _ = writeln!(out, "Run: `{}`  ", self.run_id);
        let _ = writeln!(out, "Prompt version: `{}`\n", self.prompt_version);

        for (variant, summary) in &self.variants {
            let _ = writeln!(out, "## Variant: `{}`\n", variant.filename());
            let _ = writeln!(
                out,
                "Questions: {} · Errors: {} · Conciseness pass rate: {:.1}% · Relevance: {:.2} · Grounding: {:.2} · TTFT mean: {:.0}ms · Stream mean: {:.0}ms",
                summary.overall.questions,
                summary.overall.errors,
                summary.overall.directional_conciseness_pass_rate * 100.0,
                summary.overall.mean_relevance,
                summary.overall.mean_grounding,
                summary.overall.mean_ttft_ms,
                summary.overall.mean_stream_ms
            );

            let _ = writeln!(out, "\n| Domain | Q | Errors | Conciseness | Relevance | Grounding | TTFT (ms) |");
            let _ = writeln!(out, "|---|---|---|---|---|---|---|");
            for (domain, d) in &summary.by_domain {
                let _ = writeln!(
                    out,
                    "| {} | {} | {} | {:.1}% | {:.2} | {:.2} | {:.0} |",
                    domain.display(),
                    d.questions,
                    d.errors,
                    d.directional_conciseness_pass_rate * 100.0,
                    d.mean_relevance,
                    d.mean_grounding,
                    d.mean_ttft_ms
                );
            }
            let _ = writeln!(out);
        }
        out
    }
}

fn collect_variants(rows: &[EvalRow]) -> Vec<PromptVariant> {
    let mut seen = std::collections::BTreeSet::new();
    for row in rows {
        seen.insert(row.variant);
    }
    seen.into_iter().collect()
}

fn summarise(rows: &[&EvalRow]) -> DomainSummary {
    if rows.is_empty() {
        return DomainSummary::default();
    }

    let n = rows.len() as f32;
    let errors = rows.iter().filter(|r| r.error.is_some()).count();
    let conciseness_pass = rows.iter().filter(|r| r.conciseness.passed).count() as f32;

    let (mut rel_sum, mut rel_count) = (0.0_f32, 0_f32);
    let (mut ground_sum, mut ground_count) = (0.0_f32, 0_f32);
    let mut ttft_sum = 0.0_f32;
    let mut stream_sum = 0.0_f32;

    for row in rows {
        if let Some(j) = row.directional.judge {
            rel_sum += j.relevance;
            ground_sum += j.grounding;
            rel_count += 1.0;
            ground_count += 1.0;
        }
        ttft_sum += row.directional.latency.ttft_ms as f32;
        stream_sum += row.depth.latency.stream_complete_ms as f32;
    }

    DomainSummary {
        questions: rows.len(),
        directional_conciseness_pass_rate: conciseness_pass / n,
        mean_relevance: safe_div(rel_sum, rel_count),
        mean_grounding: safe_div(ground_sum, ground_count),
        mean_ttft_ms: ttft_sum / n,
        mean_stream_ms: stream_sum / n,
        errors,
    }
}

fn safe_div(numer: f32, denom: f32) -> f32 {
    if denom == 0.0 {
        0.0
    } else {
        numer / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::judge::JudgeScores;
    use crate::metrics::{score_conciseness, score_latency, score_structure};
    use crate::runner::ThreadScore;

    fn make_row(
        domain: Domain,
        variant: PromptVariant,
        directional: &str,
        relevance: f32,
        grounding: f32,
    ) -> EvalRow {
        EvalRow {
            question_id: "q".into(),
            domain,
            variant,
            directional: ThreadScore {
                response_text: directional.into(),
                latency: score_latency(500, 6_000),
                judge: Some(JudgeScores {
                    relevance,
                    grounding,
                }),
            },
            depth: ThreadScore {
                response_text: "depth.".into(),
                latency: score_latency(700, 7_000),
                judge: None,
            },
            conciseness: score_conciseness(directional),
            structure: score_structure("a\n\nb"),
            error: None,
        }
    }

    #[test]
    fn report_rolls_up_per_variant_and_domain() {
        let run = EvalRun {
            run_id: "r".into(),
            started_at: chrono::Utc::now(),
            prompt_version: "v1".into(),
            rows: vec![
                make_row(
                    Domain::SoftwareEngineering,
                    PromptVariant::Gpt,
                    "Short.",
                    0.9,
                    0.8,
                ),
                make_row(
                    Domain::SoftwareEngineering,
                    PromptVariant::Gpt,
                    "One. Two. Three. Four.",
                    0.4,
                    0.5,
                ),
            ],
        };
        let report = Report::from_run(&run);
        let gpt = report.variants.get(&PromptVariant::Gpt).unwrap();
        assert!((gpt.overall.mean_relevance - 0.65).abs() < 0.01);
        assert!((gpt.overall.directional_conciseness_pass_rate - 0.5).abs() < 0.01);
        assert!(gpt
            .by_domain
            .contains_key(&Domain::SoftwareEngineering));
    }

    #[test]
    fn empty_run_produces_empty_report() {
        let run = EvalRun {
            run_id: "r".into(),
            started_at: chrono::Utc::now(),
            prompt_version: "v1".into(),
            rows: vec![],
        };
        let report = Report::from_run(&run);
        assert!(report.variants.is_empty());
    }
}
