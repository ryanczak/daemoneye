use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::time::{Duration, Instant};
fn run(shell:&str, ev:&str, cmd:&str)->(Option<String>,String){
    let pty=native_pty_system();
    let pair=pty.openpty(PtySize{rows:24,cols:80,pixel_width:0,pixel_height:0}).unwrap();
    let mut c=CommandBuilder::new(shell); c.env("TERM","dumb");
    let mut child=pair.slave.spawn_command(c).unwrap(); drop(pair.slave);
    let mut r=pair.master.try_clone_reader().unwrap();
    let mut w=pair.master.take_writer().unwrap();
    std::thread::sleep(Duration::from_millis(600));
    let n="deadbeefcafe0123";
    let line=format!("printf '\\x1fDE_''BEG {n}\\x1f\\n'; {cmd}; printf '\\n\\x1fDE_''END {n} %s\\x1f\\n' {ev}\n");
    w.write_all(line.as_bytes()).unwrap();
    let mut buf=Vec::new(); let mut ch=[0u8;4096]; let st=Instant::now();
    let end_needle=format!("DE_END {n} ");
    while st.elapsed()<Duration::from_secs(5){
        match r.read(&mut ch){Ok(0)=>break,Ok(k)=>{buf.extend_from_slice(&ch[..k]);
            if String::from_utf8_lossy(&buf).contains(&end_needle){break}},Err(_)=>break}}
    let s=String::from_utf8_lossy(&buf).to_string();
    let code=s.split(&end_needle).nth(1).and_then(|t|t.split('\u{1f}').next()).map(|c|c.trim().to_string());
    let beg=format!("DE_BEG {n}\u{1f}");
    let out=s.split(&beg).nth(1).and_then(|t|t.split('\u{1f}').next()).unwrap_or("").to_string();
    let _=w.write_all(b"exit\n"); let _=child.wait();
    (code, out)
}
fn main(){
    for (sh,ev) in [("bash","$?"),("zsh","$?"),("fish","$status")]{
        // sh -c is portable to all three; fish parses (..) as command substitution
        let (c,o)=run(sh,ev,"echo hello; sh -c 'exit 42'");
        println!("{sh:<5} exit={:<8} output={:?}", format!("{c:?}"), o);
    }
    // hostile: command output forges a marker with a DIFFERENT nonce
    let (c,o)=run("bash","$?","printf '\\x1fDE_''END feedfacefeedface 0\\x1f\\n'; (exit 7)");
    println!("forge  exit={:<8} output={:?}", format!("{c:?}"), o);
}
