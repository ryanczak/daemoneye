use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

fn read_until(reader: &mut Box<dyn Read + Send>, needle: &str, max: Duration) -> (Vec<u8>, bool) {
    let mut buf = Vec::new();
    let start = Instant::now();
    let mut chunk = [0u8; 4096];
    while start.elapsed() < max {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => { buf.extend_from_slice(&chunk[..n]);
                       if String::from_utf8_lossy(&buf).contains(needle) { return (buf, true); } }
            Err(e) => { eprintln!("read err: {e}"); break; }
        }
    }
    (buf, false)
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let pty = native_pty_system();
    let pair = pty.openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }).unwrap();
    let mut cmd = CommandBuilder::new(std::env::var("SHELL").unwrap_or("bash".into()));
    cmd.env("PS1", "$ "); cmd.env("TERM", "xterm-256color");
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let pid = child.process_id().unwrap();
    println!("shell pid={pid} (probe pid={})", std::process::id());
    let (_p, _) = read_until(&mut reader, "$ ", Duration::from_secs(3)); // wait for first prompt

    if mode == "orphan" {
        // Measure: what happens to the shell when the master holder exits?
        writer.write_all(b"sleep 300 & echo BG=$!\n").unwrap();
        let (out, ok) = read_until(&mut reader, "BG=", Duration::from_secs(3));
        println!("started bg sleep: ok={ok} out={:?}", String::from_utf8_lossy(&out));
        println!("exiting probe without killing child; check with: ps -o pid,ppid,stat,cmd -p {pid}");
        std::process::exit(0);
    }

    // 1) marker protocol with a real exit code
    let nonce = "9f3a1c";
    let line = format!("false; printf '\\n\\x1fDE_''END {nonce} %s\\x1f\\n' $?\n");
    writer.write_all(line.as_bytes()).unwrap();
    let (out, ok) = read_until(&mut reader, &format!("DE_END {nonce} "), Duration::from_secs(5));
    let s = String::from_utf8_lossy(&out);
    let code = s.split(&format!("DE_END {nonce} ")).nth(1).and_then(|t| t.split('\x1f').next()).map(|c| c.trim().to_string());
    println!("marker ok={ok} exit_code={code:?}");

    // 2) vt100 screen of a coloured multi-line command
    let mut parser = vt100::Parser::new(24, 80, 1000);
    parser.process(&out);
    writer.write_all(b"printf '\\e[31mERR line\\e[0m\\nplain line\\n'; printf '\\x1fDE_''END z %s\\x1f\\n' $?\n").unwrap();
    let (out2, ok2) = read_until(&mut reader, "DE_END z ", Duration::from_secs(5));
    parser.process(&out2);
    let screen = parser.screen();
    let rows: Vec<String> = (0..24).map(|r| screen.contents_between(r, 0, r, 80)).filter(|l| !l.trim().is_empty()).collect();
    println!("vt100 ok={ok2} rows={}", rows.len());
    for r in &rows { println!("  |{r}"); }
    let red = (0..24).flat_map(|r| (0..80).map(move |c| (r,c))).filter(|&(r,c)| screen.cell(r,c).map(|x| x.fgcolor()==vt100::Color::Idx(1) && x.has_contents()).unwrap_or(false)).count();
    println!("red cells={red}");

    // 3) resize + interactive program (less) enters alt screen?
    pair.master.resize(PtySize { rows: 10, cols: 40, pixel_width: 0, pixel_height: 0 }).unwrap();
    writer.write_all(b"stty size; printf '\\x1fDE_''END r %s\\x1f\\n' $?\n").unwrap();
    let (out3, _) = read_until(&mut reader, "DE_END r ", Duration::from_secs(5));
    println!("after resize stty size -> {:?}", String::from_utf8_lossy(&out3).lines().find(|l| l.trim().starts_with("10 ")));
    writer.write_all(b"printf 'a\\nb\\n' | less\n").unwrap();
    let (out4, _) = read_until(&mut reader, "NEVER", Duration::from_millis(800));
    parser.screen_mut().set_size(10, 40); parser.process(&out4);
    println!("less: alternate_screen={} bytes={}", parser.screen().alternate_screen(), out4.len());
    writer.write_all(b"q").unwrap();
    let _ = read_until(&mut reader, "NEVER", Duration::from_millis(300));

    // 4) exit
    writer.write_all(b"exit 7\n").unwrap();
    let st = child.wait().unwrap();
    println!("shell exited: success={} code={:?}", st.success(), st.exit_code());
}
