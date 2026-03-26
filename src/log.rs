/// Simple, lightweight logging macros for the daemoneye daemon.
/// These write to stdout/stderr, which is redirected to `daemon.log`
/// by `main.rs` when running in the background.
#[macro_export]
macro_rules! log_event {
    ($($arg:tt)*) => {{
        let now = chrono::Local::now();
        println!("[{}] [EVENT] {}", now.format("%Y-%m-%d %H:%M:%S"), format_args!($($arg)*));
    }};
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {{
        let now = chrono::Local::now();
        eprintln!("[{}] [WARNING] {}", now.format("%Y-%m-%d %H:%M:%S"), format_args!($($arg)*));
    }};
}

#[macro_export]
macro_rules! log_fatal {
    ($($arg:tt)*) => {{
        let now = chrono::Local::now();
        eprintln!("\x1b[31m[{}] [FATAL] {}\x1b[0m", now.format("%Y-%m-%d %H:%M:%S"), format_args!($($arg)*));
    }};
}
