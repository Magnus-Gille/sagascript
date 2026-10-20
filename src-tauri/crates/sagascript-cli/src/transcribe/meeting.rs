//! The scope `--meeting-json` allows (#343): one explicit regular file.

#[cfg(feature = "diarization")]
use std::path::PathBuf;

#[cfg(feature = "diarization")]
use sagascript_core::error::DictationError;
#[cfg(feature = "diarization")]
use sagascript_core::meeting::source::checked_meeting_metadata;

#[cfg(feature = "diarization")]
pub(super) fn validate_meeting_input_scope(
    original_inputs: &[PathBuf],
    recursive: bool,
    expanded: &[PathBuf],
) -> Result<(), DictationError> {
    if recursive || original_inputs.iter().any(|path| path.is_dir()) || expanded.len() != 1 {
        return Err(DictationError::SettingsError(
            "--meeting-json requires one explicit regular input file; directories, --recursive, and multiple inputs are unsupported"
                .to_string(),
        ));
    }
    let metadata = std::fs::metadata(&expanded[0]).map_err(|error| {
        DictationError::FileDecodeError(format!(
            "Failed to inspect meeting input '{}': {error}",
            expanded[0].display()
        ))
    })?;
    let _ = checked_meeting_metadata(metadata, "path")?;
    Ok(())
}
