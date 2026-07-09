pub mod signals;

use proptest::test_runner::{Config, FileFailurePersistence};

/// A proptest config running `cases` cases unless `PROPTEST_CASES` is set.
/// Failing inputs go to `<test file>.regressions` beside the test, since
/// proptest's default lookup expects a `src/` tree.
pub fn cases(cases: u32) -> Config {
    let mut config = if std::env::var_os("PROPTEST_CASES").is_some() {
        Config::default()
    } else {
        Config::with_cases(cases)
    };
    config.failure_persistence = Some(Box::new(FileFailurePersistence::WithSource("regressions")));
    config
}
