use super::auth::credential_round_trip;
use crate::ipc::Request;
use crate::shell::PromptReply;

/// The prompt id the round trip echoes; nothing reads it here.
const PROMPT_ID: &str = "c";

/// An oversized credential reply is a give-up, not a hang: the reader hands
/// out at most `MAX_IPC_MESSAGE_BYTES + 64 KiB` of a 64 MiB source, the
/// reason says `too large`, and neither the "nobody is attached" nor the
/// "not supplied in time" wording is reused.
#[tokio::test]
async fn credential_prompt_refuses_an_oversized_reply_within_budget() {
    let mut rx = tokio::io::BufReader::new(tokio::io::AsyncReadExt::take(
        tokio::io::repeat(b'a'),
        64 << 20,
    ));
    let mut tx = std::io::Cursor::new(Vec::new());
    let reply = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        credential_round_trip(PROMPT_ID, &mut tx, &mut rx, "sudo password".to_string()),
    )
    .await
    .expect("the reply is refused within 10 s, not read to the end");
    match reply {
        PromptReply::GiveUp(reason) => {
            assert!(
                reason.contains("too large"),
                "the give-up names the oversized line: {reason}"
            );
            assert!(
                !reason.contains("nobody is attached"),
                "the give-up must not reuse the nobody-attached wording: {reason}"
            );
            assert!(
                !reason.contains("not supplied in time"),
                "the give-up must not reuse the not-supplied-in-time wording: {reason}"
            );
        }
        other => panic!("an oversized reply is a GiveUp, got {other:?}"),
    }
    let handed_out = (64u64 << 20) - rx.get_ref().limit();
    assert!(
        handed_out <= (crate::ipc::MAX_IPC_MESSAGE_BYTES + (64 << 10)) as u64,
        "at most the cap plus 64 KiB is handed out before the refusal: {handed_out}"
    );
}

/// The round trip's answer stays zeroized end to end: the payload is typed
/// `Zeroizing<Vec<u8>>` and carries the credential plus the trailing newline.
#[tokio::test]
async fn credential_round_trip_returns_a_zeroizing_answer() {
    let line = serde_json::to_string(&Request::CredentialResponse {
        id: PROMPT_ID.to_string(),
        credential: "pw".into(),
    })
    .expect("CredentialResponse serialises");
    let mut rx = tokio::io::BufReader::new(std::io::Cursor::new(format!("{line}\n").into_bytes()));
    let mut tx = std::io::Cursor::new(Vec::new());
    let reply =
        credential_round_trip(PROMPT_ID, &mut tx, &mut rx, "sudo password".to_string()).await;
    let PromptReply::Send(answer) = reply else {
        panic!("an answered prompt is a Send, got {reply:?}");
    };
    let answer: &zeroize::Zeroizing<Vec<u8>> = &answer;
    assert_eq!(
        answer.as_slice(),
        b"pw\n",
        "the answer carries the credential and the newline"
    );
}
