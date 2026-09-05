use std::io::Write;
fn main() {
    let out = std::env::args().nth(1).unwrap();
    let pid = std::process::id();
    let pgid = unsafe { libc_getpgid() };
    let mut f = std::fs::File::create(&out).unwrap();
    writeln!(f, "child_pid={pid} child_pgid={pgid} ppid={}", std::process::id()).unwrap();
    f.flush().unwrap();
    for i in 0..25 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let mut f = std::fs::OpenOptions::new().append(true).open(&out).unwrap();
        writeln!(f, "beat {i} ppid_now={}", getppid()).unwrap();
    }
}
unsafe fn libc_getpgid() -> i32 { unsafe { getpgid_raw(0) } }
unsafe extern "C" { #[link_name="getpgid"] fn getpgid_raw(pid: i32) -> i32; }
fn getppid() -> i32 { unsafe { getppid_raw() } }
unsafe extern "C" { #[link_name="getppid"] fn getppid_raw() -> i32; }
