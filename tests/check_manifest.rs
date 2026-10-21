//! The port must account for every check the shell suite printed.

mod common;

#[test]
fn every_shell_check_has_a_rust_check() {
    assert_eq!(common::EXPECTED.len(), 23, "23 task sections");
    assert_eq!(
        common::EXPECTED.iter().sum::<usize>(),
        common::TOTAL_CHECKS,
        "task counts must sum to the 261 checks tests/run.sh printed"
    );
    assert_eq!(common::TOTAL_CHECKS, 261);
}
