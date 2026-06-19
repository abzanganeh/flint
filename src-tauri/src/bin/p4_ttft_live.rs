//! P4 manual QA — live directional TTFT measurement loop.
//!
//! Runs sequential directional-style prompts against a real LLM provider (one
//! stream per iteration — no parallel depth/clarifying) to avoid Groq 429 noise
//! from the full orchestrator firing three threads at once.
//!
//! Usage:
//!   cargo run --bin p4_ttft_live -- --provider groq --runs 15 --delay-secs 8

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use flint_lib::llm::anthropic::AnthropicProvider;
use flint_lib::llm::deepseek::new_deepseek;
use flint_lib::llm::groq::GroqProvider;
use flint_lib::llm::openai::new_openai;
use flint_lib::llm::provider::{CompletionConfig, LLMProvider};
use flint_lib::orchestrator::load_prompt;
use futures::StreamExt;
use secrecy::SecretString;

const TTFT_GATE_MS: u64 = 900;

const QUESTIONS: &[&str] = &[
    "Walk me through how you would design a rate limiter for a public API.",
    "Tell me about a time you had to deliver under a tight deadline.",
    "How do you handle disagreement with a senior engineer on a technical decision?",
    "Explain the difference between SQL and NoSQL for a high-write analytics pipeline.",
    "Describe a production incident you owned from detection to resolution.",
    "How would you prioritize technical debt versus new features this quarter?",
    "What is your approach to mentoring junior developers on the team?",
    "How do you ensure code quality in a fast-moving startup environment?",
    "Tell me about a project where you improved system reliability or latency.",
    "How would you debug intermittent 500 errors in a microservices architecture?",
    "What trade-offs would you consider when choosing between Kafka and RabbitMQ?",
    "Describe how you gather requirements from non-technical stakeholders.",
    "How do you stay current with new tools and frameworks in your stack?",
    "Tell me about a time you had to say no to a feature request and why.",
    "How would you design an idempotent payment processing workflow?",
];

struct Args {
    provider: String,
    runs: usize,
    delay_secs: u64,
}

fn parse_args() -> Result<Args> {
    let mut provider = String::from("groq");
    let mut runs = 15usize;
    let mut delay_secs = 8u64;
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < raw.len() {
        match raw[i].as_str() {
            "--provider" => {
                i += 1;
                provider = raw
                    .get(i)
                    .context("--provider requires a value")?
                    .clone();
            }
            "--runs" => {
                i += 1;
                runs = raw
                    .get(i)
                    .context("--runs requires a value")?
                    .parse()
                    .context("--runs must be a positive integer")?;
            }
            "--delay-secs" => {
                i += 1;
                delay_secs = raw
                    .get(i)
                    .context("--delay-secs requires a value")?
                    .parse()
                    .context("--delay-secs must be a non-negative integer")?;
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => bail!("unknown argument: {other} (try --help)"),
        }
        i += 1;
    }
    if runs == 0 {
        bail!("--runs must be at least 1");
    }
    Ok(Args {
        provider,
        runs,
        delay_secs,
    })
}

fn print_usage() {
    eprintln!(
        "p4_ttft_live — sequential directional TTFT for manual P4 gate\n\
         \n\
         cargo run --bin p4_ttft_live -- [OPTIONS]\n\
         \n\
         Options:\n\
           --provider groq|deepseek|openai|anthropic   (default: groq)\n\
           --runs N                                  (default: 15)\n\
           --delay-secs N                            pause between runs (default: 8)\n"
    );
}

fn load_dotenv() {
    let env_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.env");
    let _ = dotenvy::from_path(env_path);
}

fn env_key(name: &str) -> Result<SecretString> {
    std::env::var(name)
        .with_context(|| format!("missing {name} — set in Flint/.env or export it"))
        .map(SecretString::new)
}

fn resolve_provider(name: &str) -> Result<Arc<dyn LLMProvider>> {
    match name {
        "groq" => {
            let key = env_key("GROQ_API_KEY")?;
            Ok(Arc::new(GroqProvider::new(key)?))
        }
        "deepseek" => {
            let key = env_key("DEEPSEEK_API_KEY")?;
            Ok(Arc::new(new_deepseek(key)?))
        }
        "openai" => {
            let key = env_key("OPENAI_API_KEY")?;
            Ok(Arc::new(new_openai(key)?))
        }
        "anthropic" => {
            let key = env_key("ANTHROPIC_API_KEY")?;
            Ok(Arc::new(AnthropicProvider::new(key)?))
        }
        other => bail!("unsupported provider: {other}"),
    }
}

fn prompts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../prompts")
}

fn build_directional_prompt(provider_name: &str, question: &str) -> Result<String> {
    let template = load_prompt("directional", provider_name, &prompts_dir())?;
    Ok(template
        .replace("{session_domain}", "Software Engineering")
        .replace("{rag_chunks}", "[1] Led platform reliability work on a distributed API.")
        .replace("{qa_chunks}", "")
        .replace("{rolling_summary_if_compressed}", "")
        .replace("{last_n_turns}", "")
        .replace("{question}", question)
        .replace("{role}", "Senior Software Engineer")
        .replace("{key_skills}", "Rust, distributed systems, observability"))
}

fn percentile(sorted: &[u64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let n = sorted.len();
    let rank = (n as f64 * p).ceil() as usize;
    let idx = rank.saturating_sub(1).min(n - 1);
    sorted[idx] as f64
}

async fn measure_ttft(provider: &dyn LLMProvider, prompt: String) -> Result<u64> {
    let config = CompletionConfig {
        max_tokens: Some(200),
        temperature: 0.0,
        stream: true,
    };

    let started = Instant::now();
    let mut stream = provider.complete_stream(prompt, config).await?;
    let first = stream
        .next()
        .await
        .context("stream ended before first token")??;
    let ttft_ms = started.elapsed().as_millis() as u64;
    let _ = first;
    Ok(ttft_ms)
}

#[tokio::main]
async fn main() -> Result<()> {
    load_dotenv();
    let args = parse_args()?;
    let provider = resolve_provider(&args.provider)?;
    let provider_name = provider.name().to_string();
    let question_count = args.runs.min(QUESTIONS.len());

    println!(
        "P4 TTFT live — provider={provider_name} runs={question_count} delay={}s gate={TTFT_GATE_MS}ms",
        args.delay_secs
    );
    println!("---");

    let mut samples: Vec<u64> = Vec::with_capacity(question_count);

    for (run, question) in QUESTIONS.iter().take(question_count).enumerate() {
        if run > 0 && args.delay_secs > 0 {
            tokio::time::sleep(Duration::from_secs(args.delay_secs)).await;
        }

        let prompt = build_directional_prompt(&provider_name, question)?;
        let label = format!("run {}/{}", run + 1, question_count);

        let ttft_ms = match measure_ttft(provider.as_ref(), prompt).await {
            Ok(ms) => ms,
            Err(e) => {
                let msg = e.to_string();
                if let Some(rest) = msg.strip_prefix("rate_limit:") {
                    let wait_secs: u64 = rest.parse().unwrap_or(10);
                    eprintln!("{label}: 429 — waiting {wait_secs}s then retrying once");
                    tokio::time::sleep(Duration::from_secs(wait_secs)).await;
                    let prompt = build_directional_prompt(&provider_name, question)?;
                    measure_ttft(provider.as_ref(), prompt)
                        .await
                        .with_context(|| format!("{label} failed after rate-limit retry"))?
                } else {
                    return Err(e).with_context(|| format!("{label} failed"));
                }
            }
        };

        let flag = if ttft_ms > TTFT_GATE_MS { " BREACH" } else { "" };
        println!("{label}: ttft_ms={ttft_ms}{flag}");
        samples.push(ttft_ms);
    }

    let mut sorted = samples.clone();
    sorted.sort_unstable();
    let p50 = percentile(&sorted, 0.50) as u64;
    let p95 = percentile(&sorted, 0.95) as u64;
    let pass = p95 < TTFT_GATE_MS;

    println!("---");
    println!("provider={provider_name} runs={}", samples.len());
    println!("samples_ms={sorted:?}");
    println!("P50={p50}ms P95={p95}ms pass={pass}");
    if !pass {
        std::process::exit(1);
    }
    Ok(())
}
