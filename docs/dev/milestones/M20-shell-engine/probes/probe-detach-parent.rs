use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
fn main() {
    let child_bin = std::env::args().nth(1).unwrap();
    let out = std::env::args().nth(2).unwrap();
    let mypid = std::process::id();
    let mypgid = unsafe { getpgid_raw(0) };
    let mut cmd = Command::new(&child_bin);
    cmd.arg(&out)
       .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
       .process_group(0);
    let ch = cmd.spawn().expect("spawn");
    println!("parent_pid={mypid} parent_pgid={mypgid} spawned_child_pid={}", ch.id());
    // parent exits immediately WITHOUT waiting
}
unsafe extern "C" { #[link_name="getpgid"] fn getpgid_raw(pid: i32) -> i32; }
