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
    #[serde(rename = "Running")]
    Running,
    #[serde(rename = "Exited")]
    Exited,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum ShellRequest {
    Subscribe,
    Input { bytes: Vec<u8> },
    Resize { rows: u16, cols: u16 },
    Signal { signal: ShellSignal },
    Status,
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
    Status {
        state: ShellState,
        rows: u16,
        cols: u16,
        pid: u32,
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
                },
                json!({"type": "Status", "state": "Running", "rows": 40, "cols": 120, "pid": 1234}),
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
        assert_eq!(encode(ShellState::Running).unwrap(), "\"Running\"\n");
        assert_eq!(encode(ShellState::Exited).unwrap(), "\"Exited\"\n");
        assert!(serde_json::from_str::<ShellSignal>("\"int\"").is_err());
        assert!(serde_json::from_str::<ShellSignal>("\"SIGINT\"").is_err());
        assert!(serde_json::from_str::<ShellState>("\"running\"").is_err());
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
}
