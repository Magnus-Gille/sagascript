//! Which files a `transcribe` run works on: the inputs the caller named,
//! with directories expanded in a deterministic order (#343).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use sagascript_core::audio::decoder::SUPPORTED_EXTENSIONS;
use sagascript_core::error::DictationError;

pub(super) fn expand_inputs(
    inputs: &[PathBuf],
    recursive: bool,
) -> Result<Vec<PathBuf>, DictationError> {
    let mut expanded = Vec::new();
    let mut seen = HashSet::new();

    for input in inputs {
        if input.is_dir() {
            let mut directory_files = Vec::new();
            collect_directory_files(input, recursive, &mut directory_files)?;
            directory_files.sort();
            for file in directory_files {
                if seen.insert(file.clone()) {
                    expanded.push(file);
                }
            }
        } else if seen.insert(input.clone()) {
            // Keep explicit unsupported, missing, or corrupt files in the work
            // list so they produce an item-level error without hiding later
            // valid inputs.
            expanded.push(input.clone());
        }
    }

    Ok(expanded)
}

fn collect_directory_files(
    directory: &Path,
    recursive: bool,
    output: &mut Vec<PathBuf>,
) -> Result<(), DictationError> {
    let entries = std::fs::read_dir(directory).map_err(|error| {
        DictationError::FileDecodeError(format!(
            "Failed to read directory '{}': {error}",
            directory.display()
        ))
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            DictationError::FileDecodeError(format!(
                "Failed to read an entry in '{}': {error}",
                directory.display()
            ))
        })?;
        let file_type = entry.file_type().map_err(|error| {
            DictationError::FileDecodeError(format!(
                "Failed to inspect '{}': {error}",
                entry.path().display()
            ))
        })?;
        let path = entry.path();
        if file_type.is_file() && has_supported_extension(&path) {
            output.push(path);
        } else if recursive && file_type.is_dir() {
            collect_directory_files(&path, true, output)?;
        }
    }

    Ok(())
}

pub(super) fn has_supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}
