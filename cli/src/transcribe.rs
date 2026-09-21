use std::path::PathBuf;

use anyhow::{anyhow, bail};
use live_inquiry::{FakeTranscriptProvider, TranscriptProvider, TranscriptSource};

pub struct CoverArgs {
    pub template: PathBuf,
    pub transcript: Option<PathBuf>,
    pub audio: Option<PathBuf>,
    pub speech_backend: String,
    pub google_project: Option<String>,
    pub google_location: String,
    pub google_language: String,
    pub google_model: String,
    pub pretty: bool,
}

pub async fn cover(args: CoverArgs) -> anyhow::Result<()> {
    let output = match (args.transcript, args.audio) {
        (Some(transcript), None) => {
            live_inquiry::cover_transcript_file(&args.template, &transcript)?
        }
        (None, Some(audio)) => {
            let (provider, provider_label) = build_transcript_provider(
                &args.speech_backend,
                args.google_project,
                args.google_location,
                args.google_language,
                args.google_model,
            )
            .await?;
            let segments = provider.transcribe_file(&audio).await?;
            live_inquiry::cover_transcript_segments(
                &args.template,
                TranscriptSource::Audio {
                    path: audio.display().to_string(),
                    provider: provider_label,
                },
                segments,
            )?
        }
        (None, None) => bail!("pass either --transcript or --audio"),
        (Some(_), Some(_)) => bail!("pass only one of --transcript or --audio"),
    };

    if args.pretty {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("{}", serde_json::to_string(&output)?);
    }
    Ok(())
}

/// Select the speech backend for the `--audio` path. The default is `fake`,
/// which transcribes with no cloud call (see
/// [`live_inquiry::FakeTranscriptProvider`]); `google` opts into real Google
/// Speech-to-Text v2 and therefore requires a project and credentials.
///
/// Returns the provider plus the label recorded in the coverage JSON's
/// `transcript_source.provider`, so the output never claims a real
/// transcription when the fake produced it.
async fn build_transcript_provider(
    backend: &str,
    google_project: Option<String>,
    google_location: String,
    google_language: String,
    google_model: String,
) -> anyhow::Result<(Box<dyn TranscriptProvider>, String)> {
    match backend {
        "fake" => Ok((Box::new(FakeTranscriptProvider::new()), "fake".to_string())),
        "google" | "gcp" => {
            let project_id = google_project_id(google_project, |key| std::env::var(key).ok())
                .ok_or_else(|| {
                    anyhow!(
                        "GOOGLE_CLOUD_PROJECT, GCLOUD_PROJECT, NAVIGATOR_GCP_PROJECT_ID, or --google-project is required with --speech-backend google"
                    )
                })?;
            let mut config = cloud::GoogleSpeechConfig::new(project_id);
            config.location = google_location;
            config.language_code = google_language;
            config.model = google_model;
            let provider = cloud::GoogleSpeechTranscriptProvider::new_adc(config).await?;
            Ok((Box::new(provider), "google-speech-to-text-v2".to_string()))
        }
        other => bail!(
            "unknown speech backend {other:?}: expected 'fake' (default) or 'google' \
             (set --speech-backend or NAVIGATOR_SPEECH_BACKEND)"
        ),
    }
}

/// Google Cloud project for `--speech-backend google`. `--google-project`
/// wins; then `GOOGLE_CLOUD_PROJECT`, `GCLOUD_PROJECT`, and
/// `NAVIGATOR_GCP_PROJECT_ID`. Blank values are absences so an empty export
/// cannot satisfy the backend. `get` is the process environment in the CLI
/// and an injected map in tests.
fn google_project_id(
    explicit: Option<String>,
    get: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    [
        explicit,
        get("GOOGLE_CLOUD_PROJECT"),
        get("GCLOUD_PROJECT"),
        get("NAVIGATOR_GCP_PROJECT_ID"),
    ]
    .into_iter()
    .find_map(|value| {
        value.and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::google_project_id;

    fn get<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn google_project_id_prefers_flag_then_standard_env_then_navigator() {
        let env = get(&[
            ("GOOGLE_CLOUD_PROJECT", "from-google-cloud"),
            ("GCLOUD_PROJECT", "from-gcloud"),
            ("NAVIGATOR_GCP_PROJECT_ID", "from-navigator"),
        ]);
        assert_eq!(
            google_project_id(Some("from-flag".into()), &env),
            Some("from-flag".into())
        );
        assert_eq!(
            google_project_id(None, &env),
            Some("from-google-cloud".into())
        );
        assert_eq!(
            google_project_id(None, get(&[("GCLOUD_PROJECT", "from-gcloud")])),
            Some("from-gcloud".into())
        );
        assert_eq!(
            google_project_id(None, get(&[("NAVIGATOR_GCP_PROJECT_ID", "from-navigator")])),
            Some("from-navigator".into())
        );
        assert_eq!(google_project_id(None, get(&[])), None);
    }

    #[test]
    fn google_project_id_treats_blank_values_as_absent() {
        let env = get(&[
            ("GOOGLE_CLOUD_PROJECT", "   "),
            ("NAVIGATOR_GCP_PROJECT_ID", "from-navigator"),
        ]);
        assert_eq!(
            google_project_id(Some("  ".into()), &env),
            Some("from-navigator".into()),
            "a blank flag must fall through, and a blank GOOGLE_CLOUD_PROJECT must not win"
        );
    }
}
