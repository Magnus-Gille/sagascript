use super::*;
const SHA: &str = "0123456789012345678901234567890123456789";
fn successful_process() -> ProcessObservations {
    ProcessObservations {
        microphone: Observation::observed(
            Source::CurrentProcess,
            MicrophonePermissionStatus::Authorized,
        ),
        accessibility: Observation::observed(Source::CurrentProcess, AccessibilityStatus::Trusted),
        in_app_bundle: Observation::observed(Source::CurrentProcess, true),
        binary_sha256: Observation::observed(Source::CurrentProcess, "a".repeat(64)),
    }
}
fn report(process: ProcessObservations, onboarding: Observation<bool>) -> PreflightReport {
    assemble(
        SHA,
        BuildObservation::new("1.4.6", SHA, "false"),
        process,
        onboarding,
        Source::CurrentProcess,
    )
}
fn not_observed() -> Observation<bool> {
    Observation::unavailable(Source::NotObserved, ObservationStatus::NotObserved)
}
#[test]
fn every_ungranted_microphone_state_blocks_including_unknown_and_unsupported() {
    for status in [
        MicrophonePermissionStatus::Denied,
        MicrophonePermissionStatus::Restricted,
        MicrophonePermissionStatus::NotDetermined,
        MicrophonePermissionStatus::Unknown,
        MicrophonePermissionStatus::Unsupported,
    ] {
        let mut process = successful_process();
        process.microphone.value = Some(status);
        assert_eq!(
            report(process, not_observed()).summary.status,
            CheckStatus::Blocked,
            "{status:?}"
        );
    }
}
#[test]
fn unknown_observations_never_pass_even_with_stale_success_values() {
    for status in [
        ObservationStatus::Unknown,
        ObservationStatus::Unsupported,
        ObservationStatus::NotObserved,
        ObservationStatus::Error,
        ObservationStatus::Invalid,
    ] {
        for field in ["microphone", "accessibility", "binary"] {
            let mut process = successful_process();
            match field {
                "microphone" => process.microphone.status = status,
                "accessibility" => process.accessibility.status = status,
                _ => process.binary_sha256.status = status,
            }
            assert_eq!(
                report(process, not_observed()).summary.status,
                CheckStatus::Blocked
            );
        }
    }
}
#[test]
fn revision_mismatch_dirty_or_unknown_source_blocks() {
    for (revision, dirty) in [
        ("unknown", "false"),
        ("ffffffffffffffffffffffffffffffffffffffff", "false"),
        (SHA, "true"),
        (SHA, "unknown"),
    ] {
        let report = assemble(
            SHA,
            BuildObservation::new("1.4.6", revision, dirty),
            successful_process(),
            not_observed(),
            Source::CurrentProcess,
        );
        assert_eq!(report.summary.status, CheckStatus::Blocked);
    }
}
#[test]
fn optional_fixture_and_live_gui_observations_keep_distinct_provenance() {
    let no_fixture = report(successful_process(), not_observed());
    assert_eq!(no_fixture.summary.status, CheckStatus::Passed);
    assert!(no_fixture
        .summary
        .remaining_manual_steps
        .contains(&"onboarding_not_observed"));
    for source in [Source::PersistedFixture, Source::LiveGui] {
        for completed in [false, true] {
            let value = report(
                successful_process(),
                Observation::observed(source, completed),
            );
            assert_eq!(value.onboarding.source, source);
            assert_eq!(
                value.summary.status,
                if completed {
                    CheckStatus::Passed
                } else {
                    CheckStatus::Blocked
                }
            );
            assert_eq!(value.gui_session.status, ObservationStatus::NotObserved);
        }
        for status in [
            ObservationStatus::Invalid,
            ObservationStatus::Error,
            ObservationStatus::Unknown,
        ] {
            assert_eq!(
                report(
                    successful_process(),
                    Observation::unavailable(source, status)
                )
                .summary
                .status,
                CheckStatus::Blocked
            );
        }
    }
}
#[test]
fn report_contract_snapshot_and_cli_gui_payload_parity() {
    let cli = report(
        successful_process(),
        Observation::observed(Source::PersistedFixture, true),
    );
    let gui = assemble(
        SHA,
        BuildObservation::new("1.4.6", SHA, "false"),
        successful_process(),
        Observation::observed(Source::LiveGui, true),
        Source::LiveGui,
    );
    let mut cli = serde_json::to_value(cli).unwrap();
    let mut gui = serde_json::to_value(gui).unwrap();
    assert_eq!(cli["scope"], "current_process");
    assert_eq!(gui["scope"], "live_gui");
    assert_eq!(cli["onboarding"]["source"], "persisted_fixture");
    assert_eq!(gui["onboarding"]["source"], "live_gui");
    for doc in [&mut cli, &mut gui] {
        doc["scope"] = serde_json::json!("current_process");
        doc["onboarding"]["source"] = serde_json::json!("persisted_fixture");
        doc["summary"]["remaining_manual_steps"] = serde_json::json!([]);
        doc["platform"] = serde_json::json!("fixture_os");
        doc["architecture"] = serde_json::json!("fixture_arch");
    }
    assert_eq!(cli, gui);
    let snapshot: serde_json::Value =
        serde_json::from_str(include_str!("report.fixture.json")).unwrap();
    assert_eq!(cli, snapshot);
}
#[test]
fn permission_and_observation_statuses_have_stable_wire_names() {
    let statuses = [
        ObservationStatus::Observed,
        ObservationStatus::Unknown,
        ObservationStatus::Unsupported,
        ObservationStatus::NotObserved,
        ObservationStatus::Invalid,
        ObservationStatus::Error,
    ];
    assert_eq!(
        serde_json::to_value(statuses).unwrap(),
        serde_json::json!([
            "observed",
            "unknown",
            "unsupported",
            "not_observed",
            "invalid",
            "error"
        ])
    );
    assert_eq!(
        serde_json::to_value([
            AccessibilityStatus::Trusted,
            AccessibilityStatus::NotTrusted,
            AccessibilityStatus::Unsupported
        ])
        .unwrap(),
        serde_json::json!(["trusted", "not_trusted", "unsupported"])
    );
}
