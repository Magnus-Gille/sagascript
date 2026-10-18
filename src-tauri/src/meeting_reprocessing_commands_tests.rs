use super::*;
use sagascript_testsupport::TempDir;

/// The source error of `open_meeting_proposal` used to be thrown away
/// (`map_err(|_| …)`). The user still sees the fixed text, and the cause is kept.
#[test]
fn an_unreadable_proposal_keeps_its_fixed_text_and_its_cause() {
    let directory = TempDir::new();
    let missing = directory.join("proposal.json");
    let error = read_proposal_file(&missing).expect_err("a missing file is no proposal");
    assert_eq!(error.code(), "invalid_input");
    assert_eq!(
        error.to_string(),
        "Could not read a valid, bounded meeting proposal"
    );
    let cause = error.cause().expect("the cause is kept");
    assert!(!cause.is_empty(), "the cause is not empty");

    let garbage = directory.join("garbage.json");
    std::fs::write(&garbage, b"{ not a proposal").expect("write the file");
    let error = read_proposal_file(&garbage).expect_err("garbage is no proposal");
    assert_eq!(error.code(), "invalid_input");
    assert!(error.cause().is_some());
    // What the webview receives is the code and the fixed text only.
    let wire = crate::contract::to_value(&error);
    assert_eq!(
        wire["message"],
        "Could not read a valid, bounded meeting proposal"
    );
    assert!(!wire
        .to_string()
        .contains(&*directory.path().to_string_lossy()));
}
