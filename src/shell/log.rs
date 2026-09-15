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
/// command's clean output: the decoded bytes strictly between this run's BEGIN
/// marker line and the line carrying its END marker. `None` when either marker
/// is absent — a command whose markers never landed has no clean range and
/// must not be indexed.
///
/// The finder is driven by the same marker bytes `parse_outcome` reads
/// (`begin_marker` / `end_marker`), never a hand-written `DE_BEG` substring. A
/// cast line stores a marker's `0x1f` as the JSON escape `\u001f`, so the raw
/// bytes the PTY wrote cannot appear in a cast file — the search must decode
/// each line's payload exactly the way `read_command_output` decodes it. The
/// on-screen **echo** of the wrapped command line carries `DE_BEG <nonce>`
/// verbatim but never the framed form: the split-quote in `wrap_command` means
/// only the shell's own `printf` output has `\x1f` on both sides of the nonce.
pub fn marked_inner_range(cast_slice: &[u8], nonce: &Nonce) -> Option<(u64, u64)> {
    let begin = begin_marker(nonce);
    let end = end_marker(nonce);
    let mut inner_start: Option<u64> = None;
    let mut offset = 0u64;
    for line in String::from_utf8_lossy(cast_slice).lines() {
        let mut decoded = Vec::new();
        decode_payload(Some(line.to_owned()), &mut decoded);
        if find_subslice(&decoded, &begin).is_some() {
            // A PTY read can bundle the BEGIN marker with the command's
            // first bytes, so the clean bytes may begin inside this cast
            // line; include the line and let the reader's decoded-payload
            // framing strip erase the marker.
            inner_start = Some(offset);
        } else if find_subslice(&decoded, &end).is_some() {
            // Same bundling on the END side: the line's tail can be output.
            return inner_start.map(|start| (start, offset + line.len() as u64 + 1));
        }
        offset += line.len() as u64 + 1;
    }
    None
}

/// Rebuildable index derived from a cast file; never a second source of truth.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MetaIndex {
    pub shell_id: String,
    pub cast: String,
    pub started_unix: u64,
    pub commands: Vec<CommandRecord>,
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
    /// framing, so `read_command_output` needs no marker knowledge.
    pub fn append(
        meta_path: &Path,
        cast: &Path,
        shell_id: &str,
        started_unix: u64,
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
            },
        };
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
fn extend_output_line(line: Option<String>, out: &mut Vec<u8>) {
    decode_payload(line, out);
    let data = std::mem::take(out);
    *out = windowed_payload(&data);
}

/// The `"o"` payload of one cast event line, verbatim. Unparsable lines,
/// non-`"o"` events and malformed JSON contribute nothing — skipping rather
/// than failing is the contract both readers share.
fn decode_payload(line: Option<String>, out: &mut Vec<u8>) {
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
        .unwrap_or(0);
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
                Some(start + p)
            } else {
                None
            }
        })
        .unwrap_or(from.len());
    from[..stop].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn cast(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("s7-1788470000-build.cast")
    }
    fn meta(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("s7-1788470000-build.meta.json")
    }

    #[test]
    fn cast_header_is_valid_asciicast_v2() {
        let dir = tempdir().unwrap();
        let path = cast(dir.path());
        CastWriter::create(&path, 80, 24, 1_788_470_000)
            .unwrap()
            .byte_len();
        let mut contents = String::new();
        File::open(&path)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        let first_line = contents.lines().next().unwrap();
        assert!(contents.as_bytes().starts_with(b"{"));
        let v: serde_json::Value = serde_json::from_str(first_line).unwrap();
        assert_eq!(v["version"], 2);
        assert_eq!(v["width"], 80);
        assert_eq!(v["height"], 24);
        assert_eq!(v["timestamp"], 1_788_470_000);
    }

    #[test]
    fn cast_event_line_shape() {
        let dir = tempdir().unwrap();
        let path = cast(dir.path());
        let mut w = CastWriter::create(&path, 80, 24, 0).unwrap();
        w.write_output(Duration::from_secs_f64(5.0), b"hello")
            .unwrap();
        w.write_output(Duration::from_secs_f64(0.123456789), b"x")
            .unwrap();
        let mut contents = String::new();
        File::open(&path)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        let event = contents.lines().nth(1).unwrap();
        let v: serde_json::Value = serde_json::from_str(event).unwrap();
        let array = v.as_array().unwrap();
        assert_eq!(array.len(), 3);
        assert!(array[0].is_number());
        assert!(array[0].as_f64().is_some());
        assert_eq!(array[1], "o");
        assert_eq!(array[2], "hello");
        let second = contents.lines().nth(2).unwrap();
        let v: serde_json::Value = serde_json::from_str(second).unwrap();
        assert_eq!(v[0], 0.123457);
    }

    #[test]
    fn cast_marker_and_input_events_use_their_codes() {
        let dir = tempdir().unwrap();
        let path = cast(dir.path());
        let mut w = CastWriter::create(&path, 80, 24, 0).unwrap();
        w.mark(Duration::from_secs_f64(10.0), "Configuration")
            .unwrap();
        w.mark(Duration::from_secs_f64(11.0), "").unwrap();
        w.write_input(Duration::from_secs_f64(12.0), b"h").unwrap();
        let mut contents = String::new();
        File::open(&path)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        let lines: Vec<_> = contents.lines().collect();
        assert_eq!(lines[1], r#"[10, "m", "Configuration"]"#);
        assert_eq!(lines[2], r#"[11, "m", ""]"#);
        assert_eq!(lines[3], r#"[12, "i", "h"]"#);
    }

    #[test]
    fn cast_preserves_ansi_and_unit_separator_bytes() {
        let dir = tempdir().unwrap();
        let path = cast(dir.path());
        let mut w = CastWriter::create(&path, 80, 24, 0).unwrap();
        let payload = b"a\x1fb\r\n\x1b[31mred\x1b[0m";
        let first_byte = w.byte_len();
        w.write_output(Duration::from_secs_f64(1.0), payload)
            .unwrap();
        w.mark(Duration::from_secs_f64(2.0), "end").unwrap();
        let end_byte = w.byte_len();
        drop(w);
        let meta = MetaIndex {
            shell_id: "s7".into(),
            cast: path.file_name().unwrap().to_string_lossy().into_owned(),
            started_unix: 0,
            commands: vec![CommandRecord {
                index: 0,
                command: "echo".into(),
                started: 0.0,
                ended: 1.0,
                exit_code: 0,
                first_byte,
                end_byte,
            }],
        };
        let out = read_command_output(&path, &meta, 0).unwrap();
        assert_eq!(out, payload);
    }

    #[test]
    fn cast_carries_a_split_multibyte_character() {
        let dir = tempdir().unwrap();
        let path = cast(dir.path());
        let bytes = "é世界😀".as_bytes();
        for cut in [1usize, 3, 8] {
            let mut w = CastWriter::create(&path, 80, 24, 0).unwrap();
            w.write_output(Duration::ZERO, &bytes[..cut]).unwrap();
            w.write_output(Duration::ZERO, &bytes[cut..]).unwrap();
            let mut contents = String::new();
            File::open(&path)
                .unwrap()
                .read_to_string(&mut contents)
                .unwrap();
            let mut whole = Vec::new();
            for line in contents.lines().skip(1) {
                let v: serde_json::Value = serde_json::from_str(line).unwrap();
                let arr = v.as_array().unwrap();
                if arr[1] == "o" {
                    whole.extend_from_slice(arr[2].as_str().unwrap().as_bytes());
                }
            }
            assert_eq!(whole, bytes, "cut point {cut}");
        }
        // a call whose input was entirely carried emits no line at all
        let mut w = CastWriter::create(&path, 80, 24, 0).unwrap();
        let before = w.byte_len();
        w.write_output(Duration::ZERO, &bytes[..1]).unwrap();
        assert_eq!(
            w.byte_len(),
            before,
            "carried-only write must not emit a line"
        );
    }

    #[test]
    fn cast_does_not_carry_genuinely_invalid_bytes() {
        let dir = tempdir().unwrap();
        let path = cast(dir.path());
        let mut w = CastWriter::create(&path, 80, 24, 0).unwrap();
        w.write_output(Duration::from_secs_f64(1.0), &[0xff, 0x41])
            .unwrap();
        w.write_output(Duration::from_secs_f64(2.0), b"ok").unwrap();
        let mut contents = String::new();
        File::open(&path)
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(
            lines.len(),
            3,
            "two events after the header, no endless carries"
        );
        let v: serde_json::Value = serde_json::from_str(lines[2]).unwrap();
        assert_eq!(v[2], "ok");
    }

    #[test]
    fn meta_round_trips_through_save_and_load() {
        let dir = tempdir().unwrap();
        let path = meta(dir.path());
        let m = MetaIndex {
            shell_id: "s7".into(),
            cast: "x.cast".into(),
            started_unix: 1_788_470_000,
            commands: vec![
                CommandRecord {
                    index: 0,
                    command: "echo hi".into(),
                    started: 0.0,
                    ended: 1.5,
                    exit_code: 0,
                    first_byte: 78,
                    end_byte: 188,
                },
                CommandRecord {
                    index: 1,
                    command: "ls".into(),
                    started: 2.0,
                    ended: 2.25,
                    exit_code: 2,
                    first_byte: 188,
                    end_byte: 300,
                },
            ],
        };
        m.save(&path).unwrap();
        let loaded = MetaIndex::load(&path).unwrap();
        assert_eq!(loaded, m);
    }

    #[test]
    fn meta_path_for_replaces_and_appends() {
        assert_eq!(
            meta_path_for(Path::new("/x/s7-1-build.cast")),
            PathBuf::from("/x/s7-1-build.meta.json")
        );
        assert_eq!(
            meta_path_for(Path::new("/x/deploy.log")),
            PathBuf::from("/x/deploy.log.meta.json")
        );
    }

    #[test]
    fn read_command_output_rejects_an_unknown_index() {
        let dir = tempdir().unwrap();
        let cast_path = cast(dir.path());
        CastWriter::create(&cast_path, 80, 24, 0).unwrap();
        let meta = MetaIndex {
            shell_id: "s7".into(),
            cast: cast_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            started_unix: 0,
            commands: vec![CommandRecord {
                index: 0,
                command: "echo".into(),
                started: 0.0,
                ended: 0.0,
                exit_code: 0,
                first_byte: 0,
                end_byte: 0,
            }],
        };
        let err = read_command_output(&cast_path, &meta, 7).unwrap_err();
        assert!(err.to_string().contains("7"));
    }

    #[test]
    fn read_command_output_skips_a_malformed_line() {
        let dir = tempdir().unwrap();
        let path = cast(dir.path());
        // hand-build a cast with a garbage line inside the command's range
        let header = r#"{"version":2,"width":80,"height":24,"timestamp":0}"#;
        let before = r#"[0, "o", "before"]"#;
        let garbage = "this is not json";
        let marker = r#"[0, "m", "m0"]"#;
        let after = r#"[0, "o", "after"]"#;
        let file = format!("{header}\n{before}\n{garbage}\n{marker}\n{after}\n");
        std::fs::write(&path, file).unwrap();
        let first_byte = (header.len() + 1) as u64;
        let end_byte = (header.len()
            + 1
            + before.len()
            + 1
            + garbage.len()
            + 1
            + marker.len()
            + 1
            + after.len()
            + 1) as u64;
        let meta = MetaIndex {
            shell_id: "s7".into(),
            cast: path.file_name().unwrap().to_string_lossy().into_owned(),
            started_unix: 0,
            commands: vec![CommandRecord {
                index: 0,
                command: "echo".into(),
                started: 0.0,
                ended: 1.0,
                exit_code: 0,
                first_byte,
                end_byte,
            }],
        };
        let out = read_command_output(&path, &meta, 0).unwrap();
        assert_eq!(out, b"beforeafter");
    }

    #[test]
    fn cast_and_meta_round_trip_a_three_command_session() {
        let dir = tempdir().unwrap();
        let path = cast(dir.path());
        let meta_path = meta(dir.path());
        let mut w = CastWriter::create(&path, 100, 30, 1_788_470_000).unwrap();
        let sessions: [(&str, Vec<u8>, i32); 3] = [
            ("echo one", b"one\r\n"[..].to_vec(), 0),
            ("printf no-trail", b"".to_vec(), 0),
            (
                "ls /missing",
                b"ls: /missing: No such file or directory\r\n"[..].to_vec(),
                2,
            ),
        ];
        let mut records = Vec::new();
        for (i, (cmd, out, exit)) in sessions.iter().enumerate() {
            let t = 1.0 + i as f64;
            let label = format!("cmd {i}");
            w.mark(Duration::from_secs_f64(t), &label).unwrap();
            let first_byte = w.byte_len();
            w.write_output(Duration::from_secs_f64(t + 0.5), out)
                .unwrap();
            let end_byte = w.byte_len();
            let end_label = format!("end {i}");
            w.mark(Duration::from_secs_f64(t + 1.0), &end_label)
                .unwrap();
            records.push(CommandRecord {
                index: i as u32,
                command: cmd.to_string(),
                started: t,
                ended: t + 0.5,
                exit_code: *exit,
                first_byte,
                end_byte,
            });
        }
        let meta = MetaIndex {
            shell_id: "s7".into(),
            cast: path.file_name().unwrap().to_string_lossy().into_owned(),
            started_unix: 1_788_470_000,
            commands: records,
        };
        meta.save(&meta_path).unwrap();
        drop(w);
        let meta_loaded = MetaIndex::load(&meta_path).unwrap();
        assert_eq!(meta_loaded, meta, "meta index round-trips");
        for i in 0..3u32 {
            let got = read_command_output(&path, &meta_loaded, i).unwrap();
            assert_eq!(got, sessions[i as usize].1, "command {i} output");
        }
    }

    #[test]
    fn cast_flushes_a_dangling_carry_before_a_marker() {
        let dir = tempdir().unwrap();
        let path = cast(dir.path());
        let mut w = CastWriter::create(&path, 80, 24, 0).unwrap();
        w.mark(Duration::ZERO, "start 0").unwrap();
        let first_byte = w.byte_len();
        // "é" is 2 bytes; feeding only the first two bytes of "ABéZ" leaves
        // the "é" second byte dangling.
        let raw = b"AB\xc3".to_vec();
        w.write_output(Duration::from_secs_f64(1.0), &raw).unwrap();
        let mid = w.byte_len();
        w.mark(Duration::from_secs_f64(1.1), "end 0").unwrap();
        assert!(mid < w.byte_len(), "mark must emit the carried bytes");
        let end_byte = w.byte_len();
        w.mark(Duration::from_secs_f64(2.0), "start 1").unwrap();
        let first_byte1 = w.byte_len();
        w.write_output(Duration::from_secs_f64(2.1), b"ZZZ")
            .unwrap();
        let end_byte1 = w.byte_len();
        drop(w);
        let meta = MetaIndex {
            shell_id: "s7".into(),
            cast: path.file_name().unwrap().to_string_lossy().into_owned(),
            started_unix: 0,
            commands: vec![
                CommandRecord {
                    index: 0,
                    command: "dangling".into(),
                    started: 0.0,
                    ended: 1.0,
                    exit_code: 0,
                    first_byte,
                    end_byte,
                },
                CommandRecord {
                    index: 1,
                    command: "next".into(),
                    started: 2.0,
                    ended: 2.1,
                    exit_code: 0,
                    first_byte: first_byte1,
                    end_byte: end_byte1,
                },
            ],
        };
        let got = read_command_output(&path, &meta, 0).unwrap();
        let mut want = b"AB".to_vec();
        want.extend_from_slice("\u{FFFD}".as_bytes());
        assert_eq!(got, want, "command 0 must keep all its bytes");
        let next = read_command_output(&path, &meta, 1).unwrap();
        assert_eq!(next, b"ZZZ", "command 1 must not be polluted");
    }

    #[test]
    fn log_cast_and_meta_are_created_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let cast_path = cast(dir.path());
        let meta_path = meta(dir.path());
        {
            let w = CastWriter::create(&cast_path, 80, 24, 0).unwrap();
            let meta = MetaIndex {
                shell_id: "s7".into(),
                cast: cast_path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                started_unix: 0,
                commands: vec![],
            };
            meta.save(&meta_path).unwrap();
            drop(w);
        }
        for (path, label) in [(&cast_path, "cast"), (&meta_path, "meta")] {
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o600,
                "{label} file must carry the explicit 0600 mode, regardless of umask"
            );
        }
    }

    /// Write one raw cast event line (`kind` = `"o"`, `"i"`, `"m"`), through
    /// the writer's real event path.
    fn event(w: &mut CastWriter, t: f64, kind: &str, data: &str) {
        let bytes = data.as_bytes();
        match kind {
            "o" => w.write_output(Duration::from_secs_f64(t), bytes).unwrap(),
            "i" => w.write_input(Duration::from_secs_f64(t), bytes).unwrap(),
            _ => w.mark(Duration::from_secs_f64(t), data).unwrap(),
        }
    }

    /// Append one raw line to `path` — a cast line the way a host would emit
    /// it, or any arbitrary bytes. The trailing newline is supplied only when
    /// `line` does not end with one.
    fn appender(path: &std::path::Path, line: String) {
        let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(line.as_bytes()).unwrap();
        f.write_all(b"\n").unwrap();
    }

    /// Append raw bytes to `path` with no newline appended.
    fn appender_raw(path: &std::path::Path, bytes: &[u8]) {
        let mut f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(bytes).unwrap();
    }

    #[test]
    fn read_output_since_returns_only_new_output() {
        let dir = tempdir().unwrap();
        let path = cast(dir.path());
        let mut w = CastWriter::create(&path, 80, 24, 0).unwrap();
        event(&mut w, 1.0, "o", "first\n");
        event(&mut w, 2.0, "o", "second\n");
        drop(w);
        let (first, offset) = read_output_since(&path, 0).unwrap();
        assert!(
            first.windows(5).any(|w| w == b"first"),
            "the first event's payload is in the read: {first:?}"
        );
        assert!(
            first.windows(6).any(|w| w == b"second"),
            "the second event's payload is in the read: {first:?}"
        );

        // Append a third event after the first read: reading from the
        // returned offset sees only the third.
        appender(
            &path,
            format!("[3.0, \"o\", {}]", serde_json::json!("third")),
        );
        let (again, _next) = read_output_since(&path, offset).unwrap();
        assert!(
            again.windows(5).any(|w| w == b"third"),
            "the appended event's payload comes back from the new offset: {again:?}"
        );
        assert!(
            !again.windows(5).any(|w| w == b"first"),
            "already-consumed bytes are never returned again: {again:?}"
        );
    }

    #[test]
    fn read_output_since_stops_at_the_last_complete_line() {
        let dir = tempdir().unwrap();
        let path = cast(dir.path());
        let mut w = CastWriter::create(&path, 80, 24, 0).unwrap();
        event(&mut w, 1.0, "o", "whole line\n");
        drop(w);
        let (_, after_first) = read_output_since(&path, 0).unwrap();

        appender(&path, r#"[2.0, "o", "complete\n"]"#.to_string());
        // No trailing newline: the final cast line is still being written.
        appender_raw(&path, b"[2.1, \"o\", \"partX");

        let (got, offset) = read_output_since(&path, after_first).unwrap();
        assert!(
            got.windows(8).any(|w| w == b"complete"),
            "the complete line's payload is returned: {got:?}"
        );
        assert!(
            !got.windows(4).any(|w| w == b"part"),
            "the unterminated final line must stay unconsumed: {got:?}"
        );
        let len = std::fs::metadata(&path).unwrap().len();
        assert!(
            offset <= len,
            "the offset must not exceed the file length: {offset} > {len}"
        );
        assert_ne!(
            offset, len,
            "a partial final line's bytes must be left unconsumed: {offset} == {len}"
        );
        assert!(
            offset > after_first,
            "the complete line before the partial one was consumed: {offset} vs {after_first}"
        );

        // Now complete the line: the appended text finishes the partial JSON.
        appender(&path, r#"ial line"]"#.to_string());
        let (more, _) = read_output_since(&path, offset).unwrap();
        assert!(
            more.windows(10).any(|w| w == b"partXial l"),
            "a line assembled across two reads comes back whole: {more:?}"
        );
        let (_, off2) = read_output_since(&path, offset).unwrap();
        assert_eq!(
            off2,
            std::fs::metadata(&path).unwrap().len(),
            "after completion a read consumes to the new end"
        );
    }

    #[test]
    fn read_output_since_skips_input_and_marker_events() {
        let dir = tempdir().unwrap();
        let path = cast(dir.path());
        let mut w = CastWriter::create(&path, 80, 24, 0).unwrap();
        event(&mut w, 1.0, "o", "out-1\n");
        event(&mut w, 2.0, "i", "keystrokes");
        event(&mut w, 3.0, "m", "a marker");
        event(&mut w, 4.0, "o", "out-2\n");
        drop(w);
        appender(&path, "not json at all".to_string());
        appender(&path, "[5.0, \"o\", \"out-3\\\\n\"]".to_string());
        let (got, _) = read_output_since(&path, 0).unwrap();
        let text = String::from_utf8_lossy(&got).into_owned();
        assert!(text.contains("out-1"), "first output kept: {text:?}");
        assert!(text.contains("out-2"), "second output kept: {text:?}");
        assert!(
            text.contains("out-3"),
            "output after the bad line kept, newline decoded: {text:?}"
        );
        assert!(
            !text.contains("keystrokes") && !text.contains("a marker"),
            "input and marker events are this tool's blind spot: {text:?}"
        );
        assert!(
            !text.contains("not json"),
            "a malformed line is skipped, not fatal: {text:?}"
        );
    }

    #[test]
    fn read_output_since_past_the_file_end_returns_unmoved_offset() {
        let dir = tempdir().unwrap();
        let path = cast(dir.path());
        let mut w = CastWriter::create(&path, 80, 24, 0).unwrap();
        event(&mut w, 1.0, "o", "short\n");
        drop(w);
        let len = std::fs::metadata(&path).unwrap().len();
        let (got, offset) = read_output_since(&path, len + 9).unwrap();
        assert!(got.is_empty(), "nothing beyond the end: {got:?}");
        assert_eq!(offset, len + 9, "the offset is returned unchanged");
    }

    /// Assemble the cast bytes one command's window would hold: the echoed
    /// command line, then BEGIN, the body and END, all through `CastWriter` so
    /// the encoding matches what a real cast stores (the marker's `0x1f`
    /// escapes as `\u001f`, never a raw byte).
    fn framed(nonce: &Nonce, echoed_line: &str, body: &[u8], exit: u32) -> Vec<u8> {
        let dir = tempdir().unwrap();
        let path = dir.path().join("framed.cast");
        let mut w = CastWriter::create(&path, 80, 24, 1).unwrap();
        if !echoed_line.is_empty() {
            w.write_output(Duration::ZERO, echoed_line.as_bytes())
                .unwrap();
        }
        w.write_output(Duration::ZERO, &begin_marker(nonce))
            .unwrap();
        w.write_output(Duration::ZERO, body).unwrap();
        w.write_output(Duration::ZERO, &{
            let mut e = end_marker(nonce);
            e.extend_from_slice(exit.to_string().as_bytes());
            e.push(0x1f);
            e
        })
        .unwrap();
        drop(w);
        std::fs::read(&path).unwrap()
    }

    /// The `"o"` payloads `read_command_output` would return for the recorded
    /// range: decode each complete cast line in `[first, end)` the same way the
    /// reader does. The same `extend_output_line` the shippers use, so a test
    /// cannot drift from production's decoding.
    fn decoded_range(slice: &[u8], first: u64, end: u64) -> Vec<u8> {
        let mut out = Vec::new();
        for line in String::from_utf8_lossy(&slice[first as usize..end as usize])
            .lines()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
        {
            extend_output_line(Some(line), &mut out);
        }
        out
    }

    #[test]
    fn marked_inner_range_excludes_the_marker_framing() {
        let nonce = Nonce::new();
        let body: &[u8] = b"stdout line one\nstdout line two\n";
        let slice = framed(&nonce, "", body, 0);
        let (first, end) = marked_inner_range(&slice, &nonce).expect("both markers present");
        let decoded = decoded_range(&slice, first, end);
        assert_eq!(decoded, body, "the range decodes to exactly the body");
        let text = String::from_utf8_lossy(&decoded).into_owned();
        assert!(
            !text.contains("DE_"),
            "no marker text in the range: {text:?}"
        );
        let range_text = String::from_utf8_lossy(&slice[first as usize..end as usize]).into_owned();
        let mut stripped = Vec::new();
        for line in range_text.lines() {
            let mut d = Vec::new();
            extend_output_line(Some(line.to_owned()), &mut d);
            stripped.extend_from_slice(&d);
        }
        assert_eq!(
            stripped, body,
            "after the reader's framing strip the range is exactly the body"
        );
    }

    #[test]
    fn marked_inner_range_is_none_when_a_marker_is_missing() {
        let nonce = Nonce::new();
        // BEGIN present, END absent: the command has not completed.
        let begin_only = {
            let mut b = begin_marker(&nonce);
            b.extend_from_slice(b"\r\nbody\r\n");
            b
        };
        assert!(
            marked_inner_range(&begin_only, &nonce).is_none(),
            "no END marker, no range"
        );
        // END present, BEGIN absent: an echoed indicator with no real start.
        let end_only = {
            let mut e = end_marker(&nonce);
            e.push(0x1f);
            e.extend_from_slice(b"0\r\n");
            e
        };
        assert!(
            marked_inner_range(&end_only, &nonce).is_none(),
            "no BEGIN marker, no range"
        );
    }

    #[test]
    fn marked_inner_range_ignores_the_echoed_command_line() {
        let nonce = Nonce::new();
        let echoed = "model$ printf '\\x1fDE_BEG 3f9c2a1f\\x1f\\n'; cargo build; printf ...\r\n";
        let body: &[u8] = b"cargo build output\n";
        let slice = framed(&nonce, echoed, body, 0);
        let (first, end) =
            marked_inner_range(&slice, &nonce).expect("the real markers are present");
        let decoded = decoded_range(&slice, first, end);
        assert_eq!(decoded, body, "the range decodes to exactly the body");
        let text = String::from_utf8_lossy(&decoded).into_owned();
        assert!(
            !text.contains("printf"),
            "the echoed command line must not leak into the range: {text:?}"
        );
    }

    #[test]
    fn meta_index_append_numbers_commands_from_zero() {
        let dir = tempdir().unwrap();
        let cast = cast(dir.path());
        let meta = meta(dir.path());
        CastWriter::create(&cast, 80, 24, 1_788_470_000)
            .unwrap()
            .byte_len();
        for expected in 0u32..2 {
            MetaIndex::append(
                &meta,
                &cast,
                "s7",
                1_788_470_000,
                CommandRecord {
                    index: 0,
                    command: format!("cmd-{expected}"),
                    started: 1_788_470_000.0,
                    ended: 1_788_470_001.0,
                    exit_code: 0,
                    first_byte: expected as u64 * 10,
                    end_byte: expected as u64 * 10 + 5,
                },
            )
            .expect("append succeeds");
            let loaded = MetaIndex::load(&meta).expect("the index round-trips");
            assert_eq!(loaded.commands.len(), (expected + 1) as usize);
            let last = loaded.commands.last().expect("just pushed one");
            assert_eq!(last.index, expected, "commands number from zero");
            assert_eq!(last.command, format!("cmd-{expected}"));
        }
        let loaded = MetaIndex::load(&meta).unwrap();
        assert_eq!(loaded.shell_id, "s7");
        assert_eq!(loaded.started_unix, 1_788_470_000);
        assert_eq!(loaded.commands[0].index, 0);
        assert_eq!(loaded.commands[1].index, 1);
    }
}
