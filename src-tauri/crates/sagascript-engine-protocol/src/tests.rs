use super::*;
use serde_json::json;

#[test]
fn request_round_trip_all_ops() {
    let ops = vec![
        RequestOp::Hello {
            client: ClientInfo {
                name: "sagascript".into(),
                version: "1.4.0".into(),
                git_sha: "abc".into(),
            },
        },
        RequestOp::Load {
            model_dir: "/m".into(),
            model_id: "id".into(),
            compute_units: "ane".into(),
        },
        RequestOp::TranscribeWindow {
            pcm_path: "/p".into(),
            offset_samples: 5,
            num_samples: 480000,
            sample_rate: 16000,
            format: "f32le".into(),
            priority: Priority::Batch,
        },
        RequestOp::Cancel { target: 7 },
        RequestOp::Status,
        RequestOp::Ping,
        RequestOp::Unload,
        RequestOp::Shutdown,
    ];
    for (i, op) in ops.into_iter().enumerate() {
        let id = i as u64 + 1;
        let line = encode_request(id, &op);
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["op"], op.name());
        assert_eq!(decode_request(&line).unwrap(), ParsedRequest::Ok { id, op });
    }
}

#[test]
fn request_wire_shape() {
    let line = encode_request(
        3,
        &RequestOp::TranscribeWindow {
            pcm_path: "/p".into(),
            offset_samples: 0,
            num_samples: 10,
            sample_rate: 16000,
            format: "f32le".into(),
            priority: Priority::Interactive,
        },
    );
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["priority"], "interactive");
    assert_eq!(v["id"], 3);
}

#[test]
fn unknown_op_and_bad_params() {
    assert_eq!(
        decode_request(r#"{"v":1,"id":4,"op":"frobnicate","x":1}"#).unwrap(),
        ParsedRequest::UnknownOp {
            id: 4,
            op: "frobnicate".into()
        }
    );
    assert!(matches!(
        decode_request(r#"{"v":1,"id":5,"op":"cancel"}"#).unwrap(),
        ParsedRequest::BadParams { id: 5, .. }
    ));
    assert!(decode_request("not json").is_err());
    assert!(decode_request(r#"{"v":1,"op":"ping"}"#).is_err());
    assert!(decode_request(r#"{"v":1,"id":0,"op":"ping"}"#).is_err());
}

#[test]
fn unknown_request_fields_ignored() {
    let p = decode_request(r#"{"v":1,"id":2,"op":"ping","future":true}"#).unwrap();
    assert_eq!(
        p,
        ParsedRequest::Ok {
            id: 2,
            op: RequestOp::Ping
        }
    );
}

#[test]
fn hello_response_parses_with_unknown_fields() {
    let line = r#"{"id":1,"ok":true,"protocol":1,"newthing":9,
      "host":{"name":"h","version":"1","git_sha":"deadbeef","engine":"coreml","engine_version":"x","extra":1},
      "capabilities":{"sample_rate":16000,"max_window_s":30.0,"preferred_window_s":30.0,
      "preferred_overlap_s":6.0,"max_in_flight":2,"token_timestamps":true,"languages":["sv"],
      "compute_units":["ane"],"min_macos":"14.0","other":[]}}"#;
    let Incoming::Response { id, result } = decode_incoming(line).unwrap() else {
        panic!()
    };
    assert_eq!(id, 1);
    let hello: HelloResult = parse_result(result.unwrap()).unwrap();
    assert_eq!(hello.protocol, 1);
    assert_eq!(hello.host.git_sha.as_deref(), Some("deadbeef"));
    assert_eq!(hello.capabilities.max_in_flight, 2);
}

#[test]
fn hello_without_git_sha_parses_as_none() {
    let v = json!({"protocol":1,"host":{"name":"h"},"capabilities":{"sample_rate":16000,
        "max_window_s":30.0,"preferred_window_s":30.0,"preferred_overlap_s":6.0,"max_in_flight":1}});
    let hello: HelloResult = parse_result(v).unwrap();
    assert!(hello.host.git_sha.is_none());
}

#[test]
fn transcribe_result_parses() {
    let line = r#"{"id":9,"ok":true,"tokens":[{"id":123,"text":"▁hej","start":0.24,"duration":0.16,"confidence":0.98},
        {"id":5,"text":"a","start":0.4,"duration":0.08}],"audio_s":30.0,
        "timings":{"preprocess_ms":4,"encode_ms":55,"decode_ms":20}}"#;
    let Incoming::Response { result, .. } = decode_incoming(line).unwrap() else {
        panic!()
    };
    let r: TranscribeWindowResult = parse_result(result.unwrap()).unwrap();
    assert_eq!(r.tokens.len(), 2);
    assert_eq!(r.tokens[0].confidence, Some(0.98));
    assert_eq!(r.tokens[1].confidence, None);
    assert_eq!(r.timings.encode_ms, 55);
}

#[test]
fn error_codes_parse_including_unknown() {
    for (s, c) in [
        ("protocol", ErrorCode::Protocol),
        ("bad_request", ErrorCode::BadRequest),
        ("unsupported", ErrorCode::Unsupported),
        ("not_loaded", ErrorCode::NotLoaded),
        ("model_missing", ErrorCode::ModelMissing),
        ("model_load_failed", ErrorCode::ModelLoadFailed),
        ("busy", ErrorCode::Busy),
        ("cancelled", ErrorCode::Cancelled),
        ("engine", ErrorCode::Engine),
        ("internal", ErrorCode::Internal),
    ] {
        assert_eq!(ErrorCode::parse(s), c);
        assert_eq!(c.as_str(), s);
    }
    let line = r#"{"id":2,"ok":false,"error":{"code":"quantum_flux","message":"m","retryable":true,"more":1}}"#;
    let Incoming::Response { result, .. } = decode_incoming(line).unwrap() else {
        panic!()
    };
    let e = result.unwrap_err();
    assert_eq!(e.code, ErrorCode::Unknown("quantum_flux".into()));
    assert!(e.retryable);
    assert_eq!(e.message, "m");
}

#[test]
fn error_round_trip_via_encoder() {
    let line = encode_err(8, ErrorCode::Busy, "queue full", true);
    let Incoming::Response { id, result } = decode_incoming(&line).unwrap() else {
        panic!()
    };
    assert_eq!(id, 8);
    let e = result.unwrap_err();
    assert_eq!(e.code, ErrorCode::Busy);
    assert!(e.retryable);
}

#[test]
fn events_and_ok_round_trip() {
    let line = encode_event(3, "load_progress", json!({"phase":"compiling"}));
    match decode_incoming(&line).unwrap() {
        Incoming::Event { id, name, fields } => {
            assert_eq!((id, name.as_str()), (3, "load_progress"));
            assert_eq!(fields["phase"], "compiling");
        }
        other => panic!("{other:?}"),
    }
    let line = encode_ok(4, json!({}));
    assert_eq!(
        decode_incoming(&line).unwrap(),
        Incoming::Response {
            id: 4,
            result: Ok(json!({}))
        }
    );
}

#[test]
fn malformed_incoming_rejected() {
    assert!(decode_incoming("garbage").is_err());
    assert!(decode_incoming("[1]").is_err());
    assert!(decode_incoming(r#"{"ok":true}"#).is_err());
    assert!(decode_incoming(r#"{"id":1}"#).is_err());
}


#[test]
fn millisecond_fields_accept_float_encoders() {
    let load: LoadResult =
        serde_json::from_str(r#"{"model_id":"m","load_ms":95520.4529762268,"compiled":true}"#).unwrap();
    assert_eq!(load.load_ms, 95520);
    let timings: WindowTimings =
        serde_json::from_str(r#"{"preprocess_ms":1.9,"encode_ms":30,"decode_ms":24.5}"#).unwrap();
    assert_eq!((timings.preprocess_ms, timings.encode_ms, timings.decode_ms), (2, 30, 25));
    assert!(serde_json::from_str::<WindowTimings>(r#"{"encode_ms":-1}"#).is_err());
    assert!(serde_json::from_str::<WindowTimings>(r#"{"encode_ms":"30"}"#).is_err());
}
