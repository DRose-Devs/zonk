#!/bin/bash
set -e

SNAPSHOT_ARGS=(
  --snapshot-date 2003-01-02
  --snapshot-date 2003-07-01
  --snapshot-date 2004-01-02
  --snapshot-date 2004-07-01
  --snapshot-date 2005-01-03
  --snapshot-date 2005-07-01
  --snapshot-date 2006-01-03
  --snapshot-date 2006-07-03
  --snapshot-date 2007-01-03
  --snapshot-date 2007-07-03
  --snapshot-date 2008-01-02
  --snapshot-date 2008-07-01
  --snapshot-date 2009-01-02
  --snapshot-date 2009-07-01
  --snapshot-date 2010-01-04
  --snapshot-date 2010-07-01
  --snapshot-date 2011-01-03
  --snapshot-date 2011-07-01
  --snapshot-date 2012-01-03
  --snapshot-date 2012-07-02
  --snapshot-date 2013-01-02
  --snapshot-date 2013-07-01
  --snapshot-date 2014-01-02
  --snapshot-date 2014-07-01
  --snapshot-date 2015-01-02
  --snapshot-date 2015-07-01
  --snapshot-date 2016-01-04
  --snapshot-date 2016-07-01
  --snapshot-date 2017-01-03
  --snapshot-date 2017-07-03
  --snapshot-date 2018-01-02
  --snapshot-date 2018-07-02
  --snapshot-date 2019-01-02
  --snapshot-date 2019-07-01
  --snapshot-date 2020-01-02
  --snapshot-date 2020-07-01
  --snapshot-date 2021-01-04
  --snapshot-date 2021-07-01
  --snapshot-date 2022-01-03
  --snapshot-date 2022-07-01
  --snapshot-date 2023-01-03
  --snapshot-date 2023-07-03
  --snapshot-date 2024-01-02
  --snapshot-date 2024-07-01
  --snapshot-date 2025-01-02
  --snapshot-date 2025-07-25
  --snapshot-date 2025-11-07
  --snapshot-date 2026-01-16
  --snapshot-date 2026-03-11
)

for THR in "$@"; do
  echo "=== Running threshold ${THR}% ==="
  RUST_LOG=info ./target/release/zonk run breadth-washout \
    --universe ndx100 \
    --signal-mode oversold \
    --threshold "$THR" \
    --sessions 5831 \
    --assets SPY \
    "${SNAPSHOT_ARGS[@]}" \
    2>&1 | grep -E "(Strategy metrics|horizon|cumul_ret|Forward-return|Files|summary:|viz:)"
  echo ""
done
