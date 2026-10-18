//! Where a file run's audio comes from (#343): decoding the file, or, for a
//! diarized run, a validated diarization cache that stands in for decoding and
//! for the inference that made it.

use std::path::Path;
#[cfg(feature = "diarization")]
use std::path::PathBuf;
use std::time::Instant;

use indicatif::{ProgressBar, ProgressStyle};

#[cfg(feature = "diarization")]
use sagascript_core::diarization::DiarizationAnalysis;
use sagascript_core::error::DictationError;
#[cfg(feature = "diarization")]
use sagascript_core::transcription::diagnostics::{
    CoverageProfile, LanguageDetection, LanguageRegionDiagnostics,
};
#[cfg(feature = "diarization")]
use sagascript_core::transcription::TranscriptSegment;

#[cfg(feature = "diarization")]
use super::diarize::CachePolicy;
use super::meeting::MeetingControl;
#[cfg(feature = "diarization")]
use super::meeting::{meeting_checkpoint, MeetingPhase};
#[cfg(feature = "diarization")]
use super::plan::{DiarizationPlan, FileRequest};
use super::progress::Progress;
#[cfg(feature = "diarization")]
use super::resolve::model_id_string;
#[cfg(feature = "diarization")]
use super::services::Services;
use crate::cancel::Canceller;
#[cfg(feature = "diarization")]
use crate::diarization_cache::{CacheIdentity, CacheLookup, DiarizationCache};
use crate::output::{errln, Out};

/// A file decoded to what Whisper takes: 16 kHz mono samples.
pub(super) struct DecodedAudio {
    pub(super) samples: Vec<f32>,
    /// Seconds spent decoding and resampling; only the diarized run's
    /// performance report reads it.
    #[cfg_attr(not(feature = "diarization"), allow(dead_code))]
    pub(super) decode_resample_seconds: f64,
}

impl DecodedAudio {
    pub(super) fn duration_seconds(&self) -> f64 {
        self.samples.len() as f64 / 16_000.0
    }
}

/// Decodes and resamples `file`, narrating on stderr and in `--progress-json`
/// events. A cancel (the `canceller`'s, or the meeting job's flag) stops it
/// between packets.
pub(super) fn decode_file(
    file: &Path,
    canceller: &Canceller,
    control: Option<&MeetingControl<'_>>,
    progress: &Progress,
    out: &Out,
) -> Result<DecodedAudio, DictationError> {
    errln!(out, "Decoding {}...", file.display());
    let decode_started = Instant::now();
    // Byte-fraction decode bar for large files (#237): small files
    // decode sub-second, so only bar above 8 MB to avoid flicker.
    let decode_len = std::fs::metadata(file).map(|m| m.len()).unwrap_or(0);
    let decode_bar = (decode_len > 8_000_000).then(|| {
        let pb = ProgressBar::new(100);
        pb.set_style(ProgressStyle::with_template("  Decoding [{bar:40}] {pos}%").unwrap());
        pb
    });
    let on_decode = |pct: u8| {
        progress.phase("decoding", Some(pct));
        if let Some(pb) = &decode_bar {
            pb.set_position(pct as u64);
        }
    };
    let on_resample = |pct: u8| {
        progress.phase("resampling", Some(pct));
        if let Some(pb) = &decode_bar {
            if pct == 0 {
                pb.set_style(
                    ProgressStyle::with_template("  Resampling [{bar:40}] {pos}%").unwrap(),
                );
            }
            pb.set_position(pct as u64);
        }
    };
    let checkpoint = || {
        canceller.check()?;
        control.map_or(Ok(()), |control| control.check())
    };
    on_decode(0);
    let samples = sagascript_core::audio::decoder::decode_audio_file_with_stage_progress(
        file,
        Some(&checkpoint),
        &on_decode,
        &on_resample,
    )?;
    if let Some(pb) = decode_bar {
        pb.finish_and_clear();
    }
    let audio = DecodedAudio {
        decode_resample_seconds: decode_started.elapsed().as_secs_f64(),
        samples,
    };
    errln!(
        out,
        "Audio: {:.1}s, {} samples",
        audio.duration_seconds(),
        audio.samples.len()
    );
    Ok(audio)
}

// ---- the diarized run's source ---------------------------------------------------

/// A diarization cache a run may read and, on a miss, writes: where it lives,
/// the run it belongs to and the policy that governs it.
#[cfg(feature = "diarization")]
pub(super) struct CacheTarget {
    pub(super) path: PathBuf,
    pub(super) identity: CacheIdentity,
    pub(super) policy: CachePolicy,
}

/// What a validated cache hit brings, in place of decoding the file and
/// running the analysis and the Whisper timestamps again.
#[cfg(feature = "diarization")]
pub(super) struct CachedRun {
    pub(super) analysis: DiarizationAnalysis,
    pub(super) transcript: Vec<(f64, f64, String)>,
    pub(super) coverage_segments: Vec<TranscriptSegment>,
    pub(super) coverage_profile: CoverageProfile,
    pub(super) detected_language: Option<LanguageDetection>,
    pub(super) language_regions: Option<LanguageRegionDiagnostics>,
}

#[cfg(feature = "diarization")]
impl CachedRun {
    /// The loader returns a hit only for a cache with coarse coverage
    /// segments; a hit without them is a bug of the loader, reported rather
    /// than trusted.
    fn from_hit(hit: DiarizationCache) -> Result<Self, DictationError> {
        let coverage_segments = hit.coverage_segments.ok_or_else(|| {
            DictationError::DiarizationError(
                "A validated diarization cache has no coverage segments".to_string(),
            )
        })?;
        Ok(Self {
            analysis: hit.analysis,
            transcript: hit.transcript,
            coverage_segments,
            coverage_profile: hit.coverage_profile,
            detected_language: hit.detected_language,
            language_regions: hit.language_regions,
        })
    }
}

/// Where a diarized run gets its audio and its analysis: a cache hit, or the
/// decoded file (a miss, a cache that is not read, or no cache at all).
#[cfg(feature = "diarization")]
pub(super) enum Source {
    Cached(Box<CachedRun>),
    Decoded(DecodedAudio),
}

#[cfg(feature = "diarization")]
impl Source {
    pub(super) fn duration_seconds(&self) -> f64 {
        match self {
            Self::Cached(run) => run.coverage_profile.duration_seconds(),
            Self::Decoded(audio) => audio.duration_seconds(),
        }
    }

    /// Seconds spent decoding and resampling; a cache hit decoded nothing.
    pub(super) fn decode_resample_seconds(&self) -> f64 {
        match self {
            Self::Cached(_) => 0.0,
            Self::Decoded(audio) => audio.decode_resample_seconds,
        }
    }

    /// Which stretches of the audio carry speech.
    pub(super) fn coverage_profile(&self) -> CoverageProfile {
        match self {
            Self::Cached(run) => run.coverage_profile.clone(),
            Self::Decoded(audio) => CoverageProfile::from_audio(&audio.samples),
        }
    }
}

/// What a diarized run holds once its source is settled.
#[cfg(feature = "diarization")]
pub(super) struct Acquired {
    pub(super) source: Source,
    /// The cache to write on a miss; `None` without `--diarize-cache`.
    pub(super) cache: Option<CacheTarget>,
    /// Seconds from naming the cache's identity to knowing hit or miss.
    pub(super) cache_lookup_seconds: f64,
}

/// Settles a diarized run's source: looks the cache up (unless the policy
/// says not to read it), then either takes the hit or checks the diarization
/// models and decodes the file.
#[cfg(feature = "diarization")]
pub(super) fn acquire_source(
    request: &FileRequest<'_>,
    plan: &DiarizationPlan,
    services: Services<'_>,
    progress: &Progress,
    out: &Out,
) -> Result<Acquired, DictationError> {
    let FileRequest {
        file,
        language,
        model,
        control,
        ..
    } = *request;
    let flags = &plan.flags;
    let cache_lookup_started = Instant::now();
    let cache = flags
        .cache
        .as_ref()
        .map(|cache| {
            CacheIdentity::for_source_sha256_with_options(
                &plan.source_sha256,
                language.whisper_code().unwrap_or("auto"),
                model_id_string(model),
                plan.decoder_prompt.as_deref(),
                plan.options.beam_size,
                plan.options.temperature_fallback,
                plan.options.vad_model_path.is_some(),
            )
            .map(|identity| CacheTarget {
                path: cache.path.clone(),
                identity,
                policy: cache.policy,
            })
        })
        .transpose()?;
    let hit = match &cache {
        Some(target) if target.policy.reads_existing_cache() => {
            lookup_cache(target, flags.exclusive_speech_embeddings, out)?
        }
        _ => None,
    };
    let cache_lookup_seconds = cache_lookup_started.elapsed().as_secs_f64();

    if hit.is_none() && !services.analyzer.models_downloaded() {
        return Err(DictationError::DiarizationError(
            "Diarization models not found. Run: sagascript download-model diarization".to_string(),
        ));
    }

    if flags.meeting_json {
        meeting_checkpoint(control, MeetingPhase::Decoding)?;
    }
    let source = match hit {
        Some(hit) => {
            let run = CachedRun::from_hit(*hit)?;
            errln!(
                out,
                "Audio (cached): {:.1}s",
                run.coverage_profile.duration_seconds()
            );
            Source::Cached(Box::new(run))
        }
        None => Source::Decoded(decode_file(
            file,
            services.canceller,
            control,
            progress,
            out,
        )?),
    };
    if flags.meeting_json {
        meeting_checkpoint(control, MeetingPhase::Decoding)?;
    }
    Ok(Acquired {
        source,
        cache,
        cache_lookup_seconds,
    })
}

#[cfg(feature = "diarization")]
fn lookup_cache(
    target: &CacheTarget,
    exclusive_speech_embeddings: bool,
    out: &Out,
) -> Result<Option<Box<DiarizationCache>>, DictationError> {
    match crate::diarization_cache::load_with_analysis_policy(
        &target.path,
        &target.identity,
        exclusive_speech_embeddings,
    )? {
        CacheLookup::Hit(cached) => {
            errln!(out, "Reusing diarization cache: {}", target.path.display());
            Ok(Some(cached))
        }
        CacheLookup::Miss(reason) => {
            errln!(
                out,
                "Diarization cache miss ({reason}); computing analysis."
            );
            Ok(None)
        }
    }
}
