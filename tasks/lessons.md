# Lessons

## 2026-03-18

- Do not trust `ExaSeed.title` as the paper title without validation. If it looks generic or placeholder-like, such as `Quantitative Finance > ...` or `Submitted paper 1`, extract the actual title from the seed text before storing rationale or rendering reports.
- For web-seeded autoresearch candidates, report narratives must describe the linked paper as a research lead or inspiration source unless the implementation is a direct replication. Do not claim the paper itself used the exact rule being backtested when the mapping is heuristic.
- Do not force unrelated seed papers into existing paper-research rules just to populate seeded candidates. If a paper points to a net-new rule family such as deep hedging, option overlays, or portfolio construction and there is no credible supported-rule match, skip seeded candidate generation for that paper and let the fallback path handle coverage.
- Seed classification must use title-plus-abstract semantics with token-boundary matching, not raw substring scans over the full scraped arXiv page. Short tokens like `rsi` will produce false positives inside unrelated words and page chrome otherwise.
- When report/detail schemas gain new fields, add a cache-upgrade path for older `reports/autoresearch-eval-cache.jsonl` entries so regenerated reports do not surface stale zero or missing values.
- Reported equity values are not audit-grade unless the engine can reproduce the exact evaluated window and emit the concrete trade/equity path that leads from beginning equity to ending equity. Persist proof artifacts for promoted candidates instead of relying on summary metrics alone.
