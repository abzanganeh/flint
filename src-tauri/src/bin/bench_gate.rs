//! Phase 7.3 — Performance benchmark gate.
//!
//! Reads the per-bench sample JSON criterion writes under
//! `target/criterion/<group>/<id>/new/sample.json`, computes P50/P95/P99 from
//! the raw iteration times, and gates the run against the NFR targets in
//! `flint-performance.mdc`.
//!
//! Exit codes:
//!   0 — every gate green, or only WARN gates breached.
//!   1 — at least one FAIL gate breached.
//!   2 — gate setup error (missing sample, parse failure, IO).
//!
//! Run with:
//!   cargo run --bin bench_gate -- \
//!     --criterion-dir target/criterion \
//!     --report-dir target/bench-report
//!
//! `println!` / `eprintln!` exemption: this is a standalone CLI binary used
//! by CI and developers; structured tracing is overkill here and would
//! require an extra subscriber. The Phase 2 "no print" rule applies to the
//! desktop application library code, not to bin/ CLI tools.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::cmp::Ordering;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

// ────────────────────────────────────────────────────────────────────────────
// Gate definitions — sourced from flint-performance.mdc.
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Severity {
    /// Breach must fail the PR.
    Fail,
    /// Breach is logged but does not fail the PR.
    Warn,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Severity::Fail => write!(f, "FAIL"),
            Severity::Warn => write!(f, "WARN"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Gate {
    label: &'static str,
    group: &'static str,
    id: &'static str,
    severity: Severity,
    /// CI gate threshold. Breach at or above this triggers the gate.
    p95_ms_threshold: f64,
    /// Documented NFR target — included in the report for context.
    nfr_target_ms: f64,
}

/// Phase 7.3 — performance gates. Order matters only for report readability.
const GATES: &[Gate] = &[
    Gate {
        label: "RAG retrieval P95 (1k chunks)",
        group: "rag_retrieval",
        id: "retrieve_top_k/1000",
        severity: Severity::Fail,
        p95_ms_threshold: 60.0,
        nfr_target_ms: 50.0,
    },
    Gate {
        label: "Question detection Pass 1 P95",
        group: "question_detection",
        id: "pass1_mixed_corpus",
        severity: Severity::Fail,
        p95_ms_threshold: 100.0,
        nfr_target_ms: 100.0,
    },
    Gate {
        label: "RNNoise per-frame P95 (denoise only)",
        group: "rnnoise_frame",
        id: "denoise_only",
        severity: Severity::Fail,
        p95_ms_threshold: 5.0,
        nfr_target_ms: 5.0,
    },
    Gate {
        label: "RNNoise + downsample P95",
        group: "rnnoise_frame",
        id: "denoise_plus_downsample",
        severity: Severity::Warn,
        p95_ms_threshold: 6.0,
        nfr_target_ms: 5.0,
    },
    Gate {
        label: "Orchestrator overhead-only TTFT P95",
        group: "orchestrator_ttft",
        id: "primary_to_first_token/0",
        severity: Severity::Warn,
        p95_ms_threshold: 50.0,
        nfr_target_ms: 800.0,
    },
    Gate {
        label: "Orchestrator TTFT P95 (50ms provider)",
        group: "orchestrator_ttft",
        id: "primary_to_first_token/50",
        severity: Severity::Fail,
        p95_ms_threshold: 900.0,
        nfr_target_ms: 800.0,
    },
    Gate {
        label: "Prompt load P95 (directional)",
        group: "prompt_loading",
        id: "directional__llama",
        severity: Severity::Warn,
        p95_ms_threshold: 5.0,
        nfr_target_ms: 5.0,
    },
    Gate {
        label: "Confidence scoring P95",
        group: "confidence_scoring",
        id: "compute_mixed",
        severity: Severity::Warn,
        p95_ms_threshold: 1.0,
        nfr_target_ms: 1.0,
    },
];

// ────────────────────────────────────────────────────────────────────────────
// Criterion sample.json shape.
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CriterionSample {
    iters: Vec<f64>,
    times: Vec<f64>,
}

// ────────────────────────────────────────────────────────────────────────────
// Report types.
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct BenchResult {
    label: String,
    group: String,
    id: String,
    severity: Severity,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    p95_ms_threshold: f64,
    nfr_target_ms: f64,
    breached: bool,
    sample_count: usize,
}

#[derive(Debug, Serialize)]
struct Report {
    timestamp_utc: String,
    git_sha: Option<String>,
    results: Vec<BenchResult>,
    fail_count: usize,
    warn_count: usize,
    missing: Vec<String>,
}

// ────────────────────────────────────────────────────────────────────────────
// CLI — hand-rolled so the bin stays free of clap on the production build.
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
struct Cli {
    criterion_dir: PathBuf,
    report_dir: PathBuf,
    strict: bool,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            criterion_dir: PathBuf::from("target/criterion"),
            report_dir: PathBuf::from("target/bench-report"),
            strict: false,
        }
    }
}

fn parse_cli(args: impl IntoIterator<Item = String>) -> Result<Cli> {
    let mut cli = Cli::default();
    let mut iter = args.into_iter().peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--criterion-dir" => {
                let val = iter
                    .next()
                    .ok_or_else(|| anyhow!("--criterion-dir requires a value"))?;
                cli.criterion_dir = PathBuf::from(val);
            }
            "--report-dir" => {
                let val = iter
                    .next()
                    .ok_or_else(|| anyhow!("--report-dir requires a value"))?;
                cli.report_dir = PathBuf::from(val);
            }
            "--strict" => cli.strict = true,
            "-h" | "--help" => {
                println!("{}", help_text());
                std::process::exit(0);
            }
            other if other.starts_with("--") => {
                return Err(anyhow!("unknown flag: {other}"));
            }
            _ => {}
        }
    }
    Ok(cli)
}

fn help_text() -> &'static str {
    "bench_gate — enforce Phase 7.3 NFR gates against criterion samples.\n\
     \n\
     USAGE:\n    \
         bench_gate [--criterion-dir DIR] [--report-dir DIR] [--strict]\n\
     \n\
     OPTIONS:\n    \
         --criterion-dir DIR   default target/criterion\n    \
         --report-dir DIR      default target/bench-report\n    \
         --strict              treat missing bench samples as failures\n    \
         -h, --help            show this help"
}

// ────────────────────────────────────────────────────────────────────────────
// Percentile helpers.
// ────────────────────────────────────────────────────────────────────────────

fn per_iteration_ns(sample: &CriterionSample) -> Vec<f64> {
    sample
        .iters
        .iter()
        .zip(sample.times.iter())
        .filter_map(|(iters, total_ns)| {
            if *iters > 0.0 {
                Some(total_ns / iters)
            } else {
                None
            }
        })
        .collect()
}

/// Nearest-rank percentile (NIST primary definition). For a sorted vector of
/// length `n`, the percentile at `p` is the value at the `ceil(n * p)`-th
/// position (1-indexed). Clamped to the valid index range so `p == 0` and
/// `p == 1` always resolve to the min and max respectively.
fn percentile_ns(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let n = sorted.len();
    let rank = (n as f64 * p).ceil() as usize;
    let idx = rank.saturating_sub(1).min(n - 1);
    sorted[idx]
}

fn ns_to_ms(ns: f64) -> f64 {
    ns / 1_000_000.0
}

// ────────────────────────────────────────────────────────────────────────────
// Gate execution.
// ────────────────────────────────────────────────────────────────────────────

fn evaluate_gate(criterion_dir: &Path, gate: Gate) -> Result<BenchResult> {
    let sample_path = criterion_dir
        .join(gate.group)
        .join(gate.id)
        .join("new")
        .join("sample.json");

    let raw = fs::read_to_string(&sample_path)
        .with_context(|| format!("missing sample file: {}", sample_path.display()))?;
    let sample: CriterionSample = serde_json::from_str(&raw)
        .with_context(|| format!("malformed sample json at {}", sample_path.display()))?;

    let mut times = per_iteration_ns(&sample);
    if times.is_empty() {
        return Err(anyhow!(
            "sample at {} contained no iterations",
            sample_path.display()
        ));
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));

    let p50 = ns_to_ms(percentile_ns(&times, 0.50));
    let p95 = ns_to_ms(percentile_ns(&times, 0.95));
    let p99 = ns_to_ms(percentile_ns(&times, 0.99));

    Ok(BenchResult {
        label: gate.label.to_string(),
        group: gate.group.to_string(),
        id: gate.id.to_string(),
        severity: gate.severity,
        p50_ms: p50,
        p95_ms: p95,
        p99_ms: p99,
        p95_ms_threshold: gate.p95_ms_threshold,
        nfr_target_ms: gate.nfr_target_ms,
        breached: p95 >= gate.p95_ms_threshold,
        sample_count: times.len(),
    })
}

fn build_report(criterion_dir: &Path, strict: bool) -> (Report, bool) {
    let mut results = Vec::with_capacity(GATES.len());
    let mut missing = Vec::new();
    let mut fail_count = 0_usize;
    let mut warn_count = 0_usize;
    let mut hard_failed = false;

    for gate in GATES {
        match evaluate_gate(criterion_dir, *gate) {
            Ok(res) => {
                if res.breached {
                    match res.severity {
                        Severity::Fail => {
                            fail_count += 1;
                            hard_failed = true;
                        }
                        Severity::Warn => warn_count += 1,
                    }
                }
                results.push(res);
            }
            Err(err) => {
                missing.push(format!("{}/{}: {err}", gate.group, gate.id));
                if strict && gate.severity == Severity::Fail {
                    hard_failed = true;
                }
            }
        }
    }

    let report = Report {
        timestamp_utc: Utc::now().to_rfc3339(),
        git_sha: std::env::var("GITHUB_SHA").ok(),
        results,
        fail_count,
        warn_count,
        missing,
    };

    (report, hard_failed)
}

// ────────────────────────────────────────────────────────────────────────────
// Report writers.
// ────────────────────────────────────────────────────────────────────────────

fn write_json(report: &Report, dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir).with_context(|| format!("mkdir {}", dir.display()))?;
    let path = dir.join("report.json");
    let body = serde_json::to_string_pretty(report)?;
    fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

fn write_markdown(report: &Report, dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join("report.md");
    let body = render_markdown(report);
    fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

fn render_markdown(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("# Flint Performance Benchmark Report\n\n");
    out.push_str(&format!("Timestamp: `{}`\n\n", report.timestamp_utc));
    if let Some(sha) = &report.git_sha {
        out.push_str(&format!("Commit: `{sha}`\n\n"));
    }
    out.push_str(&format!(
        "**FAIL gates breached:** {} &nbsp;&nbsp;|&nbsp;&nbsp; **WARN gates breached:** {}\n\n",
        report.fail_count, report.warn_count
    ));

    out.push_str(
        "| Gate | Severity | P50 ms | P95 ms | P99 ms | Threshold | NFR target | Status |\n",
    );
    out.push_str("|---|---|---:|---:|---:|---:|---:|---|\n");
    for r in &report.results {
        let status = if r.breached {
            format!("BREACHED ({})", r.severity)
        } else {
            "OK".to_string()
        };
        out.push_str(&format!(
            "| {} | {} | {:.3} | {:.3} | {:.3} | {:.2} | {:.2} | {} |\n",
            r.label,
            r.severity,
            r.p50_ms,
            r.p95_ms,
            r.p99_ms,
            r.p95_ms_threshold,
            r.nfr_target_ms,
            status,
        ));
    }

    if !report.missing.is_empty() {
        out.push_str("\n## Missing samples\n\n");
        for m in &report.missing {
            out.push_str(&format!("- {m}\n"));
        }
    }
    out
}

// ────────────────────────────────────────────────────────────────────────────
// Entry point.
// ────────────────────────────────────────────────────────────────────────────

fn main() -> ExitCode {
    let cli = match parse_cli(std::env::args().skip(1)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("argument error: {e}\n\n{}", help_text());
            return ExitCode::from(2);
        }
    };

    let (report, hard_failed) = build_report(&cli.criterion_dir, cli.strict);

    match write_json(&report, &cli.report_dir) {
        Ok(p) => eprintln!("wrote {}", p.display()),
        Err(e) => {
            eprintln!("failed to write JSON report: {e:?}");
            return ExitCode::from(2);
        }
    }
    match write_markdown(&report, &cli.report_dir) {
        Ok(p) => eprintln!("wrote {}", p.display()),
        Err(e) => {
            eprintln!("failed to write Markdown report: {e:?}");
            return ExitCode::from(2);
        }
    }

    eprintln!("{}", render_markdown(&report));

    if hard_failed {
        eprintln!("HARD-FAIL: at least one performance gate breached.");
        ExitCode::from(1)
    } else {
        eprintln!("All FAIL gates green.");
        ExitCode::from(0)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests.
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn dummy_sample(per_iter_ns: &[f64]) -> CriterionSample {
        CriterionSample {
            iters: vec![1.0; per_iter_ns.len()],
            times: per_iter_ns.to_vec(),
        }
    }

    fn write_sample_json(dir: &Path, group: &str, id: &str, sample: &CriterionSample) {
        let path = dir.join(group).join(id).join("new");
        fs::create_dir_all(&path).unwrap();
        let body = serde_json::json!({
            "iters": sample.iters,
            "times": sample.times,
        });
        fs::write(path.join("sample.json"), body.to_string()).unwrap();
    }

    #[test]
    fn per_iteration_divides_total_time_by_iter_count() {
        let sample = CriterionSample {
            iters: vec![10.0, 20.0],
            times: vec![1_000.0, 4_000.0],
        };
        let per_iter = per_iteration_ns(&sample);
        assert_eq!(per_iter, vec![100.0, 200.0]);
    }

    #[test]
    fn per_iteration_skips_zero_iter_entries() {
        let sample = CriterionSample {
            iters: vec![0.0, 5.0],
            times: vec![999.0, 1_000.0],
        };
        assert_eq!(per_iteration_ns(&sample), vec![200.0]);
    }

    #[test]
    fn percentile_handles_empty_and_full() {
        assert_eq!(percentile_ns(&[], 0.95), 0.0);
        let v: Vec<f64> = (1..=100).map(|i| i as f64).collect();
        assert_eq!(percentile_ns(&v, 0.50), 50.0);
        assert_eq!(percentile_ns(&v, 0.95), 95.0);
        assert_eq!(percentile_ns(&v, 0.99), 99.0);
    }

    #[test]
    fn ns_to_ms_converts() {
        assert!((ns_to_ms(1_500_000.0) - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn evaluate_gate_reads_sample_and_computes_p95() {
        let tmp = tempdir().unwrap();
        let sample = dummy_sample(&[1_000_000.0; 100]); // 1ms each
        write_sample_json(tmp.path(), "rag_retrieval", "retrieve_top_k/1000", &sample);
        let gate = GATES[0];
        let res = evaluate_gate(tmp.path(), gate).unwrap();
        assert!((res.p95_ms - 1.0).abs() < 0.01);
        assert!(!res.breached);
        assert_eq!(res.sample_count, 100);
    }

    #[test]
    fn evaluate_gate_flags_breach_when_p95_above_threshold() {
        let tmp = tempdir().unwrap();
        // 99 fast iterations + one absurdly slow one — pushes P95 above 60 ms.
        let mut times = vec![1_000_000.0_f64; 90];
        times.extend(vec![80_000_000.0_f64; 10]); // 80ms each
        let sample = dummy_sample(&times);
        write_sample_json(tmp.path(), "rag_retrieval", "retrieve_top_k/1000", &sample);
        let gate = GATES[0];
        let res = evaluate_gate(tmp.path(), gate).unwrap();
        assert!(res.p95_ms > 60.0);
        assert!(res.breached);
    }

    #[test]
    fn build_report_counts_fail_and_warn_breaches() {
        let tmp = tempdir().unwrap();
        // Seed every gate with a sample so we exercise the full pipeline.
        for gate in GATES {
            let sample = dummy_sample(&[500_000.0; 100]); // 0.5 ms — green for all
            write_sample_json(tmp.path(), gate.group, gate.id, &sample);
        }
        let (report, hard_failed) = build_report(tmp.path(), false);
        assert_eq!(report.results.len(), GATES.len());
        assert_eq!(report.fail_count, 0);
        assert!(!hard_failed);
    }

    #[test]
    fn build_report_hard_fails_when_fail_severity_breached() {
        let tmp = tempdir().unwrap();
        for gate in GATES {
            let times = if gate.severity == Severity::Fail {
                // Push every FAIL gate over its threshold.
                vec![(gate.p95_ms_threshold + 10.0) * 1_000_000.0; 100]
            } else {
                vec![500_000.0; 100]
            };
            let sample = dummy_sample(&times);
            write_sample_json(tmp.path(), gate.group, gate.id, &sample);
        }
        let (report, hard_failed) = build_report(tmp.path(), false);
        assert!(hard_failed);
        assert!(report.fail_count > 0);
    }

    #[test]
    fn render_markdown_contains_all_gates() {
        let report = Report {
            timestamp_utc: "2026-06-04T00:00:00Z".to_string(),
            git_sha: Some("abc123".to_string()),
            results: vec![BenchResult {
                label: "RAG retrieval P95 (1k chunks)".to_string(),
                group: "rag_retrieval".to_string(),
                id: "retrieve_top_k/1000".to_string(),
                severity: Severity::Fail,
                p50_ms: 12.0,
                p95_ms: 45.0,
                p99_ms: 55.0,
                p95_ms_threshold: 60.0,
                nfr_target_ms: 50.0,
                breached: false,
                sample_count: 100,
            }],
            fail_count: 0,
            warn_count: 0,
            missing: vec![],
        };
        let md = render_markdown(&report);
        assert!(md.contains("RAG retrieval"));
        assert!(md.contains("abc123"));
        assert!(md.contains("OK"));
    }

    #[test]
    fn parse_cli_accepts_known_flags() {
        let cli = parse_cli(
            [
                "--criterion-dir",
                "/tmp/c",
                "--report-dir",
                "/tmp/r",
                "--strict",
            ]
            .into_iter()
            .map(String::from),
        )
        .unwrap();
        assert_eq!(cli.criterion_dir, PathBuf::from("/tmp/c"));
        assert_eq!(cli.report_dir, PathBuf::from("/tmp/r"));
        assert!(cli.strict);
    }

    #[test]
    fn parse_cli_rejects_unknown_flag() {
        let err = parse_cli(["--bogus".to_string()]).unwrap_err();
        assert!(err.to_string().contains("unknown flag"));
    }

    #[test]
    fn missing_sample_records_in_report() {
        let tmp = tempdir().unwrap();
        // Only seed half the gates.
        for gate in GATES.iter().take(2) {
            let sample = dummy_sample(&[500_000.0; 50]);
            write_sample_json(tmp.path(), gate.group, gate.id, &sample);
        }
        let (report, _) = build_report(tmp.path(), false);
        assert!(!report.missing.is_empty());
    }
}
