#![allow(clippy::doc_markdown)]
//! Browser-driven accessibility gate — runs axe-core (WCAG 2.0/2.1
//! level A + AA) against Navigator's public, portal, and lawyer surfaces in a
//! real Chromium session.
//!
//! Same prerequisites and skip policy as `browser_e2e.rs`: a live KIND cluster
//! with `chromedriver` on `$WEBDRIVER_URL`, and Lawyer granted `lawyer`. Not
//! `#[ignore]`'d — it probes for the harness and skips cleanly when
//! absent. `NAV_REQUIRE_HARNESS=1` (the CI setting) makes any unavailable
//! harness fail closed rather than
//! pass green without assertions:
//!
//! ```sh
//! cargo test -p server --test accessibility_e2e -- --test-threads=1
//! ```
//!
//! axe-core is vendored under `tests/assets/axe.min.js` and injected
//! here, at test time, over WebDriver. It is never linked from the app
//! layout and never served to users (see `tests/assets/README.md`).
//!
//! ## How this gate is scoped
//!
//! Deliberately **not** "one axe run per route". The UI is built from a shared
//! component theme, so auditing every page would re-discover the same component
//! defect on each of the twenty pages that mount it, and every new page would
//! have to be remembered into a list. Three layers instead:
//!
//! 1. **The component gate** ([`the_component_gallery_passes_axe`]). `/design`
//!    renders every public component in `webapp::components`, and
//!    `every_public_component_is_shown_in_the_design_gallery` (in
//!    `webapp/src/components.rs`) fails the build when a component is missing
//!    from it. One full-document audit there covers the whole component
//!    surface, so a `FormCard` label defect or an unnamed icon button fails
//!    once, at its source. This is the layer that grows by itself: a new
//!    component cannot be added without entering the gallery this audits.
//! 2. **The shell gate** ([`each_brand_shell_passes_a_full_document_audit`]).
//!    A shell is composed per host and no component renders one alone, so each
//!    brand's chrome gets one full-document audit — `html`, not `main`, which
//!    is what finally puts the shared nav and footer under axe rather than the
//!    four-selector spot check `assert_public_shell` performs.
//! 3. **Content routes.** Pages whose *prose* is the thing under audit: the
//!    marketing and legal copy, the blog, a team profile, the talks. Their
//!    headings, links, and contrast live in the page rather than in a
//!    component, so they are enumerated — scoped to `main`, because the shell
//!    around them is layer 2's job and repeating it here would report one
//!    chrome defect once per route.
//!
//! Route archetypes that differ only in their data — the lawyer listings, the
//! CRUD forms — are sampled rather than enumerated: they are one component with
//! different rows, and layer 1 already audited the component.
//!
//! ## Both colour schemes
//!
//! Light and dark are two arms of `@media (prefers-color-scheme: dark)`, with
//! no toggle to drive. A session therefore renders exactly one of them, and
//! which one depends on the host: CI's Chrome renders light, a developer's Mac
//! usually renders dark. PR #162's contrast defect — the firm's white wordmark
//! resolving against a near-white page — existed only in the light theme, and
//! reddened CI while being invisible to anyone reproducing it locally. Every
//! audit here therefore runs under both schemes, pinned by
//! [`ColorScheme`] and verified by [`assert_color_scheme`] so a browser that
//! ignored the flag fails loudly instead of auditing one scheme twice.
//!
//! ## `violations` versus `incomplete`
//!
//! axe returns two result sets. `violations` are decided failures and fail this
//! gate. `incomplete` are checks axe could not decide — overwhelmingly
//! `color-contrast` on text over a photograph or gradient, where the true
//! background is in pixels axe cannot read.
//!
//! Those are **reported, not failed**, and the distinction is deliberate. An
//! element over a real photograph is permanently undecidable: failing on it
//! would redden the gate on every image-backed band on the site, which converts
//! a signal into noise and gets the check deleted. But silence is what lets a
//! contrast defect sit unmeasured — the state `.home-hero` was in before #162,
//! when nothing on the page declared an opaque colour under the wordmark. So
//! every `incomplete` is printed with the colours axe did resolve, and the one
//! narrow shape that is a real defect rather than a genuine undecidable is
//! asserted on: [`undecidable_contrast_failures`].
//!
//! The cheap deterministic half of this gate — the error pages, which are not
//! routed and so cannot be reached by a browser at all — lives in
//! `webapp/src/error_pages.rs`.

use std::time::Duration;

use fantoccini::{Client, Locator};
use features::webdriver::{
    assert_color_scheme, base_url, login_as_lawyer, new_client_or_skip_in_scheme, ColorScheme,
};

/// axe-core, injected into the page at test time only.
const AXE_SRC: &str = include_str!("assets/axe.min.js");

/// CSS selector axe scopes its audit to for lawyer create forms.
const FORM_AXE_SCOPE: &str = "form.admin-form";

/// CSS selector axe scopes its audit to for a page's own body content, leaving
/// the shared chrome to the per-brand full-document audit.
const PUBLIC_AXE_SCOPE: &str = "main";

/// The whole document, chrome included. Used by the component gate and the
/// per-brand shell audit.
const DOCUMENT_AXE_SCOPE: &str = "html";

/// How one path declared by a brand is audited.
#[derive(Debug, PartialEq, Eq)]
enum AuditPlan {
    /// The concrete URLs to audit, expanded from the declared path.
    Audit(Vec<String>),
    /// Deliberately not audited, with the reason stated.
    Skip(&'static str),
}

/// The concrete URLs to audit for each declared path that carries a parameter.
///
/// A parameterised path with no entry here is a hard failure in
/// [`every_declared_public_path_is_classified`], not a silent skip: registering
/// a new `{slug}` route must come with a fixture to audit it against, or the
/// route joins the public surface with no accessibility coverage at all.
const PARAMETERISED: &[(&str, &[&str])] = &[
    (
        "/blog/{slug}",
        // Both posts. One router, but the bodies differ — `thanks-apple`
        // carries the collage, the other is prose — and a post's body is
        // authored content rather than component output.
        &["/blog/thanks-apple", "/blog/going-all-in-on-rust"],
    ),
    ("/workshops/{slug}", &["/workshops/use-the-navigator"]),
    (
        "/workshops/{slug}/slides",
        &["/workshops/use-the-navigator/slides"],
    ),
    (
        "/workshops/{slug}/step/{step}",
        &["/workshops/use-the-navigator/step/1"],
    ),
    (
        "/workshops/{slug}/display/{step}",
        &["/workshops/use-the-navigator/display/1"],
    ),
    (
        "/workshops/{slug}/certificate/sent",
        &["/workshops/use-the-navigator/certificate/sent"],
    ),
    ("/presentations/{slug}", &["/presentations/rust-in-peace"]),
    (
        "/presentations/{slug}/slides",
        &["/presentations/rust-in-peace/slides"],
    ),
    (
        "/presentations/{slug}/step/{step}",
        &["/presentations/rust-in-peace/step/1"],
    ),
    (
        "/presentations/{slug}/display/{step}",
        &["/presentations/rust-in-peace/display/1"],
    ),
    (
        "/presentations/{slug}/certificate/sent",
        &["/presentations/rust-in-peace/certificate/sent"],
    ),
];

/// Classify one path declared in `neon::PUBLIC_PATHS`.
///
/// This is what stops the gate from being a hand-kept list that drifts behind
/// the routers. The brand constants are the surface's own declaration, so the
/// audit is derived from them rather than restated beside them — a page added
/// to a host enters this gate by existing. A path this cannot classify has no
/// arm to fall into, which fails
/// [`every_declared_public_path_is_classified`] in the ordinary workspace run,
/// long before the browser gate would have quietly not covered it.
fn plan(path: &str) -> AuditPlan {
    // The three crawler documents the firm registers, matched exactly rather
    // than by suffix: a URL path is not a filename, and an exact list also
    // means a new non-HTML route has to be classified rather than slipping
    // through on its extension.
    if ["/robots.txt", "/sitemap.xml", "/llms.txt"].contains(&path) {
        return AuditPlan::Skip("a crawler document, not an HTML page");
    }
    // The certificate request itself is a `POST` handler; the page a reader
    // lands on afterwards is `…/certificate/sent`, which is audited.
    if path.ends_with("/certificate") {
        return AuditPlan::Skip("the POST-only certificate request handler");
    }
    if let Some((_, urls)) = PARAMETERISED.iter().find(|(declared, _)| *declared == path) {
        return AuditPlan::Audit(urls.iter().map(|u| (*u).to_string()).collect());
    }
    if path.contains('{') {
        return AuditPlan::Skip("UNCLASSIFIED");
    }
    AuditPlan::Audit(vec![path.to_string()])
}

/// Expand a brand's declared paths into the concrete URLs to audit.
fn audit_urls(declared: &[&str]) -> Vec<String> {
    declared
        .iter()
        .filter_map(|path| match plan(path) {
            AuditPlan::Audit(urls) => Some(urls),
            AuditPlan::Skip(_) => None,
        })
        .flatten()
        .collect()
}

/// One representative of each portal/lawyer page archetype.
///
/// Sampled rather than enumerated, per the scoping note above: `/lawyer/entities`
/// and `/lawyer/addresses` are the same listing component over different rows,
/// and the component itself is audited at `/design`. What a route adds beyond
/// its components is its *composition* — landmark nesting, heading order, the
/// page's own controls — so one route per archetype is what earns its runtime
/// here.
const PORTAL_ARCHETYPE_ROUTES: &[&str] = &[
    // The blank government-forms index — a listing with per-row download links.
    // The team home that a firm person lands on is audited as a full document in
    // [`the_authenticated_shell_passes_a_full_document_audit`].
    "/app/forms",
    // The lawyer dashboard, which composes KPI tiles and a calendar rather than
    // a listing. `/app/admin` is deliberately absent: the fixture signs in as
    // `lawyer`, and `/app` drops the owner/admin bypass, so that route renders
    // the 403 rather than the admin hub — it is audited as an error page in
    // [`the_error_pages_pass_axe_wcag_a_and_aa`], where that is the point.
    "/app/lawyer",
    // The project list (client-facing chrome) and the lawyer workbench listing.
    "/app/projects",
    // A sortable listing with row actions — a different archetype from the
    // fixed-order listings above.
    "/lawyer/playbooks",
    // A person detail page: a read view rather than a list or a form.
    "/lawyer/entities",
];

/// The lawyer create forms, scoped to the form body.
///
/// These stay enumerated rather than sampled because each is a distinct
/// `FormCard` *instance* — a different field set, and a field set is data the
/// gallery's demo form does not carry.
const LAWYER_FORM_ROUTES: &[&str] = &[
    "/lawyer/entities/new",
    "/app/projects/new",
    "/lawyer/retainers/new",
    // Beyond the four this suite has gated since #120: the playbook create
    // form, which is the only one carrying a textarea.
    //
    // `/admin/people/new` is not here for the same reason `/app/admin` is not
    // an archetype: it is admin-only, and the fixture is `lawyer`, so auditing
    // it would silently audit the 403 instead of the form. Since ENG-304 deleted
    // the `/lawyer/people/new` mirror, that is the whole people-form surface, so
    // this suite no longer audits one — the four below share the same
    // `FormCard`, and the component is audited at `/design`.
    "/lawyer/playbooks/new",
];

/// What one axe run found: decided failures, and the checks it could not
/// decide.
#[derive(Debug, Default)]
struct AxeReport {
    /// WCAG A/AA failures. Any entry fails the gate.
    violations: Vec<String>,
    /// Checks axe could not decide, formatted for the log.
    incomplete: Vec<String>,
    /// The subset of `incomplete` that is a real defect rather than a genuine
    /// undecidable — see [`undecidable_contrast_failures`].
    undecidable_contrast: Vec<String>,
}

/// Inject axe-core and run it over `scope` (a CSS selector), returning both
/// result sets.
async fn axe_report(c: &Client, scope: &str) -> AxeReport {
    c.execute(AXE_SRC, vec![]).await.expect("inject axe-core");
    let raw = c
        .execute_async(
            "const done = arguments[arguments.length - 1];\
             const root = document.querySelector(arguments[0]);\
             if (!root) { done(JSON.stringify({violations: [{id: 'axe-scope-missing', \
               help: 'no element matched ' + arguments[0], impact: 'serious', nodes: []}], \
               incomplete: []})); }\
             else { axe.run(root, {runOnly: {type: 'tag', \
               values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']}})\
               .then(r => done(JSON.stringify({violations: r.violations, \
                 incomplete: r.incomplete})))\
               .catch(e => done(JSON.stringify({violations: [{id: 'axe-run-error', \
                 help: String(e), impact: 'serious', nodes: []}], incomplete: []}))); }",
            vec![serde_json::Value::String(scope.to_owned())],
        )
        .await
        .expect("run axe-core");

    let json = raw.as_str().unwrap_or("{}");
    let parsed: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let results = |key: &str| -> Vec<serde_json::Value> {
        parsed[key].as_array().cloned().unwrap_or_default()
    };

    let incomplete_raw = results("incomplete");
    AxeReport {
        violations: results("violations").iter().map(describe).collect(),
        incomplete: incomplete_raw.iter().map(describe).collect(),
        undecidable_contrast: undecidable_contrast_failures(&incomplete_raw),
    }
}

/// One human-readable line for an axe result.
fn describe(result: &serde_json::Value) -> String {
    let id = result["id"].as_str().unwrap_or("?");
    let impact = result["impact"].as_str().unwrap_or("unknown");
    let help = result["help"].as_str().unwrap_or("");
    let targets: Vec<String> = result["nodes"]
        .as_array()
        .map(|nodes| nodes.iter().map(describe_node).collect())
        .unwrap_or_default();
    format!("[{impact}] {id}: {help} — at {}", targets.join("; "))
}

/// A node's selector, plus the colours axe resolved for it when the check was a
/// contrast check.
///
/// The colours are what make an `incomplete` line actionable: "axe could not
/// decide" says nothing on its own, while "fg #ffffff, bg null" names the
/// element that has no measurable background at all.
fn describe_node(node: &serde_json::Value) -> String {
    let target = node["target"]
        .as_array()
        .map(|t| {
            t.iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let Some(data) = contrast_data(node) else {
        return target;
    };
    let colour = |key: &str| match data[key].as_str() {
        Some(value) => value.to_string(),
        None => "none".to_string(),
    };
    let reason = data["messageKey"].as_str().unwrap_or("undetermined");
    format!(
        "{target} (fg {}, bg {}, reason {reason})",
        colour("fgColor"),
        colour("bgColor"),
    )
}

/// The `color-contrast` check payload on a node, if it carries one.
///
/// axe reports a node's check results under `any`, `all`, and `none`; the
/// contrast check lands in `any`. Reading it by `id` rather than by position
/// keeps this working when a node carries several checks.
fn contrast_data(node: &serde_json::Value) -> Option<&serde_json::Value> {
    node["any"]
        .as_array()?
        .iter()
        .find(|check| check["id"].as_str() == Some("color-contrast"))
        .map(|check| &check["data"])
}

/// The `incomplete` results that are real defects rather than genuine
/// undecidables.
///
/// axe leaves `color-contrast` undecided for two different reasons, and only
/// one of them is the page's fault:
///
/// * **Genuinely undecidable.** The text sits on a photograph, a gradient, or
///   a partially obscuring element, and the real background is in pixels axe
///   cannot sample. axe names the reason in `messageKey` (`bgImage`,
///   `bgGradient`, `imgNode`, `bgOverlap`) *and still resolves a `bgColor`* for
///   the box behind it. A human has to judge these, and failing on them would
///   redden the gate for every image-backed band on the site.
/// * **Nothing to measure.** axe walked the element's ancestors looking for an
///   opaque background and found none, so `bgColor` comes back null. That is
///   not a limit of the tool — it means the page never declares what colour is
///   behind this text, so its contrast is whatever the viewport happens to be.
///   It is exactly the state `.home-hero` was in before PR #162, where the
///   light theme put a white wordmark on a near-white page.
///
/// So the second shape is asserted on and the first is only reported. This
/// keeps the gate quiet about the hero photograph while refusing to stay quiet
/// about text with no declared background at all.
fn undecidable_contrast_failures(incomplete: &[serde_json::Value]) -> Vec<String> {
    incomplete
        .iter()
        .filter(|result| result["id"].as_str() == Some("color-contrast"))
        .flat_map(|result| {
            result["nodes"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|node| {
                    contrast_data(node).is_some_and(|data| {
                        // Null background with no named reason: axe found no
                        // opaque ancestor at all, rather than declining to read
                        // an image it did find.
                        data["bgColor"].is_null() && data["messageKey"].is_null()
                    })
                })
                .map(|node| describe_node(&node))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Navigate to `route`, wait for `scope`, and fail with a readable report if
/// axe finds a WCAG A/AA violation — or text with no measurable background.
///
/// Every `incomplete` is printed whether or not it fails, so the CI log carries
/// the full picture of what axe could not decide on each page.
async fn assert_route_passes_axe_at(
    c: &Client,
    origin: &str,
    route: &str,
    scope: &str,
    scheme: ColorScheme,
) {
    c.goto(&format!("{origin}{route}")).await.unwrap();
    c.wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css(scope))
        .await
        .unwrap_or_else(|error| panic!("`{scope}` never rendered on {route}: {error}"));
    let report = axe_report(c, scope).await;

    if !report.incomplete.is_empty() {
        println!(
            "axe could not decide {} check(s) within `{scope}` on {route} [{}]:\n  {}",
            report.incomplete.len(),
            scheme.label(),
            report.incomplete.join("\n  "),
        );
    }
    assert!(
        report.violations.is_empty(),
        "axe found {} WCAG A/AA violation(s) within `{scope}` on {route} [{}]:\n  {}",
        report.violations.len(),
        scheme.label(),
        report.violations.join("\n  "),
    );
    assert!(
        report.undecidable_contrast.is_empty(),
        "on {route} [{}], within `{scope}`, {} element(s) carry text over no \
         declared background at all — axe found no opaque ancestor to measure \
         against, so the contrast is whatever the viewport happens to be:\n  {}\n\
         Give the element (or an ancestor) an opaque background in the theme's \
         own tokens, as `.home-hero` does.",
        scheme.label(),
        report.undecidable_contrast.len(),
        report.undecidable_contrast.join("\n  "),
    );
}

async fn assert_route_passes_axe(c: &Client, route: &str, scope: &str, scheme: ColorScheme) {
    assert_route_passes_axe_at(c, &base_url(), route, scope, scheme).await;
}

/// Open a session in `scheme`, land it on `origin`, and confirm the page really
/// sees that scheme before anything is audited under its name.
async fn session_in_scheme(scheme: ColorScheme, origin: &str) -> Option<Client> {
    let c = new_client_or_skip_in_scheme(scheme).await?;
    c.goto(origin).await.unwrap();
    assert_color_scheme(&c, scheme).await;
    Some(c)
}

/// The public page contract which axe cannot see when it is scoped to `main`:
/// a named primary navigation landmark, a single main landmark, an h1, and a
/// footer.
async fn assert_public_shell(c: &Client) {
    for selector in [
        "header nav[aria-label='Primary']",
        "main",
        "main h1",
        "footer",
    ] {
        c.wait()
            .at_most(Duration::from_secs(10))
            .for_element(Locator::Css(selector))
            .await
            .unwrap_or_else(|error| panic!("public shell missing `{selector}`: {error}"));
    }
}

/// Layer 1 — the component gate.
///
/// `/design` renders every public component in the theme, kept complete by
/// `every_public_component_is_shown_in_the_design_gallery`. Auditing it as a
/// whole document is what lets the rest of this suite sample routes instead of
/// walking them: a defect in a shared component fails here, once, named by the
/// component rather than by the twentieth page that happens to mount it.
///
/// It sits behind the session boundary (it is a Navigator tool, not a public
/// reference surface), so this signs in first.
#[tokio::test]
async fn the_component_gallery_passes_axe() {
    for scheme in ColorScheme::both() {
        let Some(c) = session_in_scheme(scheme, &base_url()).await else {
            return;
        };
        login_as_lawyer(&c).await;
        assert_route_passes_axe(&c, "/design", DOCUMENT_AXE_SCOPE, scheme).await;
        c.close().await.unwrap();
    }
}

/// Layer 2 — one full-document audit per brand shell.
///
/// Every other public assertion is scoped to `main`, which is what keeps a
/// chrome defect from being reported once per route — but it also meant the
/// shared header, nav, and footer had never been under axe at all, only under
/// [`assert_public_shell`]'s four-selector existence check. This audits the
/// whole document once per brand, which is the smallest thing that actually
/// covers the chrome.
///
/// One shell serves the whole public site, so this audits it over more than one
/// page body: the same header and footer around two different documents is what
/// catches a body that breaks the document under only one of them.
#[tokio::test]
async fn the_public_shell_passes_a_full_document_audit() {
    for scheme in ColorScheme::both() {
        let Some(c) = session_in_scheme(scheme, &base_url()).await else {
            return;
        };
        for path in ["/", "/services", "/navigator"] {
            assert_route_passes_axe(&c, path, DOCUMENT_AXE_SCOPE, scheme).await;
            assert_public_shell(&c).await;
        }
        c.close().await.unwrap();
    }
}

/// The authenticated chrome is a second shell — a different navbar, the
/// impersonation slot, and the app footer — and no public route renders it. The
/// team home at `/app/team` is where a firm person lands, so it is the natural
/// place to audit that shell in full.
#[tokio::test]
async fn the_authenticated_shell_passes_a_full_document_audit() {
    for scheme in ColorScheme::both() {
        let Some(c) = session_in_scheme(scheme, &base_url()).await else {
            return;
        };
        login_as_lawyer(&c).await;
        assert_route_passes_axe(&c, "/app/team", DOCUMENT_AXE_SCOPE, scheme).await;
        c.close().await.unwrap();
    }
}

/// Every path a brand declares is either audited or explicitly skipped.
///
/// The cheap, deterministic half of the coverage rule, and the one that makes
/// the rest of this file stop being a list someone has to remember. It runs in
/// the ordinary workspace pass — no cluster, no browser — so registering a
/// public route without deciding how it is audited fails immediately, rather
/// than silently landing a page that no accessibility gate ever renders.
#[test]
fn every_declared_public_path_is_classified() {
    let unclassified: Vec<&str> = neon::PUBLIC_PATHS
        .iter()
        .filter(|path| plan(path) == AuditPlan::Skip("UNCLASSIFIED"))
        .copied()
        .collect();

    assert!(
        unclassified.is_empty(),
        "these declared public paths carry a parameter with no fixture to audit \
         them against:\n  {}\n\
         Add an entry to `PARAMETERISED` naming the concrete URL(s) the gate \
         should audit. A route that reaches the public surface without one is a \
         route no accessibility test ever renders.",
        unclassified.join("\n  "),
    );

    // Printed so a CI log says what the public gate actually covered, rather
    // than only that it passed.
    let audited = audit_urls(neon::PUBLIC_PATHS);
    println!(
        "public accessibility coverage: {} URL(s) from {} declared paths",
        audited.len(),
        neon::PUBLIC_PATHS.len(),
    );
    // A floor, not a count: it catches a classifier that started skipping real
    // pages, and is set below the true total so adding a page never has to
    // touch it.
    assert!(
        audited.len() >= 20,
        "the derived surface collapsed — {} URLs is far below what the site \
         declares, so something is classifying real pages as skips",
        audited.len(),
    );
}

/// The `incomplete` policy, on the two shapes it has to tell apart.
///
/// Both are `color-contrast` results axe declined to decide, and the whole
/// value of the policy is that it treats them differently — so it is pinned
/// here rather than left to be inferred from a live page. Against the real
/// site every undecidable carries a reason (`bgGradient`, `bgOverlap`,
/// `elmPartiallyObscured`, `pseudoContent`, `shortTextContent`), which is
/// exactly why failing on `incomplete` wholesale would be noise; this test is
/// what keeps the narrow case from being dead code.
#[test]
fn the_incomplete_policy_separates_undecidable_from_unmeasurable() {
    let result = |data: serde_json::Value| {
        serde_json::json!([{
            "id": "color-contrast",
            "impact": "serious",
            "help": "Elements must meet minimum color contrast ratio thresholds",
            "nodes": [{ "target": [".subject"], "any": [{"id": "color-contrast", "data": data}] }],
        }])
    };

    // Text over a photograph or gradient: axe names why it could not decide,
    // and a human has to judge it. Reported, never failed — the alternative
    // reddens the gate on every image-backed band the site has.
    let undecidable = result(serde_json::json!({
        "fgColor": "#ffffff", "bgColor": null, "messageKey": "bgImage",
    }));
    assert!(
        undecidable_contrast_failures(undecidable.as_array().unwrap()).is_empty(),
        "an image-backed element is a genuine undecidable, not a failure"
    );

    // No background found at all, and no reason given: the page never declares
    // what colour is behind this text, so its contrast is whatever the viewport
    // happens to be. That is a defect, and it is the state `.home-hero` was in
    // before PR #162 put an opaque shade under the wordmark.
    let unmeasurable = result(serde_json::json!({
        "fgColor": "#ffffff", "bgColor": null, "messageKey": null,
    }));
    let failures = undecidable_contrast_failures(unmeasurable.as_array().unwrap());
    assert_eq!(
        failures.len(),
        1,
        "text with no declared background at all must fail: {failures:?}"
    );
    assert!(
        failures[0].contains(".subject"),
        "the failure names the element: {failures:?}"
    );
}

/// The whole public surface, in one signed-in session.
///
/// Derived from `neon::PUBLIC_PATHS`. A lawyer session reads every page a
/// stranger can plus the gated ones, so one pass covers the host;
/// [`the_public_shell_passes_a_full_document_audit`] separately proves the
/// anonymous chrome.
#[tokio::test]
async fn neon_public_pages_pass_axe_wcag_a_and_aa() {
    let routes = audit_urls(neon::PUBLIC_PATHS);
    for scheme in ColorScheme::both() {
        let Some(c) = session_in_scheme(scheme, &base_url()).await else {
            return;
        };
        login_as_lawyer(&c).await;
        for route in &routes {
            assert_route_passes_axe(&c, route, PUBLIC_AXE_SCOPE, scheme).await;
        }
        c.close().await.unwrap();
    }
}

/// The whole public surface, derived from `neon::PUBLIC_PATHS` — both faces,
/// since one table now declares them.
///
/// Signed in as Lawyer so the same pass also reaches the gated
/// reading surface; workshops and presentations themselves are anonymous.
#[tokio::test]
async fn public_pages_pass_axe_wcag_a_and_aa() {
    let routes = audit_urls(neon::PUBLIC_PATHS);
    for scheme in ColorScheme::both() {
        let Some(c) = session_in_scheme(scheme, &base_url()).await else {
            return;
        };
        login_as_lawyer(&c).await;
        for route in &routes {
            assert_route_passes_axe(&c, route, PUBLIC_AXE_SCOPE, scheme).await;
        }
        c.close().await.unwrap();
    }
}

/// The portal and lawyer surfaces, one route per archetype.
#[tokio::test]
async fn portal_and_lawyer_surfaces_pass_axe_wcag_a_and_aa() {
    for scheme in ColorScheme::both() {
        let Some(c) = session_in_scheme(scheme, &base_url()).await else {
            return;
        };
        login_as_lawyer(&c).await;
        for route in PORTAL_ARCHETYPE_ROUTES {
            assert_route_passes_axe(&c, route, PUBLIC_AXE_SCOPE, scheme).await;
        }
        c.close().await.unwrap();
    }
}

#[tokio::test]
async fn portal_create_forms_pass_axe_wcag_a_and_aa() {
    for scheme in ColorScheme::both() {
        let Some(c) = session_in_scheme(scheme, &base_url()).await else {
            return;
        };
        login_as_lawyer(&c).await;
        for route in LAWYER_FORM_ROUTES {
            assert_route_passes_axe(&c, route, FORM_AXE_SCOPE, scheme).await;
        }
        c.close().await.unwrap();
    }
}

/// The 404 as a real response, not a rendered string.
///
/// The error pages are returned inline from ~50 handlers rather than routed
/// ([`webapp::error_pages`]), so a browser can only reach two of them: the 404
/// on any unknown path, and the 403 an authenticated non-lawyer visitor gets
/// from a lawyer route. Those two are audited here; the whole family, including
/// the 500 and the signed-in variants a browser cannot produce without
/// fabricating state, is covered deterministically by
/// `every_error_page_meets_the_document_accessibility_contract`.
#[tokio::test]
async fn the_error_pages_pass_axe_wcag_a_and_aa() {
    for scheme in ColorScheme::both() {
        let Some(c) = session_in_scheme(scheme, &base_url()).await else {
            return;
        };
        // Both audited as whole documents: an error page carries its own
        // hand-built shell rather than a router's, so `main` alone would leave
        // that shell — the one place a header is built outside `PublicShell` —
        // unaudited.
        //
        // Anonymous 404 first, because it renders the signed-out header.
        assert_route_passes_axe(&c, "/this-path-does-not-exist", DOCUMENT_AXE_SCOPE, scheme).await;

        // Then the 403, reached the way a real visitor reaches it: signed in as
        // `lawyer` and opening an admin-only route. `/app` drops the owner/admin
        // bypass, so this is the policy's own refusal rather than a fabricated
        // page.
        login_as_lawyer(&c).await;
        assert_route_passes_axe(&c, "/app/admin", DOCUMENT_AXE_SCOPE, scheme).await;
        c.close().await.unwrap();
    }
}

/// The public surface's two interactive image affordances: the home page's call
/// to action, and the blog collage's lightbox dialog.
///
/// There is deliberately no `/team` leg. This test used to open `/team` and
/// assert its profile photo carried alternative text, but the public surface
/// declares no team page — `neon::PUBLIC_PATHS` is the whole declaration and has
/// none, and nothing in the tree renders `.team-profile-card`, so the route
/// `404`s and the wait timed out rather than failing an accessibility claim.
/// Removing a page orphaned the assertion about it; the check that survived the
/// page's removal is the one below, on markup that still ships.
#[tokio::test]
async fn public_navigation_images_and_collage_dialog_are_accessible() {
    let site_base_url = base_url();
    let Some(c) = session_in_scheme(ColorScheme::Light, &site_base_url).await else {
        return;
    };

    assert_public_shell(&c).await;
    c.find(Locator::Css("a.home-statement__cta"))
        .await
        .expect("the home page has its contact call to action")
        .click()
        .await
        .unwrap();
    c.wait()
        .at_most(Duration::from_secs(10))
        .for_url(&url::Url::parse(&format!("{site_base_url}/contact")).unwrap())
        .await
        .expect("the home call to action navigates to the contact route");

    c.goto(&format!("{site_base_url}/blog/thanks-apple"))
        .await
        .unwrap();
    let trigger = c
        .wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css(
            ".blog-collage img[role='button'][aria-haspopup='dialog']",
        ))
        .await
        .expect("collage image becomes a named keyboard-operable dialog trigger");
    assert!(
        trigger
            .attr("aria-label")
            .await
            .unwrap()
            .is_some_and(|label| !label.is_empty()),
        "collage trigger must have an accessible name"
    );
    c.execute(
        "const trigger = document.querySelector(arguments[0]); \
         trigger.focus(); trigger.click(); return document.activeElement === trigger;",
        vec![serde_json::Value::String(
            ".blog-collage img[role='button'][aria-haspopup='dialog']".to_string(),
        )],
    )
    .await
    .expect("open collage dialog");
    let dialog = c
        .wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css(
            ".collage-lightbox[role='dialog'][aria-modal='true']:not([hidden])",
        ))
        .await
        .expect("collage dialog opens from its trigger");
    dialog
        .find(Locator::Css("button[aria-label='Close']"))
        .await
        .expect("dialog exposes a named close control")
        .click()
        .await
        .unwrap();
    c.wait()
        .at_most(Duration::from_secs(10))
        .for_element(Locator::Css(".collage-lightbox[hidden]"))
        .await
        .expect("close control hides the collage dialog");
    c.close().await.unwrap();
}
