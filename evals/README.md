# Flint eval harness

Phase 7.2 deliverable. Runs the 200-question bank against every prompt
variant under `prompts/` and produces structured scores for relevance,
grounding, answer conciseness, visual structure, and latency.

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

- answer conciseness pass rate `< 95%`
- any per-domain mean relevance `< 0.70`
- win rate `< 50%` vs the stored baseline (unless `--skip-win-rate`)

The first run on a fresh repo skips the win-rate check (no baseline yet).

CI smoke runs use `--skip-win-rate` because the local Ollama judge has run-to-run
variance; full win-rate comparison applies on manual `--update-baseline` runs before
merging prompt changes.

> **Manual eval backlog (post `lpav` Part B merge):** the harness always executes
> completions through the local `OllamaProvider` regardless of `--variant` — there
> is no cloud-API-key path yet, so a `--variant gpt` run is really "the gpt.txt
> prompt wording, run on whatever Ollama model is pulled locally." A smoke run
> against `llama3.1:8b` on the new answer/visual prompts shows the harness
> plumbing works end to end (no errors, judge scores populate) but the answer
> conciseness pass rate lands well under the 95% floor — expected variance for a
> small local model against prompts tuned for GPT/Claude, not a code regression.
> Before trusting the regression gate against production variants, run the
> harness once with real GPT/Claude credentials wired into the provider and
> `--update-baseline` to establish a baseline scored under the new answer/visual
> architecture (the previous baseline was deleted in slice 28/29 of `lpav` since
> it scored the retired directional/depth/clarifying threads).

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
│   ├── metrics.rs    - Rule-based: answer conciseness, visual structure, latency
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
