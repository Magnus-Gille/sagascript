//! Token-scoped playback of an explicitly selected, source-verified recording.
use std::sync::{Arc, Mutex};

use sagascript_core::meeting_media::MeetingAudio;
use serde::Serialize;
use tauri::{http, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::meeting_media_range::plan_range;
use crate::meeting_review_commands::require_settings;

struct Attachment {
    token: String,
    audio: MeetingAudio,
}

#[derive(Default)]
struct AudioSlot {
    pending: Option<String>,
    current: Option<Attachment>,
}

#[derive(Default, Clone)]
pub struct SharedMeetingAudio(Arc<Mutex<AudioSlot>>);

#[derive(Serialize)]
pub struct AudioAttachment {
    token: String,
    mime: &'static str,
}

#[tauri::command]
pub async fn attach_meeting_audio(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    audio: State<'_, SharedMeetingAudio>,
    source_sha256: String,
) -> Result<Option<AudioAttachment>, String> {
    require_settings(&window)?;
    let state = audio.inner().clone();
    let request = uuid::Uuid::new_v4().to_string();
    state
        .0
        .lock()
        .map_err(|_| "Audio attachment lock failed")?
        .pending = Some(request.clone());
    tauri::async_runtime::spawn_blocking(move || {
        // Keep the previous descriptor and token intact until selection and
        // verification have both succeeded. Cancellation has no side effects.
        let result = (|| {
            let selected = app
                .dialog()
                .file()
                .set_title("Attach original recording for playback")
                .add_filter(
                    "Audio",
                    &["wav", "flac", "ogg", "opus", "m4a", "mp4", "mp3"],
                )
                .blocking_pick_file();
            let Some(selected) = selected else {
                return Ok(None);
            };
            let path = selected
                .into_path()
                .map_err(|e| format!("Choose a local recording: {e}"))?;
            MeetingAudio::open(&path, &source_sha256)
                .map(Some)
                .map_err(|e| e.to_string())
        })();
        let mut slot = state.0.lock().map_err(|_| "Audio attachment lock failed")?;
        if slot.pending.as_deref() != Some(&request) {
            return Err("A newer audio attachment request replaced this one.".into());
        }
        slot.pending = None;
        let Some(audio) = result? else {
            return Ok(None);
        };
        let mime = audio.mime();
        let token = uuid::Uuid::new_v4().to_string();
        slot.current = Some(Attachment {
            token: token.clone(),
            audio,
        });
        Ok(Some(AudioAttachment { token, mime }))
    })
    .await
    .map_err(|e| format!("Audio attachment worker failed: {e}"))?
}

#[tauri::command]
pub fn detach_meeting_audio(
    window: tauri::WebviewWindow,
    audio: State<'_, SharedMeetingAudio>,
    token: String,
) -> Result<(), String> {
    require_settings(&window)?;
    let mut slot = audio.0.lock().map_err(|_| "Audio attachment lock failed")?;
    if slot.current.as_ref().is_some_and(|a| a.token == token) {
        slot.current = None;
        slot.pending = None;
    }
    Ok(())
}

fn empty_response(status: u16) -> http::Response<Vec<u8>> {
    let mut response = http::Response::new(Vec::new());
    *response.status_mut() =
        http::StatusCode::from_u16(status).unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR);
    response
        .headers_mut()
        .insert("cache-control", http::HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        http::HeaderValue::from_static("nosniff"),
    );
    response
}

fn response_for(
    state: &SharedMeetingAudio,
    request: &http::Request<Vec<u8>>,
) -> http::Response<Vec<u8>> {
    if request.method() != http::Method::GET && request.method() != http::Method::HEAD {
        let mut response = empty_response(405);
        response
            .headers_mut()
            .insert("allow", http::HeaderValue::from_static("GET, HEAD"));
        return response;
    }
    let token = request.uri().path().strip_prefix('/').unwrap_or("");
    if request.uri().query().is_some() || uuid::Uuid::parse_str(token).is_err() || token.len() != 36
    {
        return empty_response(404);
    }
    let Ok(mut slot) = state.0.lock() else {
        return empty_response(500);
    };
    let Some(attachment) = slot.current.as_mut().filter(|a| a.token == token) else {
        return empty_response(404);
    };
    if attachment.audio.check_unchanged().is_err() {
        return empty_response(410);
    }
    let total = attachment.audio.len();
    // HEAD reports the complete resource without allocating a body. GET is
    // bounded and requires browser byte-range requests for large recordings.
    if request.method() == http::Method::HEAD {
        return http::Response::builder()
            .status(200)
            .header("content-type", attachment.audio.mime())
            .header("content-length", total.to_string())
            .header("accept-ranges", "bytes")
            .header("cache-control", "no-store")
            .header("x-content-type-options", "nosniff")
            .body(Vec::new())
            .unwrap_or_else(|_| empty_response(500));
    }
    let range = match request.headers().get("range") {
        Some(value) => match value.to_str() {
            Ok(value) => Some(value),
            Err(_) => return empty_response(416),
        },
        None => None,
    };
    let Ok(range) = plan_range(range, total) else {
        return http::Response::builder()
            .status(416)
            .header("content-range", format!("bytes */{total}"))
            .header("accept-ranges", "bytes")
            .header("cache-control", "no-store")
            .header("x-content-type-options", "nosniff")
            .body(Vec::new())
            .unwrap_or_else(|_| empty_response(500));
    };
    let body = match attachment.audio.read_range(range.start, range.length) {
        Ok(body) => body,
        Err(_) => return empty_response(410),
    };
    let mut response = http::Response::builder()
        .status(if range.partial { 206 } else { 200 })
        .header("content-type", attachment.audio.mime())
        .header("content-length", range.length.to_string())
        .header("accept-ranges", "bytes")
        .header("cache-control", "no-store")
        .header("x-content-type-options", "nosniff");
    if range.partial {
        response = response.header(
            "content-range",
            format!(
                "bytes {}-{}/{total}",
                range.start,
                range.start + range.length - 1
            ),
        );
    }
    response.body(body).unwrap_or_else(|_| empty_response(500))
}

pub fn protocol(
    context: tauri::UriSchemeContext<'_, tauri::Wry>,
    request: http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    if context.webview_label() != "settings" {
        responder.respond(empty_response(403));
        return;
    }
    let Some(state) = context.app_handle().try_state::<SharedMeetingAudio>() else {
        responder.respond(empty_response(500));
        return;
    };
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        responder.respond(response_for(&state, &request));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::File, io::Write, path::PathBuf};

    struct TestFile {
        path: PathBuf,
        file: File,
    }

    impl TestFile {
        fn new(bytes: &[u8]) -> Self {
            let path = std::env::temp_dir().join(format!(
                "sagascript-media-protocol-{}",
                uuid::Uuid::new_v4()
            ));
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .unwrap();
            file.write_all(bytes).unwrap();
            Self { path, file }
        }

        fn path(&self) -> &std::path::Path {
            &self.path
        }
    }

    impl std::ops::Deref for TestFile {
        type Target = File;

        fn deref(&self) -> &File {
            &self.file
        }
    }

    impl std::ops::DerefMut for TestFile {
        fn deref_mut(&mut self) -> &mut File {
            &mut self.file
        }
    }

    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn fixture() -> (TestFile, Vec<u8>, SharedMeetingAudio, String) {
        let bytes = b"RIFF1234WAVEsynthetic".to_vec();
        let file = TestFile::new(&bytes);
        let source_sha256 = "f2c86d8e2d98851067257b02707870c8d6a86f945da127d5e21aa3d831100468";
        let audio = MeetingAudio::open(file.path(), source_sha256).unwrap();
        let token = uuid::Uuid::new_v4().to_string();
        let state = SharedMeetingAudio::default();
        state.0.lock().unwrap().current = Some(Attachment {
            token: token.clone(),
            audio,
        });
        (file, bytes, state, token)
    }

    #[test]
    fn source_bound_audio_supports_head_range_and_revocation() {
        let (mut file, bytes, state, token) = fixture();
        let request = http::Request::builder()
            .uri(format!("/{token}"))
            .header("range", "bytes=8-11")
            .body(Vec::new())
            .unwrap();
        let response = response_for(&state, &request);
        assert_eq!(response.status(), 206);
        assert_eq!(response.headers()["content-type"], "audio/wav");
        assert_eq!(
            response.headers()["content-range"],
            format!("bytes 8-11/{}", bytes.len())
        );
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.body(), b"WAVE");
        let head = http::Request::builder()
            .method("HEAD")
            .uri(format!("/{token}"))
            .body(Vec::new())
            .unwrap();
        let response = response_for(&state, &head);
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers()["content-length"],
            bytes.len().to_string()
        );
        assert!(response.body().is_empty());
        let bad = http::Request::builder()
            .uri(format!("/{token}"))
            .header("range", "bytes=99-")
            .body(Vec::new())
            .unwrap();
        assert_eq!(response_for(&state, &bad).status(), 416);
        file.write_all(b"changed").unwrap();
        assert_eq!(response_for(&state, &request).status(), 410);
        state.0.lock().unwrap().current = None;
        assert_eq!(response_for(&state, &request).status(), 404);
    }

    #[test]
    fn full_get_returns_authorized_audio_with_protocol_headers_and_body() {
        let (_file, bytes, state, token) = fixture();
        let request = http::Request::builder()
            .uri(format!("/{token}"))
            .body(Vec::new())
            .unwrap();
        let response = response_for(&state, &request);
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "audio/wav");
        assert_eq!(
            response.headers()["content-length"],
            bytes.len().to_string()
        );
        assert_eq!(response.headers()["accept-ranges"], "bytes");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert!(response.headers().get("content-range").is_none());
        assert_eq!(response.body(), &bytes);
    }

    #[test]
    fn suffix_and_open_ended_ranges_are_bounded_and_exact() {
        let (_file, bytes, state, token) = fixture();
        let suffix = http::Request::builder()
            .uri(format!("/{token}"))
            .header("range", "bytes=-4")
            .body(Vec::new())
            .unwrap();
        let response = response_for(&state, &suffix);
        assert_eq!(response.status(), 206);
        assert_eq!(response.headers()["content-range"], "bytes 17-20/21");
        assert_eq!(response.headers()["content-length"], "4");
        assert_eq!(response.body(), &bytes[bytes.len() - 4..]);

        let open_ended = http::Request::builder()
            .uri(format!("/{token}"))
            .header("range", "bytes=8-")
            .body(Vec::new())
            .unwrap();
        let response = response_for(&state, &open_ended);
        assert_eq!(response.status(), 206);
        assert_eq!(
            response.headers()["content-range"],
            format!("bytes 8-{}/{}", bytes.len() - 1, bytes.len())
        );
        assert_eq!(
            response.headers()["content-length"],
            (bytes.len() - 8).to_string()
        );
        assert_eq!(response.body(), &bytes[8..]);
    }

    #[test]
    fn malformed_ranges_return_416_with_unsatisfied_content_range() {
        let (_file, bytes, state, token) = fixture();
        for range in ["bytes=", "bytes=-0", "bytes=0-1,3-4", "bytes=99-"] {
            let request = http::Request::builder()
                .uri(format!("/{token}"))
                .header("range", range)
                .body(Vec::new())
                .unwrap();
            let response = response_for(&state, &request);
            assert_eq!(response.status(), 416, "{range}");
            assert_eq!(
                response.headers()["content-range"],
                format!("bytes */{}", bytes.len())
            );
            assert_eq!(response.headers()["accept-ranges"], "bytes");
            assert!(response.body().is_empty());
        }
    }

    #[test]
    fn wrong_and_revoked_tokens_never_return_audio() {
        let (_file, bytes, state, token) = fixture();
        let wrong_token = uuid::Uuid::new_v4();
        let wrong = http::Request::builder()
            .uri(format!("/{wrong_token}"))
            .body(Vec::new())
            .unwrap();
        assert_eq!(response_for(&state, &wrong).status(), 404);

        let known = http::Request::builder()
            .uri(format!("/{token}"))
            .body(Vec::new())
            .unwrap();
        assert_eq!(response_for(&state, &known).status(), 200);
        assert_eq!(response_for(&state, &known).body(), &bytes);

        state.0.lock().unwrap().current = None;
        assert_eq!(response_for(&state, &known).status(), 404);
        assert_eq!(response_for(&state, &wrong).status(), 404);
    }

    #[test]
    fn missing_tokens_and_paths_never_resolve() {
        let state = SharedMeetingAudio::default();
        for path in [
            "/etc/passwd",
            "/../recording.wav",
            "/",
            "/%2f",
            "/123",
            "/00000000-0000-0000-0000-000000000000",
            "/00000000-0000-0000-0000-000000000000?file=x",
        ] {
            let request = http::Request::builder().uri(path).body(Vec::new()).unwrap();
            assert_eq!(response_for(&state, &request).status(), 404);
        }
    }

    #[test]
    fn rejects_mutating_methods() {
        for method in ["POST", "PUT", "DELETE", "PATCH"] {
            let request = http::Request::builder()
                .method(method)
                .uri("/")
                .body(Vec::new())
                .unwrap();
            let response = response_for(&SharedMeetingAudio::default(), &request);
            assert_eq!(response.status(), 405);
            assert_eq!(response.headers()["allow"], "GET, HEAD");
        }
    }
}
