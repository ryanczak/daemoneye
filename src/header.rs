//! Inline header parser and renderer for DaemonEye-managed files.
//!
//! **Scripts** carry a comment-block header delimited by sentinel lines:
//!
//! ```text
//! #!/usr/bin/env bash
//! # --- daemoneye ---
//! # tags: [disk, cleanup]
//! # summary: Rotate logs when /var crosses threshold
//! # relates_to: [high-disk-usage]
//! # run_with_sudo: true
//! # --- /daemoneye ---
//! set -euo pipefail
//! ```
//!
//! **Runbooks** and **memories** use YAML `---` frontmatter.  Both forms parse
//! into the same [`Header`] struct so callers work with a single type.
//!
//! The comment prefix is detected from the sentinel line itself; `#`, `//`,
//! `--`, and `;` are supported.  Writers should use `#` for shell/Python and
//! `//` for JS/Rust/Go; callers pass the prefix explicitly to
//! [`render_comment_header`].

use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Public type
// ---------------------------------------------------------------------------

/// Structured metadata extracted from any DaemonEye artifact header.
///
/// Fields map 1-to-1 with the YAML frontmatter keys used by memories and
/// runbooks, and with the `# key: value` lines used by script headers.
/// Artifact-specific fields that are not in this common set (e.g.
/// `run_with_sudo: true` for scripts) are collected into [`extras`].
///
/// [`extras`]: Header::extras
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Header {
    pub tags: Vec<String>,
    pub summary: Option<String>,
    pub relates_to: Vec<String>,
    pub created: Option<String>,
    pub updated: Option<String>,
    pub expires: Option<String>,
    /// Artifact-specific key/value pairs not covered by the fields above,
    /// stored in sorted order.
    pub extras: BTreeMap<String, String>,
}

impl Header {
    /// Returns `true` when every field is empty or `None`.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
            && self.summary.is_none()
            && self.relates_to.is_empty()
            && self.created.is_none()
            && self.updated.is_none()
            && self.expires.is_none()
            && self.extras.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const OPEN_TAG: &str = " --- daemoneye ---";
const CLOSE_TAG: &str = " --- /daemoneye ---";
/// Recognised comment-prefix strings, tried in order.
const PREFIXES: &[&str] = &["#", "//", "--", ";"];

// ---------------------------------------------------------------------------
// Line iterator with byte-position tracking
// ---------------------------------------------------------------------------

struct LineIter<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> LineIter<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }
}

/// Yields `(line_without_newline, byte_start, byte_after_terminator)`.
impl<'a> Iterator for LineIter<'a> {
    type Item = (&'a str, usize, usize);

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.src.len() {
            return None;
        }
        let start = self.pos;
        let rest = &self.src[start..];
        let (content, step) = match rest.find('\n') {
            Some(i) => (&rest[..i], i + 1),
            None => (rest, rest.len()),
        };
        self.pos += step;
        Some((content, start, self.pos))
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// If `line` (already trimmed by the caller) is an opening sentinel, return
/// the matching prefix string.
fn match_open(trimmed_line: &str) -> Option<&'static str> {
    for &pfx in PREFIXES {
        if trimmed_line == &*format!("{}{}", pfx, OPEN_TAG) {
            return Some(pfx);
        }
    }
    None
}

/// Parse `key: value` lines into a [`Header`].
///
/// `lines` should already have the comment prefix stripped.
fn parse_kv_fields(lines: &[String]) -> Header {
    let mut h = Header::default();
    for line in lines {
        let Some((raw_key, raw_val)) = line.split_once(':') else {
            continue;
        };
        let key = raw_key.trim();
        let val = raw_val.trim();
        match key {
            "tags" => h.tags = parse_list(val),
            "summary" => h.summary = Some(unquote(val).to_string()),
            "relates_to" => h.relates_to = parse_list(val),
            "created" => h.created = Some(unquote(val).to_string()),
            "updated" => h.updated = Some(unquote(val).to_string()),
            "expires" => h.expires = Some(unquote(val).to_string()),
            other if !other.is_empty() => {
                h.extras.insert(other.to_string(), val.to_string());
            }
            _ => {}
        }
    }
    h
}

/// Parse an inline YAML list `[item1, "item2"]` or a bare scalar as a
/// single-element list.
fn parse_list(s: &str) -> Vec<String> {
    if let Some(inner) = s.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        inner
            .split(',')
            .map(|item| unquote(item.trim()).to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else if !s.is_empty() {
        vec![unquote(s).to_string()]
    } else {
        Vec::new()
    }
}

/// Strip a single layer of matching `"…"` or `'…'` quotes.
fn unquote(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Parse a comment-block header from script (or other comment-based) source.
///
/// Looks for `{prefix} --- daemoneye ---` … `{prefix} --- /daemoneye ---`
/// after an optional shebang (`#!`) on the first line.
///
/// Returns `(header, body_start)` where `body_start` is the byte offset of
/// the first character after the closing sentinel line.  When no valid
/// header is found (absent, unknown prefix, or unclosed), `body_start == 0`
/// and the returned header is [`Header::default()`].
pub fn parse_comment_header(src: &str) -> (Header, usize) {
    let mut found_prefix: Option<&'static str> = None;
    let mut kv: Vec<String> = Vec::new();
    let mut first = true;

    for (line, _start, after) in LineIter::new(src) {
        // Skip shebang on the very first line of the file.
        if first {
            first = false;
            if line.trim_start().starts_with("#!") {
                continue;
            }
        }

        if found_prefix.is_none() {
            if let Some(pfx) = match_open(line.trim()) {
                found_prefix = Some(pfx);
            }
            continue;
        }

        // Inside the block — look for the closing sentinel.
        let pfx = found_prefix.unwrap();
        let close_sentinel = format!("{}{}", pfx, CLOSE_TAG);
        if line.trim() == close_sentinel {
            return (parse_kv_fields(&kv), after);
        }

        // Strip the comment prefix (and one optional space) from the line.
        let content = line.trim();
        let stripped = content
            .strip_prefix(pfx)
            .map(str::trim_start)
            .unwrap_or(content);
        if !stripped.is_empty() {
            kv.push(stripped.to_string());
        }
    }

    // Unclosed block or no block found.
    (Header::default(), 0)
}

/// Render a [`Header`] as a comment-block header string using `prefix`.
///
/// Returns an empty string when the header is empty.  The rendered string
/// includes a trailing newline after the closing sentinel so callers can
/// concatenate it directly with the script body.
#[allow(dead_code)]
pub fn render_comment_header(h: &Header, prefix: &str) -> String {
    if h.is_empty() {
        return String::new();
    }

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("{}{}", prefix, OPEN_TAG));

    if !h.tags.is_empty() {
        let items = h
            .tags
            .iter()
            .map(|t| format!("\"{}\"", t))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("{} tags: [{}]", prefix, items));
    }
    if let Some(ref s) = h.summary {
        lines.push(format!("{} summary: {}", prefix, s));
    }
    if !h.relates_to.is_empty() {
        let items = h
            .relates_to
            .iter()
            .map(|r| format!("\"{}\"", r))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("{} relates_to: [{}]", prefix, items));
    }
    if let Some(ref s) = h.created {
        lines.push(format!("{} created: {}", prefix, s));
    }
    if let Some(ref s) = h.updated {
        lines.push(format!("{} updated: {}", prefix, s));
    }
    if let Some(ref s) = h.expires {
        lines.push(format!("{} expires: {}", prefix, s));
    }
    for (k, v) in &h.extras {
        lines.push(format!("{} {}: {}", prefix, k, v));
    }

    lines.push(format!("{}{}", prefix, CLOSE_TAG));
    lines.join("\n") + "\n"
}

/// Parse YAML `---` frontmatter into a [`Header`].
///
/// Expects the form `---\n{fields}\n---\n{body}`.  Returns
/// `(header, body_start)` where `body_start` is the byte offset of the first
/// character after the closing `---\n`.  When no valid frontmatter is found,
/// `body_start == 0` and the returned header is [`Header::default()`].
///
/// This is a compatibility shim so callers that previously called the private
/// parsers in `runbook.rs` / `memory.rs` can migrate to the shared type.
#[allow(dead_code)]
pub fn parse_yaml_frontmatter(src: &str) -> (Header, usize) {
    if !src.starts_with("---\n") {
        return (Header::default(), 0);
    }
    let search_from = 4; // skip "---\n"
    let end_marker = "\n---\n";
    if let Some(rel) = src[search_from..].find(end_marker) {
        let fm_end = search_from + rel;
        let body_start = fm_end + end_marker.len();
        let kv: Vec<String> = src[search_from..fm_end]
            .lines()
            .map(str::to_string)
            .collect();
        (parse_kv_fields(&kv), body_start)
    } else {
        (Header::default(), 0)
    }
}

/// Inject `session_origin: "<name>"` into YAML `---` frontmatter.
///
/// If the content already has a frontmatter block, inserts the field before
/// the closing `---`.  If there is no frontmatter, prepends a minimal block.
/// Returns the content unchanged if `session_origin` is already present.
pub fn inject_yaml_session_origin(content: &str, name: &str) -> String {
    if content.starts_with("---\n") {
        let after_open = &content[4..];
        if let Some(rel) = after_open.find("\n---\n") {
            let fm_body = &after_open[..rel];
            if fm_body.contains("session_origin:") {
                return content.to_string();
            }
            let rest = &after_open[rel..]; // starts with "\n---\n"
            return format!("---\n{}\nsession_origin: \"{}\"{}", fm_body, name, rest);
        }
    }
    // No valid frontmatter — prepend a minimal block.
    format!("---\nsession_origin: \"{}\"\n---\n{}", name, content)
}

/// Inject `session_origin: "<name>"` into a comment-block (script) header.
///
/// Uses `parse_comment_header` / `render_comment_header`.  Any shebang on the
/// first line is preserved.  If the script has no existing daemoneye header, a
/// minimal one is added.  Returns content unchanged if already stamped.
pub fn inject_comment_session_origin(content: &str, name: &str) -> String {
    let (mut hdr, body_start) = parse_comment_header(content);
    if hdr.extras.contains_key("session_origin") {
        return content.to_string();
    }
    hdr.extras
        .insert("session_origin".to_string(), name.to_string());

    let shebang_end = if content.starts_with("#!") {
        content.find('\n').map(|i| i + 1).unwrap_or(content.len())
    } else {
        0
    };
    let shebang = &content[..shebang_end];
    let body = if body_start > 0 {
        &content[body_start..]
    } else {
        &content[shebang_end..]
    };
    format!("{}{}{}", shebang, render_comment_header(&hdr, "#"), body)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_comment_header ---

    #[test]
    fn shebang_then_header() {
        let src = "#!/usr/bin/env bash\n\
                   # --- daemoneye ---\n\
                   # tags: [disk, cleanup]\n\
                   # summary: Rotate logs\n\
                   # --- /daemoneye ---\n\
                   set -euo pipefail\n";
        let (h, body_start) = parse_comment_header(src);
        assert_eq!(h.tags, vec!["disk", "cleanup"]);
        assert_eq!(h.summary.as_deref(), Some("Rotate logs"));
        assert_eq!(&src[body_start..], "set -euo pipefail\n");
    }

    #[test]
    fn header_without_shebang() {
        let src = "# --- daemoneye ---\n\
                   # tags: [cert, tls]\n\
                   # --- /daemoneye ---\n\
                   echo done\n";
        let (h, body_start) = parse_comment_header(src);
        assert_eq!(h.tags, vec!["cert", "tls"]);
        assert_eq!(&src[body_start..], "echo done\n");
    }

    #[test]
    fn no_header_returns_default() {
        let src = "#!/usr/bin/env bash\nset -e\necho hi\n";
        let (h, body_start) = parse_comment_header(src);
        assert!(h.is_empty());
        assert_eq!(body_start, 0);
    }

    #[test]
    fn unclosed_header_ignored() {
        let src = "# --- daemoneye ---\n# tags: [x]\n# (no closing sentinel)\necho done\n";
        let (h, body_start) = parse_comment_header(src);
        assert!(h.is_empty());
        assert_eq!(body_start, 0);
    }

    #[test]
    fn slash_slash_prefix() {
        let src = "// --- daemoneye ---\n\
                   // tags: [js, api]\n\
                   // summary: Fetch users\n\
                   // --- /daemoneye ---\n\
                   console.log('hi');\n";
        let (h, body_start) = parse_comment_header(src);
        assert_eq!(h.tags, vec!["js", "api"]);
        assert_eq!(h.summary.as_deref(), Some("Fetch users"));
        assert_eq!(&src[body_start..], "console.log('hi');\n");
    }

    #[test]
    fn extras_captured() {
        let src = "# --- daemoneye ---\n\
                   # tags: [nginx]\n\
                   # run_with_sudo: true\n\
                   # max_turns: 5\n\
                   # --- /daemoneye ---\n";
        let (h, _) = parse_comment_header(src);
        assert_eq!(h.tags, vec!["nginx"]);
        assert_eq!(
            h.extras.get("run_with_sudo").map(String::as_str),
            Some("true")
        );
        assert_eq!(h.extras.get("max_turns").map(String::as_str), Some("5"));
    }

    #[test]
    fn extras_sorted() {
        let src = "# --- daemoneye ---\n\
                   # zebra: z\n\
                   # alpha: a\n\
                   # --- /daemoneye ---\n";
        let (h, _) = parse_comment_header(src);
        let keys: Vec<_> = h.extras.keys().collect();
        assert_eq!(keys, vec!["alpha", "zebra"]);
    }

    #[test]
    fn relates_to_parsed() {
        let src = "# --- daemoneye ---\n\
                   # relates_to: [high-disk-usage, cleanup-runbook]\n\
                   # --- /daemoneye ---\n";
        let (h, _) = parse_comment_header(src);
        assert_eq!(h.relates_to, vec!["high-disk-usage", "cleanup-runbook"]);
    }

    #[test]
    fn quoted_tags_stripped() {
        let src = "# --- daemoneye ---\n\
                   # tags: [\"disk\", 'cleanup']\n\
                   # --- /daemoneye ---\n";
        let (h, _) = parse_comment_header(src);
        assert_eq!(h.tags, vec!["disk", "cleanup"]);
    }

    // --- render_comment_header ---

    #[test]
    fn render_empty_returns_empty_string() {
        assert_eq!(render_comment_header(&Header::default(), "#"), "");
    }

    #[test]
    fn render_basic_fields() {
        let h = Header {
            tags: vec!["disk".into(), "cleanup".into()],
            summary: Some("Rotate logs".into()),
            ..Default::default()
        };
        let out = render_comment_header(&h, "#");
        assert!(out.contains("# --- daemoneye ---"));
        assert!(out.contains("# --- /daemoneye ---"));
        assert!(out.contains("# tags: [\"disk\", \"cleanup\"]"));
        assert!(out.contains("# summary: Rotate logs"));
    }

    #[test]
    fn render_extras() {
        let mut h = Header::default();
        h.tags = vec!["nginx".into()];
        h.extras.insert("run_with_sudo".into(), "true".into());
        let out = render_comment_header(&h, "#");
        assert!(out.contains("# run_with_sudo: true"), "got: {out}");
    }

    // --- round-trip ---

    #[test]
    fn round_trip() {
        let original = Header {
            tags: vec!["disk".into(), "cleanup".into()],
            summary: Some("Rotate logs when /var is full".into()),
            relates_to: vec!["high-disk-usage".into()],
            created: Some("2026-04-20".into()),
            expires: Some("2027-01-01".into()),
            ..Default::default()
        };
        let rendered = render_comment_header(&original, "#");
        let (parsed, _) = parse_comment_header(&rendered);
        assert_eq!(parsed, original);
    }

    #[test]
    fn round_trip_with_shebang() {
        let h = Header {
            tags: vec!["cert".into()],
            summary: Some("Rotate TLS certs".into()),
            ..Default::default()
        };
        let header_block = render_comment_header(&h, "#");
        let script = format!("#!/usr/bin/env bash\n{}openssl ...\n", header_block);
        let (parsed, body_start) = parse_comment_header(&script);
        assert_eq!(parsed, h);
        assert_eq!(&script[body_start..], "openssl ...\n");
    }

    // --- parse_yaml_frontmatter ---

    #[test]
    fn yaml_frontmatter_basic() {
        let src = "---\ntags: [disk, storage]\nsummary: \"Disk runbook\"\nexpires: 2027-01-01\n---\n# Runbook: disk-check\n";
        let (h, body_start) = parse_yaml_frontmatter(src);
        assert_eq!(h.tags, vec!["disk", "storage"]);
        assert_eq!(h.summary.as_deref(), Some("Disk runbook"));
        assert_eq!(h.expires.as_deref(), Some("2027-01-01"));
        assert_eq!(&src[body_start..], "# Runbook: disk-check\n");
    }

    #[test]
    fn yaml_frontmatter_no_delimiter_returns_default() {
        let src = "tags: [x]\n# Runbook: foo\n";
        let (h, body_start) = parse_yaml_frontmatter(src);
        assert!(h.is_empty());
        assert_eq!(body_start, 0);
    }

    #[test]
    fn yaml_frontmatter_unclosed_returns_default() {
        let src = "---\ntags: [x]\n# no closing delimiter\n";
        let (h, body_start) = parse_yaml_frontmatter(src);
        assert!(h.is_empty());
        assert_eq!(body_start, 0);
    }

    #[test]
    fn yaml_frontmatter_extras() {
        let src = "---\ntags: [nginx]\nenabled: true\nmax_ghost_turns: 10\n---\nbody\n";
        let (h, _) = parse_yaml_frontmatter(src);
        assert_eq!(h.extras.get("enabled").map(String::as_str), Some("true"));
        assert_eq!(
            h.extras.get("max_ghost_turns").map(String::as_str),
            Some("10")
        );
    }

    // --- inject_yaml_session_origin ---

    #[test]
    fn inject_yaml_adds_field_before_closing_delimiter() {
        let src = "---\ntags: [disk]\nsummary: \"test\"\n---\nbody\n";
        let out = inject_yaml_session_origin(src, "my-session");
        assert!(out.contains("session_origin: \"my-session\""), "got: {out}");
        assert!(out.starts_with("---\n"), "should still start with ---");
        assert!(
            out.contains("---\nbody\n"),
            "body should be preserved: {out}"
        );
    }

    #[test]
    fn inject_yaml_no_frontmatter_prepends_block() {
        let src = "# Runbook: foo\n\nbody\n";
        let out = inject_yaml_session_origin(src, "postgres-incident");
        assert!(out.starts_with("---\nsession_origin: \"postgres-incident\"\n---\n"));
        assert!(out.ends_with("# Runbook: foo\n\nbody\n"));
    }

    #[test]
    fn inject_yaml_idempotent() {
        let src = "---\ntags: [disk]\nsession_origin: \"existing\"\n---\nbody\n";
        let out = inject_yaml_session_origin(src, "new-session");
        assert_eq!(
            out, src,
            "should not modify content with existing session_origin"
        );
    }

    // --- inject_comment_session_origin ---

    #[test]
    fn inject_comment_adds_field_to_existing_header() {
        let src = "#!/usr/bin/env bash\n\
                   # --- daemoneye ---\n\
                   # tags: [disk]\n\
                   # --- /daemoneye ---\n\
                   echo done\n";
        let out = inject_comment_session_origin(src, "my-session");
        assert!(out.contains("# session_origin: my-session"), "got: {out}");
        assert!(out.starts_with("#!/usr/bin/env bash\n"));
        assert!(out.ends_with("echo done\n"));
    }

    #[test]
    fn inject_comment_adds_header_when_none() {
        let src = "#!/usr/bin/env bash\necho hello\n";
        let out = inject_comment_session_origin(src, "my-session");
        assert!(out.starts_with("#!/usr/bin/env bash\n"));
        assert!(out.contains("# --- daemoneye ---"));
        assert!(out.contains("# session_origin: my-session"));
        assert!(out.contains("echo hello\n"));
    }

    #[test]
    fn inject_comment_idempotent() {
        let src = "#!/usr/bin/env bash\n\
                   # --- daemoneye ---\n\
                   # session_origin: existing\n\
                   # --- /daemoneye ---\n\
                   echo done\n";
        let out = inject_comment_session_origin(src, "new-session");
        assert_eq!(
            out, src,
            "should not modify content with existing session_origin"
        );
    }
}
