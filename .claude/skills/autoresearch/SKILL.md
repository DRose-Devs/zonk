# Autoresearch

Launch the autoresearch loop with common production flags.

## Usage
```
/autoresearch [mode]
```

Modes:
- `quick` — 10 candidates, 3 top, 2 rounds, core assets (fast smoke test)
- `standard` — 100 candidates, 10 top, 10 rounds, broad assets (default)
- `full` — 100 candidates, 10 top, 10 rounds, full warehouse universe
- `investable` — standard + quality gates (Sharpe >= 1.0, drawdown <= 20%)

## Instructions

1. Build the release binary:
   ```bash
   cargo build --release 2>&1 | tail -3
   ```

2. Based on the mode (default: `standard`), run the autoresearch loop:

   **quick:**
   ```bash
   cargo run --release --bin autoresearch_loop -- \
     --asset-universe core --candidates 10 --top 3 --max-rounds 2 --verbose
   ```

   **standard:**
   ```bash
   cargo run --release --bin autoresearch_loop -- \
     --seed-web --candidates 100 --top 10 --verbose
   ```

   **full:**
   ```bash
   cargo run --release --bin autoresearch_loop -- \
     --seed-web --asset-universe full --candidates 100 --top 10 --verbose
   ```

   **investable:**
   ```bash
   cargo run --release --bin autoresearch_loop -- \
     --seed-web --candidates 100 --top 10 --min-sharpe 1.0 --max-drawdown 20 --verbose
   ```

3. Stream the output to the user. When complete, summarize:
   - Number of candidates evaluated
   - Number of rounds completed
   - Best strategy: rule, asset, score, Sharpe, drawdown
   - Whether the interactive report was generated

4. Do NOT interrupt the loop once started. Let it run to completion or convergence.
