//! Encoder backend selection and the QNN (NPU) load state machine (SAGA-542).
//!
//! Everything here is pure or runs against the [`QnnRuntime`] trait, so the policy (which
//! backend, when to fall back, when to recompile a cache) is unit-tested on every platform.
//! The Windows ARM64 FFI that implements [`QnnRuntime`] is deliberately kept out of this file.
// Wired into `ort_engine` by the follow-up Windows glue; until then only tests use it.
#![allow(dead_code)]

use crate::qnn_cache::Cache;
use std::path::{Path, PathBuf};

/// What the caller asked for (`compute_units` on the load request).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Requested {
    Cpu,
    Npu,
    Auto,
}

impl Requested {
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value {
            None | Some("") | Some("auto") => Ok(Self::Auto),
            Some("cpu") => Ok(Self::Cpu),
            Some("npu") => Ok(Self::Npu),
            Some(other) => Err(format!("unknown compute units '{other}'")),
        }
    }
}

/// Facts about the machine and the installed QNN package; plain data so tests can vary them.
#[derive(Debug, Clone, Copy, Default)]
pub struct Probe {
    pub arm64_windows: bool,
    pub plugin_present: bool,
    pub htp_present: bool,
    /// The QNN encoder files match `qnn-model.json` (size and digest).
    pub qnn_package_verified: bool,
    /// ONNX Runtime exposed a QNN NPU device.
    pub npu_device_seen: bool,
}

impl Probe {
    /// `None` when the NPU is usable, otherwise the first reason it is not.
    pub fn unavailable_reason(&self) -> Option<&'static str> {
        if !self.arm64_windows {
            Some("not a Windows ARM64 machine")
        } else if !self.plugin_present || !self.htp_present {
            Some("QNN libraries are not installed")
        } else if !self.qnn_package_verified {
            Some("QNN encoder package is missing or does not match its manifest")
        } else if !self.npu_device_seen {
            Some("ONNX Runtime found no QNN NPU device")
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Cpu,
    Npu,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Npu => "npu",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub backend: Backend,
    pub reason: String,
}

/// Choose the backend. `npu_qualified` is the gate that keeps `auto` on CPU until the NPU path
/// has passed hardware acceptance (SAGA-576); an explicit `npu` request ignores it.
pub fn select(
    requested: Requested,
    probe: &Probe,
    npu_qualified: bool,
) -> Result<Decision, String> {
    let cpu = |reason: &str| Decision {
        backend: Backend::Cpu,
        reason: reason.to_string(),
    };
    match requested {
        Requested::Cpu => Ok(cpu("cpu requested")),
        Requested::Npu => match probe.unavailable_reason() {
            None => Ok(Decision {
                backend: Backend::Npu,
                reason: "npu requested".into(),
            }),
            Some(why) => Err(format!("npu requested but unavailable: {why}")),
        },
        Requested::Auto => match probe.unavailable_reason() {
            Some(why) => Ok(cpu(&format!("auto: {why}"))),
            None if !npu_qualified => Ok(cpu("auto: npu not yet qualified as default")),
            None => Ok(Decision {
                backend: Backend::Npu,
                reason: "auto: compatible QNN NPU".into(),
            }),
        },
    }
}

/// Opaque handle to a loaded encoder session.
pub trait QnnRuntime {
    type Session;
    /// Build a QNN session; `context` is the cached compiled-context path to load or write.
    fn build_session(&self, context: &Path) -> Result<Self::Session, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheState {
    Hit,
    Compiled,
    /// A cached context failed to load, was invalidated and recompiled.
    Recovered,
}

impl CacheState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Compiled => "compiled",
            Self::Recovered => "recovered",
        }
    }
}

#[derive(Debug)]
pub struct Loaded<S> {
    pub session: S,
    pub cache: CacheState,
    pub context: PathBuf,
    /// Why the cached context did not load, when it was recompiled ([`CacheState::Recovered`]).
    pub recovered_from: Option<String>,
}

fn compile<R: QnnRuntime>(cache: &Cache, runtime: &R) -> Result<(R::Session, PathBuf), String> {
    let context = cache.begin_generation()?;
    let published = runtime
        .build_session(&context)
        .and_then(|session| cache.complete_generation(&context).map(|()| session));
    match published {
        Ok(session) => {
            // The new generation supersedes every other one (including an invalidated one).
            cache.prune_except(&context);
            Ok((session, context))
        }
        Err(error) => {
            // A half-written generation is of no use and would otherwise stay on disk.
            cache.discard(&context);
            Err(error)
        }
    }
}

/// Load the NPU encoder: reuse a valid cached context, recompile once if it will not load.
pub fn load_npu<R: QnnRuntime>(cache: &Cache, runtime: &R) -> Result<Loaded<R::Session>, String> {
    if let Some(context) = cache.cached_model()? {
        match runtime.build_session(&context) {
            Ok(session) => {
                return Ok(Loaded {
                    session,
                    cache: CacheState::Hit,
                    context,
                    recovered_from: None,
                })
            }
            Err(first) => {
                cache.invalidate(&context)?;
                let (session, context) = compile(cache, runtime).map_err(|error| {
                    format!("{error} (the cached context also failed to load: {first})")
                })?;
                return Ok(Loaded {
                    session,
                    cache: CacheState::Recovered,
                    context,
                    recovered_from: Some(first),
                });
            }
        }
    }
    let (session, context) = compile(cache, runtime)?;
    Ok(Loaded {
        session,
        cache: CacheState::Compiled,
        context,
        recovered_from: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::fs;

    fn ok_probe() -> Probe {
        Probe {
            arm64_windows: true,
            plugin_present: true,
            htp_present: true,
            qnn_package_verified: true,
            npu_device_seen: true,
        }
    }

    #[test]
    fn explicit_cpu_never_uses_the_npu() {
        let d = select(Requested::Cpu, &ok_probe(), true).unwrap();
        assert_eq!(d.backend, Backend::Cpu);
    }

    #[test]
    fn explicit_npu_fails_loudly_instead_of_running_on_cpu() {
        let mut p = ok_probe();
        p.npu_device_seen = false;
        let e = select(Requested::Npu, &p, true).unwrap_err();
        assert!(e.contains("no QNN NPU device"), "{e}");
    }

    #[test]
    fn auto_falls_back_to_cpu_with_a_reason_for_each_gap() {
        for (mutate, text) in [
            (
                (|p: &mut Probe| p.arm64_windows = false) as fn(&mut Probe),
                "not a Windows ARM64",
            ),
            (|p| p.plugin_present = false, "not installed"),
            (|p| p.htp_present = false, "not installed"),
            (|p| p.qnn_package_verified = false, "manifest"),
            (|p| p.npu_device_seen = false, "no QNN NPU"),
        ] {
            let mut p = ok_probe();
            mutate(&mut p);
            let d = select(Requested::Auto, &p, true).unwrap();
            assert_eq!(d.backend, Backend::Cpu);
            assert!(d.reason.contains(text), "{}", d.reason);
        }
    }

    #[test]
    fn auto_stays_on_cpu_until_qualified() {
        let d = select(Requested::Auto, &ok_probe(), false).unwrap();
        assert_eq!(d.backend, Backend::Cpu);
        assert_eq!(
            select(Requested::Auto, &ok_probe(), true).unwrap().backend,
            Backend::Npu
        );
    }

    #[test]
    fn parse_rejects_unknown_values() {
        assert_eq!(Requested::parse(None), Ok(Requested::Auto));
        assert_eq!(Requested::parse(Some("npu")), Ok(Requested::Npu));
        assert!(Requested::parse(Some("gpu")).is_err());
    }

    struct Fake {
        /// Number of initial build_session calls that fail.
        fail_first: Cell<u32>,
        calls: Cell<u32>,
    }

    impl QnnRuntime for Fake {
        type Session = u32;
        fn build_session(&self, context: &Path) -> Result<u32, String> {
            let n = self.calls.get() + 1;
            self.calls.set(n);
            if self.fail_first.get() > 0 {
                self.fail_first.set(self.fail_first.get() - 1);
                return Err("boom".into());
            }
            // A real compile leaves the context artifact behind.
            fs::write(context, b"ctx").map_err(|e| e.to_string())?;
            let weights = context.with_file_name(crate::qnn_cache::WEIGHTS_FILE);
            fs::write(weights, b"weights").map_err(|e| e.to_string())?;
            Ok(n)
        }
    }

    fn temp(tag: &str) -> PathBuf {
        let p =
            std::env::temp_dir().join(format!("sagascript-backend-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn compile_then_hit_then_recover_from_a_broken_cache() {
        let root = temp("flow");
        let cache = Cache::open(&root, "id-1").unwrap();
        let rt = Fake {
            fail_first: Cell::new(0),
            calls: Cell::new(0),
        };
        assert_eq!(load_npu(&cache, &rt).unwrap().cache, CacheState::Compiled);
        assert_eq!(load_npu(&cache, &rt).unwrap().cache, CacheState::Hit);
        rt.fail_first.set(1); // the cached context no longer loads
        let recovered = load_npu(&cache, &rt).unwrap();
        assert_eq!(recovered.cache, CacheState::Recovered);
        assert_eq!(recovered.recovered_from.as_deref(), Some("boom"));
        assert_eq!(load_npu(&cache, &rt).unwrap().cache, CacheState::Hit);
        assert_eq!(generations(&cache), 1, "superseded generations are removed");
        let _ = fs::remove_dir_all(&root);
    }

    /// Every `gen-*` directory anywhere under `dir`.
    fn count_generations(dir: &Path) -> usize {
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|ty| ty.is_dir()))
            .map(|entry| {
                let own = usize::from(entry.file_name().to_string_lossy().starts_with("gen-"));
                own + count_generations(&entry.path())
            })
            .sum()
    }

    fn generations(cache: &Cache) -> usize {
        // The cache directory is the parent of any generation; find it from the newest model.
        let model = cache.cached_model().unwrap().unwrap();
        let dir = model.parent().unwrap().parent().unwrap().to_path_buf();
        fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("gen-"))
            .count()
    }

    #[test]
    fn a_failed_recompile_reports_both_errors_and_leaves_no_generation() {
        let root = temp("both");
        let cache = Cache::open(&root, "id-3").unwrap();
        let rt = Fake {
            fail_first: Cell::new(0),
            calls: Cell::new(0),
        };
        load_npu(&cache, &rt).unwrap();
        rt.fail_first.set(2); // the cached context and the recompile both fail
        let error = load_npu(&cache, &rt).unwrap_err();
        assert!(error.contains("boom"), "{error}");
        assert!(
            error.contains("cached context also failed to load: boom"),
            "{error}"
        );
        assert!(cache.cached_model().unwrap().is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_failed_compile_is_an_error_and_leaves_nothing_reusable() {
        let root = temp("fail");
        let cache = Cache::open(&root, "id-2").unwrap();
        let rt = Fake {
            fail_first: Cell::new(1),
            calls: Cell::new(0),
        };
        assert!(load_npu(&cache, &rt).is_err());
        assert!(cache.cached_model().unwrap().is_none());
        let leftovers = count_generations(&root);
        assert_eq!(leftovers, 0, "a failed compile leaves no generation behind");
        let _ = fs::remove_dir_all(&root);
    }
}
