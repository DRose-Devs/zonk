# zonk AGENTS (Codex Working Instructions)

## Scope

This file is the codex-specific execution guide for this repository.
It complements `CLAUDE.md` with mandatory constraints for automated work.

## Core rule

- Autoresearch loops MUST be implemented and executed in Rust.
- Do not use Python for looping or orchestration of candidate generation, backtest runs, scoring, or ranking.
- This repository’s automated discovery workflow is paper-research-first: use arXiv/Exa seeds, then execute as `paper-research`.

## Zonk architecture at a glance

- Binary strategy runner: `zonk` (`src/main.rs`, `src/lib.rs`)
- Core CLI: `src/cli.rs`
- Core data pipeline: `src/data/*`
- Strategy implementations: `src/strategies/*`
- Metrics: `src/metrics/*`
- Autoresearch loop binary: `src/bin/autoresearch_loop.rs`

## Rust autoresearch command

- Build:
  - `cargo build --release`
- Run:
  - `cargo run --release --bin autoresearch_loop -- --seed-web --candidates 100 --top 10 --verbose`
  - `cargo run --release --bin autoresearch_loop -- --seed-web --verbose`
  - Optional dates/sessions override:
    - `--train-start 2020-01-01 --train-end 2024-12-31 --test-start 2025-01-01 --test-end 2026-03-11 --train-sessions 1008 --test-sessions 252`
  - Optional binary override:
    - `--zonk-bin target/release/zonk`

### Iterative refinement (default)

Looping is the default (`--max-rounds 10`). After round 0, the loop refines top winners by perturbing parameters within discrete grids and swapping assets. Stops on convergence (`--patience 3`, `--min-improvement 0.02`) or frontier exhaustion.

| Flag | Default | Description |
|------|---------|-------------|
| `--max-rounds` | 10 | Maximum refinement rounds |
| `--patience` | 3 | Stale rounds before stopping |
| `--min-improvement` | 0.02 | Minimum relative improvement to reset patience |
| `--refine-top` | 5 | Winners to refine per round |
| `--refine-variants` | 30 | Max variants per winner |
| `--no-loop` | false | Disable refinement (single-pass legacy) |

### Asset universe expansion

Controls which assets are tested during refinement rounds. Round 0 always uses core assets (5) for speed.

| Flag | Default | Description |
|------|---------|-------------|
| `--asset-universe` | `broad` | `core` (5), `broad` (SP500+NDX100 ~550), `full` (all viable warehouse), or preset name |
| `--refine-asset-swaps` | 10 | Max asset swap variants per winner per round |
| `--min-asset-rows` | auto | Min parquet rows for viability (default: train+test sessions) |

### Quality gates

Filter final report to only "investable" strategies. Applied after all rounds complete.

| Flag | Default | Description |
|------|---------|-------------|
| `--min-sharpe` | none | Minimum test-window Sharpe ratio |
| `--max-drawdown` | none | Maximum test-window drawdown (absolute %, e.g. `20` = reject worse than -20%) |

When gates are active, the report shows pass/fail counts. If none pass, unfiltered results shown for reference.

### Convergence summary

When the refinement loop terminates (patience, frontier exhaustion, or max rounds), a diagnostic summary prints:
- Stop reason
- Total evaluated/passed counts and exhausted refinement centers
- Rule and asset distribution among passing candidates
- Score trajectory from first to last round

### Evaluation cache

`reports/autoresearch-eval-cache.jsonl` persists results across runs keyed by parameter signature + date windows. Deterministic grid and repeated seeded candidates are served instantly. Use `--no-cache` to force re-evaluation (e.g., after strategy code changes).

### Audit trail and registry

- Persisted top-10 winners must have train/test audit artifacts under `reports/autoresearch-audits/` with exact evaluated periods, trade ledgers, and equity traces.
- Promoted winners must also be upserted into `reports/autoresearch-strategy-registry.json`, keyed by stable parameter signature, so future implementation work can start from audited candidates rather than only JSONL history.

## Data and candidate constraints

- Backtests use local warehouse parquet data only.
- Loop candidates must execute as `paper-research` only.
- The loop should run with Exa/arXiv candidate seeding via `--seed-web` for new strategy discovery.
- Candidate pool should target at least 100 research candidates by default.

## Output artifacts

- `reports/autoresearch-ledger.jsonl` — append-only log of top-ranked results per run
- `reports/autoresearch-exa-ideas.json` — raw Exa/arXiv seeds (when `--seed-web` is set)
- `reports/autoresearch-top10-interactive-report.html` — branded interactive report
- `reports/autoresearch-audits/` — auditable train/test proof JSONs for persisted top-10 candidates
- `reports/autoresearch-strategy-registry.json` — promoted strategy registry for future real-world implementation work
- `reports/autoresearch-eval-cache.jsonl` — persistent evaluation cache (cross-run)
- Prefer reading these after each production loop before promoting candidates.

## Required behavior for the loop

- Candidate discovery uses arXiv-focused Exa search when `--seed-web` is provided.
- Build net-new candidates from web-seeded hypotheses + deterministic paper-research mutations.
- Run walk-forward train/test windows.
- Score train/test and rank using the loop scoring formula.
- Keep only candidates that pass JSON parsing and metric gates.

## Brand & Visual Design (Mandatory — All Artifacts)

**Every visual artifact produced by agents in this project — HTML reports, dashboards, websites, landing pages, interactive tools, data visualizations, charts, or any other browser-rendered output — MUST follow the zonk brand design system.** No exceptions. This applies to all code paths, not just autoresearch reports.

### What to use

- **Primary spec**: `DESIGN.md` — this is the authoritative design-system strategy for browser-rendered output.
- **Template**: `design/report-template.html` — use this as the generator base for autoresearch HTML reports, but keep it aligned with `DESIGN.md`.
- **Local assets**: `design/tokens.css` and `design/brand-guidelines.html` — implementation helpers, not the authority over `DESIGN.md`.
- **External source of truth**: Figma `Reports` file — `https://www.figma.com/design/0TCEsZxLVO6x5pJkOSOwCl/Reports?node-id=0-1&t=K0IBsu4WJyJ8qmjn-1`

### Hard constraints (apply to ALL visual output, not just reports)

1. **Design authority**: follow `DESIGN.md` first, and use the Figma `Reports` file as the target format when it is accessible.
2. **Creative direction**: every report should read as "The Digital Curator" — editorial, asymmetrical, high-trust, and data-dense without falling back to generic SaaS UI patterns.
3. **Surface rule**: avoid obvious boxed-in sectioning. Prefer tonal shifts between surfaces over visible 1px borders, except for accessibility-driven ghost borders.
4. **Typography rule**: prestige serif for display, functional sans for body, and calculated sans for data/labels, per `DESIGN.md`.
5. **Shape rule**: no rounded-corner UI language. Default to square-cornered structure unless a future spec explicitly changes it.
6. **Accent rule**: use the lime accent sparingly and only for critical insight or action states.
7. **Scope**: these rules apply to ALL browser-rendered artifacts.

### When generating a report

```
1. Read DESIGN.md first
2. When available, inspect the Figma Reports file and align the report format to it
3. Use design/report-template.html as the generator base
4. Replace /* PASTE_ROWS_HERE */ with the actual rows JSON array
5. Update title, dates, and KPI values
6. Write to reports/autoresearch-top10-interactive-report.html
```

## Environment

- For seed fetching, `EXA_API_KEY` must be loaded in environment.
- `.env` should include this key; start from `.env.example`:
  - `cp .env.example .env`
  - Fill `EXA_API_KEY`.
- One-time sync from shell:
  - `source ~/.zshrc >/dev/null 2>&1 && printf 'EXA_API_KEY=%s\n' "$EXA_API_KEY" > .env`
- `EXA_API_KEY` is required for seeded production loops.
