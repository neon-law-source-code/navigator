//! `navigator ops firms doctor` — the deployment-wide Admin-DRI invariant
//! report (ENG-499).
//!
//! Read-only: it connects to the `SurrealDB` the sourced `.devx/env` names —
//! the same store `web` reads — and prints which active Firms do not hold
//! exactly one eligible Admin DRI. It never appoints, clears, or otherwise
//! repairs a row; [`store::firms::appoint_admin_dri`] is the only writer, and
//! fixing a reported Firm means calling it (through the Owner surface) after
//! a human decides who the DRI should be.

use std::process::ExitCode;

use anyhow::{Context, Result};

use crate::palette;

/// `navigator ops firms doctor`.
pub fn run() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("could not start a runtime: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(diagnose()) {
        Ok(report) => print_report(&report),
        Err(e) => {
            eprintln!("could not read the Admin-DRI report: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn diagnose() -> Result<Vec<store::firms::AdminDriStatus>> {
    let surreal = store::surreal::connect_from_env()
        .await
        .context("connect to SurrealDB")?;
    store::firms::admin_dri_invariant_report(&surreal)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

fn print_report(report: &[store::firms::AdminDriStatus]) -> ExitCode {
    use store::firms::AdminDriProblem;

    if report.is_empty() {
        println!("{}", palette::dim("no active Firms"));
        return ExitCode::SUCCESS;
    }

    let mut healthy = true;
    for status in report {
        let (marker, detail) = match &status.problem {
            None => (palette::dim("ok  "), "one eligible Admin DRI".to_string()),
            Some(AdminDriProblem::Missing) => {
                healthy = false;
                (palette::highlight("FAIL"), "no Admin DRI".to_string())
            }
            Some(AdminDriProblem::Multiple(ids)) => {
                healthy = false;
                (
                    palette::highlight("FAIL"),
                    format!("{} people hold the designation: {ids:?}", ids.len()),
                )
            }
            Some(AdminDriProblem::Ineligible(person_id)) => {
                healthy = false;
                (
                    palette::highlight("FAIL"),
                    format!("{person_id} no longer carries an eligible admin membership"),
                )
            }
        };
        println!("{marker}  {:<24}  {detail}", status.firm_name);
    }

    println!();
    if healthy {
        println!(
            "{}",
            palette::dim("every active Firm holds exactly one eligible Admin DRI")
        );
        ExitCode::SUCCESS
    } else {
        println!(
            "{}",
            palette::highlight(
                "one or more Firms need a human to appoint or transfer their Admin DRI \
                 — this report never repairs one"
            )
        );
        ExitCode::FAILURE
    }
}
