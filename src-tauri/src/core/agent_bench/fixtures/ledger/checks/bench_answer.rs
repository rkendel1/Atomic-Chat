//! Held-out check for the `ledger-explain-rounding` task.

#[test]
fn answer_names_the_rounding_function() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/ANSWER.txt");
    let text = std::fs::read_to_string(path).expect("ANSWER.txt exists in the project root");
    let answer = text
        .trim()
        .trim_matches('`')
        .trim_start_matches("crate::")
        .trim_start_matches("ledger::");
    assert_eq!(answer, "money::round_half_even");
}
