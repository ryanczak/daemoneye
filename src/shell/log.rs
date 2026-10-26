//! asciicast v2 recorder for a shell session and its `.meta.json` command index.
//!
//! Writes newline-delimited JSON: a header line, then per-record event lines
//! `[t, code, data]`. The command index records each command's byte range in
//! the cast file plus its real exit code, so reads slice a single command in
//! O(1) and return exactly that command's output bytes.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::pty::{Nonce, begin_marker, end_marker, find_subslice};

const MAX_CARRY: usize = 3;

/// Writes an asciicast v2 recording, one JSON line per record, flushed per
/// record so a live tail sees motion immediately.
pub struct CastWriter {
    file: File,
    carry: Vec<u8>,
}

#[derive(Serialize)]
struct Header {
    version: u32,
    width: u16,
    height: u16,
    timestamp: u64,
}

impl CastWriter {
    pub fn create(path: &Path, cols: u16, rows: u16, started_unix: u64) -> Result<Self> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        let header = Header {
            version: 2,
            width: cols,
            height: rows,
            timestamp: started_unix,
        };
        writeln!(file, "{}", serde_json::to_string(&header)?)?;
        file.flush()?;
        Ok(Self {
            file,
            carry: Vec::with_capacity(MAX_CARRY),
        })
    }

    pub fn write_output(&mut self, at: Duration, bytes: &[u8]) -> Result<()> {
        self.write_event(at, "o", bytes)
    }

    pub fn write_input(&mut self, at: Duration, bytes: &[u8]) -> Result<()> {
        self.write_event(at, "i", bytes)
    }

    pub fn mark(&mut self, at: Duration, label: &str) -> Result<()> {
        self.flush_carry(at)?;
        let line = format!(
            "[{}, \"m\", {}]\n",
            fmt_time(at),
            serde_json::to_string(label)?
        );
        self.file.write_all(line.as_bytes())?;
        self.file.flush()?;
        Ok(())
    }

    pub fn byte_len(&self) -> u64 {
        self.file.metadata().map(|m| m.len()).unwrap_or(0)
    }

    fn write_event(&mut self, at: Duration, code: &'static str, bytes: &[u8]) -> Result<()> {
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(bytes);
        let (text, carry): (String, Vec<u8>) = match std::str::from_utf8(&buf) {
            Ok(s) => (s.to_string(), Vec::new()),
            Err(e) if e.error_len().is_none() => {
                let valid = e.valid_up_to();
                (
                    String::from_utf8_lossy(&buf[..valid]).into_owned(),
                    buf[valid..].to_vec(),
                )
            }
            Err(_) => (String::from_utf8_lossy(&buf).into_owned(), Vec::new()),
        };
        self.carry = carry;
        if text.is_empty() {
            return Ok(());
        }
        let line = format!(
            "[{}, \"{}\", {}]\n",
            fmt_time(at),
            code,
            serde_json::to_string(&text)?
        );
        self.file.write_all(line.as_bytes())?;
        self.file.flush()?;
        Ok(())
    }

    fn flush_carry(&mut self, at: Duration) -> Result<()> {
        if self.carry.is_empty() {
            return Ok(());
        }
        // The carry is, by construction, an incomplete sequence: no more input
        // will arrive for the finishing command, so it can never be completed.
        // Emit it lossily rather than hold it, so the accepted bytes land in
        // this command's range instead of polluting the next write.
        let text = String::from_utf8_lossy(&self.carry).into_owned();
        self.carry.clear();
        if text.is_empty() {
            return Ok(());
        }
        let line = format!(
            "[{}, \"o\", {}]\n",
            fmt_time(at),
            serde_json::to_string(&text)?
        );
        self.file.write_all(line.as_bytes())?;
        self.file.flush()?;
        Ok(())
    }
}

fn fmt_time(at: Duration) -> String {
    let secs = at.as_secs_f64();
    format!("{}", (secs * 1e6).round() / 1e6)
}

/// One command's entry in the meta index.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CommandRecord {
    pub index: u32,
    pub command: String,
    pub started: f64,
    pub ended: f64,
    pub exit_code: i32,
    pub first_byte: u64,
    pub end_byte: u64,
}
/// The byte range, as cast-file offsets relative to the given slice, of one
/// command's clean output. `None` when either marker is absent — a command
/// whose markers never landed has no clean range and must not be indexed.
///
/// A cast event line holds exactly one PTY read chunk (the host's pump thread
/// writes one `"o"` event per chunk), and a PTY read boundary can fall
/// anywhere — including in the middle of a marker — so a marker can be split
/// across two event lines, visible to neither line alone. The finder
/// therefore decodes each line's `"o"` payload — a cast line stores a
/// marker's `0x1f` as the JSON escape `\u001f`, so the raw bytes the PTY
/// wrote cannot appear in a cast file — joins the payloads in cast order,
/// which is the PTY's byte stream itself, and searches **that** stream for
/// the framing `parse_outcome` reads (`begin_marker` / `end_marker`, never a
/// hand-written `DE_BEG` substring). The found positions are mapped back to
/// whole cast lines: the range starts where the line carrying the BEGIN
/// marker's first byte starts and ends where the line carrying the END
/// marker's last byte ends — the boundaries `read_command_output` decodes.
///
/// The on-screen **echo** of the wrapped command line carries `DE_BEG <nonce>`
/// verbatim but never the framed form: the split-quote in `wrap_command`
/// means only the shell's own `printf` output has `\x1f` on both sides of the
/// nonce, so searching the framed needles cannot match the echo.
pub fn marked_inner_range(cast_slice: &[u8], nonce: &Nonce) -> Option<(u64, u64)> {
    let begin = begin_marker(nonce);
    let end = end_marker(nonce);
    // One cast line = one PTY chunk. Each entry is the line's cast offset,
    // its raw length (for the range end) and its decoded payload.
    let mut lines: Vec<(u64, u64, Vec<u8>)> = Vec::new();
    let mut stream_len = 0usize;
    let mut offset = 0u64;
    for line in String::from_utf8_lossy(cast_slice).lines() {
        let mut decoded = Vec::new();
        decode_payload(Some(line.to_owned()), &mut decoded);
        let raw_len = line.len() as u64;
        stream_len += decoded.len();
        lines.push((offset, raw_len, std::mem::take(&mut decoded)));
        offset += raw_len + 1;
    }
    // The joined payloads are the PTY output in write order, so a marker
    // split across two chunks is contiguous here even though no single cast
    // line holds it.
    let mut stream = Vec::with_capacity(stream_len);
    for (_, _, payload) in &lines {
        stream.extend_from_slice(payload);
    }
    let beg_pos = find_subslice(&stream, &begin)?;
    let end_pos = find_subslice(&stream[beg_pos + begin.len()..], &end)? + beg_pos + begin.len();
    let line_holding = |stream_pos: usize| -> usize {
        let mut consumed = 0usize;
        for (i, (_, _, payload)) in lines.iter().enumerate() {
            if stream_pos < consumed + payload.len() {
                return i;
            }
            consumed += payload.len();
        }
        lines.len().saturating_sub(1)
    };
    let (start_line, end_line) = (line_holding(beg_pos), line_holding(end_pos + end.len() - 1));
    let (start_cast, end_line_offset, end_line_len) =
        (lines[start_line].0, lines[end_line].0, lines[end_line].1);
    Some((start_cast, end_line_offset + end_line_len + 1))
}

/// Rebuildable index derived from a cast file; never a second source of truth.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MetaIndex {
    pub shell_id: String,
    pub cast: String,
    pub started_unix: u64,
    pub commands: Vec<CommandRecord>,
    /// The owner's key (`Owner::key()`) — `chat:<session>` or `ghost:<job>`.
    /// `None` in an index written before the owner was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// `"local"` or the configured host name (`HostId::as_str()`). `None` in
    /// an index written before the host was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

impl MetaIndex {
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(json.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()?;
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        let mut contents = String::new();
        file.read_to_string(&mut contents)?;
        Ok(serde_json::from_str(&contents)?)
    }

    /// One more command in the index. The absolute `first_byte`/`end_byte` are
    /// byte offsets into `cast`; the caller already excluded the marker
    /// framing, so `read_command_output` needs no marker knowledge. `owner`
    /// and `host` fill an index that does not carry them yet; one that does
    /// keeps what it has.
    pub fn append(
        meta_path: &Path,
        cast: &Path,
        shell_id: &str,
        started_unix: u64,
        owner: Option<&str>,
        host: Option<&str>,
        record: CommandRecord,
    ) -> Result<()> {
        let mut meta = match MetaIndex::load(meta_path) {
            Ok(m) => m,
            Err(_) => MetaIndex {
                shell_id: shell_id.to_owned(),
                cast: cast
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                started_unix,
                commands: Vec::new(),
                owner: None,
                host: None,
            },
        };
        if meta.owner.is_none() {
            meta.owner = owner.map(str::to_owned);
        }
        if meta.host.is_none() {
            meta.host = host.map(str::to_owned);
        }
        let record = CommandRecord {
            index: meta.commands.len() as u32,
            ..record
        };
        meta.commands.push(record);
        meta.save(meta_path)
    }
}

/// The `.meta.json` sidecar path beside `cast`, replacing a `.cast` extension.
/// A path whose extension is not `.cast` gets `.meta.json` appended, so
/// `…/x.log` becomes `…/x.log.meta.json` rather than losing its `.log`.
pub fn meta_path_for(cast: &Path) -> PathBuf {
    let file_name = cast.file_name().unwrap_or_default();
    let lossy = file_name.to_string_lossy();
    if let Some(stem) = lossy.strip_suffix(".cast") {
        cast.with_file_name(format!("{stem}.meta.json"))
    } else {
        let mut os = file_name.to_owned();
        os.push(".meta.json");
        cast.with_file_name(os)
    }
}

/// Concatenated `"o"` payload bytes of the command at `index`, sliced from
/// the cast file by byte range. Skips `"i"` and `"m"` lines; a malformed line
/// inside the range is skipped rather than fatal.
pub fn read_command_output(cast: &Path, meta: &MetaIndex, index: u32) -> Result<Vec<u8>> {
    let record = meta
        .commands
        .iter()
        .find(|r| r.index == index)
        .ok_or_else(|| anyhow::anyhow!("no command record with index {index}"))?;
    let mut file = File::open(cast)?;
    file.seek(SeekFrom::Start(record.first_byte))?;
    let len = (record.end_byte - record.first_byte) as usize;
    let mut bytes = vec![0u8; len];
    file.read_exact(&mut bytes)?;
    let reader = BufReader::new(&bytes[..]);
    let mut out = Vec::new();
    for line in reader.lines() {
        extend_output_line(line.ok(), &mut out);
    }
    Ok(out)
}

/// Concatenated `"o"` payload bytes of the **complete** cast lines in
/// `cast[from..]`, and the byte offset one past the last complete line. The
/// offset deliberately stops before a still-unterminated final line, so the
/// next call — after the host has finished writing it — sees it whole. `"i"`
/// and `"m"` events and malformed lines are skipped, never fatal. A `from`
/// past the file's length (a truncated or rotated file) returns empty bytes
/// and `from` unchanged rather than seeking past the end.
pub fn read_output_since(cast: &Path, from: u64) -> Result<(Vec<u8>, u64)> {
    let mut file = File::open(cast)?;
    let len = file.metadata()?.len();
    if from >= len {
        return Ok((Vec::new(), from));
    }
    file.seek(SeekFrom::Start(from))?;
    let mut buf = Vec::with_capacity((len - from).min(1 << 20) as usize);
    file.read_to_end(&mut buf)?;
    let consumed = from + last_complete_line_end(&buf) as u64;
    let reader = BufReader::new(&buf[..last_complete_line_end(&buf)]);
    let mut out = Vec::new();
    for line in reader.lines() {
        extend_output_line(line.ok(), &mut out);
    }
    Ok((out, consumed))
}

/// Index one past the last `\n` in `buf`, or 0 when there is none — the bytes
/// after it are an incomplete final line and must stay unconsumed.
fn last_complete_line_end(buf: &[u8]) -> usize {
    match buf.iter().rposition(|b| *b == b'\n') {
        Some(i) => i + 1,
        None => 0,
    }
}

/// Extend `out` with the `"o"` payload of one cast event line. An unparsable
/// line, a malformed JSON line and any non-output event contribute nothing —
/// skipping rather than failing is the contract both readers share.
pub(crate) fn extend_output_line(line: Option<String>, out: &mut Vec<u8>) {
    decode_payload(line, out);
    let data = std::mem::take(out);
    *out = windowed_payload(&data);
}

/// The `"o"` payload of one cast event line, verbatim. Unparsable lines,
/// non-`"o"` events and malformed JSON contribute nothing — skipping rather
/// than failing is the contract both readers share.
pub(crate) fn decode_payload(line: Option<String>, out: &mut Vec<u8>) {
    let Some(line) = line else { return };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
        return;
    };
    let Some(array) = value.as_array() else {
        return;
    };
    if array.len() != 3 || array[1].as_str() != Some("o") {
        return;
    }
    let Some(data) = array[2].as_str() else {
        return;
    };
    out.extend_from_slice(data.as_bytes());
}

/// Reduce one decoded `"o"` payload to the clean bytes the marker protocol
/// frames as this command's output — the read-side twin of
/// [`parse_outcome`](crate::shell::parse_outcome)'s trim on the
/// subscription stream.
///
/// PTY read chunking is arbitrary: a read can carry the on-screen echo of
/// the wrapped command, the BEGIN marker, the output and the END marker in
/// one cast line, so a byte range that excludes whole cast lines cannot
/// return clean output for a bundled cast. The reader therefore windows each
/// payload itself: bytes before the payload's BEGIN marker are framing (the
/// echo), bytes from the END marker's exit code on are framing too. This is
/// a no-op for payloads without markers and for every pre-existing record
/// whose range already excludes them.
///
/// The framed shapes — `\x1fDE_BEG <32 hex>\x1f` and
/// `\x1fDE_END <32 hex> <digits>\x1f` — match protocol framing only; the
/// on-screen echo carries the split-quoted command text, which never has an
/// `\x1f` on both sides of the nonce.
fn windowed_payload(data: &[u8]) -> Vec<u8> {
    if !data.contains(&0x1f) {
        return data.to_vec();
    }
    let begin = b"\x1fDE_BEG ";
    let end = b"\x1fDE_END ";
    let start = find_subslice(data, begin)
        .map(|p| {
            let after = p + begin.len();
            let nonce_end = data[after..]
                .iter()
                .take_while(|b| b.is_ascii_hexdigit())
                .count()
                + after;
            match data.get(nonce_end) {
                Some(&0x1f) => {
                    let mut at = nonce_end + 1;
                    // BEGIN's own newline is framing.
                    if data.get(at..at + 2) == Some(b"\r\n") {
                        at += 2;
                    } else if data.get(at) == Some(&b'\n') {
                        at += 1;
                    }
                    at
                }
                _ => p,
            }
        })
        .unwrap_or_else(|| {
            // No complete BEGIN marker here — but a PTY read can cut one
            // anywhere, so this payload may still begin inside the marker
            // (`G <nonce>…`). Such a fragment is framing the reader must
            // drop, or the split marker leaks its tail as output.
            if leading_fragment_len(data, begin) > 0 {
                data.len()
            } else {
                0
            }
        });
    let from = &data[start..];
    let stop = find_subslice(from, end)
        .and_then(|p| {
            let after = p + end.len();
            let nonce_end = from[after..]
                .iter()
                .take_while(|b| b.is_ascii_hexdigit())
                .count()
                + after;
            if from.get(nonce_end) != Some(&b' ') {
                return None;
            }
            let mut at = nonce_end + 1;
            let digits = from[at..].iter().take_while(|b| b.is_ascii_digit()).count();
            if digits == 0 {
                return None;
            }
            at += digits;
            if from.get(at) == Some(&0x1f) {
                Some(p)
            } else {
                None
            }
        })
        .unwrap_or(from.len())
        .min(from.len());
    from[..stop].to_vec()
}

/// Length of the needle's head fragment sitting at the start of `data`: the
/// longest `k` such that `data[..k]` equals the needle's last `k` bytes. A
/// PTY read can split a marker, so a cast payload may start inside one.
/// `0` when `data` does not begin inside `needle`.
fn leading_fragment_len(data: &[u8], needle: &[u8]) -> usize {
    let limit = needle.len().saturating_sub(1).min(data.len());
    (1..=limit)
        .rev()
        .find(|&k| data[..k] == needle[needle.len() - k..])
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "log_tests.rs"]
mod tests;
