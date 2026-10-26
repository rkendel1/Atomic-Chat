use ledger::text::slugify;

#[test]
fn slugify_joins_words_with_dashes() {
    assert_eq!(slugify("Hello, World!"), "hello-world");
    assert_eq!(slugify("  Q3 -- Report  "), "q3-report");
    assert_eq!(slugify(""), "");
}
