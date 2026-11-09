//! The OSC 7501 parser (M36 phase 21a).

use std::collections::BTreeMap;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;


// ── The five states ──────────────────────────────────────────────────────---

/// The spec's five states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Idle,
    Working,
    Blocked,
    Done,
    Error,
}

impl State {
    fn from_str(s: &[u8]) -> Option<Self> {
        match s {
            b"idle" => Some(Self::Idle),
            b"working" => Some(Self::Working),
            b"blocked" => Some(Self::Blocked),
            b"done" => Some(Self::Done),
            b"error" => Some(Self::Error),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Error => "error",
        }
    }

    /// `working` and `blocked` die when the program that reported them stops
    /// holding the terminal; the rest survive until replaced or cleared.
    pub fn is_ephemeral(self) -> bool {
        matches!(self, Self::Working | Self::Blocked)
    }
}

/// The three kinds a `blocked` report may name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Permission,
    Question,
    Auth,
}

impl Kind {
    fn from_str(s: &[u8]) -> Option<Self> {
        match s {
            b"permission" => Some(Self::Permission),
            b"question" => Some(Self::Question),
            b"auth" => Some(Self::Auth),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Question => "question",
            Self::Auth => "auth",
        }
    }
}

/// One program's status, exactly as its report left it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Record {
    /// The `/`-separated id path; `None` is the root record.
    #[serde(default)]
    pub id: Option<String>,
    pub state: State,
    /// Only meaningful with `state = blocked`; always `None` otherwise.
    #[serde(default)]
    pub kind: Option<Kind>,
    /// `0..=100`; absent reads as indeterminate.
    #[serde(default)]
    pub progress: Option<u8>,
    /// The record's own `app`, or the nearest ancestor's at read time.
    #[serde(default)]
    pub app: Option<String>,
    /// Decoded title text — data, never markup.
    #[serde(default)]
    pub title: Option<String>,
    /// Decoded msg text — data, never markup.
    #[serde(default)]
    pub msg: Option<String>,
}


// ── Grammar ──────────────────────────────────────────────────────────────---

const KEY_LIMIT: usize = 16;
const MSG_ENCODED_LIMIT: usize = 2732;
const MSG_DECODED_LIMIT: usize = 2048;
const TITLE_ENCODED_LIMIT: usize = 256;
const TITLE_DECODED_LIMIT: usize = 192;
const ID_TOTAL_LIMIT: usize = 128;
const ID_SEGMENT_LIMIT: usize = 32;
const ID_LEVELS_LIMIT: usize = 8;
/// Payload length that reads as the 4096-byte sequence limit.
const SEQUENCE_DROP: usize = 4088;
/// The records-per-terminal cap; the least recently updated goes past it.
const RECORDS_CAP: usize = 256;

/// Which limit a report broke, named so tests can pin each drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    Key,
    MsgEncoded,
    MsgDecoded,
    TitleEncoded,
    TitleDecoded,
    App,
    Id,
    Sequence,
}

/// Why one pair dropped. `Key` also covers an unknown `state` value: the
/// state pair is the report's spine, so an unreadable one drops the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairError {
    State,
    Id,
    Progress,
    App,
    Title(Limit),
    Msg(Limit),
    Key,
}

fn segment_ok(seg: &str) -> bool {
    !seg.is_empty()
        && seg.len() <= ID_SEGMENT_LIMIT
        && seg
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-'))
}

/// The `id` grammar: 8 `/`-separated segments of `[A-Za-z0-9_.+-]{1,32}`,
/// 128 bytes total.
pub fn parse_id(id: &str) -> Option<String> {
    (id.len() <= ID_TOTAL_LIMIT
        && id.split('/').count() <= ID_LEVELS_LIMIT
        && !id.is_empty()
        && id.split('/').all(segment_ok))
    .then(|| id.to_string())
}

/// The `app` grammar: one segment of the id shape.
fn parse_app(app: &str) -> Option<String> {
    segment_ok(app).then(|| app.to_string())
}

/// The decoded-text security rule: no U+0000–U+001F, no U+007F, no
/// U+0080–U+009F. Newline and tab are U+000A / U+0009, so they are in.
fn control_free(text: &str) -> bool {
    text.chars()
        .all(|c| !c.is_control() && !('\u{80}'..='\u{9f}').contains(&c))
}

/// Standard base64 with optional padding, then the decoded limits and the
/// control rule.
fn decode_field(raw: &str, decoded_limit: usize) -> Option<String> {
    let text = String::from_utf8(STANDARD.decode(raw).ok()?).ok()?;
    (control_free(&text) && text.len() <= decoded_limit).then_some(text)
}

/// One parsed pair's value, keyed by its key.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pair {
    State(State),
    Id(String),
    Kind(Option<Kind>),
    Progress(u8),
    App(String),
    Title(String),
    Msg(String),
}

fn parse_pair(key: &str, value: &str) -> Result<Pair, PairError> {
    if key.len() > KEY_LIMIT || !key.bytes().all(|b| b.is_ascii_lowercase()) {
        return Err(PairError::Key);
    }
    match key {
        "state" => State::from_str(value.as_bytes()).map(Pair::State).ok_or(PairError::State),
        "id" => parse_id(value).map(Pair::Id).ok_or(PairError::Id),
        // An unknown kind is absent, not malformed (Keys section).
        "kind" => Ok(Pair::Kind(Kind::from_str(value.as_bytes()))),
        "progress" => value
            .parse::<u8>()
            .ok()
            .filter(|p| *p <= 100)
            .map(Pair::Progress)
            .ok_or(PairError::Progress),
        "app" => parse_app(value).map(Pair::App).ok_or(PairError::App),
        "title" => {
            if value.len() > TITLE_ENCODED_LIMIT {
                Err(PairError::Title(Limit::TitleEncoded))
            } else {
                decode_field(value, TITLE_DECODED_LIMIT)
                    .map(Pair::Title)
                    .ok_or(PairError::Title(Limit::TitleDecoded))
            }
        }
        "msg" => {
            if value.len() > MSG_ENCODED_LIMIT {
                Err(PairError::Msg(Limit::MsgEncoded))
            } else {
                decode_field(value, MSG_DECODED_LIMIT)
                    .map(Pair::Msg)
                    .ok_or(PairError::Msg(Limit::MsgDecoded))
            }
        }
        // Unknown keys are ignored, not reported.
        _ => Ok(Pair::Kind(None)),
    }
}

// ── The report ───────────────────────────────────────────────────────────---

/// What one OSC 7501 sequence carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// `OSC 7501 ; ? ST`, the feature query.
    Query,
    /// One program-status report: the values the pairs set. Each `Option`
    /// reads exactly as the digest's Keys section: absent vs. malformed, so
    /// `reduce` can tell a dropped record from a half-applied one.
    Status {
        state: Option<State>,
        id: Option<String>,
        kind: Option<Kind>,
        progress: Option<u8>,
        app: Option<String>,
        title: Option<String>,
        msg: Option<String>,
    },
}

/// Reduce the values a report carried into the [`Record`] its id's slot
/// should now hold, or drop the whole report for one of the whole-report
/// conditions (Syntax, Keys, Limits).
///
/// - an unknown or missing `state` → dropped;
/// - any `kind` on a state other than `blocked` → the kind is absent;
/// - an `id` failing the grammar has already been dropped by the pair pass,
///   so the root record cannot be overwritten by a bad id.
pub(crate) fn reduce(
    state: Option<State>,
    kind: Option<Kind>,
    id: Option<String>,
) -> Option<(Option<String>, State, Option<Kind>)> {
    let state = state?;
    let kind = if state == State::Blocked { kind } else { None };
    Some((id, state, kind))
}

/// Parse the payload — the bytes after `7501;` vte already split off — into
/// a [`Report`]. `None` for a payload that is not the protocol's shape.
/// `limit` receives which sequence limit a dropped payload broke.
pub fn parse(payload: &[u8], limit: &mut Option<Limit>) -> Option<Report> {
    if payload.len() >= SEQUENCE_DROP {
        *limit = Some(Limit::Sequence);
        return None;
    }
    let text = std::str::from_utf8(payload).ok()?;
    if text.split_once(';').is_some() {
        // The value grammar holds no `;`, so a semicolon is not this
        // protocol's shape: the query arrives as exactly `?`.
        return None;
    }
    if text.trim() == "?" {
        return Some(Report::Query);
    }
    let mut state = None;
    let mut id = None;
    let mut kind = None;
    let mut progress = None;
    let mut app = None;
    let mut title = None;
    let mut msg = None;
    let mut saw_pair = false;
    for pair in text.split(':') {
        let pair = pair.trim();
        if pair.is_empty() {
            return None;
        }
        let Some((key, value)) = pair.split_once('=') else {
            // A pair without `=` is malformed; it is skipped and the rest
            // still applies (Syntax section).
            continue;
        };
        saw_pair = true;
        match parse_pair(&key.trim().to_lowercase(), value.trim()) {
            Ok(Pair::State(s)) => state = Some(s),
            Ok(Pair::Id(i)) => id = Some(i),
            Ok(Pair::Kind(k)) => kind = k,
            Ok(Pair::Progress(p)) => progress = Some(p),
            Ok(Pair::App(a)) => app = Some(a),
            Ok(Pair::Title(t)) => title = Some(t),
            Ok(Pair::Msg(m)) => msg = Some(m),
            Err(_) => {}
        }
    }
    if !saw_pair {
        return None;
    }
    let (id, state, kind) = reduce(state, kind, id)?;
    Some(Report::Status {
        state: Some(state),
        id,
        kind,
        progress,
        app,
        title,
        msg,
    })
}

/// The record table one shell host holds.
#[derive(Debug, Default)]
pub struct Table {
    records: BTreeMap<String, Slot>,
    /// The root record: `Option` of the same shape, no id.
    root: Option<Slot>,
}

/// One report's slot in the table: the record and how recently it updated.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Slot {
    record: Record,
    updated: u64,
}

impl Table {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// The records as the frame carries them: root first, then by id.
    pub fn records(&self) -> Vec<Record> {
        let mut out = Vec::new();
        if let Some(slot) = &self.root {
            out.push(slot.record.clone());
        }
        for slot in self.records.values() {
            out.push(slot.record.clone());
        }
        out
    }

    /// Apply one report: insert or replace the addressed record whole.
    pub fn apply(&mut self, report: Record, updated: u64) {
        let slot = Slot {
            record: report,
            updated,
        };
        match &slot.record.id {
            None => self.root = Some(slot),
            Some(id) => {
                if self.records.len() >= RECORDS_CAP && !self.records.contains_key(id)
                    && let Some(oldest) = self
                        .records
                        .iter()
                        .min_by_key(|(_, s)| s.updated)
                        .map(|(k, _)| k.clone())
                    {
                        self.records.remove(&oldest);
                    }
                self.records.insert(id.clone(), slot);
            }
        }
    }

    /// Clear the record `id` names and every record beneath it.
    pub fn clear_one(&mut self, id: &str) {
        self.records.retain(|other, _| {
            !(other == id || other.strip_prefix(&format!("{id}/")).is_some())
        });
        if self.root.as_ref().is_some_and(|s| s.record.id.as_deref() == Some(id)) {
            self.root = None;
        }
    }

    /// Drop working and blocked records whose program no longer holds the
    /// terminal (`holding` false).
    pub fn expire(&mut self, holding: bool) {
        if holding {
            return;
        }
        self.root = self
            .root
            .take()
            .filter(|s| !s.record.state.is_ephemeral());
        self.records.retain(|_, s| !s.record.state.is_ephemeral());
    }
}
// The `reduce` helper folds a Report::Status into the record a slot gets;
// tests exercise it directly until phase 21b wires the frame.
