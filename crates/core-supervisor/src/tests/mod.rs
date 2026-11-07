//! Native lease assertions must not share an FD table with unrelated test forks.

const ISOLATED_CASE: &str = "POLARIS_TEST_ISOLATED_LEASE_CASE";

/// Return true in the parent after this exact case passed in a fresh test image.
/// The caller creates its fixture/lease only after this returns false in the child.
pub(crate) fn isolated_lease_case(test: &str) -> bool {
    if let Some(selected) = std::env::var_os(ISOLATED_CASE) {
        assert_eq!(selected.to_str(), Some(test));
        let args: Vec<_> = std::env::args().skip(1).collect();
        assert_eq!(
            args,
            [
                "--exact",
                test,
                "--nocapture",
                "--test-threads=1",
                "--color=never"
            ]
        );
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            test,
            "--nocapture",
            "--test-threads=1",
            "--color=never",
        ])
        .env(ISOLATED_CASE, test)
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        output.status.success() && exactly_one_case_passed(&stdout),
        "isolated case {test}: status={}\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

fn exactly_one_case_passed(stdout: &str) -> bool {
    let mut results = stdout
        .lines()
        .filter(|line| line.starts_with("test result:"));
    stdout
        .lines()
        .filter(|line| *line == "running 1 test")
        .count()
        == 1
        && results.next().is_some_and(|line| {
            line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; ")
        })
        && results.next().is_none()
}

#[test]
fn isolated_result_requires_one_actual_passing_case() {
    let passed = "running 1 test\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 245 filtered out; finished in 0.01s\n";
    assert!(exactly_one_case_passed(passed));
    for invalid in [
        passed.replace("running 1 test", "running 0 tests"),
        passed.replace("1 passed", "0 passed"),
        passed.replace("0 ignored", "1 ignored"),
        passed.replace("0 failed", "1 failed"),
        passed.replace("test result: ok.", "test result: FAILED."),
        format!("{passed}{passed}"),
        String::new(),
    ] {
        assert!(!exactly_one_case_passed(&invalid), "{invalid}");
    }
}
