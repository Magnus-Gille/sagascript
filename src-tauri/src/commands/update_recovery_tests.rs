use super::*;

#[cfg(test)]
mod recovery_payload_tests {
    use super::update_recovery_payload;
    use crate::update_recovery::RecoverySnapshot;
    use serde_json::{json, Value};

    /// The refusals of the draft guard are `invalid_input`; a draft file that
    /// cannot be used is `io_error`, with the file's path removed on the wire.
    #[test]
    fn recovery_failures_say_what_kind_of_failure_they_are() {
        use crate::command_error::CommandError;
        use crate::update_recovery::RecoveryError;

        for snapshot in [
            RecoverySnapshot {
                schema_version: 2,
                timestamp: 1,
                payload: json!({"schema_version": 1}),
            },
            RecoverySnapshot {
                schema_version: 1,
                timestamp: 1,
                payload: json!([]),
            },
        ] {
            let error = update_recovery_payload(Some(snapshot)).unwrap_err();
            assert_eq!(error.code(), "invalid_input");
            assert!(error.to_string().contains("the stored draft was retained"));
        }

        let too_large = CommandError::from(RecoveryError::PayloadTooLarge {
            actual: 9,
            maximum: 4,
        });
        assert_eq!(too_large.code(), "invalid_input");

        // A stored file that is no draft and a payload that does not serialize are
        // about the draft, like a stored envelope the guard refuses.
        let parse_error = || serde_json::from_str::<Value>("{").unwrap_err();
        let corrupt = CommandError::from(RecoveryError::Corrupt {
            path: "/Users/zoe/Library/Application Support/update-recovery.json".into(),
            source: parse_error(),
        });
        assert_eq!(corrupt.code(), "invalid_input");
        assert_eq!(
            CommandError::from(RecoveryError::Serialize(parse_error())).code(),
            "invalid_input"
        );
        let unusable = CommandError::from(RecoveryError::Io {
            operation: "read",
            path: "/Users/zoe/update-recovery.json".into(),
            source: std::io::Error::other("disk gone"),
        });
        assert_eq!(unusable.code(), "io_error");

        let unreadable = CommandError::from(RecoveryError::InvalidPath(
            "/Users/zoe/Library/Application Support/update-recovery.json".into(),
        ));
        assert_eq!(unreadable.code(), "io_error");
        let wire = crate::contract::to_value(&unreadable);
        let message = wire["message"].as_str().expect("a message");
        assert!(message.contains("update-recovery.json"), "{message}");
        assert!(
            !message.contains("zoe"),
            "the account name stays off the wire: {message}"
        );
    }

    #[test]
    fn missing_snapshot_is_the_only_absent_draft() {
        assert_eq!(update_recovery_payload(None).unwrap(), None);
        for payload in [Value::Null, json!(false), json!(1), json!(""), json!([])] {
            let snapshot = RecoverySnapshot {
                schema_version: 1,
                timestamp: 1,
                payload,
            };
            assert!(update_recovery_payload(Some(snapshot)).is_err());
        }
    }

    #[test]
    fn unsupported_envelope_version_is_not_an_absent_draft() {
        for schema_version in [0, 2] {
            let snapshot = RecoverySnapshot {
                schema_version,
                timestamp: 1,
                payload: json!({"schema_version": 1}),
            };
            assert!(update_recovery_payload(Some(snapshot)).is_err());
        }
    }

    #[test]
    fn object_payload_is_preserved_for_strict_frontend_validation() {
        let payload = json!({"schema_version": 1, "dictation": null, "future_field": [1, 2]});
        let snapshot = RecoverySnapshot {
            schema_version: 1,
            timestamp: 1,
            payload: payload.clone(),
        };
        assert_eq!(
            update_recovery_payload(Some(snapshot)).unwrap(),
            Some(payload)
        );
    }
}
