use chrono::{Datelike, Duration, NaiveDate, Utc};
use clap::Parser;
use comfy_table::{ContentArrangement, Table, presets::UTF8_FULL};
use dotenvy::dotenv;
use reqwest::blocking::Client;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    env,
    fs::File,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
};

/// Research Analysis Framework
///
/// This framework governs how every piece of academic research is evaluated, translated into
/// a trading strategy, and backtested. All narrative functions in this module implement the
/// five steps defined here. The framework is applied by:
///
/// - `research_basis()`        → Step 1: Mental Model Synthesis
/// - `critical_evaluation()`   → Step 2: Critical Evaluation
/// - `rule_description()`      → Step 3: The Trading Strategy (entry/exit signal)
/// - `investment_case()`       → Step 4: Asset Universe & Inputs/Outputs
/// - `backtest_architecture()` → Step 5: Rust Backtest Architecture (zonk pipeline)
/// - `profitability_blurb()`   → Quantitative performance summary across train/test windows
///
/// When processing any research paper or hypothesis, each function must reason from the
/// perspective defined in RESEARCH_ANALYSIS_FRAMEWORK: a Senior Quantitative Researcher,
/// Derivatives Trader, and Rust Algorithmic Trading Architect with 30 years of experience.
const RESEARCH_ANALYSIS_FRAMEWORK: &str = r#"
Role: Act as a Senior Quantitative Researcher, Derivatives Trader, and Rust Algorithmic Trading Architect with 30 years of experience.

Context: I am providing you with academic quantitative research (e.g., from ArXiv). I need you to evaluate the research, extract the core alpha or predictive edge, and translate it into a systematic, deployable trading strategy.
- Target Asset Classes: Stocks, Options, and/or Futures. You must select the optimal vehicles based on the strategy's timeframe and execution mechanics.
- Tech Stack: I am building this in a custom Rust framework called `zonk`. The framework leverages `polars` (with lazy, temporal, and rolling_window features) for vectorized data wrangling, `nalgebra` for matrix math, and `reqwest` for data ingestion.

Task: Read the provided research. Build a rigorous mental model of the core concepts, critically evaluate the methodology, synthesize a tradable strategy, define the exact inputs/outputs, and output the programmatic data pipeline for a Rust-based backtest.

Constraints/Formatting:
- Use a highly professional, academic, yet practical quant tone.
- Format the output using clear Markdown headings, bullet points, and code blocks for Rust pseudo-code.
- Cite specific claims, metrics, or formulas from the provided text to justify your design.
- Do not provide generic advice. Give exact parameters, lookback periods, thresholds, and asset tickers based on your synthesis of the paper.

Steps:
1. Mental Model Synthesis: Summarize the core thesis of the paper. What specific market inefficiency, anomaly, or predictive edge is the author claiming to have found? Explain the theoretical mechanism behind it.
2. Critical Evaluation: Stress-test the academic claims. Are there obvious risks of overfitting, lookahead bias, or regime-dependency? Did the authors likely ignore realistic transaction costs or slippage?
3. The Trading Strategy: Define a specific, tradable strategy derived from this research. What is the precise entry/exit signal? Which specific instruments (e.g., ES futures, SPX straddles, individual equities) are best suited to capture this edge while minimizing friction?
4. Asset Universe & Inputs/Outputs: List the exact financial data inputs required (e.g., 1-minute OHLCV, daily interest rates, alternative data). Define the target output/signal (e.g., continuous portfolio weight, binary long/short trigger).
5. Rust Backtest Architecture (`zonk` framework): Outline the code architecture required to backtest this using `polars` in Rust. Provide a structured pipeline:
   - Data ingestion & Joining
   - Feature Engineering (translating the paper's math into `polars` expressions)
   - Signal Generation
   - PnL Simulation logic (including explicit handling of the transaction costs identified in step 2).
"#;

const RESEARCH_STRATEGY: &str = "paper-research";
const RULE_TREND_MOMENTUM: &str = "trend_momentum";
const RULE_TREND_PULLBACK: &str = "trend_pullback";
const RULE_RSI_REVERSION: &str = "rsi_reversion";
const RULE_VOL_REGIME: &str = "volatility_regime";
const RULE_VOL_SPREAD: &str = "vol_spread";
const RULE_MEAN_REVERSION: &str = "mean_reversion_filter";
const RULE_VVIX_REGIME: &str = "vvix_regime";

const SEED_QUERIES: &[&str] = &[
    "site:arxiv.org VIX trading strategy options volatility",
    "site:arxiv.org volatility risk premium harvesting strategy",
    "site:arxiv.org implied volatility realized volatility spread trading",
    "site:arxiv.org VIX futures term structure contango backwardation",
    "site:arxiv.org volatility regime switching trading signal",
    "site:arxiv.org mean reversion VIX volatility index strategy",
    "site:arxiv.org tail risk hedging VIX options equity",
    "site:arxiv.org CBOE volatility index prediction machine learning",
];

const MIN_CANDIDATES_TARGET_DEFAULT: usize = 100;
const DEFAULT_TRAIN_SESSIONS: i64 = 1008;
const DEFAULT_TEST_SESSIONS: i64 = 252;
const FAST_WINDOW_SET: &[u32] = &[6, 8, 10, 12, 14, 16, 20, 24, 30, 35];
const SLOW_WINDOW_SET: &[u32] = &[18, 25, 35, 50, 70, 90, 120, 160, 220];
const RSI_WINDOW_SET: &[u32] = &[8, 10, 12, 14, 16, 20, 22, 24];
const VOL_WINDOW_SET: &[u32] = &[10, 14, 18, 20, 22, 24, 30, 35, 40];
const RSI_OVERSOLD_SET: &[u32] = &[18, 20, 22, 24, 26, 28, 30, 32, 35];
const RSI_OVERBOUGHT_SET: &[u32] = &[60, 65, 68, 70, 72, 74, 76, 78, 80];
const VOL_CAP_SET: &[f64] = &[0.20, 0.25, 0.30, 0.35, 0.40, 0.45, 0.50, 0.60, 0.70, 0.80];
const VOL_CAP_SPREAD_SET: &[f64] = &[
    -0.30, -0.20, -0.10, 0.10, 0.15, 0.20, 0.25, 0.30, 0.40, 0.50, 0.60, 0.80,
];
const MR_ENTRY_THRESHOLD_SET: &[f64] = &[0.005, 0.01, 0.015, 0.02, 0.025, 0.03, 0.04, 0.05];
const MR_SCALE_SET: &[f64] = &[50.0, 100.0, 150.0, 200.0, 300.0, 500.0];
const VVIX_WINDOW_SET: &[u32] = &[21, 42, 63, 126, 189, 252];
const VVIX_THRESHOLD_SET: &[f64] = &[0.50, 0.60, 0.70, 0.75, 0.80, 0.85, 0.90];
const RESEARCH_ASSETS: &[&str] = &["SPY", "QQQ", "SPXL", "IWM", "TQQQ"];
const INTERACTIVE_REPORT_TEMPLATE: &str = include_str!("../../design/report-template.html");

#[derive(Clone, Debug)]
struct Candidate {
    candidate_id: String,
    strategy: String,
    rule: String,
    args: Vec<String>,
    rationale: String,
    source: String,
    focus_asset: String,
    is_seeded: bool,
    _min_signals: u32,
    _min_observations: u32,
}

#[derive(Clone, Debug)]
struct ScoredRun {
    score: f64,
    details: Value,
}

#[derive(Clone, Serialize, Deserialize)]
struct AuditWindowMeta {
    artifact_path: String,
    trade_count: usize,
    actual_period_start: String,
    actual_period_end: String,
}

#[derive(Clone, Serialize)]
struct CandidateReport {
    candidate_id: String,
    strategy: String,
    category: String,
    args: Vec<String>,
    rule: String,
    rationale: String,
    source: String,
    focus_asset: String,
    train_score: f64,
    test_score: f64,
    combined_score: f64,
    train_details: Value,
    test_details: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    train_audit: Option<AuditWindowMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    test_audit: Option<AuditWindowMeta>,
    is_seeded: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct StrategyRegistrySnapshot {
    observed_at: String,
    candidate_id: String,
    source: String,
    rationale: String,
    round: Option<u32>,
    combined_score: f64,
    train_score: f64,
    test_score: f64,
    train_details: Value,
    test_details: Value,
    train_audit: Option<AuditWindowMeta>,
    test_audit: Option<AuditWindowMeta>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StrategyRegistryEntry {
    signature: String,
    registry_status: String,
    strategy: String,
    rule: String,
    focus_asset: String,
    args: Vec<String>,
    is_seeded: bool,
    candidate_ids: Vec<String>,
    sources: Vec<String>,
    first_top10_at: String,
    last_top10_at: String,
    times_in_top10: u64,
    latest: StrategyRegistrySnapshot,
    best: StrategyRegistrySnapshot,
}

#[derive(Default, Serialize, Deserialize)]
struct StrategyRegistry {
    updated_at: String,
    entries: Vec<StrategyRegistryEntry>,
}

#[derive(Deserialize, Serialize)]
struct ExaSeed {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct ExaResponse {
    #[serde(default)]
    results: Vec<ExaSeed>,
}

#[derive(Clone)]
struct RecordedResult {
    round: Option<u32>,
    signature: String,
    report: CandidateReport,
}

struct RoundSummary {
    round: u32,
    generated: usize,
    evaluated: usize,
    passed: usize,
    round_best: Option<f64>,
    global_best: Option<f64>,
    rel_improvement: f64,
}

struct LoopState {
    #[allow(dead_code)]
    round: u32,
    all_evaluated: HashSet<String>,
    exhausted_centers: HashSet<String>,
    recorded_results: Vec<RecordedResult>,
    round_summaries: Vec<RoundSummary>,
}

impl LoopState {
    fn new() -> Self {
        Self {
            round: 0,
            all_evaluated: HashSet::new(),
            exhausted_centers: HashSet::new(),
            recorded_results: Vec::new(),
            round_summaries: Vec::new(),
        }
    }

    fn current_best_score(&self) -> Option<f64> {
        self.recorded_results
            .iter()
            .map(|r| r.report.combined_score)
            .fold(None, |acc, score| match acc {
                None => Some(score),
                Some(best) => Some(if score > best { score } else { best }),
            })
    }

    fn retain_novel_candidates(&mut self, candidates: Vec<Candidate>) -> Vec<Candidate> {
        candidates
            .into_iter()
            .filter(|c| {
                let sig = param_signature(c);
                self.all_evaluated.insert(sig)
            })
            .collect()
    }

    fn record_round(
        &mut self,
        round: u32,
        generated: usize,
        evaluated: usize,
        round_reports: Vec<CandidateReport>,
        legacy_mode: bool,
    ) {
        let passed = round_reports.len();
        let round_best = round_reports
            .iter()
            .map(|r| r.combined_score)
            .fold(None::<f64>, |acc, s| Some(acc.map_or(s, |a: f64| a.max(s))));

        for report in round_reports {
            let sig = param_signature_from_args(&report.args);
            self.recorded_results.push(RecordedResult {
                round: if legacy_mode { None } else { Some(round) },
                signature: sig,
                report,
            });
        }

        let global_best = self.current_best_score();
        let prev_best = if self.round_summaries.is_empty() {
            None
        } else {
            self.round_summaries.last().and_then(|s| s.global_best)
        };

        let rel_improvement = match (prev_best, global_best) {
            (Some(prev), Some(curr)) => (curr - prev) / prev.abs().max(1.0),
            _ => 0.0,
        };

        self.round_summaries.push(RoundSummary {
            round,
            generated,
            evaluated,
            passed,
            round_best,
            global_best,
            rel_improvement,
        });
        self.round = round;
    }

    fn mark_center_exhausted(&mut self, signature: &str) {
        self.exhausted_centers.insert(signature.to_string());
    }

    fn is_center_exhausted(&self, signature: &str) -> bool {
        self.exhausted_centers.contains(signature)
    }
}

#[derive(Parser)]
struct Args {
    #[arg(long, default_value_t = String::from("target/release/zonk"))]
    zonk_bin: String,

    #[arg(long, default_value_t = MIN_CANDIDATES_TARGET_DEFAULT)]
    candidates: usize,

    #[arg(long, default_value_t = 10)]
    top: usize,

    #[arg(long)]
    seed_web: bool,

    #[arg(long)]
    include_grid: bool,

    #[arg(long, default_value_t = 17)]
    random_seed: u64,

    #[arg(long, default_value = "2020-01-01")]
    train_start: String,

    #[arg(long, default_value = "2024-12-31")]
    train_end: String,

    #[arg(long, default_value = "2025-01-01")]
    test_start: String,

    #[arg(long, default_value = "2026-03-11")]
    test_end: String,

    #[arg(long, default_value_t = DEFAULT_TRAIN_SESSIONS)]
    train_sessions: i64,

    #[arg(long, default_value_t = DEFAULT_TEST_SESSIONS)]
    test_sessions: i64,

    #[arg(long)]
    verbose: bool,

    /// Maximum refinement rounds (default: 10). Looping is the new default; use --no-loop for legacy single-pass
    #[arg(long, default_value_t = 10)]
    max_rounds: u32,

    /// Rounds without meaningful improvement before stopping (default: 3)
    #[arg(long, default_value_t = 3)]
    patience: u32,

    /// Minimum relative improvement to reset patience counter (default: 0.02)
    #[arg(long, default_value_t = 0.02)]
    min_improvement: f64,

    /// Number of top winners to refine each round (default: 5)
    #[arg(long, default_value_t = 5)]
    refine_top: usize,

    /// Maximum refinement variants per winner (default: 30)
    #[arg(long, default_value_t = 30)]
    refine_variants: usize,

    /// Disable iterative refinement (single-pass legacy mode)
    #[arg(long)]
    no_loop: bool,

    /// Disable evaluation cache (re-evaluate all candidates from scratch)
    #[arg(long)]
    no_cache: bool,

    /// Asset universe for refinement: core (5 tickers), broad (SP500+NDX100 ~550),
    /// full (all viable warehouse symbols), or a preset name
    #[arg(long, default_value = "broad")]
    asset_universe: String,

    /// Max asset swap variants per winner per refinement round
    #[arg(long, default_value_t = 10)]
    refine_asset_swaps: usize,

    /// Minimum parquet rows for asset viability (default: auto = train+test sessions)
    #[arg(long)]
    min_asset_rows: Option<usize>,

    /// Minimum test-window Sharpe ratio for a strategy to be "investable" (default: none)
    #[arg(long)]
    min_sharpe: Option<f64>,

    /// Maximum test-window drawdown (absolute %) for a strategy to be "investable" (default: none).
    /// e.g. --max-drawdown 20 rejects strategies with test drawdown worse than -20%
    #[arg(long)]
    max_drawdown: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone)]
struct CachedEval {
    eval_key: String,
    passed: bool,
    train_score: f64,
    test_score: f64,
    combined_score: f64,
    train_details: Value,
    test_details: Value,
}

const EVAL_CACHE_PATH: &str = "reports/autoresearch-eval-cache.jsonl";
const STRATEGY_REGISTRY_PATH: &str = "reports/autoresearch-strategy-registry.json";
const DEFAULT_PAPER_RESEARCH_BEGINNING_EQUITY: f64 = 1_000_000.0;
const SEED_CLASSIFICATION_FALLBACK_CHARS: usize = 1600;

fn eval_cache_key(
    signature: &str,
    train_start: &str,
    train_end: &str,
    test_start: &str,
    test_end: &str,
) -> String {
    format!(
        "{sig}|{ts}|{te}|{xs}|{xe}",
        sig = signature,
        ts = train_start,
        te = train_end,
        xs = test_start,
        xe = test_end
    )
}

fn load_eval_cache(path: &Path) -> HashMap<String, CachedEval> {
    let mut cache = HashMap::new();
    let Ok(content) = std::fs::read_to_string(path) else {
        return cache;
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(mut entry) = serde_json::from_str::<CachedEval>(line) {
            entry.train_details = backfill_beginning_equity(&entry.train_details);
            entry.test_details = backfill_beginning_equity(&entry.test_details);
            cache.insert(entry.eval_key.clone(), entry);
        }
    }
    cache
}

fn append_eval_cache(path: &Path, entry: &CachedEval) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = File::options().append(true).create(true).open(path) {
        let _ = writeln!(
            file,
            "{}",
            serde_json::to_string(entry).unwrap_or_else(|_| "{}".to_string())
        );
    }
}

fn cached_evaluate_candidate(
    candidate: &Candidate,
    idx: usize,
    total: usize,
    zonk_bin: &Path,
    train_start: &str,
    train_end: &str,
    test_start: &str,
    test_end: &str,
    train_sessions: i64,
    test_sessions: i64,
    verbose: bool,
    cache: &HashMap<String, CachedEval>,
    cache_path: &Path,
) -> Option<CandidateReport> {
    let sig = param_signature(candidate);
    let key = eval_cache_key(&sig, train_start, train_end, test_start, test_end);

    if let Some(hit) = cache.get(&key) {
        if verbose {
            let status = if hit.passed { "PASS" } else { "SKIP" };
            println!(
                "  [cache {status}] candidate {}/{}: {} (score={:.3})",
                idx, total, candidate.candidate_id, hit.combined_score
            );
        }
        if !hit.passed {
            return None;
        }
        return Some(CandidateReport {
            candidate_id: candidate.candidate_id.clone(),
            strategy: candidate.strategy.clone(),
            category: strategy_category(&candidate.strategy).to_string(),
            args: candidate.args.clone(),
            rule: candidate.rule.clone(),
            rationale: candidate.rationale.clone(),
            source: candidate.source.clone(),
            focus_asset: candidate.focus_asset.clone(),
            train_score: hit.train_score,
            test_score: hit.test_score,
            combined_score: hit.combined_score,
            train_details: hit.train_details.clone(),
            test_details: hit.test_details.clone(),
            train_audit: None,
            test_audit: None,
            is_seeded: candidate.is_seeded,
        });
    }

    // Cache miss — evaluate normally
    let result = evaluate_candidate(
        candidate,
        idx,
        total,
        zonk_bin,
        train_start,
        train_end,
        test_start,
        test_end,
        train_sessions,
        test_sessions,
        verbose,
    );

    // Write to cache
    let entry = match &result {
        Some(report) => CachedEval {
            eval_key: key,
            passed: true,
            train_score: report.train_score,
            test_score: report.test_score,
            combined_score: report.combined_score,
            train_details: report.train_details.clone(),
            test_details: report.test_details.clone(),
        },
        None => CachedEval {
            eval_key: key,
            passed: false,
            train_score: 0.0,
            test_score: 0.0,
            combined_score: 0.0,
            train_details: json!({}),
            test_details: json!({}),
        },
    };
    append_eval_cache(cache_path, &entry);

    result
}

fn safe_float(value: &Value) -> Option<f64> {
    match value {
        Value::Number(num) => num.as_f64().filter(|v| v.is_finite()),
        Value::String(s) => s.parse::<f64>().ok().filter(|v| v.is_finite()),
        _ => None,
    }
}

fn backfill_beginning_equity(details: &Value) -> Value {
    let mut upgraded = details.clone();
    if let Some(object) = upgraded.as_object_mut() {
        let beginning_equity = object
            .get("beginning_equity")
            .and_then(safe_float)
            .unwrap_or(0.0);
        let final_equity = object
            .get("final_equity")
            .and_then(safe_float)
            .unwrap_or(0.0);
        if beginning_equity <= 0.0 && final_equity > 0.0 {
            object.insert(
                "beginning_equity".to_string(),
                json!(DEFAULT_PAPER_RESEARCH_BEGINNING_EQUITY),
            );
        }
    }
    upgraded
}

fn arg_value<'a>(args: &'a [String], flag: &'a str) -> Option<&'a str> {
    let mut iter = args.iter();
    while let Some(v) = iter.next() {
        if v == flag {
            return iter.next().map(String::as_str);
        }
    }
    None
}

fn arg_u32(args: &[String], flag: &str) -> Option<u32> {
    arg_value(args, flag).and_then(|value: &str| value.parse::<u32>().ok())
}

fn arg_f64(args: &[String], flag: &str) -> Option<f64> {
    arg_value(args, flag).and_then(|value: &str| value.parse::<f64>().ok())
}

fn arg_str(args: &[String], flag: &str) -> Option<String> {
    arg_value(args, flag).map(|v| v.to_string())
}

fn fmt_pct(v: f64) -> String {
    format!("{:.1}%", v.abs() * 100.0)
}

fn fmt_num(v: f64) -> String {
    format!("{:.3}", v)
}

/// Step 3 of RESEARCH_ANALYSIS_FRAMEWORK: The Trading Strategy.
///
/// Defines the precise entry/exit signal and the instruments used to capture the edge.
fn rule_description(rule: &str, args: &[String], asset: &str) -> String {
    match rule {
        RULE_TREND_MOMENTUM => {
            let fast = arg_u32(args, "--fast-window").unwrap_or(12);
            let slow = arg_u32(args, "--slow-window").unwrap_or(40);
            format!(
                "Trend momentum on {asset}: buys when the {fast}-day moving average is above the {slow}-day average and price is above the short MA, aiming to ride persistent up-trends while exiting on MA crosses."
            )
        }
        RULE_TREND_PULLBACK => {
            let fast = arg_u32(args, "--fast-window").unwrap_or(12);
            let slow = arg_u32(args, "--slow-window").unwrap_or(40);
            format!(
                "Trend pullback on {asset}: buys when price drops below the {fast}-day MA but stays above the {slow}-day MA, capturing controlled dip re-entries inside a larger uptrend."
            )
        }
        RULE_RSI_REVERSION => {
            let rsi_window = arg_u32(args, "--rsi-window").unwrap_or(14);
            let oversold = arg_f64(args, "--rsi-oversold").unwrap_or(35.0);
            format!(
                "RSI reversion on {asset}: enters positions when RSI({rsi_window}) falls below {oversold:.0}, targeting mean-reversion from oversold conditions."
            )
        }
        RULE_VOL_REGIME => {
            let vol_window = arg_u32(args, "--vol-window").unwrap_or(20);
            let vol_cap = arg_f64(args, "--vol-cap").unwrap_or(0.45);
            format!(
                "Volatility regime on {asset}: holds through calmer market states, only trading when realized {vol_window}-day volatility stays within the lowest {:.0}% regime (vol_cap).",
                (vol_cap * 100.0).round()
            )
        }
        RULE_VOL_SPREAD => {
            let vol_window = arg_u32(args, "--vol-window").unwrap_or(20);
            let vol_cap = arg_f64(args, "--vol-cap").unwrap_or(0.20);
            if vol_cap >= 0.0 {
                format!(
                    "VRP harvest on {asset}: goes long when VIX-implied volatility exceeds {vol_window}-day realized volatility by more than {:.0}%, capturing the variance risk premium as implied vol mean-reverts toward realized.",
                    (vol_cap * 100.0).round()
                )
            } else {
                format!(
                    "Volatility snap-back on {asset}: goes long when {vol_window}-day realized volatility overshoots VIX-implied by more than {:.0}%, betting on a reversion as turbulence subsides.",
                    (vol_cap.abs() * 100.0).round()
                )
            }
        }
        RULE_MEAN_REVERSION => {
            let slow = arg_u32(args, "--slow-window").unwrap_or(40);
            let mr_threshold = arg_f64(args, "--mr-entry-threshold").unwrap_or(0.02);
            format!(
                "Mean reversion filter on {asset}: computes fair value as {slow}-day SMA, then goes long when price drops more than {:.1}% below fair value, capturing reversion to the filtered mean.",
                mr_threshold * 100.0
            )
        }
        RULE_VVIX_REGIME => {
            let vvix_window = arg_u32(args, "--vvix-window").unwrap_or(63);
            let vvix_threshold = arg_f64(args, "--vvix-threshold").unwrap_or(0.75);
            let mode = arg_str(args, "--vvix-mode").unwrap_or("risk_off".to_string());
            if mode == "contrarian" {
                format!(
                    "VVIX regime (contrarian) on {asset}: goes long when VVIX exceeds its {:.0}th percentile over {vvix_window} days, betting that elevated vol-of-vol signals underpriced variance and a subsequent equity bounce.",
                    vvix_threshold * 100.0
                )
            } else {
                format!(
                    "VVIX regime (risk-off) on {asset}: goes long when VVIX is below its {:.0}th percentile over {vvix_window} days, capturing equity premium during calm vol-of-vol conditions.",
                    vvix_threshold * 100.0
                )
            }
        }
        _ => format!("Paper-research candidate on {asset} with adaptive research-rule logic."),
    }
}

/// Step 4 of RESEARCH_ANALYSIS_FRAMEWORK: Asset Universe & Inputs/Outputs.
///
/// Describes the market inefficiency exploited, the financial data inputs required,
/// and the theoretical justification for selecting the target asset.
fn investment_case(rule: &str, asset: &str) -> String {
    match rule {
        RULE_TREND_MOMENTUM => format!(
            "Trend-following strategies on {asset} exploit the empirical tendency for asset prices to persist in directional moves. \
            By requiring alignment between short- and long-term moving averages, the strategy filters out choppy markets and enters \
            only when momentum is confirmed. This approach is favored by systematic traders seeking to capture the bulk of sustained \
            rallies while avoiding whipsaw losses during sideways periods."
        ),
        RULE_TREND_PULLBACK => format!(
            "Pullback entries on {asset} target temporary dips within an established uptrend — buying the short-term weakness while \
            the longer-term trend remains intact. This is a core institutional technique: it offers better risk/reward than chasing \
            breakouts because the entry price is lower and the stop is tighter. The strategy assumes that the dominant trend will \
            resume after a brief correction, a pattern well-documented in equity markets."
        ),
        RULE_RSI_REVERSION => format!(
            "Mean-reversion via RSI on {asset} exploits the tendency for oversold conditions to precede short-term bounces. When \
            RSI drops to extreme lows, selling pressure is typically exhausted and a reflexive recovery follows. This strategy is \
            particularly effective in liquid, large-cap instruments where panic selling creates predictable snap-back opportunities. \
            It provides diversification from trend-following approaches by profiting from the opposite market dynamic."
        ),
        RULE_VOL_REGIME => format!(
            "Volatility-regime filtering on {asset} restricts exposure to periods of low realized volatility, avoiding the fat-tailed \
            losses that occur during high-volatility regimes. The premise is that returns are more favorable and predictable in calm \
            markets. By stepping aside when volatility spikes, the strategy sacrifices some upside participation in exchange for \
            dramatically reduced drawdowns — a trade-off valued by risk-conscious portfolio managers."
        ),
        RULE_VOL_SPREAD => format!(
            "The Variance Risk Premium (VRP) on {asset} exploits the well-documented tendency for VIX-implied volatility to \
            overstate subsequently realized volatility. Carr & Wu (2009) established that this spread is persistently positive \
            and represents compensation for bearing volatility risk. Recent GARCH-LSTM hybrid forecasting models (arXiv 2407.16780) \
            demonstrate that the spread is predictable and tradeable. This strategy enters long positions when the implied-vs-realized \
            spread exceeds a threshold, harvesting the premium as implied vol mean-reverts toward realized. The negative-threshold \
            variant captures snap-back opportunities when realized volatility overshoots implied, signaling an imminent return to \
            calmer conditions."
        ),
        RULE_MEAN_REVERSION => format!(
            "Mean reversion filter on {asset} implements the fair-value deviation framework from Xu et al. (2026, SSRN 6225198). \
            The strategy treats a smoothed price (SMA) as a proxy for latent fair value, then computes the relative mispricing \
            δ = (spot − fair_value) / fair_value. When price drops significantly below fair value (δ < −threshold), the strategy \
            enters long, expecting reversion. This exploits the empirically documented tendency for short-term price deviations \
            from moving averages to revert, particularly in liquid, large-cap instruments. The approach is grounded in weak-form \
            efficiency: prices contain transient deviations that create predictable snap-back opportunities."
        ),
        RULE_VVIX_REGIME => format!(
            "VVIX regime filter on {asset} implements findings from Bevilacqua & Hizmeri (2026, SSRN 6212458). \
            The paper documents that morning VVIX (volatility-of-volatility) strongly predicts next-day variance asset \
            returns with t-stats up to 6.1. The mechanism is limited attention and slow-moving beliefs: investors fail \
            to promptly incorporate VVIX uncertainty into prices. In risk-off mode, the strategy avoids equity exposure \
            during elevated VVIX (when uncertainty is high and variance is underpriced). In contrarian mode, it enters \
            long when VVIX is high, targeting the delayed price adjustment documented in the paper."
        ),
        _ => format!(
            "This paper-research candidate on {asset} applies an adaptive signal derived from academic research to identify \
            favorable entry and exit conditions. The strategy is designed to exploit empirically-documented market patterns \
            with systematic, rules-based execution."
        ),
    }
}

/// Step 2 of RESEARCH_ANALYSIS_FRAMEWORK: Critical Evaluation.
///
/// Stress-tests the academic claims behind each rule. Identifies risks of overfitting,
/// lookahead bias, regime-dependency, and whether the authors ignored realistic
/// transaction costs or slippage.
fn critical_evaluation(rule: &str, asset: &str, args: &[String]) -> String {
    let vol_window = arg_u32(args, "--vol-window").unwrap_or(20);
    let fast = arg_u32(args, "--fast-window").unwrap_or(12);
    let slow = arg_u32(args, "--slow-window").unwrap_or(50);

    let common_tail = format!(
        "Transaction costs are modeled using the IBKR Tiered fee schedule with per-share commissions \
        and exchange fees. Slippage is not explicitly modeled but is mitigated by trading highly liquid \
        instruments ({asset}) at daily close prices. The train/test split (2020–2024 train, 2025+ test) \
        guards against in-sample overfitting, though the test window remains short."
    );

    match rule {
        RULE_TREND_MOMENTUM => format!(
            "Regime dependency is the primary risk: dual-MA crossover ({fast}/{slow}) systems are well-documented \
            to underperform in range-bound markets where frequent whipsaws erode returns. The parameter space \
            (fast × slow window) is large enough that naive grid search risks overfitting to the specific \
            volatility regime of the training window. Lookahead bias is absent — signals use only past closes. \
            {common_tail}"
        ),
        RULE_TREND_PULLBACK => format!(
            "Pullback strategies assume the dominant trend resumes after a temporary dip. This assumption \
            fails during genuine trend reversals, where the pullback entry becomes a falling-knife trade. \
            The {fast}-day / {slow}-day MA framework introduces two degrees of freedom; overfitting risk \
            is moderate. The strategy also implicitly assumes sufficient liquidity for daily rebalancing \
            without adverse price impact. {common_tail}"
        ),
        RULE_RSI_REVERSION => {
            let rsi_window = arg_u32(args, "--rsi-window").unwrap_or(14);
            let rsi_oversold = arg_f64(args, "--rsi-oversold").unwrap_or(30.0);
            format!(
                "Mean-reversion via RSI({rsi_window}) < {rsi_oversold:.0} carries significant regime risk: \
                oversold conditions in bear markets often precede further declines rather than bounces. \
                The strategy's edge is strongest in range-bound or mildly trending markets. Parameter \
                sensitivity is high — small changes in RSI window or threshold can materially alter signal \
                frequency and performance. Authors of RSI-based strategies typically ignore the impact of \
                leverage decay in instruments like TQQQ, which compounds daily and can erode edge over \
                multi-day holding periods. {common_tail}"
            )
        }
        RULE_VOL_REGIME => format!(
            "Volatility-regime strategies depend on the assumption that low-vol periods offer superior \
            risk-adjusted returns. This is regime-dependent by design: the {vol_window}-day realized vol \
            filter may miss sudden volatility spikes (gap risk) since it relies on trailing data. \
            The percentile-based threshold adds a non-parametric layer that is relatively robust to \
            distributional assumptions but may be slow to adapt to structural market changes. \
            Overfitting risk is moderate given two free parameters (vol_window, vol_cap). {common_tail}"
        ),
        RULE_VOL_SPREAD => format!(
            "The Variance Risk Premium (VRP) is one of the most well-documented anomalies in finance, \
            reducing overfitting concerns relative to purely technical signals. However, the spread between \
            VIX-implied and {vol_window}-day realized vol is noisy at short horizons and can produce false \
            signals during volatility regime transitions. Key risks: (1) VIX reflects 30-day implied vol \
            while realized vol is computed over a {vol_window}-day window — this horizon mismatch can \
            introduce systematic bias; (2) the VRP compresses during crisis periods when both implied and \
            realized vol spike, reducing signal effectiveness precisely when risk management matters most; \
            (3) negative-threshold snap-back entries assume mean-reversion in volatility, which can fail \
            during structural breaks. The GARCH-LSTM hybrid approach cited in the literature uses forward-looking \
            model selection that would constitute lookahead bias in a live trading context. {common_tail}"
        ),
        RULE_MEAN_REVERSION => {
            let mr_threshold = arg_f64(args, "--mr-entry-threshold").unwrap_or(0.02);
            format!(
                "Mean reversion filter carries regime risk: in sustained bear markets, price can remain \
                below the SMA for extended periods, turning every entry into a losing trade. The {slow}-day \
                SMA window is the primary degree of freedom — too short amplifies whipsaw, too long delays \
                entry past the reversion window. The entry threshold ({:.1}%) adds a second parameter; \
                combined with window length this creates moderate overfitting risk. The strategy implicitly \
                assumes stationarity of the deviation process, which breaks during structural shifts. \
                The original Xu et al. (2026) paper uses neural filters (CNN, WaveNet, DKF) rather than \
                simple SMA, so this implementation is a simplified proxy — the SMA approximation loses \
                the adaptive regime-switching capability of the neural filters. {common_tail}",
                mr_threshold * 100.0
            )
        }
        RULE_VVIX_REGIME => {
            let vvix_window = arg_u32(args, "--vvix-window").unwrap_or(63);
            format!(
                "The VVIX regime filter carries several risks. (1) The paper's alpha is in variance assets \
                (VIX futures, SPX straddles), not equities; applying the signal to equity ETFs is an indirect \
                adaptation with likely lower Sharpes. (2) The paper specifically uses 10:00 EST VVIX; daily \
                close VVIX may capture less alpha since predictive power dissipates by midday. (3) The \
                {vvix_window}-day percentile window is a non-parametric threshold that can be slow to adapt \
                to structural changes in the VVIX distribution. (4) VVIX data availability is sparser than \
                VIX, and missing data points produce NaN gaps that reduce signal coverage. The paper's Sharpe \
                ratios (2.0-3.2) are for variance assets; equity adaptation will be materially lower. \
                {common_tail}"
            )
        }
        _ => format!(
            "This candidate uses an adaptive research-derived signal on {asset}. Standard risks apply: \
            parameter overfitting to the training window, regime-dependency of the underlying signal, \
            and potential divergence between academic backtest assumptions and live execution realities. \
            {common_tail}"
        ),
    }
}

/// Step 5 of RESEARCH_ANALYSIS_FRAMEWORK: Rust Backtest Architecture.
///
/// Outlines the zonk pipeline for each rule: data ingestion, feature engineering,
/// signal generation, and PnL simulation with transaction cost handling.
fn backtest_architecture(rule: &str, asset: &str, args: &[String]) -> String {
    let vol_window = arg_u32(args, "--vol-window").unwrap_or(20);
    let fast = arg_u32(args, "--fast-window").unwrap_or(12);
    let slow = arg_u32(args, "--slow-window").unwrap_or(50);

    let common_pipeline = format!(
        "Data ingestion: load_ticker_ohlcv(\"{asset}\") reads daily OHLCV from \
        ~/market-warehouse bronze parquet via polars LazyFrame::scan_parquet(). \
        PnL simulation: daily mark-to-market with IBKR tiered fee model \
        (ibkr_roundtrip_cost per rebalance). Signal → binary long/flat mask; equity curve \
        tracks shares × close + cash remainder."
    );

    match rule {
        RULE_TREND_MOMENTUM | RULE_TREND_PULLBACK => format!(
            "{common_pipeline} Feature engineering: rolling mean over {fast} and {slow} day windows \
            on close prices (moving_average helper). Signal: MA crossover comparison — long when \
            fast MA > slow MA and price > fast MA (momentum) or price < fast MA and price > slow MA \
            (pullback). No external data dependencies beyond the asset's OHLCV parquet."
        ),
        RULE_RSI_REVERSION => {
            let rsi_window = arg_u32(args, "--rsi-window").unwrap_or(14);
            let rsi_oversold = arg_f64(args, "--rsi-oversold").unwrap_or(30.0);
            format!(
                "{common_pipeline} Feature engineering: rolling RSI({rsi_window}) computed from \
                close-to-close price changes (Wilder's smoothed average gain/loss). Signal: \
                binary long trigger when RSI < {rsi_oversold:.0}. Single data input: asset close prices."
            )
        }
        RULE_VOL_REGIME => format!(
            "{common_pipeline} Feature engineering: {vol_window}-day rolling standard deviation of \
            log returns, annualized via sqrt(365.25). Percentile rank computed across all valid \
            volatility observations. Signal: long when current vol <= vol_cap percentile threshold. \
            Single data input: asset close prices."
        ),
        RULE_VOL_SPREAD => format!(
            "{common_pipeline} Data ingestion (additional): load_vix_ohlcv() reads VIX from \
            asset_class=volatility/symbol=VIX parquet — no HTTP download. Date alignment via \
            HashMap<NaiveDate, f64> lookup, NaN fill for missing VIX dates. Feature engineering: \
            realized_vol = std(log_returns, {vol_window}) × sqrt(252); implied_vol = VIX_close / 100. \
            Spread = (implied - realized) / max(realized, 0.01). Signal: long when spread > threshold \
            (VRP harvest) or spread < negative threshold (snap-back). Two data inputs: asset OHLCV + \
            VIX OHLCV, both from local warehouse parquet."
        ),
        RULE_MEAN_REVERSION => {
            let mr_threshold = arg_f64(args, "--mr-entry-threshold").unwrap_or(0.02);
            format!(
                "{common_pipeline} Feature engineering: rolling SMA over {slow} day window on close prices \
                (moving_average helper) to compute fair value. Signal: relative mispricing \
                δ = (close − SMA) / SMA; long trigger when δ < −{:.3}. Single data input: asset close prices.",
                mr_threshold
            )
        }
        RULE_VVIX_REGIME => {
            let vvix_window = arg_u32(args, "--vvix-window").unwrap_or(63);
            format!(
                "{common_pipeline} Data ingestion (additional): load_volatility_index_ohlcv(\"VVIX\") reads \
                VVIX from asset_class=volatility/symbol=VVIX parquet. Date alignment via HashMap<NaiveDate, f64> \
                lookup, NaN fill for missing dates. Feature engineering: rolling percentile rank of VVIX close \
                over {vvix_window}-day window. Signal: long when VVIX percentile rank is below threshold \
                (risk_off mode) or above threshold (contrarian mode). Two data inputs: asset OHLCV + VVIX OHLCV, \
                both from local warehouse parquet."
            )
        }
        _ => format!(
            "{common_pipeline} Feature engineering and signal generation follow the rule-specific \
            logic defined in paper_research.rs::build_signal_mask()."
        ),
    }
}

/// Step 1 of RESEARCH_ANALYSIS_FRAMEWORK: Mental Model Synthesis.
///
/// Summarizes the core thesis and theoretical mechanism behind the strategy,
/// grounded in the specific research paper that seeded the hypothesis.
fn research_basis(
    rule: &str,
    asset: &str,
    rationale: &str,
    source: &str,
    args: &[String],
) -> String {
    // Extract the paper title from "ArXiv-seeded hypothesis (N): <title>"
    let paper_title = rationale
        .find(": ")
        .map(|i| &rationale[i + 2..])
        .unwrap_or(rationale)
        .trim();

    let is_grid = source == "deterministic-grid" || source == "refinement";

    if is_grid {
        let rule_label = match rule {
            RULE_TREND_MOMENTUM => "trend-following momentum",
            RULE_TREND_PULLBACK => "pullback-within-uptrend",
            RULE_RSI_REVERSION => "RSI mean-reversion",
            RULE_VOL_REGIME => "volatility-regime filtering",
            RULE_VOL_SPREAD => "implied-vs-realized vol spread (VRP harvest)",
            RULE_MEAN_REVERSION => "mean-reversion fair-value filter",
            RULE_VVIX_REGIME => "VVIX vol-of-vol regime filter",
            _ => "adaptive signal",
        };
        return format!(
            "This candidate was generated from a systematic parameter grid search rather than a specific \
            research paper. It applies a {rule_label} approach to {asset}, exploring parameter combinations \
            drawn from empirically-grounded ranges used across quantitative equity research. While not tied \
            to a single academic hypothesis, the underlying signal logic is well-documented in the \
            market microstructure and technical analysis literature."
        );
    }

    let fast = arg_u32(args, "--fast-window").unwrap_or(12);
    let slow = arg_u32(args, "--slow-window").unwrap_or(50);
    let rsi_window = arg_u32(args, "--rsi-window").unwrap_or(14);
    let rsi_oversold = arg_u32(args, "--rsi-oversold").unwrap_or(30);
    let vol_window = arg_u32(args, "--vol-window").unwrap_or(20);
    let vol_cap = arg_f64(args, "--vol-cap").unwrap_or(0.40);

    match rule {
        RULE_TREND_MOMENTUM => format!(
            "Seeded from the paper \"{paper_title}\", this candidate treats the source as a research lead rather \
            than a literal blueprint. The implemented hypothesis tests directional price persistence on {asset} \
            with a concrete zonk rule, not a direct replication of the paper's methodology. It operationalizes \
            using a dual moving-average crossover ({fast}-day fast / {slow}-day slow), entering long \
            positions only when both the short-term trend and the broader price direction confirm upward \
            momentum. The source link shows the paper that inspired this hypothesis; the rule described \
            here is the exact strategy variant currently under test."
        ),
        RULE_TREND_PULLBACK => format!(
            "Seeded from the paper \"{paper_title}\", this candidate uses the source as a prompt for a \
            pullback-within-trend hypothesis on {asset}, not as a direct implementation of the paper. \
            The tested rule uses a {fast}-day / {slow}-day moving-average \
            framework to detect moments when price retreats below the short-term average while the \
            longer-term trend remains intact, entering positions at a statistical discount to the \
            prevailing trend. The source link captures the research seed; the rule described here is the \
            actual zonk translation being evaluated."
        ),
        RULE_RSI_REVERSION => format!(
            "Seeded from the paper \"{paper_title}\", this candidate maps the source into an RSI \
            mean-reversion hypothesis on {asset}. It is not a direct replication of the paper's model \
            or claims; the paper serves as the research lead, while the rule below is the concrete \
            strategy under test. The candidate triggers entries \
            when RSI({rsi_window}) drops below {rsi_oversold}, targeting the reflexive bounce that \
            typically follows exhaustive selling pressure. In leveraged instruments like {asset}, these \
            oversold conditions can be amplified by daily rebalancing effects, which is why the report \
            highlights the implemented signal separately from the source paper."
        ),
        RULE_VOL_REGIME => format!(
            "Seeded from the paper \"{paper_title}\", this candidate uses the source as a research cue for \
            a volatility-regime filter on {asset}. The report is describing the exact rule under test, not \
            claiming that the paper itself used this exact specification. The candidate \
            applies a {vol_window}-day realized volatility filter to {asset}, restricting exposure \
            to periods when volatility remains below the {:.0}th percentile (vol_cap = {vol_cap:.2}). \
            The source link captures the paper that inspired the hypothesis, while the narrative here \
            explains the concrete zonk implementation.",
            vol_cap * 100.0
        ),
        RULE_VOL_SPREAD => format!(
            "Seeded from the paper \"{paper_title}\", this candidate turns the source into an \
            implied-vs-realized volatility spread hypothesis on {asset}. The paper is the inspiration \
            source, while the rule below is the specific zonk translation being backtested. \
            When VIX overstates subsequent realized vol (positive spread), the strategy enters long positions \
            to harvest the premium. In the negative-threshold variant, it targets snap-back opportunities \
            when realized volatility overshoots implied, signaling an imminent return to calmer conditions. \
            The vol_cap threshold of {vol_cap:.2} controls the minimum spread magnitude required to trigger entry."
        ),
        RULE_MEAN_REVERSION => {
            let mr_threshold = arg_f64(args, "--mr-entry-threshold").unwrap_or(0.02);
            format!(
                "Seeded from the paper \"{paper_title}\", this candidate implements a mean reversion \
                filter hypothesis on {asset} inspired by Xu et al. (2026, SSRN 6225198). The paper proposes \
                regime-aware neural signal filters (CNN, WaveNet, DKF, S4) to extract fair value from noisy \
                price data and trade the spot-filter spread. This zonk translation uses a {slow}-day SMA as \
                the fair value proxy, entering long when relative mispricing δ = (spot − SMA) / SMA drops \
                below −{:.1}%. The SMA is a simplified stand-in for the paper's LAFO-trained neural filter; \
                the trading logic (threshold entry on the spread) follows the paper's Section 5 formulation.",
                mr_threshold * 100.0
            )
        }
        RULE_VVIX_REGIME => {
            let vvix_window = arg_u32(args, "--vvix-window").unwrap_or(63);
            let vvix_threshold = arg_f64(args, "--vvix-threshold").unwrap_or(0.75);
            let mode = arg_str(args, "--vvix-mode").unwrap_or("risk_off".to_string());
            format!(
                "Seeded from the paper \"{paper_title}\", this candidate implements a VVIX regime \
                filter on {asset} inspired by Bevilacqua & Hizmeri (2026, SSRN 6212458). The paper \
                documents that morning VVIX strongly predicts next-day variance asset returns (Sharpe \
                2.0-3.2 for VIX futures). This zonk adaptation uses the VVIX percentile rank over \
                {vvix_window} days as a regime filter: in {mode} mode, it goes long when VVIX percentile \
                is {} the {:.0}th threshold. The paper's alpha is in variance assets; this equity \
                adaptation captures the information content of VVIX as a regime signal.",
                if mode == "contrarian" { "above" } else { "below" },
                vvix_threshold * 100.0
            )
        }
        _ => format!(
            "This candidate draws on the research paper \"{paper_title}\" to construct an adaptive \
            trading signal for {asset}. The paper's findings on market dynamics and price behavior \
            are translated into a systematic, rules-based strategy designed to capture the specific \
            edge identified in the research while managing downside risk through disciplined \
            position sizing and signal-driven entry/exit logic."
        ),
    }
}

fn has_arg(args: &[String], flag: &str) -> bool {
    args.iter().any(|v| v == flag)
}

fn format_command_line(cmd: &Path, args: &[String]) -> String {
    let mut parts = vec![cmd.display().to_string()];
    for arg in args {
        if arg.contains(' ') {
            parts.push(format!("\"{arg}\""));
        } else {
            parts.push(arg.clone());
        }
    }
    parts.join(" ")
}

fn normalize_text(value: &str) -> String {
    let collapsed = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    format!(" {collapsed} ")
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn looks_like_subject_bucket(title: &str) -> bool {
    title.contains(" > ")
}

fn looks_like_generic_seed_title(title: &str) -> bool {
    let normalized = collapse_whitespace(title).to_ascii_lowercase();
    looks_like_subject_bucket(title)
        || (normalized.starts_with("submitted paper ")
            && normalized["submitted paper ".len()..]
                .chars()
                .all(|ch| ch.is_ascii_digit()))
}

fn looks_like_arxiv_identifier(value: &str) -> bool {
    let token = value
        .trim()
        .strip_prefix("arXiv:")
        .unwrap_or(value.trim())
        .split_whitespace()
        .next()
        .unwrap_or("");

    if token.is_empty() {
        return false;
    }

    let core = token
        .rsplit_once('v')
        .filter(|(_, suffix)| suffix.chars().all(|c| c.is_ascii_digit()))
        .map(|(prefix, _)| prefix)
        .unwrap_or(token);

    if let Some((lhs, rhs)) = core.split_once('.') {
        return lhs.len() == 4
            && lhs.chars().all(|c| c.is_ascii_digit())
            && (4..=5).contains(&rhs.len())
            && rhs.chars().all(|c| c.is_ascii_digit());
    }

    if let Some((lhs, rhs)) = core.split_once('/') {
        return !lhs.is_empty()
            && lhs
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
            && rhs.len() >= 7
            && rhs.chars().all(|c| c.is_ascii_digit());
    }

    false
}

fn extract_title_from_seed_text(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("# Title:") {
            let title = collapse_whitespace(rest);
            if !title.is_empty() && !looks_like_generic_seed_title(&title) {
                return Some(title);
            }
        }
        if let Some(rest) = line.strip_prefix("Title:") {
            let title = collapse_whitespace(rest);
            if !title.is_empty() && !looks_like_generic_seed_title(&title) {
                return Some(title);
            }
        }
    }

    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            if let Some((id, title)) = line.split_once("] ") {
                let id = id.trim_start_matches('[').trim();
                if !looks_like_arxiv_identifier(id) {
                    continue;
                }
                let title = collapse_whitespace(title);
                if !title.is_empty() && !looks_like_generic_seed_title(&title) {
                    return Some(title);
                }
            }
        }
    }

    let lines: Vec<String> = text
        .lines()
        .map(|line| collapse_whitespace(line.trim()))
        .collect();
    for (idx, title) in lines.iter().enumerate() {
        if title.is_empty()
            || title.len() < 12
            || title.starts_with('#')
            || title.starts_with("http")
            || title.starts_with("arXiv:")
            || title.starts_with("Authors:")
            || title.eq_ignore_ascii_case("abstract")
            || title.contains('@')
            || looks_like_generic_seed_title(title)
        {
            continue;
        }

        if let Some(next_line) = lines.get(idx + 1) {
            let continuation = !next_line.is_empty()
                && next_line.len() <= 40
                && !next_line.starts_with('#')
                && !next_line.starts_with("Authors:")
                && !next_line.eq_ignore_ascii_case("abstract")
                && !next_line.contains('@')
                && next_line
                    .chars()
                    .all(|ch| ch.is_ascii_alphabetic() || ch.is_ascii_whitespace() || ch == '-');
            if continuation {
                let combined = collapse_whitespace(&format!("{title} {next_line}"));
                let alpha_count = combined.chars().filter(|c| c.is_ascii_alphabetic()).count();
                if alpha_count >= 6 {
                    return Some(combined);
                }
            }
        }

        let alpha_count = title.chars().filter(|c| c.is_ascii_alphabetic()).count();
        if alpha_count >= 6 {
            return Some(title.clone());
        }
    }

    None
}

fn seed_paper_title(seed: &ExaSeed) -> String {
    let title = collapse_whitespace(&seed.title);
    if !title.is_empty() && !looks_like_generic_seed_title(&title) {
        return title;
    }

    if let Some(parsed) = extract_title_from_seed_text(&seed.text) {
        return parsed;
    }

    if !title.is_empty() {
        return title;
    }

    "Untitled arXiv seed".to_string()
}

fn extract_abstract_from_seed_text(text: &str) -> Option<String> {
    let normalized = text.replace("\r\n", "\n");
    let abstract_start = normalized
        .find("> Abstract:")
        .map(|idx| idx + "> Abstract:".len())
        .or_else(|| {
            normalized
                .find("Abstract:")
                .map(|idx| idx + "Abstract:".len())
        })?;

    let mut abstract_text = normalized[abstract_start..].trim();
    for marker in [
        "\n## ",
        "\n# ",
        "\n| Subjects:",
        "\nSubmission history",
        "\n## Submission history",
        "\nFull-text links:",
        "\nCurrent browse context:",
        "\n### ",
        "\nReferences & Citations",
        "\nBibliographic",
        "\nCode, Data, Media",
        "\nRelated Papers",
    ] {
        if let Some(idx) = abstract_text.find(marker) {
            abstract_text = &abstract_text[..idx];
        }
    }

    let abstract_text = collapse_whitespace(abstract_text.trim_start_matches('>').trim());
    if abstract_text.is_empty() {
        return None;
    }

    Some(abstract_text)
}

fn seed_classification_blob(seed: &ExaSeed) -> String {
    let title = seed_paper_title(seed);
    let abstract_or_prefix = extract_abstract_from_seed_text(&seed.text).unwrap_or_else(|| {
        collapse_whitespace(&seed.text)
            .chars()
            .take(SEED_CLASSIFICATION_FALLBACK_CHARS)
            .collect::<String>()
    });
    normalize_text(&format!("{title} {abstract_or_prefix}"))
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles
        .iter()
        .any(|needle| contains_phrase(haystack, needle))
}

fn contains_phrase(haystack: &str, needle: &str) -> bool {
    haystack.contains(&normalize_text(needle))
}

fn extract_seed_tags(seed: &ExaSeed) -> HashSet<&'static str> {
    let blob = seed_classification_blob(seed);
    let mut tags = HashSet::new();

    if contains_any(
        &blob,
        &[
            "momentum",
            "trend",
            "trend following",
            "trend-following",
            "breakout",
            "moving average",
            "moving-average",
            "relative strength",
        ],
    ) {
        tags.insert("momentum");
    }
    if contains_any(
        &blob,
        &[
            "pullback",
            "buy the dip",
            "dip buying",
            "dip-buying",
            "retracement",
        ],
    ) {
        tags.insert("pullback");
    }
    if contains_any(
        &blob,
        &[
            "volatility regime",
            "regime switching",
            "markov switching",
            "volatility clustering",
            "garch",
            " arch ",
        ],
    ) {
        tags.insert("regime");
    }
    if contains_any(
        &blob,
        &[
            "reversion",
            "mean reversion",
            "oversold",
            "rsi",
            "oscillator",
            "mean-reverting",
            "overbought",
        ],
    ) {
        tags.insert("reversion");
    }
    if contains_any(
        &blob,
        &[
            "vix",
            "implied volatility",
            "realized volatility",
            "volatility spread",
            "vrp",
            "variance risk premium",
            "garch",
            "implied vs realized",
        ],
    ) {
        tags.insert("vol_spread");
    }
    tags
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SeedRuleFamily {
    ExistingRule(&'static str),
    NeedsNewRule(&'static str),
    NoMatch,
}

fn keyword_score(blob: &str, phrases: &[(&str, i32)]) -> i32 {
    phrases
        .iter()
        .filter(|(phrase, _)| contains_phrase(blob, phrase))
        .map(|(_, weight)| *weight)
        .sum()
}

fn classify_seed_rule_family(seed: &ExaSeed) -> SeedRuleFamily {
    let blob = seed_classification_blob(seed);
    let tags = extract_seed_tags(seed);

    let supported_scores = [
        (
            RULE_TREND_MOMENTUM,
            keyword_score(
                &blob,
                &[
                    ("momentum", 5),
                    ("trend following", 5),
                    ("trend-following", 5),
                    ("time series momentum", 6),
                    ("moving average", 4),
                    ("moving-average", 4),
                    ("breakout", 4),
                    ("relative strength", 3),
                ],
            ),
        ),
        (
            RULE_TREND_PULLBACK,
            keyword_score(
                &blob,
                &[
                    ("pullback", 6),
                    ("buy the dip", 6),
                    ("dip buying", 6),
                    ("dip-buying", 6),
                    ("retracement", 3),
                ],
            ),
        ),
        (
            RULE_RSI_REVERSION,
            keyword_score(
                &blob,
                &[
                    ("rsi", 7),
                    ("mean reversion", 6),
                    ("mean-reversion", 6),
                    ("oversold", 5),
                    ("overbought", 5),
                    ("oscillator", 3),
                    ("contrarian", 2),
                ],
            ),
        ),
        (
            RULE_VOL_REGIME,
            keyword_score(
                &blob,
                &[
                    ("volatility regime", 7),
                    ("regime switching", 7),
                    ("markov switching", 6),
                    ("volatility clustering", 6),
                    ("volatility forecasting", 6),
                    ("forecasting vix", 7),
                    ("vix forecasting", 7),
                    ("volatility index", 4),
                    ("leverage effect", 3),
                    ("garch", 4),
                    (" arch ", 4),
                    ("low volatility regime", 4),
                    ("high volatility regime", 4),
                ],
            ),
        ),
        (
            RULE_VOL_SPREAD,
            keyword_score(
                &blob,
                &[
                    ("variance risk premium", 7),
                    ("vrp", 7),
                    ("implied vs realized", 6),
                    ("volatility spread", 5),
                    ("implied volatility", 3),
                    ("realized volatility", 3),
                    ("vix futures", 5),
                    ("vix options", 5),
                    ("vix", 3),
                    ("term structure", 3),
                    ("contango", 4),
                    ("backwardation", 4),
                ],
            ),
        ),
        (
            RULE_MEAN_REVERSION,
            keyword_score(
                &blob,
                &[
                    ("mean reversion filter", 8),
                    ("signal filtering", 6),
                    ("fair value", 5),
                    ("mispricing", 6),
                    ("filtered mean reversion", 7),
                    ("sma reversion", 5),
                    ("ema reversion", 5),
                    ("price deviation", 4),
                    ("equilibrium price", 4),
                    ("low-pass filter", 5),
                    ("lafo", 7),
                    ("kalman filter trading", 5),
                    ("spot filter spread", 6),
                ],
            ),
        ),
        (
            RULE_VVIX_REGIME,
            keyword_score(
                &blob,
                &[
                    ("vvix", 8),
                    ("volatility of volatility", 7),
                    ("vol of vol", 7),
                    ("volatility uncertainty", 6),
                    ("limited attention", 5),
                    ("slow moving beliefs", 5),
                    ("morning volatility", 6),
                    ("variance risk premium vvix", 8),
                    ("vix options", 4),
                ],
            ),
        ),
    ];

    let best_supported = supported_scores
        .into_iter()
        .max_by_key(|(_, score)| *score)
        .unwrap_or((RULE_TREND_MOMENTUM, 0));
    let best_non_rsi_supported = supported_scores
        .into_iter()
        .filter(|(rule, _)| *rule != RULE_RSI_REVERSION)
        .max_by_key(|(_, score)| *score)
        .unwrap_or((RULE_TREND_MOMENTUM, 0));

    let unsupported_scores = [
        (
            "options_hedging",
            keyword_score(
                &blob,
                &[
                    ("deep hedging", 9),
                    ("hedging", 4),
                    ("delta hedging", 6),
                    ("gamma hedging", 6),
                    ("option portfolio", 6),
                    ("no-trade region", 5),
                    ("no trade region", 5),
                    ("transaction costs", 2),
                ],
            ),
        ),
        (
            "reinforcement_learning",
            keyword_score(
                &blob,
                &[
                    ("reinforcement learning", 8),
                    ("deep reinforcement learning", 8),
                    ("actor-critic", 6),
                    ("policy gradient", 5),
                    ("q-learning", 5),
                ],
            ),
        ),
        (
            "portfolio_construction",
            keyword_score(
                &blob,
                &[
                    ("portfolio management", 6),
                    ("portfolio optimization", 6),
                    ("portfolio overlay", 5),
                    ("risk parity", 6),
                    ("asset allocation", 5),
                ],
            ),
        ),
        (
            "option_pricing",
            keyword_score(
                &blob,
                &[
                    ("option pricing", 9),
                    ("pricing kernel", 8),
                    ("volatility risk aversion", 8),
                    ("risk-neutral", 6),
                    ("heston", 6),
                    ("volatility surface", 6),
                    ("option prices", 4),
                    ("pricing errors", 4),
                    ("calibration", 4),
                ],
            ),
        ),
        (
            "execution_microstructure",
            keyword_score(
                &blob,
                &[
                    ("order book", 6),
                    ("market making", 6),
                    ("execution", 5),
                    ("market microstructure", 5),
                    ("liquidity provider", 4),
                ],
            ),
        ),
        (
            "intraday_alpha",
            keyword_score(
                &blob,
                &[
                    ("intraday", 6),
                    ("open-to-close", 5),
                    ("close-to-open", 5),
                    ("high frequency", 6),
                    ("hourly", 4),
                    ("minute", 4),
                ],
            ),
        ),
    ];

    let best_unsupported = unsupported_scores
        .into_iter()
        .max_by_key(|(_, score)| *score)
        .unwrap_or(("none", 0));

    let best_supported_score = best_supported.1
        + if tags.contains("momentum") && best_supported.0 == RULE_TREND_MOMENTUM {
            2
        } else if tags.contains("pullback") && best_supported.0 == RULE_TREND_PULLBACK {
            2
        } else if tags.contains("reversion") && best_supported.0 == RULE_RSI_REVERSION {
            2
        } else if tags.contains("regime") && best_supported.0 == RULE_VOL_REGIME {
            2
        } else if tags.contains("vol_spread") && best_supported.0 == RULE_VOL_SPREAD {
            2
        } else {
            0
        };
    let explicit_rsi_signal = contains_any(&blob, &["rsi", "oversold", "overbought", "oscillator"]);
    let volatility_context = contains_any(
        &blob,
        &[
            "vix",
            "volatility index",
            "volatility forecasting",
            "forecasting vix",
            "vix forecasting",
            "implied volatility",
            "realized volatility",
            "volatility risk premium",
            "volatility spread",
        ],
    );

    if best_supported.0 == RULE_RSI_REVERSION
        && !explicit_rsi_signal
        && volatility_context
        && best_non_rsi_supported.1 >= 3
    {
        return SeedRuleFamily::ExistingRule(best_non_rsi_supported.0);
    }

    if best_supported_score == 0 {
        if best_unsupported.1 > 0 {
            return SeedRuleFamily::NeedsNewRule(best_unsupported.0);
        }
        return SeedRuleFamily::NoMatch;
    }

    if best_unsupported.1 > 0 && best_supported_score <= best_unsupported.1 {
        return SeedRuleFamily::NeedsNewRule(best_unsupported.0);
    }

    if best_supported_score < 5 {
        if best_unsupported.1 > 0 {
            return SeedRuleFamily::NeedsNewRule(best_unsupported.0);
        }
        return SeedRuleFamily::NoMatch;
    }

    SeedRuleFamily::ExistingRule(best_supported.0)
}

fn signature_for_candidate(candidate: &Candidate) -> String {
    format!(
        "{}|{}|{}|{}",
        candidate.strategy,
        candidate.rule,
        candidate.focus_asset,
        candidate.args.join("|")
    )
}

fn param_signature_from_args(args: &[String]) -> String {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let key = &args[i];
        if key == "--hypothesis-id" {
            i += 2; // skip flag and value
            continue;
        }
        if key.starts_with("--") {
            if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                let val = &args[i + 1];
                let normalized = if let Ok(f) = val.parse::<f64>() {
                    format!("{:.2}", f)
                } else {
                    val.clone()
                };
                pairs.push((key.clone(), normalized));
                i += 2;
            } else {
                pairs.push((key.clone(), String::new()));
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
        .iter()
        .map(|(k, v)| {
            if v.is_empty() {
                k.clone()
            } else {
                format!("{k}={v}")
            }
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn param_signature(candidate: &Candidate) -> String {
    param_signature_from_args(&candidate.args)
}

fn fetch_exa_ideas(queries: &[&str], limit: usize) -> Vec<ExaSeed> {
    let api_key = env::var("EXA_API_KEY").unwrap_or_default();
    if api_key.trim().is_empty() {
        return Vec::new();
    }

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .ok();
    let Some(client) = client else {
        return Vec::new();
    };

    let mut seen = HashSet::new();
    let mut ideas = Vec::new();

    for query in queries {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&api_key).unwrap_or_else(|_| HeaderValue::from_static("")),
        );

        let payload = json!({
            "query": query,
            "numResults": limit,
            "contents": {"text": true},
        });

        let response = client
            .post("https://api.exa.ai/search")
            .headers(headers)
            .json(&payload)
            .send()
            .and_then(|r| r.error_for_status());
        let Ok(response) = response else {
            continue;
        };

        let body = response.text().unwrap_or_default();
        let parsed: Result<ExaResponse, _> = serde_json::from_str(&body);
        if parsed.is_err() {
            continue;
        }
        let parsed = parsed.unwrap_or(ExaResponse {
            results: Vec::new(),
        });

        for row in parsed.results {
            if row.url.trim().is_empty() {
                continue;
            }
            let title = seed_paper_title(&row);
            if title.trim().is_empty() {
                continue;
            }
            let key = format!("{}||{}", title.to_lowercase(), row.url.to_lowercase());
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key);
            ideas.push(ExaSeed {
                title,
                url: row.url,
                text: row.text,
            });
        }
    }

    ideas
}

fn sample_or_default<T: Clone>(values: &[T], idx: usize, offset: usize) -> T {
    values[(idx + offset) % values.len()].clone()
}

fn seed_candidate(
    seed_idx: usize,
    seed: &ExaSeed,
    rule: &str,
    focus_asset: &str,
    fast: u32,
    slow: u32,
    rsi_window: Option<u32>,
    rsi_oversold: Option<u32>,
    rsi_overbought: Option<u32>,
    vol_window: Option<u32>,
    vol_cap: Option<f64>,
    variant: usize,
) -> Candidate {
    let mut args = vec![
        "--asset".to_string(),
        focus_asset.to_string(),
        "--rule".to_string(),
        rule.to_string(),
        "--fast-window".to_string(),
        fast.to_string(),
        "--slow-window".to_string(),
        slow.to_string(),
        "--rsi-window".to_string(),
        rsi_window.unwrap_or(14).to_string(),
        "--rsi-oversold".to_string(),
        rsi_oversold.unwrap_or(30).to_string(),
        "--rsi-overbought".to_string(),
        rsi_overbought.unwrap_or(70).to_string(),
        "--vol-window".to_string(),
        vol_window.unwrap_or(20).to_string(),
        "--vol-cap".to_string(),
        format!("{:.2}", vol_cap.unwrap_or(0.40)),
    ];
    if !has_arg(&args, "--hypothesis-id") {
        args.push("--hypothesis-id".to_string());
        args.push(format!("seed-{}-{}", seed_idx, variant));
    }

    Candidate {
        candidate_id: format!("seed-{seed_idx:03}-{rule}-v{variant}"),
        strategy: RESEARCH_STRATEGY.to_string(),
        rule: rule.to_string(),
        args,
        rationale: format!(
            "ArXiv-seeded hypothesis ({}): {}",
            seed_idx,
            seed_paper_title(seed)
        ),
        source: if seed.url.is_empty() {
            "seed".to_string()
        } else {
            seed.url.clone()
        },
        focus_asset: focus_asset.to_string(),
        is_seeded: true,
        _min_observations: 20,
        _min_signals: 10,
    }
}

fn build_seed_candidates(seed: &ExaSeed, idx: usize, assets: &[&str]) -> Vec<Candidate> {
    let mut out = Vec::new();

    let selected_rules = match classify_seed_rule_family(seed) {
        SeedRuleFamily::ExistingRule(rule) => vec![rule],
        SeedRuleFamily::NeedsNewRule(_) | SeedRuleFamily::NoMatch => {
            return out;
        }
    };

    for (offset, rule) in selected_rules.iter().enumerate() {
        for variant in 0..3 {
            let focus_asset = sample_or_default(&assets, idx, offset * 2 + variant).to_string();

            match *rule {
                RULE_TREND_MOMENTUM | RULE_TREND_PULLBACK => {
                    let fast = sample_or_default(&FAST_WINDOW_SET, idx, offset + variant) as u32;
                    let slow = sample_or_default(
                        &SLOW_WINDOW_SET,
                        idx.saturating_add(variant + 1),
                        offset + 1,
                    ) as u32;
                    if slow <= fast {
                        continue;
                    }
                    out.push(seed_candidate(
                        idx,
                        seed,
                        rule,
                        &focus_asset,
                        fast,
                        slow,
                        None,
                        None,
                        None,
                        None,
                        None,
                        variant,
                    ));
                }
                RULE_RSI_REVERSION => {
                    let fast = sample_or_default(&FAST_WINDOW_SET, idx, variant) as u32;
                    let slow = sample_or_default(&FAST_WINDOW_SET, idx + 3, variant + 2) as u32;
                    let rsi_window = sample_or_default(&RSI_WINDOW_SET, idx, variant);
                    let rsi_oversold = sample_or_default(&RSI_OVERSOLD_SET, idx, variant * 2);
                    let rsi_overbought = sample_or_default(&RSI_OVERBOUGHT_SET, idx, variant + 1);
                    out.push(seed_candidate(
                        idx,
                        seed,
                        rule,
                        &focus_asset,
                        fast.max(2),
                        slow.max(fast + 2),
                        Some(rsi_window),
                        Some(rsi_oversold),
                        Some(rsi_overbought),
                        None,
                        None,
                        variant,
                    ));
                }
                RULE_VOL_REGIME => {
                    let fast = sample_or_default(&FAST_WINDOW_SET, idx, variant);
                    let slow = sample_or_default(&SLOW_WINDOW_SET, idx + 1, variant * 2);
                    let vol_window = sample_or_default(&VOL_WINDOW_SET, idx, variant + 1);
                    let vol_cap = sample_or_default(&VOL_CAP_SET, idx, variant + 2);
                    out.push(seed_candidate(
                        idx,
                        seed,
                        rule,
                        &focus_asset,
                        fast,
                        slow,
                        None,
                        None,
                        None,
                        Some(vol_window),
                        Some(vol_cap),
                        variant,
                    ));
                }
                RULE_VOL_SPREAD => {
                    // Pin fast/slow to defaults — vol_spread doesn't use MA params
                    let vol_window = sample_or_default(&VOL_WINDOW_SET, idx, variant + 1);
                    let vol_cap = sample_or_default(&VOL_CAP_SPREAD_SET, idx, variant + 2);
                    out.push(seed_candidate(
                        idx,
                        seed,
                        rule,
                        &focus_asset,
                        12, // fixed default
                        50, // fixed default
                        None,
                        None,
                        None,
                        Some(vol_window),
                        Some(vol_cap),
                        variant,
                    ));
                }
                RULE_MEAN_REVERSION => {
                    // Fair value SMA window = slow; fast is unused
                    let slow = sample_or_default(&SLOW_WINDOW_SET, idx + 1, variant * 2);
                    let mr_threshold = sample_or_default(&MR_ENTRY_THRESHOLD_SET, idx, variant + 1);
                    let mr_scale = sample_or_default(&MR_SCALE_SET, idx, variant + 2);
                    let mut c = seed_candidate(
                        idx,
                        seed,
                        rule,
                        &focus_asset,
                        12, // fixed default — unused by this rule
                        slow,
                        None,
                        None,
                        None,
                        None,
                        None,
                        variant,
                    );
                    c.args.push("--mr-entry-threshold".to_string());
                    c.args.push(format!("{:.3}", mr_threshold));
                    c.args.push("--mr-scale".to_string());
                    c.args.push(format!("{:.1}", mr_scale));
                    out.push(c);
                }
                RULE_VVIX_REGIME => {
                    let vvix_window = sample_or_default(&VVIX_WINDOW_SET, idx, variant + 1);
                    let vvix_threshold = sample_or_default(&VVIX_THRESHOLD_SET, idx, variant + 2);
                    let mode = if variant % 2 == 0 { "risk_off" } else { "contrarian" };
                    let mut c = seed_candidate(
                        idx,
                        seed,
                        rule,
                        &focus_asset,
                        12, 50, // fixed defaults — unused
                        None, None, None, None, None,
                        variant,
                    );
                    c.args.push("--vvix-window".to_string());
                    c.args.push(vvix_window.to_string());
                    c.args.push("--vvix-threshold".to_string());
                    c.args.push(format!("{:.2}", vvix_threshold));
                    c.args.push("--vvix-mode".to_string());
                    c.args.push(mode.to_string());
                    out.push(c);
                }
                _ => {}
            }
        }
    }

    out
}

fn build_deterministic_grid_candidates(min_candidates: usize, assets: &[&str]) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut id = 0usize;

    for rule in [
        RULE_TREND_MOMENTUM,
        RULE_TREND_PULLBACK,
        RULE_RSI_REVERSION,
        RULE_VOL_REGIME,
    ] {
        for asset_idx in 0..assets.len() {
            for i in 0..FAST_WINDOW_SET.len() {
                for j in 0..SLOW_WINDOW_SET.len() {
                    let fast = FAST_WINDOW_SET[i];
                    let slow = SLOW_WINDOW_SET[j];
                    if (rule == RULE_TREND_MOMENTUM || rule == RULE_TREND_PULLBACK) && slow <= fast
                    {
                        continue;
                    }
                    if out.len() >= min_candidates.saturating_mul(2) {
                        return out;
                    }
                    let candidate = match rule {
                        RULE_TREND_MOMENTUM | RULE_TREND_PULLBACK => seed_candidate(
                            0,
                            &ExaSeed {
                                title: "deterministic grid".to_string(),
                                url: String::new(),
                                text: String::new(),
                            },
                            rule,
                            assets[asset_idx],
                            fast,
                            slow,
                            None,
                            None,
                            None,
                            None,
                            None,
                            id,
                        ),
                        RULE_RSI_REVERSION => {
                            let rsi_window = RSI_WINDOW_SET[(i + j) % RSI_WINDOW_SET.len()];
                            let rsi_oversold =
                                RSI_OVERSOLD_SET[(i + asset_idx) % RSI_OVERSOLD_SET.len()];
                            let rsi_overbought =
                                RSI_OVERBOUGHT_SET[(j + asset_idx) % RSI_OVERBOUGHT_SET.len()];
                            let mut args = vec![
                                "--asset".to_string(),
                                assets[asset_idx].to_string(),
                                "--rule".to_string(),
                                rule.to_string(),
                                "--rsi-window".to_string(),
                                rsi_window.to_string(),
                                "--rsi-oversold".to_string(),
                                rsi_oversold.to_string(),
                                "--rsi-overbought".to_string(),
                                rsi_overbought.to_string(),
                                "--fast-window".to_string(),
                                fast.to_string(),
                                "--slow-window".to_string(),
                                slow.to_string(),
                            ];
                            args.extend(vec!["--hypothesis-id".to_string(), format!("grid-{id}")]);
                            Candidate {
                                candidate_id: format!("grid-rsi-{id}"),
                                strategy: RESEARCH_STRATEGY.to_string(),
                                rule: rule.to_string(),
                                args,
                                rationale: "Deterministic paper-research RSI reversion variant (grid fallback)".to_string(),
                                source: "deterministic-grid".to_string(),
                                focus_asset: assets[asset_idx].to_string(),
                                is_seeded: false,
                                _min_observations: 20,
                                _min_signals: 10,
                            }
                        }
                        RULE_VOL_REGIME => {
                            let vol_window = VOL_WINDOW_SET[(i + j) % VOL_WINDOW_SET.len()];
                            let vol_cap = VOL_CAP_SET[(i + asset_idx) % VOL_CAP_SET.len()];
                            let mut args = vec![
                                "--asset".to_string(),
                                assets[asset_idx].to_string(),
                                "--rule".to_string(),
                                rule.to_string(),
                                "--vol-window".to_string(),
                                vol_window.to_string(),
                                "--vol-cap".to_string(),
                                format!("{:.2}", vol_cap),
                                "--fast-window".to_string(),
                                fast.to_string(),
                                "--slow-window".to_string(),
                                slow.to_string(),
                            ];
                            args.extend(vec!["--hypothesis-id".to_string(), format!("grid-{id}")]);
                            Candidate {
                                candidate_id: format!("grid-vol-{id}"),
                                strategy: RESEARCH_STRATEGY.to_string(),
                                rule: rule.to_string(),
                                args,
                                rationale: "Deterministic paper-research volatility-regime variant (grid fallback)".to_string(),
                                source: "deterministic-grid".to_string(),
                                focus_asset: assets[asset_idx].to_string(),
                                is_seeded: false,
                                _min_observations: 20,
                                _min_signals: 10,
                            }
                        }
                        _ => unreachable!(),
                    };
                    id = id.saturating_add(1);
                    out.push(candidate);
                }
            }
        }
    }

    // Separate grid loop for vol_spread: VOL_WINDOW_SET × VOL_CAP_SPREAD_SET × assets
    // Bypasses FAST × SLOW outer loop to avoid wasting candidates on irrelevant MA dimension
    for asset_idx in 0..assets.len() {
        for vi in 0..VOL_WINDOW_SET.len() {
            for ci in 0..VOL_CAP_SPREAD_SET.len() {
                if out.len() >= min_candidates.saturating_mul(2) {
                    return out;
                }
                let vol_window = VOL_WINDOW_SET[vi];
                let vol_cap = VOL_CAP_SPREAD_SET[ci];
                let mut args = vec![
                    "--asset".to_string(),
                    assets[asset_idx].to_string(),
                    "--rule".to_string(),
                    RULE_VOL_SPREAD.to_string(),
                    "--vol-window".to_string(),
                    vol_window.to_string(),
                    "--vol-cap".to_string(),
                    format!("{:.2}", vol_cap),
                    "--fast-window".to_string(),
                    "12".to_string(),
                    "--slow-window".to_string(),
                    "50".to_string(),
                ];
                args.extend(vec!["--hypothesis-id".to_string(), format!("grid-{id}")]);
                out.push(Candidate {
                    candidate_id: format!("grid-vspread-{id}"),
                    strategy: RESEARCH_STRATEGY.to_string(),
                    rule: RULE_VOL_SPREAD.to_string(),
                    args,
                    rationale: "Deterministic paper-research vol-spread variant (grid fallback)"
                        .to_string(),
                    source: "deterministic-grid".to_string(),
                    focus_asset: assets[asset_idx].to_string(),
                    is_seeded: false,
                    _min_observations: 20,
                    _min_signals: 10,
                });
                id = id.saturating_add(1);
            }
        }
    }

    // Separate grid loop for mean_reversion_filter: SLOW_WINDOW_SET × MR_ENTRY_THRESHOLD_SET × assets
    for asset_idx in 0..assets.len() {
        for si in 0..SLOW_WINDOW_SET.len() {
            for ti in 0..MR_ENTRY_THRESHOLD_SET.len() {
                if out.len() >= min_candidates.saturating_mul(2) {
                    return out;
                }
                let slow = SLOW_WINDOW_SET[si];
                let mr_threshold = MR_ENTRY_THRESHOLD_SET[ti];
                let mut args = vec![
                    "--asset".to_string(),
                    assets[asset_idx].to_string(),
                    "--rule".to_string(),
                    RULE_MEAN_REVERSION.to_string(),
                    "--slow-window".to_string(),
                    slow.to_string(),
                    "--fast-window".to_string(),
                    "12".to_string(),
                    "--mr-entry-threshold".to_string(),
                    format!("{:.3}", mr_threshold),
                    "--mr-scale".to_string(),
                    "200.0".to_string(),
                ];
                args.extend(vec!["--hypothesis-id".to_string(), format!("grid-{id}")]);
                out.push(Candidate {
                    candidate_id: format!("grid-mr-{id}"),
                    strategy: RESEARCH_STRATEGY.to_string(),
                    rule: RULE_MEAN_REVERSION.to_string(),
                    args,
                    rationale: "Deterministic paper-research mean-reversion-filter variant (grid fallback)"
                        .to_string(),
                    source: "deterministic-grid".to_string(),
                    focus_asset: assets[asset_idx].to_string(),
                    is_seeded: false,
                    _min_observations: 20,
                    _min_signals: 10,
                });
                id = id.saturating_add(1);
            }
        }
    }

    // Separate grid loop for vvix_regime: VVIX_WINDOW_SET × VVIX_THRESHOLD_SET × modes × assets
    for asset_idx in 0..assets.len() {
        for wi in 0..VVIX_WINDOW_SET.len() {
            for ti in 0..VVIX_THRESHOLD_SET.len() {
                for mode in &["risk_off", "contrarian"] {
                    if out.len() >= min_candidates.saturating_mul(2) {
                        return out;
                    }
                    let vvix_window = VVIX_WINDOW_SET[wi];
                    let vvix_threshold = VVIX_THRESHOLD_SET[ti];
                    let mut args = vec![
                        "--asset".to_string(),
                        assets[asset_idx].to_string(),
                        "--rule".to_string(),
                        RULE_VVIX_REGIME.to_string(),
                        "--fast-window".to_string(),
                        "12".to_string(),
                        "--slow-window".to_string(),
                        "50".to_string(),
                        "--vvix-window".to_string(),
                        vvix_window.to_string(),
                        "--vvix-threshold".to_string(),
                        format!("{:.2}", vvix_threshold),
                        "--vvix-mode".to_string(),
                        mode.to_string(),
                    ];
                    args.extend(vec!["--hypothesis-id".to_string(), format!("grid-{id}")]);
                    out.push(Candidate {
                        candidate_id: format!("grid-vvix-{id}"),
                        strategy: RESEARCH_STRATEGY.to_string(),
                        rule: RULE_VVIX_REGIME.to_string(),
                        args,
                        rationale: "Deterministic paper-research vvix-regime variant (grid fallback)"
                            .to_string(),
                        source: "deterministic-grid".to_string(),
                        focus_asset: assets[asset_idx].to_string(),
                        is_seeded: false,
                        _min_observations: 20,
                        _min_signals: 10,
                    });
                    id = id.saturating_add(1);
                }
            }
        }
    }

    out
}

fn build_candidate_pool(
    seed_ideas: &[ExaSeed],
    seed_web: bool,
    include_grid: bool,
    min_candidates: usize,
    assets: &[&str],
) -> Vec<Candidate> {
    let mut pool = Vec::new();
    let mut seen = HashSet::new();

    let push = |candidate: Candidate, pool: &mut Vec<Candidate>, seen: &mut HashSet<String>| {
        let sig = signature_for_candidate(&candidate);
        if seen.insert(sig) {
            pool.push(candidate);
        }
    };

    if seed_web && !seed_ideas.is_empty() {
        for (idx, seed) in seed_ideas.iter().enumerate() {
            let candidates = build_seed_candidates(seed, idx + 1, assets);
            for candidate in candidates {
                push(candidate, &mut pool, &mut seen);
            }
        }
    }

    if include_grid || pool.len() < min_candidates {
        let target = min_candidates.saturating_sub(pool.len());
        for mut candidate in build_deterministic_grid_candidates(target.max(1), assets) {
            candidate.is_seeded = false;
            candidate.source = "deterministic-grid".to_string();
            push(candidate, &mut pool, &mut seen);
        }
    }

    if pool.is_empty() {
        return build_deterministic_grid_candidates(
            min_candidates.max(MIN_CANDIDATES_TARGET_DEFAULT),
            assets,
        );
    }

    pool
}

fn estimate_sessions(start_date: &str, end_date: &str) -> Option<i64> {
    let start = NaiveDate::parse_from_str(start_date, "%Y-%m-%d").ok()?;
    let end = NaiveDate::parse_from_str(end_date, "%Y-%m-%d").ok()?;
    if end < start {
        return None;
    }

    let mut day = start;
    let mut count = 0i64;
    while day <= end {
        if day.weekday().num_days_from_monday() < 5 {
            count += 1;
        }
        day += Duration::days(1);
    }
    Some(count)
}

fn sessions_for_window(start_date: &str, end_date: &str, fallback: i64) -> i64 {
    estimate_sessions(start_date, end_date)
        .unwrap_or(fallback)
        .max(1)
}

fn score_paper_research(payload: &Value) -> Option<ScoredRun> {
    let rows = payload.get("results")?.as_array()?;
    if rows.is_empty() {
        return None;
    }

    let row = rows
        .iter()
        .find(|row| {
            row.get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase()
                != "buy & hold"
        })
        .or_else(|| rows.first())?;

    let cagr = row.get("cagr").and_then(safe_float)?;
    let sharpe = row.get("sharpe").and_then(safe_float)?;
    let dd = row.get("max_drawdown").and_then(safe_float)?;
    let var95 = row.get("var_95").and_then(safe_float).unwrap_or(0.0);
    let beginning_equity = row
        .get("beginning_equity")
        .and_then(safe_float)
        .unwrap_or(DEFAULT_PAPER_RESEARCH_BEGINNING_EQUITY);
    let final_equity = row.get("final_equity").and_then(safe_float).unwrap_or(0.0);

    let score = 6.0 * cagr + 3.0 * sharpe - 1.4 * dd.abs() - 0.5 * var95.max(0.0);
    Some(ScoredRun {
        score,
        details: json!({
            "name": row.get("name"),
            "beginning_equity": beginning_equity,
            "final_equity": final_equity,
            "cagr": cagr,
            "sharpe": sharpe,
            "max_drawdown": dd,
            "var_95": var95,
            "period_start": payload.get("period_start").cloned().unwrap_or(Value::Null),
            "period_end": payload.get("period_end").cloned().unwrap_or(Value::Null),
            "capital": payload.get("capital").cloned().unwrap_or(json!(DEFAULT_PAPER_RESEARCH_BEGINNING_EQUITY)),
        }),
    })
}

fn score_payload(payload: &Value) -> Option<ScoredRun> {
    score_paper_research(payload)
}

fn strategy_category(strategy: &str) -> &'static str {
    if strategy == RESEARCH_STRATEGY {
        "Research"
    } else {
        "Other"
    }
}

fn run_strategy_payload(
    strategy: &str,
    strategy_args: &[String],
    zonk_bin: &Path,
    start_date: &str,
    end_date: &str,
    sessions: i64,
    stage: &str,
    include_audit: bool,
    verbose: bool,
) -> Option<Value> {
    let mut args = vec![
        "--output".to_string(),
        "json".to_string(),
        "run".to_string(),
        strategy.to_string(),
    ];
    args.extend(strategy_args.iter().cloned());
    if strategy == RESEARCH_STRATEGY {
        args.push("--start-date".to_string());
        args.push(start_date.to_string());
    }
    args.push("--end-date".to_string());
    args.push(end_date.to_string());
    args.push("--sessions".to_string());
    args.push(sessions.to_string());
    if include_audit && strategy == RESEARCH_STRATEGY {
        args.push("--include-audit".to_string());
    }

    if verbose {
        println!("  [{stage}] {}", format_command_line(zonk_bin, &args));
    }

    let mut cmd = Command::new(zonk_bin);
    cmd.args(&args);
    let output = cmd.output().ok()?;
    if !output.status.success() {
        if verbose {
            eprintln!(
                "  zonk failed (code={:?})",
                output.status.code().unwrap_or(-1)
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.is_empty() {
                eprintln!("  stderr: {}", stderr.trim());
            }
        }
        return None;
    }

    let payload = match serde_json::from_slice(&output.stdout) {
        Ok(payload) => payload,
        Err(err) => {
            if verbose {
                eprintln!("  failed parsing zonk output: {err}");
                let stdout = String::from_utf8_lossy(&output.stdout);
                eprintln!("  stdout: {}", stdout.trim());
            }
            return None;
        }
    };

    Some(payload)
}

fn run_candidate(
    candidate: &Candidate,
    zonk_bin: &Path,
    start_date: &str,
    end_date: &str,
    sessions: i64,
    stage: &str,
    verbose: bool,
) -> Option<ScoredRun> {
    let payload = run_strategy_payload(
        &candidate.strategy,
        &candidate.args,
        zonk_bin,
        start_date,
        end_date,
        sessions,
        stage,
        false,
        verbose,
    )?;
    score_payload(&payload)
}

fn format_detail_summary(details: &Value) -> String {
    let cagr = safe_float(details.get("cagr").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let sharpe = safe_float(details.get("sharpe").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let dd = safe_float(details.get("max_drawdown").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let var95 = safe_float(details.get("var_95").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let beginning_equity =
        safe_float(details.get("beginning_equity").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let equity = safe_float(details.get("final_equity").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let period_start = details
        .get("period_start")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let period_end = details
        .get("period_end")
        .and_then(Value::as_str)
        .unwrap_or("?");
    format!(
        "name={} period={}..{} beginning_equity={:.2} cagr={:.3} sharpe={:.3} dd={:.2}% var95={:.3} final_equity={:.2}",
        details.get("name").and_then(Value::as_str).unwrap_or("?"),
        period_start,
        period_end,
        beginning_equity,
        cagr,
        sharpe,
        dd,
        var95,
        equity
    )
}

fn profitability_blurb(
    train: &Value,
    test: &Value,
    strategy: &str,
    rule: &str,
    args: &[String],
    asset: &str,
) -> String {
    let train_cagr = safe_float(train.get("cagr").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let test_cagr = safe_float(test.get("cagr").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let train_sharpe = safe_float(train.get("sharpe").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let test_sharpe = safe_float(test.get("sharpe").unwrap_or(&Value::Null)).unwrap_or(0.0);
    let train_dd = safe_float(train.get("max_drawdown").unwrap_or(&Value::Null))
        .unwrap_or(0.0)
        .abs();
    let test_dd = safe_float(test.get("max_drawdown").unwrap_or(&Value::Null))
        .unwrap_or(0.0)
        .abs();
    let signal = rule_description(rule, args, asset);
    let mechanics = format!(
        "Why it works: {} This strategy is run through `{}` in `zonk`.",
        signal, strategy
    );

    if train_cagr > 0.05 && test_cagr > 0.02 && train_sharpe > 0.2 && test_sharpe > 0.1 {
        format!(
            "{} Train: CAGR {}, Sharpe {}, max drawdown {}. Test: CAGR {}, Sharpe {}, max drawdown {}. Strongly consistent equity growth across both windows.",
            mechanics,
            fmt_pct(train_cagr),
            fmt_num(train_sharpe),
            fmt_pct(train_dd),
            fmt_pct(test_cagr),
            fmt_num(test_sharpe),
            fmt_pct(test_dd),
        )
    } else if train_sharpe > 0.0 && test_sharpe > 0.0 {
        format!(
            "{} Train: CAGR {}, Sharpe {} (drawdown {}). Test: CAGR {}, Sharpe {} (drawdown {}). Risk-adjusted edge is present in both windows but less stable than ideal.",
            mechanics,
            fmt_pct(train_cagr),
            fmt_num(train_sharpe),
            fmt_pct(train_dd),
            fmt_pct(test_cagr),
            fmt_num(test_sharpe),
            fmt_pct(test_dd),
        )
    } else if test_cagr > 0.0 && test_sharpe > 0.0 {
        format!(
            "{} Train: CAGR {}, Sharpe {} with {} max drawdown. Test: CAGR {}, Sharpe {} with {} max drawdown. The signal appears to adapt better in forward periods; monitor regime dependence.",
            mechanics,
            fmt_pct(train_cagr),
            fmt_num(train_sharpe),
            fmt_pct(train_dd),
            fmt_pct(test_cagr),
            fmt_num(test_sharpe),
            fmt_pct(test_dd),
        )
    } else {
        format!(
            "{} Train: CAGR {}, Sharpe {} ({} max drawdown). Test: CAGR {}, Sharpe {} ({} max drawdown). This candidate is not yet clearly robust and should be treated as exploratory.",
            strategy,
            fmt_pct(train_cagr),
            fmt_num(train_sharpe),
            fmt_pct(train_dd),
            fmt_pct(test_cagr),
            fmt_num(test_sharpe),
            fmt_pct(test_dd)
        )
    }
}

fn evaluate_candidate(
    candidate: &Candidate,
    idx: usize,
    total: usize,
    zonk_bin: &Path,
    train_start: &str,
    train_end: &str,
    test_start: &str,
    test_end: &str,
    train_sessions: i64,
    test_sessions: i64,
    verbose: bool,
) -> Option<CandidateReport> {
    if verbose {
        println!(
            "\n{} candidate {}/{}: {}",
            strategy_category(&candidate.strategy),
            idx,
            total,
            candidate.candidate_id
        );
        println!(
            "  strategy: {} | rule: {}",
            candidate.strategy, candidate.rule
        );
        println!(
            "  asset: {} | source: {}",
            candidate.focus_asset, candidate.source
        );
        println!("  rationale: {}", candidate.rationale);
        println!("  args: {}", candidate.args.join(" "));
        println!(
            "  train window: {} -> {} (sessions={})",
            train_start, train_end, train_sessions
        );
    }
    let train_window_sessions = sessions_for_window(train_start, train_end, train_sessions);
    let test_window_sessions = sessions_for_window(test_start, test_end, test_sessions);

    let train_run = run_candidate(
        candidate,
        zonk_bin,
        train_start,
        train_end,
        train_window_sessions,
        "train",
        verbose,
    )?;
    if verbose {
        println!("  train score: {:.4}", train_run.score);
        println!(
            "  train summary: {}",
            format_detail_summary(&train_run.details)
        );
        println!(
            "  test window: {} -> {} (sessions={})",
            test_start, test_end, test_window_sessions
        );
    }

    let test_run = run_candidate(
        candidate,
        zonk_bin,
        test_start,
        test_end,
        test_window_sessions,
        "test",
        verbose,
    )?;
    if verbose {
        println!("  test score: {:.4}", test_run.score);
        println!(
            "  test summary: {}",
            format_detail_summary(&test_run.details)
        );
    }

    let c_train = train_run
        .details
        .get("cagr")
        .and_then(safe_float)
        .unwrap_or(0.0);
    let s_train = train_run
        .details
        .get("sharpe")
        .and_then(safe_float)
        .unwrap_or(0.0);
    let s_test = test_run
        .details
        .get("sharpe")
        .and_then(safe_float)
        .unwrap_or(0.0);
    if c_train <= 0.0 {
        return None;
    }
    if s_train < -3.0 || s_test < -3.0 {
        return None;
    }
    if train_run
        .details
        .get("final_equity")
        .and_then(safe_float)
        .unwrap_or(0.0)
        < 100_000.0
    {
        return None;
    }

    let combined_score = 0.65 * train_run.score + 0.35 * test_run.score;
    Some(CandidateReport {
        candidate_id: candidate.candidate_id.clone(),
        strategy: candidate.strategy.clone(),
        category: strategy_category(&candidate.strategy).to_string(),
        args: candidate.args.clone(),
        rule: candidate.rule.clone(),
        rationale: candidate.rationale.clone(),
        source: candidate.source.clone(),
        focus_asset: candidate.focus_asset.clone(),
        train_score: train_run.score,
        test_score: test_run.score,
        combined_score,
        train_details: train_run.details,
        test_details: test_run.details,
        train_audit: None,
        test_audit: None,
        is_seeded: candidate.is_seeded,
    })
}

fn shuffle_candidates(items: &mut [Candidate], seed: u64) {
    if items.len() < 2 {
        return;
    }

    let mut state = seed.wrapping_add(0x9E3779B97F4A7C15);
    for i in (1..items.len()).rev() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let j = (state % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

/// Reproducible Fisher-Yates subset sampling.
///
/// Returns up to `n` items from `pool` in a deterministic order based on `seed`.
/// If pool.len() <= n, returns the entire pool (shuffled).
fn deterministic_sample(pool: &[String], n: usize, seed: u64) -> Vec<String> {
    if pool.is_empty() {
        return Vec::new();
    }
    let mut items = pool.to_vec();
    let mut state = seed.wrapping_add(0x9E3779B97F4A7C15);
    for i in (1..items.len()).rev() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        let j = (state % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
    items.truncate(n);
    items
}

fn rank_rows(rows: &[CandidateReport]) -> Vec<CandidateReport> {
    let mut ranked = rows.to_vec();
    ranked.sort_by(|a, b| {
        let seeded = b.is_seeded.cmp(&a.is_seeded);
        if seeded != std::cmp::Ordering::Equal {
            return seeded;
        }
        let score_cmp = b
            .combined_score
            .partial_cmp(&a.combined_score)
            .unwrap_or(std::cmp::Ordering::Equal);
        if score_cmp != std::cmp::Ordering::Equal {
            return score_cmp;
        }
        b.train_score
            .partial_cmp(&a.train_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ranked
}

fn sanitize_filename_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    sanitized.trim_matches('-').to_string()
}

fn read_audit_window_meta(artifact_path: &str, payload: &Value) -> Option<AuditWindowMeta> {
    let audit = payload.get("audit")?;
    let trade_count = audit.get("executed_trade_count")?.as_u64()? as usize;
    let actual_period_start = audit.get("actual_period_start")?.as_str()?.to_string();
    let actual_period_end = audit.get("actual_period_end")?.as_str()?.to_string();
    Some(AuditWindowMeta {
        artifact_path: artifact_path.to_string(),
        trade_count,
        actual_period_start,
        actual_period_end,
    })
}

fn persist_report_audit(
    report: &CandidateReport,
    zonk_bin: &Path,
    reports_dir: &Path,
    start_date: &str,
    end_date: &str,
    sessions: i64,
    window_label: &str,
    verbose: bool,
) -> io::Result<Option<AuditWindowMeta>> {
    let audits_dir = reports_dir.join("autoresearch-audits");
    std::fs::create_dir_all(&audits_dir)?;

    let file_name = format!(
        "{}-{}-{}-to-{}.json",
        sanitize_filename_component(&report.candidate_id),
        window_label,
        sanitize_filename_component(start_date),
        sanitize_filename_component(end_date)
    );
    let absolute_path = audits_dir.join(file_name);
    let relative_path = format!(
        "autoresearch-audits/{}",
        absolute_path.file_name().unwrap().to_string_lossy()
    );

    let Some(payload) = run_strategy_payload(
        &report.strategy,
        &report.args,
        zonk_bin,
        start_date,
        end_date,
        sessions,
        &format!("{window_label}-audit"),
        true,
        verbose,
    ) else {
        return Ok(None);
    };

    let pretty = serde_json::to_string_pretty(&payload)
        .unwrap_or_else(|_| serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string()));
    std::fs::write(&absolute_path, pretty)?;
    Ok(read_audit_window_meta(&relative_path, &payload))
}

#[derive(Serialize)]
struct InteractiveRow {
    rank: usize,
    candidate_id: String,
    strategy: String,
    category: String,
    source: String,
    rule: String,
    focus_asset: String,
    args: Vec<String>,
    combined_score: f64,
    train_score: f64,
    test_score: f64,
    train_details: Value,
    test_details: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    train_audit: Option<AuditWindowMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    test_audit: Option<AuditWindowMeta>,
    why: String,
    is_seeded: bool,
    rationale: String,
    strategy_description: String,
    critical_evaluation: String,
    investment_rationale: String,
    backtest_architecture: String,
}

#[derive(Serialize)]
struct InteractiveReportMeta {
    generated_date: String,
    total_ranked: usize,
    round_count: usize,
    train_start: String,
    train_end: String,
    test_start: String,
    test_end: String,
    framework_tag: String,
}

fn build_interactive_report_html(rows: &[CandidateReport], meta: &InteractiveReportMeta) -> String {
    let top_rows: Vec<InteractiveRow> = rows
        .iter()
        .enumerate()
        .map(|(idx, row)| InteractiveRow {
            rank: idx + 1,
            candidate_id: row.candidate_id.clone(),
            strategy: row.strategy.clone(),
            category: row.category.clone(),
            source: row.source.clone(),
            rule: row.rule.clone(),
            focus_asset: row.focus_asset.clone(),
            args: row.args.clone(),
            combined_score: row.combined_score,
            train_score: row.train_score,
            test_score: row.test_score,
            train_details: row.train_details.clone(),
            test_details: row.test_details.clone(),
            train_audit: row.train_audit.clone(),
            test_audit: row.test_audit.clone(),
            why: profitability_blurb(
                &row.train_details,
                &row.test_details,
                &row.strategy,
                &row.rule,
                &row.args,
                &row.focus_asset,
            ),
            is_seeded: row.is_seeded,
            rationale: research_basis(
                &row.rule,
                &row.focus_asset,
                &row.rationale,
                &row.source,
                &row.args,
            ),
            strategy_description: rule_description(&row.rule, &row.args, &row.focus_asset),
            critical_evaluation: critical_evaluation(&row.rule, &row.focus_asset, &row.args),
            investment_rationale: investment_case(&row.rule, &row.focus_asset),
            backtest_architecture: backtest_architecture(&row.rule, &row.focus_asset, &row.args),
        })
        .collect();

    let rows_json = serde_json::to_string_pretty(&top_rows).unwrap_or_else(|_| "[]".to_string());
    let meta_json = serde_json::to_string_pretty(meta).unwrap_or_else(|_| "{}".to_string());
    INTERACTIVE_REPORT_TEMPLATE
        .replace("/* PASTE_META_HERE */", &meta_json)
        .replace("/* PASTE_ROWS_HERE */", &rows_json)
}

fn save_interactive_report(
    path: &Path,
    rows: &[CandidateReport],
    meta: &InteractiveReportMeta,
) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let html = build_interactive_report_html(rows, meta);
    std::fs::write(path, html)
}

fn save_ledger(path: &Path, rows: &[RecordedResult]) -> io::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = File::options().append(true).create(true).open(path)?;
    let ts = Utc::now().to_rfc3339();
    for row in rows {
        let mut record = json!({
            "timestamp": ts,
            "candidate_id": row.report.candidate_id,
            "strategy": row.report.strategy,
            "category": row.report.category,
            "rule": row.report.rule,
            "args": row.report.args,
            "source": row.report.source,
            "focus_asset": row.report.focus_asset,
            "train_score": row.report.train_score,
            "test_score": row.report.test_score,
            "combined_score": row.report.combined_score,
            "is_seeded": row.report.is_seeded,
            "train_details": row.report.train_details,
            "test_details": row.report.test_details,
            "train_audit": row.report.train_audit.clone(),
            "test_audit": row.report.test_audit.clone(),
        });
        if let Some(round) = row.round {
            record["round"] = json!(round);
        }
        writeln!(
            file,
            "{}",
            serde_json::to_string(&record).unwrap_or_else(|_| "{}".to_string())
        )?;
    }
    Ok(())
}

fn push_unique(items: &mut Vec<String>, value: &str) {
    if !items.iter().any(|item| item == value) {
        items.push(value.to_string());
    }
}

fn registry_snapshot(row: &RecordedResult, observed_at: &str) -> StrategyRegistrySnapshot {
    StrategyRegistrySnapshot {
        observed_at: observed_at.to_string(),
        candidate_id: row.report.candidate_id.clone(),
        source: row.report.source.clone(),
        rationale: row.report.rationale.clone(),
        round: row.round,
        combined_score: row.report.combined_score,
        train_score: row.report.train_score,
        test_score: row.report.test_score,
        train_details: row.report.train_details.clone(),
        test_details: row.report.test_details.clone(),
        train_audit: row.report.train_audit.clone(),
        test_audit: row.report.test_audit.clone(),
    }
}

fn load_strategy_registry(path: &Path) -> StrategyRegistry {
    let Ok(content) = std::fs::read_to_string(path) else {
        return StrategyRegistry::default();
    };
    serde_json::from_str(&content).unwrap_or_default()
}

fn save_strategy_registry(path: &Path, rows: &[RecordedResult]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut registry = load_strategy_registry(path);
    let now = Utc::now().to_rfc3339();

    for row in rows {
        let snapshot = registry_snapshot(row, &now);
        if let Some(existing) = registry
            .entries
            .iter_mut()
            .find(|entry| entry.signature == row.signature)
        {
            existing.last_top10_at = now.clone();
            existing.times_in_top10 += 1;
            existing.is_seeded = existing.is_seeded || row.report.is_seeded;
            existing.latest = snapshot.clone();
            push_unique(&mut existing.candidate_ids, &row.report.candidate_id);
            push_unique(&mut existing.sources, &row.report.source);
            if snapshot.combined_score > existing.best.combined_score {
                existing.best = snapshot;
            }
            continue;
        }

        registry.entries.push(StrategyRegistryEntry {
            signature: row.signature.clone(),
            registry_status: "research_candidate".to_string(),
            strategy: row.report.strategy.clone(),
            rule: row.report.rule.clone(),
            focus_asset: row.report.focus_asset.clone(),
            args: row.report.args.clone(),
            is_seeded: row.report.is_seeded,
            candidate_ids: vec![row.report.candidate_id.clone()],
            sources: vec![row.report.source.clone()],
            first_top10_at: now.clone(),
            last_top10_at: now.clone(),
            times_in_top10: 1,
            latest: snapshot.clone(),
            best: snapshot,
        });
    }

    registry.updated_at = now;
    registry.entries.sort_by(|a, b| {
        b.best
            .combined_score
            .partial_cmp(&a.best.combined_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.last_top10_at.cmp(&a.last_top10_at))
    });

    let text =
        serde_json::to_string_pretty(&registry).unwrap_or_else(|_| "{\"entries\":[]}".to_string());
    std::fs::write(path, text)
}

fn save_seed_ideas(path: &Path, items: &[ExaSeed]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let payload = json!({
        "generated_at": Utc::now().to_rfc3339(),
        "count": items.len(),
        "items": items,
    });
    let text = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
    std::fs::write(path, text)
}

fn print_top(rows: &[CandidateReport], k: usize) {
    let rows = rank_rows(rows);
    println!("\nTop candidates:");
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        "rank", "score", "seeded", "category", "strategy", "rule", "source", "asset", "train",
        "test",
    ]);
    for (i, row) in rows.iter().take(k).enumerate() {
        table.add_row(vec![
            (i + 1).to_string(),
            format!("{:.3}", row.combined_score),
            if row.is_seeded {
                "seed".to_string()
            } else {
                "fallback".to_string()
            },
            row.category.clone(),
            row.strategy.clone(),
            row.rule.clone(),
            row.source.clone(),
            row.focus_asset.clone(),
            format!("{:.4}", row.train_score),
            format!("{:.4}", row.test_score),
        ]);
    }
    println!("{}", table);
}

fn print_best(best: &CandidateReport) {
    println!("\nBest candidate:");
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec!["field", "value"]);
    table.add_row(vec!["strategy".to_string(), best.strategy.clone()]);
    table.add_row(vec!["rule".to_string(), best.rule.clone()]);
    table.add_row(vec!["candidate_id".to_string(), best.candidate_id.clone()]);
    table.add_row(vec!["category".to_string(), best.category.clone()]);
    table.add_row(vec!["asset".to_string(), best.focus_asset.clone()]);
    table.add_row(vec![
        "score".to_string(),
        format!("{:.4}", best.combined_score),
    ]);
    table.add_row(vec![
        "train/test".to_string(),
        format!("{:.4} / {:.4}", best.train_score, best.test_score),
    ]);
    table.add_row(vec!["source".to_string(), best.source.clone()]);
    table.add_row(vec![
        "command".to_string(),
        format!(
            "zonk --output json run {} {}",
            best.strategy,
            best.args.join(" ")
        ),
    ]);
    table.add_row(vec!["rationale".to_string(), best.rationale.clone()]);
    table.add_row(vec![
        "train details".to_string(),
        format_detail_summary(&best.train_details),
    ]);
    table.add_row(vec![
        "test details".to_string(),
        format_detail_summary(&best.test_details),
    ]);
    println!("{}", table);
}

fn has_converged(summaries: &[RoundSummary], patience: u32, min_improvement: f64) -> bool {
    if summaries.len() <= 1 {
        return false;
    }
    let mut stale_count = 0u32;
    for i in 1..summaries.len() {
        let prev_best = summaries[i - 1].global_best;
        let curr_best = summaries[i].global_best;
        let rel_imp = match (prev_best, curr_best) {
            (Some(prev), Some(curr)) => (curr - prev) / prev.abs().max(1.0),
            _ => 0.0,
        };
        if rel_imp < min_improvement {
            stale_count += 1;
        } else {
            stale_count = 0;
        }
    }
    stale_count >= patience
}

/// Filter ranked results through hard quality gates.
///
/// Only retains candidates whose **test-window** Sharpe >= min_sharpe
/// and **test-window** max drawdown (absolute) <= max_drawdown.
fn apply_quality_gates(
    ranked: &[CandidateReport],
    min_sharpe: Option<f64>,
    max_drawdown: Option<f64>,
) -> Vec<CandidateReport> {
    ranked
        .iter()
        .filter(|r| {
            let test_sharpe = safe_float(r.test_details.get("sharpe").unwrap_or(&Value::Null))
                .unwrap_or(f64::NEG_INFINITY);
            let test_dd = safe_float(r.test_details.get("max_drawdown").unwrap_or(&Value::Null))
                .unwrap_or(f64::NEG_INFINITY);

            if let Some(ms) = min_sharpe {
                if test_sharpe < ms {
                    return false;
                }
            }
            if let Some(md) = max_drawdown {
                // max_drawdown is stored as a negative fraction (e.g. -0.15 = -15%)
                // user specifies --max-drawdown 20 meaning "reject if worse than -20%"
                if test_dd.abs() * 100.0 > md {
                    return false;
                }
            }
            true
        })
        .cloned()
        .collect()
}

/// Print a diagnostic summary when the refinement loop terminates.
fn print_convergence_summary(loop_state: &LoopState, stop_reason: &str) {
    println!("\n--- Convergence Summary ---");
    println!("Stop reason: {}", stop_reason);

    let total_evaluated = loop_state.all_evaluated.len();
    let total_passed = loop_state.recorded_results.len();
    let rounds = loop_state.round_summaries.len();
    println!(
        "Total evaluated: {} | Passed scoring: {} | Rounds: {}",
        total_evaluated, total_passed, rounds
    );

    if !loop_state.exhausted_centers.is_empty() {
        println!(
            "Exhausted refinement centers: {}",
            loop_state.exhausted_centers.len()
        );
    }

    // Rule distribution among passing candidates
    let mut rule_counts: HashMap<String, usize> = HashMap::new();
    let mut asset_counts: HashMap<String, usize> = HashMap::new();
    for r in &loop_state.recorded_results {
        *rule_counts.entry(r.report.rule.clone()).or_insert(0) += 1;
        *asset_counts
            .entry(r.report.focus_asset.clone())
            .or_insert(0) += 1;
    }

    let mut rules: Vec<_> = rule_counts.into_iter().collect();
    rules.sort_by(|a, b| b.1.cmp(&a.1));
    let rule_summary: Vec<String> = rules.iter().map(|(r, c)| format!("{r}({c})")).collect();
    println!("Rules explored: {}", rule_summary.join(", "));

    let mut assets: Vec<_> = asset_counts.into_iter().collect();
    assets.sort_by(|a, b| b.1.cmp(&a.1));
    let top_assets: Vec<String> = assets
        .iter()
        .take(10)
        .map(|(a, c)| format!("{a}({c})"))
        .collect();
    println!("Top assets tested: {}", top_assets.join(", "));

    // Best score trajectory
    if rounds >= 2 {
        let first_best = loop_state
            .round_summaries
            .first()
            .and_then(|s| s.global_best);
        let last_best = loop_state
            .round_summaries
            .last()
            .and_then(|s| s.global_best);
        if let (Some(first), Some(last)) = (first_best, last_best) {
            let total_improvement = if first.abs() > 1e-10 {
                (last - first) / first.abs() * 100.0
            } else {
                0.0
            };
            println!(
                "Score trajectory: {:.3} -> {:.3} ({:+.1}% total)",
                first, last, total_improvement
            );
        }
    }
    println!("---");
}

fn adjacent_values_u32(set: &[u32], current: u32) -> Vec<u32> {
    let mut result = Vec::new();
    let pos = set.iter().position(|&v| v == current);
    match pos {
        Some(idx) => {
            if idx > 0 {
                result.push(set[idx - 1]);
            }
            if idx + 1 < set.len() {
                result.push(set[idx + 1]);
            }
        }
        None => {
            let mut below = None;
            let mut above = None;
            for &v in set {
                if v < current {
                    below = Some(v);
                } else if v > current && above.is_none() {
                    above = Some(v);
                }
            }
            if let Some(b) = below {
                result.push(b);
            }
            if let Some(a) = above {
                result.push(a);
            }
        }
    }
    result
}

fn adjacent_values_f64(set: &[f64], current: f64) -> Vec<f64> {
    let mut result = Vec::new();
    let pos = set.iter().position(|&v| (v - current).abs() < 1e-6);
    match pos {
        Some(idx) => {
            if idx > 0 {
                result.push(set[idx - 1]);
            }
            if idx + 1 < set.len() {
                result.push(set[idx + 1]);
            }
        }
        None => {
            let mut below = None;
            let mut above = None;
            for &v in set {
                if v < current - 1e-6 {
                    below = Some(v);
                } else if v > current + 1e-6 && above.is_none() {
                    above = Some(v);
                }
            }
            if let Some(b) = below {
                result.push(b);
            }
            if let Some(a) = above {
                result.push(a);
            }
        }
    }
    result
}

fn make_refined_candidate(
    rule: &str,
    asset: &str,
    fast: u32,
    slow: u32,
    rsi_window: u32,
    rsi_oversold: u32,
    rsi_overbought: u32,
    vol_window: u32,
    vol_cap: f64,
    round: u32,
    winner_idx: usize,
    variant_idx: usize,
) -> Candidate {
    let candidate_id = format!("refine-R{round}-W{winner_idx:02}-V{variant_idx:03}");
    let args = vec![
        "--asset".to_string(),
        asset.to_string(),
        "--rule".to_string(),
        rule.to_string(),
        "--fast-window".to_string(),
        fast.to_string(),
        "--slow-window".to_string(),
        slow.to_string(),
        "--rsi-window".to_string(),
        rsi_window.to_string(),
        "--rsi-oversold".to_string(),
        rsi_oversold.to_string(),
        "--rsi-overbought".to_string(),
        rsi_overbought.to_string(),
        "--vol-window".to_string(),
        vol_window.to_string(),
        "--vol-cap".to_string(),
        format!("{:.2}", vol_cap),
        "--hypothesis-id".to_string(),
        candidate_id.clone(),
    ];
    Candidate {
        candidate_id,
        strategy: RESEARCH_STRATEGY.to_string(),
        rule: rule.to_string(),
        args,
        rationale: format!("Refinement variant (round {round}, winner {winner_idx})"),
        source: "refinement".to_string(),
        focus_asset: asset.to_string(),
        is_seeded: false,
        _min_signals: 10,
        _min_observations: 20,
    }
}

fn refine_around_winner(
    winner: &CandidateReport,
    loop_state: &LoopState,
    round: u32,
    winner_idx: usize,
    max_variants: usize,
    max_asset_swaps: usize,
    viable_assets: &[String],
    seed: u64,
    verbose: bool,
) -> Vec<Candidate> {
    let args = &winner.args;
    let rule = &winner.rule;

    let fast = match arg_u32(args, "--fast-window") {
        Some(v) => v,
        None => {
            if verbose {
                println!(
                    "  [refine] skipping winner {}: missing --fast-window",
                    winner.candidate_id
                );
            }
            return Vec::new();
        }
    };
    let slow = match arg_u32(args, "--slow-window") {
        Some(v) => v,
        None => {
            if verbose {
                println!(
                    "  [refine] skipping winner {}: missing --slow-window",
                    winner.candidate_id
                );
            }
            return Vec::new();
        }
    };
    let rsi_window = arg_u32(args, "--rsi-window").unwrap_or(14);
    let rsi_oversold = arg_u32(args, "--rsi-oversold").unwrap_or(30);
    let rsi_overbought = arg_u32(args, "--rsi-overbought").unwrap_or(70);
    let vol_window = arg_u32(args, "--vol-window").unwrap_or(20);
    let vol_cap = arg_f64(args, "--vol-cap").unwrap_or(0.40);
    let asset = winner.focus_asset.as_str();

    let mut candidates = Vec::new();
    let mut variant_idx = 0usize;

    match rule.as_str() {
        RULE_TREND_MOMENTUM | RULE_TREND_PULLBACK => {
            for adj_fast in adjacent_values_u32(FAST_WINDOW_SET, fast) {
                if adj_fast < slow {
                    candidates.push(make_refined_candidate(
                        rule,
                        asset,
                        adj_fast,
                        slow,
                        rsi_window,
                        rsi_oversold,
                        rsi_overbought,
                        vol_window,
                        vol_cap,
                        round,
                        winner_idx,
                        variant_idx,
                    ));
                    variant_idx += 1;
                }
            }
            for adj_slow in adjacent_values_u32(SLOW_WINDOW_SET, slow) {
                if fast < adj_slow {
                    candidates.push(make_refined_candidate(
                        rule,
                        asset,
                        fast,
                        adj_slow,
                        rsi_window,
                        rsi_oversold,
                        rsi_overbought,
                        vol_window,
                        vol_cap,
                        round,
                        winner_idx,
                        variant_idx,
                    ));
                    variant_idx += 1;
                }
            }
        }
        RULE_RSI_REVERSION => {
            for adj in adjacent_values_u32(RSI_WINDOW_SET, rsi_window) {
                candidates.push(make_refined_candidate(
                    rule,
                    asset,
                    fast,
                    slow,
                    adj,
                    rsi_oversold,
                    rsi_overbought,
                    vol_window,
                    vol_cap,
                    round,
                    winner_idx,
                    variant_idx,
                ));
                variant_idx += 1;
            }
            for adj in adjacent_values_u32(RSI_OVERSOLD_SET, rsi_oversold) {
                if adj < rsi_overbought {
                    candidates.push(make_refined_candidate(
                        rule,
                        asset,
                        fast,
                        slow,
                        rsi_window,
                        adj,
                        rsi_overbought,
                        vol_window,
                        vol_cap,
                        round,
                        winner_idx,
                        variant_idx,
                    ));
                    variant_idx += 1;
                }
            }
            for adj in adjacent_values_u32(RSI_OVERBOUGHT_SET, rsi_overbought) {
                if rsi_oversold < adj {
                    candidates.push(make_refined_candidate(
                        rule,
                        asset,
                        fast,
                        slow,
                        rsi_window,
                        rsi_oversold,
                        adj,
                        vol_window,
                        vol_cap,
                        round,
                        winner_idx,
                        variant_idx,
                    ));
                    variant_idx += 1;
                }
            }
        }
        RULE_VOL_REGIME => {
            for adj in adjacent_values_u32(VOL_WINDOW_SET, vol_window) {
                candidates.push(make_refined_candidate(
                    rule,
                    asset,
                    fast,
                    slow,
                    rsi_window,
                    rsi_oversold,
                    rsi_overbought,
                    adj,
                    vol_cap,
                    round,
                    winner_idx,
                    variant_idx,
                ));
                variant_idx += 1;
            }
            for adj in adjacent_values_f64(VOL_CAP_SET, vol_cap) {
                candidates.push(make_refined_candidate(
                    rule,
                    asset,
                    fast,
                    slow,
                    rsi_window,
                    rsi_oversold,
                    rsi_overbought,
                    vol_window,
                    adj,
                    round,
                    winner_idx,
                    variant_idx,
                ));
                variant_idx += 1;
            }
        }
        RULE_VOL_SPREAD => {
            // Only perturb vol_window and vol_cap — fast/slow are irrelevant
            for adj in adjacent_values_u32(VOL_WINDOW_SET, vol_window) {
                candidates.push(make_refined_candidate(
                    rule,
                    asset,
                    fast,
                    slow,
                    rsi_window,
                    rsi_oversold,
                    rsi_overbought,
                    adj,
                    vol_cap,
                    round,
                    winner_idx,
                    variant_idx,
                ));
                variant_idx += 1;
            }
            for adj in adjacent_values_f64(VOL_CAP_SPREAD_SET, vol_cap) {
                candidates.push(make_refined_candidate(
                    rule,
                    asset,
                    fast,
                    slow,
                    rsi_window,
                    rsi_oversold,
                    rsi_overbought,
                    vol_window,
                    adj,
                    round,
                    winner_idx,
                    variant_idx,
                ));
                variant_idx += 1;
            }
        }
        RULE_MEAN_REVERSION => {
            let mr_threshold = arg_f64(args, "--mr-entry-threshold").unwrap_or(0.02);
            let mr_scale = arg_f64(args, "--mr-scale").unwrap_or(200.0);
            // Perturb slow window (SMA period)
            for adj_slow in adjacent_values_u32(SLOW_WINDOW_SET, slow) {
                let mut c = make_refined_candidate(
                    rule, asset, fast, adj_slow, rsi_window, rsi_oversold,
                    rsi_overbought, vol_window, vol_cap,
                    round, winner_idx, variant_idx,
                );
                c.args.push("--mr-entry-threshold".to_string());
                c.args.push(format!("{:.3}", mr_threshold));
                c.args.push("--mr-scale".to_string());
                c.args.push(format!("{:.1}", mr_scale));
                candidates.push(c);
                variant_idx += 1;
            }
            // Perturb entry threshold
            for adj in adjacent_values_f64(MR_ENTRY_THRESHOLD_SET, mr_threshold) {
                let mut c = make_refined_candidate(
                    rule, asset, fast, slow, rsi_window, rsi_oversold,
                    rsi_overbought, vol_window, vol_cap,
                    round, winner_idx, variant_idx,
                );
                c.args.push("--mr-entry-threshold".to_string());
                c.args.push(format!("{:.3}", adj));
                c.args.push("--mr-scale".to_string());
                c.args.push(format!("{:.1}", mr_scale));
                candidates.push(c);
                variant_idx += 1;
            }
        }
        RULE_VVIX_REGIME => {
            let vvix_window = arg_u32(args, "--vvix-window").unwrap_or(63);
            let vvix_threshold = arg_f64(args, "--vvix-threshold").unwrap_or(0.75);
            let vvix_mode = arg_str(args, "--vvix-mode").unwrap_or("risk_off".to_string());
            // Perturb VVIX window
            for adj in adjacent_values_u32(VVIX_WINDOW_SET, vvix_window) {
                let mut c = make_refined_candidate(
                    rule, asset, fast, slow, rsi_window, rsi_oversold,
                    rsi_overbought, vol_window, vol_cap,
                    round, winner_idx, variant_idx,
                );
                c.args.push("--vvix-window".to_string());
                c.args.push(adj.to_string());
                c.args.push("--vvix-threshold".to_string());
                c.args.push(format!("{:.2}", vvix_threshold));
                c.args.push("--vvix-mode".to_string());
                c.args.push(vvix_mode.clone());
                candidates.push(c);
                variant_idx += 1;
            }
            // Perturb threshold
            for adj in adjacent_values_f64(VVIX_THRESHOLD_SET, vvix_threshold) {
                let mut c = make_refined_candidate(
                    rule, asset, fast, slow, rsi_window, rsi_oversold,
                    rsi_overbought, vol_window, vol_cap,
                    round, winner_idx, variant_idx,
                );
                c.args.push("--vvix-window".to_string());
                c.args.push(vvix_window.to_string());
                c.args.push("--vvix-threshold".to_string());
                c.args.push(format!("{:.2}", adj));
                c.args.push("--vvix-mode".to_string());
                c.args.push(vvix_mode.clone());
                candidates.push(c);
                variant_idx += 1;
            }
        }
        _ => {
            if verbose {
                println!("  [refine] unsupported rule for refinement: {}", rule);
            }
            return Vec::new();
        }
    }

    // Asset swaps — sample from viable pool, excluding current asset
    let swap_pool: Vec<String> = viable_assets
        .iter()
        .filter(|a| a.as_str() != asset)
        .cloned()
        .collect();
    let swap_seed = seed
        .wrapping_mul(round as u64 + 1)
        .wrapping_add(winner_idx as u64 + 0xA5A5);
    let sampled_assets = deterministic_sample(&swap_pool, max_asset_swaps, swap_seed);
    for alt_asset in &sampled_assets {
        let mut c = make_refined_candidate(
            rule,
            alt_asset,
            fast,
            slow,
            rsi_window,
            rsi_oversold,
            rsi_overbought,
            vol_window,
            vol_cap,
            round,
            winner_idx,
            variant_idx,
        );
        // Carry forward rule-specific args for asset swaps
        if rule == RULE_MEAN_REVERSION {
            let mr_threshold = arg_f64(args, "--mr-entry-threshold").unwrap_or(0.02);
            let mr_scale = arg_f64(args, "--mr-scale").unwrap_or(200.0);
            c.args.push("--mr-entry-threshold".to_string());
            c.args.push(format!("{:.3}", mr_threshold));
            c.args.push("--mr-scale".to_string());
            c.args.push(format!("{:.1}", mr_scale));
        } else if rule == RULE_VVIX_REGIME {
            let vvix_window = arg_u32(args, "--vvix-window").unwrap_or(63);
            let vvix_threshold = arg_f64(args, "--vvix-threshold").unwrap_or(0.75);
            let vvix_mode = arg_str(args, "--vvix-mode").unwrap_or("risk_off".to_string());
            c.args.push("--vvix-window".to_string());
            c.args.push(vvix_window.to_string());
            c.args.push("--vvix-threshold".to_string());
            c.args.push(format!("{:.2}", vvix_threshold));
            c.args.push("--vvix-mode".to_string());
            c.args.push(vvix_mode);
        }
        candidates.push(c);
        variant_idx += 1;
    }

    // Local + global dedup
    let mut seen = HashSet::new();
    candidates.retain(|c| {
        let sig = param_signature(c);
        seen.insert(sig.clone()) && !loop_state.all_evaluated.contains(&sig)
    });

    // Deterministic shuffle based on seed + round + winner signature hash
    let sig_hash = param_signature_from_args(&winner.args)
        .bytes()
        .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
    let refinement_seed = seed.wrapping_mul(round as u64 + 1).wrapping_add(sig_hash);
    shuffle_candidates(&mut candidates, refinement_seed);

    candidates.truncate(max_variants);
    candidates
}

fn refine_around_winners(
    winners_ranked: &[CandidateReport],
    loop_state: &mut LoopState,
    refine_top: usize,
    refine_variants: usize,
    max_asset_swaps: usize,
    viable_assets: &[String],
    round: u32,
    seed: u64,
    verbose: bool,
) -> Vec<Candidate> {
    let mut all_variants = Vec::new();
    let mut eligible_count = 0usize;
    let mut newly_exhausted = Vec::new();

    for (idx, winner) in winners_ranked.iter().enumerate() {
        if eligible_count >= refine_top {
            break;
        }

        let sig = param_signature_from_args(&winner.args);
        if loop_state.is_center_exhausted(&sig) {
            if verbose {
                println!(
                    "  [refine] skipping exhausted center: {}",
                    winner.candidate_id
                );
            }
            continue;
        }

        let variants = refine_around_winner(
            winner,
            loop_state,
            round,
            idx,
            refine_variants,
            max_asset_swaps,
            viable_assets,
            seed,
            verbose,
        );

        if variants.is_empty() {
            newly_exhausted.push(sig);
            if verbose {
                println!(
                    "  [refine] winner {} exhausted (0 novel variants)",
                    winner.candidate_id
                );
            }
            continue;
        }

        if verbose {
            println!(
                "  [refine] winner {} produced {} variants",
                winner.candidate_id,
                variants.len()
            );
        }

        all_variants.extend(variants);
        eligible_count += 1;
    }

    for sig in newly_exhausted {
        loop_state.mark_center_exhausted(&sig);
    }

    // Cross-winner dedup
    let mut seen = HashSet::new();
    all_variants.retain(|c| {
        let sig = param_signature(c);
        seen.insert(sig)
    });

    all_variants
}

fn print_round_summary(summary: &RoundSummary) {
    let round_best_str = summary
        .round_best
        .map_or("N/A".to_string(), |v| format!("{:.2}", v));
    let global_best_str = summary
        .global_best
        .map_or("N/A".to_string(), |v| format!("{:.2}", v));
    let imp_str = if summary.rel_improvement.abs() < 1e-10 {
        "+0.0%".to_string()
    } else {
        format!("{:+.1}%", summary.rel_improvement * 100.0)
    };
    println!(
        "Round {}: generated={} evaluated={} passed={} round_best={} global_best={} {}",
        summary.round,
        summary.generated,
        summary.evaluated,
        summary.passed,
        round_best_str,
        global_best_str,
        imp_str,
    );
}

fn main() {
    let _ = dotenv();
    let args = Args::parse();
    let zonk_bin = PathBuf::from(&args.zonk_bin);
    if !zonk_bin.exists() {
        eprintln!("zonk binary not found: {}", zonk_bin.display());
        return;
    }

    let legacy_mode = args.no_loop || args.max_rounds == 1;

    // Build viable asset pool based on --asset-universe
    let train_sessions_est =
        estimate_sessions(&args.train_start, &args.train_end).unwrap_or(args.train_sessions);
    let test_sessions_est =
        estimate_sessions(&args.test_start, &args.test_end).unwrap_or(args.test_sessions);
    let min_rows = args
        .min_asset_rows
        .unwrap_or((train_sessions_est + test_sessions_est) as usize);

    let viable_assets: Vec<String> = match args.asset_universe.as_str() {
        "core" => RESEARCH_ASSETS.iter().map(|s| s.to_string()).collect(),
        "broad" => {
            let mut pool: HashSet<String> = RESEARCH_ASSETS.iter().map(|s| s.to_string()).collect();
            for preset_name in &["sp500", "ndx100"] {
                match zonk::data::presets::load_preset(preset_name) {
                    Ok((_, tickers)) => {
                        pool.extend(tickers);
                    }
                    Err(e) => {
                        eprintln!("Warning: could not load preset {preset_name}: {e}");
                    }
                }
            }
            let mut sorted: Vec<String> = pool.into_iter().collect();
            sorted.sort();
            sorted
        }
        "full" => match zonk::data::discovery::discover_viable_symbols(None, min_rows) {
            Ok(symbols) => symbols,
            Err(e) => {
                eprintln!("Failed to discover viable symbols: {e}");
                RESEARCH_ASSETS.iter().map(|s| s.to_string()).collect()
            }
        },
        preset_name => match zonk::data::presets::load_preset(preset_name) {
            Ok((_, tickers)) => {
                let mut pool: HashSet<String> = tickers.into_iter().collect();
                pool.extend(RESEARCH_ASSETS.iter().map(|s| s.to_string()));
                let mut sorted: Vec<String> = pool.into_iter().collect();
                sorted.sort();
                sorted
            }
            Err(e) => {
                eprintln!(
                    "Failed to load preset '{}': {e}. Falling back to core assets.",
                    preset_name
                );
                RESEARCH_ASSETS.iter().map(|s| s.to_string()).collect()
            }
        },
    };

    println!(
        "Autoresearch run: paper-research only | strategy seed web: {} | candidates up to {}",
        args.seed_web, args.candidates
    );
    println!(
        "Asset universe: {} ({} viable assets, min_rows={})",
        args.asset_universe,
        viable_assets.len(),
        min_rows
    );
    println!(
        "Train window: {} -> {} (fallback sessions {}), Test window: {} -> {} (fallback sessions {})",
        args.train_start,
        args.train_end,
        args.train_sessions,
        args.test_start,
        args.test_end,
        args.test_sessions
    );
    if !legacy_mode {
        println!(
            "Iterative refinement: max_rounds={} patience={} min_improvement={:.2} refine_top={} refine_variants={} asset_swaps={}",
            args.max_rounds,
            args.patience,
            args.min_improvement,
            args.refine_top,
            args.refine_variants,
            args.refine_asset_swaps
        );
    }
    if args.min_sharpe.is_some() || args.max_drawdown.is_some() {
        let mut gates = Vec::new();
        if let Some(ms) = args.min_sharpe {
            gates.push(format!("min_sharpe={ms:.2}"));
        }
        if let Some(md) = args.max_drawdown {
            gates.push(format!("max_drawdown={md:.1}%"));
        }
        println!("Quality gates: {}", gates.join(", "));
    }

    let seed_ideas = if args.seed_web {
        let ideas = fetch_exa_ideas(SEED_QUERIES, 25);
        if args.verbose {
            println!("fetched {} arXiv seeds from Exa", ideas.len());
        }
        if let Err(err) = save_seed_ideas(Path::new("reports/autoresearch-exa-ideas.json"), &ideas)
        {
            eprintln!("Failed to write exa seed file: {err}");
        }
        ideas
    } else {
        Vec::new()
    };

    let mut candidates = build_candidate_pool(
        &seed_ideas,
        args.seed_web,
        args.include_grid || !args.seed_web,
        args.candidates,
        RESEARCH_ASSETS,
    );

    if candidates.is_empty() {
        eprintln!("No candidates were generated; check data source and network key");
        return;
    }

    shuffle_candidates(&mut candidates, args.random_seed);
    if candidates.len() > args.candidates {
        candidates.truncate(args.candidates);
    }

    if args.verbose {
        println!("candidate pool: {} total", candidates.len());
        let seeded_count = candidates.iter().filter(|r| r.is_seeded).count();
        println!(
            "seeded: {} / fallback: {}",
            seeded_count,
            candidates.len() - seeded_count
        );
    }

    let train_window_sessions =
        sessions_for_window(&args.train_start, &args.train_end, args.train_sessions);
    let test_window_sessions =
        sessions_for_window(&args.test_start, &args.test_end, args.test_sessions);

    let mut loop_state = LoopState::new();

    // Load evaluation cache
    let cache_path = PathBuf::from(EVAL_CACHE_PATH);
    let eval_cache = if args.no_cache {
        if args.verbose {
            println!("evaluation cache disabled (--no-cache)");
        }
        HashMap::new()
    } else {
        let c = load_eval_cache(&cache_path);
        if args.verbose && !c.is_empty() {
            println!(
                "loaded {} cached evaluations from {}",
                c.len(),
                EVAL_CACHE_PATH
            );
        }
        c
    };

    // === Phase 1: Round 0 Exploration ===
    let novel_candidates = loop_state.retain_novel_candidates(candidates);
    let initial_count = novel_candidates.len();
    let mut round_reports = Vec::new();

    for (idx, candidate) in novel_candidates.iter().enumerate() {
        if let Some(report) = cached_evaluate_candidate(
            candidate,
            idx + 1,
            initial_count,
            &zonk_bin,
            &args.train_start,
            &args.train_end,
            &args.test_start,
            &args.test_end,
            train_window_sessions,
            test_window_sessions,
            args.verbose,
            &eval_cache,
            &cache_path,
        ) {
            round_reports.push(report);
        } else if args.verbose {
            println!("  rejected: scoring gate failed or zonk execution error");
        }
    }

    loop_state.record_round(0, initial_count, initial_count, round_reports, legacy_mode);
    if let Some(summary) = loop_state.round_summaries.last() {
        print_round_summary(summary);
    }

    // === Phase 2: Iterative Refinement ===
    if !legacy_mode {
        let mut stopped_early = false;
        for round in 1..args.max_rounds {
            if has_converged(
                &loop_state.round_summaries,
                args.patience,
                args.min_improvement,
            ) {
                print_convergence_summary(
                    &loop_state,
                    &format!(
                        "patience exhausted after {} rounds (patience={}, min_improvement={:.2})",
                        round, args.patience, args.min_improvement
                    ),
                );
                stopped_early = true;
                break;
            }

            let all_reports: Vec<CandidateReport> = loop_state
                .recorded_results
                .iter()
                .map(|r| r.report.clone())
                .collect();
            let ranked = rank_rows(&all_reports);

            let refined = refine_around_winners(
                &ranked,
                &mut loop_state,
                args.refine_top,
                args.refine_variants,
                args.refine_asset_swaps,
                &viable_assets,
                round,
                args.random_seed,
                args.verbose,
            );

            if refined.is_empty() {
                print_convergence_summary(
                    &loop_state,
                    &format!(
                        "refinement frontier exhausted after {} rounds (all top winners fully explored)",
                        round
                    ),
                );
                stopped_early = true;
                break;
            }

            if args.verbose {
                println!(
                    "Round {}: {} novel refinement candidates",
                    round,
                    refined.len()
                );
            }

            let novel = loop_state.retain_novel_candidates(refined);
            let generated = novel.len();
            let mut round_reports = Vec::new();

            for (idx, candidate) in novel.iter().enumerate() {
                if let Some(report) = cached_evaluate_candidate(
                    candidate,
                    idx + 1,
                    generated,
                    &zonk_bin,
                    &args.train_start,
                    &args.train_end,
                    &args.test_start,
                    &args.test_end,
                    train_window_sessions,
                    test_window_sessions,
                    args.verbose,
                    &eval_cache,
                    &cache_path,
                ) {
                    round_reports.push(report);
                } else if args.verbose {
                    println!("  rejected: scoring gate failed or zonk execution error");
                }
            }

            loop_state.record_round(round, generated, generated, round_reports, legacy_mode);
            if let Some(summary) = loop_state.round_summaries.last() {
                print_round_summary(summary);
            }
        }
        if !stopped_early {
            print_convergence_summary(
                &loop_state,
                &format!("max rounds reached ({})", args.max_rounds),
            );
        }
    }

    // === Phase 3: Final Reporting ===
    if args.verbose {
        println!(
            "completed: {} total results across {} rounds",
            loop_state.recorded_results.len(),
            loop_state.round_summaries.len()
        );
    }

    if loop_state.recorded_results.is_empty() {
        println!("No candidate passed scoring gates.");
        return;
    }

    let all_reports: Vec<CandidateReport> = loop_state
        .recorded_results
        .iter()
        .map(|r| r.report.clone())
        .collect();
    let ranked = rank_rows(&all_reports);

    // Apply hard quality gates if specified
    let has_gates = args.min_sharpe.is_some() || args.max_drawdown.is_some();
    let investable = if has_gates {
        let filtered = apply_quality_gates(&ranked, args.min_sharpe, args.max_drawdown);
        let mut gate_desc = Vec::new();
        if let Some(ms) = args.min_sharpe {
            gate_desc.push(format!("Sharpe >= {ms:.2}"));
        }
        if let Some(md) = args.max_drawdown {
            gate_desc.push(format!("drawdown <= {md:.1}%"));
        }
        println!(
            "\nQuality gates ({}): {}/{} candidates pass",
            gate_desc.join(", "),
            filtered.len(),
            ranked.len()
        );
        filtered
    } else {
        ranked.clone()
    };

    let top_k = args.top.min(investable.len());
    if top_k > 0 {
        print_top(&investable, top_k);
        if let Some(best) = investable.first() {
            print_best(best);
        }
    } else if has_gates {
        println!("No candidates passed quality gates.");
        // Still show top unfiltered results for reference
        let fallback_k = args.top.min(ranked.len());
        if fallback_k > 0 {
            println!("\nTop candidates (before quality gates):");
            print_top(&ranked, fallback_k);
        }
    }

    // Round history table
    if !legacy_mode && loop_state.round_summaries.len() > 1 {
        println!("\nRound history:");
        let mut table = Table::new();
        table.load_preset(UTF8_FULL);
        table.set_content_arrangement(ContentArrangement::Dynamic);
        table.set_header(vec![
            "Round",
            "Generated",
            "Evaluated",
            "Passed",
            "Round Best",
            "Global Best",
            "Improvement",
        ]);
        for s in &loop_state.round_summaries {
            let rb = s
                .round_best
                .map_or("N/A".to_string(), |v| format!("{:.3}", v));
            let gb = s
                .global_best
                .map_or("N/A".to_string(), |v| format!("{:.3}", v));
            let imp = if s.rel_improvement.abs() < 1e-10 {
                "+0.0%".to_string()
            } else {
                format!("{:+.1}%", s.rel_improvement * 100.0)
            };
            table.add_row(vec![
                s.round.to_string(),
                s.generated.to_string(),
                s.evaluated.to_string(),
                s.passed.to_string(),
                rb,
                gb,
                imp,
            ]);
        }
        println!("{}", table);
    }

    let report_path = Path::new("reports/autoresearch-top10-interactive-report.html");
    let reports_dir = report_path.parent().unwrap_or_else(|| Path::new("reports"));
    let train_window_sessions =
        sessions_for_window(&args.train_start, &args.train_end, train_sessions_est);
    let test_window_sessions =
        sessions_for_window(&args.test_start, &args.test_end, test_sessions_est);
    let mut top_entries: Vec<RecordedResult> = Vec::new();
    let mut seen_top_sigs = HashSet::new();
    for top_report in ranked.iter().take(10) {
        let signature = param_signature_from_args(&top_report.args);
        if !seen_top_sigs.insert(signature.clone()) {
            continue;
        }
        if let Some(recorded) = loop_state
            .recorded_results
            .iter()
            .find(|recorded| recorded.signature == signature)
        {
            top_entries.push(recorded.clone());
        }
    }

    for entry in &mut top_entries {
        match persist_report_audit(
            &entry.report,
            &zonk_bin,
            reports_dir,
            &args.train_start,
            &args.train_end,
            train_window_sessions,
            "train",
            args.verbose,
        ) {
            Ok(audit) => entry.report.train_audit = audit,
            Err(err) => eprintln!(
                "Failed to persist train audit for {}: {err}",
                entry.report.candidate_id
            ),
        }
        match persist_report_audit(
            &entry.report,
            &zonk_bin,
            reports_dir,
            &args.test_start,
            &args.test_end,
            test_window_sessions,
            "test",
            args.verbose,
        ) {
            Ok(audit) => entry.report.test_audit = audit,
            Err(err) => eprintln!(
                "Failed to persist test audit for {}: {err}",
                entry.report.candidate_id
            ),
        }
    }

    let top_reports: Vec<CandidateReport> = top_entries
        .iter()
        .map(|entry| entry.report.clone())
        .collect();
    let report_meta = InteractiveReportMeta {
        generated_date: Utc::now().to_rfc3339(),
        total_ranked: ranked.len(),
        round_count: loop_state.round_summaries.len(),
        train_start: args.train_start.clone(),
        train_end: args.train_end.clone(),
        test_start: args.test_start.clone(),
        test_end: args.test_end.clone(),
        framework_tag: "5-step Research Analysis Framework".to_string(),
    };
    if let Err(err) = save_interactive_report(report_path, &top_reports, &report_meta) {
        eprintln!("Failed to write interactive report: {err}");
    }

    if let Err(err) = save_ledger(Path::new("reports/autoresearch-ledger.jsonl"), &top_entries) {
        eprintln!("Failed to save ledger: {err}");
    }
    if let Err(err) = save_strategy_registry(Path::new(STRATEGY_REGISTRY_PATH), &top_entries) {
        eprintln!("Failed to save strategy registry: {err}");
    }

    let _ = Command::new("open")
        .arg("reports/autoresearch-top10-interactive-report.html")
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: returns RESEARCH_ASSETS as Vec<String> for test use.
    fn core_assets() -> Vec<String> {
        RESEARCH_ASSETS.iter().map(|s| s.to_string()).collect()
    }

    fn make_summary(round: u32, global_best: Option<f64>, rel_improvement: f64) -> RoundSummary {
        RoundSummary {
            round,
            generated: 10,
            evaluated: 10,
            passed: 5,
            round_best: global_best,
            global_best,
            rel_improvement,
        }
    }

    // === Convergence tests ===

    #[test]
    fn test_has_converged_false_before_patience() {
        let summaries = vec![
            make_summary(0, Some(1.0), 0.0),
            make_summary(1, Some(1.005), 0.005),
            make_summary(2, Some(1.008), 0.003),
        ];
        // patience=3, only 2 stale rounds (rounds 1 and 2 have < 0.02 improvement)
        assert!(!has_converged(&summaries, 3, 0.02));
    }

    #[test]
    fn test_has_converged_true_after_patience_small_gains() {
        let summaries = vec![
            make_summary(0, Some(1.0), 0.0),
            make_summary(1, Some(1.005), 0.005),
            make_summary(2, Some(1.008), 0.003),
            make_summary(3, Some(1.010), 0.002),
        ];
        // patience=3, all 3 rounds after round 0 have < 0.02 improvement
        assert!(has_converged(&summaries, 3, 0.02));
    }

    #[test]
    fn test_has_converged_false_on_meaningful_gain() {
        let summaries = vec![
            make_summary(0, Some(1.0), 0.0),
            make_summary(1, Some(1.005), 0.005),
            make_summary(2, Some(1.10), 0.095), // meaningful improvement resets counter
            make_summary(3, Some(1.105), 0.005),
        ];
        assert!(!has_converged(&summaries, 3, 0.02));
    }

    #[test]
    fn test_has_converged_handles_zero_or_negative_baseline() {
        // Global best is 0 → denominator becomes max(0, 1.0) = 1.0
        let summaries = vec![
            make_summary(0, Some(0.0), 0.0),
            make_summary(1, Some(0.005), 0.005),
            make_summary(2, Some(0.008), 0.003),
            make_summary(3, Some(0.010), 0.002),
        ];
        assert!(has_converged(&summaries, 3, 0.02));

        // Negative baseline
        let summaries_neg = vec![
            make_summary(0, Some(-2.0), 0.0),
            make_summary(1, Some(-1.99), 0.005),
            make_summary(2, Some(-1.98), 0.005),
            make_summary(3, Some(-1.97), 0.005),
        ];
        assert!(has_converged(&summaries_neg, 3, 0.02));
    }

    #[test]
    fn test_has_converged_round_0_alone_never_converges() {
        let summaries = vec![make_summary(0, Some(1.0), 0.0)];
        assert!(!has_converged(&summaries, 1, 0.02));
    }

    // === Adjacency tests ===

    #[test]
    fn test_adjacent_first_element() {
        let set = &[6, 8, 10, 12];
        let result = adjacent_values_u32(set, 6);
        assert_eq!(result, vec![8]);
    }

    #[test]
    fn test_adjacent_last_element() {
        let set = &[6, 8, 10, 12];
        let result = adjacent_values_u32(set, 12);
        assert_eq!(result, vec![10]);
    }

    #[test]
    fn test_adjacent_middle_element() {
        let set = &[6, 8, 10, 12];
        let result = adjacent_values_u32(set, 8);
        assert_eq!(result, vec![6, 10]);
    }

    #[test]
    fn test_adjacent_missing_value() {
        let set = &[6, 8, 10, 12];
        let result = adjacent_values_u32(set, 9);
        assert_eq!(result, vec![8, 10]);
    }

    #[test]
    fn test_adjacent_no_duplicates() {
        let set = &[6, 8, 10, 12];
        let result = adjacent_values_u32(set, 10);
        assert_eq!(result.len(), 2);
        assert_ne!(result[0], result[1]);
    }

    #[test]
    fn test_adjacent_f64_middle() {
        let set = &[0.20, 0.25, 0.30, 0.35];
        let result = adjacent_values_f64(set, 0.25);
        assert_eq!(result.len(), 2);
        assert!((result[0] - 0.20).abs() < 1e-6);
        assert!((result[1] - 0.30).abs() < 1e-6);
    }

    #[test]
    fn test_adjacent_f64_missing() {
        let set = &[0.20, 0.30, 0.40];
        let result = adjacent_values_f64(set, 0.25);
        assert_eq!(result.len(), 2);
        assert!((result[0] - 0.20).abs() < 1e-6);
        assert!((result[1] - 0.30).abs() < 1e-6);
    }

    // === Signature tests ===

    #[test]
    fn test_signature_excludes_hypothesis_id() {
        let args_with_hyp = vec![
            "--asset".to_string(),
            "SPY".to_string(),
            "--rule".to_string(),
            "trend_momentum".to_string(),
            "--fast-window".to_string(),
            "12".to_string(),
            "--slow-window".to_string(),
            "50".to_string(),
            "--hypothesis-id".to_string(),
            "seed-1-0".to_string(),
        ];
        let args_without_hyp = vec![
            "--asset".to_string(),
            "SPY".to_string(),
            "--rule".to_string(),
            "trend_momentum".to_string(),
            "--fast-window".to_string(),
            "12".to_string(),
            "--slow-window".to_string(),
            "50".to_string(),
        ];
        assert_eq!(
            param_signature_from_args(&args_with_hyp),
            param_signature_from_args(&args_without_hyp)
        );
    }

    #[test]
    fn test_signature_stable_float_formatting() {
        let args1 = vec![
            "--vol-cap".to_string(),
            "0.40".to_string(),
            "--asset".to_string(),
            "SPY".to_string(),
        ];
        let args2 = vec![
            "--vol-cap".to_string(),
            "0.4".to_string(),
            "--asset".to_string(),
            "SPY".to_string(),
        ];
        assert_eq!(
            param_signature_from_args(&args1),
            param_signature_from_args(&args2)
        );
    }

    #[test]
    fn test_signature_sorted_keys() {
        let args1 = vec![
            "--asset".to_string(),
            "SPY".to_string(),
            "--rule".to_string(),
            "trend_momentum".to_string(),
        ];
        let args2 = vec![
            "--rule".to_string(),
            "trend_momentum".to_string(),
            "--asset".to_string(),
            "SPY".to_string(),
        ];
        assert_eq!(
            param_signature_from_args(&args1),
            param_signature_from_args(&args2)
        );
    }

    // === Candidate construction tests ===

    #[test]
    fn test_refined_id_format() {
        let c = make_refined_candidate(
            RULE_TREND_MOMENTUM,
            "SPY",
            12,
            50,
            14,
            30,
            70,
            20,
            0.40,
            2,
            3,
            7,
        );
        assert_eq!(c.candidate_id, "refine-R2-W03-V007");
    }

    #[test]
    fn test_stable_arg_ordering() {
        let c = make_refined_candidate(
            RULE_RSI_REVERSION,
            "QQQ",
            10,
            35,
            14,
            28,
            72,
            20,
            0.40,
            1,
            0,
            0,
        );
        // Verify args contain expected key-value pairs
        assert_eq!(arg_value(&c.args, "--asset"), Some("QQQ"));
        assert_eq!(arg_value(&c.args, "--rule"), Some(RULE_RSI_REVERSION));
        assert_eq!(arg_u32(&c.args, "--fast-window"), Some(10));
        assert_eq!(arg_u32(&c.args, "--slow-window"), Some(35));
        assert_eq!(arg_u32(&c.args, "--rsi-window"), Some(14));
        assert_eq!(arg_u32(&c.args, "--rsi-oversold"), Some(28));
        assert_eq!(arg_u32(&c.args, "--rsi-overbought"), Some(72));
        assert_eq!(arg_u32(&c.args, "--vol-window"), Some(20));
        assert!((arg_f64(&c.args, "--vol-cap").unwrap() - 0.40).abs() < 1e-6);
    }

    // === Refinement logic tests ===

    #[test]
    fn test_fast_less_than_slow_invariant() {
        let winner = CandidateReport {
            candidate_id: "test-trend".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec![
                "--asset".to_string(),
                "SPY".to_string(),
                "--rule".to_string(),
                RULE_TREND_MOMENTUM.to_string(),
                "--fast-window".to_string(),
                "12".to_string(),
                "--slow-window".to_string(),
                "50".to_string(),
                "--rsi-window".to_string(),
                "14".to_string(),
                "--rsi-oversold".to_string(),
                "30".to_string(),
                "--rsi-overbought".to_string(),
                "70".to_string(),
                "--vol-window".to_string(),
                "20".to_string(),
                "--vol-cap".to_string(),
                "0.40".to_string(),
            ],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            train_audit: None,
            test_audit: None,
            is_seeded: false,
        };
        let loop_state = LoopState::new();
        let variants = refine_around_winner(
            &winner,
            &loop_state,
            1,
            0,
            100,
            10,
            &core_assets(),
            42,
            false,
        );
        for v in &variants {
            let fast = arg_u32(&v.args, "--fast-window").unwrap();
            let slow = arg_u32(&v.args, "--slow-window").unwrap();
            assert!(
                fast < slow,
                "fast ({fast}) must be < slow ({slow}) for {}",
                v.candidate_id
            );
        }
    }

    #[test]
    fn test_oversold_less_than_overbought_invariant() {
        let winner = CandidateReport {
            candidate_id: "test-rsi".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec![
                "--asset".to_string(),
                "QQQ".to_string(),
                "--rule".to_string(),
                RULE_RSI_REVERSION.to_string(),
                "--fast-window".to_string(),
                "10".to_string(),
                "--slow-window".to_string(),
                "35".to_string(),
                "--rsi-window".to_string(),
                "14".to_string(),
                "--rsi-oversold".to_string(),
                "28".to_string(),
                "--rsi-overbought".to_string(),
                "72".to_string(),
                "--vol-window".to_string(),
                "20".to_string(),
                "--vol-cap".to_string(),
                "0.40".to_string(),
            ],
            rule: RULE_RSI_REVERSION.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "QQQ".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            train_audit: None,
            test_audit: None,
            is_seeded: false,
        };
        let loop_state = LoopState::new();
        let variants = refine_around_winner(
            &winner,
            &loop_state,
            1,
            0,
            100,
            10,
            &core_assets(),
            42,
            false,
        );
        for v in &variants {
            let os = arg_u32(&v.args, "--rsi-oversold").unwrap();
            let ob = arg_u32(&v.args, "--rsi-overbought").unwrap();
            assert!(
                os < ob,
                "oversold ({os}) must be < overbought ({ob}) for {}",
                v.candidate_id
            );
        }
    }

    #[test]
    fn test_dedup_against_global_seen() {
        let winner = CandidateReport {
            candidate_id: "test-trend".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec![
                "--asset".to_string(),
                "SPY".to_string(),
                "--rule".to_string(),
                RULE_TREND_MOMENTUM.to_string(),
                "--fast-window".to_string(),
                "12".to_string(),
                "--slow-window".to_string(),
                "50".to_string(),
                "--rsi-window".to_string(),
                "14".to_string(),
                "--rsi-oversold".to_string(),
                "30".to_string(),
                "--rsi-overbought".to_string(),
                "70".to_string(),
                "--vol-window".to_string(),
                "20".to_string(),
                "--vol-cap".to_string(),
                "0.40".to_string(),
            ],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            train_audit: None,
            test_audit: None,
            is_seeded: false,
        };

        // Get initial variants
        let loop_state = LoopState::new();
        let variants_1 = refine_around_winner(
            &winner,
            &loop_state,
            1,
            0,
            100,
            10,
            &core_assets(),
            42,
            false,
        );
        assert!(!variants_1.is_empty());

        // Mark all those signatures as seen
        let mut loop_state_2 = LoopState::new();
        for v in &variants_1 {
            loop_state_2.all_evaluated.insert(param_signature(v));
        }

        // Second refinement should produce no novel variants
        let variants_2 = refine_around_winner(
            &winner,
            &loop_state_2,
            2,
            0,
            100,
            10,
            &core_assets(),
            42,
            false,
        );
        assert!(
            variants_2.is_empty(),
            "expected 0 novel variants, got {}",
            variants_2.len()
        );
    }

    #[test]
    fn test_exhausted_winners_skipped() {
        let winner = CandidateReport {
            candidate_id: "test-trend".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec![
                "--asset".to_string(),
                "SPY".to_string(),
                "--rule".to_string(),
                RULE_TREND_MOMENTUM.to_string(),
                "--fast-window".to_string(),
                "12".to_string(),
                "--slow-window".to_string(),
                "50".to_string(),
                "--rsi-window".to_string(),
                "14".to_string(),
                "--rsi-oversold".to_string(),
                "30".to_string(),
                "--rsi-overbought".to_string(),
                "70".to_string(),
                "--vol-window".to_string(),
                "20".to_string(),
                "--vol-cap".to_string(),
                "0.40".to_string(),
            ],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            train_audit: None,
            test_audit: None,
            is_seeded: false,
        };

        let mut loop_state = LoopState::new();
        let sig = param_signature_from_args(&winner.args);
        loop_state.mark_center_exhausted(&sig);

        let result = refine_around_winners(
            &[winner],
            &mut loop_state,
            5,
            20,
            10,
            &core_assets(),
            1,
            42,
            false,
        );
        assert!(result.is_empty());
    }

    #[test]
    fn test_deterministic_ordering() {
        let winner = CandidateReport {
            candidate_id: "test-trend".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec![
                "--asset".to_string(),
                "SPY".to_string(),
                "--rule".to_string(),
                RULE_TREND_MOMENTUM.to_string(),
                "--fast-window".to_string(),
                "12".to_string(),
                "--slow-window".to_string(),
                "50".to_string(),
                "--rsi-window".to_string(),
                "14".to_string(),
                "--rsi-oversold".to_string(),
                "30".to_string(),
                "--rsi-overbought".to_string(),
                "70".to_string(),
                "--vol-window".to_string(),
                "20".to_string(),
                "--vol-cap".to_string(),
                "0.40".to_string(),
            ],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            train_audit: None,
            test_audit: None,
            is_seeded: false,
        };
        let loop_state = LoopState::new();
        let v1 = refine_around_winner(
            &winner,
            &loop_state,
            1,
            0,
            100,
            10,
            &core_assets(),
            42,
            false,
        );
        let v2 = refine_around_winner(
            &winner,
            &loop_state,
            1,
            0,
            100,
            10,
            &core_assets(),
            42,
            false,
        );
        let ids1: Vec<&str> = v1.iter().map(|c| c.candidate_id.as_str()).collect();
        let ids2: Vec<&str> = v2.iter().map(|c| c.candidate_id.as_str()).collect();
        assert_eq!(ids1, ids2, "same seed should produce same ordering");
    }

    // === Ledger tests ===

    #[test]
    fn test_ledger_round_metadata() {
        let report = CandidateReport {
            candidate_id: "test-1".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec!["--asset".to_string(), "SPY".to_string()],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            train_audit: None,
            test_audit: None,
            is_seeded: false,
        };
        let recorded = RecordedResult {
            round: Some(2),
            signature: "test-sig".to_string(),
            report,
        };

        let dir = std::env::temp_dir().join("zonk_test_ledger");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_ledger.jsonl");
        let _ = std::fs::remove_file(&path);

        save_ledger(&path, &[recorded]).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(parsed["round"], json!(2));
        assert_eq!(parsed["candidate_id"], json!("test-1"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_ledger_legacy_backward_compatible() {
        let report = CandidateReport {
            candidate_id: "test-legacy".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec!["--asset".to_string(), "SPY".to_string()],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            train_audit: None,
            test_audit: None,
            is_seeded: false,
        };
        let recorded = RecordedResult {
            round: None, // legacy mode
            signature: "test-sig".to_string(),
            report,
        };

        let dir = std::env::temp_dir().join("zonk_test_ledger_legacy");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_ledger_legacy.jsonl");
        let _ = std::fs::remove_file(&path);

        save_ledger(&path, &[recorded]).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(content.trim()).unwrap();
        // Legacy mode: no "round" field
        assert!(
            parsed.get("round").is_none(),
            "legacy mode should not include round field"
        );
        assert_eq!(parsed["candidate_id"], json!("test-legacy"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_ledger_includes_audit_metadata() {
        let report = CandidateReport {
            candidate_id: "test-audit".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec!["--asset".to_string(), "QQQ".to_string()],
            rule: RULE_VOL_SPREAD.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "QQQ".to_string(),
            train_score: 1.0,
            test_score: 0.9,
            combined_score: 0.97,
            train_details: json!({"period_start": "2020-01-02", "period_end": "2024-12-31"}),
            test_details: json!({"period_start": "2025-01-02", "period_end": "2026-03-11"}),
            train_audit: Some(AuditWindowMeta {
                artifact_path: "autoresearch-audits/test-audit-train.json".to_string(),
                trade_count: 123,
                actual_period_start: "2020-01-02".to_string(),
                actual_period_end: "2024-12-31".to_string(),
            }),
            test_audit: Some(AuditWindowMeta {
                artifact_path: "autoresearch-audits/test-audit-test.json".to_string(),
                trade_count: 45,
                actual_period_start: "2025-01-02".to_string(),
                actual_period_end: "2026-03-11".to_string(),
            }),
            is_seeded: true,
        };
        let recorded = RecordedResult {
            round: Some(3),
            signature: "test-audit-sig".to_string(),
            report,
        };

        let dir = std::env::temp_dir().join("zonk_test_ledger_audit");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test_ledger_audit.jsonl");
        let _ = std::fs::remove_file(&path);

        save_ledger(&path, &[recorded]).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(
            parsed["train_audit"]["artifact_path"],
            json!("autoresearch-audits/test-audit-train.json")
        );
        assert_eq!(parsed["test_audit"]["trade_count"], json!(45));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_read_audit_window_meta_extracts_expected_fields() {
        let payload = json!({
            "audit": {
                "actual_period_start": "2020-01-02",
                "actual_period_end": "2024-12-31",
                "executed_trade_count": 88
            }
        });

        let meta = read_audit_window_meta("autoresearch-audits/demo.json", &payload)
            .expect("expected audit metadata");
        assert_eq!(meta.artifact_path, "autoresearch-audits/demo.json");
        assert_eq!(meta.trade_count, 88);
        assert_eq!(meta.actual_period_start, "2020-01-02");
        assert_eq!(meta.actual_period_end, "2024-12-31");
    }

    #[test]
    fn test_build_interactive_report_html_replaces_template_placeholders() {
        let report = CandidateReport {
            candidate_id: "seed-001-vol_spread-v0".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec![
                "--asset".to_string(),
                "QQQ".to_string(),
                "--rule".to_string(),
                RULE_VOL_SPREAD.to_string(),
            ],
            rule: RULE_VOL_SPREAD.to_string(),
            rationale: "Research lead from volatility spread literature.".to_string(),
            source: "https://arxiv.org/abs/1234.5678".to_string(),
            focus_asset: "QQQ".to_string(),
            train_score: 2.1,
            test_score: 1.9,
            combined_score: 2.0,
            train_details: json!({
                "name": "PaperResearch [QQQ|vol_spread]",
                "beginning_equity": 1000000.0,
                "final_equity": 1542500.0,
                "cagr": 0.12,
                "sharpe": 0.88,
                "max_drawdown": 0.11,
                "var_95": -0.01
            }),
            test_details: json!({
                "name": "PaperResearch [QQQ|vol_spread]",
                "beginning_equity": 1000000.0,
                "final_equity": 1184000.0,
                "cagr": 0.09,
                "sharpe": 0.73,
                "max_drawdown": 0.08,
                "var_95": -0.01
            }),
            train_audit: Some(AuditWindowMeta {
                artifact_path: "autoresearch-audits/seed-001-vol_spread-v0-train.json".to_string(),
                trade_count: 184,
                actual_period_start: "2020-01-02".to_string(),
                actual_period_end: "2024-12-31".to_string(),
            }),
            test_audit: Some(AuditWindowMeta {
                artifact_path: "autoresearch-audits/seed-001-vol_spread-v0-test.json".to_string(),
                trade_count: 90,
                actual_period_start: "2025-01-02".to_string(),
                actual_period_end: "2026-03-11".to_string(),
            }),
            is_seeded: true,
        };
        let meta = InteractiveReportMeta {
            generated_date: "2026-03-18T20:00:00Z".to_string(),
            total_ranked: 10,
            round_count: 5,
            train_start: "2020-01-01".to_string(),
            train_end: "2024-12-31".to_string(),
            test_start: "2025-01-01".to_string(),
            test_end: "2026-03-11".to_string(),
            framework_tag: "5-step Research Analysis Framework".to_string(),
        };

        let html = build_interactive_report_html(&[report], &meta);
        assert!(html.contains("The Strategy Frontier"));
        assert!(html.contains("seed-001-vol_spread-v0"));
        assert!(html.contains("5-step Research Analysis Framework"));
        assert!(!html.contains("/* PASTE_META_HERE */"));
        assert!(!html.contains("/* PASTE_ROWS_HERE */"));
    }

    #[test]
    fn test_save_strategy_registry_creates_entry() {
        let report = CandidateReport {
            candidate_id: "seed-001-vol_spread-v0".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec!["--asset".to_string(), "QQQ".to_string()],
            rule: RULE_VOL_SPREAD.to_string(),
            rationale: "test rationale".to_string(),
            source: "https://example.com/paper".to_string(),
            focus_asset: "QQQ".to_string(),
            train_score: 1.2,
            test_score: 1.1,
            combined_score: 1.15,
            train_details: json!({"final_equity": 1200000.0}),
            test_details: json!({"final_equity": 1100000.0}),
            train_audit: Some(AuditWindowMeta {
                artifact_path: "autoresearch-audits/demo-train.json".to_string(),
                trade_count: 12,
                actual_period_start: "2020-01-02".to_string(),
                actual_period_end: "2024-12-31".to_string(),
            }),
            test_audit: Some(AuditWindowMeta {
                artifact_path: "autoresearch-audits/demo-test.json".to_string(),
                trade_count: 8,
                actual_period_start: "2025-01-02".to_string(),
                actual_period_end: "2026-03-11".to_string(),
            }),
            is_seeded: true,
        };
        let recorded = RecordedResult {
            round: Some(0),
            signature: "sig-001".to_string(),
            report,
        };

        let dir = std::env::temp_dir().join("zonk_test_registry_create");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("strategy_registry.json");
        let _ = std::fs::remove_file(&path);

        save_strategy_registry(&path, &[recorded]).unwrap();
        let registry: StrategyRegistry =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(registry.entries.len(), 1);
        let entry = &registry.entries[0];
        assert_eq!(entry.signature, "sig-001");
        assert_eq!(entry.registry_status, "research_candidate");
        assert_eq!(entry.times_in_top10, 1);
        assert_eq!(entry.latest.candidate_id, "seed-001-vol_spread-v0");
        assert_eq!(
            entry.latest.train_audit.as_ref().unwrap().artifact_path,
            "autoresearch-audits/demo-train.json"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_save_strategy_registry_upserts_by_signature() {
        let dir = std::env::temp_dir().join("zonk_test_registry_upsert");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("strategy_registry.json");
        let _ = std::fs::remove_file(&path);

        let make_recorded =
            |candidate_id: &str, source: &str, combined_score: f64, round: Option<u32>| {
                RecordedResult {
                    round,
                    signature: "shared-sig".to_string(),
                    report: CandidateReport {
                        candidate_id: candidate_id.to_string(),
                        strategy: RESEARCH_STRATEGY.to_string(),
                        category: "Research".to_string(),
                        args: vec!["--asset".to_string(), "QQQ".to_string()],
                        rule: RULE_VOL_SPREAD.to_string(),
                        rationale: "test rationale".to_string(),
                        source: source.to_string(),
                        focus_asset: "QQQ".to_string(),
                        train_score: combined_score - 0.1,
                        test_score: combined_score + 0.1,
                        combined_score,
                        train_details: json!({"final_equity": 1000000.0 + combined_score}),
                        test_details: json!({"final_equity": 1000000.0 + combined_score}),
                        train_audit: None,
                        test_audit: None,
                        is_seeded: true,
                    },
                }
            };

        save_strategy_registry(
            &path,
            &[make_recorded(
                "seed-001-vol_spread-v0",
                "https://example.com/one",
                1.25,
                Some(0),
            )],
        )
        .unwrap();
        save_strategy_registry(
            &path,
            &[make_recorded(
                "seed-099-vol_spread-v2",
                "https://example.com/two",
                1.75,
                Some(2),
            )],
        )
        .unwrap();

        let registry: StrategyRegistry =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(registry.entries.len(), 1);
        let entry = &registry.entries[0];
        assert_eq!(entry.times_in_top10, 2);
        assert_eq!(entry.latest.candidate_id, "seed-099-vol_spread-v2");
        assert_eq!(entry.best.candidate_id, "seed-099-vol_spread-v2");
        assert_eq!(entry.best.combined_score, 1.75);
        assert_eq!(entry.candidate_ids.len(), 2);
        assert_eq!(entry.sources.len(), 2);

        let _ = std::fs::remove_file(&path);
    }

    // === LoopState tests ===

    #[test]
    fn test_loop_state_retain_novel_candidates() {
        let mut state = LoopState::new();
        let c1 = make_refined_candidate(
            RULE_TREND_MOMENTUM,
            "SPY",
            12,
            50,
            14,
            30,
            70,
            20,
            0.40,
            0,
            0,
            0,
        );
        let c2 = make_refined_candidate(
            RULE_TREND_MOMENTUM,
            "QQQ",
            12,
            50,
            14,
            30,
            70,
            20,
            0.40,
            0,
            0,
            1,
        );
        // c3 is a duplicate of c1 (same params, different hypothesis-id due to different variant_idx but same param signature)
        let c3 = make_refined_candidate(
            RULE_TREND_MOMENTUM,
            "SPY",
            12,
            50,
            14,
            30,
            70,
            20,
            0.40,
            0,
            0,
            2,
        );

        let novel = state.retain_novel_candidates(vec![c1, c2, c3]);
        // c1 and c2 should pass, c3 should be deduped
        assert_eq!(novel.len(), 2);
    }

    #[test]
    fn test_loop_state_current_best_score() {
        let mut state = LoopState::new();
        assert!(state.current_best_score().is_none());

        let report = CandidateReport {
            candidate_id: "test".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec!["--asset".to_string(), "SPY".to_string()],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            train_audit: None,
            test_audit: None,
            is_seeded: false,
        };
        state.recorded_results.push(RecordedResult {
            round: Some(0),
            signature: "test".to_string(),
            report,
        });
        assert!((state.current_best_score().unwrap() - 0.93).abs() < 1e-6);
    }

    // === vol_spread tests ===

    #[test]
    fn test_extract_seed_tags_vix_keywords() {
        let vix_seed = ExaSeed {
            title: "VIX trading strategy with implied volatility".to_string(),
            url: "https://arxiv.org/abs/2407.16780".to_string(),
            text: "This paper studies the variance risk premium and GARCH models.".to_string(),
        };
        let tags = extract_seed_tags(&vix_seed);
        assert!(
            tags.contains("vol_spread"),
            "expected vol_spread tag for VIX-related seed, got: {tags:?}"
        );

        let garch_seed = ExaSeed {
            title: "GARCH-LSTM hybrid forecast".to_string(),
            url: String::new(),
            text: "Forecasting realized volatility with neural networks.".to_string(),
        };
        let tags2 = extract_seed_tags(&garch_seed);
        assert!(
            tags2.contains("vol_spread"),
            "expected vol_spread tag for GARCH seed, got: {tags2:?}"
        );

        // Non-VIX seed should NOT get vol_spread tag
        let momentum_seed = ExaSeed {
            title: "Momentum strategies in equity markets".to_string(),
            url: String::new(),
            text: "Cross-sectional momentum and trend following.".to_string(),
        };
        let tags3 = extract_seed_tags(&momentum_seed);
        assert!(
            !tags3.contains("vol_spread"),
            "non-VIX seed should not get vol_spread tag"
        );
    }

    #[test]
    fn test_short_keywords_require_token_boundaries() {
        let blob = seed_classification_blob(&ExaSeed {
            title: "Construction and Hedging of Equity Index Options Portfolios".to_string(),
            url: "https://arxiv.org/html/2407.13908v1".to_string(),
            text: "University of Warsaw research group. Keywords: Implied Volatility, Volatility Risk Premium, Volatility Spreads, Dynamic Hedging.".to_string(),
        });

        assert!(
            !contains_any(&blob, &["rsi"]),
            "short keyword should not match inside unrelated words"
        );
        assert!(
            contains_any(&blob, &["volatility spreads"]),
            "expected real phrase match to keep working"
        );
    }

    #[test]
    fn test_deep_hedging_seed_requires_new_rule_family() {
        let seed = ExaSeed {
            title: "Deep Hedging with Options Using the Implied Volatility Surface".to_string(),
            url: "https://arxiv.org/html/2504.06208v1".to_string(),
            text: "We propose an enhanced deep hedging framework for index option portfolios, grounded in a realistic market simulator that captures the joint dynamics of S&P 500 returns and the full implied volatility surface. The hedging strategy also considers the variance risk premium embedded in the hedging instruments and explicitly accounts for transaction costs.".to_string(),
        };

        assert_eq!(
            classify_seed_rule_family(&seed),
            SeedRuleFamily::NeedsNewRule("options_hedging")
        );

        let candidates = build_seed_candidates(&seed, 19, RESEARCH_ASSETS);
        assert!(
            candidates.is_empty(),
            "unsupported deep hedging paper should not be forced into an existing rule"
        );
    }

    #[test]
    fn test_mean_reversion_seed_maps_to_rsi_rule_only() {
        let seed = ExaSeed {
            title: "Optimal Mean Reversion Trading".to_string(),
            url: "https://arxiv.org/abs/1602.05858".to_string(),
            text: "This paper studies mean reversion trading with oversold and overbought conditions and explicit oscillator thresholds.".to_string(),
        };

        assert_eq!(
            classify_seed_rule_family(&seed),
            SeedRuleFamily::ExistingRule(RULE_RSI_REVERSION)
        );

        let candidates = build_seed_candidates(&seed, 7, RESEARCH_ASSETS);
        assert!(!candidates.is_empty(), "expected seeded RSI candidates");
        assert!(
            candidates.iter().all(|c| c.rule == RULE_RSI_REVERSION),
            "mean reversion seed should only map to RSI reversion variants"
        );
    }

    #[test]
    fn test_option_pricing_seed_requires_new_rule_family() {
        let seed = ExaSeed {
            title: "Option Pricing with Time-Varying Volatility Risk Aversion".to_string(),
            url: "https://arxiv.org/abs/2204.06943".to_string(),
            text: "[2204.06943] Option Pricing with Time-Varying Volatility Risk Aversion\n\n> Abstract: We introduce a pricing kernel with time-varying volatility risk aversion to explain observed time variations in the shape of the pricing kernel. When combined with the Heston-Nandi GARCH model, this framework yields a tractable option pricing model in which the variance risk ratio emerges as a key variable. We demonstrate substantial reductions in pricing errors through an empirical application to the S&P 500 index, the CBOE VIX, and option prices.\n\n## Submission history\nRelated Papers\nMean reversion in order flow".to_string(),
        };

        assert_eq!(
            classify_seed_rule_family(&seed),
            SeedRuleFamily::NeedsNewRule("option_pricing")
        );

        let candidates = build_seed_candidates(&seed, 30, RESEARCH_ASSETS);
        assert!(
            candidates.is_empty(),
            "option-pricing paper should not be forced into an existing directional rule"
        );
    }

    #[test]
    fn test_options_portfolio_seed_prefers_vol_spread_family() {
        let seed = ExaSeed {
            title: "Construction and Hedging of Equity Index Options Portfolios".to_string(),
            url: "https://arxiv.org/html/2407.13908v1".to_string(),
            text: "###### Abstract\nThis research presents a comprehensive evaluation of systematic index option-writing strategies, focusing on S&P500 index options. We compare the performance of hedging strategies using the Black-Scholes-Merton model and different sizing methods based on delta and the VIX Index. Based on the concept of volatility risk premium and aiming to exploit options premiums by selling volatility, systematic option writing strategies such as volatility spreads are utilized with various hedging schemes and sizing methodologies.".to_string(),
        };

        assert_eq!(
            classify_seed_rule_family(&seed),
            SeedRuleFamily::ExistingRule(RULE_VOL_SPREAD)
        );

        let candidates = build_seed_candidates(&seed, 12, RESEARCH_ASSETS);
        assert!(
            !candidates.is_empty(),
            "expected vol-spread seeded candidates"
        );
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.rule == RULE_VOL_SPREAD),
            "options portfolio seed should map to vol_spread variants only"
        );
    }

    #[test]
    fn test_vix_forecasting_seed_prefers_volatility_regime_over_rsi() {
        let seed = ExaSeed {
            title: "Forecasting VIX using interpretable Kolmogorov-Arnold networks".to_string(),
            url: "https://arxiv.org/abs/2502.00980".to_string(),
            text: "Abstract: This paper presents the use of Kolmogorov-Arnold Networks for forecasting the CBOE Volatility Index (VIX). The closed-form forecast provides interpretable insights into key characteristics of the VIX, including mean reversion and the leverage effect.".to_string(),
        };

        assert_eq!(
            classify_seed_rule_family(&seed),
            SeedRuleFamily::ExistingRule(RULE_VOL_REGIME)
        );

        let candidates = build_seed_candidates(&seed, 135, RESEARCH_ASSETS);
        assert!(
            !candidates.is_empty(),
            "expected seeded volatility candidates"
        );
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.rule == RULE_VOL_REGIME),
            "VIX forecasting paper should not fall back to RSI reversion"
        );
    }

    #[test]
    fn test_seed_candidate_uses_paper_title_from_seed_text_when_exa_title_is_generic() {
        let seed = ExaSeed {
            title: "Quantitative Finance > Portfolio Management".to_string(),
            url: "https://arxiv.org/abs/2512.12420".to_string(),
            text: "[2512.12420] Deep Hedging with Reinforcement Learning: A Practical Framework for Option Risk Management\n# Title:Deep Hedging with Reinforcement Learning: A Practical Framework for Option Risk Management".to_string(),
        };

        let candidate = seed_candidate(
            19,
            &seed,
            RULE_RSI_REVERSION,
            "TQQQ",
            6,
            16,
            Some(16),
            Some(24),
            Some(70),
            None,
            None,
            1,
        );

        assert!(
            candidate
                .rationale
                .contains("Deep Hedging with Reinforcement Learning"),
            "expected rationale to contain the parsed paper title, got: {}",
            candidate.rationale
        );
        assert!(
            !candidate
                .rationale
                .contains("Quantitative Finance > Portfolio Management"),
            "generic subject bucket should not be used as the paper title"
        );
    }

    #[test]
    fn test_seed_paper_title_falls_back_to_pdf_title_line_without_citation_noise() {
        let seed = ExaSeed {
            title: String::new(),
            url: "https://arxiv.org/pdf/2010.12245".to_string(),
            text: "Option Hedging with Risk Averse Reinforcement Learning\nEdoardo Vittori, Michele Trapletti\nABSTRACT\n...\n[5] . (5)\n".to_string(),
        };

        assert_eq!(
            seed_paper_title(&seed),
            "Option Hedging with Risk Averse Reinforcement Learning"
        );
    }

    #[test]
    fn test_seed_paper_title_skips_submitted_paper_placeholder() {
        let seed = ExaSeed {
            title: "Submitted paper 1".to_string(),
            url: "https://arxiv.org/pdf/1701.05016".to_string(),
            text: "arXiv:1701.05016v1 [q-fin.PM] 18 Jan 2017\nSubmitted paper 1\nMean-Reverting Portfolio Design with Budget\nConstraint\nZiping Zhao, Student Member, IEEE, and Daniel P. Palomar, Fellow, IEEE\nAbstract".to_string(),
        };

        assert_eq!(
            seed_paper_title(&seed),
            "Mean-Reverting Portfolio Design with Budget Constraint"
        );
    }

    #[test]
    fn test_research_basis_for_seeded_candidate_is_explicitly_hypothesis_driven() {
        let args = vec![
            "--rsi-window".to_string(),
            "16".to_string(),
            "--rsi-oversold".to_string(),
            "24".to_string(),
        ];
        let narrative = research_basis(
            RULE_RSI_REVERSION,
            "TQQQ",
            "ArXiv-seeded hypothesis (19): Deep Hedging with Reinforcement Learning: A Practical Framework for Option Risk Management",
            "https://arxiv.org/abs/2512.12420",
            &args,
        );

        assert!(
            narrative.contains("research lead"),
            "seeded narrative should explain that the paper is a research lead: {narrative}"
        );
        assert!(
            narrative.contains("not a direct replication"),
            "seeded narrative should avoid claiming direct replication: {narrative}"
        );
    }

    #[test]
    fn test_score_paper_research_preserves_beginning_equity() {
        let payload = json!({
            "period_start": "2020-01-02",
            "period_end": "2024-12-31",
            "capital": 1_000_000.0,
            "results": [{
                "name": "PaperResearch [SPY|trend_momentum]",
                "beginning_equity": 1_000_000.0,
                "final_equity": 1_250_000.0,
                "cagr": 0.10,
                "sharpe": 0.8,
                "max_drawdown": -0.12,
                "var_95": -0.01
            }]
        });

        let scored = score_paper_research(&payload).expect("expected score");
        assert_eq!(
            safe_float(scored.details.get("beginning_equity").unwrap()).unwrap(),
            1_000_000.0
        );
        assert_eq!(
            safe_float(scored.details.get("final_equity").unwrap()).unwrap(),
            1_250_000.0
        );
        assert_eq!(
            scored.details.get("period_start").unwrap(),
            &json!("2020-01-02")
        );
        assert_eq!(
            scored.details.get("period_end").unwrap(),
            &json!("2024-12-31")
        );
    }

    #[test]
    fn test_load_eval_cache_backfills_missing_beginning_equity() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("valid time")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zonk-eval-cache-{unique}.jsonl"));
        let line = serde_json::to_string(&json!({
            "eval_key": "demo",
            "passed": true,
            "train_score": 1.0,
            "test_score": 1.0,
            "combined_score": 2.0,
            "train_details": {"name": "PaperResearch [QQQ|vol_spread]", "final_equity": 1_250_000.0},
            "test_details": {"name": "PaperResearch [QQQ|vol_spread]", "beginning_equity": 0.0, "final_equity": 1_100_000.0}
        }))
        .expect("serialize cache line");
        std::fs::write(&path, format!("{line}\n")).expect("write temp cache");

        let cache = load_eval_cache(&path);
        let _ = std::fs::remove_file(&path);
        let entry = cache.get("demo").expect("expected cache entry");

        assert_eq!(
            safe_float(entry.train_details.get("beginning_equity").unwrap()).unwrap(),
            DEFAULT_PAPER_RESEARCH_BEGINNING_EQUITY
        );
        assert_eq!(
            safe_float(entry.test_details.get("beginning_equity").unwrap()).unwrap(),
            DEFAULT_PAPER_RESEARCH_BEGINNING_EQUITY
        );
    }

    #[test]
    fn test_refine_around_winner_vol_spread() {
        let winner = CandidateReport {
            candidate_id: "test-vspread".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec![
                "--asset".to_string(),
                "SPY".to_string(),
                "--rule".to_string(),
                RULE_VOL_SPREAD.to_string(),
                "--fast-window".to_string(),
                "12".to_string(),
                "--slow-window".to_string(),
                "50".to_string(),
                "--rsi-window".to_string(),
                "14".to_string(),
                "--rsi-oversold".to_string(),
                "30".to_string(),
                "--rsi-overbought".to_string(),
                "70".to_string(),
                "--vol-window".to_string(),
                "22".to_string(),
                "--vol-cap".to_string(),
                "0.20".to_string(),
            ],
            rule: RULE_VOL_SPREAD.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            train_audit: None,
            test_audit: None,
            is_seeded: false,
        };
        let loop_state = LoopState::new();
        let variants = refine_around_winner(
            &winner,
            &loop_state,
            1,
            0,
            100,
            10,
            &core_assets(),
            42,
            false,
        );
        assert!(
            !variants.is_empty(),
            "expected vol_spread refinement variants"
        );

        // Verify variants perturb vol_window or vol_cap (or asset swap), not fast/slow
        let mut has_vol_window_change = false;
        let mut has_vol_cap_change = false;
        for v in &variants {
            let vw = arg_u32(&v.args, "--vol-window").unwrap_or(22);
            let vc = arg_f64(&v.args, "--vol-cap").unwrap_or(0.20);
            if vw != 22 {
                has_vol_window_change = true;
            }
            if (vc - 0.20).abs() > 1e-6 {
                has_vol_cap_change = true;
            }
        }
        assert!(
            has_vol_window_change,
            "expected at least one vol_window perturbation"
        );
        assert!(
            has_vol_cap_change,
            "expected at least one vol_cap perturbation"
        );
    }

    #[test]
    fn test_rule_description_vol_spread() {
        let args_positive = vec![
            "--vol-window".to_string(),
            "22".to_string(),
            "--vol-cap".to_string(),
            "0.20".to_string(),
        ];
        let desc = rule_description(RULE_VOL_SPREAD, &args_positive, "SPY");
        assert!(
            desc.contains("VRP harvest"),
            "positive threshold should describe VRP harvest, got: {desc}"
        );
        assert!(desc.contains("SPY"), "description should mention asset");

        let args_negative = vec![
            "--vol-window".to_string(),
            "22".to_string(),
            "--vol-cap".to_string(),
            "-0.20".to_string(),
        ];
        let desc_neg = rule_description(RULE_VOL_SPREAD, &args_negative, "QQQ");
        assert!(
            desc_neg.contains("snap-back"),
            "negative threshold should describe snap-back, got: {desc_neg}"
        );
    }

    #[test]
    fn test_grid_vol_spread_candidates_unique() {
        // Use a large target so we don't hit the early-return cap before reaching vol_spread
        let candidates = build_deterministic_grid_candidates(5000, RESEARCH_ASSETS);
        let vol_spread_candidates: Vec<_> = candidates
            .iter()
            .filter(|c| c.rule == RULE_VOL_SPREAD)
            .collect();
        assert!(
            !vol_spread_candidates.is_empty(),
            "grid should produce vol_spread candidates"
        );

        // Check for unique signatures
        let mut sigs = HashSet::new();
        for c in &vol_spread_candidates {
            let sig = param_signature(c);
            assert!(
                sigs.insert(sig.clone()),
                "duplicate vol_spread signature: {sig}"
            );
        }

        // Verify fast/slow are pinned to defaults
        for c in &vol_spread_candidates {
            let fast = arg_u32(&c.args, "--fast-window").unwrap_or(0);
            let slow = arg_u32(&c.args, "--slow-window").unwrap_or(0);
            assert_eq!(fast, 12, "vol_spread grid should pin fast to 12");
            assert_eq!(slow, 50, "vol_spread grid should pin slow to 50");
        }
    }

    // === Deterministic sample tests ===

    #[test]
    fn test_deterministic_sample_reproducible() {
        let pool: Vec<String> = (0..50).map(|i| format!("TICK{i}")).collect();
        let a = deterministic_sample(&pool, 10, 42);
        let b = deterministic_sample(&pool, 10, 42);
        assert_eq!(a, b, "same seed must produce same subset");
    }

    #[test]
    fn test_deterministic_sample_different_seeds() {
        let pool: Vec<String> = (0..50).map(|i| format!("TICK{i}")).collect();
        let a = deterministic_sample(&pool, 10, 42);
        let b = deterministic_sample(&pool, 10, 99);
        assert_ne!(a, b, "different seeds should produce different subsets");
    }

    #[test]
    fn test_deterministic_sample_small_pool() {
        let pool: Vec<String> = vec!["A".into(), "B".into(), "C".into()];
        let result = deterministic_sample(&pool, 10, 42);
        assert_eq!(result.len(), 3, "pool <= n should return entire pool");
        // All original items should be present
        let mut sorted = result.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["A", "B", "C"]);
    }

    // === Asset swap refinement tests ===

    #[test]
    fn test_refine_asset_swaps_from_viable_pool() {
        let viable = vec![
            "SPY".to_string(),
            "QQQ".to_string(),
            "AAPL".to_string(),
            "MSFT".to_string(),
            "GOOGL".to_string(),
            "AMZN".to_string(),
        ];
        let winner = CandidateReport {
            candidate_id: "test-pool".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec![
                "--asset".to_string(),
                "SPY".to_string(),
                "--rule".to_string(),
                RULE_TREND_MOMENTUM.to_string(),
                "--fast-window".to_string(),
                "12".to_string(),
                "--slow-window".to_string(),
                "50".to_string(),
                "--rsi-window".to_string(),
                "14".to_string(),
                "--rsi-oversold".to_string(),
                "30".to_string(),
                "--rsi-overbought".to_string(),
                "70".to_string(),
                "--vol-window".to_string(),
                "20".to_string(),
                "--vol-cap".to_string(),
                "0.40".to_string(),
            ],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            train_audit: None,
            test_audit: None,
            is_seeded: false,
        };
        let loop_state = LoopState::new();
        let variants =
            refine_around_winner(&winner, &loop_state, 1, 0, 200, 10, &viable, 42, false);

        // Collect all asset swaps (non-SPY assets)
        let swap_assets: Vec<&str> = variants
            .iter()
            .map(|v| v.focus_asset.as_str())
            .filter(|a| *a != "SPY")
            .collect();

        // Should have some swaps from the viable pool
        assert!(
            !swap_assets.is_empty(),
            "expected asset swaps from viable pool"
        );

        // All swap assets must come from the viable pool
        for asset in &swap_assets {
            assert!(
                viable.iter().any(|v| v.as_str() == *asset),
                "swap asset {asset} not in viable pool"
            );
        }
    }

    #[test]
    fn test_refine_asset_swaps_excludes_current_asset() {
        let viable = vec![
            "SPY".to_string(),
            "QQQ".to_string(),
            "AAPL".to_string(),
            "MSFT".to_string(),
        ];
        let winner = CandidateReport {
            candidate_id: "test-exclude".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec![
                "--asset".to_string(),
                "SPY".to_string(),
                "--rule".to_string(),
                RULE_VOL_REGIME.to_string(),
                "--fast-window".to_string(),
                "12".to_string(),
                "--slow-window".to_string(),
                "50".to_string(),
                "--rsi-window".to_string(),
                "14".to_string(),
                "--rsi-oversold".to_string(),
                "30".to_string(),
                "--rsi-overbought".to_string(),
                "70".to_string(),
                "--vol-window".to_string(),
                "20".to_string(),
                "--vol-cap".to_string(),
                "0.40".to_string(),
            ],
            rule: RULE_VOL_REGIME.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            train_audit: None,
            test_audit: None,
            is_seeded: false,
        };
        let loop_state = LoopState::new();
        let variants =
            refine_around_winner(&winner, &loop_state, 1, 0, 200, 10, &viable, 42, false);

        // Asset swap variants should never include the winner's asset as a swap target
        // (parameter perturbation variants keep the same asset, which is fine)
        let asset_swap_variants: Vec<&Candidate> =
            variants.iter().filter(|v| v.focus_asset != "SPY").collect();
        for v in &asset_swap_variants {
            assert_ne!(
                v.focus_asset, "SPY",
                "asset swap should never re-use the winner's asset"
            );
        }
    }

    #[test]
    fn test_backward_compat_core_universe() {
        // When viable_assets = RESEARCH_ASSETS, behavior should match old code
        let winner = CandidateReport {
            candidate_id: "test-compat".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec![
                "--asset".to_string(),
                "SPY".to_string(),
                "--rule".to_string(),
                RULE_TREND_MOMENTUM.to_string(),
                "--fast-window".to_string(),
                "12".to_string(),
                "--slow-window".to_string(),
                "50".to_string(),
                "--rsi-window".to_string(),
                "14".to_string(),
                "--rsi-oversold".to_string(),
                "30".to_string(),
                "--rsi-overbought".to_string(),
                "70".to_string(),
                "--vol-window".to_string(),
                "20".to_string(),
                "--vol-cap".to_string(),
                "0.40".to_string(),
            ],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            train_audit: None,
            test_audit: None,
            is_seeded: false,
        };
        let loop_state = LoopState::new();
        // Use core assets (same 5 as RESEARCH_ASSETS), allow all swaps
        let variants = refine_around_winner(
            &winner,
            &loop_state,
            1,
            0,
            200,
            10,
            &core_assets(),
            42,
            false,
        );

        // Should have variants with assets from core list only
        for v in &variants {
            assert!(
                RESEARCH_ASSETS.contains(&v.focus_asset.as_str()),
                "core universe variant has unexpected asset: {}",
                v.focus_asset
            );
        }

        // Should have exactly 4 asset swap variants (RESEARCH_ASSETS minus SPY)
        let swap_count = variants.iter().filter(|v| v.focus_asset != "SPY").count();
        assert_eq!(
            swap_count, 4,
            "core universe should produce 4 asset swaps (5 - current)"
        );
    }

    // === Quality gate tests ===

    fn make_report_with_metrics(
        asset: &str,
        test_sharpe: f64,
        test_drawdown: f64,
        combined_score: f64,
    ) -> CandidateReport {
        CandidateReport {
            candidate_id: format!("test-{asset}"),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec!["--asset".to_string(), asset.to_string()],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: asset.to_string(),
            train_score: 1.0,
            test_score: combined_score,
            combined_score,
            train_details: json!({"sharpe": 1.5, "max_drawdown": -0.10}),
            test_details: json!({"sharpe": test_sharpe, "max_drawdown": test_drawdown}),
            train_audit: None,
            test_audit: None,
            is_seeded: false,
        }
    }

    #[test]
    fn test_quality_gates_min_sharpe() {
        let candidates = vec![
            make_report_with_metrics("SPY", 1.5, -0.10, 5.0), // passes
            make_report_with_metrics("QQQ", 0.8, -0.05, 4.0), // fails sharpe
            make_report_with_metrics("AAPL", 1.0, -0.15, 3.0), // passes (exactly at threshold)
        ];
        let filtered = apply_quality_gates(&candidates, Some(1.0), None);
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].focus_asset, "SPY");
        assert_eq!(filtered[1].focus_asset, "AAPL");
    }

    #[test]
    fn test_quality_gates_max_drawdown() {
        let candidates = vec![
            make_report_with_metrics("SPY", 1.5, -0.10, 5.0), // -10% passes (< 20%)
            make_report_with_metrics("QQQ", 2.0, -0.25, 4.0), // -25% fails (> 20%)
            make_report_with_metrics("AAPL", 1.0, -0.20, 3.0), // -20% passes (exactly at threshold)
        ];
        let filtered = apply_quality_gates(&candidates, None, Some(20.0));
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].focus_asset, "SPY");
        assert_eq!(filtered[1].focus_asset, "AAPL");
    }

    #[test]
    fn test_quality_gates_combined() {
        let candidates = vec![
            make_report_with_metrics("SPY", 1.5, -0.10, 5.0), // passes both
            make_report_with_metrics("QQQ", 0.8, -0.05, 4.0), // fails sharpe
            make_report_with_metrics("AAPL", 1.2, -0.25, 3.0), // fails drawdown
            make_report_with_metrics("MSFT", 1.1, -0.15, 2.0), // passes both
        ];
        let filtered = apply_quality_gates(&candidates, Some(1.0), Some(20.0));
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].focus_asset, "SPY");
        assert_eq!(filtered[1].focus_asset, "MSFT");
    }

    #[test]
    fn test_quality_gates_no_filters() {
        let candidates = vec![
            make_report_with_metrics("SPY", 0.5, -0.30, 5.0),
            make_report_with_metrics("QQQ", -1.0, -0.50, 4.0),
        ];
        let filtered = apply_quality_gates(&candidates, None, None);
        assert_eq!(filtered.len(), 2, "no gates should pass everything");
    }

    #[test]
    fn test_quality_gates_all_rejected() {
        let candidates = vec![
            make_report_with_metrics("SPY", 0.5, -0.30, 5.0),
            make_report_with_metrics("QQQ", 0.3, -0.40, 4.0),
        ];
        let filtered = apply_quality_gates(&candidates, Some(1.0), Some(20.0));
        assert!(filtered.is_empty(), "all should be rejected");
    }

    // === Convergence summary tests ===

    #[test]
    fn test_convergence_summary_does_not_panic() {
        // Just verify print_convergence_summary doesn't panic with various states
        let empty_state = LoopState::new();
        print_convergence_summary(&empty_state, "test: empty state");

        let mut state = LoopState::new();
        let report = CandidateReport {
            candidate_id: "test".to_string(),
            strategy: RESEARCH_STRATEGY.to_string(),
            category: "Research".to_string(),
            args: vec!["--asset".to_string(), "SPY".to_string()],
            rule: RULE_TREND_MOMENTUM.to_string(),
            rationale: "test".to_string(),
            source: "test".to_string(),
            focus_asset: "SPY".to_string(),
            train_score: 1.0,
            test_score: 0.8,
            combined_score: 0.93,
            train_details: json!({}),
            test_details: json!({}),
            train_audit: None,
            test_audit: None,
            is_seeded: false,
        };
        state.record_round(0, 10, 10, vec![report.clone()], false);
        state.record_round(1, 5, 5, vec![report], false);
        state.mark_center_exhausted("test-sig");
        print_convergence_summary(&state, "test: with data");
    }
}
