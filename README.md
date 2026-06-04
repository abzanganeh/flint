# Flint
<p align="center">
  <img src="src/assets/flint-hero.png" alt="Flint hero" width="720" />
</p>
Real-time AI co-pilot desktop app for live conversations (e.g. job interviews). Flint listens to system audio, transcribes locally, and surfaces parallel AI guidance in a stealth overlay — invisible to the other party.

**Repository:** [github.com/abzanganeh/flint](https://github.com/abzanganeh/flint)

## What it does

- Captures **system audio** (interviewer), not your microphone, for question detection and responses
- Runs **local** transcription (Whisper), noise suppression (RNNoise), and VAD
- Fires **parallel** directional, depth, and clarifying LLM threads on detected questions
- **RAG** over session context via sqlite-vec + local embeddings
- **Stealth overlay** (Tauri): always-on-top, transparent, excluded from screen capture where the OS allows
- **Groq** cloud inference with **Ollama** fallback; API keys in the OS keychain only

## Tech stack

| Layer | Technology |
|-------|------------|
| Desktop | Tauri 2.x |
| Backend | Rust (tokio) |
| Frontend | React 18, TypeScript, Tailwind |
| Auth / persistence | Supabase |
| Local vectors | sqlite-vec, fastembed (bge-small-en-v1.5) |

## Prerequisites

- [Rust](https://rustup.rs/) (stable, 2021 edition)
- [Node.js](https://nodejs.org/) 20+
- Linux: GTK / WebKit dev libraries (see below)
- Optional: [Supabase CLI](https://supabase.com/docs/guides/cli) for local auth/DB during development

## Linux system dependencies

```bash
./scripts/install-linux-deps.sh
```

Or install manually:

```bash
sudo apt update
sudo apt install -y libwebkit2gtk-4.1-dev build-essential libssl-dev \
  libasound2-dev libgtk-3-dev libpango1.0-dev libgdk-pixbuf-2.0-dev libatk1.0-dev \
  libsoup-3.0-dev libjavascriptcoregtk-4.1-dev librsvg2-dev patchelf \
  libxdo-dev libayatana-appindicator3-dev
```

## Development

```bash
npm install
npm run tauri dev
```

Local Supabase (optional):

```bash
npm run supabase:start
```

## Build

```bash
npm run tauri build
```

Rust only:

```bash
cd src-tauri && cargo build
```

## Tests

```bash
cargo test --manifest-path src-tauri/Cargo.toml
npm test
```

## Project layout

```
src-tauri/src/   # Rust: audio, transcription, RAG, orchestrator, LLM, session
src/             # React UI (panels, screens, commands, events)
prompts/         # Versioned LLM prompts (gpt / claude / llama variants)
supabase/        # Migrations and local Supabase config
tests/           # Integration and e2e tests
evals/           # Prompt eval harness
```

## Git workflow

Work happens on **feature branches** (one meaningful implementation chunk per branch). Push the branch when the session is done; open a PR into `main` for review. See `.cursor/rules/flint-git-workflow.mdc` for agent conventions.

## Security notes

- API keys never touch disk unencrypted (OS keychain)
- Audio is never written to disk
- Do not commit `.env` or real credentials

## License

Flint is licensed under the [Business Source License 1.1](LICENSE) (BSL 1.1).
Personal, non-commercial, and internal evaluation use is permitted. Commercial
production use requires a separate license — see [COMMERCIAL.md](COMMERCIAL.md).

Source converts to [Apache 2.0](https://www.apache.org/licenses/LICENSE-2.0) on
the **Change Date** defined in `LICENSE` (eighth anniversary of each version’s
first public release, subject to BSL’s standard terms).
