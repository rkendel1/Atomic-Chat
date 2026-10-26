//! The bundled benchmark: one fixture project and the tasks that run on it.
//!
//! The fixture is a dependency-free Rust crate (`fixtures/ledger`), embedded in
//! the app and copied into a fresh workspace for every run, so benchmark runs
//! never touch Atomic Chat or any other project. Verification needs only
//! `cargo` (no network: `--offline`), and no task needs a hosted model.

use super::task::{BenchTask, Evaluation, Limits, VerificationCommand, WorkdirRef, TASK_SCHEMA};

pub const SUITE_ID: &str = "atomic-ledger";
pub const SUITE_VERSION: u32 = 1;

type Files = &'static [(&'static str, &'static str)];

macro_rules! fixture_file {
    ($path:literal, $src:literal) => {
        ($path, include_str!(concat!("fixtures/ledger/", $src)))
    };
}

/// The fixture's files as every variant shares them. `.gitignore` is stored
/// without its dot so it never applies to this repository.
const LEDGER_BASE: Files = &[
    fixture_file!("Cargo.toml", "Cargo.toml"),
    fixture_file!(".gitignore", "gitignore"),
    fixture_file!("README.md", "README.md"),
    fixture_file!("src/lib.rs", "src/lib.rs"),
    fixture_file!("src/money.rs", "src/money.rs"),
    fixture_file!("src/text.rs", "src/text.rs"),
    fixture_file!("tests/money.rs", "tests/money.rs"),
    fixture_file!("tests/pricing.rs", "tests/pricing.rs"),
    fixture_file!("tests/text.rs", "tests/text.rs"),
];

const LEDGER_PRICING: Files = &[fixture_file!("src/pricing.rs", "src/pricing.rs")];
/// The same module with a deterministic bug: it truncates instead of rounding.
const LEDGER_PRICING_BUG: Files = &[fixture_file!(
    "src/pricing.rs",
    "variants/pricing_truncating.rs"
)];

const CHECKS_ANSWER: Files = &[fixture_file!(
    "tests/bench_answer.rs",
    "checks/bench_answer.rs"
)];
const CHECKS_PRICING: Files = &[fixture_file!("tests/pricing.rs", "checks/pricing.rs")];
const CHECKS_TRUNCATE: Files = &[fixture_file!(
    "tests/bench_truncate.rs",
    "checks/bench_truncate.rs"
)];

/// Fixture ids and their overlays on the base files.
const FIXTURES: &[(&str, Files)] = &[
    ("ledger", LEDGER_PRICING),
    ("ledger-discount-bug", LEDGER_PRICING_BUG),
];

const HELD_OUT: &[(&str, Files)] = &[
    ("ledger-answer", CHECKS_ANSWER),
    ("ledger-pricing", CHECKS_PRICING),
    ("ledger-truncate", CHECKS_TRUNCATE),
];

pub fn fixture_exists(id: &str) -> bool {
    FIXTURES.iter().any(|(f, _)| *f == id)
}

pub fn fixture_ids() -> Vec<&'static str> {
    FIXTURES.iter().map(|(f, _)| *f).collect()
}

/// Every file of a fixture, relative path first.
pub fn fixture_files(id: &str) -> Option<Vec<(&'static str, &'static str)>> {
    let (_, overlay) = FIXTURES.iter().find(|(f, _)| *f == id)?;
    let mut files: Vec<_> = LEDGER_BASE
        .iter()
        .filter(|(p, _)| !overlay.iter().any(|(o, _)| o == p))
        .copied()
        .collect();
    files.extend(overlay.iter().copied());
    files.sort_by(|a, b| a.0.cmp(b.0));
    Some(files)
}

pub fn held_out_files(id: &str) -> Option<Files> {
    HELD_OUT.iter().find(|(h, _)| *h == id).map(|(_, f)| *f)
}

/// Writes a fixture into `dir`, which must be empty or missing.
pub fn materialize(id: &str, dir: &std::path::Path) -> Result<(), String> {
    let files = fixture_files(id).ok_or_else(|| format!("unknown fixture {id:?}"))?;
    write_files(dir, &files)
}

pub fn write_files(dir: &std::path::Path, files: &[(&str, &str)]) -> Result<(), String> {
    for (rel, content) in files {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, content)
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    }
    Ok(())
}

fn cargo(name: &str, args: &[&str]) -> VerificationCommand {
    VerificationCommand {
        name: name.to_string(),
        program: "cargo".to_string(),
        args: args.iter().map(|a| a.to_string()).collect(),
        timeout_secs: Some(600),
    }
}

// One call per bundled task reads better than a struct literal per task.
#[allow(clippy::too_many_arguments)]
fn builtin(
    id: &str,
    name: &str,
    description: &str,
    goal: &str,
    fixture: &str,
    criteria: &[&str],
    commands: Vec<VerificationCommand>,
    held_out: &str,
) -> BenchTask {
    BenchTask {
        schema: TASK_SCHEMA.to_string(),
        id: id.to_string(),
        version: SUITE_VERSION,
        name: name.to_string(),
        description: description.to_string(),
        goal: goal.to_string(),
        workdir: WorkdirRef::Fixture {
            fixture: fixture.to_string(),
        },
        success_criteria: criteria.iter().map(|c| c.to_string()).collect(),
        evaluation: Evaluation::Command {
            commands,
            held_out_checks: Some(held_out.to_string()),
        },
        limits: Limits {
            timeout_secs: Some(900),
            max_turns: Some(16),
        },
        builtin: true,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

/// The bundled tasks, each testing a different capability.
pub fn builtin_tasks() -> Vec<BenchTask> {
    vec![
        builtin(
            "ledger-explain-rounding",
            "Understand: find the rounding rule",
            "Codebase understanding. The answer is checked by a held-out test the agent never sees.",
            "In this Rust project, find the function that decides how fractions of a cent are \
             rounded to whole cents. Write its path as module::function (for example \
             text::slugify) as the only content of a new file ANSWER.txt in the project root. \
             Do not change any other file.",
            "ledger",
            &[
                "ANSWER.txt contains exactly the module::function path of the rounding function",
                "All existing tests still pass",
            ],
            vec![
                cargo("Answer is correct", &["test", "--offline", "--test", "bench_answer"]),
                cargo("Existing tests pass", &["test", "--offline"]),
            ],
            "ledger-answer",
        ),
        builtin(
            "ledger-fix-discount",
            "Fix: failing discount test",
            "Fix a deterministic failing test. tests/pricing.rs is restored before verification, \
             so editing the test cannot pass it.",
            "The test discount_rounds_half_even in tests/pricing.rs fails. Find the cause in the \
             library code and fix it. Do not change the tests.",
            "ledger-discount-bug",
            &[
                "cargo test passes, including tests/pricing.rs exactly as shipped",
                "No test file is modified",
            ],
            vec![
                cargo("Pricing tests pass", &["test", "--offline", "--test", "pricing"]),
                cargo("All tests pass", &["test", "--offline"]),
            ],
            "ledger-pricing",
        ),
        builtin(
            "ledger-add-truncate",
            "Change: add truncate_words",
            "A bounded code change that must preserve existing behavior. Checked by a held-out \
             test plus the existing suite.",
            "Add a public function `truncate_words(s: &str, max_words: usize) -> String` to \
             src/text.rs. It returns the first max_words whitespace-separated words of s joined \
             by single spaces, followed by \"...\" only when words were dropped. With \
             max_words = 0 it returns an empty string. Keep every existing function and test \
             unchanged.",
            "ledger",
            &[
                "ledger::text::truncate_words exists with that signature and behavior",
                "All existing tests still pass",
            ],
            vec![
                cargo(
                    "truncate_words behaves as specified",
                    &["test", "--offline", "--test", "bench_truncate"],
                ),
                cargo("All tests pass", &["test", "--offline"]),
            ],
            "ledger-truncate",
        ),
    ]
}
