//! Load the baked-in workshop manifest.
//!
//! Workshops and presentations share one authored markdown manifest.

use std::fs;
use std::io;
use std::path::Path;

use pulldown_cmark::{html, Event, Options, Parser, Tag};

use super::{WorkshopChapter, WorkshopMaterial, WorkshopSection};
use crate::content_loader::ContentLoadError;

struct ManifestEntry {
    category: &'static str,
    slug: &'static str,
    title: &'static str,
    description: &'static str,
    /// Who the material is for, shown as the audience tag on the
    /// category overview so a reader self-selects fast.
    audience: &'static str,
    /// The you-voiced takeaway shown as the overview card body —
    /// what the reader walks out with, never a guaranteed outcome.
    benefit: &'static str,
    filename: &'static str,
}

/// Subdirectory under the workshops content root where the
/// Neon Law Navigator workshop's materials live.
const NAVIGATOR_FOLDER: &str = "navigator";

const NAVIGATOR_MANIFEST: &[ManifestEntry] = &[
    ManifestEntry {
        category: "workshops",
        slug: "use-the-navigator",
        title: "Using Neon Law Navigator",
        description: "A single hands-on workshop for Lawyer users of the application. \
                      Open the litigation matter, bind the shared retainer template, and inspect \
                      the client portal through the configured AIDA connector.",
        audience: "For lawyer users",
        benefit: "You walk out with a litigation matter walkthrough and a \
                  three-minute demo you can run at your firm. You also see the five stored \
                  authorization roles in context: Owner governs the system, Admin manages the \
                  installation, Lawyers run the legal workflow, Clerks are supervised \
                  non-lawyers, and Clients use the portal for their matters.",
        filename: "README.md",
    },
    ManifestEntry {
        category: "workshops",
        slug: "deploy-the-navigator",
        title: "Operating Neon Law Navigator",
        description: "Stand up and operate your own Neon Law Navigator instance on a custom Google Cloud project. Six \
                      grounded steps walk `navigator ops gcp setup` — APIs, VPC, four buckets, \
                      and a GKE Autopilot cluster — with a dry-run that shows every API call \
                      before a packet leaves your laptop.",
        audience: "For admin users",
        benefit: "You walk out running the same stack a working law firm runs, on your own \
                  Google Cloud project, for your own community. One command does most of the \
                  work, and a dry-run shows you every step before a packet leaves your laptop. \
                  This is the workshop for the admin tier: people and role administration, billing, \
                  secrets, OIDC, runtime configuration, and release verification.",
        filename: "DEPLOY.md",
    },
    ManifestEntry {
        category: "workshops",
        slug: "contribute-to-the-navigator",
        title: "Contributing to Neon Law Navigator",
        description: "Five ways authorized lawyers improve the product — open an issue, share \
                      what you learned, join a workshop or a presentation, or simply use it. No \
                      code required for most of them.",
        audience: "For the community",
        benefit: "You walk out knowing how authorized lawyers improve the product: open an issue, \
                  improve a template, add or map a fillable government PDF from the common \
                  question bank, or show the team what broke when you used it.",
        filename: "CONTRIBUTE.md",
    },
    // A conference talk. Every code slide is an exact copy of the workspace
    // file it cites; the
    // `rust_in_peace_snippets_are_exact_copies_of_cited_sources` test fails
    // the build if one drifts.
    ManifestEntry {
        category: "presentations",
        slug: "rust-in-peace",
        title: "Rust in Peace",
        description:
            "A Neon Law talk for Rust NYC on how we use Rust to improve access to \
             justice: deterministic workflows from law — statute to Cucumber feature to template \
             to notation — dissected one modular, attorney-gated step at a time, with every code \
             slide an exact copy of the shipped repository kept honest by a grounding test.",
        audience: "For the hackers",
        benefit: "You walk out able to argue, from the real code, why a reviewed and repeatable \
                  workflow beats prompting an LLM. Every slide is an exact copy of the shipped \
                  repository — a build test fails if one drifts — so you react to the real \
                  thing, not a diagram.",
        filename: "RUST_IN_PEACE.md",
    },
];

/// Load every manifest entry for the Neon Law Navigator workshop. Missing
/// files are silently skipped so a partial install still boots; the
/// index page drops cards for materials it couldn't find.
pub fn load_navigator(content_root: &Path) -> Result<Vec<WorkshopMaterial>, ContentLoadError> {
    let folder = content_root.join(NAVIGATOR_FOLDER);
    let mut materials = Vec::new();
    for entry in NAVIGATOR_MANIFEST {
        let path = folder.join(entry.filename);
        let raw = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(ContentLoadError::Io {
                    path: path.display().to_string(),
                    source: err,
                });
            }
        };
        materials.push(material_from_markdown(
            entry.category,
            entry.slug,
            entry.title,
            entry.description,
            entry.audience,
            entry.benefit,
            &raw,
        ));
    }
    Ok(materials)
}

/// Parse one stepped-content document into a [`WorkshopMaterial`]:
/// split `##` chapters into `###` sections, render each section to HTML, and keep
/// the raw markdown for the copy-to-clipboard button.
///
/// Fed by the workshop loader with manifest-declared files from disk.
/// The title, description, audience, and benefit come from the caller,
/// not the markdown, so the surface controls its own chrome.
pub(crate) fn material_from_markdown(
    category: &str,
    slug: &str,
    title: &str,
    description: &str,
    audience: &str,
    benefit: &str,
    raw: &str,
) -> WorkshopMaterial {
    // Workshop pages declare `kind: workshop` (plus title/description) in a
    // leading frontmatter block so the linter classifies them explicitly —
    // but that metadata is page chrome the manifest owns, never slide
    // content. Strip it before splitting/rendering so it can't surface as
    // literal text (its `---` would otherwise read as a slide divider).
    let raw = strip_frontmatter(raw);
    let (intro_md, chapter_specs) = split_chapters(raw);
    let mut chapters = Vec::with_capacity(chapter_specs.len());
    let mut sections = Vec::new();
    for (chapter_title, chapter_preamble, section_specs) in chapter_specs {
        let section_start = sections.len();
        for (title, body_md) in section_specs {
            // Each `###` section is one slide: split its body on the first
            // top-level `---` thematic break into the slide face (above)
            // and the presenter notes (below).
            let (face_md, notes_md) = split_face_notes(&body_md);
            sections.push(WorkshopSection {
                title,
                body_html: render_markdown(&face_md),
                notes_html: render_markdown(&notes_md),
            });
        }
        chapters.push(WorkshopChapter {
            title: chapter_title,
            preamble_html: render_markdown(&chapter_preamble.join("\n")),
            section_start,
            section_count: sections.len() - section_start,
        });
    }
    WorkshopMaterial {
        category: category.to_string(),
        slug: slug.to_string(),
        title: title.to_string(),
        description: description.to_string(),
        audience: audience.to_string(),
        benefit: benefit.to_string(),
        // The page chrome owns the sole `<h1>`; strip the leading
        // `#` title so the rendered body doesn't repeat it.
        body_html: render_markdown(&strip_leading_h1(raw)),
        intro_html: render_markdown(&intro_md),
        chapters,
        sections,
        raw_markdown: raw.to_string(),
    }
}

/// Strip a leading YAML frontmatter block (`---` … `---`) and return the
/// body that follows. Returns `raw` unchanged when it declares no
/// frontmatter, so a page written without one still loads.
fn strip_frontmatter(raw: &str) -> &str {
    let Some(after_open) = raw.strip_prefix("---\n") else {
        return raw;
    };
    let Some(close) = after_open.find("\n---") else {
        return raw;
    };
    // `close` points at the newline before the closing `---`; advance past
    // the whole closing-delimiter line to the first line of the body.
    let after_delim = &after_open[close + 1..];
    match after_delim.find('\n') {
        Some(nl) => after_delim[nl + 1..].trim_start_matches('\n'),
        None => "",
    }
}

fn render_markdown(src: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_FOOTNOTES);
    // Highlight fenced code server-side with syntect — the workshop slides and
    // the "Rust in Peace" talk styled their code through the vendored
    // highlight.js; the shared seam preserves that colouring with no client JS.
    //
    // Route every image `src` through the same asset seam the blog uses, so a
    // slide author can publish a picture to the deployment's assets bucket
    // with a repo-relative `img/…` path. Without this the browser resolves
    // that path against the slide's own URL (`/presentations/{slug}/step/{n}`)
    // and 404s. A root-relative `/public/…` source is left alone — that is the
    // tracked lane, which ships inside the container image.
    let events: Vec<_> = Parser::new_ext(src, opts)
        .map(|event| match event {
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                title,
                id,
            }) => Event::Start(Tag::Image {
                link_type,
                dest_url: views::assets::rewrite_image_src(&dest_url).into(),
                title,
                id,
            }),
            other => other,
        })
        .collect();
    let mut out = String::new();
    // Video before highlighting: both are event-stream passes, and a clip
    // upgraded to `<video>` must not still look like an image to anything
    // downstream.
    let events = views::markdown::upgrade_video_images(events);
    html::push_html(
        &mut out,
        views::markdown::highlight_code_blocks(events).into_iter(),
    );
    out
}

/// Drop a single leading top-level (`# `) heading so the rendered body
/// does not duplicate the title the page chrome already renders as the
/// document's `<h1>`. Only the *first* such line, and only before any
/// content, is removed; `## ` and deeper headings are untouched.
fn strip_leading_h1(src: &str) -> String {
    let trimmed = src.trim_start();
    match trimmed.strip_prefix("# ") {
        Some(after_hash) => {
            // Drop the rest of the title line and any blank lines that
            // follow it, keeping the body verbatim.
            let body = after_hash.split_once('\n').map_or("", |(_, rest)| rest);
            body.trim_start().to_string()
        }
        // No leading H1 — return the source untouched.
        None => src.to_string(),
    }
}

/// True for an ATX `# ` heading but not `## ` or deeper.
fn is_h1(line: &str) -> bool {
    line.starts_with("# ")
}

type SectionSource = (String, String);
type ChapterSource = (String, Vec<String>, Vec<SectionSource>);

/// Split workshop markdown into `(intro, chapters)`. The intro is everything
/// before the first `##` heading (with the leading `#` title stripped). Each
/// `##` starts a chapter; prose before its first `###` is the chapter preamble,
/// and each `###` beneath it starts a section/slide whose heading line remains
/// in its rendered face. Fenced headings are always content, never structure.
fn split_chapters(src: &str) -> (String, Vec<ChapterSource>) {
    let mut intro: Vec<String> = Vec::new();
    let mut chapters: Vec<ChapterSource> = Vec::new();
    let mut current_chapter: Option<ChapterSource> = None;
    let mut current_section: Option<(String, Vec<String>)> = None;
    let mut in_fence = false;
    let mut title_stripped = false;

    for line in src.lines() {
        if is_fence(line) {
            in_fence = !in_fence;
        }

        if !in_fence {
            if let Some(heading) = line.strip_prefix("## ") {
                if let Some((title, body)) = current_section.take() {
                    if let Some((_, _, sections)) = current_chapter.as_mut() {
                        sections.push((title, body.join("\n")));
                    }
                }
                if let Some(chapter) = current_chapter.take() {
                    chapters.push(chapter);
                }
                current_chapter = Some((heading.trim().to_string(), Vec::new(), Vec::new()));
                continue;
            }
            if let Some(heading) = line.strip_prefix("### ") {
                if current_chapter.is_some() {
                    if let Some((title, body)) = current_section.take() {
                        if let Some((_, _, sections)) = current_chapter.as_mut() {
                            sections.push((title, body.join("\n")));
                        }
                    }
                    current_section = Some((heading.trim().to_string(), vec![line.to_string()]));
                    continue;
                }
            }
        }

        if let Some((_, body)) = current_section.as_mut() {
            body.push(line.to_string());
        } else if let Some((_, preamble, _)) = current_chapter.as_mut() {
            // A chapter heading is organizational, but prose before its first
            // section introduces that chapter on the overview page.
            preamble.push(line.to_string());
        } else if !title_stripped && intro.is_empty() && line.trim().is_empty() {
            // Skip blank lines before the title.
        } else if !title_stripped && is_h1(line) {
            // Drop the leading title — the page chrome renders it.
            title_stripped = true;
        } else {
            intro.push(line.to_string());
        }
    }
    if let Some((title, body)) = current_section.take() {
        if let Some((_, _, sections)) = current_chapter.as_mut() {
            sections.push((title, body.join("\n")));
        }
    }
    if let Some(chapter) = current_chapter.take() {
        chapters.push(chapter);
    }
    (intro.join("\n"), chapters)
}

/// True for a ```` ``` ```` or `~~~` fence marker (any indentation).
fn is_fence(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("```") || trimmed.starts_with("~~~")
}

/// Split one `##` section's markdown into `(slide_face, presenter_notes)`
/// on the first top-level `---` thematic break. The face is the slide
/// shown on top; the notes are the prose shown beneath it. A `---` inside
/// a fenced code block is sample text, never a divider. With no divider
/// the whole section is the face and the notes come back empty.
fn split_face_notes(section_md: &str) -> (String, String) {
    let lines: Vec<&str> = section_md.lines().collect();
    let mut in_fence = false;
    for (i, line) in lines.iter().enumerate() {
        if is_fence(line) {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence && is_thematic_break(line) {
            let face = lines[..i].join("\n");
            let notes = lines[i + 1..].join("\n");
            return (face.trim_end().to_string(), notes.trim().to_string());
        }
    }
    (section_md.trim_end().to_string(), String::new())
}

/// True for a `---` thematic break — a line that, trimmed, is three or
/// more dashes and nothing else. This is the slide/notes divider; other
/// break styles (`***`, `___`) are left as ordinary `<hr>` in the face.
fn is_thematic_break(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 3 && t.bytes().all(|b| b == b'-')
}

#[cfg(test)]
mod tests {
    use super::{
        load_navigator, material_from_markdown, split_chapters, split_face_notes,
        strip_frontmatter, strip_leading_h1, NAVIGATOR_MANIFEST,
    };
    use std::fs;
    use tempfile::TempDir;

    /// The slides + presenter-notes format is the contract for every
    /// workshop, now and in the future: each `##` slide must carry a
    /// `---` divider with presenter notes beneath it. This walks the real
    /// baked content (not a fixture) and fails the build if any slide is
    /// missing its face or its notes — so a new workshop can't ship in the
    /// old prose-only shape.
    #[test]
    fn every_material_has_chapters_and_section_notes() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../server/content/workshops");
        let materials = load_navigator(std::path::Path::new(root)).unwrap();
        assert!(
            !materials.is_empty(),
            "real workshop content failed to load from {root}"
        );
        for m in &materials {
            assert!(
                !m.chapters.is_empty(),
                "material `{}` must carry at least one chapter",
                m.slug
            );
            for chapter in &m.chapters {
                assert!(
                    chapter.section_count > 0,
                    "material `{}` chapter `{}` must contain at least one section",
                    m.slug,
                    chapter.title
                );
            }
            assert_eq!(
                m.chapters
                    .iter()
                    .map(|chapter| chapter.section_count)
                    .sum::<usize>(),
                m.sections.len(),
                "material `{}` chapter ranges must cover every section exactly once",
                m.slug
            );
            assert!(
                !m.sections.is_empty(),
                "workshop `{}` has no `##` slides",
                m.slug
            );
            for (i, s) in m.sections.iter().enumerate() {
                assert!(
                    !s.body_html.trim().is_empty(),
                    "workshop `{}` slide {} (`{}`) has an empty slide face",
                    m.slug,
                    i + 1,
                    s.title
                );
                assert!(
                    !s.notes_html.trim().is_empty(),
                    "workshop `{}` slide {} (`{}`) is missing presenter notes — every slide \
                     needs a `---` divider followed by notes",
                    m.slug,
                    i + 1,
                    s.title
                );
            }
        }
    }

    /// Count the `**Term** — definition` openings in one rendered list item.
    /// The separator is matched as any dash — em, en, or hyphen — with any
    /// spacing around it, because the guard below must catch a crammed
    /// bullet however its author typed the dash, not only in the em-dash
    /// form the content happens to use today. A trailing space is required
    /// so a hyphenated word (`<strong>tower</strong>-http`) is not read as a
    /// definition.
    fn term_definition_count(item: &str) -> usize {
        const CLOSE: &str = "</strong>";
        item.match_indices(CLOSE)
            .filter(|(at, _)| {
                let rest = item[at + CLOSE.len()..].trim_start();
                let after_dash = rest
                    .strip_prefix('—')
                    .or_else(|| rest.strip_prefix('–'))
                    .or_else(|| rest.strip_prefix('-'));
                after_dash.is_some_and(|rest| rest.starts_with(char::is_whitespace))
            })
            .count()
    }

    /// A slide bullet is a scanning aid: one `**Term** — definition` per
    /// line, so the room reads the list at a glance. Markdown folds an
    /// indented continuation line into the *preceding* list item, so a
    /// wrapped bullet silently collapses several terms into one dense
    /// `<li>` — it still renders, just as a wall of text. This walks the
    /// real baked content and fails the build when any slide face ships a
    /// bullet carrying more than one term, which is the shape that defect
    /// always takes.
    #[test]
    fn no_slide_bullet_crams_multiple_terms_into_one_item() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../server/content/workshops");
        let materials = load_navigator(std::path::Path::new(root)).unwrap();
        assert!(
            !materials.is_empty(),
            "real workshop content failed to load from {root}"
        );
        for m in &materials {
            for s in &m.sections {
                for item in s.body_html.split("<li>").skip(1) {
                    let item = item.split("</li>").next().unwrap_or_default();
                    let terms = term_definition_count(item);
                    assert!(
                        terms <= 1,
                        "workshop `{}` slide `{}` has a bullet with {terms} `**Term** —` items \
                         crammed into one list entry — give each term its own `- ` bullet: {item}",
                        m.slug,
                        s.title
                    );
                }
            }
        }
    }

    /// The dash tolerance above is the whole point of the guard, so pin it:
    /// every separator an author might reach for counts, and a hyphenated
    /// word does not.
    #[test]
    fn term_definition_count_reads_every_dash_but_not_a_hyphenated_word() {
        assert_eq!(
            term_definition_count("<strong>Remember</strong> — name it."),
            1
        );
        assert_eq!(
            term_definition_count("<strong>Remember</strong> - name it."),
            1
        );
        assert_eq!(
            term_definition_count("<strong>Remember</strong> – name it."),
            1
        );
        assert_eq!(
            term_definition_count("<strong>Remember</strong>  —  name it."),
            1
        );
        // The crammed shape this guard exists to catch, in hyphen form.
        assert_eq!(
            term_definition_count(
                "<strong>Remember</strong> - name it. <strong>Apply</strong> - do it."
            ),
            2
        );
        // A hyphenated crate name is not a term definition.
        assert_eq!(
            term_definition_count("<strong>tower</strong>-http is a crate."),
            0
        );
        // A bold term with no dash at all is not a term definition.
        assert_eq!(
            term_definition_count("<strong>Scope is a field.</strong> Every step."),
            0
        );
    }

    #[test]
    fn strip_frontmatter_removes_a_leading_yaml_block() {
        assert_eq!(
            strip_frontmatter("---\nkind: workshop\ntitle: T\n---\n\n# Body\n\nProse.\n"),
            "# Body\n\nProse.\n"
        );
        // No frontmatter — returned untouched.
        assert_eq!(
            strip_frontmatter("# Body\n\nProse.\n"),
            "# Body\n\nProse.\n"
        );
    }

    #[test]
    fn declared_frontmatter_never_leaks_into_the_rendered_workshop() {
        // A workshop page declares `kind: workshop` so the linter classifies
        // it — but that metadata must never surface as slide text (its `---`
        // would otherwise read as a divider). The rendered intro, body, and
        // raw markdown all begin at the body, not the frontmatter.
        let raw = "---\nkind: workshop\ntitle: Using It\ndescription: A page.\n---\n\n\
                   # Using It\n\nIntro prose.\n\n## Intro\n\n### Step one\n\nFace.\n\n---\n\nNotes.\n";
        let m = material_from_markdown(
            "workshops",
            "using-it",
            "Using It",
            "A page.",
            "For lawyers",
            "You learn.",
            raw,
        );
        for surface in [&m.intro_html, &m.body_html, &m.raw_markdown] {
            assert!(
                !surface.contains("kind: workshop") && !surface.contains("description: A page"),
                "frontmatter leaked into a rendered surface: {surface}"
            );
        }
        // The real slide content still parses.
        assert_eq!(m.sections.len(), 1);
        assert_eq!(m.sections[0].title, "Step one");
    }

    /// A slide can carry a picture published to the deployment's assets
    /// bucket. The author writes the repo-relative `img/…` key and the
    /// renderer resolves it against `NAVIGATOR_ASSET_BASE_URL` (`/public`
    /// by default), exactly as the blog does. Without this the browser
    /// resolves the path against the slide's own URL — `/presentations/
    /// {slug}/step/{n}/img/…` — and the picture 404s in every deployment.
    #[test]
    fn a_relative_slide_image_routes_through_the_asset_seam() {
        let m = material_from_markdown(
            "presentations",
            "talk",
            "Talk",
            "A talk.",
            "For the hackers",
            "You learn.",
            "# Talk\n\n## Intro\n\n### Slide\n\n![a logo](img/lvrug/lvrug.png)\n\n---\n\nNotes.\n",
        );
        assert!(
            m.sections[0]
                .body_html
                .contains(r#"src="/public/img/lvrug/lvrug.png""#),
            "a repo-relative slide image must resolve against the asset base, got: {}",
            m.sections[0].body_html
        );
    }

    /// The tracked lane must survive the rewrite: a root-relative source
    /// is already a URL the container image serves, so re-resolving it
    /// would double the prefix. External and `data:` sources are equally
    /// off-limits.
    #[test]
    fn an_absolute_slide_image_source_is_left_untouched() {
        let m = material_from_markdown(
            "presentations",
            "talk",
            "Talk",
            "A talk.",
            "For the hackers",
            "You learn.",
            "# Talk\n\n## Intro\n\n### Slide\n\n![shipped](/public/workshops/navigator/x.jpg)\n\n\
             ![remote](https://example.com/y.png)\n\n---\n\nNotes.\n",
        );
        let html = &m.sections[0].body_html;
        assert!(
            html.contains(r#"src="/public/workshops/navigator/x.jpg""#),
            "the tracked `/public/…` lane must pass through unchanged, got: {html}"
        );
        assert!(
            html.contains(r#"src="https://example.com/y.png""#),
            "an absolute source must pass through unchanged, got: {html}"
        );
    }

    /// Markdown has no video syntax, so a clip is written with the image
    /// syntax and the renderer picks the element. The destination still
    /// routes through the asset seam, so a slide's video publishes to the
    /// bucket exactly like its pictures, and the alt text survives as the
    /// accessible name because `<video>` has no `alt` attribute.
    #[test]
    fn a_video_source_renders_as_a_video_element() {
        let m = material_from_markdown(
            "presentations",
            "talk",
            "Talk",
            "A talk.",
            "For the hackers",
            "You learn.",
            "# Talk\n\n## Intro\n\n### Slide\n\n![the portal, recorded](img/demo/portal.mp4)\n\n\
             ---\n\nNotes.\n",
        );
        let html = &m.sections[0].body_html;
        assert!(
            html.contains(r#"<video src="/public/img/demo/portal.mp4""#),
            "an mp4 must render as <video> resolved through the asset seam, got: {html}"
        );
        assert!(
            !html.contains("<img"),
            "a video must not also render as an <img>, got: {html}"
        );
        assert!(
            html.contains(r#"aria-label="the portal, recorded""#),
            "the caption must become the accessible name, got: {html}"
        );
        assert!(
            html.contains("controls") && !html.contains("autoplay"),
            "playback is user-initiated, never autoplay, got: {html}"
        );
    }

    /// The upgrade is keyed on the extension alone, so an ordinary picture
    /// on the same slide must still be an `<img>`.
    #[test]
    fn a_picture_beside_a_video_still_renders_as_an_image() {
        let m = material_from_markdown(
            "presentations",
            "talk",
            "Talk",
            "A talk.",
            "For the hackers",
            "You learn.",
            "# Talk\n\n## Intro\n\n### Slide\n\n![a logo](img/lvrug/lvrug.png)\n\n\
             ![a clip](img/demo/portal.mp4)\n\n---\n\nNotes.\n",
        );
        let html = &m.sections[0].body_html;
        assert!(
            html.contains(r#"<img src="/public/img/lvrug/lvrug.png""#),
            "a png stays an image, got: {html}"
        );
        assert!(
            html.contains(r#"<video src="/public/img/demo/portal.mp4""#),
            "an mp4 becomes a video, got: {html}"
        );
    }

    #[test]
    fn split_face_notes_divides_on_the_first_top_level_break() {
        let (face, notes) = split_face_notes("## Build\n\nSlide face.\n\n---\n\nPresenter notes.");
        assert!(face.contains("Slide face"));
        assert!(!face.contains("Presenter notes"));
        assert!(face.contains("## Build"));
        assert_eq!(notes, "Presenter notes.");
    }

    #[test]
    fn split_face_notes_returns_empty_notes_without_a_divider() {
        let (face, notes) = split_face_notes("## Build\n\nJust a face, no notes.");
        assert!(face.contains("Just a face"));
        assert!(notes.is_empty());
    }

    #[test]
    fn split_face_notes_ignores_a_break_inside_a_code_fence() {
        // A `---` line inside a fenced block is YAML/sample text, not the
        // slide/notes divider.
        let (face, notes) = split_face_notes(
            "## Build\n\n```yaml\nkey: value\n---\nmore: yaml\n```\n\n---\n\nNotes.",
        );
        assert!(face.contains("more: yaml"), "fenced --- stays in the face");
        assert_eq!(notes, "Notes.");
    }

    #[test]
    fn loaded_section_carries_face_and_notes_html() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("navigator");
        fs::create_dir_all(&target).unwrap();
        fs::write(
            target.join("README.md"),
            "# T\n\nLede.\n\n## Intro\n\n### Step one\n\nThe slide.\n\n---\n\nThe notes.\n\n## Wrap Up\n\n### Finish\n\nDone.\n\n---\n\nClosing notes.\n",
        )
        .unwrap();
        let m = &load_navigator(dir.path()).unwrap()[0];
        assert!(m.sections[0].body_html.contains("The slide"));
        assert!(!m.sections[0].body_html.contains("The notes"));
        assert!(m.sections[0].notes_html.contains("The notes"));
    }

    #[test]
    fn load_navigator_returns_empty_when_directory_missing() {
        let materials = load_navigator(std::path::Path::new("/no/such/dir/12345")).unwrap();
        assert!(materials.is_empty());
    }

    /// Walk a deck's ``From `path/to/file`:`` attributions and check each
    /// fenced block that follows one is an exact copy of the cited workspace
    /// file. Returns how many snippets were grounded, or the first problem.
    ///
    /// Split out of the test below so the drift detection is itself proven.
    /// The shipped decks currently cite no sources at all, so a walk over
    /// them alone never enters this loop and would pass while checking
    /// nothing — which is exactly what a walk with no fixtures behind it did.
    fn verify_cited_snippets(markdown: &str, workspace_root: &str) -> Result<usize, String> {
        let lines: Vec<&str> = markdown.lines().collect();
        let mut grounded = 0;
        let mut i = 0;
        while i < lines.len() {
            if let Some(path) = lines[i]
                .strip_prefix("From `")
                .and_then(|rest| rest.strip_suffix("`:"))
            {
                let mut open = i + 1;
                while open < lines.len() && !lines[open].starts_with("```") {
                    open += 1;
                }
                if open >= lines.len() {
                    return Err(format!("attribution for {path} has no code fence after it"));
                }
                let mut close = open + 1;
                while close < lines.len() && lines[close] != "```" {
                    close += 1;
                }
                if close >= lines.len() {
                    return Err(format!("code fence for {path} is never closed"));
                }
                let snippet = lines[open + 1..close].join("\n");
                let source = fs::read_to_string(format!("{workspace_root}/{path}"))
                    .map_err(|e| format!("cited source {path} is unreadable: {e}"))?;
                if !source.contains(&snippet) {
                    return Err(format!(
                        "slide snippet drifted from {path} — update the talk to match the source"
                    ));
                }
                grounded += 1;
                i = close;
            }
            i += 1;
        }
        Ok(grounded)
    }

    /// The "Rust in Peace" talk became a workshop when the standalone
    /// Presentations surface was removed. Its convention survives the move:
    /// every code slide is introduced by ``From `path/to/file`:`` followed
    /// by a fenced block, and must be an **exact copy** of that workspace
    /// file. This walks the baked talk, reads each cited file from the
    /// workspace (not a second baked copy, which would always pass), and
    /// fails the build when a snippet drifts.
    ///
    /// No count is asserted: how many snippets a deck cites is an authoring
    /// decision, and a trimmed deck citing none is not a regression. What the
    /// convention is worth is proven by the fixtures below instead.
    #[test]
    fn rust_in_peace_snippets_are_exact_copies_of_cited_sources() {
        const TALK: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../server/content/workshops/navigator/RUST_IN_PEACE.md"
        ));
        let workspace_root = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
        if let Err(problem) = verify_cited_snippets(TALK, workspace_root) {
            panic!("{problem}");
        }
    }

    /// A snippet that matches its cited source is grounded, and one that has
    /// drifted from it is caught. This is the assertion the real-deck walk
    /// above cannot make while the shipped decks cite nothing.
    #[test]
    fn a_cited_snippet_is_grounded_and_a_drifted_one_is_caught() {
        let root = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("src")).unwrap();
        fs::write(
            root.path().join("src/lib.rs"),
            "fn one() -> u8 {\n    1\n}\n",
        )
        .unwrap();
        let workspace_root = root.path().to_str().unwrap();

        let grounded = "From `src/lib.rs`:\n\n```rust\nfn one() -> u8 {\n    1\n}\n```\n";
        assert_eq!(
            verify_cited_snippets(grounded, workspace_root),
            Ok(1),
            "a snippet copied from its source is grounded"
        );

        let drifted = "From `src/lib.rs`:\n\n```rust\nfn one() -> u8 {\n    2\n}\n```\n";
        let problem = verify_cited_snippets(drifted, workspace_root).unwrap_err();
        assert!(
            problem.contains("drifted from src/lib.rs"),
            "a changed line must be reported as drift, got: {problem}"
        );
    }

    /// The three malformed shapes the walk has to reject rather than skip: an
    /// attribution with no fence after it, a fence that never closes, and a
    /// citation of a file that is not there. Each would otherwise let a code
    /// slide ship unchecked.
    #[test]
    fn a_malformed_or_missing_citation_is_rejected() {
        let root = tempfile::TempDir::new().unwrap();
        fs::write(root.path().join("real.rs"), "fn real() {}\n").unwrap();
        let workspace_root = root.path().to_str().unwrap();

        let no_fence = "From `real.rs`:\n\nprose, not a fenced block\n";
        assert!(
            verify_cited_snippets(no_fence, workspace_root)
                .unwrap_err()
                .contains("no code fence"),
            "an attribution with no fence after it is rejected"
        );

        let unclosed = "From `real.rs`:\n\n```rust\nfn real() {}\n";
        assert!(
            verify_cited_snippets(unclosed, workspace_root)
                .unwrap_err()
                .contains("never closed"),
            "a fence that never closes is rejected"
        );

        let missing = "From `gone.rs`:\n\n```rust\nfn gone() {}\n```\n";
        assert!(
            verify_cited_snippets(missing, workspace_root)
                .unwrap_err()
                .contains("unreadable"),
            "a citation of a file that is not there is rejected"
        );
    }

    /// A deck that cites nothing walks clean and grounds nothing — the shape
    /// every shipped deck currently has. Pinned so the walk's own behaviour on
    /// that input is stated rather than assumed.
    #[test]
    fn a_deck_that_cites_no_source_grounds_nothing() {
        assert_eq!(
            verify_cited_snippets("### A slide\n\nProse only.\n", "/no/such/root"),
            Ok(0)
        );
    }

    #[test]
    fn load_navigator_loads_the_rust_in_peace_talk_as_a_workshop() {
        // The talk now rides the workshop manifest; with its file present it
        // loads beside README/DEPLOY with steps split on its `##` beats.
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("navigator");
        fs::create_dir_all(&target).unwrap();
        fs::write(
            target.join("RUST_IN_PEACE.md"),
            "# Rust in Peace\n\nLede.\n\n## Intro\n\n### Agenda\n\nWhat we'll cover.\n\n## Wrap Up\n\n### Review\n\nReview it.\n",
        )
        .unwrap();
        let materials = load_navigator(dir.path()).unwrap();
        let talk = materials
            .iter()
            .find(|m| m.slug == "rust-in-peace")
            .expect("rust-in-peace loads as a workshop");
        assert_eq!(talk.title, "Rust in Peace");
        assert_eq!(talk.sections[0].title, "Agenda");
    }

    #[test]
    fn load_navigator_loads_the_using_workshop_from_readme() {
        // With only README.md on disk, the other manifest entries
        // (DEPLOY/CONTRIBUTE/RUST_IN_PEACE) are silently skipped, so the
        // load is exactly the "Using Neon Law Navigator" workshop.
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("navigator");
        fs::create_dir_all(&target).unwrap();
        fs::write(
            target.join("README.md"),
            "# Runbook\n\nWelcome to Neon Law Navigator.\n",
        )
        .unwrap();
        let materials = load_navigator(dir.path()).unwrap();
        assert_eq!(materials.len(), 1, "only README.md is on disk");
        assert_eq!(materials[0].category, "workshops");
        assert_eq!(materials[0].slug, "use-the-navigator");
        assert_eq!(materials[0].title, "Using Neon Law Navigator");
        // The audience tag and you-voiced benefit ride the manifest, not
        // the markdown — the overview card is fed from these.
        assert_eq!(materials[0].audience, "For lawyer users");
        assert!(
            materials[0].benefit.starts_with("You walk out"),
            "benefit is second-person and leads with the takeaway, got: {}",
            materials[0].benefit
        );
    }

    #[test]
    fn navigator_workshop_card_metadata_pins_role_audiences() {
        // The overview cards are fed from the real manifest, not the
        // workshop markdown. Pin all role-facing cards so the public
        // Workshop and presentation audience labels cannot drift silently.
        for (slug, audience, required_benefit_terms) in [
            (
                "use-the-navigator",
                "For lawyer users",
                [
                    "authorization roles",
                    "Owner",
                    "Admin",
                    "Lawyer",
                    "Clerks",
                    "Clients",
                ]
                .as_slice(),
            ),
            (
                "deploy-the-navigator",
                "For admin users",
                ["admin tier", "billing", "secrets", "OIDC"].as_slice(),
            ),
            (
                "contribute-to-the-navigator",
                "For the community",
                [
                    "authorized lawyers improve the product",
                    "fillable government PDF",
                    "common question bank",
                ]
                .as_slice(),
            ),
        ] {
            let entry = NAVIGATOR_MANIFEST
                .iter()
                .find(|entry| entry.slug == slug)
                .unwrap_or_else(|| panic!("missing manifest entry for {slug}"));
            assert_eq!(entry.category, "workshops", "{slug} category");
            assert_eq!(entry.audience, audience, "{slug} audience");
            assert!(
                entry.benefit.starts_with("You walk out"),
                "{slug} benefit should stay second-person, got: {}",
                entry.benefit
            );
            for term in required_benefit_terms {
                assert!(
                    entry.benefit.contains(term),
                    "{slug} benefit must mention {term:?}, got: {}",
                    entry.benefit
                );
            }
        }
    }

    #[test]
    fn rendered_body_drops_the_leading_title_h1() {
        // The page chrome renders the workshop title as the document's
        // sole <h1>; the markdown body must not repeat it (the bug:
        // two identical <h1>s on /…/readme).
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("navigator");
        fs::create_dir_all(&target).unwrap();
        fs::write(
            target.join("README.md"),
            "# Runbook\n\nWelcome.\n\n## First step\n\nDo the thing.\n",
        )
        .unwrap();
        let materials = load_navigator(dir.path()).unwrap();
        assert!(
            !materials[0].body_html.contains("<h1>"),
            "rendered body must carry no <h1>, got: {}",
            materials[0].body_html
        );
        // …but the raw markdown the copy button hands back keeps the
        // title so the downloaded file is self-describing.
        assert!(materials[0].raw_markdown.starts_with("# Runbook"));
    }

    #[test]
    fn workshop_splits_into_ordered_chapters_and_sections_with_intro() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("navigator");
        fs::create_dir_all(&target).unwrap();
        fs::write(
            target.join("README.md"),
            "# Title\n\nOrientation lede.\n\n## Intro\n\n### Step one\n\nAlpha.\n\n## Wrap Up\n\n### Step two\n\nBeta.\n",
        )
        .unwrap();
        let m = &load_navigator(dir.path()).unwrap()[0];
        assert!(
            m.intro_html.contains("Orientation lede") && !m.intro_html.contains("<h2"),
            "intro is the pre-heading lede, got: {}",
            m.intro_html
        );
        assert_eq!(m.sections.len(), 2);
        assert_eq!(m.chapters.len(), 2);
        assert_eq!(m.chapters[0].title, "Intro");
        assert_eq!(m.chapters[0].section_start, 0);
        assert_eq!(m.chapters[0].section_count, 1);
        assert_eq!(m.chapters[1].title, "Wrap Up");
        assert_eq!(m.chapters[1].section_start, 1);
        assert_eq!(m.chapters[1].section_count, 1);
        assert_eq!(m.sections[0].title, "Step one");
        assert_eq!(m.sections[1].title, "Step two");
        assert!(m.sections[0].body_html.contains("<h3>Step one</h3>"));
        assert!(m.sections[0].body_html.contains("Alpha"));
        assert!(!m.sections[0].body_html.contains("Beta"));
    }

    #[test]
    fn chapter_preamble_is_preserved_without_becoming_a_slide() {
        let m = material_from_markdown(
            "workshops",
            "preamble",
            "Preamble",
            "A page.",
            "For lawyers",
            "You learn.",
            "# Preamble\n\n## Intro\n\nStart with this orientation.\n\n### First step\n\nFace.\n\n---\n\nNotes.\n",
        );

        assert_eq!(m.sections.len(), 1, "the preamble is not a numbered slide");
        assert!(m.chapters[0]
            .preamble_html
            .contains("Start with this orientation."));
        assert!(!m.sections[0]
            .body_html
            .contains("Start with this orientation."));
    }

    #[test]
    fn strip_leading_h1_only_touches_the_first_top_level_heading() {
        assert_eq!(strip_leading_h1("# Title\n\nBody"), "Body");
        assert_eq!(strip_leading_h1("\n\n# Title\nBody"), "Body");
        // `##` is a section heading, not the title — leave it.
        assert_eq!(strip_leading_h1("## Section\nBody"), "## Section\nBody");
        // No leading H1 at all → unchanged.
        assert_eq!(strip_leading_h1("Just text"), "Just text");
    }

    #[test]
    fn split_chapters_ignores_headings_inside_code_fences() {
        // `##` and `###` lines inside a fenced block are sample text, not
        // chapter or section boundaries.
        let (_intro, chapters) = split_chapters(
            "# T\n\n## Intro\n\n### Real\n\n```\n## not a chapter\n### not a section\n```\n\nEnd.\n",
        );
        assert_eq!(chapters.len(), 1, "only the real ## heading splits");
        assert_eq!(chapters[0].0, "Intro");
        assert_eq!(chapters[0].2.len(), 1);
        assert_eq!(chapters[0].2[0].0, "Real");
        assert!(chapters[0].2[0].1.contains("## not a chapter"));
        assert!(chapters[0].2[0].1.contains("### not a section"));
    }

    #[test]
    fn load_navigator_skips_missing_files_without_error() {
        let dir = TempDir::new().unwrap();
        // Folder exists but README absent — the manifest entry is
        // silently dropped, no error returned.
        fs::create_dir_all(dir.path().join("navigator")).unwrap();
        let materials = load_navigator(dir.path()).unwrap();
        assert!(materials.is_empty());
    }
}
