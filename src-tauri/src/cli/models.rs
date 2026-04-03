use clap::Args;

use crate::error::DictationError;
use crate::settings::{Language, WhisperModel};
use crate::transcription::model;

use super::transcribe::{model_id_string, parse_language, parse_model};

#[derive(Args)]
pub struct ListModelsArgs {
    /// Filter by language [possible values: en, sv, no, auto (less accurate)]
    #[arg(short, long, value_name = "LANG")]
    pub language: Option<String>,
}

#[derive(Args)]
pub struct DownloadModelArgs {
    /// Model ID to download [see: sagascript list-models]
    pub model: String,
}

pub fn list(args: ListModelsArgs) -> Result<(), DictationError> {
    let languages: Vec<Language> = if let Some(lang_str) = &args.language {
        vec![parse_language(lang_str)?]
    } else {
        vec![
            Language::English,
            Language::Swedish,
            Language::Norwegian,
            Language::Auto,
        ]
    };

    // Header
    println!(
        "{:<20} {:<10} {:<8} {:<12} {:<12}",
        "MODEL ID", "NAME", "SIZE", "DOWNLOADED", "LANGUAGE"
    );
    println!("{}", "-".repeat(62));

    for lang in &languages {
        let models = WhisperModel::models_for_language(*lang);
        for &m in models {
            let downloaded = if model::is_model_downloaded(m) {
                "yes"
            } else {
                "no"
            };

            println!(
                "{:<20} {:<10} {:>5} MB  {:<12} {:<12}",
                model_id_string(m),
                m.display_name(),
                m.size_mb(),
                downloaded,
                lang.display_name(),
            );
        }
    }

    // Diarization models section (only when no language filter, or always show)
    #[cfg(feature = "diarization")]
    if args.language.is_none() {
        use crate::diarization::model as diar_model;
        use crate::diarization::model::DiarizationModel;

        println!();
        println!("Diarization models (speaker identification):");
        println!("{}", "-".repeat(62));

        for &m in DiarizationModel::ALL {
            let downloaded = if diar_model::is_model_downloaded(m) {
                "yes"
            } else {
                "no"
            };

            println!(
                "{:<20} {:<10} {:>5} MB  {:<12} {:<12}",
                m.model_id(),
                m.display_name(),
                m.size_mb(),
                downloaded,
                "—",
            );
        }
    }

    Ok(())
}

#[derive(Args)]
pub struct DeleteModelArgs {
    /// Model ID to delete [see: sagascript list-models]
    pub model: String,
}

pub fn delete(args: DeleteModelArgs) -> Result<(), DictationError> {
    let whisper_model = parse_model(&args.model)?;

    if !model::is_model_downloaded(whisper_model) {
        eprintln!(
            "Model '{}' is not downloaded.",
            whisper_model.display_name()
        );
        return Ok(());
    }

    let path = model::model_path(whisper_model);
    std::fs::remove_file(&path).map_err(|e| {
        DictationError::SettingsError(format!(
            "Failed to delete model file '{}': {e}",
            path.display()
        ))
    })?;

    eprintln!(
        "Deleted {} ({})",
        whisper_model.display_name(),
        path.display()
    );
    Ok(())
}

pub async fn download(args: DownloadModelArgs) -> Result<(), DictationError> {
    // Try diarization model IDs first (when feature is enabled)
    #[cfg(feature = "diarization")]
    {
        use crate::diarization::model::DiarizationModel;

        // "diarization" meta-ID downloads both models
        if DiarizationModel::is_meta_id(&args.model) {
            for &m in DiarizationModel::ALL {
                download_diarization_model(m).await?;
            }
            return Ok(());
        }

        if let Some(diar) = DiarizationModel::from_id(&args.model) {
            return download_diarization_model(diar).await;
        }
    }

    let whisper_model = parse_model(&args.model)?;

    if model::is_model_downloaded(whisper_model) {
        let path = model::model_path(whisper_model);
        eprintln!(
            "Model '{}' is already downloaded at {}",
            whisper_model.display_name(),
            path.display()
        );
        println!("{}", path.display());
        return Ok(());
    }

    eprintln!(
        "Downloading {} (~{} MB)...",
        whisper_model.display_name(),
        whisper_model.size_mb()
    );

    let path = model::download_model(whisper_model, |downloaded, total| {
        if total > 0 {
            let pct = (downloaded as f64 / total as f64 * 100.0) as u32;
            let mb_done = downloaded as f64 / 1_048_576.0;
            let mb_total = total as f64 / 1_048_576.0;
            eprint!("\r  {:.1}/{:.1} MB ({pct}%)", mb_done, mb_total);
        } else {
            let mb_done = downloaded as f64 / 1_048_576.0;
            eprint!("\r  {:.1} MB downloaded", mb_done);
        }
    })
    .await?;

    eprintln!(); // newline after progress
    eprintln!("Download complete.");
    println!("{}", path.display());
    Ok(())
}

#[cfg(feature = "diarization")]
async fn download_diarization_model(
    model: crate::diarization::model::DiarizationModel,
) -> Result<(), DictationError> {
    use crate::diarization::model as diar_model;

    if diar_model::is_model_downloaded(model) {
        let path = diar_model::model_path(model);
        eprintln!(
            "Model '{}' is already downloaded at {}",
            model.display_name(),
            path.display()
        );
        println!("{}", path.display());
        return Ok(());
    }

    eprintln!(
        "Downloading {} (~{} MB)...",
        model.display_name(),
        model.size_mb()
    );

    let path = diar_model::download_model(model, |downloaded, total| {
        if total > 0 {
            let pct = (downloaded as f64 / total as f64 * 100.0) as u32;
            let mb_done = downloaded as f64 / 1_048_576.0;
            let mb_total = total as f64 / 1_048_576.0;
            eprint!("\r  {:.1}/{:.1} MB ({pct}%)", mb_done, mb_total);
        } else {
            let mb_done = downloaded as f64 / 1_048_576.0;
            eprint!("\r  {:.1} MB downloaded", mb_done);
        }
    })
    .await?;

    eprintln!(); // newline after progress
    eprintln!("Download complete.");
    println!("{}", path.display());
    Ok(())
}
