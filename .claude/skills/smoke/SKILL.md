# Smoke

Run the full doob smoke test suite: format check, unit tests, release build, and autoresearch smoke tests across all universe modes.

## Usage
```
/smoke
```

## Instructions

1. Run formatting check:
   ```bash
   cargo fmt --check
   ```

2. Run all unit tests:
   ```bash
   cargo test
   ```
   Report the test counts (lib + bin).

3. Build release binary:
   ```bash
   cargo build --release
   ```

4. Run autoresearch smoke tests (all 3 universe modes):
   ```bash
   # Core mode (backward compat)
   cargo run --release --bin autoresearch_loop -- \
     --asset-universe core --candidates 10 --top 3 --max-rounds 2 --verbose

   # Broad mode (default)
   cargo run --release --bin autoresearch_loop -- \
     --candidates 10 --top 3 --max-rounds 2 --verbose

   # Full warehouse mode
   cargo run --release --bin autoresearch_loop -- \
     --asset-universe full --candidates 10 --top 3 --max-rounds 2 --verbose
   ```

5. Report results:
   - Format: PASS/FAIL
   - Unit tests: count and status
   - Each smoke test: completed or error
   - Overall: PASS only if all steps succeed
