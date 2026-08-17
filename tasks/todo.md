# zonk — Plan & Progress

## Current State (2026-03-18)

### Uncommitted Work
- `CLAUDE.md` — minor updates (+14/-0 lines)
- `src/bin/autoresearch_loop.rs` — major expansion (+498/-41 lines)
- `src/data/discovery.rs` — new symbol discovery logic (+99 lines)

### Recent Milestones (last 5 commits)
- [x] `vol_spread` rule + Research Analysis Framework in autoresearch
- [x] Enriched research basis narratives, VIX-focused seed queries, evaluation cache
- [x] Iterative refinement loop, branded reports, evaluation cache
- [x] Human-readable profit descriptions in top-10 report
- [x] Paper-research net-new strategy loop and reporting

### Completed (earlier)
- [x] Rust-only rewrite — full Python removal, feature parity
- [x] `--output json` / `--output md` global flags
- [x] 192 unit tests + 106 CLI integration tests
- [x] Local warehouse parquet reads (no Yahoo Finance)
- [x] Design system (`design/tokens.css`, templates)
- [x] 5 paper-research rules: trend_momentum, trend_pullback, rsi_reversion, volatility_regime, vol_spread
- [x] Breadth strategies: washout, MA, dual-MA, NDX-100 SMA
- [x] Asset universe expansion (core/broad/full/preset)

## TODO

- [ ] Commit current uncommitted changes (autoresearch loop expansion + discovery)
- [ ] Review and stabilize autoresearch iterative refinement loop
- [ ] Expand test coverage for new discovery.rs and autoresearch_loop.rs changes
- [ ] Evaluate adding new paper-research signal rules
- [ ] Performance benchmarking on full universe (~3,500 symbols)

## Current Run Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3 -> T4

Tasks:
- [x] T1 Write execution plan and review scaffold for standard autoresearch run `depends_on: []`
- [x] T2 Build release binary for `autoresearch_loop` `depends_on: [T1]`
- [x] T3 Run Rust autoresearch loop in `standard` mode with seeded web discovery `depends_on: [T2]`
- [x] T4 Verify generated artifacts and document results in review section `depends_on: [T3]`

## Current Run Review (2026-03-18)

Summary:
- Standard Rust autoresearch completed with `--seed-web --candidates 100 --top 10 --verbose` on the broad asset universe.
- Exa seeding returned 114 items; the run formed a 100-candidate pool and loaded 815 cached evaluations.
- Five recorded rounds completed (`0` through `4`). The round-history table shown in terminal output sums to 313 evaluations across those rounds.
- The final persisted top retained result in the ledger/report is `seed-011-vol_spread-v2` on `QQQ` with combined score `3.4160`, train Sharpe `0.816`, test Sharpe `0.945`, train drawdown `0.143%`, and test drawdown `0.067%`.
- Observation: the terminal round-history snippet showed a higher transient global-best than the final persisted top-10; verification below uses the persisted ledger/report outputs as the source of truth.

Verification:
- `cargo build --release` finished successfully.
- `reports/autoresearch-top10-interactive-report.html` exists and was updated at `2026-03-18T16:26:24.629Z`.
- `reports/autoresearch-ledger.jsonl` was appended with 10 entries at timestamp `2026-03-18T16:26:24.629442+00:00`.
- `reports/autoresearch-exa-ideas.json` exists and contains 114 `items`.
- `reports/autoresearch-eval-cache.jsonl` was updated during the run.

Artifacts:
- `reports/autoresearch-top10-interactive-report.html`
- `reports/autoresearch-ledger.jsonl`
- `reports/autoresearch-exa-ideas.json`
- `reports/autoresearch-eval-cache.jsonl`

## Follow-up Run Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3 -> T4

Tasks:
- [x] T1 Record plan and review scaffold for wider non-core asset autoresearch run `depends_on: []`
- [x] T2 Inspect CLI/code paths for asset-universe and refinement controls `depends_on: [T1]`
- [x] T3 Execute wider Rust autoresearch run with stronger non-core asset exploration settings `depends_on: [T2]`
- [x] T4 Verify artifacts/results and document follow-up run outcome `depends_on: [T3]`

## Follow-up Run Review (2026-03-18)

Summary:
- Wider follow-up run completed with `--seed-web --asset-universe full --candidates 100 --top 10 --refine-top 10 --refine-variants 40 --refine-asset-swaps 50 --verbose`.
- Exa seeding returned 117 items; the run formed a 100-candidate pool and loaded 1083 cached evaluations.
- Eight recorded rounds completed (`0` through `7`). The round-history table sums to 2893 evaluations across those rounds.
- Live refinement clearly explored non-core assets. Terminal output included candidates such as `FDM`, `AIVC`, `CRTO`, `OVS`, `AMZA`, `THD`, `HWBK`, `ABBV`, `ACES`, `PPBT`, `HEDJ`, and `HIMS`. The tail of the eval cache after the run contains 460 unique assets.
- The persisted top retained result in the ledger/report is `seed-002-vol_spread-v1` on `QQQ` with combined score `4.2461`, round `0`, and test Sharpe `1.324`.
- Important discrepancy: round history reported a higher global-best score of `5.291` at round `4`, but the persisted report/ledger top-10 remained core-only (`QQQ`, `SPXL`, `TQQQ`) and did not include any non-core assets.

Verification:
- `cargo build --release` finished successfully before the run.
- `reports/autoresearch-top10-interactive-report.html` exists and was updated at `2026-03-18T16:35:39.368Z`.
- `reports/autoresearch-ledger.jsonl` was appended with 10 entries at timestamp `2026-03-18T16:35:39.368948+00:00`.
- `reports/autoresearch-exa-ideas.json` exists and was updated during the run.
- `reports/autoresearch-eval-cache.jsonl` grew substantially and its recent tail shows wide non-core asset coverage.
- Code review note: final report HTML is generated from `ranked.iter().take(10)` and ledger entries are then recovered by matching those signatures against `loop_state.recorded_results`, so the persisted outputs should reflect ranked top-10 if ranking/reporting is behaving correctly.

Artifacts:
- `reports/autoresearch-top10-interactive-report.html`
- `reports/autoresearch-ledger.jsonl`
- `reports/autoresearch-exa-ideas.json`
- `reports/autoresearch-eval-cache.jsonl`

## HTML Report Bug Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3 -> T4

Tasks:
- [x] T1 Record bug-fix plan and capture the user correction as a lesson `depends_on: []`
- [x] T2 Reproduce the report/source mismatch in persisted artifacts and isolate the code path `depends_on: [T1]`
- [x] T3 Fix seeded title extraction and report narrative wording so linked paper metadata stays aligned `depends_on: [T2]`
- [x] T4 Add regression coverage, run `cargo test`, regenerate the report, and document results `depends_on: [T3]`

## HTML Report Bug Review (2026-03-18)

Summary:
- Root cause 1: seeded candidate rationale used `seed.title` verbatim, but Exa often returns subject buckets such as `Quantitative Finance > Portfolio Management` instead of the actual paper title.
- Root cause 2: report copy for seeded candidates implied a direct paper-to-rule mapping ("the paper's findings on mean-reverting behavior"), even though seeded rules are heuristic hypothesis translations.
- Fix: added seed-title extraction helpers to recover real paper titles from seed text when Exa titles are generic, updated seeded report narratives to say the paper is a research lead rather than a direct replication, rebuilt the release binary, and regenerated the HTML report.
- Result: the refreshed report no longer contains generic subject-bucket titles in top rows, and seeded narratives now explicitly call out the hypothesis-driven mapping.

Verification:
- Reproduced the bad persisted row in the prior report/ledger for `seed-019-rsi_reversion-v1`, where the source URL pointed at `arXiv:2512.12420` while the narrative showed a generic subject-bucket title.
- Added and passed targeted regression tests:
- `cargo test --bin autoresearch_loop test_seed_candidate_uses_paper_title_from_seed_text_when_exa_title_is_generic`
- `cargo test --bin autoresearch_loop test_research_basis_for_seeded_candidate_is_explicitly_hypothesis_driven`
- Rebuilt with `cargo build --release --bin autoresearch_loop`.
- Regenerated the report via `target/release/autoresearch_loop --seed-web --candidates 100 --top 10 --verbose`.
- Verified the refreshed `reports/autoresearch-top10-interactive-report.html` contains seeded narratives with `research lead` / `not a direct replication` wording and no `Quantitative Finance > ...` titles in the top-10 rows.

Artifacts:
- `reports/autoresearch-top10-interactive-report.html`
- `reports/autoresearch-ledger.jsonl`
- `src/bin/autoresearch_loop.rs`
- `tasks/lessons.md`

## Seed Mapping + Beginning Equity Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3

Tasks:
- [x] T1 Record the user correction, update lessons, and write the execution scaffold `depends_on: []`
- [x] T2 Tighten seeded paper-to-rule matching and add beginning-equity fields to the report payload/rendering `depends_on: [T1]`
- [x] T3 Rebuild binaries, regenerate the report, and verify beginning equity plus stricter seeded mappings in persisted artifacts `depends_on: [T2]`

## Seed Mapping + Beginning Equity Review (2026-03-18)

Summary:
- Added `beginning_equity` to strategy metrics serialization and surfaced it through report details, ledger entries, cached evaluations, and terminal summaries.
- Tightened seeded rule-family classification to use title-plus-abstract text with token-boundary phrase matching instead of raw substring scans across the full scraped arXiv page.
- Added unsupported-family routing for option-pricing papers and regression coverage to keep hedging/option-model papers from being forced into RSI-style rules unless there is a credible supported-rule match.
- Improved seed title extraction to skip placeholder PDF labels such as `Submitted paper 1` and recover the real paper title from nearby header lines.
- Added cache upgrade logic so older successful eval-cache rows with missing or invalid `beginning_equity` are backfilled to the paper-research default capital during load.
- Regenerated the production report. The latest persisted top row is `seed-110-vol_spread-v1` on `QQQ` with combined score `4.2461`, train score `3.6659`, and test score `5.3235`.

Verification:
- `cargo fmt --all`
- `cargo test --bin autoresearch_loop`
- `cargo test compute_strategy_metrics`
- `cargo build --release --bin zonk --bin autoresearch_loop`
- `cargo build --release --bin autoresearch_loop`
- `target/release/autoresearch_loop --seed-web --candidates 100 --top 10 --verbose`
- Verified `reports/autoresearch-top10-interactive-report.html` top-10 rows contain no `beginning_equity: 0.0` values.
- Verified the latest top-10 report rows contain no seeded `rsi_reversion` entries whose rationale references hedging or option-pricing papers.
- Verified the latest `reports/autoresearch-ledger.jsonl` tail has no zero beginning-equity entries.

Artifacts:
- `reports/autoresearch-top10-interactive-report.html`
- `reports/autoresearch-ledger.jsonl`
- `src/bin/autoresearch_loop.rs`
- `src/strategies/common.rs`
- `tasks/lessons.md`

## Equity Provenance Review Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3

Tasks:
- [x] T1 Record the inspection scope for beginning/ending equity and trade-proof provenance `depends_on: []`
- [x] T2 Inspect the backtest/metrics code path for how equity is computed and what execution evidence is retained `depends_on: [T1]`
- [x] T3 Run the current top candidate, verify the reported equity values against the configured window, and summarize proof gaps if any `depends_on: [T2]`

## Equity Provenance Review (2026-03-18)

Summary:
- `beginning_equity` is the first element of the simulated equity curve and `final_equity` is the last element; they are not annualized values.
- In `paper_research`, equity is compounded over the evaluation window by taking a full-notional long trade from close `i` to close `i+1` whenever the signal mask is true, then subtracting an IBKR round-trip fee.
- Autoresearch currently passes train/test windows to `zonk` as trailing session counts inferred from weekday counts. That means the effective `period_start` in `zonk` can drift earlier than the nominal start date shown by the autoresearch loop because actual trading sessions exclude market holidays.
- Current persisted outputs are reproducible but not audit-grade: JSON/report/ledger retain aggregate metrics and annual returns, but they do not retain a trade-by-trade execution ledger.

Verification:
- Inspected `src/strategies/common.rs`, `src/strategies/paper_research.rs`, `src/metrics/performance.rs`, `src/metrics/fees.rs`, and `src/bin/autoresearch_loop.rs`.
- Re-ran the current top candidate directly with `zonk --output json` for both train and test windows.
- Verified train output for `seed-110-vol_spread-v1` on `QQQ`: `period_start=2019-10-24`, `period_end=2024-12-31`, `beginning_equity=1000000.0`, `final_equity=2032430.8840000012`.
- Verified test output for `seed-110-vol_spread-v1` on `QQQ`: `period_start=2024-12-11`, `period_end=2026-03-11`, `beginning_equity=1000000.0`, `final_equity=1305635.8369999956`.

Artifacts:
- `src/strategies/common.rs`
- `src/strategies/paper_research.rs`
- `src/metrics/performance.rs`
- `src/metrics/fees.rs`
- `src/bin/autoresearch_loop.rs`

## Audit Trail Implementation Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3 -> T4

Tasks:
- [x] T1 Record the audit-trail requirement, update lessons, and write the execution scaffold `depends_on: []`
- [x] T2 Add exact-window evaluation plus optional trade/equity audit output to `paper-research` JSON `depends_on: [T1]`
- [x] T3 Persist top-candidate train/test audit artifacts and surface them in the report/ledger `depends_on: [T2]`
- [x] T4 Add regression coverage, rerun autoresearch, and verify the audit trail end-to-end `depends_on: [T3]`

## Audit Trail Implementation Review (2026-03-18)

Summary:
- `paper-research` now supports explicit `--start-date` evaluation windows and optional `--include-audit` JSON output containing an execution audit with exact actual period bounds, per-trade ledger entries, and a per-bar equity trace.
- The autoresearch loop now reruns the persisted top-10 candidates with audit output enabled, writes 20 train/test JSON artifacts under `reports/autoresearch-audits/`, and carries audit metadata into both the HTML report and the append-only ledger.
- The regenerated report now shows Actual Period, Executed Trades, and Audit Trail links for each train/test pane, so beginning and ending equity can be traced to concrete simulated trades rather than inferred from summary metrics alone.
- The latest persisted top row is `seed-026-vol_spread-v0` on `QQQ` with combined score `2.9947`. Its audit-backed actual windows are `2020-01-02 -> 2024-12-31` (405 train trades) and `2025-01-02 -> 2026-03-11` (127 test trades).

Verification:
- `cargo fmt --all`
- `cargo test --lib`
- `cargo test --bin autoresearch_loop`
- `cargo build --release --bin zonk --bin autoresearch_loop`
- `target/release/autoresearch_loop --seed-web --candidates 100 --top 10 --verbose`
- Verified `reports/autoresearch-audits/` contains 20 JSON files for the persisted top-10 train/test windows.
- Verified `reports/autoresearch-ledger.jsonl` includes `train_audit` and `test_audit` objects with artifact paths, actual period bounds, and trade counts.
- Verified `reports/autoresearch-top10-interactive-report.html` contains Actual Period, Executed Trades, and Audit Trail render hooks pointing at the generated JSON artifacts.
- Inspected `reports/autoresearch-audits/seed-026-vol_spread-v0-train-2020-01-01-to-2024-12-31.json` and confirmed it contains both the `trades` ledger and the `equity_trace` proving the path from `$1,000,000` beginning equity to `$1,748,085.05` ending equity.

Artifacts:
- `src/strategies/paper_research.rs`
- `src/bin/autoresearch_loop.rs`
- `reports/autoresearch-top10-interactive-report.html`
- `reports/autoresearch-ledger.jsonl`
- `reports/autoresearch-audits/`

## Strategy Registry Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3 -> T4

Tasks:
- [x] T1 Record the strategy-registry requirement and write the execution scaffold `depends_on: []`
- [x] T2 Implement a durable registry schema and upsert/save logic for promoted top-10 candidates `depends_on: [T1]`
- [x] T3 Persist the registry from the autoresearch loop and add regression coverage `depends_on: [T2]`
- [x] T4 Rebuild, rerun autoresearch, and verify the registry artifact end-to-end `depends_on: [T3]`

## Strategy Registry Review (2026-03-18)

Summary:
- Added a dedicated promoted-strategy registry at `reports/autoresearch-strategy-registry.json`, separate from the eval cache and append-only ledger.
- Registry entries are keyed by stable parameter signature and retain lifecycle metadata (`registry_status`), deduplicated candidate/source history, and both `latest` and `best` audited observations for each promoted strategy.
- The autoresearch loop now upserts the persisted top-10 into that registry after writing the report and ledger, so future implementation work has a structured catalog of research candidates rather than only historical JSONL rows.
- The latest registry build contains 10 promoted entries. The current top registry entry is `seed-001-vol_spread-v0` on `QQQ` with combined score `3.6949`, train/test audit links, and `times_in_top10 = 1`.

Verification:
- `cargo fmt --all`
- `cargo test --bin autoresearch_loop`
- `cargo build --release --bin autoresearch_loop`
- `target/release/autoresearch_loop --seed-web --candidates 100 --top 10 --verbose`
- Verified `reports/autoresearch-strategy-registry.json` exists and was updated at `2026-03-18 12:30:39` local time.
- Verified the registry contains 10 entries, each with `registry_status = "research_candidate"`, stable `signature`, `latest`, and `best` snapshots.
- Verified the top registry entry includes audit artifact links for both train and test windows, so the registry can point directly at proof files when a candidate is later promoted to hand-coded strategy work.

Artifacts:
- `src/bin/autoresearch_loop.rs`
- `reports/autoresearch-strategy-registry.json`

## Release Docs Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3 -> T4

Tasks:
- [x] T1 Inspect the current repo state, relevant docs, and version references for the audit/registry release `depends_on: []`
- [x] T2 Update docs and agent files to describe audit artifacts, registry behavior, and release outputs `depends_on: [T1]`
- [x] T3 Bump the crate version, verify the release diff, and update this review section `depends_on: [T2]`
- [x] T4 Commit the relevant changes and push `main` to `origin` `depends_on: [T3]`

## Release Docs Review (2026-03-18)

Summary:
- Documented the new autoresearch audit-trail and promoted-strategy registry outputs in `README.md`, `AGENTS.md`, and `CLAUDE.md` so the repo instructions now match the current workflow.
- Bumped the crate version from `0.1.0` to `0.2.0` in both `Cargo.toml` and the root package entry in `Cargo.lock` to reflect the feature release.
- Kept the release commit scoped to the audit-trail/registry work and associated generated artifacts, while leaving unrelated `.claude` worktree changes untouched.

Verification:
- `cargo test --bin autoresearch_loop`
- `cargo build --release --bin zonk --bin autoresearch_loop --quiet`
- Verified release binaries were rebuilt at `2026-03-18 12:57` local time.
- Reviewed diffs for `README.md`, `AGENTS.md`, `CLAUDE.md`, `Cargo.toml`, and `Cargo.lock` to confirm the new artifacts and version bump are described consistently.

## Claude Cleanup Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3

Tasks:
- [x] T1 Inspect the remaining `.claude` worktree changes and record the cleanup scope `depends_on: []`
- [x] T2 Stage the `.claude` cleanup and verify the exact diff being committed `depends_on: [T1]`
- [x] T3 Commit the `.claude` cleanup and push `main` to `origin` `depends_on: [T2]`

## Claude Cleanup Review (2026-03-18)

Summary:
- Removed the Claude-side pre-tool hook wiring from `.claude/settings.json`, leaving an empty JSON object instead of a `PreToolUse` bash hook configuration.
- Committed the deletion of `.claude/hooks/pre-commit-check.sh`, which had been the shell entrypoint for the Claude pre-commit checks.
- Kept the cleanup isolated to the `.claude` files plus this task-log update, then pushed the cleanup commit directly to `origin/main`.

Verification:
- Inspected `git diff --cached -- .claude/settings.json .claude/hooks/pre-commit-check.sh tasks/todo.md` before committing.
- Verified the cleanup commit was pushed to `origin/main`.

## Ruff Cache Cleanup Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3

Tasks:
- [x] T1 Record the Ruff-cache cleanup scope and confirm the remaining worktree changes `depends_on: []`
- [x] T2 Stage the `.gitignore` update and verify the exact diff being committed `depends_on: [T1]`
- [x] T3 Commit the Ruff-cache cleanup and push `main` to `origin` `depends_on: [T2]`

## Ruff Cache Cleanup Review (2026-03-18)

Summary:
- Added `.ruff_cache/` to `.gitignore` so Ruff's local cache stays out of the repo worktree.
- Removed the existing repo-local `.ruff_cache` directory from disk as disposable tool cache.
- Kept the cleanup limited to the ignore rule and this task-log update, then pushed it directly to `origin/main`.

Verification:
- Confirmed `.ruff_cache` was not tracked by git and no longer exists in the repo root.
- Inspected `git diff --cached -- .gitignore tasks/todo.md` before committing.
- Verified the cleanup commit was pushed to `origin/main`.

## Design MD Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3

Tasks:
- [x] T1 Read the official Design MD overview/format docs and inspect the local design assets `depends_on: []`
- [x] T2 Write a repo-specific `DESIGN.md` that references the design assets and follows the Design MD structure `depends_on: [T1]`
- [x] T3 Verify the new `DESIGN.md` against the source docs and local assets, then update this review section `depends_on: [T2]`

## Design MD Review (2026-03-18)

Summary:
- Added a new root-level `DESIGN.md` that follows Stitch's documented section order: Overview, Colors, Typography, Elevation, Components, and Do's and Don'ts.
- Grounded the document in the local design source files, especially `design/tokens.css`, `design/brand-guidelines.html`, and `design/report-template.html`, so design agents can map the markdown back to the repo's actual token and layout system.
- Translated the repo's light-theme zonk brand constraints into plain markdown guidance suitable for DESIGN.md consumers, including token roles, typography hierarchy, elevation rules, layout defaults, component behavior, and explicit prohibitions against the deprecated dark-blue theme.

Verification:
- Reviewed the rendered Stitch docs for `What is DESIGN.md?`, `The DESIGN.md format`, and the adjacent `View, edit, and export` page to confirm the required structure and usage model.
- Verified `DESIGN.md` preserves the documented section order and stays plain markdown with no special syntax.
- Cross-checked the new content against `design/tokens.css`, `design/brand-guidelines.html`, `design/report-template.html`, and the repo's existing design rules in `AGENTS.md`.

## HTML Report Overhaul Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3

Tasks:
- [x] T1 Inspect the current report generation flow, template, generated outputs, and the new `DESIGN.md` constraints `depends_on: []`
- [x] T2 Redesign the source template and any supporting generation code so generated reports follow `DESIGN.md` and the zonk brand system end-to-end `depends_on: [T1]`
- [x] T3 Regenerate and verify the updated report for hierarchy, responsiveness, accessibility, and data integrity, then record the review `depends_on: [T2]`

## HTML Report Overhaul Review (2026-03-18)

Summary:
- Replaced the old table-first HTML report template with a new `Autoresearch Strategy Deck` layout driven by `DESIGN.md` and the local zonk design system.
- Refactored `src/bin/autoresearch_loop.rs` so the report renderer now injects both report metadata and row data into `design/report-template.html`, instead of maintaining a large inline HTML string in Rust.
- The new report presents a teal editorial hero, KPI summary strip, filter workbench, ranked candidate table, and a sticky strategy inspector that surfaces beginning/ending equity, requested versus actual windows, trade counts, audit links, and the five Research Analysis Framework sections.
- Added responsive mobile cards and verified that row selection and filtering update the inspector cleanly without JavaScript errors.
- Scope note: the live generator-backed report is `reports/autoresearch-top10-interactive-report.html`; the legacy `reports/refactor-report.html` was not part of the active Rust generation flow for this task.

Verification:
- `cargo fmt --all`
- `cargo test --bin autoresearch_loop`
- `cargo build --release --bin autoresearch_loop --quiet`
- `target/release/autoresearch_loop --seed-web --candidates 100 --top 10 --verbose`
- Browser render check against the generated local file at desktop (`1440px`) and mobile (`430px`) widths using a temporary isolated `playwright-core` install under `/tmp/zonk-browser-check`
- Interaction check: filtering for `SPY` reduced the visible results to 3 candidates and clicking the second row switched the inspector from `seed-144-vol_spread-v1` to `seed-003-vol_spread-v2` without console errors

Artifacts:
- `DESIGN.md`
- `design/report-template.html`
- `src/bin/autoresearch_loop.rs`
- `reports/autoresearch-top10-interactive-report.html`

## Design Lab Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3

Tasks:
- [x] T1 Detect the repo frontend/runtime constraints, design memory, and live visual surfaces for design-lab setup `depends_on: []`
- [x] T2 Gather the remaining design brief inputs from the user and define the temporary lab target plus evaluation criteria `depends_on: [T1]`
- [x] T3 Generate the temporary standalone design lab with five report variations and present the preview workflow `depends_on: [T2]`

## Design Lab Review (2026-03-18)

Summary:
- Built a temporary standalone design lab for `autoresearch-top10-interactive-report` because the repo has no app framework or package-manager-backed frontend runtime.
- Translated the user brief into five distinct full-page redesign directions focused on hierarchy, subtle motion, and improved legibility while staying anchored to the zonk brand tokens and typography system.
- Added an in-browser feedback overlay so review comments can be attached to labeled surfaces inside each variant and exported back into the terminal as structured markdown.
- Tightened the overlay targeting after verification so clicking the Variant B comparison surface resolves to `Dense comparison table` instead of a nested table cell.

Verification:
- Opened `.claude-design/lab/index.html` in Chrome via Playwright against the local file path.
- Verified the lab loads without console errors or page errors.
- Verified all five variants (`A` through `E`) render and the summary strip renders five cards.
- Verified the feedback panel toggles correctly and the modal opens on the intended labeled surface for Variant B.

Artifacts:
- `.claude-design/design-brief.json`
- `.claude-design/run-log.md`
- `.claude-design/lab/index.html`
- `.claude-design/lab/data/fixtures.js`
- `.claude-design/lab/feedback-overlay.js`

## Variant D Finalization Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3

Tasks:
- [x] T1 Inspect the current report template/generator and map the selected Variant D command-center layout into the production report structure `depends_on: []`
- [x] T2 Implement the Variant D redesign in the live report template and regenerate the current HTML artifact from persisted report data `depends_on: [T1]`
- [x] T3 Verify the regenerated report in Chrome, document the final result, create design-memory artifacts, and clean up `.claude-design/` `depends_on: [T2]`

## Variant D Finalization Review (2026-03-18)

Summary:
- Implemented the winning Variant D direction as the live autoresearch report: a three-panel command-center layout with a left control rail, center briefing plus frontier cards, and a right audit rail.
- Replaced the old table-plus-inspector composition in `design/report-template.html` with a clearer selection-driven card workflow while preserving filters, sort controls, source links, audit links, beginning equity, ending equity, and the five research framework narratives.
- Regenerated `reports/autoresearch-top10-interactive-report.html` from the persisted embedded report JSON so the current artifact matches the new template without requiring a fresh strategy run.
- Added `DESIGN_PLAN.md` and `DESIGN_MEMORY.md` so the chosen report direction is documented for future UI/report work.
- Cleaned up the temporary design-lab workspace by deleting `.claude-design/` after finalization.

Verification:
- `cargo test --bin autoresearch_loop`
- Opened `reports/autoresearch-top10-interactive-report.html` in Chrome via Playwright against the local file path
- Verified no browser console errors or page errors
- Verified 10 candidate cards render initially, candidate selection updates the briefing and active state, and asset filtering updates the visible count and frontier card set

Artifacts:
- `design/report-template.html`
- `reports/autoresearch-top10-interactive-report.html`
- `src/bin/autoresearch_loop.rs`
- `DESIGN_PLAN.md`
- `DESIGN_MEMORY.md`

## Top-Level paper-research CLI Bug Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3

Tasks:
- [x] T1 Reproduce the reported `zonk paper-research ...` failure and inspect how the top-level CLI currently parses `paper-research` `depends_on: []`
- [x] T2 Implement a compatible fix for direct `paper-research` invocation and add regression coverage if appropriate `depends_on: [T1]`
- [x] T3 Verify the exact reported command succeeds and document the result in a review section `depends_on: [T2]`

## Top-Level paper-research CLI Bug Review (2026-03-18)

Summary:
- Reproduced the failure exactly: `zonk paper-research ...` was rejected because `paper-research` only existed as a nested `run paper-research` subcommand in the top-level CLI parser.
- Added a top-level compatibility shortcut in `src/cli.rs` so `zonk paper-research ...` now routes to the same `PaperResearchArgs` parser as `zonk run paper-research ...`.
- Refactored `src/main.rs` to share strategy dispatch through a helper so both entry paths execute the same runtime behavior and elapsed-time handling.
- Added a regression test that parses the exact top-level invocation shape, including the trailing global `--output json` flag and the full set of `vol_spread` parameters.
- Refreshed the installed binary at `~/.cargo/bin/zonk` with `cargo install --path . --force --quiet` so the plain `zonk` command now picks up the fix in this environment.

Verification:
- `cargo fmt --all`
- `cargo test parse_top_level_paper_research_compat`
- `cargo build --quiet`
- `./target/debug/zonk paper-research --asset QQQ --rule vol_spread --fast-window 12 --slow-window 50 --rsi-window 14 --rsi-oversold 30 --rsi-overbought 70 --vol-window 10 --vol-cap 0.15 --hypothesis-id seed-24-2 --output json`
- `zonk paper-research --asset QQQ --rule vol_spread --fast-window 12 --slow-window 50 --rsi-window 14 --rsi-oversold 30 --rsi-overbought 70 --vol-window 10 --vol-cap 0.15 --hypothesis-id seed-24-2 --output json`

Artifacts:
- `src/cli.rs`
- `src/main.rs`

## design/ Relocation Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3

Tasks:
- [x] T1 Choose replacement locations for the former `branding/` assets and record the relocation plan `depends_on: []`
- [x] T2 Move the template/tokens/reference files to `design/` and update code/docs to the new paths `depends_on: [T1]`
- [x] T3 Verify no `branding` references remain, remove the old directory, and document the result `depends_on: [T2]`

## design/ Relocation Review (2026-03-18)

Summary:
- Removed the `branding/` directory entirely by relocating its three live assets to `design/`: `design/report-template.html`, `design/tokens.css`, and `design/brand-guidelines.html`.
- Updated the Rust autoresearch loop to load the report template from `design/report-template.html` instead of the deleted path.
- Rewrote all repo references that still pointed at `branding/` or the old directory guidance, including `AGENTS.md`, `CLAUDE.md`, `DESIGN.md`, `DESIGN_MEMORY.md`, `DESIGN_PLAN.md`, and the historical notes in `tasks/todo.md`.
- Left the relocated assets functionally intact; this was a path and documentation cleanup, not a visual redesign.

Verification:
- `rg -n "branding/|branding directory|\\bbranding\\b" .` returned no matches
- `find branding -maxdepth 2 -print 2>/dev/null || true` returned nothing
- `cargo test build_interactive_report_html_replaces_template_placeholders --bin autoresearch_loop`

Artifacts:
- `design/report-template.html`
- `design/tokens.css`
- `design/brand-guidelines.html`
- `src/bin/autoresearch_loop.rs`

## Editorial Intelligence + Figma Direction Plan (2026-03-18)

Dependency graph:
- T1 -> T2 -> T3

Tasks:
- [x] T1 Replace `DESIGN.md` with the new "Editorial Intelligence" design-system specification provided by the user `depends_on: []`
- [x] T2 Inspect Figma MCP availability and the referenced `Reports` file so the report direction can align to the source design `depends_on: [T1]`
- [x] T3 Update the ongoing report-template guidance to use the new spec and Figma direction going forward, then document the result `depends_on: [T2]`

## Editorial Intelligence + Figma Direction Review (2026-03-18)

Summary:
- Replaced `DESIGN.md` with the user-provided "Editorial Intelligence" design-system strategy verbatim.
- Checked for Figma MCP access in this session and found none: MCP resources/templates were empty, and direct access to the provided Figma URL returned `403`, so the actual Figma file could not be inspected from this environment.
- Updated repo-level design guidance in `AGENTS.md`, `CLAUDE.md`, and `DESIGN_MEMORY.md` so future report work treats `DESIGN.md` as the authority and the provided Figma `Reports` file as the external target format when it becomes accessible.
- Shifted the shared design token layer in `design/tokens.css` and the shared report generator template in `design/report-template.html` toward the new editorial system: Newsreader for display, Inter for body, Space Grotesk for labels/data, square-cornered structure, tonal surfaces, gradient hero treatment, and less boxed-in UI framing.
- Regenerated the current `reports/autoresearch-top10-interactive-report.html` from its embedded JSON so the live artifact picks up the updated template direction immediately.

Verification:
- `list_mcp_resources` returned no resources
- `list_mcp_resource_templates` returned no resource templates
- `curl -L -I <figma-url>` returned `HTTP/2 403`
- `cargo test build_interactive_report_html_replaces_template_placeholders --bin autoresearch_loop`
- Browser smoke check against `reports/autoresearch-top10-interactive-report.html`
- Verified the regenerated report loads without console/page errors and uses `Inter` for body, `Newsreader` for the hero title, and `Space Grotesk` for section labels

Artifacts:
- `DESIGN.md`
- `AGENTS.md`
- `CLAUDE.md`
- `DESIGN_MEMORY.md`
- `design/tokens.css`
- `design/report-template.html`
- `reports/autoresearch-top10-interactive-report.html`

## Figma Reports Implementation Plan (2026-03-19)

Dependency graph:
- T1 -> T2 -> T3

Tasks:
- [x] T1 Pull the referenced Figma `Reports` file context and inspect the current local report/template implementation `depends_on: []`
- [x] T2 Apply the Figma-informed design to the shared report template and regenerate the live report artifact `depends_on: [T1]`
- [x] T3 Verify the regenerated report in browser, document the result, and call out any remaining gaps versus Figma `depends_on: [T2]`

## Figma Reports Implementation Review (2026-03-19)

Summary:
- Pulled the referenced Figma `Reports` file through MCP and used concrete frame metadata plus design context for the body shell, top app bar, hero section, metrics row, filter strip, primary visualization, candidate briefing cards, and audit rail.
- Replaced the old command-center template in `design/report-template.html` with a Figma-aligned editorial layout: left curator rail, glass top navigation, asymmetrical hero, four-metric strip, primary alpha-vector chart, hidden grid configuration drawer, two-column candidate briefing cards, and an audit rail that keeps proof surfaces visible.
- Kept the report honest to live data instead of reproducing placeholder Figma copy verbatim. The chart is a normalized profile synthesized from audited summary metrics, and the metrics/telemetry labels are mapped to fields the report actually has today.
- Regenerated `reports/autoresearch-top10-interactive-report.html` from the updated binary so the live artifact now carries the new design system rather than only the template source.
- Updated the stale regression assertion in `src/bin/autoresearch_loop.rs` so the template test now checks for the new report title instead of the retired `Autoresearch Command Center` string.

Verification:
- `cargo test --bin autoresearch_loop`
- `cargo test`
- `cargo build --bin autoresearch_loop --quiet`
- `target/debug/autoresearch_loop --seed-web --candidates 100 --top 10 --verbose`
- Playwright smoke check against `reports/autoresearch-top10-interactive-report.html`
- Verified browser summary: title `zonk Strategy Frontier · 10 visible`, hero `The Strategy Frontier`, `4` metric cards, `10` candidate cards, audit heading `Audit Rail`, selected title `vol_spread`, configure control text `Configure Grid`
- Verified no browser console errors or page errors
- Verified visually from a full-page screenshot that the new shell renders the intended Figma-derived composition with sidebar, hero, metrics, vector panel, candidate grid, and audit rail

Remaining gaps vs Figma:
- The report preserves a hidden configuration drawer behind `Configure Grid` so filtering/searching remains usable; the Figma frame shows only the collapsed strip state.
- The primary alpha chart uses a normalized profile derived from available report metrics because the single-file report payload does not yet embed full equity traces inline.
- The audit rail extends beyond the visible Figma crop to retain execution-window proof and artifact links that the project requires for auditability.

Artifacts:
- `design/report-template.html`
- `reports/autoresearch-top10-interactive-report.html`
- `src/bin/autoresearch_loop.rs`

## Lessons
_(Move to `tasks/lessons.md` as they accumulate)_
