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
use std::path::{Path, PathBuf};
use std::time::Duration;

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
        let mut file = File::create(path)?;
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
        let mut file = File::create(path)?;
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
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(array) = value.as_array() else {
            continue;
        };
        if array.len() != 3 {
            continue;
        }
        if array[1].as_str() != Some("o") {
            continue;
        }
        let Some(data) = array[2].as_str() else {
            continue;
        };
        out.extend_from_slice(data.as_bytes());
    }
    Ok(out)
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
}
