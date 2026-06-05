//! Eval harness CLI.
//!
//! ```text
//! cargo run -p evals -- \
//!   --questions-dir evals/questions \
//!   --prompts-dir prompts \
//!   --results-dir evals/results \
//!   --domain software_engineering \
//!   --limit 5 \
//!   --variant gpt
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use flint_lib::llm::ollama::OllamaProvider;
use flint_lib::llm::provider::LLMProvider;
use tracing::{error, info};

use evals::bank::{select_questions, Domain, QuestionBank};
use evals::baseline::BaselineStore;
use evals::gate::evaluate;
use evals::judge::OllamaJudge;
use evals::report::Report;
use evals::runner::{EvalRunner, EvalRunnerConfig, PromptVariant};

#[derive(Debug, Parser)]
#[command(name = "evals", version, about = "Flint prompt evaluation harness")]
struct Cli {
    /// Directory containing per-domain question JSON files.
    #[arg(long, default_value = "evals/questions")]
    questions_dir: PathBuf,

    /// Directory containing the versioned prompt templates.
    #[arg(long, default_value = "prompts")]
    prompts_dir: PathBuf,

    /// Directory where per-run results and baseline are persisted.
    #[arg(long, default_value = "evals/results")]
    results_dir: PathBuf,

    /// Restrict evaluation to a single domain (defaults to all).
    #[arg(long)]
    domain: Option<DomainArg>,

    /// Cap the number of questions evaluated. Useful for smoke runs.
    #[arg(long)]
    limit: Option<usize>,

    /// Restrict to a single prompt variant. Default runs all production
    /// variants (gpt, claude, llama).
    #[arg(long)]
    variant: Option<VariantArg>,

    /// Save the report as the new baseline if the gate passes.
    #[arg(long)]
    update_baseline: bool,

    /// Skip the regression gate (used for first runs).
    #[arg(long)]
    no_gate: bool,
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum DomainArg {
    SoftwareEngineering,
    ProductManagement,
    Finance,
    Marketing,
    Sales,
    Operations,
    Universal,
}

impl From<DomainArg> for Domain {
    fn from(value: DomainArg) -> Self {
        match value {
            DomainArg::SoftwareEngineering => Domain::SoftwareEngineering,
            DomainArg::ProductManagement => Domain::ProductManagement,
            DomainArg::Finance => Domain::Finance,
            DomainArg::Marketing => Domain::Marketing,
            DomainArg::Sales => Domain::Sales,
            DomainArg::Operations => Domain::Operations,
            DomainArg::Universal => Domain::Universal,
        }
    }
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum VariantArg {
    Gpt,
    Claude,
    Llama,
}

impl From<VariantArg> for PromptVariant {
    fn from(value: VariantArg) -> Self {
        match value {
            VariantArg::Gpt => PromptVariant::Gpt,
            VariantArg::Claude => PromptVariant::Claude,
            VariantArg::Llama => PromptVariant::Llama,
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    let cli = Cli::parse();

    match run(cli).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            error!(error = %e, "eval run failed");
            ExitCode::from(2)
        }
    }
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

async fn run(cli: Cli) -> anyhow::Result<bool> {
    let bank = QuestionBank::load(&cli.questions_dir)
        .with_context(|| format!("loading bank from {}", cli.questions_dir.display()))?;
    info!(total = bank.total(), "question bank loaded");
    if bank.total() == 0 {
        anyhow::bail!(
            "question bank is empty at {} — seed at least one domain file before running",
            cli.questions_dir.display()
        );
    }

    let questions = select_questions(&bank, cli.domain.map(Into::into), cli.limit);
    info!(count = questions.len(), "running eval on selected questions");

    let provider: Arc<dyn LLMProvider> =
        Arc::new(OllamaProvider::new().context("constructing Ollama provider")?);

    let judge = Arc::new(OllamaJudge::new(provider.clone(), Some(&cli.prompts_dir))?);

    let variants = match cli.variant {
        Some(v) => vec![v.into()],
        None => PromptVariant::PRODUCTION_VARIANTS.to_vec(),
    };

    let runner = EvalRunner::new(EvalRunnerConfig {
        prompts_dir: cli.prompts_dir.clone(),
        provider,
        judge,
        variants,
        max_concurrent: 1,
    });

    let run = runner.run(&questions).await?;
    info!(rows = run.rows.len(), "run complete");

    let report = Report::from_run(&run);

    std::fs::create_dir_all(&cli.results_dir)?;
    let short_id: String = run.run_id.chars().take(8).collect();
    let json_path = cli.results_dir.join(format!("{short_id}.json"));
    let md_path = cli.results_dir.join(format!("{short_id}.md"));
    report.write_json(&json_path)?;
    report.write_markdown(&md_path)?;
    info!(json = %json_path.display(), md = %md_path.display(), "report written");

    let baseline_store = BaselineStore::new(&cli.results_dir);
    let baseline = baseline_store.load()?;
    let gate_passed = if cli.no_gate {
        info!("regression gate skipped (--no-gate)");
        true
    } else {
        let outcome = evaluate(&report, baseline.as_ref());
        if outcome.passed {
            info!("regression gate passed");
        } else {
            error!("regression gate failed");
            for v in &outcome.violations {
                error!(violation = ?v);
            }
        }
        outcome.passed
    };

    if gate_passed && cli.update_baseline {
        baseline_store.save(&report)?;
        info!(path = %cli.results_dir.join("baseline.json").display(), "baseline updated");
    }

    Ok(gate_passed)
}
