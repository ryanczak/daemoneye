//! The `user@host:sN` label the terminal approval panel is titled with: the
//! user the shell runs as, the host it runs on, and the shell the approved
//! run is pinned to. The pieces are pure except [`local_username`], whose
//! passwd lookup is the daemon's own user.

/// Split an ssh destination into `(user, host)`: the last `@` separates
/// them, a leading `ssh://` scheme and a trailing `:port` are stripped. A
/// target with no `@` has no user — `db01` is host `db01`, user `None`.
pub fn ssh_user_host(target: &str) -> (Option<String>, String) {
    let stripped = target.strip_prefix("ssh://").unwrap_or(target);
    let (user, rest) = match stripped.rsplit_once('@') {
        Some((u, h)) => (Some(u.to_string()), h),
        None => (None, stripped),
    };
    // A port colon lives after the last `@`, so the host half is what can
    // carry one; a bare user@ with nothing after it keeps `None` as the host.
    let (host, _port) = rest.split_once(':').unwrap_or((rest, ""));
    let host = if host.is_empty() {
        rest.to_string()
    } else {
        host.to_string()
    };
    (user, host)
}

/// The passwd name for the daemon's effective uid, through `libc`'s
/// `getpwuid_r`. Falls back to `$USER`, then to the numeric uid as a string.
pub fn local_username() -> String {
    let uid = unsafe { libc::geteuid() };
    if let Some(name) = passwd_name(uid) {
        return name;
    }
    if let Ok(user) = std::env::var("USER")
        && !user.trim().is_empty()
    {
        return user;
    }
    uid.to_string()
}

/// The passwd name for `uid` through the reentrant `getpwuid_r`: the daemon
/// is multi-threaded, and `getpwuid`'s static buffer is not thread-safe.
/// `None` when there is no entry, the buffer is too small, or the name is
/// empty or not UTF-8.
fn passwd_name(uid: libc::uid_t) -> Option<String> {
    // sysconf gives the glibc-recommended start; it is -1 when it does not
    // know, so fall back to the same 16 KiB the fixed buffer used.
    let initial = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let initial = if initial > 0 {
        initial as usize
    } else {
        16 * 1024
    };
    passwd_name_from(uid, initial)
}

fn passwd_name_from(uid: libc::uid_t, initial_len: usize) -> Option<String> {
    let mut len = initial_len.max(1);
    loop {
        if let Some(name) = passwd_name_with_buffer(uid, len) {
            return Some(name);
        }
        if len >= 1024 * 1024 {
            return None;
        }
        len = (len * 2).min(1024 * 1024);
    }
}

fn passwd_name_with_buffer(uid: libc::uid_t, len: usize) -> Option<String> {
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0u8; len];
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: every pointer is to a live local; `getpwuid_r` writes the
    // entry's strings into `buf`, which outlives the `CStr` read below, and
    // `result` is checked before `pw.pw_name` is read.
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pw,
            buf.as_mut_ptr().cast::<libc::c_char>(),
            buf.len(),
            &mut result,
        )
    };
    if rc == libc::ERANGE {
        return None;
    }
    if rc != 0 || result.is_null() || pw.pw_name.is_null() {
        return None;
    }
    let name = unsafe { std::ffi::CStr::from_ptr(pw.pw_name) };
    let name = name.to_str().ok()?;
    (!name.is_empty()).then(|| name.to_string())
}

/// The panel title from the label pieces: `user@host:sN`, `host:sN` without
/// a user, and `…:new` when no shell was picked.
pub fn shell_where_label(user: Option<&str>, host: &str, shell: Option<&str>) -> String {
    let who = match user {
        Some(u) if !u.is_empty() => format!("{u}@{host}"),
        _ => host.to_string(),
    };
    format!("{who}:{}", shell.unwrap_or("new"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_user_host_splits_the_target() {
        let (user, host) = ssh_user_host("matt@web01.lan");
        assert_eq!(user.as_deref(), Some("matt"), "a plain user@host splits");
        assert_eq!(host, "web01.lan");

        let (user, host) = ssh_user_host("db01");
        assert_eq!(user, None, "a bare name has no user: no `@`");
        assert_eq!(host, "db01");

        let (user, host) = ssh_user_host("ssh://matt@web01.lan:2222");
        assert_eq!(user.as_deref(), Some("matt"), "the scheme is stripped");
        assert_eq!(host, "web01.lan", "neither the port nor the scheme appears");
    }

    #[test]
    fn ssh_user_host_takes_the_last_at_sign() {
        // An user part that itself carries an `@` shape is still split at
        // the last one: the host is the tail.
        let (user, host) = ssh_user_host("matt+tag@web01.lan");
        assert_eq!(user.as_deref(), Some("matt+tag"));
        assert_eq!(host, "web01.lan");
    }

    #[test]
    fn ssh_user_host_keeps_a_bare_host_with_a_port() {
        let (user, host) = ssh_user_host("ssh://web01.lan:2222");
        assert_eq!(user, None);
        assert_eq!(host, "web01.lan", "a scheme-less user is still None");
    }

    #[test]
    fn shell_where_label_formats_each_case() {
        assert_eq!(
            shell_where_label(Some("matt"), "pinky", Some("s3")),
            "matt@pinky:s3"
        );
        assert_eq!(
            shell_where_label(None, "db01", Some("s4")),
            "db01:s4",
            "no user, no `@`"
        );
        assert_eq!(
            shell_where_label(Some("matt"), "pinky", None),
            "matt@pinky:new",
            "no shell picked reads `new`"
        );
    }

    #[test]
    fn a_tiny_passwd_buffer_still_finds_the_name() {
        let uid = unsafe { libc::geteuid() };
        let tiny = passwd_name_from(uid, 8);
        let expected = passwd_name(uid);
        assert_eq!(
            tiny, expected,
            "an 8-byte initial buffer retried to the same name as the default"
        );
        assert!(
            tiny.is_some(),
            "the test process's own euid has a passwd name"
        );
    }

    #[test]
    fn local_username_is_a_plain_name() {
        let name = local_username();
        assert!(!name.is_empty(), "a username is never empty: {name:?}");
        assert!(
            !name.contains('@') && !name.contains(':') && !name.chars().any(char::is_whitespace),
            "the username is a plain name, got: {name:?}"
        );
    }
}
