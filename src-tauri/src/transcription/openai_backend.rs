use reqwest::multipart;
use tracing::{error, info};

use crate::audio::wav::encode_wav;
use crate::credentials::KeyringService;
use crate::error::DictationError;
use crate::settings::Language;

use super::backend::TranscriptionBackend;

const API_URL: &str = "https://api.openai.com/v1/audio/transcriptions";
const MODEL: &str = "whisper-1";
const MAX_AUDIO_SIZE_BYTES: usize = 25 * 1024 * 1024;

pub struct OpenAIBackend {
    client: reqwest::Client,
    keyring: KeyringService,
}

impl OpenAIBackend {
    pub fn new(keyring: KeyringService) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("Failed to create HTTP client");

        Self { client, keyring }
    }
}

#[async_trait::async_trait]
impl TranscriptionBackend for OpenAIBackend {
    async fn is_ready(&self) -> bool {
        self.keyring.get_api_key().is_some()
    }

    async fn warm_up(&self) -> Result<(), DictationError> {
        if self.keyring.get_api_key().is_none() {
            return Err(DictationError::ApiKeyMissing);
        }
        Ok(())
    }

    async fn transcribe(&self, audio: &[f32], language: Language) -> Result<String, DictationError> {
        let api_key = self
            .keyring
            .get_api_key()
            .ok_or(DictationError::ApiKeyMissing)?;

        if audio.is_empty() {
            return Err(DictationError::NoAudioCaptured);
        }

        info!("Starting remote transcription of {} samples", audio.len());

        let wav_data = encode_wav(audio);

        if wav_data.len() > MAX_AUDIO_SIZE_BYTES {
            let size_mb = wav_data.len() as f64 / (1024.0 * 1024.0);
            return Err(DictationError::TranscriptionFailed(format!(
                "Audio file too large ({size_mb:.1}MB). Maximum is 25MB."
            )));
        }

        let file_part = multipart::Part::bytes(wav_data)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .unwrap();

        let mut form = multipart::Form::new()
            .text("model", MODEL)
            .part("file", file_part);

        if let Some(code) = language.whisper_code() {
            form = form.text("language", code.to_string());
        }

        let response = self
            .client
            .post(API_URL)
            .bearer_auth(&api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| DictationError::NetworkError(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            error!("OpenAI API error: {status} - {body}");
            if status.as_u16() == 401 {
                return Err(DictationError::ApiKeyMissing);
            }
            return Err(DictationError::NetworkError(format!("API error: {status}")));
        }

        #[derive(serde::Deserialize)]
        struct TranscriptionResponse {
            text: String,
        }

        let result: TranscriptionResponse = response
            .json()
            .await
            .map_err(|e| DictationError::TranscriptionFailed(format!("Failed to parse response: {e}")))?;

        info!("Remote transcription complete: {} chars", result.text.len());
        Ok(result.text.trim().to_string())
    }
}
