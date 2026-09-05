use std::process::{Command, Stdio};
fn main() {
    let child_bin = std::env::args().nth(1).unwrap();
    let out = std::env::args().nth(2).unwrap();
    println!("parent_pid={} parent_pgid={}", std::process::id(), unsafe { getpgid_raw(0) });
    let mut cmd = Command::new(&child_bin);
    cmd.arg(&out).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let ch = cmd.spawn().expect("spawn");
    println!("spawned_child_pid={}", ch.id());
}
unsafe extern "C" { #[link_name="getpgid"] fn getpgid_raw(pid: i32) -> i32; }
