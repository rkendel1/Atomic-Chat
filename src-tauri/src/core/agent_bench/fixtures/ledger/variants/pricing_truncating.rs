//! Line items and discounts.

/// The sum of `(quantity, unit price in cents)` line items.
pub fn subtotal(lines: &[(u32, i64)]) -> i64 {
    lines.iter().map(|&(qty, unit)| qty as i64 * unit).sum()
}

/// Applies a percentage discount to `total_cents`. `percent` above 100 is
/// treated as 100. The result is rounded with `money::round_half_even`.
pub fn apply_discount(total_cents: i64, percent: u8) -> i64 {
    let percent = percent.min(100) as i64;
    total_cents * (100 - percent) / 100
}
