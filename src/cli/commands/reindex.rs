//! `daemoneye reindex` — rebuild the derived memory index from disk.

use crate::memory::index::{ReconcileReport, reconcile_index};
use std::io::{self, Write};
use std::process;

/// Render the operator-facing report. Pure, so the wording is unit-testable.
fn format_report(report: &ReconcileReport) -> String {
    match report.rows_after.cmp(&report.rows_before) {
        std::cmp::Ordering::Greater => {
            let added = report.rows_after - report.rows_before;
            format!(
                "Index rebuilt: {} → {} rows ({} added).",
                report.rows_before, report.rows_after, added
            )
        }
        std::cmp::Ordering::Less => {
            let removed = report.rows_before - report.rows_after;
            format!(
                "Index rebuilt: {} → {} rows ({} removed).",
                report.rows_before, report.rows_after, removed
            )
        }
        std::cmp::Ordering::Equal => {
            format!(
                "Index rebuilt: {} rows (count unchanged — the rebuild still replaced every row).",
                report.rows_after
            )
        }
    }
}

pub fn run_reindex() {
    match reconcile_index() {
        Ok(report) => {
            println!("{}", format_report(&report));
            let _ = io::stdout().flush();
        }
        Err(e) => {
            eprintln!("\x1b[31mReindex failed: {e:#}\x1b[0m");
            let _ = io::stderr().flush();
            process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_wording_when_rows_grow() {
        let report = ReconcileReport {
            rows_before: 3,
            rows_after: 9,
        };
        let out = format_report(&report);
        assert!(out.contains("3 → 9"), "output: {out}");
        assert!(out.contains("6 added"), "output: {out}");
    }

    #[test]
    fn report_wording_when_rows_shrink() {
        let report = ReconcileReport {
            rows_before: 9,
            rows_after: 3,
        };
        let out = format_report(&report);
        assert!(out.contains("9 → 3"), "output: {out}");
        assert!(out.contains("6 removed"), "output: {out}");
    }

    #[test]
    fn report_wording_when_count_is_unchanged_does_not_claim_no_op() {
        let report = ReconcileReport {
            rows_before: 9,
            rows_after: 9,
        };
        let out = format_report(&report);
        assert!(out.contains("9 rows"), "output: {out}");
        assert!(
            !out.contains("up to date"),
            "must not claim up to date: {out}"
        );
        assert!(
            !out.contains("no changes"),
            "must not claim no changes: {out}"
        );
    }
}
