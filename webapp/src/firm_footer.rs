//! The `/app` footer: a single centered copyright line naming the Firm that
//! wears the current brand, resolved per request rather than a process-wide
//! constant (ENG-589).
//!
//! [`FirmFooterModel`] is also the shape `crate::public_chrome::inject_public_utility`
//! (in `portal`) reads to override the public chrome's "Our Family" row with
//! the live Firm data once one exists — see [`compiled_family_brands`], which
//! that chrome calls directly for its own compiled-time fallback. `brands`
//! stays on this model for that reason even though [`FirmFooter`] itself no
//! longer renders it.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// One brand the resolved Firm wears.
///
/// `href` is empty for a runtime-created brand with no dedicated host to
/// link to yet (a later issue gives one).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct FirmFooterBrand {
    pub label: String,
    pub href: String,
    pub current: bool,
    /// The compiled brand mark shown beside the name in public family rows.
    #[serde(default)]
    pub logo_href: String,
    /// What this brand actually does, in a few words. Empty for a
    /// runtime-created brand that has no compiled line.
    #[serde(default)]
    pub byline: String,
}

/// The data the `/app` footer needs, resolved once per request from the Firm
/// that wears the current brand — never a process-wide constant.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct FirmFooterModel {
    pub legal_entity: String,
    /// Every brand the Firm wears, in registry order with the current one
    /// flagged. Not rendered by [`FirmFooter`]; kept for
    /// `inject_public_utility`'s public-chrome override. See the module docs.
    pub brands: Vec<FirmFooterBrand>,
    pub copyright_year: i32,
}

/// The `/app` footer: one centered copyright line naming the resolved Firm's
/// legal entity.
#[component]
pub fn FirmFooter(model: FirmFooterModel) -> Element {
    rsx! {
        footer { class: "app-footer",
            p { class: "app-footer__copyright", "© {model.copyright_year} {model.legal_entity}" }
        }
    }
}

/// Render the footer to a standalone HTML string for injection into every
/// `/app` HTML response. Server-only: `dioxus-ssr` is not in the wasm bundle.
#[cfg(feature = "server")]
#[must_use]
pub fn render_firm_footer(model: FirmFooterModel) -> String {
    let mut dom = VirtualDom::new_with_props(FirmFooter, FirmFooterProps { model });
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

/// The compiled "Our Family" row for a request on `current`: every house
/// brand the request-scoped firm trades under (`views::brand::firm_family`),
/// in registry order, each named by its compiled wordmark and linking its
/// production home, with `current` flagged. Empty under a bundle that renamed
/// the firm, whose footer must not list another firm's brands.
///
/// Shared by [`compiled_firm_footer_model`] and the public chrome, so the two
/// cannot disagree about who is in the family before a Firm row overrides
/// both.
#[cfg(feature = "server")]
#[must_use]
pub fn compiled_family_brands(current: views::brand::BrandKey) -> Vec<FirmFooterBrand> {
    views::brand::firm_family()
        .iter()
        // "Our Family" is a set of links, so it lists only brands a reader
        // can actually reach — the launch gate's set, the same one
        // `portal::canonical_host` admits and `cli::devx::ship` renders
        // certificates for. A brand is registered here well before its host
        // serves anything; see `views::brand::BrandKey::is_live`.
        //
        // `current` is retained beyond that set for exactly one caller: a
        // developer previewing a held-out brand through its
        // `local_port_env_var` door, where a footer that omitted the brand
        // being previewed would be the wrong preview. A public request can
        // no longer arrive wearing a held-out key at all — the router
        // refuses those hosts — so on a real host this clause is
        // unreachable, and `the_family_row_lists_only_admitted_brands`
        // holds that.
        .filter(|key| key.is_live() || **key == current)
        .map(|key| FirmFooterBrand {
            label: compiled_footer_label(*key),
            href: key.public_home_href(),
            current: *key == current,
            logo_href: key
                .resolve_branding(&views::brand::DEFAULT_BRANDING)
                .firm
                .logo_href
                .to_string(),
            byline: key.family_byline().to_string(),
        })
        .collect()
}

/// The concise names the footer uses for the two public family entries whose
/// masthead names are longer or more specific than their family labels.
#[cfg(feature = "server")]
fn compiled_footer_label(key: views::brand::BrandKey) -> String {
    match key {
        views::brand::BrandKey::Neon => "Neon Law".to_string(),
        views::brand::BrandKey::DeleteYourData => "DeleteYourData.com".to_string(),
        _ => key
            .resolve_branding(&views::brand::DEFAULT_BRANDING)
            .firm
            .site_name
            .to_string(),
    }
}

/// The no-store fallback: the compiled `Branding` for `current`, with the
/// compiled family as its "Our Family" row. What a fresh deployment (no
/// `firm` rows yet) or a brand key no Firm wears resolves to.
#[cfg(feature = "server")]
#[must_use]
pub fn compiled_firm_footer_model(
    current: views::brand::BrandKey,
    copyright_year: i32,
) -> FirmFooterModel {
    let branding = current.resolve_branding(&views::brand::DEFAULT_BRANDING);
    let mut brands = compiled_family_brands(current);
    // A firm with no compiled family (a renamed white-label bundle) still
    // names its one brand, so the model is never empty of identity.
    if brands.is_empty() {
        brands.push(FirmFooterBrand {
            label: compiled_footer_label(current),
            href: String::new(),
            current: true,
            logo_href: current
                .resolve_branding(&views::brand::DEFAULT_BRANDING)
                .firm
                .logo_href
                .to_string(),
            byline: current.family_byline().to_string(),
        });
    }
    FirmFooterModel {
        legal_entity: branding.firm.legal_entity.to_string(),
        brands,
        copyright_year,
    }
}

/// Resolve [`FirmFooterModel`] for `current`: the Firm wearing that brand
/// key, its Entity's legal name, and every brand key it wears (in registry
/// order, current first), each named from its live `brand` row — a
/// runtime-created brand is listed exactly like a compiled one, never
/// silently dropped. No Firm wearing `current` (or no live rows at all)
/// falls back to [`compiled_firm_footer_model`].
///
/// # Errors
///
/// Never returns an error: any store failure degrades to the compiled
/// fallback rather than breaking every page's footer.
#[cfg(feature = "server")]
pub async fn resolve_firm_footer_model(
    surreal: &store::surreal::SurrealDb,
    current: views::brand::BrandKey,
    copyright_year: i32,
) -> FirmFooterModel {
    let fallback = || compiled_firm_footer_model(current, copyright_year);

    let Ok(Some(firm_id)) = store::firms::firm_id_for_brand_key(surreal, current.as_str()).await
    else {
        return fallback();
    };
    let Ok(Some(firm)) = store::firms::find_by_id(surreal, firm_id).await else {
        return fallback();
    };
    let Some(entity_id) = firm.entity_id else {
        return fallback();
    };
    let Ok(Some(entity)) = store::entities::find_by_id(surreal, entity_id).await else {
        return fallback();
    };
    let Ok(keys) = store::firms::brand_keys_for_firm(surreal, firm_id).await else {
        return fallback();
    };
    if keys.is_empty() {
        return fallback();
    }

    let mut brands = Vec::with_capacity(keys.len());
    for key in &keys {
        let Ok(Some(brand)) = store::brands::find_by_key(surreal, key).await else {
            continue;
        };
        let compiled = views::brand::BrandKey::ALL
            .iter()
            .find(|candidate| candidate.as_str() == key);
        // Daybridge's public family row intentionally omits the Lawyer Shook
        // holding page, whether the compiled fallback or live Firm rows feed
        // the footer.
        if current == views::brand::BrandKey::Daybridge
            && compiled == Some(&views::brand::BrandKey::LawyerShook)
        {
            continue;
        }
        // Same reachability rule the compiled fallback applies: a row in the
        // `brand` table does not mean a host serves it. A runtime-created
        // brand has no compiled key and so no launch state — it is listed,
        // because nothing here knows better than the operator who made it.
        if compiled.is_some_and(|candidate| !candidate.is_live())
            && key.as_str() != current.as_str()
        {
            continue;
        }
        brands.push(FirmFooterBrand {
            label: compiled
                .map(|key| compiled_footer_label(*key))
                .unwrap_or(brand.name),
            href: compiled
                .map(|key| key.public_home_href())
                .unwrap_or_default(),
            current: key == current.as_str(),
            logo_href: compiled
                .map(|key| {
                    key.resolve_branding(&views::brand::DEFAULT_BRANDING)
                        .firm
                        .logo_href
                        .to_string()
                })
                .unwrap_or_default(),
            // A runtime brand carries no compiled line; a compiled one does.
            byline: compiled
                .map(|key| key.family_byline().to_string())
                .unwrap_or_default(),
        });
    }
    if brands.is_empty() {
        return fallback();
    }

    FirmFooterModel {
        legal_entity: entity.name,
        brands,
        copyright_year,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssr(app: fn() -> Element) -> String {
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    fn model(brands: Vec<FirmFooterBrand>) -> FirmFooterModel {
        FirmFooterModel {
            legal_entity: "Shook Law PLLC".to_string(),
            brands,
            copyright_year: 2026,
        }
    }

    #[test]
    fn the_footer_renders_a_centered_copyright_line() {
        fn app() -> Element {
            rsx! {
                FirmFooter { model: model(vec![]) }
            }
        }
        let html = ssr(app);
        assert!(html.contains("© 2026 Shook Law PLLC"), "{html}");
        assert!(html.contains("app-footer__copyright"), "{html}");
        assert!(html.starts_with("<footer"), "{html}");
    }

    /// The footer renders nothing beyond the copyright line, whatever the
    /// model's brands carry — `brands` rides the model only for
    /// `inject_public_utility`'s public-chrome override.
    #[test]
    fn the_footer_renders_only_the_copyright_line() {
        fn app() -> Element {
            rsx! {
                FirmFooter {
                    model: model(vec![
                        FirmFooterBrand {
                            label: "Neon Law".to_string(),
                            href: "https://www.neonlaw.com".to_string(),
                            current: true,
                            logo_href: "/public/logo.svg".to_string(),
                            byline: "flat-fee legal services for emerging tech".to_string(),
                        },
                        FirmFooterBrand {
                            label: "DeleteYourData.com".to_string(),
                            href: "https://www.deleteyourdata.com".to_string(),
                            current: false,
                            logo_href: "/public/brand/delete-your-data/logo.svg".to_string(),
                            byline: String::new(),
                        },
                    ]),
                }
            }
        }
        let html = ssr(app);
        assert!(!html.contains("Neon Law"), "{html}");
        assert!(!html.contains("DeleteYourData.com"), "{html}");
        assert!(!html.contains("<nav"), "{html}");
        assert!(!html.contains("<a "), "{html}");
    }

    /// Every class the footer emits is styled by the theme it ships with.
    /// Mirrors `sample_matters_banner`'s own guard: a renamed class with no
    /// matching rule renders as unstyled text and nothing else catches it.
    #[test]
    fn every_class_the_footer_emits_is_styled_by_the_theme() {
        fn app() -> Element {
            rsx! {
                FirmFooter { model: model(vec![]) }
            }
        }

        let theme =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../server/public/css/theme.css");
        let css = std::fs::read_to_string(&theme)
            .unwrap_or_else(|e| panic!("the theme stylesheet must be readable: {e}"));
        let out = ssr(app);

        let mut classes = Vec::new();
        let mut rest = out.as_str();
        while let Some(at) = rest.find("class=\"") {
            rest = &rest[at + 7..];
            let Some(end) = rest.find('"') else { break };
            classes.extend(rest[..end].split_whitespace().map(str::to_string));
            rest = &rest[end..];
        }
        assert!(!classes.is_empty(), "the footer emits classes: {out}");

        for class in classes
            .iter()
            .filter(|class| class.starts_with("app-footer"))
        {
            assert!(
                css.contains(&format!(".{class}")),
                "`{class}` is emitted by the footer but has no rule in \
                 server/public/css/theme.css"
            );
        }
    }

    /// The compiled fallback still resolves the whole live compiled family —
    /// every house brand, in registry order, the request's key current and
    /// every other linking its production home — for `inject_public_utility`
    /// to read, even though `FirmFooter` itself renders none of it.
    #[cfg(feature = "server")]
    #[test]
    fn the_compiled_fallback_resolves_the_legal_entity_and_the_whole_family() {
        let model = compiled_firm_footer_model(views::brand::BrandKey::DeleteYourData, 2026);
        let labels: Vec<&str> = model.brands.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Neon Law",
                "DeleteYourData.com",
                "DeleteYourDebt.com",
                "Vesta Estate Planning",
                "Misericordia Injury Law",
                "Abhaya Immigration",
                "Lawyer Shook",
                "Summons Defense"
            ]
        );
        let current: Vec<bool> = model.brands.iter().map(|b| b.current).collect();
        assert_eq!(
            current,
            [false, true, false, false, false, false, false, false]
        );
        assert_eq!(model.brands[0].href, "https://www.neonlaw.com");
        assert_eq!(model.brands[6].href, "https://www.lawyershook.com");
        assert_eq!(model.brands[7].href, "https://www.summonsdefense.nyc");
        assert_eq!(model.legal_entity, "Shook Law PLLC");
    }

    #[cfg(feature = "server")]
    #[test]
    fn the_footer_uses_the_requested_public_family_labels() {
        assert_eq!(
            compiled_footer_label(views::brand::BrandKey::Neon),
            "Neon Law"
        );
        assert_eq!(
            compiled_footer_label(views::brand::BrandKey::DeleteYourData),
            "DeleteYourData.com"
        );
    }

    /// ENG-589: the resolver reads the live Firm/Entity/brand rows, not a
    /// process-wide constant — a Firm wearing the compiled `neon` key names
    /// its own Entity, in registry order, current first.
    #[cfg(feature = "server")]
    #[tokio::test]
    async fn resolves_the_firm_wearing_the_compiled_key() {
        let surreal = store::surreal::test_support::mem().await;
        let admin = store::test_support::ensure_person(
            &surreal,
            &store::persons::NewPerson::with_role(
                "Firm Admin",
                "firm-footer-admin@example.com",
                store::persons::Role::Admin,
            ),
        )
        .await;
        let entity = store::entities::create(
            &surreal,
            &store::entities::NewEntity {
                name: "Shook Law PLLC".to_string(),
                entity_type_id: store::test_support::SEED_ENTITY_TYPE_ID,
                jurisdiction_id: store::test_support::SEED_ENTITY_JURISDICTION_ID,
                phone: None,
                url: None,
                xero_id: None,
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap();
        let firm = store::firms::create(
            &surreal,
            &store::firms::NewFirm {
                name: "Shook Law".to_string(),
                status: "active".to_string(),
                entity_id: entity.id,
                admin_dri_person_id: admin.id,
            },
        )
        .await
        .unwrap();
        store::firms::attach_brand(&surreal, firm.id, "neon")
            .await
            .unwrap();
        store::firms::attach_brand(&surreal, firm.id, "delete-your-data")
            .await
            .unwrap();

        let model = resolve_firm_footer_model(&surreal, views::brand::BrandKey::Neon, 2026).await;

        assert_eq!(model.legal_entity, "Shook Law PLLC");
        assert_eq!(model.brands.len(), 2);
        assert!(model.brands[0].current);
        assert_eq!(model.brands[0].label, "Neon Law");
    }

    /// A second Firm wearing a runtime-created brand (never one of the compiled
    /// compiled keys) alongside a compiled one: the footer for its compiled
    /// key names *that* Firm's Entity and lists both of *its* brands by their
    /// live names — never the other, seeded Firm's name or brands.
    #[cfg(feature = "server")]
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn a_second_firm_with_a_runtime_brand_is_isolated_from_the_first() {
        let surreal = store::surreal::test_support::mem().await;

        // The first Firm, wearing the other compiled key.
        let admin_a = store::test_support::ensure_person(
            &surreal,
            &store::persons::NewPerson::with_role(
                "Firm A Admin",
                "firm-a-admin@example.com",
                store::persons::Role::Admin,
            ),
        )
        .await;
        let entity_a = store::entities::create(
            &surreal,
            &store::entities::NewEntity {
                name: "Firm A Legal Entity LLC".to_string(),
                entity_type_id: store::test_support::SEED_ENTITY_TYPE_ID,
                jurisdiction_id: store::test_support::SEED_ENTITY_JURISDICTION_ID,
                phone: None,
                url: None,
                xero_id: None,
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap();
        let firm_a = store::firms::create(
            &surreal,
            &store::firms::NewFirm {
                name: "Firm A".to_string(),
                status: "active".to_string(),
                entity_id: entity_a.id,
                admin_dri_person_id: admin_a.id,
            },
        )
        .await
        .unwrap();
        store::firms::attach_brand(&surreal, firm_a.id, "neon")
            .await
            .unwrap();

        // The second Firm, wearing the reachable compiled key plus a
        // runtime-created one with no dedicated host yet.
        let admin_b = store::test_support::ensure_person(
            &surreal,
            &store::persons::NewPerson::with_role(
                "Firm B Admin",
                "firm-b-admin@example.com",
                store::persons::Role::Admin,
            ),
        )
        .await;
        let entity_b = store::entities::create(
            &surreal,
            &store::entities::NewEntity {
                name: "Firm B Legal Entity LLC".to_string(),
                entity_type_id: store::test_support::SEED_ENTITY_TYPE_ID,
                jurisdiction_id: store::test_support::SEED_ENTITY_JURISDICTION_ID,
                phone: None,
                url: None,
                xero_id: None,
                firm_anchor_key: None,
            },
        )
        .await
        .unwrap();
        let firm_b = store::firms::create(
            &surreal,
            &store::firms::NewFirm {
                name: "Firm B".to_string(),
                status: "active".to_string(),
                entity_id: entity_b.id,
                admin_dri_person_id: admin_b.id,
            },
        )
        .await
        .unwrap();
        store::brands::create(
            &surreal,
            store::persons::Role::Owner,
            None,
            &store::brands::NewBrand {
                name: "Acme Runtime Brand".to_string(),
                key: "acme-runtime-brand".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        store::firms::attach_brand(&surreal, firm_b.id, "lawyer-shook")
            .await
            .unwrap();
        store::firms::attach_brand(&surreal, firm_b.id, "acme-runtime-brand")
            .await
            .unwrap();

        let model =
            resolve_firm_footer_model(&surreal, views::brand::BrandKey::LawyerShook, 2026).await;

        assert_eq!(model.legal_entity, "Firm B Legal Entity LLC");
        assert_ne!(model.legal_entity, "Firm A Legal Entity LLC");
        let labels: Vec<&str> = model.brands.iter().map(|b| b.label.as_str()).collect();
        assert!(labels.contains(&"Lawyer Shook"));
        assert!(labels.contains(&"Acme Runtime Brand"));
        assert!(
            !labels.iter().any(|label| label.contains("neon")),
            "Firm A's brand must never appear on Firm B's footer: {labels:?}"
        );
    }

    /// A brand key no Firm wears — the pre-seed database — falls back to the
    /// compiled `Branding`, unchanged from every deployment's footer today.
    #[cfg(feature = "server")]
    #[tokio::test]
    async fn a_brand_key_no_firm_wears_falls_back_to_the_compiled_branding() {
        let surreal = store::surreal::test_support::mem().await;

        let model = resolve_firm_footer_model(&surreal, views::brand::BrandKey::Neon, 2026).await;

        assert_eq!(
            model.legal_entity,
            views::brand::DEFAULT_BRANDING.firm.legal_entity
        );
        // Only the reachable brands: the registry holds more than the
        // footer advertises. See `BrandKey::is_live`.
        assert_eq!(
            model.brands.len(),
            views::brand::BrandKey::ALL
                .iter()
                .filter(|key| key.is_live())
                .count()
        );
        assert!(model.brands[0].current);
    }

    /// `DeleteYourDebt`'s byline is a regulatory boundary, not a style choice.
    ///
    /// "Defend against debt collectors" describes FDCPA and
    /// collection-defence work. Wording that promises to settle, reduce, or
    /// negotiate down a balance describes debt settlement — a separate
    /// regulated activity under the FTC Telemarketing Sales Rule's
    /// advance-fee provisions and state debt-adjuster licensing, whose
    /// attorney exemption is narrower than it is usually assumed to be. A
    /// copy edit here is a scope change, so it fails the build.
    #[test]
    fn no_family_byline_drifts_toward_debt_settlement() {
        for key in views::brand::BrandKey::ALL {
            let byline = key.family_byline().to_lowercase();
            for banned in [
                "settle",
                "settlement",
                "reduce what you owe",
                "negotiate",
                "pennies",
                "write off",
                "forgive",
                "eliminate your debt",
            ] {
                assert!(
                    !byline.contains(banned),
                    "{} byline contains settlement framing {banned:?}: {byline:?}",
                    key.as_str(),
                );
            }
        }
    }

    /// A brand nobody can visit stays out of "Our Family".
    ///
    /// The row is a link, so listing an unreachable host advertises a
    /// practice a reader cannot get to. For the NYC summons practice it is
    /// sharper than a dead link: holding out a New York practice before
    /// admission is a licensing problem, not a cosmetic one. The brand whose
    /// page the reader is *on* is exempt, since it renders as plain text
    /// rather than a link and a site that omitted itself would be stranger
    /// still.
    #[cfg(feature = "server")]
    #[test]
    fn an_unreachable_brand_stays_out_of_the_family_row() {
        let rows = compiled_family_brands(views::brand::BrandKey::Neon);
        let labels: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();

        for key in views::brand::BrandKey::ALL {
            let label = compiled_footer_label(*key);
            if key.is_live() {
                assert!(
                    labels.contains(&label.as_str()),
                    "{label} is live and is listed: {labels:?}"
                );
            } else {
                assert!(
                    !labels.contains(&label.as_str()),
                    "{label} is not reachable and must not be advertised: {labels:?}"
                );
            }
        }
    }

    /// The seeded `brand` row wears the same wordmark the compiled
    /// `Branding` publishes.
    ///
    /// The two footer paths read different sources for one label:
    /// [`compiled_family_brands`] takes `SiteBrand::site_name`, and
    /// [`resolve_firm_footer_model`] takes `brand.name` from the row
    /// `store::seed` wrote. `store` cannot depend on `views`, so that row's
    /// name is a hand-copied constant — and this is the only place in the
    /// workspace that can see both halves and hold them together. Without
    /// it, a deployment with a seeded Firm quietly publishes a different
    /// wordmark than the same page renders before seeding.
    #[cfg(feature = "server")]
    #[test]
    fn every_compiled_brand_seeds_its_published_wordmark() {
        for key in views::brand::BrandKey::ALL {
            let published = key
                .resolve_branding(&views::brand::DEFAULT_BRANDING)
                .firm
                .site_name;
            assert_eq!(
                store::seed::compiled_brand_name(key.as_str()),
                Some(published),
                "{} seeds a brand row named for the wordmark it publishes",
                key.as_str(),
            );
        }
    }

    /// Every compiled brand carries a line, so a "Our Family" row (the
    /// public chrome's, or a future `/app` one) can never render a bare
    /// wordmark for a brand the firm actually ships.
    #[test]
    fn every_compiled_brand_has_a_byline() {
        for key in views::brand::BrandKey::ALL {
            assert!(
                !key.family_byline().is_empty(),
                "{} has no family byline",
                key.as_str(),
            );
        }
    }
}
