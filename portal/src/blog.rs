//! The firm blog served at `/blog` (index) and `/blog/:slug` (one post).
//!
//! One markdown file per post under `server/content/blog/`, named
//! `YYYYMMDD_slug.md` (e.g. `20260619_thanks_apple.md`). Adding a post is
//! "drop a dated markdown file in the directory" — the loader walks the
//! tree at boot and builds an in-memory index, mirroring `about`/
//! `marketing`.
//!
//! The `YYYYMMDD` prefix is the publish date — the one piece of metadata
//! we derive from the filename rather than the front-matter. It sorts the
//! index newest-first and dates each post. Everything after the first `_`
//! is the slug, lowered to kebab-case for the URL (`/blog/<slug>`, so
//! `thanks_apple` is served at `/blog/thanks-apple`). A file whose prefix
//! is not a valid date is skipped with a warning rather than failing the
//! boot.
//!
//! Front-matter (`title`, `description`) and the markdown body are parsed
//! by the shared [`marketing::loader`], so a post file is shaped exactly
//! like a marketing fragment plus the dated filename convention.

use std::path::Path;
use std::sync::Arc;

use chrono::NaiveDate;
use walkdir::WalkDir;

use crate::content_loader::ContentLoadError;
use crate::marketing;

/// File basenames inside the blog tree that are NOT posts.
const NON_POST_FILES: &[&str] = &["README.md", ".gitkeep"];

/// One published blog post. Built from a dated markdown file's
/// front-matter plus body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlogPost {
    /// Routing key — the part of the filename after the `YYYYMMDD_`
    /// date prefix, in kebab-case (`thanks_apple` → `thanks-apple`).
    /// Served at `/blog/<slug>`.
    pub slug: String,
    /// Publish date, parsed from the filename's `YYYYMMDD` prefix.
    pub date: NaiveDate,
    /// Post title (front-matter `title`).
    pub title: String,
    /// One-line summary (front-matter `description`); used for the
    /// index blurb and the per-post `<meta description>`.
    pub description: String,
    /// Rendered HTML body (NOT raw markdown).
    pub body_html: String,
}

/// `Arc`-wrapped lookup shared as router state. Cheap to clone.
#[derive(Debug, Clone, Default)]
pub struct BlogIndex {
    posts: Arc<Vec<BlogPost>>,
}

impl BlogIndex {
    #[must_use]
    pub fn new(posts: Vec<BlogPost>) -> Self {
        Self {
            posts: Arc::new(posts),
        }
    }

    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// All posts, newest first.
    #[must_use]
    pub fn posts(&self) -> &[BlogPost] {
        &self.posts
    }

    /// Look up one post by its slug.
    #[must_use]
    pub fn get(&self, slug: &str) -> Option<&BlogPost> {
        self.posts.iter().find(|p| p.slug == slug)
    }

    /// `true` when no posts are loaded — the index renders an empty
    /// state.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.posts.is_empty()
    }
}

/// Split a post filename stem into its `(date, slug)` parts. Returns
/// `None` when the stem has no `_`, an empty slug, or a prefix that
/// isn't a valid `YYYYMMDD` date — the loader then skips the file.
///
/// The slug is the canonical kebab-case URL form
/// ([`views::slug::to_url`]): `20260619_thanks_apple.md` is served at
/// `/blog/thanks-apple`, so the dated-underscore filename convention and
/// the hyphenated URL convention stay decoupled.
fn parse_post_filename(stem: &str) -> Option<(NaiveDate, String)> {
    let (date_part, slug) = stem.split_once('_')?;
    if slug.is_empty() {
        return None;
    }
    let date = NaiveDate::parse_from_str(date_part, "%Y%m%d").ok()?;
    Some((date, views::slug::to_url(slug)))
}

/// Walk `dir` for blog posts. Returns an empty index (not an error)
/// when `dir` doesn't exist, so a fork with no blog yet boots cleanly.
pub fn load_dir(dir: &Path) -> Result<BlogIndex, ContentLoadError> {
    let mut posts = Vec::new();
    if !dir.exists() {
        return Ok(BlogIndex::empty());
    }
    for entry in WalkDir::new(dir).follow_links(false) {
        let entry = entry.map_err(|e| ContentLoadError::Io {
            path: dir.display().to_string(),
            source: std::io::Error::other(e),
        })?;
        let path = entry.path();
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if NON_POST_FILES.contains(&name) {
            continue;
        }
        if path.extension().and_then(|x| x.to_str()) != Some("md") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let Some((date, slug)) = parse_post_filename(stem) else {
            tracing::warn!(
                file = name,
                "skipping blog file: name is not YYYYMMDD_slug.md"
            );
            continue;
        };
        let raw = std::fs::read_to_string(path).map_err(|e| ContentLoadError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        let doc =
            marketing::loader::parse(&raw, &slug).ok_or(ContentLoadError::MissingFrontmatter {
                path: path.display().to_string(),
            })?;
        posts.push(BlogPost {
            slug,
            date,
            title: doc.title,
            description: doc.description,
            body_html: doc.body_html,
        });
    }
    // Newest first; ties (same day) break on slug for a deterministic
    // order in tests.
    posts.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.slug.cmp(&b.slug)));
    Ok(BlogIndex::new(posts))
}

#[cfg(test)]
mod tests {
    use super::{load_dir, parse_post_filename, BlogIndex};
    use chrono::NaiveDate;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn empty_index_is_empty() {
        let ix = BlogIndex::empty();
        assert!(ix.is_empty());
        assert!(ix.posts().is_empty());
        assert!(ix.get("anything").is_none());
    }

    #[test]
    fn bundled_posts_directory_loads_cleanly() {
        // Guards the real `server/content/blog/` tree and documents the
        // authoring contract by example: every shipped post has a
        // `YYYYMMDD_slug.md` name whose prefix is the publish date and
        // whose remainder is the URL slug, plus `title`/`description`
        // front-matter. The first post — `20260619_thanks_apple.md` —
        // is served at the kebab-case URL `/blog/thanks-apple`, dated
        // 2026-06-19.
        let ix = load_dir(std::path::Path::new(crate::DEFAULT_BLOG_DIR)).unwrap();
        let post = ix
            .get("thanks-apple")
            .expect("first post should load from the bundled blog dir at slug `thanks-apple`");
        assert_eq!(post.date, NaiveDate::from_ymd_opt(2026, 6, 19).unwrap());
        assert_eq!(post.title, "Thanks, Apple");
        assert!(
            !post.description.is_empty(),
            "every post needs a description"
        );
        assert!(!post.body_html.is_empty(), "every post needs a body");
    }

    #[test]
    fn thanks_apple_collage_renders_as_a_bootstrap_grid_routed_through_the_asset_seam() {
        // The post leads with the rainbow photo as a big standalone picture,
        // then closes with a Bootstrap grid (`.row.blog-collage` of sixteen
        // `.col` tiles) that absorbs the four photos that used to sit
        // inline in the letter plus a later row of farewell snapshots. Each
        // tile is authored as a markdown `![]()`
        // separated by blank lines so pulldown-cmark still parses it — and
        // therefore routes its `src` through the asset seam (`/public` in
        // tests) — even though it sits inside raw HTML. This pins the lead,
        // the grid shape, and that every tile is a real resolved `<img>`.
        let ix = load_dir(std::path::Path::new(crate::DEFAULT_BLOG_DIR)).unwrap();
        let post = ix.get("thanks-apple").expect("thanks-apple post loads");
        // The Bootstrap grid wrapper passed through as raw HTML.
        assert!(
            post.body_html.contains("class=\"row g-2 blog-collage\""),
            "collage must render as a Bootstrap row, got: {}",
            post.body_html
        );
        // Sixteen tiles total: fifteen standard `col-md-4` squares plus the
        // Apple Park sunset, widened to a full-row `col-md-12` desktop banner.
        assert_eq!(
            post.body_html.matches("col-6 col-md-4").count(),
            15,
            "the grid must hold fifteen standard tiles, got: {}",
            post.body_html
        );
        assert!(
            post.body_html
                .contains("class=\"col-6 col-md-12 blog-collage-wide\""),
            "the sunset must render as a full-width desktop banner, got: {}",
            post.body_html
        );
        // Every collage photo — including the four moved in from the letter
        // and the new Sharks shot — resolves through the asset seam.
        for slug in [
            "collage-1",
            "collage-8",
            "apple-park-team",
            "ethiopian-dinner",
            "team-lunch",
            "london-tower-bridge",
            "sharks-game",
            "apple-park-sunset",
            "farewell-crew",
            "curry-night",
            "travels-abroad",
        ] {
            assert!(
                post.body_html
                    .contains(&format!("src=\"/public/img/thanks-apple/{slug}.jpg\"")),
                "collage tile `{slug}` must resolve through the asset seam, got: {}",
                post.body_html
            );
        }
        // The rainbow leads as a big picture BEFORE the grid; the grid now
        // closes on the full-width Apple Park sunset banner, which sits last —
        // after the apple-park-team group photo.
        let grid = post
            .body_html
            .find("blog-collage")
            .expect("grid wrapper present");
        let rainbow = post
            .body_html
            .find("collage-6.jpg")
            .expect("rainbow lead present");
        assert!(
            rainbow < grid,
            "the rainbow must lead as a big picture before the collage grid"
        );
        let team = post
            .body_html
            .find("apple-park-team.jpg")
            .expect("apple-park-team tile present");
        let sunset = post
            .body_html
            .rfind("apple-park-sunset.jpg")
            .expect("sunset tile present");
        assert!(
            sunset > team && sunset > grid,
            "the grid must close on the full-width Apple Park sunset banner"
        );
        // The old bullet-list collage is gone.
        assert!(
            !post.body_html.contains("<ul>"),
            "the collage must no longer render as a bullet list"
        );
    }

    #[test]
    fn going_all_in_on_rust_leads_with_the_gcs_backed_ferris_nlf_art() {
        let ix = load_dir(std::path::Path::new(crate::DEFAULT_BLOG_DIR)).unwrap();
        let post = ix
            .get("going-all-in-on-rust")
            .expect("going-all-in-on-rust post loads");
        let ferris_src =
            "src=\"/public/img/going-all-in-on-rust/ferris-rust-logo-nlf-20260705.png\"";
        // "Leads with" is an ordering promise, not just presence: the Ferris
        // art must be the FIRST image and sit ahead of the body prose, so a
        // later-inserted image can't quietly displace it as the lead.
        let first_img = post.body_html.find("<img").unwrap_or_else(|| {
            panic!(
                "the Rust post should render the Ferris artwork, got: {}",
                post.body_html
            )
        });
        // The first `<img` tag in the rendered body must be the Ferris art:
        // its `src` falls before any subsequent image opens.
        let next_img = post.body_html[first_img + 1..]
            .find("<img")
            .map_or(post.body_html.len(), |rel| first_img + 1 + rel);
        let lead_img = &post.body_html[first_img..next_img];
        assert!(
            lead_img.contains(ferris_src),
            "the Ferris artwork must be the FIRST image, got: {}",
            post.body_html
        );
        let prose_at = post
            .body_html
            .find("Most of our work arrives the way yours probably does")
            .expect("post renders its opening line");
        assert!(
            first_img < prose_at,
            "the Ferris artwork must lead, ahead of the body prose, got: {}",
            post.body_html
        );
    }

    #[test]
    fn load_dir_returns_empty_index_when_directory_missing() {
        let ix = load_dir(std::path::Path::new("/no/such/blog/dir/xyz")).unwrap();
        assert!(ix.is_empty());
    }

    #[test]
    fn parses_date_and_slug_from_filename() {
        let (date, slug) = parse_post_filename("20260619_thanks_apple").unwrap();
        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 6, 19).unwrap());
        // The slug is the kebab-case URL form, even though the file name
        // keeps its underscores.
        assert_eq!(slug, "thanks-apple");
    }

    #[test]
    fn rejects_filenames_that_are_not_dated() {
        assert!(parse_post_filename("thanks_apple").is_none());
        assert!(parse_post_filename("nodate").is_none());
        assert!(parse_post_filename("20261301_bad_month").is_none());
        assert!(parse_post_filename("20260619_").is_none());
    }

    #[test]
    fn load_dir_reads_a_dated_post() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("20260619_thanks_apple.md"),
            "---\n\
             title: Thanks, Apple\n\
             description: A short note of thanks.\n\
             ---\n\
             We want to say thank you.\n",
        )
        .unwrap();
        let ix = load_dir(tmp.path()).unwrap();
        let posts = ix.posts();
        assert_eq!(posts.len(), 1);
        let p = &posts[0];
        assert_eq!(p.slug, "thanks-apple");
        assert_eq!(p.date, NaiveDate::from_ymd_opt(2026, 6, 19).unwrap());
        assert_eq!(p.title, "Thanks, Apple");
        assert_eq!(p.description, "A short note of thanks.");
        assert!(p.body_html.contains("thank you"));
        assert!(ix.get("thanks-apple").is_some());
    }

    #[test]
    fn load_dir_sorts_newest_first_and_skips_readme_and_undated() {
        let tmp = TempDir::new().unwrap();
        let post = |title: &str| format!("---\ntitle: {title}\n---\n{title}\n");
        fs::write(tmp.path().join("20260101_older.md"), post("Older")).unwrap();
        fs::write(tmp.path().join("20260619_newer.md"), post("Newer")).unwrap();
        fs::write(tmp.path().join("README.md"), "# not a post\n").unwrap();
        fs::write(
            tmp.path().join("draft.md"),
            "---\ntitle: Draft\n---\nundated\n",
        )
        .unwrap();
        let ix = load_dir(tmp.path()).unwrap();
        let slugs: Vec<&str> = ix.posts().iter().map(|p| p.slug.as_str()).collect();
        assert_eq!(slugs, vec!["newer", "older"]);
    }
}
