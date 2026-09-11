//! The Firm show page's trailing-30-day invoice graphs (ENG-591): two
//! horizontal grouped bar charts — invoiced cents and paid cents, by brand
//! and by lawyer DRI — drawn as inline server-rendered SVG. No charting
//! library, following the precedent already set by the status pie in
//! `webapp::lawyer_dashboard`.
//!
//! **Privacy.** Every label this module renders is a brand's display name, a
//! lawyer DRI's display name, or the literal `"Unassigned"` —
//! [`store::xero_invoices::firm_thirty_day_rollup`] is the one place that
//! builds those labels, and it never carries a Project code, matter name,
//! client name, or Xero invoice id into them. This module has no other
//! field to leak one through.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// One label's invoiced and paid cents — one bar-pair.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct InvoiceBarView {
    pub label: String,
    pub invoiced_cents: i64,
    pub paid_cents: i64,
}

/// One currency's two grouped bar charts.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct CurrencyInvoiceGraphsView {
    pub currency: String,
    pub by_brand: Vec<InvoiceBarView>,
    pub by_lawyer_dri: Vec<InvoiceBarView>,
}

/// The whole graphs section: one [`CurrencyInvoiceGraphsView`] per currency
/// that appeared in the trailing 30 days. Empty renders the empty state.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
pub struct FirmInvoiceGraphsView {
    pub currencies: Vec<CurrencyInvoiceGraphsView>,
}

#[cfg(feature = "server")]
impl From<store::xero_invoices::FirmInvoiceRollup> for FirmInvoiceGraphsView {
    fn from(rollup: store::xero_invoices::FirmInvoiceRollup) -> Self {
        fn bars(groups: Vec<store::xero_invoices::InvoiceGroupTotal>) -> Vec<InvoiceBarView> {
            groups
                .into_iter()
                .map(|group| InvoiceBarView {
                    label: group.label,
                    invoiced_cents: group.invoiced_cents,
                    paid_cents: group.paid_cents,
                })
                .collect()
        }
        Self {
            currencies: rollup
                .currencies
                .into_iter()
                .map(|currency| CurrencyInvoiceGraphsView {
                    currency: currency.currency,
                    by_brand: bars(currency.by_brand),
                    by_lawyer_dri: bars(currency.by_lawyer_dri),
                })
                .collect(),
        }
    }
}

/// The full width of a bar's track, in SVG user units.
const BAR_MAX_WIDTH: i64 = 280;
const BAR_HEIGHT: i64 = 14;
const BAR_GAP: i64 = 4;
const GROUP_GAP: i64 = 10;
const LABEL_WIDTH: i64 = 140;
const CHART_LEFT: i64 = LABEL_WIDTH + 10;

/// Whole-dollar formatting: cents are truncated to the dollar, matching the
/// axis's own whole-dollar unit rather than showing a stray remainder.
fn format_dollars(cents: i64) -> String {
    format!("${}", cents / 100)
}

/// A bar's pixel width out of [`BAR_MAX_WIDTH`], computed in integer maths —
/// the same convention `lawyer_dashboard::pie_share_css` uses for its pie
/// shares, kept here rather than reaching for float scaling.
fn bar_width(cents: i64, max_cents: i64) -> i64 {
    if max_cents <= 0 {
        return 0;
    }
    cents
        .saturating_mul(BAR_MAX_WIDTH)
        .checked_div(max_cents)
        .unwrap_or(0)
}

/// One grouped bar chart: every bar's label at the left, an invoiced bar
/// over a paid bar, both scaled to the same `max_cents` so every group in
/// the chart is comparable.
#[component]
fn BarChart(bars: Vec<InvoiceBarView>, heading: String) -> Element {
    if bars.is_empty() {
        return rsx! {};
    }
    let max_cents = bars
        .iter()
        .map(|bar| bar.invoiced_cents.max(bar.paid_cents))
        .max()
        .unwrap_or(0);
    let row_height = BAR_HEIGHT * 2 + BAR_GAP + GROUP_GAP;
    let chart_height = row_height * i64::try_from(bars.len()).unwrap_or(0);
    let chart_width = CHART_LEFT + BAR_MAX_WIDTH + 70;
    let view_box = format!("0 0 {chart_width} {chart_height}");

    rsx! {
        div { class: "firm-invoice-chart",
            h3 { "{heading}" }
            svg {
                view_box: "{view_box}",
                width: "{chart_width}",
                height: "{chart_height}",
                role: "img",
                for (index , bar) in bars.iter().enumerate() {
                    {
                        let y_group = i64::try_from(index).unwrap_or(0) * row_height;
                        let invoiced_width = bar_width(bar.invoiced_cents, max_cents);
                        let paid_width = bar_width(bar.paid_cents, max_cents);
                        let y_invoiced = y_group;
                        let y_paid = y_group + BAR_HEIGHT + BAR_GAP;
                        let label_y = y_group + BAR_HEIGHT;
                        rsx! {
                            g { class: "firm-invoice-bar-group",
                                text {
                                    x: "0",
                                    y: "{label_y}",
                                    class: "firm-invoice-bar-label",
                                    "{bar.label}"
                                }
                                rect {
                                    x: "{CHART_LEFT}",
                                    y: "{y_invoiced}",
                                    width: "{invoiced_width}",
                                    height: "{BAR_HEIGHT}",
                                    class: "firm-invoice-bar firm-invoice-bar--invoiced",
                                    title { "{bar.label} invoiced: {format_dollars(bar.invoiced_cents)}" }
                                }
                                rect {
                                    x: "{CHART_LEFT}",
                                    y: "{y_paid}",
                                    width: "{paid_width}",
                                    height: "{BAR_HEIGHT}",
                                    class: "firm-invoice-bar firm-invoice-bar--paid",
                                    title { "{bar.label} paid: {format_dollars(bar.paid_cents)}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One currency's section: the "By brand" and "By lawyer DRI" charts.
#[component]
fn CurrencySection(view: CurrencyInvoiceGraphsView, show_currency: bool) -> Element {
    rsx! {
        section { class: "firm-invoice-currency",
            if show_currency {
                h3 { class: "firm-invoice-currency__code", "{view.currency}" }
            }
            BarChart { bars: view.by_brand, heading: "By brand".to_string() }
            BarChart { bars: view.by_lawyer_dri, heading: "By lawyer DRI".to_string() }
        }
    }
}

/// The Firm show page's invoice graphs section (ENG-591): trailing-30-day
/// invoiced and paid cents, grouped by brand and by lawyer DRI. Renders its
/// own empty state when no invoice on any of the Firm's Projects fell in
/// the window, rather than a chart with zero-height bars.
#[component]
pub fn FirmInvoiceGraphs(view: FirmInvoiceGraphsView) -> Element {
    rsx! {
        section { id: "firm-invoice-graphs",
            h2 { "Invoices (trailing 30 days)" }
            if view.currencies.is_empty() {
                p { class: "nav-muted", "No invoices in the trailing 30 days." }
            } else {
                for currency in view.currencies.iter().cloned() {
                    CurrencySection { view: currency, show_currency: view.currencies.len() > 1 }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CurrencyInvoiceGraphsView, FirmInvoiceGraphs, FirmInvoiceGraphsProps,
        FirmInvoiceGraphsView, InvoiceBarView,
    };
    use dioxus::prelude::VirtualDom;

    fn render(view: FirmInvoiceGraphsView) -> String {
        let mut dom =
            VirtualDom::new_with_props(FirmInvoiceGraphs, FirmInvoiceGraphsProps { view });
        dom.rebuild_in_place();
        dioxus_ssr::render(&dom)
    }

    #[test]
    fn renders_an_empty_state_with_no_currencies() {
        let html = render(FirmInvoiceGraphsView::default());
        assert!(
            html.contains("No invoices in the trailing 30 days."),
            "{html}"
        );
        assert!(!html.contains("<svg"), "{html}");
    }

    #[test]
    fn renders_a_bar_pair_per_group_with_a_title_on_every_bar() {
        let html = render(FirmInvoiceGraphsView {
            currencies: vec![CurrencyInvoiceGraphsView {
                currency: "USD".to_string(),
                by_brand: vec![
                    InvoiceBarView {
                        label: "Neon Law".to_string(),
                        invoiced_cents: 10_000,
                        paid_cents: 5_000,
                    },
                    InvoiceBarView {
                        label: "DeleteYourData.com".to_string(),
                        invoiced_cents: 2_000,
                        paid_cents: 0,
                    },
                ],
                by_lawyer_dri: vec![InvoiceBarView {
                    label: "Unassigned".to_string(),
                    invoiced_cents: 12_000,
                    paid_cents: 5_000,
                }],
            }],
        });
        assert!(html.contains("By brand"), "{html}");
        assert!(html.contains("By lawyer DRI"), "{html}");
        assert!(html.contains("Neon Law invoiced: $100"), "{html}");
        assert!(html.contains("Neon Law paid: $50"), "{html}");
        assert!(html.contains("DeleteYourData.com invoiced: $20"), "{html}");
        assert!(html.contains("Unassigned invoiced: $120"), "{html}");
        // A single currency stays unlabelled — nothing to disambiguate.
        assert!(!html.contains("firm-invoice-currency__code"), "{html}");
    }

    #[test]
    fn a_second_currency_gets_its_own_labelled_section() {
        let html = render(FirmInvoiceGraphsView {
            currencies: vec![
                CurrencyInvoiceGraphsView {
                    currency: "USD".to_string(),
                    by_brand: vec![InvoiceBarView {
                        label: "Neon Law".to_string(),
                        invoiced_cents: 1_000,
                        paid_cents: 0,
                    }],
                    by_lawyer_dri: vec![],
                },
                CurrencyInvoiceGraphsView {
                    currency: "EUR".to_string(),
                    by_brand: vec![InvoiceBarView {
                        label: "Neon Law".to_string(),
                        invoiced_cents: 2_000,
                        paid_cents: 0,
                    }],
                    by_lawyer_dri: vec![],
                },
            ],
        });
        assert!(html.contains(">USD<"), "{html}");
        assert!(html.contains(">EUR<"), "{html}");
    }

    /// ENG-591 privacy: this module has no field to carry a Project code,
    /// matter name, or Xero invoice id through, and this renders every kind
    /// of label the store can hand it (a brand name, a lawyer DRI name, and
    /// "Unassigned") to prove none of that leaks in by accident.
    #[test]
    fn no_project_code_matter_name_or_xero_id_appears_in_the_rendered_html() {
        let html = render(FirmInvoiceGraphsView {
            currencies: vec![CurrencyInvoiceGraphsView {
                currency: "USD".to_string(),
                by_brand: vec![InvoiceBarView {
                    label: "Neon Law".to_string(),
                    invoiced_cents: 1_000,
                    paid_cents: 0,
                }],
                by_lawyer_dri: vec![InvoiceBarView {
                    label: "Unassigned".to_string(),
                    invoiced_cents: 1_000,
                    paid_cents: 0,
                }],
            }],
        });
        for forbidden in ["xero", "matter", "project_id", "INV-"] {
            assert!(
                !html.to_lowercase().contains(&forbidden.to_lowercase()),
                "{forbidden} leaked into: {html}"
            );
        }
    }
}
