//! Held-out check for the `ledger-add-truncate` task.

use ledger::text::truncate_words;

#[test]
fn keeps_short_text_unchanged() {
    assert_eq!(truncate_words("one two", 3), "one two");
    assert_eq!(truncate_words("one two three", 3), "one two three");
}

#[test]
fn drops_words_past_the_limit_and_marks_it() {
    assert_eq!(truncate_words("one two three four", 2), "one two...");
}

#[test]
fn collapses_whitespace() {
    assert_eq!(truncate_words("  one\t two\n", 5), "one two");
}

#[test]
fn zero_words_is_empty() {
    assert_eq!(truncate_words("one two", 0), "");
    assert_eq!(truncate_words("", 0), "");
}
