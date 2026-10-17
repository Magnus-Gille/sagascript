//! The machine state the settings code depends on, injected by the caller.
//!
//! Settings never ask the machine themselves: what is downloaded, and whether
//! Pianissimo can run here, arrive through these types. The store takes a
//! [`MachineProbe`] (`store::load_with`, `store::try_update_with`), the
//! `*_gated` methods of [`Settings`](super::Settings) take plain values. The
//! probes of *this* machine live in `crate::machine`, above both settings and
//! transcription (#372).

use crate::models::WhisperModel;

/// Machine state the lineup migration depends on, injected for testability.
pub struct LineupEnv<'a> {
    /// `pianissimo_backend::runtime_supported_on_this_os()`.
    pub pianissimo_supported: bool,
    /// `pianissimo_model::is_downloaded()`.
    pub pianissimo_downloaded: bool,
    /// `model::is_model_downloaded`.
    pub whisper_downloaded: &'a dyn Fn(WhisperModel) -> bool,
}

/// Machine state a migration step may need. Probed lazily: only a pending
/// Swedish lineup step asks for [`MachineProbe::lineup_env`].
pub(crate) trait MachineProbe {
    /// `pianissimo_backend::runtime_supported_on_this_os()`.
    fn pianissimo_supported(&self) -> bool;
    /// What the Swedish model-lineup step needs to know about this machine.
    fn lineup_env(&self) -> LineupEnv<'_>;
}

/// Fixed machine state, for tests.
impl MachineProbe for LineupEnv<'_> {
    fn pianissimo_supported(&self) -> bool {
        self.pianissimo_supported
    }

    fn lineup_env(&self) -> LineupEnv<'_> {
        LineupEnv {
            pianissimo_supported: self.pianissimo_supported,
            pianissimo_downloaded: self.pianissimo_downloaded,
            whisper_downloaded: self.whisper_downloaded,
        }
    }
}

#[cfg(test)]
#[path = "probes_tests.rs"]
mod tests;
