//! Wire protocol frame types for the shell-host socket.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellSignal {
    #[serde(rename = "INT")]
    Int,
    #[serde(rename = "TERM")]
    Term,
    #[serde(rename = "STOP")]
    Stop,
    #[serde(rename = "CONT")]
    Cont,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellState {
    #[serde(rename = "Idle")]
    Idle,
    #[serde(rename = "Running")]
    Running,
    #[serde(rename = "Paused")]
    Paused,
    #[serde(rename = "Exited")]
    Exited,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum ShellRequest {
    /// Start receiving every future output chunk.
    Subscribe,
    /// Answer with the current screen (`ShellResponse::Screen`) and stream
    /// every later chunk.
    SubscribeScreen,
    Input {
        bytes: Vec<u8>,
    },
    Resize {
        rows: u16,
        cols: u16,
    },
    Signal {
        signal: ShellSignal,
    },
    Status,
    /// Render the current screen: the last `scrollback` rows that scrolled
    /// off the top prepended to the visible screen, with inverse-video runs
    /// marked `[SEL: …]` and — when `annotated` — the colour runs marked.
    Screen {
        annotated: bool,
        scrollback: u16,
    },
    /// Hang up the shell itself (SIGHUP to its pid), ending it and its jobs.
    Close,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum ShellResponse {
    Ok,
    Error {
        message: String,
    },
    Chunk {
        bytes: Vec<u8>,
    },
    /// The starting screen for a `SubscribeScreen` subscriber: escape codes
    /// that redraw the host's current terminal, followed later by `Chunk`s.
    Screen {
        bytes: Vec<u8>,
    },
    /// A rendered `ShellRequest::Screen`: scrollback + the marked screen.
    ScreenText {
        text: String,
        rows: u16,
        cols: u16,
        cursor: (u16, u16),
        alt_screen: bool,
        /// The screen's bracketed-paste mode. `false` on a host from before
        /// this phase, the safe default: the paste rule then never brackets.
        #[serde(default)]
        bracketed_paste: bool,
        /// The screen's application-cursor mode. `false` on a host from
        /// before this phase.
        #[serde(default)]
        application_cursor: bool,
    },
    Status {
        state: ShellState,
        rows: u16,
        cols: u16,
        pid: u32,
        /// The slave's line discipline, read from the master fd: `Some(true)`
        /// when `ICANON` is off. `None` on a host from before this milestone.
        #[serde(default)]
        raw_mode: Option<bool>,
        /// The screen model's alt-screen bit. `None` on a pre-milestone host.
        #[serde(default)]
        alt_screen: Option<bool>,
        /// Basename of argv[0] of the foreground group leader, `None` when
        /// the group is the shell's own. `None` on a pre-milestone host.
        #[serde(default)]
        foreground: Option<String>,
    },
    Exited {
        code: i32,
    },
}

pub fn encode(frame: impl Serialize) -> serde_json::Result<String> {
    serde_json::to_string(&frame).map(|mut s| {
        s.push('\n');
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(s: &str) -> serde_json::Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn proto_request_frames_match_the_pinned_wire_format() {
        let cases = [
            (ShellRequest::Subscribe, json!({"type": "Subscribe"})),
            (
                ShellRequest::Input {
                    bytes: b"hi\n".to_vec(),
                },
                json!({"type": "Input", "bytes": [104, 105, 10]}),
            ),
            (
                ShellRequest::Resize {
                    rows: 40,
                    cols: 120,
                },
                json!({"type": "Resize", "rows": 40, "cols": 120}),
            ),
            (
                ShellRequest::Signal {
                    signal: ShellSignal::Int,
                },
                json!({"type": "Signal", "signal": "INT"}),
            ),
            (ShellRequest::Status, json!({"type": "Status"})),
        ];
        for (frame, expected) in cases {
            let wire = encode(&frame).unwrap();
            assert_eq!(
                wire.as_bytes().last(),
                Some(&b'\n'),
                "frame must end with a newline"
            );
            assert_eq!(parse(&wire), expected);
        }
    }

    #[test]
    fn proto_response_frames_match_the_pinned_wire_format() {
        let cases = [
            (ShellResponse::Ok, json!({"type": "Ok"})),
            (
                ShellResponse::Error {
                    message: "no such shell".into(),
                },
                json!({"type": "Error", "message": "no such shell"}),
            ),
            (
                ShellResponse::Chunk {
                    bytes: b"hi".to_vec(),
                },
                json!({"type": "Chunk", "bytes": [104, 105]}),
            ),
            (
                ShellResponse::Status {
                    state: ShellState::Running,
                    rows: 40,
                    cols: 120,
                    pid: 1234,
                    raw_mode: Some(true),
                    alt_screen: Some(false),
                    foreground: Some("vim".into()),
                },
                json!({
                    "type": "Status",
                    "state": "Running",
                    "rows": 40,
                    "cols": 120,
                    "pid": 1234,
                    "raw_mode": true,
                    "alt_screen": false,
                    "foreground": "vim"
                }),
            ),
            (
                // A host from before this milestone sends no foreground
                // fields; serde's `#[serde(default)]` must parse it as all
                // `None` so the classifier reads `Unknown`, never a prompt.
                ShellResponse::Status {
                    state: ShellState::Idle,
                    rows: 24,
                    cols: 80,
                    pid: 1,
                    raw_mode: None,
                    alt_screen: None,
                    foreground: None,
                },
                json!({
                    "type": "Status",
                    "state": "Idle",
                    "rows": 24,
                    "cols": 80,
                    "pid": 1,
                    "raw_mode": null,
                    "alt_screen": null,
                    "foreground": null
                }),
            ),
            (
                ShellResponse::Exited { code: 0 },
                json!({"type": "Exited", "code": 0}),
            ),
        ];
        for (frame, expected) in cases {
            let wire = encode(&frame).unwrap();
            assert_eq!(
                wire.as_bytes().last(),
                Some(&b'\n'),
                "frame must end with a newline"
            );
            assert_eq!(parse(&wire), expected);
        }
    }

    #[test]
    fn proto_signal_and_state_use_their_wire_strings() {
        assert_eq!(encode(ShellSignal::Int).unwrap(), "\"INT\"\n");
        assert_eq!(encode(ShellSignal::Term).unwrap(), "\"TERM\"\n");
        assert_eq!(encode(ShellSignal::Stop).unwrap(), "\"STOP\"\n");
        assert_eq!(encode(ShellSignal::Cont).unwrap(), "\"CONT\"\n");
        assert_eq!(encode(ShellState::Idle).unwrap(), "\"Idle\"\n");
        assert_eq!(encode(ShellState::Running).unwrap(), "\"Running\"\n");
        assert_eq!(encode(ShellState::Paused).unwrap(), "\"Paused\"\n");
        assert_eq!(encode(ShellState::Exited).unwrap(), "\"Exited\"\n");
        assert!(serde_json::from_str::<ShellSignal>("\"int\"").is_err());
        assert!(serde_json::from_str::<ShellSignal>("\"SIGINT\"").is_err());
        assert!(serde_json::from_str::<ShellState>("\"running\"").is_err());
        assert_eq!(
            serde_json::from_str::<ShellState>("\"Paused\"").unwrap(),
            ShellState::Paused
        );
        assert_eq!(
            serde_json::from_str::<ShellState>("\"Idle\"").unwrap(),
            ShellState::Idle
        );
    }

    #[test]
    fn proto_bytes_survive_a_non_utf8_round_trip() {
        let payload = vec![0xff, 0x00, b'a', 0xfe];
        let wire = encode(&ShellRequest::Input {
            bytes: payload.clone(),
        })
        .unwrap();
        let decoded: ShellRequest = serde_json::from_str(wire.trim_end()).unwrap();
        match decoded {
            ShellRequest::Input { bytes } => assert_eq!(bytes, payload, "byte-exact round trip"),
            other => panic!("unexpected variant: {other:?}"),
        }
        let wire2 = encode(&ShellResponse::Chunk {
            bytes: payload.clone(),
        })
        .unwrap();
        let decoded2: ShellResponse = serde_json::from_str(wire2.trim_end()).unwrap();
        match decoded2 {
            ShellResponse::Chunk { bytes } => assert_eq!(bytes, payload),
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn proto_screen_frames_round_trip() {
        assert_eq!(
            encode(&ShellRequest::SubscribeScreen).unwrap(),
            "{\"type\":\"SubscribeScreen\"}\n",
            "request frame is the pinned wire form"
        );
        let frame = ShellResponse::Screen {
            bytes: vec![0x1b, b'[', b'H', 0xff],
        };
        let wire = encode(&frame).unwrap();
        let decoded: ShellResponse = serde_json::from_str(wire.trim_end()).unwrap();
        assert_eq!(decoded, frame, "byte-exact Screen round trip");

        let request = ShellRequest::Screen {
            annotated: true,
            scrollback: 40,
        };
        assert_eq!(
            encode(&request).unwrap(),
            "{\"type\":\"Screen\",\"annotated\":true,\"scrollback\":40}\n",
            "the Screen request carries its two fields on the pinned wire form"
        );
        let text = ShellResponse::ScreenText {
            text: "line1\n[SEL: picked]\n$ ".into(),
            rows: 24,
            cols: 80,
            cursor: (12, 34),
            alt_screen: true,
            bracketed_paste: true,
            application_cursor: false,
        };
        let wire = encode(&text).unwrap();
        let decoded: ShellResponse = serde_json::from_str(wire.trim_end()).unwrap();
        assert_eq!(decoded, text, "ScreenText round trip");
        assert_eq!(
            parse(&wire),
            json!({
                "type": "ScreenText",
                "text": "line1\n[SEL: picked]\n$ ",
                "rows": 24,
                "cols": 80,
                "cursor": [12, 34],
                "alt_screen": true,
                "bracketed_paste": true,
                "application_cursor": false
            }),
            "the pinned wire form of a ScreenText reply"
        );
    }

    #[test]
    fn proto_encode_never_emits_an_internal_newline() {
        let payloads: [ShellRequest; 2] = [
            ShellRequest::Input {
                bytes: vec![b'\n', b'\r'],
            },
            ShellRequest::Input {
                bytes: vec![b'x', b'\n'],
            },
        ];
        for frame in payloads {
            let wire = encode(&frame).unwrap();
            let body = &wire[..wire.len() - 1];
            assert!(!body.contains('\n'), "no internal newline: {body:?}");
        }
    }

    #[test]
    fn proto_status_from_a_pre_phase_host_parses_with_none_fields() {
        // A host from before this milestone answers exactly the old wire
        // shape: no raw_mode, no alt_screen, no foreground key at all.
        let old = r#"{"type":"Status","state":"Running","rows":24,"cols":80,"pid":4711}
"#;
        let decoded: ShellResponse = serde_json::from_str(old.trim_end()).unwrap();
        match decoded {
            ShellResponse::Status {
                state,
                rows,
                cols,
                pid,
                raw_mode,
                alt_screen,
                foreground,
            } => {
                assert_eq!(state, ShellState::Running, "state parses");
                assert_eq!((rows, cols, pid), (24, 80, 4711), "geometry parses");
                assert_eq!(raw_mode, None, "missing raw_mode defaults to None");
                assert_eq!(alt_screen, None, "missing alt_screen defaults to None");
                assert_eq!(foreground, None, "missing foreground defaults to None");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn screen_text_without_key_modes_defaults_them_off() {
        // A host from before this phase sends no mode keys; a reader must
        // see both modes as off — the safe default, which never brackets.
        let old = r#"{"type":"ScreenText","text":"$ ","rows":24,"cols":80,"cursor":[0,2],"alt_screen":false}
"#;
        let decoded: ShellResponse = serde_json::from_str(old.trim_end()).unwrap();
        match decoded {
            ShellResponse::ScreenText {
                bracketed_paste,
                application_cursor,
                ..
            } => {
                assert!(!bracketed_paste, "missing bracketed_paste defaults to off");
                assert!(
                    !application_cursor,
                    "missing application_cursor defaults to off"
                );
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
