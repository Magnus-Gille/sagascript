//! Source-labelled live meetings. Capture and inference adapters share these contracts.
#[cfg(all(feature = "record", target_os = "macos"))]
pub mod capture;
pub mod document;
pub mod queue;
pub mod recording;
#[cfg(any(test, all(feature = "record", target_os = "macos")))]
mod refine;
pub mod segmenter;

#[cfg(all(feature = "record", target_os = "macos"))]
pub mod echo;
pub mod resampler;

pub mod state;

#[cfg(all(feature = "record", target_os = "macos"))]
mod runtime;
pub mod session;
pub mod timeline;
