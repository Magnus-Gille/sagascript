/// The pure AVFoundation status mappings of `macos_mic`, whose other code
/// needs a real macOS bundle.
#[cfg(target_os = "macos")]
mod macos_mic_tests {
    use crate::commands::permissions::macos_mic::{
        access_request_decision, authorization_status_label, AccessRequestDecision,
    };

    #[test]
    fn maps_avfoundation_authorization_constants_correctly() {
        assert_eq!(authorization_status_label(0, false), "not_determined");
        assert_eq!(authorization_status_label(1, false), "restricted");
        assert_eq!(authorization_status_label(2, false), "denied");
        assert_eq!(authorization_status_label(3, false), "authorized");
    }

    #[test]
    fn denied_cache_can_be_refreshed_without_remapping_authorized() {
        assert_eq!(authorization_status_label(2, true), "authorized");
        assert_eq!(authorization_status_label(3, false), "authorized");
    }

    #[test]
    fn request_decision_live_queries_undetermined_and_cached_denied() {
        assert_eq!(
            access_request_decision(0),
            AccessRequestDecision::QuerySystem
        );
        assert_eq!(
            access_request_decision(2),
            AccessRequestDecision::QuerySystem
        );
    }

    #[test]
    fn request_decision_completes_known_restricted_and_authorized_states() {
        assert_eq!(
            access_request_decision(1),
            AccessRequestDecision::Complete(false)
        );
        assert_eq!(
            access_request_decision(3),
            AccessRequestDecision::Complete(true)
        );
    }
}
