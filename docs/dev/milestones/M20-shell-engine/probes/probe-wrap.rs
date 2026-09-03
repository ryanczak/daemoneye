use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::time::{Duration, Instant};
fn main() {
    let pty = native_pty_system();
    let pair = pty.openpty(PtySize{rows:24,cols:80,pixel_width:0,pixel_height:0}).unwrap();
    let mut cmd = CommandBuilder::new("bash"); cmd.env("TERM","dumb");
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    // A command whose echo far exceeds 80 cols, with a BEGIN marker too.
    let filler = "x".repeat(180);
    let line = format!("printf '\\x1fDE_''BEG n0nc3\\x1f\\n'; echo '{filler}' | cut -c1-10; printf '\\n\\x1fDE_''END n0nc3 %s\\x1f\\n' $?\n");
    writer.write_all(line.as_bytes()).unwrap();
    let mut buf=Vec::new(); let mut c=[0u8;4096]; let st=Instant::now();
    while st.elapsed()<Duration::from_secs(4) {
        match reader.read(&mut c){Ok(0)=>break,Ok(n)=>{buf.extend_from_slice(&c[..n]);
            if String::from_utf8_lossy(&buf).contains("DE_END n0nc3 "){break}},Err(_)=>break}
    }
    let s=String::from_utf8_lossy(&buf);
    println!("echo line count before BEG marker: {}", s.split("\u{1f}DE_BEG").next().unwrap_or("").matches("\r\n").count());
    // extract strictly between markers
    let between = s.split("DE_BEG n0nc3\u{1f}").nth(1).and_then(|t| t.split('\u{1f}').next()).unwrap_or("");
    println!("BETWEEN MARKERS (escaped): {:?}", between);
    let _=writer.write_all(b"exit\n"); let _=child.wait();
}
