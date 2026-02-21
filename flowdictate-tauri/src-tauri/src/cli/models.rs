use clap::Args;

use crate::error::DictationError;
use crate::settings::{Language, WhisperModel};
use crate::transcription::model;

use super::transcribe::{model_id_string, parse_language, parse_model};

#[derive(Args)]
pub struct ListModelsArgs {
    /// Filter by language [possible values: en, sv, no, auto]
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

    Ok(())
}

pub async fn download(args: DownloadModelArgs) -> Result<(), DictationError> {
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
