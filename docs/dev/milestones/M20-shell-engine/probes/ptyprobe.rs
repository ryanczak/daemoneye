use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

fn read_until(r: &mut Box<dyn Read + Send>, needle: &str, max: Duration) -> (Vec<u8>, bool) {
    let mut buf = Vec::new();
    let start = Instant::now();
    let mut chunk = [0u8; 4096];
    while start.elapsed() < max {
        match r.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if String::from_utf8_lossy(&buf).contains(needle) {
                    return (buf, true);
                }
            }
            Err(_) => break,
        }
    }
    (buf, false)
}

/// Probe one shell: does the split-quote marker survive, and is the exit code right?
fn probe_shell(shell: &str, exit_var: &str) {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
        .unwrap();
    let mut cmd = CommandBuilder::new(shell);
    cmd.env("TERM", "dumb");
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    std::thread::sleep(Duration::from_millis(600)); // let the prompt settle

    let nonce = "a1b2c3d4e5f60718";
    // TYPED text carries the split; OUTPUT carries the joined marker.
    let typed = format!(
        "exit 3 2>/dev/null; false; printf '\\n\\x1fDE_''END {nonce} %s\\x1f\\n' {exit_var}\n"
    );
    let typed = format!("false; printf '\\n\\x1fDE_''END {nonce} %s\\x1f\\n' {exit_var}\n");
    let _ = typed;
    let line = format!("false; printf '\\n\\x1fDE_''END {nonce} %s\\x1f\\n' {exit_var}\n");
    writer.write_all(line.as_bytes()).unwrap();

    let needle = format!("DE_END {nonce} ");
    let (out, found) = read_until(&mut reader, &needle, Duration::from_secs(5));
    let s = String::from_utf8_lossy(&out);
    let code = s
        .split(&needle)
        .nth(1)
        .and_then(|t| t.split('\u{1f}').next())
        .map(|c| c.trim().to_string());
    // Did the ECHO of the typed line contain the joined marker? (must be NO)
    let echo_line = s.lines().next().unwrap_or("");
    let echo_has_joined = echo_line.contains(&format!("DE_END {nonce}"));
    println!(
        "{shell:<5} found={found:<5} exit_code={:<8} echo_contains_joined_marker={echo_has_joined}",
        format!("{code:?}")
    );
    let _ = writer.write_all(b"exit\n");
    let _ = child.wait();
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "utf8" {
        // Multi-byte char split across PTY reads?
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize { rows: 24, cols: 200, pixel_width: 0, pixel_height: 0 })
            .unwrap();
        let mut cmd = CommandBuilder::new("bash");
        cmd.env("TERM", "dumb");
        let mut child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut writer = pair.master.take_writer().unwrap();
        std::thread::sleep(Duration::from_millis(500));
        // 4000 multi-byte chars, guaranteed to cross a 4096-byte read boundary
        writer
            .write_all(b"for i in $(seq 1 400); do printf '\\u00e9\\u4e16\\u754c\\U0001F600'; done; printf '\\n\\x1fDE_''END z %s\\x1f\\n' $?\n")
            .unwrap();
        let (out, ok) = read_until(&mut reader, "DE_END z ", Duration::from_secs(5));
        let mut chunks_invalid = 0usize;
        // simulate byte-wise chunking at 4096 and check naive per-chunk from_utf8
        for c in out.chunks(4096) {
            if std::str::from_utf8(c).is_err() {
                chunks_invalid += 1;
            }
        }
        println!(
            "utf8: ok={ok} total_bytes={} chunks={} chunks_failing_naive_from_utf8={chunks_invalid}",
            out.len(),
            out.chunks(4096).count()
        );
        println!("whole-buffer from_utf8 valid: {}", std::str::from_utf8(&out).is_ok());
        let _ = writer.write_all(b"exit\n");
        let _ = child.wait();
        return;
    }
    probe_shell("bash", "$?");
    probe_shell("zsh", "$?");
    probe_shell("fish", "$status");
}
