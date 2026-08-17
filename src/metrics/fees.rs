/// IBKR Tiered fee model (US equities).

const IBKR_PER_SHARE: f64 = 0.0035;
const IBKR_EXCHANGE_REG: f64 = 0.0030;
const IBKR_TOTAL_PER_SHARE: f64 = IBKR_PER_SHARE + IBKR_EXCHANGE_REG;
const IBKR_MIN_ORDER: f64 = 0.35;
const IBKR_MAX_PCT: f64 = 0.01;

fn one_side(n_shares: f64, trade_value: f64) -> f64 {
    let raw = n_shares * IBKR_TOTAL_PER_SHARE;
    let raw = raw.max(IBKR_MIN_ORDER);
    raw.min(IBKR_MAX_PCT * trade_value)
}

/// IBKR tiered round-trip cost for a fully-invested position.
///
/// Returns total dollar cost for buy + sell.
pub fn ibkr_roundtrip_cost(equity: f64, price: f64) -> f64 {
    let shares = (equity / price) as i64;
    if shares <= 0 {
        return 0.0;
    }
    let trade_value = shares as f64 * price;
    one_side(shares as f64, trade_value) + one_side(shares as f64, trade_value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn test_normal_cost() {
        let cost = ibkr_roundtrip_cost(100_000.0, 500.0);
        let shares = 200.0;
        let per_side = shares * IBKR_TOTAL_PER_SHARE;
        assert_relative_eq!(cost, per_side * 2.0, epsilon = 1e-10);
    }

    #[test]
    fn test_min_order() {
        let cost = ibkr_roundtrip_cost(100.0, 100.0);
        assert_relative_eq!(cost, 0.35 * 2.0, epsilon = 1e-10);
    }

    #[test]
    fn test_max_cap() {
        let cost = ibkr_roundtrip_cost(10_000.0, 0.10);
        let shares: i64 = 100_000;
        let trade_value = shares as f64 * 0.10;
        let max_per_side = 0.01 * trade_value;
        let raw_per_side = shares as f64 * IBKR_TOTAL_PER_SHARE;
        assert_relative_eq!(cost, max_per_side * 2.0, epsilon = 1e-10);
        assert!(raw_per_side > max_per_side);
    }

    #[test]
    fn test_zero_shares() {
        assert_eq!(ibkr_roundtrip_cost(0.0, 100.0), 0.0);
        assert_eq!(ibkr_roundtrip_cost(50.0, 100.0), 0.0);
    }
}
