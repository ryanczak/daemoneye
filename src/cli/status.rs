use anyhow::Result;
use tokio::io::BufReader;

use crate::cli::commands::{connect, liveness_line, recv, send_request};
use crate::cli::palette::{self, ColorDepth};
use crate::cli::render::terminal_width;
use crate::config::default_pid_path;
use crate::daemon::daemon_liveness;
use crate::daemon::instance::read_pid;
use crate::ipc::{Request, Response};

// ── Color palette ─────────────────────────────────────────────────────────────

fn depth() -> ColorDepth {
    static DEPTH: std::sync::OnceLock<ColorDepth> = std::sync::OnceLock::new();
    *DEPTH.get_or_init(palette::detect_from_env)
}

fn c_accent(s: &str) -> String {
    format!(
        "\x1b[1m{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (100, 210, 255), 117, 96)
    )
}
fn c_key(s: &str) -> String {
    format!(
        "{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (140, 140, 165), 103, 90)
    )
}
fn c_val(s: &str) -> String {
    format!(
        "\x1b[1m{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (220, 220, 240), 254, 97)
    )
}
fn c_ok(s: &str) -> String {
    format!(
        "{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (80, 210, 130), 78, 92)
    )
}
fn c_err(s: &str) -> String {
    format!(
        "{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (230, 80, 80), 167, 91)
    )
}
fn c_warn(s: &str) -> String {
    format!(
        "{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (250, 190, 50), 214, 93)
    )
}
fn c_num(s: &str) -> String {
    format!(
        "{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (130, 195, 255), 111, 94)
    )
}
fn c_dim(s: &str) -> String {
    format!(
        "{}{s}\x1b[0m",
        palette::sgr_fg(depth(), (80, 80, 105), 60, 90)
    )
}

fn dsep() -> String {
    c_dim("  ·  ")
}

/// The COST TODAY `cap` line: today's spend against the cap, with the
/// percentage used. Empty at `cap <= 0` — no cap, no line. Pure so it can
/// be tested.
fn cap_line(spent: f64, cap: f64) -> String {
    if cap <= 0.0 {
        return String::new();
    }
    let pct = (spent / cap * 100.0) as usize;
    format!(
        "{}{}{}{}",
        c_warn(&format!("${:.4}", spent)),
        c_dim(" of "),
        c_val(&format!("${:.2}", cap)),
        c_dim(&format!(" ({pct}%)"))
    )
}

// ── Section builder ───────────────────────────────────────────────────────────

const KEY_W: usize = 14;

struct Section {
    title: &'static str,
    rows: Vec<Row>,
}

enum Row {
    KV(String, String),
    Cont(String),
}

impl Section {
    fn new(title: &'static str) -> Self {
        Self {
            title,
            rows: Vec::new(),
        }
    }

    fn kv(&mut self, key: &str, val: impl Into<String>) {
        self.rows.push(Row::KV(key.to_string(), val.into()));
    }

    fn cont(&mut self, text: impl Into<String>) {
        self.rows.push(Row::Cont(text.into()));
    }

    fn render(&self, term_width: usize) {
        let dash_count = term_width
            .saturating_sub(self.title.chars().count() + 4)
            .max(2);
        println!(
            "{} {} {}",
            c_dim("──"),
            c_accent(self.title),
            c_dim(&"─".repeat(dash_count))
        );
        for row in &self.rows {
            match row {
                Row::KV(k, v) => {
                    println!("  {}  {v}", c_key(&format!("{k:<KEY_W$}")));
                }
                Row::Cont(text) => {
                    println!("  {:<KEY_W$}  {text}", "");
                }
            }
        }
        println!();
    }
}

// ── Formatting helpers ────────────────────────────────────────────────────────

/// The SANDBOX section: runtime health plus the daemon's own sandboxes, from
/// a `Request::ContainerStatus` report. Shared by `status` and
/// `sandbox status`.
fn sandbox_section(report: &crate::ipc::ContainerStatusReport) -> Section {
    let mut sbx = Section::new("SANDBOX");
    sbx.kv(
        "enabled",
        if report.enabled {
            c_ok("yes")
        } else {
            c_dim("no")
        },
    );
    sbx.kv(
        "runtime",
        if report.runtime_ok {
            c_ok(&report.runtime_detail)
        } else {
            c_err(&report.runtime_detail)
        },
    );
    sbx.kv("image", c_val(&report.image_detail));
    sbx.kv("containers", c_num(&report.containers.len().to_string()));
    for ci in &report.containers {
        sbx.cont(format!(
            "{} {} {} {}",
            c_key(&ci.id),
            c_val(&ci.state),
            if ci.is_ghost {
                c_warn("ghost")
            } else {
                c_dim("session")
            },
            c_dim(ci.session.as_deref().unwrap_or("-")),
        ));
    }
    sbx
}

pub(crate) fn fmt_uptime(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{}h {}m {}s", h, m, s)
    } else if m > 0 {
        format!("{}m {}s", m, s)
    } else {
        format!("{}s", s)
    }
}

fn fmt_unlimited_u32(v: u32) -> String {
    if v == 0 {
        c_dim("unlimited")
    } else {
        c_num(&v.to_string())
    }
}

fn fmt_unlimited_usize(v: usize) -> String {
    if v == 0 {
        c_dim("unlimited")
    } else {
        c_num(&v.to_string())
    }
}

fn fmt_ok_fail(ok: usize, fail: usize) -> String {
    let ok_s = if ok > 0 {
        c_ok(&format!("{ok} ok"))
    } else {
        c_dim("0 ok")
    };
    let fail_s = if fail > 0 {
        c_err(&format!("{fail} failed"))
    } else {
        c_dim("0 failed")
    };
    format!("{ok_s}{}{fail_s}", dsep())
}

fn fmt_appr_denied(a: usize, d: usize) -> String {
    let ap = if a > 0 {
        c_ok(&format!("{a} approved"))
    } else {
        c_dim("0 approved")
    };
    let dn = if d > 0 {
        c_err(&format!("{d} denied"))
    } else {
        c_dim("0 denied")
    };
    format!("{ap}  {dn}")
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[path = "status_sections.rs"]
mod sections;
use sections::*;

pub async fn run_status() -> Result<()> {
    match connect().await {
        Err(_) => {
            let liveness = daemon_liveness().await;
            let pid = read_pid(&default_pid_path());
            eprintln!("{}", c_err(&liveness_line(liveness, pid)));
            std::process::exit(1);
        }
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(&mut tx, Request::Status).await?;
            match recv(&mut rx).await {
                Ok(resp) => {
                    if !render_daemon_status(resp).await {
                        eprintln!("{}", c_err("Daemon did not return status."));
                        std::process::exit(1);
                    }
                }
                Err(_) => {
                    eprintln!("{}", c_err("Daemon did not return status."));
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}

/// `daemoneye sandbox status` — the daemon's SANDBOX section on its own.
pub async fn run_sandbox_status() -> Result<()> {
    match connect().await {
        Err(_) => {
            let liveness = daemon_liveness().await;
            let pid = read_pid(&default_pid_path());
            eprintln!("{}", c_err(&liveness_line(liveness, pid)));
            std::process::exit(1);
        }
        Ok(stream) => {
            let (rx, mut tx) = stream.into_split();
            let mut rx = BufReader::new(rx);
            send_request(&mut tx, Request::ContainerStatus).await?;
            match recv(&mut rx).await {
                Ok(Response::ContainerStatus { report }) => {
                    sandbox_section(&report).render(terminal_width());
                }
                _ => {
                    eprintln!("{}", c_err("Daemon did not return sandbox status."));
                    std::process::exit(1);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_line_names_spend_and_cap() {
        let line = cap_line(0.5, 2.0);
        assert!(line.contains("$0.5000"), "the spend appears: {line}");
        assert!(line.contains("$2.00"), "the cap appears: {line}");
        assert!(line.contains("25%"), "the percentage used appears: {line}");
        assert!(
            !cap_line(0.0, 0.0).contains("$"),
            "no line is rendered at cap 0 — the caller checks cap > 0"
        );
    }

    fn status_report(
        containers: Vec<crate::ipc::ContainerInfo>,
    ) -> crate::ipc::ContainerStatusReport {
        crate::ipc::ContainerStatusReport {
            enabled: true,
            runtime_ok: true,
            runtime_detail: "sbx v0.46.0".to_string(),
            image_detail: "not used by sbx".to_string(),
            containers,
        }
    }

    fn container(id: &str, state: &str, is_ghost: bool) -> crate::ipc::ContainerInfo {
        crate::ipc::ContainerInfo {
            id: id.to_string(),
            state: state.to_string(),
            image: "shell".to_string(),
            session: Some("s".to_string()),
            is_ghost,
        }
    }

    fn rows(section: &Section) -> &Vec<Row> {
        &section.rows
    }

    #[test]
    fn sandbox_section_lists_each_sandbox() {
        let report = status_report(vec![
            container("de-ghost-r1", "running", true),
            container("de-chat-s7", "stopped", false),
        ]);

        let section = sandbox_section(&report);

        let kv_values = |key: &str| -> Vec<String> {
            rows(&section)
                .iter()
                .filter_map(|r| match r {
                    Row::KV(k, v) if k == key => Some(v.clone()),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(kv_values("enabled").len(), 1, "one enabled row");
        assert_eq!(kv_values("runtime").len(), 1, "one runtime row");
        assert!(
            kv_values("runtime")[0].contains("sbx v0.46.0"),
            "runtime value carries the version"
        );
        assert_eq!(kv_values("image").len(), 1, "one image row");
        assert!(
            kv_values("containers")[0].contains('2'),
            "`containers` value counts the rows"
        );
        let conts: Vec<&String> = rows(&section)
            .iter()
            .filter_map(|r| match r {
                Row::Cont(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(conts.len(), 2, "one continuation row per container");
        assert!(
            conts
                .iter()
                .all(|t| t.contains("de-ghost-r1") || t.contains("de-chat-s7")),
            "each row names its container id"
        );
    }

    #[test]
    fn sandbox_section_marks_an_unusable_runtime() {
        let report = crate::ipc::ContainerStatusReport {
            enabled: true,
            runtime_ok: false,
            runtime_detail: "sandbox unavailable: x".to_string(),
            image_detail: "not used by sbx".to_string(),
            containers: Vec::new(),
        };

        let section = sandbox_section(&report);

        let kv_values = |key: &str| -> Vec<String> {
            rows(&section)
                .iter()
                .filter_map(|r| match r {
                    Row::KV(k, v) if k == key => Some(v.clone()),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(kv_values("runtime").len(), 1, "one runtime row");
        assert!(
            kv_values("runtime")[0].contains("sandbox unavailable: x"),
            "the runtime row must carry the operator-facing reason"
        );
        let conts = rows(&section)
            .iter()
            .filter(|r| matches!(r, Row::Cont(_)))
            .count();
        assert_eq!(conts, 0, "no containers when sbx is unusable");
    }
}
