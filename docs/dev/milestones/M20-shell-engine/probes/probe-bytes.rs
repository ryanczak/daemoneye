use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::time::{Duration, Instant};
fn main() {
    let pty = native_pty_system();
    let pair = pty.openpty(PtySize{rows:24,cols:120,pixel_width:0,pixel_height:0}).unwrap();
    let mut cmd = CommandBuilder::new("bash");
    cmd.env("TERM","dumb");
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    writer.write_all(b"printf 'no-trailing-newline'; (exit 42); printf '\\n\\x1fDE_''END n0nc3 %s\\x1f\\n' $?\n").unwrap();
    let mut buf=Vec::new(); let mut c=[0u8;4096]; let st=Instant::now();
    while st.elapsed()<Duration::from_secs(4) {
        match reader.read(&mut c){Ok(0)=>break,Ok(n)=>{buf.extend_from_slice(&c[..n]);
            if String::from_utf8_lossy(&buf).contains("DE_END n0nc3 "){break}},Err(_)=>break}
    }
    println!("RAW BYTES (escaped):\n{}", buf.iter().map(|b| match b {
        0x1f=>"\\x1f".to_string(), b'\r'=>"\\r".to_string(), b'\n'=>"\\n".to_string(),
        0x20..=0x7e=>(*b as char).to_string(), _=>format!("\\x{:02x}",b)}).collect::<String>());
    let _=writer.write_all(b"exit\n"); let _=child.wait();
}
