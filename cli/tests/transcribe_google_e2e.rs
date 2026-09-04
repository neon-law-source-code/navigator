#![allow(clippy::doc_markdown)]
//! Live Google Speech-to-Text E2E for the CLI demo path.
//!
//! This is intentionally opt-in and self-skips green unless
//! `NAVIGATOR_RUN_LIVE_SPEECH_E2E=1` is set. It downloads Google's public
//! Brooklyn Bridge speech sample, runs the actual `navigator` binary through
//! the `transcribe --audio` command, and asserts that Google STT
//! returns the expected transcript text in the coverage JSON.
//!
//! Run locally with a gitignored `.env` and Application Default Credentials:
//!
//! ```bash
//! set -a; source .env; set +a
//! NAVIGATOR_RUN_LIVE_SPEECH_E2E=1 cargo test -p cli --test transcribe_google_e2e -- --nocapture
//! ```
//!
//! Required env:
//! - `GOOGLE_CLOUD_PROJECT`, `GCLOUD_PROJECT`, or
//!   `NAVIGATOR_GCP_PROJECT_ID`.
//! - Google Application Default Credentials that can call Speech-to-Text v2.
//! - Cloud Speech-to-Text API enabled for that project. The opted-in test
//!   fails loudly when Google returns `SERVICE_DISABLED`, because that is real
//!   environment drift rather than a missing local fixture.

use std::fs;

use assert_cmd::Command;
use serde_json::Value;

const BROOKLYN_BRIDGE_AUDIO: &str =
    "https://storage.googleapis.com/cloud-samples-data/speech/brooklyn_bridge.flac";

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

fn google_project_id() -> Option<String> {
    env("GOOGLE_CLOUD_PROJECT")
        .or_else(|| env("GCLOUD_PROJECT"))
        .or_else(|| env("NAVIGATOR_GCP_PROJECT_ID"))
}

#[tokio::test]
async fn cli_demo_transcribes_audio_with_google_speech() {
    if std::env::var("NAVIGATOR_RUN_LIVE_SPEECH_E2E").is_err() {
        eprintln!("skipping live Google Speech E2E; set NAVIGATOR_RUN_LIVE_SPEECH_E2E=1 to run");
        return;
    }

    let Some(project_id) = google_project_id() else {
        eprintln!(
            "skipping: GOOGLE_CLOUD_PROJECT, GCLOUD_PROJECT, or NAVIGATOR_GCP_PROJECT_ID is unset"
        );
        return;
    };

    let temp = tempfile::tempdir().expect("create tempdir");
    let audio_path = temp.path().join("brooklyn_bridge.flac");
    let template_path = temp.path().join("live_speech_e2e.md");

    let audio = reqwest::get(BROOKLYN_BRIDGE_AUDIO)
        .await
        .expect("download public Google speech sample")
        .error_for_status()
        .expect("Google speech sample returns success")
        .bytes()
        .await
        .expect("read Google speech sample bytes");
    fs::write(&audio_path, audio).expect("write speech sample");

    fs::write(
        &template_path,
        r"---
title: Live Speech E2E
code: live_speech_e2e
questionnaire:
  BEGIN:
    _: custom_yes_no__recording_consent
  custom_yes_no__recording_consent:
    _: END
  END: {}
---

# Live Speech E2E
",
    )
    .expect("write template");

    let assert = Command::cargo_bin("navigator")
        .expect("navigator binary")
        .args([
            "notations",
            "transcribe",
            "--audio",
            audio_path.to_str().expect("audio path utf8"),
            "--template",
            template_path.to_str().expect("template path utf8"),
            "--speech-backend",
            "google",
            "--google-project",
            &project_id,
            "--pretty",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("stdout utf8");
    let json: Value = serde_json::from_str(&stdout).expect("coverage JSON");
    assert_eq!(
        json["transcript_source"]["kind"], "audio",
        "the CLI should report that Google transcribed an audio source"
    );
    assert_eq!(json["template_code"], "live_speech_e2e");

    let transcript = json["transcript_text"]
        .as_str()
        .expect("transcript_text is a string")
        .to_lowercase();
    assert!(
        transcript.contains("brooklyn") && transcript.contains("bridge"),
        "expected Brooklyn Bridge transcript, got: {transcript:?}"
    );

    let findings = json["findings"].as_array().expect("findings array");
    assert_eq!(findings.len(), 1, "template has one inquiry");
    assert_eq!(
        findings[0]["inquiry_code"],
        "custom_yes_no__recording_consent"
    );
}
