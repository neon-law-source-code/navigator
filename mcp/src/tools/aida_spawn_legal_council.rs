//! `aida_spawn_legal_council` MCP tool.
//!
//! Returns the Legal Council brief — the twelve-lawyer review pattern
//! documented in `docs/agent-decision-councils.md` — packaged as a
//! prompt the calling LLM can run against a draft. The bench is a *council*
//! (c-o-u-n-c-i-l — a group) of the firm's *counsels* (c-o-u-n-s-e-l —
//! the attorneys): a council of counsels. AIDA is the agent that
//! carries the tool, not the name of the council. The server does not
//! call an LLM; it ships the canonical personas + the user's draft and
//! lets the model on the other end do the synthesis.
//!
//! Capricorn (managing partner) leads the bench and Scorpio (ethics
//! counsel) sharpens. By default only those two voices are convened —
//! pass `full: true` for all twelve.
//!
//! Conversational use: a drafter (lawyer or paralegal in a chat
//! interface) pastes copy that will *become* a notation — a template
//! body, a questionnaire prompt, an engagement-letter paragraph —
//! and the model uses the returned brief to produce voices +
//! revised copy. No database state changes — safe to invoke
//! speculatively.

use std::fmt::Write as _;

use serde::Deserialize;
use serde_json::{json, Value};

use super::ToolError;

/// One voice on the bench. Stable across invocations; the value
/// compounds when the cast stays the same.
struct Voice {
    sign: &'static str,
    glyph: &'static str,
    role: &'static str,
    stance: &'static str,
}

/// The canonical twelve voices, ordered with Capricorn first
/// (leader of the bench), then Scorpio (the default sharpener),
/// then Aries → Pisces. The MCP brief and `docs/agent-decision-councils.md`
/// agree on this cast — if you reorder one, reorder the other.
const VOICES: &[Voice] = &[
    Voice {
        sign: "Capricorn",
        glyph: "♑",
        role: "Managing Partner / Senior Counsel — institutional memory",
        stance: "Leader of the bench; speaks first. What does the bar's ethics opinion say? \
                 What did we promise the regulator last year? What language has failed in the \
                 firm's history? Favor convention over cleverness.",
    },
    Voice {
        sign: "Scorpio",
        glyph: "♏",
        role: "Ethics & Compliance Counsel — cut to the core",
        stance: "What is the one fiduciary duty everything else rests on? Where does the draft \
                 silently invite a conflict, a UPL violation, or a duty of candor problem? \
                 Name the load-bearing trust claim.",
    },
    Voice {
        sign: "Aries",
        glyph: "♈",
        role: "Trial Attorney (plaintiff or defense) — lead with the harm",
        stance: "Who is being injured, and by whom, if this language fails? Don't bury the \
                 stakes behind procedure. Name the worst plausible outcome first — whether the \
                 harm runs toward the plaintiff or the defendant.",
    },
    Voice {
        sign: "Taurus",
        glyph: "♉",
        role: "General Business Attorney (transactional + business-litigation lens) — \
               make it operative",
        stance: "Business work is both sides of the contract lifecycle: form the entity, draft \
                 the deal, and imagine the demand letter and the complaint that follow if the \
                 deal breaks. If a clerk \
                 could not execute on this sentence — file the articles, send the demand \
                 letter, sign the agreement — it isn't drafting yet, it's a wish. Operative \
                 verb, trigger, consideration, date.",
    },
    Voice {
        sign: "Gemini",
        glyph: "♊",
        role: "Appellate Attorney — notice the duality",
        stance: "The same statutory term carries two meanings; the same fact pattern reads two \
                 ways. Where is the draft secretly overloaded? Where will a reviewing court \
                 find ambiguity?",
    },
    Voice {
        sign: "Cancer",
        glyph: "♋",
        role: "Legal Aid / Tenant-Defense Attorney — empathy for the reader",
        stance: "The Person reading this is going through deep struggles — fighting an eviction, \
                 navigating a benefits cutoff, reading on a phone at 2 a.m. between shifts. They \
                 may not speak English natively. They are bold enough to be here. What do they \
                 see first? What confuses them? Ask the dumb question on purpose, and address \
                 them as the rights-fighter they already are.",
    },
    Voice {
        sign: "Leo",
        glyph: "♌",
        role: "Immigration Defense Attorney — boldly fight for the right to stay",
        stance: "Hostile-terrain advocacy: removal court, asylum credible-fear, hardship \
                 narratives, ICE detention. Speak boldly for the client whose right to remain \
                 is on the line; the lion does not flinch from unpopular cases. The story is \
                 the brief — tell it in the cadence the family will repeat at dinner.",
    },
    Voice {
        sign: "Virgo",
        glyph: "♍",
        role: "Tax Attorney — exacting precision",
        stance: "Exact section cite (IRC, NRS Chapter 363, NAC Chapter 372). Exact deadline \
                 (April 15, the quarterly NV Department of Taxation due dates, the \
                 annual-report anniversary). Exact form, exact schedule, exact attachment. \
                 The rule that triggers a notice of deficiency if the draft is sloppy. Strike \
                 imprecise verbs.",
    },
    Voice {
        sign: "Libra",
        glyph: "♎",
        role: "Mediator / Family Law Attorney — weigh both sides",
        stance: "Whose interests does this protect, and at whose cost? What is the smallest \
                 concession that preserves the protection? Mediate between 'say everything' \
                 and 'say only the operative thing.'",
    },
    Voice {
        sign: "Sagittarius",
        glyph: "♐",
        role: "Public Interest / Civil Rights Attorney — big picture",
        stance: "Why does this matter beyond this one client? Does it honor the firm's mission \
                 of cheap, routine, attorney-supervised access to justice? Or does it drift \
                 toward the bespoke high-touch work the model rejects?",
    },
    Voice {
        sign: "Aquarius",
        glyph: "♒",
        role: "Legal Tech / Knowledge Management Attorney — systems pattern",
        stance: "Where else does this shape appear — in another template, another questionnaire, \
                 another retainer variant? Can the clause be templated and reused? Is the new \
                 copy a special case of something already general?",
    },
    Voice {
        sign: "Pisces",
        glyph: "♓",
        role: "Estate-Planning Counselor / Mental Health Court — honor the human story",
        stance: "The Person had a life before they had a matter — a family they want to provide \
                 for, an estate they have built, choices they made under hard circumstances. \
                 Be kind to the prior arrangement; someone chose it for a reason. Watch for \
                 language that shames the reader for the situation they are asking for help \
                 with.",
    },
];

/// Default voices — Capricorn leads, Scorpio sharpens. Anything
/// else is opt-in via `full: true`.
const DEFAULT_VOICE_COUNT: usize = 2;

#[must_use]
pub fn descriptor() -> Value {
    json!({
        "name": "aida_spawn_legal_council",
        "description":
            "Convene the firm's Legal Council — a twelve-lawyer review \
             pattern (a council of counsels) that shapes draft legal copy \
             *before* it becomes a notation \
             (template body, questionnaire prompt, engagement-letter \
             paragraph, follow-up email). The tool returns the counsel \
             brief and the draft; the calling model produces the voices \
             and the rewrite. By default only Capricorn (managing \
             partner, leads the bench) and Scorpio (ethics counsel, \
             cuts to the core) speak — pass `full: true` for all twelve. \
             Pass `draft` (the copy under review, required), optionally \
             a `question` framing what kind of copy this will become \
             (e.g., 'questionnaire prompt for the LLC formation \
             notation'), and `full: true` to convene the entire bench. \
             Returns a single text block ready to feed back to the \
             model that called this tool. Does NOT write to the \
             database or give legal advice — it sharpens drafting.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "draft": {
                    "type": "string",
                    "description":
                        "The draft copy under review. Required. Paste the \
                         raw text; if it has structure (numbered \
                         paragraphs, headings), keep it — the voices \
                         will cite it back."
                },
                "question": {
                    "type": "string",
                    "description":
                        "Optional framing: what is this copy *for*? \
                         e.g., 'questionnaire prompt for the LLC \
                         formation notation', 'engagement-letter \
                         paragraph on scope', 'access-to-justice blurb \
                         for the practice pages'. Sharpens the bench; \
                         drop it and the voices will infer."
                },
                "full": {
                    "type": "boolean",
                    "description":
                        "When true, convene all twelve voices (Capricorn \
                         → Scorpio → Aries → … → Pisces). When false or \
                         omitted, only Capricorn and Scorpio speak — \
                         the default per the `legal-council` skill."
                }
            },
            "required": ["draft"],
            "additionalProperties": false
        }
    })
}

#[derive(Debug, Deserialize)]
struct Args {
    draft: String,
    #[serde(default)]
    question: Option<String>,
    #[serde(default)]
    full: bool,
}

// Async to match the uniform `tools::call_tool` dispatch shape —
// every tool is awaited there — even though the body is pure CPU
// work with no `.await`. Don't drop the `async`.
#[allow(clippy::unused_async)]
pub async fn call(arguments: &Value) -> Result<Value, ToolError> {
    let args: Args = serde_json::from_value(arguments.clone())
        .map_err(|e| ToolError::InvalidArguments(e.to_string()))?;

    if args.draft.trim().is_empty() {
        return Err(ToolError::InvalidArguments(
            "`draft` must be non-empty".to_string(),
        ));
    }

    let voices: &[Voice] = if args.full {
        VOICES
    } else {
        &VOICES[..DEFAULT_VOICE_COUNT]
    };

    let framing = args
        .question
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .unwrap_or(
            "Shape this draft copy before it becomes a notation, \
             questionnaire prompt, or client-facing paragraph.",
        );

    let mode_label = if args.full {
        "full bench (twelve voices)"
    } else {
        "default pair (Capricorn + Scorpio)"
    };

    // `write!`/`writeln!` into a `String` is infallible — the `Write` impl
    // for `String` returns `Ok(())` unconditionally. Discarding the
    // `Result` keeps us out of `expect`/`unwrap` per rust-best-practices.
    let mut brief = String::new();
    brief.push_str("# Legal Council convened\n\n");
    let _ = writeln!(brief, "**Mode:** {mode_label}\n");
    let _ = writeln!(brief, "**Framing:** {framing}\n");
    brief.push_str(
        "The Legal Council is a drafting aid. It sharpens copy that will *become* a \
         notation — a template body, a questionnaire prompt, an engagement-letter paragraph, \
         a follow-up email. It does not produce final legal documents and it never gives \
         legal advice to a third party.\n\n",
    );
    brief.push_str("## Voices on the bench\n\n");
    brief.push_str(
        "Each voice contributes **one short, concrete sentence** about the draft below. \
         A voice may pass explicitly. Capricorn speaks first.\n\n",
    );
    for v in voices {
        let _ = writeln!(
            brief,
            "- **{} {}** — *{}.* {}",
            v.sign, v.glyph, v.role, v.stance
        );
    }
    brief.push_str("\n## Draft under review\n\n");
    brief.push_str("```\n");
    brief.push_str(args.draft.trim_end());
    brief.push_str("\n```\n\n");
    brief.push_str("## Required response shape\n\n");
    if args.full {
        brief.push_str(
            "1. **Framing** — one sentence restating what this copy will become.\n\
             2. **Findings** (optional) — what is actually true in the draft. Cite paragraphs \
             or exact phrases.\n\
             3. **Voices** — one line per voice in the order listed above, each grounded in \
             *this* draft. No generic philosophy.\n\
             4. **Consensus** — 3–5 bullets: the rewrites agreed on, the trade-offs surfaced, \
             the gaps that need the user's go/no-go.\n\
             5. **Revised copy** — the draft, rewritten. If a gap requires the user's \
             decision, name it; do not invent the answer.\n",
        );
    } else {
        brief.push_str(
            "1. **Framing** — one sentence restating what this copy will become.\n\
             2. **Capricorn** — one concrete sentence grounded in firm convention, bar \
             ethics, or prior incident.\n\
             3. **Scorpio** — one concrete sentence naming the load-bearing trust claim or \
             the hidden conflict.\n\
             4. **Revised copy** — the draft, rewritten in light of both voices. Show \
             before/after if the change is small.\n",
        );
    }

    let summary = if args.full {
        format!(
            "Legal Council convened — full bench ({} voices). Synthesize voices + revised copy.",
            voices.len()
        )
    } else {
        "Legal Council convened — default pair (Capricorn + Scorpio). Synthesize voices \
         + revised copy."
            .to_string()
    };

    let voice_names: Vec<&str> = voices.iter().map(|v| v.sign).collect();

    Ok(json!({
        "content": [{ "type": "text", "text": brief }],
        "structuredContent": {
            "mode": if args.full { "full" } else { "default" },
            "voices": voice_names,
            "framing": framing,
            "summary": summary,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::{call, descriptor, DEFAULT_VOICE_COUNT, VOICES};
    use crate::tools::ToolError;
    use serde_json::json;

    #[test]
    fn voices_lead_with_capricorn_then_scorpio() {
        // The bench order is load-bearing: the skill, the descriptor,
        // and the rendered brief all promise Capricorn first.
        assert_eq!(VOICES[0].sign, "Capricorn");
        assert_eq!(VOICES[1].sign, "Scorpio");
        assert_eq!(DEFAULT_VOICE_COUNT, 2);
    }

    #[test]
    fn bench_has_exactly_twelve_voices_one_per_zodiac_sign() {
        assert_eq!(VOICES.len(), 12);
        let signs: std::collections::BTreeSet<&str> = VOICES.iter().map(|v| v.sign).collect();
        assert_eq!(signs.len(), 12, "every zodiac sign must appear once");
    }

    #[test]
    fn descriptor_names_the_tool_under_aida_namespace() {
        let d = descriptor();
        assert_eq!(d["name"], "aida_spawn_legal_council");
        assert_eq!(d["inputSchema"]["additionalProperties"], false);
        let required = d["inputSchema"]["required"].as_array().unwrap();
        let required_names: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(required_names, vec!["draft"]);
        let props = d["inputSchema"]["properties"].as_object().unwrap();
        assert!(props.contains_key("draft"));
        assert!(props.contains_key("question"));
        assert!(props.contains_key("full"));
    }

    #[tokio::test]
    async fn missing_draft_is_invalid_arguments() {
        let err = call(&json!({})).await.unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidArguments(_)),
            "expected InvalidArguments, got {err:?}"
        );
    }

    #[tokio::test]
    async fn empty_draft_is_invalid_arguments() {
        let err = call(&json!({ "draft": "   \n  " })).await.unwrap_err();
        assert!(
            matches!(err, ToolError::InvalidArguments(_)),
            "expected InvalidArguments, got {err:?}"
        );
    }

    #[tokio::test]
    async fn default_invocation_lists_only_capricorn_and_scorpio() {
        let result = call(&json!({ "draft": "We may or may not represent you." }))
            .await
            .unwrap();
        let sc = &result["structuredContent"];
        assert_eq!(sc["mode"], "default");
        let voices: Vec<&str> = sc["voices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(voices, vec!["Capricorn", "Scorpio"]);

        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Capricorn"));
        assert!(text.contains("Scorpio"));
        // Default bench must NOT advertise the other ten voices in
        // the rendered brief — that would confuse the calling model
        // about who is meant to speak.
        assert!(
            !text.contains("Aries"),
            "default brief must not name Aries: {text}"
        );
        assert!(
            !text.contains("Pisces"),
            "default brief must not name Pisces: {text}"
        );
        // The draft must be echoed back so the calling model has it
        // in context without re-passing.
        assert!(text.contains("We may or may not represent you."));
    }

    #[tokio::test]
    async fn full_invocation_lists_all_twelve_voices_in_order() {
        let result = call(&json!({
            "draft": "Welcome to Neon Law.",
            "question": "access-to-justice blurb",
            "full": true,
        }))
        .await
        .unwrap();
        let sc = &result["structuredContent"];
        assert_eq!(sc["mode"], "full");
        let voices: Vec<&str> = sc["voices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(voices.len(), 12);
        assert_eq!(voices[0], "Capricorn");
        assert_eq!(voices[1], "Scorpio");
        assert_eq!(voices[2], "Aries");
        assert_eq!(voices.last().copied(), Some("Pisces"));
        assert_eq!(sc["framing"], "access-to-justice blurb");
    }

    #[tokio::test]
    async fn brief_calls_out_no_legal_advice_guardrail() {
        // Mission-critical: the Legal Council shapes drafting, it does not
        // give a third party legal advice. The brief itself must say
        // so out loud so a downstream model cannot drift into UPL.
        let result = call(&json!({ "draft": "Some draft." })).await.unwrap();
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("never gives legal advice"),
            "brief must reaffirm the no-legal-advice guardrail; got: {text}"
        );
    }
}
