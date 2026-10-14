//! The key vocabulary: turning tmux-style key names into the bytes a
//! terminal sends, and the paste-bracket rule.
//!
//! Pure and shared: attached mode (`attach_keys`) and the `send_input` tool
//! encode from here, so what the user types by hand and what the model sends
//! agree byte for byte.

use anyhow::anyhow;

/// The shell's terminal modes that change what a key should send.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShellModes {
    /// The shell asked for bracketed paste (`ESC [ ? 2004 h`).
    pub bracketed_paste: bool,
    /// The shell asked for application cursor keys (`ESC [ ? 1 h`).
    pub application_cursor: bool,
}

impl ShellModes {
    pub fn of(screen: &vt100::Screen) -> Self {
        Self {
            bracketed_paste: screen.bracketed_paste(),
            application_cursor: screen.application_cursor(),
        }
    }
}

/// What [`encode_keys`] produced: the bytes, and the key names that went out
/// as CSI u — the form a program may not honour, so the caller's result can
/// say which keys need a screen read to confirm.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EncodedKeys {
    pub bytes: Vec<u8>,
    pub csi_u: Vec<String>,
}

/// The key a name (after its prefixes) denotes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Enter,
    Escape,
    Tab,
    BTab,
    Up,
    Down,
    Right,
    Left,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    BSpace,
    Space,
    F(u8),
    Char(char),
}

/// The vocabulary a bare (un-prefixed) name may name, lower-case.
const NAMED: &[(&str, Key)] = &[
    ("enter", Key::Enter),
    ("escape", Key::Escape),
    ("tab", Key::Tab),
    ("btab", Key::BTab),
    ("up", Key::Up),
    ("down", Key::Down),
    ("right", Key::Right),
    ("left", Key::Left),
    ("home", Key::Home),
    ("end", Key::End),
    ("pageup", Key::PageUp),
    ("pagedown", Key::PageDown),
    ("delete", Key::Delete),
    ("bspace", Key::BSpace),
    ("space", Key::Space),
    ("f1", Key::F(1)),
    ("f2", Key::F(2)),
    ("f3", Key::F(3)),
    ("f4", Key::F(4)),
    ("f5", Key::F(5)),
    ("f6", Key::F(6)),
    ("f7", Key::F(7)),
    ("f8", Key::F(8)),
    ("f9", Key::F(9)),
    ("f10", Key::F(10)),
    ("f11", Key::F(11)),
    ("f12", Key::F(12)),
];

impl Key {
    /// Final byte of the cursor/navigation group xterm gives a parameter
    /// form (`\x1b[1;<m><final>`): arrows and `Home`/`End`.
    fn cursor(self) -> Option<u8> {
        match self {
            Key::Up => Some(b'A'),
            Key::Down => Some(b'B'),
            Key::Right => Some(b'C'),
            Key::Left => Some(b'D'),
            Key::Home => Some(b'H'),
            Key::End => Some(b'F'),
            _ => None,
        }
    }

    /// The `~`-terminated keys with their xterm parameter `n`.
    fn tilde(self) -> Option<u8> {
        match self {
            Key::Delete => Some(3),
            Key::PageUp => Some(5),
            Key::PageDown => Some(6),
            Key::F(n) => match n {
                5 => Some(15),
                6 => Some(17),
                7 => Some(18),
                8 => Some(19),
                9 => Some(20),
                10 => Some(21),
                11 => Some(23),
                12 => Some(24),
                _ => None,
            },
            _ => None,
        }
    }

    /// `F1`–`F4`, the `\x1b[1;<m>P`–`S` group.
    fn pfkey(self) -> Option<u8> {
        match self {
            Key::F(n) if (1..=4).contains(&n) => Some(b'P' + (n - 1)),
            _ => None,
        }
    }

    /// Rule 5's code point for CSI u.
    fn codepoint(self) -> Option<u32> {
        match self {
            Key::Enter => Some(13),
            Key::Escape => Some(27),
            Key::Tab | Key::BTab => Some(9),
            Key::BSpace => Some(127),
            Key::Space => Some(32),
            Key::Char(c) => Some(u32::from(c.to_ascii_lowercase())),
            _ => None,
        }
    }
}

/// The error every unknown name gets: the whole vocabulary, so the model can
/// pick again without a second round trip.
fn unknown_key(name: &str) -> anyhow::Error {
    anyhow!(
        "unknown key {name:?}: name one of Enter, Escape, Tab, BTab, Up, Down, \
         Right, Left, Home, End, PageUp, PageDown, Delete, BSpace, Space, F1 \
         through F12, or one printable character, with the C-, M- and S- \
         prefixes in any order"
    )
}

/// One parsed key name: the three prefixes, the key they qualify, and the
/// original name (what `csi_u` reports).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Parsed {
    name: String,
    ctrl: bool,
    meta: bool,
    shift: bool,
    key: Key,
}

/// Parse one key name: the `C-`/`M-`/`S-` prefixes in any order (each at
/// most once), then a named key (case-insensitive) or one printable ASCII
/// character.
fn parse_key(name: &str) -> anyhow::Result<Parsed> {
    let (ctrl, meta, shift, rest) = prefixes(name)?;
    // With a prefix, a single character is that character: `C-Space` is the
    // space *character* under the control rule (0x00), not the named
    // plain-table key, and `S-1` is CSI u's rule 5, not an error.
    let key = match named_key(rest) {
        Some(key) if (ctrl || meta || shift) && key == Key::Space => {
            // The space *character* under a prefix takes the control rule
            // (`C-Space` = 0x00); the named plain-table key is for a bare
            // `Space`.
            Key::Char(' ')
        }
        Some(key) => key,
        None => match single_char(rest) {
            Some(c) if c.is_ascii_graphic() || c == ' ' => Key::Char(c),
            _ => return Err(unknown_key(name)),
        },
    };
    Ok(Parsed {
        name: name.to_string(),
        ctrl,
        meta,
        shift,
        key,
    })
}

/// The one character `rest` is, when it is exactly one character.
fn single_char(rest: &str) -> Option<char> {
    let mut chars = rest.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// Consume the `C-`/`M-`/`S-` prefixes, each at most once, in any order.
fn prefixes(name: &str) -> anyhow::Result<(bool, bool, bool, &str)> {
    let mut ctrl = false;
    let mut meta = false;
    let mut shift = false;
    let mut rest = name;
    loop {
        if let Some(r) = rest.strip_prefix("C-") {
            if ctrl {
                return Err(unknown_key(name));
            }
            ctrl = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("M-") {
            if meta {
                return Err(unknown_key(name));
            }
            meta = true;
            rest = r;
        } else if let Some(r) = rest.strip_prefix("S-") {
            if shift {
                return Err(unknown_key(name));
            }
            shift = true;
            rest = r;
        } else {
            break;
        }
    }
    if rest.is_empty() {
        return Err(unknown_key(name));
    }
    Ok((ctrl, meta, shift, rest))
}

fn named_key(rest: &str) -> Option<Key> {
    let lower = rest.to_ascii_lowercase();
    NAMED
        .iter()
        .find(|(name, _)| *name == lower)
        .map(|(_, key)| *key)
}

/// The xterm modifier parameter: `1 + (S) + (M<<1) + (C<<2)`.
fn modifier_param(p: &Parsed) -> u8 {
    1 + u8::from(p.shift) + 2 * u8::from(p.meta) + 4 * u8::from(p.ctrl)
}

/// Rules 1–2: the legacy byte for `C-` alone on a control-able character or
/// `S-` alone on a letter; `None` when no such byte exists.
fn legacy_byte(p: &Parsed) -> Option<u8> {
    let Key::Char(c) = p.key else {
        return None;
    };
    if p.meta {
        return None;
    }
    if p.ctrl {
        if p.shift {
            return None;
        }
        // `C-Space`: the space *character* maps through, but the named key
        // `Space` stays in the plain table (`C-Space` = 0x00, the NUL byte).
        return matches!(
            c.to_ascii_uppercase(),
            'A'..='Z' | '@' | '[' | '\\' | ']' | '^' | '_' | ' '
        )
        .then(|| c.to_ascii_uppercase() as u8 & 0x1f);
    }
    if p.shift {
        return c
            .is_ascii_alphabetic()
            .then(|| c.to_ascii_uppercase() as u8);
    }
    None
}

/// The plain table: one key, no prefixes. Application cursor mode moves the
/// arrows and `Home`/`End` from the `CSI` forms to the `SS3` ones; nothing
/// else in the table changes with the modes.
fn plain_bytes(key: Key, modes: ShellModes) -> Vec<u8> {
    match key {
        Key::Enter => b"\r".to_vec(),
        Key::Escape => b"\x1b".to_vec(),
        Key::Tab => b"\t".to_vec(),
        Key::BTab => b"\x1b[Z".to_vec(),
        Key::BSpace => vec![0x7f],
        Key::Space => b" ".to_vec(),
        Key::Up | Key::Down | Key::Right | Key::Left | Key::Home | Key::End => {
            let last = key.cursor().unwrap_or(b'?');
            if modes.application_cursor {
                vec![0x1b, b'O', last]
            } else {
                vec![0x1b, b'[', last]
            }
        }
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::F(n) if (1..=4).contains(&n) => vec![0x1b, b'O', b'P' + (n - 1)],
        Key::F(_) => format!("\x1b[{}~", key.tilde().unwrap_or(0)).into_bytes(),
        Key::Char(c) => c.to_string().into_bytes(),
    }
}

/// Encode one parsed key, appending to `out`; a key that went out as CSI u
/// is named in `csi_u`.
///
/// The modifier rules in order: the legacy `C-`/`S-` bytes, `S-Tab` as
/// `BTab`, xterm's parameter form for the cursor/`~`/`F1`–`F4` groups,
/// `ESC` before the bare key's bytes for `M-`, and CSI u for everything
/// else.
fn encode_one(p: &Parsed, modes: ShellModes, out: &mut Vec<u8>, csi_u: &mut Vec<String>) {
    if let Some(b) = legacy_byte(p) {
        out.push(b);
        return;
    }
    if p.shift && !p.ctrl && !p.meta && p.key == Key::Tab {
        out.extend_from_slice(b"\x1b[Z");
        return;
    }
    // Rule 3: with any modifier these take xterm's parameter form, and
    // application cursor mode does not apply to them.
    if p.meta || p.ctrl || p.shift {
        if let Some(last) = p.key.cursor() {
            out.extend_from_slice(
                format!("\x1b[1;{}{}", modifier_param(p), last as char).as_bytes(),
            );
            return;
        }
        if let Some(n) = p.key.tilde() {
            out.extend_from_slice(format!("\x1b[{};{}~", n, modifier_param(p)).as_bytes());
            return;
        }
        if let Some(last) = p.key.pfkey() {
            out.extend_from_slice(
                format!("\x1b[1;{}{}", modifier_param(p), last as char).as_bytes(),
            );
            return;
        }
        // Rule 4: `M-` where the bare key has legacy/plain bytes → `ESC` +
        // those bytes.
        if p.meta {
            let bare = Parsed {
                meta: false,
                ..p.clone()
            };
            if let Some(bare_bytes) = bare_legacy_bytes(&bare, modes) {
                out.push(0x1b);
                out.extend_from_slice(&bare_bytes);
                return;
            }
        }
        // Rule 5: everything else.
        if let Some(cp) = p.key.codepoint() {
            out.extend_from_slice(format!("\x1b[{};{}u", cp, modifier_param(p)).as_bytes());
            csi_u.push(p.name.clone());
        }
        return;
    }
    out.extend_from_slice(&plain_bytes(p.key, modes));
}

/// Rule 4's condition: the bytes `key` sends with no `M-` and no other
/// modifier than those `bare` carries, when those bytes are a legacy or
/// plain form (`None` when the bare key would go out as CSI u).
fn bare_legacy_bytes(bare: &Parsed, modes: ShellModes) -> Option<Vec<u8>> {
    if let Some(b) = legacy_byte(bare) {
        return Some(vec![b]);
    }
    if bare.shift && !bare.ctrl && bare.key == Key::Tab {
        return Some(b"\x1b[Z".to_vec());
    }
    if !bare.ctrl && !bare.shift {
        return Some(plain_bytes(bare.key, modes));
    }
    None
}

/// Encode tmux-style key names into the bytes a terminal sends.
///
/// Any bad name fails the whole call: no partial bytes. `csi_u` names, in
/// order, the keys that went out as CSI u — the form a program may not
/// honour, so the caller can say which keys need a screen read to confirm.
pub fn encode_keys(names: &[String], modes: ShellModes) -> anyhow::Result<EncodedKeys> {
    let parsed = names
        .iter()
        .map(|name| parse_key(name))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut bytes = Vec::new();
    let mut csi_u = Vec::new();
    for p in &parsed {
        encode_one(p, modes, &mut bytes, &mut csi_u);
    }
    Ok(EncodedKeys { bytes, csi_u })
}

const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

/// The paste rule for `text` against a shell with `modes`: the bytes to send
/// and whether they are bracketed.
///
/// `paste: None` brackets iff `text` contains a newline **and** the shell
/// asked for bracketed paste — the same rule `attach_keys` applies. `Some`
/// overrides: `Some(true)` with the mode off is an error naming the mode,
/// not a silent downgrade, and `Some(false)` never brackets. Before
/// wrapping, every `ESC [ 201 ~` in `text` is removed, or the text could end
/// the paste early and run the rest as typed keys.
pub fn paste_bytes(
    text: &str,
    paste: Option<bool>,
    modes: ShellModes,
) -> anyhow::Result<(Vec<u8>, bool)> {
    let brackets = match paste {
        None => text.contains('\n') && modes.bracketed_paste,
        Some(true) if modes.bracketed_paste => true,
        Some(true) => {
            return Err(anyhow!(
                "paste: true needs the shell's bracketed paste mode on, and it \
                 is off; send the text without paste, or turn the mode on in \
                 the shell first"
            ));
        }
        Some(false) => false,
    };
    let cleaned = text.replace(PASTE_END, "");
    if !brackets {
        return Ok((cleaned.into_bytes(), false));
    }
    Ok((
        format!("{PASTE_START}{cleaned}{PASTE_END}").into_bytes(),
        true,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(names: &[&str], modes: ShellModes) -> EncodedKeys {
        let owned: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        encode_keys(&owned, modes).expect("valid vocabulary")
    }

    fn bytes_of(names: &[&str], modes: ShellModes) -> Vec<u8> {
        encoded(names, modes).bytes
    }

    #[test]
    fn keys_encode_the_plain_vocabulary() {
        let plain = ShellModes::default();
        let cases: &[(&str, &[u8])] = &[
            ("Enter", b"\r"),
            ("Escape", b"\x1b"),
            ("Tab", b"\t"),
            ("BTab", b"\x1b[Z"),
            ("S-Tab", b"\x1b[Z"),
            ("BSpace", b"\x7f"),
            ("Space", b" "),
            ("Up", b"\x1b[A"),
            ("Down", b"\x1b[B"),
            ("Right", b"\x1b[C"),
            ("Left", b"\x1b[D"),
            ("Home", b"\x1b[H"),
            ("End", b"\x1b[F"),
            ("PageUp", b"\x1b[5~"),
            ("PageDown", b"\x1b[6~"),
            ("Delete", b"\x1b[3~"),
            ("F1", b"\x1bOP"),
            ("F2", b"\x1bOQ"),
            ("F3", b"\x1bOR"),
            ("F4", b"\x1bOS"),
            ("F5", b"\x1b[15~"),
            ("F6", b"\x1b[17~"),
            ("F7", b"\x1b[18~"),
            ("F8", b"\x1b[19~"),
            ("F9", b"\x1b[20~"),
            ("F10", b"\x1b[21~"),
            ("F11", b"\x1b[23~"),
            ("F12", b"\x1b[24~"),
            ("a", b"a"),
            ("0", b"0"),
            ("enter", b"\r"),
            ("ENTER", b"\r"),
        ];
        for (name, want) in cases {
            assert_eq!(
                bytes_of(&[name], plain),
                want.to_vec(),
                "{name} must encode as its plain-table bytes"
            );
            assert!(
                encoded(&[name], plain).csi_u.is_empty(),
                "{name} is a legacy form, not CSI u"
            );
        }
    }

    #[test]
    fn keys_cursor_keys_follow_application_cursor_mode() {
        let app = ShellModes {
            application_cursor: true,
            ..ShellModes::default()
        };
        assert_eq!(bytes_of(&["Up"], app), b"\x1bOA".to_vec());
        assert_eq!(bytes_of(&["Down"], app), b"\x1bOB".to_vec());
        assert_eq!(bytes_of(&["Right"], app), b"\x1bOC".to_vec());
        assert_eq!(bytes_of(&["Left"], app), b"\x1bOD".to_vec());
        assert_eq!(bytes_of(&["Home"], app), b"\x1bOH".to_vec());
        assert_eq!(bytes_of(&["End"], app), b"\x1bOF".to_vec());
        assert_eq!(
            bytes_of(&["PageUp"], app),
            b"\x1b[5~".to_vec(),
            "PageUp has no application-cursor form"
        );
        assert_eq!(
            bytes_of(&["F5"], app),
            b"\x1b[15~".to_vec(),
            "F5 has no application-cursor form"
        );
    }

    #[test]
    fn keys_control_and_meta_use_legacy_bytes() {
        let plain = ShellModes::default();
        assert_eq!(bytes_of(&["C-c"], plain), vec![0x03]);
        assert_eq!(bytes_of(&["C-A"], plain), vec![0x01]);
        assert_eq!(bytes_of(&["C-Space"], plain), vec![0x00]);
        assert_eq!(bytes_of(&["C-["], plain), vec![0x1b]);
        assert_eq!(bytes_of(&["S-a"], plain), b"A".to_vec());
        assert_eq!(bytes_of(&["M-b"], plain), b"\x1bb".to_vec());
        assert_eq!(bytes_of(&["M-Enter"], plain), b"\x1b\r".to_vec());
        assert_eq!(bytes_of(&["M-C-x"], plain), b"\x1b\x18".to_vec());
        assert_eq!(bytes_of(&["S-Tab"], plain), b"\x1b[Z".to_vec());
        let encoded = encoded(&["C-c", "M-b", "S-a"], plain);
        assert!(
            encoded.csi_u.is_empty(),
            "legacy bytes are never reported as CSI u: {:?}",
            encoded.csi_u
        );
    }

    #[test]
    fn keys_modified_cursor_and_function_keys_use_xterm_parameters() {
        let plain = ShellModes::default();
        let app = ShellModes {
            application_cursor: true,
            ..ShellModes::default()
        };
        assert_eq!(bytes_of(&["C-Up"], plain), b"\x1b[1;5A".to_vec());
        assert_eq!(bytes_of(&["S-F5"], plain), b"\x1b[15;2~".to_vec());
        assert_eq!(bytes_of(&["M-F1"], plain), b"\x1b[1;3P".to_vec());
        assert_eq!(bytes_of(&["C-S-Home"], plain), b"\x1b[1;6H".to_vec());
        assert_eq!(
            bytes_of(&["C-Up"], app),
            b"\x1b[1;5A".to_vec(),
            "the parameter form ignores application cursor mode"
        );
        let encoded = encoded(&["C-Up", "S-F5", "M-F1", "C-S-Home"], plain);
        assert!(
            encoded.csi_u.is_empty(),
            "xterm parameter forms are not CSI u: {:?}",
            encoded.csi_u
        );
    }

    #[test]
    fn keys_without_a_legacy_byte_go_out_as_csi_u() {
        let plain = ShellModes::default();
        let cases: &[(&str, &[u8])] = &[
            ("S-Enter", b"\x1b[13;2u"),
            ("C-Enter", b"\x1b[13;5u"),
            ("C-S-p", b"\x1b[112;6u"),
            ("C-Tab", b"\x1b[9;5u"),
            ("C-1", b"\x1b[49;5u"),
        ];
        for (name, want) in cases {
            let e = encoded(&[name], plain);
            assert_eq!(e.bytes, want.to_vec(), "{name} goes out as CSI u");
            assert_eq!(e.csi_u, [name.to_string()], "{name} is named in csi_u");
        }
        let mixed = encoded(&["Up", "S-Enter"], plain);
        assert_eq!(
            mixed.bytes,
            b"\x1b[A\x1b[13;2u".to_vec(),
            "the CSI u key follows the plain one, in order"
        );
        assert_eq!(mixed.csi_u, ["S-Enter".to_string()]);
    }

    #[test]
    fn keys_unknown_name_is_an_error_naming_the_vocabulary() {
        let plain = ShellModes::default();
        for bad in ["Ctrl-c", "F13", "C-", "X-a", "", "PgUp", "RETURN"] {
            let owned = vec![bad.to_string()];
            let err = encode_keys(&owned, plain).expect_err(bad);
            assert!(
                format!("{err}").contains("BTab"),
                "{bad:?}: the error must list the vocabulary: {err}"
            );
        }
        let owned = vec!["Up".to_string(), "Bogus".to_string()];
        let err = encode_keys(&owned, plain).expect_err("Bogus");
        assert!(
            format!("{err}").contains("Bogus"),
            "the bad name is named: {err}"
        );
    }

    #[test]
    fn paste_brackets_only_when_the_shell_asked() {
        let off = ShellModes::default();
        let on = ShellModes {
            bracketed_paste: true,
            ..ShellModes::default()
        };
        let (bytes, bracketed) = paste_bytes("one line", None, on).expect("no newline, no paste");
        assert_eq!(bytes, b"one line".to_vec());
        assert!(!bracketed);
        let (bytes, bracketed) = paste_bytes("a\nb", None, off).expect("mode off");
        assert_eq!(bytes, b"a\nb".to_vec());
        assert!(!bracketed);
        let (bytes, bracketed) = paste_bytes("a\nb", None, on).expect("mode on");
        assert_eq!(bytes, b"\x1b[200~a\nb\x1b[201~".to_vec());
        assert!(bracketed);
        let err = paste_bytes("a\nb", Some(true), off).expect_err("mode off");
        assert!(
            format!("{err}").contains("bracketed paste"),
            "the error names the mode: {err}"
        );
        let (bytes, bracketed) = paste_bytes("a\nb", Some(false), on).expect("never brackets");
        assert_eq!(bytes, b"a\nb".to_vec());
        assert!(!bracketed);
    }

    #[test]
    fn paste_strips_an_embedded_end_bracket() {
        let on = ShellModes {
            bracketed_paste: true,
            ..ShellModes::default()
        };
        let (bytes, bracketed) = paste_bytes("a\x1b[201~b\n", None, on).expect("the mode is on");
        assert_eq!(bytes, b"\x1b[200~ab\n\x1b[201~".to_vec());
        assert!(bracketed);
    }
}
