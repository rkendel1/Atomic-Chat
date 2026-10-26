use ledger::money::{format_cents, round_half_even};

#[test]
fn rounds_to_the_nearest_cent() {
    assert_eq!(round_half_even(1249), 12);
    assert_eq!(round_half_even(1251), 13);
}

#[test]
fn exact_halves_round_to_even() {
    assert_eq!(round_half_even(1250), 12);
    assert_eq!(round_half_even(1350), 14);
    assert_eq!(round_half_even(-150), -2);
}

#[test]
fn formats_dollars() {
    assert_eq!(format_cents(1234), "$12.34");
    assert_eq!(format_cents(-5), "-$0.05");
    assert_eq!(format_cents(0), "$0.00");
}
