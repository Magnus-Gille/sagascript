use super::*;

#[cfg(test)]
mod recovery_payload_tests {
    use super::update_recovery_payload;
    use crate::update_recovery::RecoverySnapshot;
    use serde_json::{json, Value};

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
