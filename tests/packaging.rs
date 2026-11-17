//! The Homebrew formula must hand cargo one copy of each flag.
//!
//! Homebrew's `std_cargo_args` already passes `--locked`. Cargo stops with
//! `the argument '--locked' cannot be used multiple times` when a formula adds
//! its own copy, so the install line carries no flag of its own.

const FORMULA: &str = include_str!("../packaging/homebrew/lightsout.rb");

#[test]
fn formula_installs_through_std_cargo_args() {
    assert!(
        FORMULA.contains(r#"system "cargo", "install", *std_cargo_args"#),
        "the formula must install through std_cargo_args, which passes --locked and --path"
    );
}

#[test]
fn formula_passes_no_duplicate_cargo_flag() {
    assert!(
        !FORMULA.contains(r#""--locked""#),
        "std_cargo_args already passes --locked, and cargo refuses it twice"
    );
    assert!(
        !FORMULA.contains(r#""--root"#),
        "std_cargo_args already passes --root, and cargo refuses it twice"
    );
}
