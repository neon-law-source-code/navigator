//! Local-only E2E for the default (`fake`) speech backend.
//!
//! Unlike the Google E2E ([`transcribe_google_e2e`]), this test makes
//! NO cloud call and needs no credentials or opt-in env var: it exercises the
//! `--audio` path with the default `fake` backend, which transcribes from a
//! sidecar `<audio>.txt` file. This is the behavior a developer gets locally
//! without enabling real Speech-to-Text.

use std::fs;

use assert_cmd::Command;
use serde_json::Value;

#[test]
fn cli_demo_transcribes_audio_with_fake_backend_from_sidecar() {
    let temp = tempfile::tempdir().expect("create tempdir");
    let audio_path = temp.path().join("clip.flac");
    let sidecar_path = temp.path().join("clip.flac.txt");
    let template_path = temp.path().join("fake_speech.md");

    // The fake never reads the audio bytes; a placeholder file is enough.
    fs::write(&audio_path, b"not real audio").expect("write placeholder audio");
    fs::write(&sidecar_path, "yes I consent to recording this sitting\n")
        .expect("write sidecar transcript");

    fs::write(
        &template_path,
        r"---
title: Fake Speech
code: fake_speech
questionnaire:
  BEGIN:
    _: custom_yes_no__recording_consent
  custom_yes_no__recording_consent:
    _: END
  END: {}
---

# Fake Speech
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
            "--pretty",
        ])
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("stdout utf8");
    let json: Value = serde_json::from_str(&stdout).expect("coverage JSON");

    // The default backend is the fake — the coverage JSON must say so, never
    // claiming a real transcription happened.
    assert_eq!(json["transcript_source"]["kind"], "audio");
    assert_eq!(
        json["transcript_source"]["provider"], "fake",
        "default --audio path must report the fake provider, got: {}",
        json["transcript_source"]
    );
    assert_eq!(json["template_code"], "fake_speech");

    let transcript = json["transcript_text"]
        .as_str()
        .expect("transcript_text is a string");
    assert_eq!(
        transcript.trim(),
        "yes I consent to recording this sitting",
        "the fake should transcribe from the sidecar file"
    );

    let segments = json["transcript_segments"]
        .as_array()
        .expect("transcript_segments array");
    assert!(!segments.is_empty(), "expected at least one segment");

    let findings = json["findings"].as_array().expect("findings array");
    assert_eq!(findings.len(), 1, "template has one inquiry");
    assert_eq!(
        findings[0]["inquiry_code"],
        "custom_yes_no__recording_consent"
    );
}
