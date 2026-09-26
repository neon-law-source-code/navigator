//! Illustrative fees on the same recovery after identical expenses.

use std::fmt::Write as _;

use dioxus::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Comparison {
    recovery: u32,
    expenses: u32,
    our_fee: u32,
    their_fee: u32,
    our_share: u32,
    their_share: u32,
}

impl Comparison {
    fn new(recovery_dollars: u32, expense_dollars: u32) -> Self {
        let recovery = recovery_dollars.clamp(25_000, 500_000) * 100;
        let expenses = expense_dollars.min(recovery / 100) * 100;
        let base = recovery - expenses;
        let our_fee = base / 100 * 20;
        let their_fee = base / 100 * 33;
        Self {
            recovery,
            expenses,
            our_fee,
            their_fee,
            our_share: base - our_fee,
            their_share: base - their_fee,
        }
    }
}

fn money(cents: u32) -> String {
    let digits = (cents / 100).to_string();
    let mut formatted = String::from("$");
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    if !cents.is_multiple_of(100) {
        let _ = write!(formatted, ".{:02}", cents % 100);
    }
    formatted
}

#[component]
fn RecoveryBar(
    label: &'static str,
    rate: &'static str,
    share: u32,
    fee: u32,
    expenses: u32,
    recovery: u32,
    primary: bool,
) -> Element {
    let retained = money(share);
    let share_width = f64::from(share) / f64::from(recovery) * 100.0;
    let fee_width = f64::from(fee) / f64::from(recovery) * 100.0;
    let cost_width = f64::from(expenses) / f64::from(recovery) * 100.0;
    rsx! {
        div { class: "cyber-bar-label", strong { "{label}" } span { "{rate} fee" } }
        div { class: if primary { "cyber-bar cyber-bar--primary" } else { "cyber-bar" }, "aria-hidden": "true",
            div { class: "cyber-bar__share", style: "width:{share_width}%" }
            div { class: "cyber-bar__fee", style: "width:{fee_width}%" }
            div { class: "cyber-bar__cost", style: "width:{cost_width}%" }
        }
        p { class: "cyber-retained", "{retained} to you" }
    }
}

#[component]
pub(super) fn FeeCalculator(note: String) -> Element {
    let mut recovery = use_signal(|| 100_000_u32);
    let mut expenses = use_signal(|| 5_000_u32);
    let comparison = Comparison::new(recovery(), expenses());
    let gross = money(comparison.recovery);
    let difference = money(comparison.our_share - comparison.their_share);
    rsx! {
        div { class: "cyber-calculator",
            p { class: "cyber-eyebrow", "SEE THE DIFFERENCE" }
            label { class: "cyber-amount", r#for: "cyber-recovery", "Illustrative recovery" output { "for": "cyber-recovery", "{gross}" } }
            input { class: "nav-input", id: "cyber-recovery", r#type: "range", min: "25000", max: "500000", step: "5000", value: "{recovery}",
                oninput: move |event| { if let Ok(value) = event.value().parse::<u32>() { let value = value.clamp(25_000, 500_000); let capped_expenses = expenses().min(value); recovery.set(value); expenses.set(capped_expenses); } },
            }
            div { class: "cyber-range", span { "$25,000" } span { "$500,000" } }
            label { class: "cyber-expenses", r#for: "cyber-expenses", "Case expenses ($)"
                input { class: "nav-input", id: "cyber-expenses", r#type: "number", inputmode: "numeric", min: "0", max: "{recovery}", step: "1", value: "{expenses}",
                    oninput: move |event| { expenses.set(event.value().parse::<u32>().unwrap_or_default().min(recovery())); },
                }
            }
            div { "aria-label": "Recovery allocation comparison", role: "group",
                RecoveryBar { label: "CYBERINJURYLAW", rate: "20%", primary: true, share: comparison.our_share, fee: comparison.our_fee, expenses: comparison.expenses, recovery: comparison.recovery }
                RecoveryBar { label: "33% COMPARISON", rate: "33%", primary: false, share: comparison.their_share, fee: comparison.their_fee, expenses: comparison.expenses, recovery: comparison.recovery }
            }
            div { class: "cyber-legend", span { class: "cyber-key-share", "Your share" } span { class: "cyber-key-fee", "Attorney fee" } span { class: "cyber-key-cost", "Expenses" } }
            div { class: "cyber-difference", "aria-live": "polite", span { "MORE IN YOUR POCKET" } strong { "+{difference}" } }
            p { class: "cyber-fine", "{note}" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparison_deducts_equal_expenses_before_applying_fees() {
        let comparison = Comparison::new(100_000, 5_000);
        assert_eq!(comparison.our_share, 7_600_000);
        assert_eq!(comparison.their_share, 6_365_000);
        assert_eq!(comparison.our_share - comparison.their_share, 1_235_000);
        for gross in [25_000, 100_000, 500_000] {
            for costs in [0, 5_000, gross, u32::MAX] {
                let c = Comparison::new(gross, costs);
                assert_eq!(c.our_share + c.our_fee + c.expenses, c.recovery);
                assert_eq!(c.their_share + c.their_fee + c.expenses, c.recovery);
                assert!(c.our_share >= c.their_share);
            }
        }
    }

    #[test]
    fn comparison_preserves_cents_and_bounds_extreme_input() {
        let c = Comparison::new(100_000, 5_001);
        assert_eq!(money(c.our_share), "$75,999.20");
        assert_eq!(money(c.their_share), "$63,649.33");
        assert_eq!(Comparison::new(0, 0).recovery, 2_500_000);
        assert_eq!(Comparison::new(u32::MAX, u32::MAX).our_share, 0);
    }
}
