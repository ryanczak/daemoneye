//! The shared approve/deny reply check (S15): a reply of the right variant
//! whose id is not the prompt's is a denial, never an approval.

use crate::ipc::Request;

/// The `(id, approved)` pair one of the seven approve/deny replies carries.
/// `None` for every other request kind.
pub(crate) fn approval_reply_decision(reply: &Request) -> Option<(&str, bool)> {
    match reply {
        Request::ScriptWriteResponse { id, approved }
        | Request::ScriptDeleteResponse { id, approved }
        | Request::RunbookWriteResponse { id, approved }
        | Request::RunbookDeleteResponse { id, approved }
        | Request::ScheduleWriteResponse { id, approved }
        | Request::ScheduleRemoveResponse { id, approved }
        | Request::GhostSpawnResponse { id, approved } => Some((id, *approved)),
        _ => None,
    }
}

/// Whether the reply line answers prompt `id`, and what it decided. A reply
/// of the right variant whose id is not the prompt's is a denial (S15): it
/// was typed against another prompt and must never approve this one. Any
/// other reply — wrong variant, unparseable — is a denial, as before.
pub(crate) fn reply_approval(id: &str, line: &[u8], prompt: &str) -> bool {
    let Some(reply) = serde_json::from_slice::<Request>(line).ok() else {
        return false;
    };
    let Some((resp_id, approved)) = approval_reply_decision(&reply) else {
        return false;
    };
    if resp_id != id {
        log::warn!(
            "{prompt}: reply id {resp_id:?} does not match prompt id {id:?}; \
             the reply is treated as a denial"
        );
        return false;
    }
    approved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_approval_for_another_prompt_is_a_denial() {
        let mismatches = [
            Request::ScriptWriteResponse {
                id: "other".to_string(),
                approved: true,
            },
            Request::ScriptDeleteResponse {
                id: "other".to_string(),
                approved: true,
            },
            Request::RunbookWriteResponse {
                id: "other".to_string(),
                approved: true,
            },
            Request::RunbookDeleteResponse {
                id: "other".to_string(),
                approved: true,
            },
            Request::ScheduleWriteResponse {
                id: "other".to_string(),
                approved: true,
            },
            Request::ScheduleRemoveResponse {
                id: "other".to_string(),
                approved: true,
            },
            Request::GhostSpawnResponse {
                id: "other".to_string(),
                approved: true,
            },
        ];
        for reply in &mismatches {
            let line = serde_json::to_vec(reply).expect("reply serialises");
            assert!(
                !reply_approval("c1", &line, "test prompt"),
                "{reply:?}: an approved reply for another prompt is a denial"
            );
        }
        for reply in mismatches {
            let matching = match reply {
                Request::ScriptWriteResponse { approved, .. } => Request::ScriptWriteResponse {
                    id: "c1".to_string(),
                    approved,
                },
                Request::ScriptDeleteResponse { approved, .. } => Request::ScriptDeleteResponse {
                    id: "c1".to_string(),
                    approved,
                },
                Request::RunbookWriteResponse { approved, .. } => Request::RunbookWriteResponse {
                    id: "c1".to_string(),
                    approved,
                },
                Request::RunbookDeleteResponse { approved, .. } => Request::RunbookDeleteResponse {
                    id: "c1".to_string(),
                    approved,
                },
                Request::ScheduleWriteResponse { approved, .. } => Request::ScheduleWriteResponse {
                    id: "c1".to_string(),
                    approved,
                },
                Request::ScheduleRemoveResponse { approved, .. } => {
                    Request::ScheduleRemoveResponse {
                        id: "c1".to_string(),
                        approved,
                    }
                }
                Request::GhostSpawnResponse { approved, .. } => Request::GhostSpawnResponse {
                    id: "c1".to_string(),
                    approved,
                },
                _ => unreachable!("only the seven approve/deny variants are in the table"),
            };
            let line = serde_json::to_vec(&matching).expect("reply serialises");
            assert!(
                reply_approval("c1", &line, "test prompt"),
                "an approved reply with the matching id approves"
            );
        }
    }
}
