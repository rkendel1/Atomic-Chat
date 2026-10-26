//! Amounts and how fractions of a cent become whole cents.

/// Rounds an amount given in hundredths of a cent to whole cents.
///
/// Exact halves go to the even neighbour (banker's rounding), so repeated
/// rounding does not drift in one direction.
pub fn round_half_even(hundredths: i64) -> i64 {
    let whole = hundredths.div_euclid(100);
    let rest = hundredths.rem_euclid(100);
    match rest {
        r if r > 50 => whole + 1,
        r if r < 50 => whole,
        _ => whole + (whole & 1),
    }
}

/// Formats cents as dollars: `1234` is `$12.34`, `-5` is `-$0.05`.
pub fn format_cents(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let abs = cents.unsigned_abs();
    format!("{sign}${}.{:02}", abs / 100, abs % 100)
}
