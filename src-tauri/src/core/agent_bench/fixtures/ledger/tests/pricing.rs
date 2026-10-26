use ledger::pricing::{apply_discount, subtotal};

#[test]
fn subtotal_sums_line_items() {
    assert_eq!(subtotal(&[(2, 350), (1, 99)]), 799);
    assert_eq!(subtotal(&[]), 0);
}

#[test]
fn discount_rounds_half_even() {
    // 1001 * 85% = 850.85 cents, which rounds to 851.
    assert_eq!(apply_discount(1001, 15), 851);
    // 5 * 50% = 2.5 cents, an exact half: rounds to the even 2.
    assert_eq!(apply_discount(5, 50), 2);
    assert_eq!(apply_discount(1000, 0), 1000);
}

#[test]
fn discount_is_capped_at_one_hundred_percent() {
    assert_eq!(apply_discount(1234, 100), 0);
    assert_eq!(apply_discount(1234, 250), 0);
}
