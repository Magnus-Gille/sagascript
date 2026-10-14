use super::*;
use sagascript_testsupport::TempDir;

const TEST_SHA: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn integrity(size: u64) -> DownloadIntegrity {
    DownloadIntegrity {
        sha256: TEST_SHA,
        size,
    }
}

// -- validate_download --

#[test]
fn accepts_valid_ggml_prefix_with_matching_length() {
    let prefix = [0x6c, 0x6d, 0x67, 0x67, 0x0a, 0x00, 0x00, 0x00];
    assert!(validate_download(
        &prefix,
        142_000_000,
        Some(142_000_000),
        TEST_SHA,
        integrity(142_000_000),
        Some(&GGML_MAGIC),
    )
    .is_ok());
}

#[test]
fn rejects_html_rate_limit_page() {
    let prefix = b"<!DOCTYPE html><html><head><title>429</title>";
    let err = validate_download(
        prefix,
        prefix.len() as u64,
        Some(prefix.len() as u64),
        TEST_SHA,
        integrity(prefix.len() as u64),
        Some(&GGML_MAGIC),
    )
    .unwrap_err();
    assert!(
        err.contains("magic"),
        "error should mention magic bytes: {err}"
    );
}

#[test]
fn rejects_git_lfs_pointer_file() {
    let prefix =
        b"version https://git-lfs.github.com/spec/v1\noid sha256:deadbeef\nsize 142000000\n";
    let err = validate_download(
        prefix,
        prefix.len() as u64,
        None,
        TEST_SHA,
        integrity(prefix.len() as u64),
        Some(&GGML_MAGIC),
    )
    .unwrap_err();
    assert!(
        err.contains("magic"),
        "error should mention magic bytes: {err}"
    );
}

#[test]
fn rejects_length_mismatch_even_without_magic_check() {
    // Simulates a dropped connection: server promised 1000 bytes, only 400 arrived.
    let prefix = b"partial-body-bytes";
    let err =
        validate_download(prefix, 400, Some(1000), TEST_SHA, integrity(400), None).unwrap_err();
    assert!(
        err.contains("400") && err.contains("1000"),
        "error should name both byte counts: {err}"
    );
}

#[test]
fn skips_length_check_when_content_length_unknown_or_zero() {
    let prefix = GGML_MAGIC;
    assert!(validate_download(
        &prefix,
        999,
        None,
        TEST_SHA,
        integrity(999),
        Some(&GGML_MAGIC)
    )
    .is_ok());
    assert!(validate_download(
        &prefix,
        999,
        Some(0),
        TEST_SHA,
        integrity(999),
        Some(&GGML_MAGIC),
    )
    .is_ok());
}

#[test]
fn skips_magic_check_when_none_e_g_onnx() {
    // ONNX files are protobuf-encoded with no fixed magic; only length is checked.
    let onnx_like_prefix = [0x08, 0x07, 0x12, 0x07];
    assert!(validate_download(
        &onnx_like_prefix,
        27_000_000,
        Some(27_000_000),
        TEST_SHA,
        integrity(27_000_000),
        None,
    )
    .is_ok());
}

#[test]
fn rejects_exact_size_mismatch_even_when_server_length_matches() {
    let err =
        validate_download(b"model", 512, Some(512), TEST_SHA, integrity(1024), None).unwrap_err();
    assert!(err.contains("exactly 1024"), "error: {err}");
}

#[test]
fn rejects_sha256_mismatch() {
    let err = validate_download(
        b"model",
        512,
        Some(512),
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        integrity(512),
        None,
    )
    .unwrap_err();
    assert!(err.contains("SHA-256 mismatch"), "error: {err}");
}

#[test]
fn verify_file_hashes_existing_artifact_before_use() {
    let dir = TempDir::new();
    let path = dir.join("model.bin");
    std::fs::write(&path, b"verified model").unwrap();
    let expected = DownloadIntegrity {
        sha256: "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d",
        size: 14,
    };
    assert!(verify_file(&path, expected).is_ok());
    std::fs::write(&path, b"tampered model").unwrap();
    assert!(verify_file(&path, expected).is_err());
}

// -- persistent verification stamp (unix only) --

#[cfg(unix)]
mod stamp {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const BODY: &[u8] = b"verified model";
    const SHA: &str = "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d";
    const INTEGRITY: DownloadIntegrity = DownloadIntegrity {
        sha256: SHA,
        size: 14,
    };

    /// Run one verification with a counting hasher; returns hash count.
    fn run(path: &Path, integrity: DownloadIntegrity, force: bool) -> (usize, bool) {
        let count = AtomicUsize::new(0);
        let hasher = |file, p: &Path| {
            count.fetch_add(1, Ordering::SeqCst);
            sha256_of_file(file, p)
        };
        let ok = verify_file_detailed_with(path, integrity, force, &hasher).is_ok();
        (count.load(Ordering::SeqCst), ok)
    }

    /// Fresh dir + model file. Each test uses its own path, so the
    /// process-wide cache (keyed by path) never leaks between tests; to
    /// exercise the *stamp* we clear that cache entry before each run.
    fn setup() -> (TempDir, PathBuf) {
        let dir = TempDir::new();
        let path = dir.join("model.bin");
        std::fs::write(&path, BODY).unwrap();
        (dir, path)
    }

    fn forget_process_cache(path: &Path) {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        verification_cache().lock().unwrap().remove(&key);
    }

    fn stamped(path: &Path) -> (PathBuf, PathBuf) {
        let (count, ok) = run(path, INTEGRITY, false);
        assert!((count, ok) == (1, true));
        forget_process_cache(path);
        (path.to_path_buf(), verification_stamp_path(path))
    }

    #[test]
    fn first_verify_hashes_and_writes_stamp_then_skips() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, path) = setup();
        assert_eq!(run(&path, INTEGRITY, false), (1, true));
        let stamp_path = verification_stamp_path(&path);
        let stamp: VerificationStamp =
            serde_json::from_slice(&std::fs::read(&stamp_path).unwrap()).unwrap();
        assert_eq!(stamp.schema, 1);
        assert_eq!(stamp.sha256, SHA);
        assert_eq!(stamp.size, 14);
        let mode = std::fs::metadata(&stamp_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        // Same process: in-process cache. New "process": stamp only.
        assert_eq!(run(&path, INTEGRITY, false), (0, true));
        forget_process_cache(&path);
        assert_eq!(run(&path, INTEGRITY, false), (0, true));
        // Forced verification always hashes.
        assert_eq!(run(&path, INTEGRITY, true), (1, true));
        assert!(verify_file_forced(&path, INTEGRITY).is_ok());
    }

    #[test]
    fn touched_mtime_rehashes() {
        let (_dir, path) = setup();
        stamped(&path);
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        drop(file);
        assert_eq!(run(&path, INTEGRITY, false), (1, true));
        forget_process_cache(&path);
        assert_eq!(
            run(&path, INTEGRITY, false),
            (0, true),
            "stamp was rewritten"
        );
    }

    #[test]
    fn changed_size_rehashes_and_fails() {
        let (_dir, path) = setup();
        stamped(&path);
        std::fs::write(&path, b"longer verified model").unwrap();
        // Size mismatch is rejected before hashing.
        assert_eq!(run(&path, INTEGRITY, false), (0, false));
    }

    #[test]
    fn replaced_file_new_inode_rehashes() {
        let (dir, path) = setup();
        stamped(&path);
        let replacement = dir.join("other.bin");
        std::fs::write(&replacement, BODY).unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert_eq!(run(&path, INTEGRITY, false), (1, true));
    }

    #[test]
    fn same_size_tampering_is_caught() {
        let (_dir, path) = setup();
        stamped(&path);
        std::fs::write(&path, b"tampered model").unwrap();
        assert_eq!(run(&path, INTEGRITY, false), (1, false));
        assert!(
            !verification_stamp_path(&path).exists(),
            "stale stamp removed"
        );
    }

    #[test]
    fn stamp_for_a_different_expected_sha_is_ignored() {
        let (_dir, path) = setup();
        stamped(&path);
        let other = DownloadIntegrity {
            sha256: "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            size: 14,
        };
        assert_eq!(run(&path, other, false), (1, false));
    }

    #[test]
    fn corrupt_stamp_rehashes_and_is_rewritten() {
        let (_dir, path) = setup();
        let (_, stamp_path) = stamped(&path);
        std::fs::write(&stamp_path, b"{not json").unwrap();
        assert_eq!(run(&path, INTEGRITY, false), (1, true));
        forget_process_cache(&path);
        assert_eq!(run(&path, INTEGRITY, false), (0, true));
    }

    #[test]
    fn unwritable_directory_does_not_fail_verification() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, path) = setup();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = run(&path, INTEGRITY, false);
        let wrote = verification_stamp_path(&path).exists();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(result, (1, true));
        // (Running as root would bypass the mode bits.)
        assert!(!wrote || unsafe_is_root());
    }

    fn unsafe_is_root() -> bool {
        std::env::var("USER").is_ok_and(|user| user == "root")
    }

    #[test]
    fn concurrent_verifiers_leave_a_valid_stamp() {
        let (dir, path) = setup();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    // Bypass the shared in-process cache so all race on the stamp.
                    verify_file_forced(&path, INTEGRITY).is_ok()
                })
            })
            .collect();
        for handle in handles {
            assert!(handle.join().unwrap());
        }
        forget_process_cache(&path);
        assert_eq!(run(&path, INTEGRITY, false), (0, true));
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn removing_a_stamp_is_idempotent() {
        let (_dir, path) = setup();
        stamped(&path);
        remove_verification_stamp(&path);
        remove_verification_stamp(&path);
        assert!(!verification_stamp_path(&path).exists());
    }
}

#[test]
fn verify_file_works_on_a_small_stack() {
    let dir = TempDir::new();
    let path = dir.join("model.bin");
    std::fs::write(&path, b"verified model").unwrap();
    let expected = DownloadIntegrity {
        sha256: "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d",
        size: 14,
    };

    // This is intentionally below the verifier's former 1 MiB stack
    // buffer. A regression therefore fails on Windows with a stack
    // overflow instead of depending on the host process stack size.
    let result = std::thread::Builder::new()
        .name("sagascript-small-stack-verification".to_string())
        .stack_size(256 * 1024)
        .spawn({
            let path = path.clone();
            move || verify_file(&path, expected)
        })
        .unwrap()
        .join()
        .unwrap();

    assert!(result.is_ok());
}

#[test]
fn prepare_existing_artifact_keeps_verified_file() {
    let dir = TempDir::new();
    let path = dir.join("model.bin");
    std::fs::write(&path, b"verified model").unwrap();
    let expected = DownloadIntegrity {
        sha256: "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d",
        size: 14,
    };

    assert_eq!(
        prepare_existing_artifact(&path, expected).unwrap(),
        ExistingArtifact::Verified
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"verified model");
}

#[test]
fn prepare_existing_artifact_removes_same_size_hash_mismatch() {
    let dir = TempDir::new();
    let path = dir.join("model.bin");
    std::fs::write(&path, b"tampered model").unwrap();
    let expected = DownloadIntegrity {
        sha256: "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d",
        size: 14,
    };

    assert_eq!(
        prepare_existing_artifact(&path, expected).unwrap(),
        ExistingArtifact::RemovedInvalid
    );
    assert!(!path.exists());
}

#[test]
fn prepare_existing_artifact_preserves_file_for_invalid_manifest() {
    let dir = TempDir::new();
    let path = dir.join("model.bin");
    std::fs::write(&path, b"untrusted bytes").unwrap();
    let invalid_manifest = DownloadIntegrity {
        sha256: "not-a-sha256",
        size: 1,
    };

    assert!(prepare_existing_artifact(&path, invalid_manifest).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"untrusted bytes");
}

#[test]
fn prepare_existing_artifact_preserves_path_on_io_error() {
    let dir = TempDir::new();
    let path = dir.join("model.bin");
    std::fs::create_dir_all(&path).unwrap();
    let expected = DownloadIntegrity {
        sha256: TEST_SHA,
        size: std::fs::metadata(&path).unwrap().len(),
    };

    assert!(prepare_existing_artifact(&path, expected).is_err());
    assert!(
        path.is_dir(),
        "generic I/O failure must not delete the path"
    );
}

#[cfg(unix)]
#[test]
fn prepare_existing_artifact_preserves_dangling_symlink() {
    use std::os::unix::fs::symlink;

    let dir = TempDir::new();
    let path = dir.join("model.bin");
    symlink(dir.join("missing-target"), &path).unwrap();

    assert!(prepare_existing_artifact(&path, integrity(1)).is_err());
    assert!(
        std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink(),
        "an open failure must not remove or replace the existing path"
    );
}

fn serve_once(body: &'static [u8]) -> String {
    let (url, _handle) = serve_scripts(vec![vec![
        ok_head(Some(body.len())),
        Step::Write(body.to_vec()),
    ]]);
    url
}

#[tokio::test]
async fn invalid_existing_artifact_is_replaced_by_verified_download() {
    let dir = TempDir::new();
    let path = dir.join("model.bin");
    std::fs::write(&path, b"tampered model").unwrap();
    let expected = DownloadIntegrity {
        sha256: "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d",
        size: 14,
    };

    assert_eq!(
        prepare_existing_artifact(&path, expected).unwrap(),
        ExistingArtifact::RemovedInvalid
    );
    download_to_path(
        &serve_once(b"verified model"),
        &path,
        "bin",
        expected,
        None,
        |_, _| {},
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read(&path).unwrap(), b"verified model");
}

#[tokio::test]
async fn failed_replacement_never_installs_partial_artifact() {
    let dir = TempDir::new();
    let path = dir.join("model.bin");
    std::fs::write(&path, b"tampered model").unwrap();
    let expected = DownloadIntegrity {
        sha256: "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d",
        size: 14,
    };

    prepare_existing_artifact(&path, expected).unwrap();
    assert!(download_to_path(
        &serve_once(b"partial"),
        &path,
        "bin",
        expected,
        None,
        |_, _| {},
    )
    .await
    .is_err());
    assert!(!path.exists());
    assert!(
        std::fs::read_dir(&dir).unwrap().next().is_none(),
        "failed replacement must clean its unique temp file"
    );
}

// -- bounded downloads --

const GOOD_SHA: &str = "6c736b3dfa943bf4e7c61df78d1dfcad9a3d8b56369f0559670497b19127e74d";

enum Step {
    Write(Vec<u8>),
    Sleep(Duration),
}

fn ok_head(content_length: Option<usize>) -> Step {
    let mut head = String::from("HTTP/1.1 200 OK\r\nConnection: close\r\n");
    if let Some(n) = content_length {
        head.push_str(&format!("Content-Length: {n}\r\n"));
    }
    head.push_str("\r\n");
    Step::Write(head.into_bytes())
}

fn redirect_head() -> Step {
    Step::Write(
        b"HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            .to_vec(),
    )
}

/// Serves one scripted connection per entry, in order. The handle yields
/// the server-side write errors so success-path tests can assert none.
fn serve_scripts(
    scripts: Vec<Vec<Step>>,
) -> (String, std::thread::JoinHandle<Vec<std::io::Error>>) {
    use std::io::{Read as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let mut errors = Vec::new();
        listener.set_nonblocking(true).unwrap();
        // Never wait forever for a client: give up after a fixed budget so
        // the thread always exits on its own.
        let give_up = std::time::Instant::now() + Duration::from_secs(8);
        for script in scripts {
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() > give_up {
                            return errors;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => {
                        errors.push(e);
                        return errors;
                    }
                }
            };
            stream.set_nonblocking(false).unwrap();
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request);
            for step in script {
                match step {
                    Step::Write(bytes) => {
                        if let Err(e) = stream.write_all(&bytes).and_then(|_| stream.flush()) {
                            errors.push(e);
                            break;
                        }
                    }
                    Step::Sleep(d) => std::thread::sleep(d),
                }
            }
        }
        errors
    });
    (format!("http://{address}/model.bin"), handle)
}

/// Joins a server thread without blocking the async runtime (a blocked
/// current-thread runtime cannot close the client socket, which can
/// deadlock a server mid-write). The thread also self-terminates.
async fn join_server(server: std::thread::JoinHandle<Vec<std::io::Error>>) -> Vec<std::io::Error> {
    tokio::time::timeout(
        Duration::from_secs(15),
        tokio::task::spawn_blocking(move || server.join().unwrap()),
    )
    .await
    .expect("server thread did not exit")
    .unwrap()
}

fn limits(idle_ms: u64, total_base_ms: u64) -> DownloadLimits {
    DownloadLimits {
        connect_timeout: Duration::from_secs(2),
        idle_timeout: Duration::from_millis(idle_ms),
        total_base: Duration::from_millis(total_base_ms),
        min_bytes_per_sec: u64::MAX,
    }
}

fn fast_limits() -> DownloadLimits {
    limits(300, 20_000)
}

fn pinned(size: u64) -> DownloadIntegrity {
    DownloadIntegrity {
        sha256: GOOD_SHA,
        size,
    }
}

fn assert_empty(dir: &Path, why: &str) {
    assert!(std::fs::read_dir(dir).unwrap().next().is_none(), "{why}");
}

/// Runs a download of `size` pinned bytes against `scripts`, returning the
/// result, elapsed time and the (still existing) directory.
async fn run_scripts(
    scripts: Vec<Vec<Step>>,
    size: u64,
    limits: DownloadLimits,
) -> (
    Result<(), DictationError>,
    Duration,
    TempDir,
    Vec<std::io::Error>,
) {
    let dir = TempDir::new();
    let (url, server) = serve_scripts(scripts);
    let start = std::time::Instant::now();
    let result = download_to_path_with_limits(
        &url,
        &dir.join("model.bin"),
        "bin",
        pinned(size),
        None,
        limits,
        |_, _| {},
    )
    .await;
    let took = start.elapsed();
    (result, took, dir, join_server(server).await)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_body_without_length_fails_promptly_and_leaves_nothing() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let (r, took, dir, _) = run_scripts(
            vec![vec![
                ok_head(None),
                Step::Write(vec![b'a'; 4 * 1024 * 1024]),
                Step::Sleep(Duration::from_secs(3)),
            ]],
            14,
            fast_limits(),
        )
        .await;
        let err = r.unwrap_err().to_string();
        assert!(err.contains("exceeds the pinned manifest size"), "{err}");
        assert!(took < Duration::from_millis(2500), "{took:?}");
        assert_empty(&dir, "oversized download left a file");
    })
    .await
    .expect("test exceeded its hard timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cumulative_overflow_across_acceptable_chunks_is_rejected() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let part = || Step::Write(vec![b'a'; 8]);
        let pause = || Step::Sleep(Duration::from_millis(100));
        let (r, _, dir, _) = run_scripts(
            vec![vec![
                ok_head(None),
                part(),
                pause(),
                part(),
                pause(),
                part(),
            ]],
            14,
            fast_limits(),
        )
        .await;
        let err = r.unwrap_err().to_string();
        assert!(err.contains("exceeds the pinned manifest size"), "{err}");
        assert_empty(&dir, "overflowing download left a file");
    })
    .await
    .expect("test exceeded its hard timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_content_length_is_rejected_before_any_write() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let dir = TempDir::new();
        let (url, server) = serve_scripts(vec![vec![
            ok_head(Some(1_000_000)),
            Step::Write(vec![b'a'; 14]),
            Step::Sleep(Duration::from_secs(1)),
        ]]);
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = calls.clone();
        let err = download_to_path_with_limits(
            &url,
            &dir.join("model.bin"),
            "bin",
            pinned(14),
            None,
            fast_limits(),
            move |_, _| {
                seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("Server advertised 1000000"), "{err}");
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "no chunk may be processed"
        );
        assert_empty(&dir, "rejected download left a file");
        join_server(server).await;
    })
    .await
    .expect("test exceeded its hard timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_body_fails_within_idle_deadline_and_leaves_nothing() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let (r, took, dir, _) = run_scripts(
            vec![vec![
                ok_head(Some(14)),
                Step::Write(vec![b'a'; 5]),
                Step::Sleep(Duration::from_secs(3)),
            ]],
            14,
            fast_limits(),
        )
        .await;
        let err = r.unwrap_err().to_string();
        assert!(err.contains("stalled"), "{err}");
        assert!(took < Duration::from_millis(2500), "{took:?}");
        assert_empty(&dir, "stalled download left a file");
    })
    .await
    .expect("test exceeded its hard timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_headers_fail_within_idle_deadline() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let (r, took, dir, _) = run_scripts(
            vec![vec![Step::Sleep(Duration::from_secs(3))]],
            14,
            fast_limits(),
        )
        .await;
        assert!(r.unwrap_err().to_string().contains("stalled"));
        assert!(took < Duration::from_millis(2500), "{took:?}");
        assert_empty(&dir, "header stall left a file");
    })
    .await
    .expect("test exceeded its hard timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirect_chain_slower_than_idle_in_total_but_not_per_hop_succeeds() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let slow = || Step::Sleep(Duration::from_millis(400));
        let (r, took, dir, errors) = run_scripts(
            vec![
                vec![slow(), redirect_head()],
                vec![slow(), redirect_head()],
                vec![
                    slow(),
                    ok_head(Some(14)),
                    Step::Write(b"verified model".to_vec()),
                ],
            ],
            14,
            limits(1000, 20_000),
        )
        .await;
        r.unwrap();
        assert!(
            took > Duration::from_millis(1000),
            "chain must exceed one idle period: {took:?}"
        );
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            std::fs::read(dir.join("model.bin")).unwrap(),
            b"verified model"
        );
    })
    .await
    .expect("test exceeded its hard timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirect_loop_is_bounded() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let scripts = (0..=MAX_REDIRECTS).map(|_| vec![redirect_head()]).collect();
        let (r, _, dir, _) = run_scripts(scripts, 14, fast_limits()).await;
        assert!(r.unwrap_err().to_string().contains("Too many redirects"));
        assert_empty(&dir, "redirect loop left a file");
    })
    .await
    .expect("test exceeded its hard timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_pinned_download_succeeds_with_limits() {
    tokio::time::timeout(Duration::from_secs(30), async {
        for head in [ok_head(Some(14)), ok_head(None)] {
            let (r, _, dir, errors) = run_scripts(
                vec![vec![head, Step::Write(b"verified model".to_vec())]],
                14,
                fast_limits(),
            )
            .await;
            r.unwrap();
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(
                std::fs::read(dir.join("model.bin")).unwrap(),
                b"verified model"
            );
        }
    })
    .await
    .expect("test exceeded its hard timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chunked_download_without_content_length_succeeds() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let head = Step::Write(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec(),
        );
        let (r, _, dir, errors) = run_scripts(
            vec![vec![
                head,
                Step::Write(b"8\r\nverified\r\n".to_vec()),
                Step::Sleep(Duration::from_millis(50)),
                Step::Write(b"6\r\n model\r\n0\r\n\r\n".to_vec()),
            ]],
            14,
            fast_limits(),
        )
        .await;
        r.unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            std::fs::read(dir.join("model.bin")).unwrap(),
            b"verified model"
        );
    })
    .await
    .expect("test exceeded its hard timeout");
}

/// Callback that outlasts the total deadline while data is still arriving.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn total_deadline_expiring_during_processing_fails() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let dir = TempDir::new();
        let (url, server) = serve_scripts(vec![vec![
            ok_head(Some(14)),
            Step::Write(b"verified".to_vec()),
            Step::Sleep(Duration::from_millis(50)),
            Step::Write(b" model".to_vec()),
        ]]);
        let err = download_to_path_with_limits(
            &url,
            &dir.join("model.bin"),
            "bin",
            pinned(14),
            None,
            limits(2000, 300),
            |_, _| std::thread::sleep(Duration::from_millis(500)),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("total time limit"), "{err}");
        assert_empty(&dir, "expired download left a file");
        join_server(server).await;
    })
    .await
    .expect("test exceeded its hard timeout");
}

/// Deadline passes during the final chunk's callback; EOF is immediately
/// ready afterwards and must not be allowed to succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn total_deadline_expiring_just_before_eof_fails() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let dir = TempDir::new();
        let (url, server) = serve_scripts(vec![vec![
            ok_head(Some(14)),
            Step::Write(b"verified model".to_vec()),
        ]]);
        let err = download_to_path_with_limits(
            &url,
            &dir.join("model.bin"),
            "bin",
            pinned(14),
            None,
            limits(2000, 300),
            |_, _| std::thread::sleep(Duration::from_millis(500)),
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("total time limit"), "{err}");
        assert!(!dir.join("model.bin").exists());
        assert_empty(&dir, "expired download left a file");
        join_server(server).await;
    })
    .await
    .expect("test exceeded its hard timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aborting_the_download_task_removes_the_partial_file() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let dir = TempDir::new();
        let (url, server) = serve_scripts(vec![vec![
            ok_head(Some(14)),
            Step::Write(b"verified".to_vec()),
            Step::Sleep(Duration::from_secs(2)),
        ]]);
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let tx = std::sync::Mutex::new(tx);
        let path = dir.join("model.bin");
        let task = tokio::spawn(async move {
            download_to_path_with_limits(
                &url,
                &path,
                "bin",
                pinned(14),
                None,
                limits(10_000, 20_000),
                move |_, _| {
                    let _ = tx.lock().unwrap().send(());
                },
            )
            .await
        });
        // Wait (off the runtime thread) until a partial file has been written.
        tokio::task::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .await
            .unwrap();
        assert!(
            std::fs::read_dir(&dir).unwrap().next().is_some(),
            "partial file expected"
        );
        task.abort();
        let _ = task.await;
        assert_empty(&dir, "aborted download left a partial file");
        join_server(server).await;
    })
    .await
    .expect("test exceeded its hard timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn panicking_progress_callback_removes_the_partial_file() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let dir = TempDir::new();
        let (url, server) = serve_scripts(vec![vec![
            ok_head(Some(14)),
            Step::Write(b"verified".to_vec()),
            Step::Sleep(Duration::from_millis(300)),
        ]]);
        let path = dir.join("model.bin");
        let task = tokio::spawn(async move {
            download_to_path_with_limits(
                &url,
                &path,
                "bin",
                pinned(14),
                None,
                fast_limits(),
                |_, _| panic!("callback boom"),
            )
            .await
        });
        assert!(task.await.unwrap_err().is_panic());
        assert_empty(&dir, "panicking callback left a partial file");
        join_server(server).await;
    })
    .await
    .expect("test exceeded its hard timeout");
}

#[test]
fn redirect_downgrade_is_refused_and_https_upgrade_allowed() {
    let https = reqwest::Url::parse("https://huggingface.co/a/model.bin").unwrap();
    let err = redirect_target(&https, "http://cdn.example/model.bin").unwrap_err();
    assert!(
        err.contains("downgrade") && err.contains("cdn.example"),
        "{err}"
    );
    assert_eq!(
        redirect_target(&https, "/other").unwrap().as_str(),
        "https://huggingface.co/other"
    );
    let http = reqwest::Url::parse("http://127.0.0.1:1/a").unwrap();
    assert!(redirect_target(&http, "https://cdn.example/x").is_ok());
}

#[test]
fn only_standard_redirect_statuses_are_followed() {
    for ok in [301, 302, 303, 307, 308] {
        assert!(is_followed_redirect(
            reqwest::StatusCode::from_u16(ok).unwrap()
        ));
    }
    for bad in [300, 304, 305, 306, 310] {
        assert!(!is_followed_redirect(
            reqwest::StatusCode::from_u16(bad).unwrap()
        ));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_redirect_status_is_an_error() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let multiple = Step::Write(
            b"HTTP/1.1 300 Multiple Choices\r\nLocation: /next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                .to_vec(),
        );
        let (r, _, dir, _) = run_scripts(vec![vec![multiple]], 14, fast_limits()).await;
        let err = r.unwrap_err().to_string();
        assert!(err.contains("Unsupported redirect status 300"), "{err}");
        assert_empty(&dir, "unsupported redirect left a file");
    })
    .await
    .expect("test exceeded its hard timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirected_response_failing_integrity_leaves_no_file() {
    tokio::time::timeout(Duration::from_secs(30), async {
        // Right length, wrong SHA-256.
        let (r, _, dir, _) = run_scripts(
            vec![
                vec![redirect_head()],
                vec![ok_head(Some(14)), Step::Write(b"tampered model".to_vec())],
            ],
            14,
            fast_limits(),
        )
        .await;
        assert!(r.is_err());
        assert!(!dir.join("model.bin").exists());
        assert_empty(&dir, "integrity failure left a file");
    })
    .await
    .expect("test exceeded its hard timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_while_file_creation_is_queued_leaves_no_file() {
    tokio::time::timeout(Duration::from_secs(30), async {
        // Saturate the blocking pool is impractical; instead cancel right
        // after the response arrives, racing creation. Whatever the
        // interleaving, no file may remain once creation has settled.
        for _ in 0..20 {
            let dir = TempDir::new();
            let (url, server) = serve_scripts(vec![vec![
                ok_head(Some(14)),
                Step::Sleep(Duration::from_millis(300)),
            ]]);
            let path = dir.join("model.bin");
            let task = tokio::spawn(async move {
                download_to_path_with_limits(
                    &url,
                    &path,
                    "bin",
                    pinned(14),
                    None,
                    limits(5000, 20_000),
                    |_, _| {},
                )
                .await
            });
            tokio::time::sleep(Duration::from_millis(30)).await;
            task.abort();
            let _ = task.await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            assert_empty(&dir, "cancelled download left a file");
            join_server(server).await;
        }
    })
    .await
    .expect("test exceeded its hard timeout");
}

#[test]
fn total_deadline_scales_with_size() {
    let l = DownloadLimits::default();
    assert!(l.total_deadline(3 * 1024 * 1024 * 1024) > Duration::from_secs(6 * 3600));
    assert!(l.total_deadline(0) >= Duration::from_secs(300));
}

// -- unique_tmp_path --

#[test]
fn unique_tmp_path_is_unique_and_carries_marker() {
    let dest = Path::new("/models/ggml-base.bin");
    let a = unique_tmp_path(dest, "bin");
    let b = unique_tmp_path(dest, "bin");
    assert_ne!(a, b, "two calls must never collide");
    for p in [&a, &b] {
        assert_eq!(p.parent(), dest.parent());
        let name = p.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("ggml-base.bin."), "name: {name}");
        assert!(name.ends_with(".tmp"), "name: {name}");
    }
}

// -- sweep_orphaned_tmp_files --

#[test]
fn sweep_removes_only_stale_tmp_files() {
    let dir = TempDir::new();

    let stale = dir.join("stale.bin.tmp");
    std::fs::write(&stale, b"leftover").unwrap();
    // Backdate mtime well past the sweep threshold using std's stable
    // `File::set_times` (no extra dependency needed).
    let ancient = SystemTime::now() - (ORPHAN_TMP_MAX_AGE + Duration::from_secs(60));
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&stale)
        .unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(ancient))
        .unwrap();

    let fresh = dir.join("fresh.bin.tmp");
    std::fs::write(&fresh, b"in progress").unwrap();

    let keep = dir.join("keep.bin"); // not a .tmp — must never be touched
    std::fs::write(&keep, b"real model").unwrap();

    sweep_orphaned_tmp_files(&dir);

    assert!(!stale.exists(), "stale tmp file should be swept");
    assert!(
        fresh.exists(),
        "fresh tmp file must survive — it might belong to a concurrent download"
    );
    assert!(keep.exists(), "non-tmp file must never be touched");
}

#[test]
fn sweep_tolerates_missing_directory() {
    // Must not panic if the models dir doesn't exist yet (fresh install).
    let dir = TempDir::new();
    sweep_orphaned_tmp_files(&dir.join("missing"));
}

// -- install step (#368): fsync before the rename, off the executor --

#[derive(Debug, Clone, PartialEq, Eq)]
enum FsOp {
    CreateDir(PathBuf),
    Sync(PathBuf),
    Rename(PathBuf, PathBuf),
}

type Gate = (
    std::sync::Mutex<std::sync::mpsc::Sender<()>>,
    std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
);

/// The real file operations, recording what ran, in order, and on which thread.
#[derive(Default)]
struct RecordingFs {
    ops: std::sync::Mutex<Vec<(FsOp, std::thread::ThreadId)>>,
    fail_sync: bool,
    /// When set, `sync_file` reports that it started, then waits for a release.
    hold_sync: Option<Gate>,
}

impl RecordingFs {
    fn record(&self, op: FsOp) {
        self.ops
            .lock()
            .unwrap()
            .push((op, std::thread::current().id()));
    }

    fn ops(&self) -> Vec<FsOp> {
        self.ops
            .lock()
            .unwrap()
            .iter()
            .map(|(op, _)| op.clone())
            .collect()
    }

    fn renamed(&self) -> bool {
        self.ops().iter().any(|op| matches!(op, FsOp::Rename(..)))
    }
}

impl DownloadFs for RecordingFs {
    fn create_dir_all(&self, dir: &Path) -> std::io::Result<()> {
        self.record(FsOp::CreateDir(dir.to_path_buf()));
        RealFs.create_dir_all(dir)
    }

    fn sync_file(&self, file: &std::fs::File, path: &Path) -> std::io::Result<()> {
        self.record(FsOp::Sync(path.to_path_buf()));
        if let Some((started, release)) = &self.hold_sync {
            started.lock().unwrap().send(()).unwrap();
            release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
        }
        if self.fail_sync {
            return Err(std::io::Error::other("injected sync failure"));
        }
        RealFs.sync_file(file, path)
    }

    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        self.record(FsOp::Rename(from.to_path_buf(), to.to_path_buf()));
        RealFs.rename(from, to)
    }
}

fn serve_valid_model() -> (String, std::thread::JoinHandle<Vec<std::io::Error>>) {
    serve_scripts(vec![vec![
        ok_head(Some(14)),
        Step::Write(b"verified model".to_vec()),
    ]])
}

async fn download_through(
    url: &str,
    dest: &Path,
    fs: std::sync::Arc<RecordingFs>,
) -> Result<(), DictationError> {
    let env = DownloadEnv {
        fs,
        ..DownloadEnv::production(fast_limits())
    };
    download_with(url, dest, "bin", pinned(14), None, |_, _| {}, &env).await
}

/// Polls `condition` until it holds or `within` has passed.
async fn eventually(within: Duration, condition: impl Fn() -> bool) -> bool {
    let give_up = std::time::Instant::now() + within;
    while !condition() {
        if std::time::Instant::now() > give_up {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    true
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn temp_file_is_synced_before_it_is_renamed_into_place() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let dir = TempDir::new();
        let dest = dir.join("model.bin");
        let (url, server) = serve_valid_model();
        let fs = std::sync::Arc::new(RecordingFs::default());

        download_through(&url, &dest, fs.clone()).await.unwrap();

        let ops = fs.ops();
        let [FsOp::CreateDir(created), FsOp::Sync(synced), FsOp::Rename(from, to)] = ops.as_slice()
        else {
            panic!("expected create_dir_all, sync, rename in that order, got {ops:?}");
        };
        assert_eq!(created, dir.path());
        assert_eq!(synced, from, "the synced file is the one renamed");
        assert_eq!(to, &dest);
        assert_eq!(std::fs::read(&dest).unwrap(), b"verified model");
        join_server(server).await;
    })
    .await
    .expect("test exceeded its hard timeout");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_temp_file_sync_installs_nothing() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let dir = TempDir::new();
        let dest = dir.join("model.bin");
        let (url, server) = serve_valid_model();
        let fs = std::sync::Arc::new(RecordingFs {
            fail_sync: true,
            ..RecordingFs::default()
        });

        let err = download_through(&url, &dest, fs.clone())
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("Failed to sync temp file"), "{err}");
        assert!(!fs.renamed(), "renamed after a failed sync: {:?}", fs.ops());
        assert_empty(&dir, "a failed sync left a file");
        join_server(server).await;
    })
    .await
    .expect("test exceeded its hard timeout");
}

/// `#[tokio::test]` runs a current-thread runtime: file work done inline
/// would run on this very thread.
#[tokio::test]
async fn download_file_operations_run_off_the_async_executor() {
    let executor = std::thread::current().id();
    let dir = TempDir::new();
    let dest = dir.join("model.bin");
    let (url, server) = serve_valid_model();
    let fs = std::sync::Arc::new(RecordingFs::default());

    download_through(&url, &dest, fs.clone()).await.unwrap();

    let ops = fs.ops.lock().unwrap().clone();
    let on_executor: Vec<_> = ops
        .iter()
        .filter(|(_, thread)| *thread == executor)
        .map(|(op, _)| op)
        .collect();
    assert!(
        on_executor.is_empty(),
        "blocking file operations ran on the async executor thread: {on_executor:?}"
    );
    assert_eq!(ops.len(), 3, "{ops:?}");
    #[cfg(unix)]
    assert!(verification_stamp_path(&dest).exists());
    join_server(server).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_while_the_temp_file_syncs_installs_nothing() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let dir = TempDir::new();
        let dest = dir.join("model.bin");
        let (url, server) = serve_valid_model();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let fs = std::sync::Arc::new(RecordingFs {
            hold_sync: Some((
                std::sync::Mutex::new(started_tx),
                std::sync::Mutex::new(release_rx),
            )),
            ..RecordingFs::default()
        });
        let task = tokio::spawn({
            let (fs, dest) = (fs.clone(), dest.clone());
            async move { download_through(&url, &dest, fs).await }
        });
        tokio::task::spawn_blocking(move || started_rx.recv_timeout(Duration::from_secs(10)))
            .await
            .unwrap()
            .expect("the install step never synced the temp file");

        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        release_tx.send(()).unwrap();

        // The blocking install step finishes on its own after the release.
        assert!(
            eventually(Duration::from_secs(5), || std::fs::read_dir(&dir)
                .unwrap()
                .next()
                .is_none())
            .await,
            "a download cancelled before its rename left {:?}",
            std::fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .map(|e| e.file_name())
                .collect::<Vec<_>>()
        );
        assert!(!fs.renamed(), "renamed after cancellation: {:?}", fs.ops());
        join_server(server).await;
    })
    .await
    .expect("test exceeded its hard timeout");
}
