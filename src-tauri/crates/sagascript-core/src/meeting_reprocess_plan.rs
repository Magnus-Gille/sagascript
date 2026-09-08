//! Pure, revision-checked work selection for meeting reprocessing.
//!
//! A plan carries the caller-computed input identities and the exact work the
//! selected mode requires.  It is deliberately only a value object: callers
//! still have to enforce the selected work at the runtime orchestration layer.

use serde::{de::Error as DeError, Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const REPROCESSING_PLAN_SCHEMA_VERSION: u32 = 1;

/// The bounded reprocessing operation selected by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReprocessingMode {
    Recluster,
    Rediarize,
    Full,
}

/// Deterministic runtime work implied by a [`ReprocessingMode`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequiredWork {
    pub decode_audio: bool,
    pub transcription: bool,
    pub language_detection: bool,
    pub segmentation: bool,
    pub embeddings: bool,
    pub clustering: bool,
}

impl RequiredWork {
    fn for_mode(mode: ReprocessingMode) -> Self {
        match mode {
            ReprocessingMode::Recluster => Self {
                decode_audio: false,
                transcription: false,
                language_detection: false,
                segmentation: false,
                embeddings: false,
                clustering: true,
            },
            ReprocessingMode::Rediarize => Self {
                decode_audio: true,
                transcription: false,
                language_detection: false,
                segmentation: true,
                embeddings: true,
                clustering: true,
            },
            ReprocessingMode::Full => Self {
                decode_audio: true,
                transcription: true,
                language_detection: true,
                segmentation: true,
                embeddings: true,
                clustering: true,
            },
        }
    }
}

/// Caller-computed identities that make a plan specific to its inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReprocessingContext {
    pub source_sha256: String,
    pub previous_revision: String,
    pub transcription_context_sha256: String,
    pub analysis_context_sha256: String,
    pub cache_sha256: Option<String>,
}

impl ReprocessingContext {
    fn validate_for_mode(&self, mode: ReprocessingMode) -> Result<(), ReprocessingPlanError> {
        for (field, value) in [
            ("source_sha256", &self.source_sha256),
            ("previous_revision", &self.previous_revision),
            (
                "transcription_context_sha256",
                &self.transcription_context_sha256,
            ),
            ("analysis_context_sha256", &self.analysis_context_sha256),
        ] {
            if !is_sha256(value) {
                return Err(ReprocessingPlanError::InvalidField(field));
            }
        }

        match (mode, self.cache_sha256.as_deref()) {
            (ReprocessingMode::Full, Some(_)) => {
                Err(ReprocessingPlanError::InvalidField("cache_sha256"))
            }
            (ReprocessingMode::Full, None) => Ok(()),
            (_, None) => Err(ReprocessingPlanError::InvalidField("cache_sha256")),
            (_, Some(cache_sha256)) if !is_sha256(cache_sha256) => {
                Err(ReprocessingPlanError::InvalidField("cache_sha256"))
            }
            (_, Some(_)) => Ok(()),
        }
    }
}

/// Errors returned while constructing or checking a reprocessing plan.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ReprocessingPlanError {
    #[error("unsupported reprocessing plan schema version {0}")]
    UnsupportedSchema(u32),
    #[error("invalid reprocessing plan field {0}")]
    InvalidField(&'static str),
    #[error("reprocessing plan revision does not match its contents")]
    RevisionMismatch,
    #[error("reprocessing plan context is stale")]
    StaleContext,
    #[error("reprocessing plan serialization failed")]
    Serialization,
}

/// A versioned, immutable wire value describing one reprocessing run.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReprocessingPlan {
    pub schema_version: u32,
    pub mode: ReprocessingMode,
    pub context: ReprocessingContext,
    pub threshold: f32,
    pub required_work: RequiredWork,
    pub revision: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReprocessingPlanWire {
    schema_version: u32,
    mode: ReprocessingMode,
    context: ReprocessingContext,
    threshold: f32,
    required_work: RequiredWork,
    revision: String,
}

impl<'de> Deserialize<'de> for ReprocessingPlan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ReprocessingPlanWire::deserialize(deserializer)?;
        let plan = Self {
            schema_version: wire.schema_version,
            mode: wire.mode,
            context: wire.context,
            threshold: wire.threshold,
            required_work: wire.required_work,
            revision: wire.revision,
        };
        plan.validate().map_err(D::Error::custom)?;
        Ok(plan)
    }
}

#[derive(Serialize)]
struct RevisionPayload<'a> {
    schema_version: u32,
    mode: ReprocessingMode,
    context: &'a ReprocessingContext,
    threshold: f32,
    required_work: &'a RequiredWork,
}

impl ReprocessingPlan {
    /// Build a plan with mode-derived work flags and a deterministic revision.
    pub fn new(
        mode: ReprocessingMode,
        context: ReprocessingContext,
        threshold: f32,
    ) -> Result<Self, ReprocessingPlanError> {
        context.validate_for_mode(mode)?;
        validate_threshold(threshold)?;

        let mut plan = Self {
            schema_version: REPROCESSING_PLAN_SCHEMA_VERSION,
            mode,
            context,
            threshold,
            required_work: RequiredWork::for_mode(mode),
            revision: String::new(),
        };
        plan.revision = plan.compute_revision()?;
        plan.validate()?;
        Ok(plan)
    }

    /// Validate the schema, context, mode-derived work, and content revision.
    pub fn validate(&self) -> Result<(), ReprocessingPlanError> {
        if self.schema_version != REPROCESSING_PLAN_SCHEMA_VERSION {
            return Err(ReprocessingPlanError::UnsupportedSchema(
                self.schema_version,
            ));
        }
        self.context.validate_for_mode(self.mode)?;
        validate_threshold(self.threshold)?;
        if self.required_work != RequiredWork::for_mode(self.mode) {
            return Err(ReprocessingPlanError::InvalidField("required_work"));
        }
        if !is_sha256(&self.revision) {
            return Err(ReprocessingPlanError::InvalidField("revision"));
        }
        if self.compute_revision()? != self.revision {
            return Err(ReprocessingPlanError::RevisionMismatch);
        }
        Ok(())
    }

    /// Revalidate a plan against the exact context and revision held by the caller.
    pub fn revalidate(
        &self,
        current_context: &ReprocessingContext,
        expected_plan_revision: &str,
    ) -> Result<(), ReprocessingPlanError> {
        self.validate()?;
        if expected_plan_revision != self.revision {
            return Err(ReprocessingPlanError::RevisionMismatch);
        }
        current_context.validate_for_mode(self.mode)?;
        if current_context != &self.context {
            return Err(ReprocessingPlanError::StaleContext);
        }
        Ok(())
    }

    fn compute_revision(&self) -> Result<String, ReprocessingPlanError> {
        let payload = RevisionPayload {
            schema_version: self.schema_version,
            mode: self.mode,
            context: &self.context,
            threshold: self.threshold,
            required_work: &self.required_work,
        };
        let bytes =
            serde_json::to_vec(&payload).map_err(|_| ReprocessingPlanError::Serialization)?;
        Ok(hex_sha256(&bytes))
    }
}

fn validate_threshold(threshold: f32) -> Result<(), ReprocessingPlanError> {
    if !threshold.is_finite() || !(0.0..=2.0).contains(&threshold) {
        return Err(ReprocessingPlanError::InvalidField("threshold"));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(character: char) -> String {
        character.to_string().repeat(64)
    }

    fn context(cache: bool) -> ReprocessingContext {
        ReprocessingContext {
            source_sha256: sha('a'),
            previous_revision: sha('b'),
            transcription_context_sha256: sha('c'),
            analysis_context_sha256: sha('d'),
            cache_sha256: cache.then(|| sha('e')),
        }
    }

    fn plan(mode: ReprocessingMode) -> ReprocessingPlan {
        ReprocessingPlan::new(mode, context(mode != ReprocessingMode::Full), 0.75).unwrap()
    }

    #[test]
    fn work_flags_are_exact_and_revisions_are_repeatable() {
        let recluster = plan(ReprocessingMode::Recluster);
        assert_eq!(
            recluster.required_work,
            RequiredWork {
                decode_audio: false,
                transcription: false,
                language_detection: false,
                segmentation: false,
                embeddings: false,
                clustering: true,
            }
        );
        assert_eq!(
            plan(ReprocessingMode::Rediarize).required_work,
            RequiredWork {
                decode_audio: true,
                transcription: false,
                language_detection: false,
                segmentation: true,
                embeddings: true,
                clustering: true,
            }
        );
        assert_eq!(
            plan(ReprocessingMode::Full).required_work,
            RequiredWork {
                decode_audio: true,
                transcription: true,
                language_detection: true,
                segmentation: true,
                embeddings: true,
                clustering: true,
            }
        );

        let repeat = plan(ReprocessingMode::Recluster);
        assert_eq!(recluster, repeat);
        assert_eq!(recluster.revision, repeat.revision);
    }

    #[test]
    fn every_context_identity_can_make_a_plan_stale() {
        let plan = plan(ReprocessingMode::Rediarize);
        let mut stale_contexts = Vec::new();

        let mut source = plan.context.clone();
        source.source_sha256 = sha('f');
        stale_contexts.push(source);

        let mut previous = plan.context.clone();
        previous.previous_revision = sha('f');
        stale_contexts.push(previous);

        let mut transcription = plan.context.clone();
        transcription.transcription_context_sha256 = sha('f');
        stale_contexts.push(transcription);

        let mut analysis = plan.context.clone();
        analysis.analysis_context_sha256 = sha('f');
        stale_contexts.push(analysis);

        let mut cache = plan.context.clone();
        cache.cache_sha256 = Some(sha('f'));
        stale_contexts.push(cache);

        for stale in stale_contexts {
            assert_eq!(
                plan.revalidate(&stale, &plan.revision),
                Err(ReprocessingPlanError::StaleContext)
            );
        }
    }

    #[test]
    fn threshold_must_be_finite_and_within_bounds() {
        for threshold in [-0.01, 2.01, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                ReprocessingPlan::new(ReprocessingMode::Recluster, context(true), threshold)
                    .unwrap_err(),
                ReprocessingPlanError::InvalidField("threshold")
            );
        }
    }

    #[test]
    fn schema_revision_work_and_cache_tampering_are_rejected() {
        let plan = plan(ReprocessingMode::Recluster);

        let mut unsupported = serde_json::to_value(&plan).unwrap();
        unsupported["schema_version"] = serde_json::json!(2);
        let error = serde_json::from_value::<ReprocessingPlan>(unsupported)
            .unwrap_err()
            .to_string();
        assert!(error.contains("unsupported reprocessing plan schema version 2"));

        let mut invalid_revision = plan.clone();
        invalid_revision.revision = sha('f');
        assert_eq!(
            invalid_revision.validate(),
            Err(ReprocessingPlanError::RevisionMismatch)
        );
        invalid_revision.revision = "not-a-sha".to_owned();
        assert_eq!(
            invalid_revision.validate(),
            Err(ReprocessingPlanError::InvalidField("revision"))
        );

        let mut invalid_work = plan.clone();
        invalid_work.required_work.clustering = false;
        assert_eq!(
            invalid_work.validate(),
            Err(ReprocessingPlanError::InvalidField("required_work"))
        );

        let mut full_context = context(false);
        full_context.cache_sha256 = Some(sha('e'));
        assert_eq!(
            ReprocessingPlan::new(ReprocessingMode::Full, full_context, 0.5).unwrap_err(),
            ReprocessingPlanError::InvalidField("cache_sha256")
        );
        let mut selective_context = context(true);
        selective_context.cache_sha256 = None;
        assert_eq!(
            ReprocessingPlan::new(ReprocessingMode::Recluster, selective_context, 0.5).unwrap_err(),
            ReprocessingPlanError::InvalidField("cache_sha256")
        );
    }

    #[test]
    fn unknown_fields_and_nested_tampering_are_rejected() {
        let plan = plan(ReprocessingMode::Full);
        let mut unknown = serde_json::to_value(&plan).unwrap();
        unknown["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ReprocessingPlan>(unknown).is_err());

        let mut unknown_context = serde_json::to_value(&plan).unwrap();
        unknown_context["context"]["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ReprocessingPlan>(unknown_context).is_err());

        let mut unknown_work = serde_json::to_value(&plan).unwrap();
        unknown_work["required_work"]["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ReprocessingPlan>(unknown_work).is_err());
    }

    #[test]
    fn serialization_round_trip_and_revalidation_are_stable() {
        let plan = plan(ReprocessingMode::Rediarize);
        let encoded = serde_json::to_string(&plan).unwrap();
        let decoded: ReprocessingPlan = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, plan);
        assert_eq!(plan.revalidate(&plan.context, &plan.revision), Ok(()));
        assert_eq!(
            plan.revalidate(&plan.context, &sha('f')),
            Err(ReprocessingPlanError::RevisionMismatch)
        );
    }
}
