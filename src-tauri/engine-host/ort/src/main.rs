//! `sagascript-engine-host-ort`: Pianissimo on ONNX Runtime (CPU), engine-host protocol v1.
//! stdout carries protocol lines only; diagnostics go to stderr.

mod ort_engine;
mod pcm;
mod server;
mod tdt;
mod boost;
mod vocab;

use ort_engine::{OrtEngine, RuntimeConfig};
use server::{HostBuild, Server};
use std::io::BufRead;

const GIT_SHA: &str = env!("SAGASCRIPT_HOST_GIT_SHA");
const DIRTY: &str = env!("SAGASCRIPT_HOST_DIRTY");

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(2);
}

fn main() {
    #[cfg(windows)]
    ort_engine::restrict_dll_search();
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.iter().any(|a| a == "--version") {
        println!(
            "sagascript-engine-host {} ({GIT_SHA}, dirty={DIRTY}, engine=onnx)",
            env!("CARGO_PKG_VERSION")
        );
        return;
    }
    let mut protocol = None;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--protocol" => {
                index += 1;
                protocol = Some(arguments.get(index).cloned().unwrap_or_else(|| fail("--protocol requires a value")));
            }
            // Accepted for interface parity with the Core ML host; ONNX Runtime keeps no on-disk cache.
            "--cache-dir" => {
                index += 1;
                if index >= arguments.len() {
                    fail("--cache-dir requires a value");
                }
            }
            other => fail(&format!("unknown argument: {other}")),
        }
        index += 1;
    }
    if protocol.as_deref() != Some("1") {
        fail("usage: sagascript-engine-host-ort --protocol 1 [--cache-dir DIR]");
    }

    let server = Server::new(
        OrtEngine::new(RuntimeConfig::from_env()),
        HostBuild {
            version: env!("CARGO_PKG_VERSION").to_string(),
            git_sha: GIT_SHA.to_string(),
            dirty: DIRTY == "true",
        },
        Box::new(std::io::stdout()),
        Box::new(|| std::process::exit(0)),
    );

    #[cfg(unix)]
    {
        let parent = std::os::unix::process::parent_id();
        let watched = std::sync::Arc::clone(&server);
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            if std::os::unix::process::parent_id() != parent {
                watched.receive_eof();
                std::process::exit(0);
            }
        });
    }

    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => server.handle_line(line.trim_end_matches(['\n', '\r'])),
            Err(error) => {
                eprintln!("stdin read error: {error}");
                break;
            }
        }
    }
    server.receive_eof();
    std::process::exit(0);
}
