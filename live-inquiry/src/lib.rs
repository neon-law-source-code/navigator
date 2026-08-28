use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[async_trait]
pub trait TranscriptProvider: Send + Sync {
    async fn transcribe_file(&self, audio: &Path) -> anyhow::Result<Vec<TranscriptSegment>>;
}

/// A [`TranscriptProvider`] that produces a deterministic transcript without
/// calling any cloud speech API.
///
/// This is the **default** backend for local dev and tests; real Google
/// Speech-to-Text is opt-in (`NAVIGATOR_SPEECH_BACKEND=google`). Keeping the
/// fake here in `live-inquiry` — which has no cloud-provider dependency — means
/// the default `--audio` path compiles and runs with no GCP SDK, credentials,
/// or network access.
///
/// `transcribe_file` resolves its text in this order:
/// 1. a fixed transcript supplied via [`FakeTranscriptProvider::with_transcript`];
/// 2. a sidecar text file next to the audio (`<audio path>.txt`), so a developer
///    can script a realistic offline transcript for a given clip;
/// 3. a deterministic placeholder derived from the file name, so output that
///    slips through unconfigured is visibly synthetic rather than plausible.
#[derive(Debug, Default, Clone)]
pub struct FakeTranscriptProvider {
    fixed: Option<String>,
}

impl FakeTranscriptProvider {
    /// A fake that reads a sidecar `<audio>.txt` when present, else emits a
    /// labelled placeholder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A fake that always returns `transcript`, ignoring the audio path. Useful
    /// for scripted unit tests of the coverage engine.
    #[must_use]
    pub fn with_transcript(transcript: impl Into<String>) -> Self {
        Self {
            fixed: Some(transcript.into()),
        }
    }

    fn resolve(&self, audio: &Path) -> String {
        if let Some(text) = &self.fixed {
            return text.clone();
        }
        let sidecar = sidecar_transcript_path(audio);
        if let Ok(text) = std::fs::read_to_string(&sidecar) {
            if !text.trim().is_empty() {
                return text;
            }
        }
        let name = audio
            .file_name()
            .map_or_else(|| "audio".to_string(), |n| n.to_string_lossy().into_owned());
        format!("fake transcript for {name}")
    }
}

#[async_trait]
impl TranscriptProvider for FakeTranscriptProvider {
    async fn transcribe_file(&self, audio: &Path) -> anyhow::Result<Vec<TranscriptSegment>> {
        Ok(segment_transcript(&self.resolve(audio)))
    }
}

/// The sidecar transcript path the fake reads for a given audio file: the audio
/// path with `.txt` appended (e.g. `clip.flac` -> `clip.flac.txt`).
#[must_use]
pub fn sidecar_transcript_path(audio: &Path) -> PathBuf {
    let mut raw = audio.as_os_str().to_owned();
    raw.push(".txt");
    PathBuf::from(raw)
}

#[derive(Debug, Deserialize)]
struct TemplateFrontmatter {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    questionnaire: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize)]
struct Records<T> {
    records: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct QuestionSeed {
    code: String,
    prompt: String,
    #[serde(default)]
    question_type: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Inquiry {
    pub code: String,
    pub prompt: String,
    pub answer_type: String,
    pub source: InquirySource,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InquirySource {
    TemplateQuestion {
        template_code: String,
        question_code: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscriptSegment {
    pub id: String,
    pub provider_sequence: usize,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    LikelyAnswered,
    NeedsFollowUp,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoverageFinding {
    pub inquiry_code: String,
    pub status: CoverageStatus,
    pub confidence: f32,
    pub proposed_answer: Option<String>,
    pub evidence_segment_ids: Vec<String>,
    pub follow_up_prompt: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CoverageOutput {
    pub template_code: String,
    pub transcript_source: TranscriptSource,
    pub transcript_text: String,
    pub inquiries: Vec<Inquiry>,
    pub transcript_segments: Vec<TranscriptSegment>,
    pub findings: Vec<CoverageFinding>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TranscriptSource {
    Audio { path: String, provider: String },
    TranscriptFile { path: String },
}

pub struct LoadedTemplate {
    pub code: String,
    questionnaire: BTreeMap<String, BTreeMap<String, String>>,
}

pub fn cover_transcript_file(template: &Path, transcript: &Path) -> anyhow::Result<CoverageOutput> {
    let transcript_text = std::fs::read_to_string(transcript)
        .with_context(|| format!("read transcript {}", transcript.display()))?;
    cover_text(
        load_template(template)?,
        &transcript_text,
        TranscriptSource::TranscriptFile {
            path: transcript.display().to_string(),
        },
    )
}

pub fn cover_transcript_segments(
    template: &Path,
    transcript_source: TranscriptSource,
    transcript_segments: Vec<TranscriptSegment>,
) -> anyhow::Result<CoverageOutput> {
    let transcript_text = transcript_segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    cover_segments(
        load_template(template)?,
        &transcript_text,
        transcript_source,
        transcript_segments,
    )
}

pub fn cover_transcript_text(
    template: &Path,
    transcript_text: &str,
    transcript_source: TranscriptSource,
) -> anyhow::Result<CoverageOutput> {
    cover_text(load_template(template)?, transcript_text, transcript_source)
}

/// Run coverage for a template already resolved in memory (the DB template
/// body) rather than a file on disk — the server-side entry point the intake
/// walk drives so the CLI stays a thin HTTP client.
pub fn cover_text(
    template: LoadedTemplate,
    transcript_text: &str,
    transcript_source: TranscriptSource,
) -> anyhow::Result<CoverageOutput> {
    let transcript_segments = segment_transcript(transcript_text);
    cover_segments(
        template,
        transcript_text,
        transcript_source,
        transcript_segments,
    )
}

fn cover_segments(
    template: LoadedTemplate,
    transcript_text: &str,
    transcript_source: TranscriptSource,
    transcript_segments: Vec<TranscriptSegment>,
) -> anyhow::Result<CoverageOutput> {
    let questions = load_question_registry()?;
    let inquiries = normalize_inquiries(&template, &questions)?;
    let findings = evaluate_coverage(&inquiries, &transcript_segments, transcript_text);
    Ok(CoverageOutput {
        template_code: template.code,
        transcript_source,
        transcript_text: transcript_text.to_string(),
        inquiries,
        transcript_segments,
        findings,
    })
}

pub fn load_template(path: &Path) -> anyhow::Result<LoadedTemplate> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read template {}", path.display()))?;
    let fallback_code = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("untitled");
    load_template_from_str(&raw, fallback_code)
        .with_context(|| format!("template {}", path.display()))
}

/// Build a [`LoadedTemplate`] from a template body already in memory (the DB
/// template body). The frontmatter `code` wins; `fallback_code` names the
/// template only when the frontmatter omits it (the notation's template code).
pub fn load_template_from_str(body: &str, fallback_code: &str) -> anyhow::Result<LoadedTemplate> {
    let frontmatter =
        rules::frontmatter::extract(body).ok_or_else(|| anyhow!("template has no frontmatter"))?;
    let parsed: TemplateFrontmatter =
        serde_yaml::from_str(frontmatter).context("parse template frontmatter")?;
    Ok(LoadedTemplate {
        code: parsed.code.unwrap_or_else(|| fallback_code.to_string()),
        questionnaire: parsed.questionnaire,
    })
}

fn load_question_registry() -> anyhow::Result<BTreeMap<String, QuestionSeed>> {
    const QUESTION_YAML: &str = include_str!("../../store/seeds/Question.yaml");
    let parsed: Records<QuestionSeed> =
        serde_yaml::from_str(QUESTION_YAML).context("parse canonical Question.yaml")?;
    Ok(parsed
        .records
        .into_iter()
        .map(|q| (q.code.clone(), q))
        .collect())
}

fn normalize_inquiries(
    template: &LoadedTemplate,
    questions: &BTreeMap<String, QuestionSeed>,
) -> anyhow::Result<Vec<Inquiry>> {
    let mut current = "BEGIN".to_string();
    let mut seen = BTreeSet::new();
    let mut inquiries = Vec::new();
    while current != "END" {
        if !seen.insert(current.clone()) {
            bail!("questionnaire cycle at `{current}`");
        }
        let transitions = template
            .questionnaire
            .get(&current)
            .ok_or_else(|| anyhow!("questionnaire state `{current}` has no transitions"))?;
        let next = transitions
            .get("_")
            .or_else(|| transitions.values().next())
            .ok_or_else(|| anyhow!("questionnaire state `{current}` has no next state"))?;
        if next == "END" {
            break;
        }
        let Some(question) = question_for_state(next, questions) else {
            bail!("questionnaire references unknown question `{next}`");
        };
        inquiries.push(Inquiry {
            code: next.clone(),
            prompt: question.prompt.clone(),
            answer_type: question
                .question_type
                .clone()
                .unwrap_or_else(|| "string".to_string()),
            source: InquirySource::TemplateQuestion {
                template_code: template.code.clone(),
                question_code: next.clone(),
            },
        });
        current = next.clone();
    }
    Ok(inquiries)
}

fn question_for_state<'a>(
    state: &str,
    questions: &'a BTreeMap<String, QuestionSeed>,
) -> Option<&'a QuestionSeed> {
    questions.get(state).or_else(|| {
        let (question_type, _) = state.split_once("__")?;
        questions.get(question_type)
    })
}

pub fn segment_transcript(transcript: &str) -> Vec<TranscriptSegment> {
    let mut chunks: Vec<String> = transcript
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if chunks.len() <= 1 {
        chunks = sentence_chunks(transcript);
    }
    chunks
        .into_iter()
        .enumerate()
        .map(|(idx, text)| TranscriptSegment {
            id: format!("segment_{}", idx + 1),
            provider_sequence: idx + 1,
            text,
        })
        .collect()
}

fn sentence_chunks(transcript: &str) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut start = 0;
    for (idx, ch) in transcript.char_indices() {
        if matches!(ch, '.' | '?' | '!') {
            let end = idx + ch.len_utf8();
            let chunk = transcript[start..end].trim();
            if !chunk.is_empty() {
                chunks.push(chunk.to_string());
            }
            start = end;
        }
    }
    let tail = transcript[start..].trim();
    if !tail.is_empty() {
        chunks.push(tail.to_string());
    }
    if chunks.is_empty() && !transcript.trim().is_empty() {
        chunks.push(transcript.trim().to_string());
    }
    chunks
}

fn evaluate_coverage(
    inquiries: &[Inquiry],
    segments: &[TranscriptSegment],
    transcript: &str,
) -> Vec<CoverageFinding> {
    inquiries
        .iter()
        .map(|inquiry| {
            let (answer, evidence_segment_ids) =
                proposed_answer(&inquiry.code, transcript, segments);
            let status = if answer.is_some() {
                CoverageStatus::LikelyAnswered
            } else {
                CoverageStatus::NeedsFollowUp
            };
            CoverageFinding {
                inquiry_code: inquiry.code.clone(),
                status,
                confidence: if status == CoverageStatus::LikelyAnswered {
                    0.74
                } else {
                    0.2
                },
                proposed_answer: answer,
                evidence_segment_ids,
                follow_up_prompt: (status == CoverageStatus::NeedsFollowUp)
                    .then(|| inquiry.prompt.clone()),
            }
        })
        .collect()
}

fn proposed_answer(
    code: &str,
    transcript: &str,
    segments: &[TranscriptSegment],
) -> (Option<String>, Vec<String>) {
    let lower = transcript.to_lowercase();
    if prompt_key(code) == "recording_consent" && lower.contains("consent") {
        return (
            Some("Yes".to_string()),
            evidence_segments(segments, &["consent"], None),
        );
    }
    for label in labels_for(prompt_key(code)) {
        if let Some(value) = value_after_label(&lower, transcript, label) {
            return (
                Some(value.clone()),
                evidence_segments(segments, &[*label], Some(&value)),
            );
        }
    }
    (None, Vec::new())
}

fn prompt_key(code: &str) -> &str {
    code.rsplit_once("__").map_or(code, |(_, key)| key)
}

fn labels_for(code: &str) -> &'static [&'static str] {
    match code {
        "testator_name" => &["testator", "full legal name", "my name is"],
        "executor_name" => &["executor"],
        "successor_trustee" => &["successor trustee", "trustee"],
        "guardian_for_minors" => &["guardian"],
        "residuary_beneficiary" => &["residuary beneficiary", "beneficiary"],
        "healthcare_agent" => &["health-care agent", "healthcare agent", "health care agent"],
        "financial_agent" => &["financial agent"],
        "settlement_terms" => &["settlement terms"],
        _ => &[],
    }
}

fn value_after_label(lower: &str, transcript: &str, label: &str) -> Option<String> {
    let at = lower.find(label)?;
    let mut start = at + label.len();
    let lower_tail = lower[start..].trim_start();
    let trimmed = lower[start..].len() - lower_tail.len();
    start += trimmed;
    for prefix in [
        ":",
        "-",
        "is going to be",
        "will be",
        "would be",
        "should be",
        "is",
    ] {
        let tail = lower[start..].trim_start();
        if let Some(stripped) = tail.strip_prefix(prefix) {
            start = lower.len() - stripped.len();
            break;
        }
    }
    let tail = transcript[start..].trim_start_matches([' ', ':', '-']);
    let end = tail.find(['.', ';', '\n']).unwrap_or(tail.len());
    let value = tail[..end].trim().trim_matches('"');
    (!value.is_empty()).then(|| value.to_string())
}

fn evidence_segments(
    segments: &[TranscriptSegment],
    labels: &[&str],
    answer: Option<&str>,
) -> Vec<String> {
    let answer_lower = answer.map(str::to_lowercase);
    segments
        .iter()
        .filter(|segment| {
            let lower = segment.text.to_lowercase();
            labels.iter().any(|label| lower.contains(label))
                || answer_lower
                    .as_ref()
                    .is_some_and(|value| lower.contains(value))
        })
        .map(|segment| segment.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_provider_uses_fixed_transcript_when_provided() {
        let provider = FakeTranscriptProvider::with_transcript("yes I consent. my name is Ada.");
        // The audio path is ignored when a fixed transcript is set.
        let text = provider.resolve(Path::new("/does/not/exist.flac"));
        assert_eq!(text, "yes I consent. my name is Ada.");
        let segments = segment_transcript(&text);
        assert_eq!(segments.len(), 2, "two sentences -> two segments");
    }

    #[test]
    fn fake_provider_reads_sidecar_transcript_when_present() {
        let dir = std::env::temp_dir().join(format!("live-inquiry-fake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let audio = dir.join("clip.flac");
        std::fs::write(
            sidecar_transcript_path(&audio),
            "the river is wide and old\n",
        )
        .unwrap();

        let text = FakeTranscriptProvider::new().resolve(&audio);
        assert_eq!(text.trim(), "the river is wide and old");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fake_provider_falls_back_to_labelled_placeholder() {
        // No fixed text, no sidecar file next to this path.
        let text = FakeTranscriptProvider::new().resolve(Path::new("/tmp/missing-clip.flac"));
        assert_eq!(text, "fake transcript for missing-clip.flac");
    }

    #[test]
    fn normalizes_estate_questionnaire_into_inquiries() {
        let template = LoadedTemplate {
            code: "sitting__transcript".to_string(),
            questionnaire: serde_yaml::from_str(
                r"
BEGIN:
  _: custom_yes_no__recording_consent
custom_yes_no__recording_consent:
  _: person__testator
person__testator:
  _: END
END: {}
",
            )
            .unwrap(),
        };
        let questions = load_question_registry().unwrap();

        let inquiries = normalize_inquiries(&template, &questions).unwrap();

        assert_eq!(
            inquiries
                .iter()
                .map(|inquiry| inquiry.code.as_str())
                .collect::<Vec<_>>(),
            vec!["custom_yes_no__recording_consent", "person__testator"]
        );
        assert_eq!(inquiries[1].prompt, "Who is {{for_label}}?");
    }

    #[test]
    fn cover_text_runs_against_an_in_memory_template_body() {
        // The server resolves a notation's template from the DB, not a file, so
        // coverage must run from a body string. A transcript that mentions
        // consent covers the recording-consent inquiry; an inquiry the
        // transcript never touches still needs follow-up.
        let body = "---\ncode: sitting__transcript\nquestionnaire:\n  BEGIN:\n    _: custom_yes_no__recording_consent\n  custom_yes_no__recording_consent:\n    _: person__testator\n  person__testator:\n    _: END\n  END: {}\n---\nBody prose.\n";
        let template = load_template_from_str(body, "fallback").unwrap();
        assert_eq!(template.code, "sitting__transcript");

        let output = cover_text(
            template,
            "The client gave their consent to record.",
            TranscriptSource::TranscriptFile {
                path: "sitting.txt".to_string(),
            },
        )
        .unwrap();

        let consent = output
            .findings
            .iter()
            .find(|finding| finding.inquiry_code == "custom_yes_no__recording_consent")
            .expect("consent inquiry present");
        assert_eq!(consent.status, CoverageStatus::LikelyAnswered);
        assert_eq!(consent.proposed_answer.as_deref(), Some("Yes"));

        let testator = output
            .findings
            .iter()
            .find(|finding| finding.inquiry_code == "person__testator")
            .expect("testator inquiry present");
        assert_eq!(testator.status, CoverageStatus::NeedsFollowUp);
        assert!(testator.proposed_answer.is_none());
    }

    #[test]
    fn coverage_finds_answers_and_evidence_segments() {
        let inquiries = vec![
            Inquiry {
                code: "custom_yes_no__recording_consent".to_string(),
                prompt: "Do you consent to recording this sitting?".to_string(),
                answer_type: "yes_no".to_string(),
                source: InquirySource::TemplateQuestion {
                    template_code: "sitting__transcript".to_string(),
                    question_code: "custom_yes_no__recording_consent".to_string(),
                },
            },
            Inquiry {
                code: "custom_text__settlement_terms".to_string(),
                prompt: "What settlement terms would resolve this dispute?".to_string(),
                answer_type: "string".to_string(),
                source: InquirySource::TemplateQuestion {
                    template_code: "onboarding__settlement".to_string(),
                    question_code: "custom_text__settlement_terms".to_string(),
                },
            },
            Inquiry {
                code: "custom_text__disputed_reason".to_string(),
                prompt: "Why do you dispute the claim?".to_string(),
                answer_type: "string".to_string(),
                source: InquirySource::TemplateQuestion {
                    template_code: "onboarding__settlement".to_string(),
                    question_code: "custom_text__disputed_reason".to_string(),
                },
            },
        ];
        let transcript =
            "I consent to recording this sitting.\nSettlement terms will be mutual release.";
        let segments = segment_transcript(transcript);

        let findings = evaluate_coverage(&inquiries, &segments, transcript);

        assert_eq!(findings[0].status, CoverageStatus::LikelyAnswered);
        assert_eq!(
            findings[1].proposed_answer.as_deref(),
            Some("mutual release")
        );
        assert_eq!(findings[1].evidence_segment_ids, vec!["segment_2"]);
        assert_eq!(findings[2].status, CoverageStatus::NeedsFollowUp);
        assert!(findings[2].follow_up_prompt.is_some());
    }
}
