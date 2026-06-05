# Flint eval harness

Phase 7.2 deliverable. Runs the 200-question bank against every prompt
variant under `prompts/` and produces structured scores for relevance,
grounding, conciseness, depth structure, and latency.

## Quick start

```bash
# Smoke run: 5 questions, single variant, against a locally-running Ollama
cargo run -p evals --release -- \
  --questions-dir evals/questions \
  --prompts-dir prompts \
  --limit 5 \
  --variant gpt

# Full run across all three production variants
cargo run -p evals --release

# Update the stored baseline once a passing run is reviewed
cargo run -p evals --release -- --update-baseline
```

Reports are written to `evals/results/<short-id>.json` and
`evals/results/<short-id>.md`. The baseline lives at
`evals/results/baseline.json` and is the only result file checked into git.

## Regression gate

A run **fails** if any of the following hold:

- directional conciseness pass rate `< 95%`
- any per-domain mean relevance `< 0.70`
- win rate `< 50%` vs the stored baseline

The first run on a fresh repo skips the win-rate check (no baseline yet).

## Question bank

| File | Domain | Count |
|---|---|---|
| `questions/software_engineering.json` | Software engineering | 40 |
| `questions/product_management.json` | Product management | 30 |
| `questions/finance.json` | Finance | 25 |
| `questions/marketing.json` | Marketing | 25 |
| `questions/sales.json` | Sales | 20 |
| `questions/operations.json` | Operations | 20 |
| `questions/universal.json` | Universal | 40 |
| | **Total** | **200** |

Questions are sourced from canonical interview guides for tier 1
(FAANG / Big Tech), tier 2 (Stripe, Uber, Anthropic, Airbnb), and tier 3
(mid-cap, smaller companies) employers. Each question is tagged with a
domain and category so the report can call out per-segment regressions.

## Architecture

```
evals/
├── src/
│   ├── lib.rs        - module exports
│   ├── main.rs       - CLI driver (clap)
│   ├── bank.rs       - Question, QuestionBank, Domain, Category
│   ├── baseline.rs   - load/save baseline, archive per-run results
│   ├── error.rs      - EvalError
│   ├── gate.rs       - RegressionGate + violations
│   ├── judge.rs      - LLM-as-judge (Ollama-backed) for relevance + grounding
│   ├── metrics.rs    - Rule-based: conciseness, structure, latency
│   ├── report.rs     - Aggregation + Markdown/JSON writers
│   └── runner.rs     - Drives every (question x variant) pair
├── questions/        - 200 questions across 7 domains
├── prompts/eval_judge - Judge prompt template
└── results/          - Output (gitignored except baseline.json)
```

## CI integration

`.github/workflows/eval-prompts.yml` runs the harness on every PR that
touches `prompts/**` or `evals/**`. The job installs Ollama, pulls a
small model (`llama3.2:3b`), runs a 10-question smoke eval across all
production variants, and uploads the report as a workflow artifact.
