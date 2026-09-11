//! One data-driven firm footer, resolved per request from the Firm that
//! wears the current brand (ENG-589) — replacing the process-wide compiled
//! `/app` footer and the `FIRM_BRAND.legal_entity` constant the public
//! chrome's footer used to name.
//!
//! [`FirmFooterModel`] is the one resolved shape both surfaces draw from:
//! [`FirmFooter`] renders it directly for `/app`, and
//! `crate::public_chrome::firm_public_chrome_from_context` maps its
//! `legal_entity`/`brands` onto `crate::components::SiteFooterLegal`'s own
//! props rather than nesting this component — that footer interleaves the
//! copyright line with a trademark notice, per-attorney bar licenses, and the
//! attorney-advertising disclaimer, none of which this minimal model has any
//! business carrying. One resolver, not one piece of markup.
//!
//! A brand key no Firm wears (the pre-seed database, or a compiled key with
//! no `firm_brand` row yet) falls back to the compiled `Branding` the public
//! chrome already falls back to, so a fresh deployment renders the same
//! footer it always has.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

use crate::components::POWERED_BY_NEON_LAW_NAVIGATOR;

/// One brand the resolved Firm wears, for the footer's brands row.
///
/// `href` is empty for a runtime-created brand with no dedicated host to
/// link to yet (a later issue gives one) — rendered as plain text, the same
/// treatment the current brand gets, rather than an inert `<a href="">`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct FirmFooterBrand {
    pub label: String,
    pub href: String,
    pub current: bool,
}

/// The data every Navigator footer needs, resolved once per request from the
/// Firm that wears the current brand — never a process-wide constant.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct FirmFooterModel {
    pub legal_entity: String,
    /// Every brand the Firm wears, current brand first. Empty or a single
    /// entry renders no brands row.
    pub brands: Vec<FirmFooterBrand>,
    pub copyright_year: i32,
    /// The published release this deployment runs. Empty under `cargo run`.
    pub navigator_version: String,
}

/// The `/app` footer: the copyright naming the resolved Firm's legal entity,
/// the brands row (a runtime-created brand listed exactly like a compiled
/// one), and the shared platform line.
#[component]
pub fn FirmFooter(model: FirmFooterModel) -> Element {
    rsx! {
        footer { class: "app-footer",
            p { class: "app-footer__copyright", "© {model.copyright_year} {model.legal_entity}" }
            if model.brands.len() > 1 {
                nav { class: "app-footer__brands", "aria-label": "Brands",
                    ul { class: "app-footer__brands-list",
                        for brand in model.brands.iter() {
                            li { class: "app-footer__brands-item", key: "{brand.label}",
                                if brand.current || brand.href.is_empty() {
                                    span { class: "app-footer__brands-current", "{brand.label}" }
                                } else {
                                    a {
                                        class: "app-footer__brands-link",
                                        href: "{brand.href}",
                                        "{brand.label}"
                                    }
                                }
                            }
                        }
                    }
                }
            }
            p { class: "app-footer__platform",
                "{POWERED_BY_NEON_LAW_NAVIGATOR}"
                if !model.navigator_version.is_empty() {
                    span { class: "app-footer__release", " #{model.navigator_version}" }
                }
            }
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

/// The no-store fallback: the compiled `Branding` for `current`, as its own
/// one-entry brands row. What a fresh deployment (no `firm` rows yet) or a
/// brand key no Firm wears renders — unchanged from the footer every
/// deployment has always shown.
#[cfg(feature = "server")]
#[must_use]
pub fn compiled_firm_footer_model(
    current: views::brand::BrandKey,
    copyright_year: i32,
    navigator_version: String,
) -> FirmFooterModel {
    let branding = current.resolve_branding(&views::brand::DEFAULT_BRANDING);
    FirmFooterModel {
        legal_entity: branding.firm.legal_entity.to_string(),
        brands: vec![FirmFooterBrand {
            label: branding.firm.site_name.to_string(),
            href: String::new(),
            current: true,
        }],
        copyright_year,
        navigator_version,
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
    navigator_version: String,
) -> FirmFooterModel {
    let fallback =
        || compiled_firm_footer_model(current, copyright_year, navigator_version.clone());

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
        brands.push(FirmFooterBrand {
            label: brand.name,
            href: compiled
                .map(|key| key.public_home_href())
                .unwrap_or_default(),
            current: key == current.as_str(),
        });
    }
    if brands.is_empty() {
        return fallback();
    }

    FirmFooterModel {
        legal_entity: entity.name,
        brands,
        copyright_year,
        navigator_version,
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
            navigator_version: String::new(),
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

    #[test]
    fn the_footer_renders_the_shared_platform_line_without_a_version() {
        fn app() -> Element {
            rsx! {
                FirmFooter { model: model(vec![]) }
            }
        }
        let html = ssr(app);
        assert!(
            html.contains(POWERED_BY_NEON_LAW_NAVIGATOR),
            "the shared wording: {html}"
        );
        assert!(
            !html.contains("app-footer__release"),
            "no version under cargo run: {html}"
        );
    }

    #[test]
    fn the_footer_appends_the_release_stamp() {
        fn app() -> Element {
            rsx! {
                FirmFooter {
                    model: FirmFooterModel {
                        navigator_version: "26.8.20".to_string(),
                        ..model(vec![])
                    },
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains("#26.8.20"), "{html}");
    }

    /// A single brand — the ordinary case for most deployments — renders no
    /// brands row, matching the pre-ENG-589 footer that never learned the
    /// list.
    #[test]
    fn a_single_brand_renders_no_brands_row() {
        fn app() -> Element {
            rsx! {
                FirmFooter {
                    model: model(vec![FirmFooterBrand {
                        label: "Neon Law".to_string(),
                        href: "https://www.neonlaw.com".to_string(),
                        current: true,
                    }]),
                }
            }
        }
        let html = ssr(app);
        assert!(!html.contains("app-footer__brands"), "{html}");
    }

    /// A Firm wearing several brands lists every one, in the given order,
    /// with the current brand unlinked and every other one a link.
    #[test]
    fn several_brands_render_in_order_with_the_current_one_unlinked() {
        fn app() -> Element {
            rsx! {
                FirmFooter {
                    model: model(vec![
                        FirmFooterBrand {
                            label: "Neon Law".to_string(),
                            href: "https://www.neonlaw.com".to_string(),
                            current: true,
                        },
                        FirmFooterBrand {
                            label: "DeleteYourData.com".to_string(),
                            href: "https://www.deleteyourdata.com".to_string(),
                            current: false,
                        },
                    ]),
                }
            }
        }
        let html = ssr(app);
        let neon = html.find("Neon Law").expect("neon");
        let dyd = html.find("DeleteYourData.com").expect("dyd");
        assert!(neon < dyd, "registry order: {html}");
        assert!(
            html.contains(r#"class="app-footer__brands-current""#),
            "current brand is not a link: {html}"
        );
        assert!(
            !html.contains(r#"href="https://www.neonlaw.com""#),
            "current brand is unlinked: {html}"
        );
        assert!(
            html.contains(r#"href="https://www.deleteyourdata.com""#),
            "other brands link their home host: {html}"
        );
    }

    /// A runtime-created brand with no dedicated host yet renders as plain
    /// text rather than an inert `<a href="">`.
    #[test]
    fn a_runtime_brand_with_no_href_renders_unlinked_not_as_an_empty_anchor() {
        fn app() -> Element {
            rsx! {
                FirmFooter {
                    model: model(vec![
                        FirmFooterBrand {
                            label: "Neon Law".to_string(),
                            href: "https://www.neonlaw.com".to_string(),
                            current: true,
                        },
                        FirmFooterBrand {
                            label: "Acme Runtime Brand".to_string(),
                            href: String::new(),
                            current: false,
                        },
                    ]),
                }
            }
        }
        let html = ssr(app);
        assert!(html.contains("Acme Runtime Brand"), "{html}");
        assert!(!html.contains(r#"href="""#), "no empty anchor: {html}");
    }

    /// Every class the footer emits is styled by the theme it ships with.
    /// Mirrors `sample_matters_banner`'s own guard: a renamed class with no
    /// matching rule renders as unstyled text and nothing else catches it.
    #[test]
    fn every_class_the_footer_emits_is_styled_by_the_theme() {
        fn app() -> Element {
            rsx! {
                FirmFooter {
                    model: FirmFooterModel {
                        navigator_version: "26.8.20".to_string(),
                        ..model(vec![
                            FirmFooterBrand {
                                label: "Neon Law".to_string(),
                                href: "https://www.neonlaw.com".to_string(),
                                current: true,
                            },
                            FirmFooterBrand {
                                label: "DeleteYourData.com".to_string(),
                                href: "https://www.deleteyourdata.com".to_string(),
                                current: false,
                            },
                        ])
                    },
                }
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

        for class in classes {
            assert!(
                css.contains(&format!(".{class}")),
                "`{class}` is emitted by the footer but has no rule in \
                 server/public/css/theme.css"
            );
        }
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

        let model =
            resolve_firm_footer_model(&surreal, views::brand::BrandKey::Neon, 2026, String::new())
                .await;

        assert_eq!(model.legal_entity, "Shook Law PLLC");
        assert_eq!(model.brands.len(), 2);
        assert!(model.brands[0].current);
        assert_eq!(model.brands[0].label, "neon");
    }

    /// A second Firm wearing a runtime-created brand (never one of the three
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

        let model = resolve_firm_footer_model(
            &surreal,
            views::brand::BrandKey::LawyerShook,
            2026,
            String::new(),
        )
        .await;

        assert_eq!(model.legal_entity, "Firm B Legal Entity LLC");
        assert_ne!(model.legal_entity, "Firm A Legal Entity LLC");
        let labels: Vec<&str> = model.brands.iter().map(|b| b.label.as_str()).collect();
        assert!(labels.contains(&"lawyer-shook"));
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

        let model =
            resolve_firm_footer_model(&surreal, views::brand::BrandKey::Neon, 2026, String::new())
                .await;

        assert_eq!(
            model.legal_entity,
            views::brand::DEFAULT_BRANDING.firm.legal_entity
        );
        assert_eq!(model.brands.len(), 1);
        assert!(model.brands[0].current);
    }
}
