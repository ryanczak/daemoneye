//! Ingress validation: client-supplied names that become paths under the
//! daemon's config directory are checked before the request is dispatched.

use crate::ipc::Request;
use crate::util::validate_path_name;

fn check(kind: &str, name: &str) -> Option<String> {
    validate_path_name(kind, name).err()
}

/// Returns an error when a client-supplied name that will be joined into a
/// path fails `validate_path_name`. Session ids and prompt names are checked
/// here, at the single IPC ingress point.
pub fn ingress_name_error(req: &Request) -> Option<String> {
    match req {
        Request::Ask {
            session_id, prompt, ..
        } => {
            if let Some(id) = session_id
                && let Some(e) = check("session id", id)
            {
                return Some(e);
            }
            if let Some(p) = prompt
                && let Some(e) = check("prompt name", p)
            {
                return Some(e);
            }
            None
        }
        Request::SaveSession {
            session_id, name, ..
        }
        | Request::LoadSession {
            session_id, name, ..
        } => check("session id", session_id).or_else(|| check("session id", name)),
        Request::SetModel { session_id, .. }
        | Request::ListModels { session_id }
        | Request::SetPane { session_id, .. }
        | Request::ListPanesForSession { session_id }
        | Request::QueryLimits { session_id }
        | Request::ResetSessionToolCount { session_id }
        | Request::Cancel { session_id }
        | Request::ChatAttach { session_id } => check("session id", session_id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::Request;

    fn ask(session_id: Option<&str>, prompt: Option<&str>) -> Request {
        Request::Ask {
            query: "q".into(),
            tmux_pane: None,
            session_id: session_id.map(str::to_string),
            chat_pane: None,
            prompt: prompt.map(str::to_string),
            chat_width: None,
            tmux_session: None,
            target_pane: None,
            model: None,
        }
    }

    #[test]
    fn ingress_refuses_traversing_names() {
        assert!(
            ingress_name_error(&ask(Some("../x"), None)).is_some(),
            "Ask with a traversing session_id must be refused"
        );
        assert!(
            ingress_name_error(&ask(Some("266f80f28f5db7e6"), Some("../config"))).is_some(),
            "Ask with a traversing prompt must be refused"
        );
        let set_model_bad = Request::SetModel {
            session_id: "a/b".into(),
            model: "m".into(),
        };
        assert!(
            ingress_name_error(&set_model_bad).is_some(),
            "SetModel with a path separator in session_id must be refused"
        );
    }

    #[test]
    fn ingress_accepts_the_ids_the_daemon_and_cli_mint() {
        assert!(
            ingress_name_error(&ask(Some("266f80f28f5db7e6"), None)).is_none(),
            "a 16-hex session id must pass"
        );
        assert!(
            ingress_name_error(&ask(
                Some("3f2a9c1e-7b4d-4e8a-9f00-12ab34cd56ef"),
                Some("sre")
            ))
            .is_none(),
            "a UUID session id with prompt 'sre' must pass"
        );
        assert!(
            ingress_name_error(&ask(None, None)).is_none(),
            "Ask without a session id must pass"
        );
        assert!(
            ingress_name_error(&Request::Status).is_none(),
            "Status carries no names and must pass"
        );
    }
}
