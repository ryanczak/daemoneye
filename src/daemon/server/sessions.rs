//! Named-session IPC handlers: save, load, list, delete, rename.

use crate::config::Config;
use crate::daemon::session::*;
use crate::daemon::utils::*;
use crate::ipc::Response;
use anyhow::Result;

fn unknown_session_refusal(sessions: &SessionStore, session_id: &str) -> Option<String> {
    if with_sessions(sessions, |store| store.contains_key(session_id)) {
        None
    } else {
        Some(format!(
            "unknown session '{session_id}': nothing was saved or loaded"
        ))
    }
}

// ── Named session CRUD handlers ──────────────────────────────────────────────

pub(super) async fn handle_save_session<W>(
    tx: &mut W,
    sessions: &SessionStore,
    session_id: String,
    name: String,
    description: String,
    force: bool,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    if let Some(msg) = unknown_session_refusal(sessions, &session_id) {
        send_response_split(tx, Response::Error(msg)).await?;
        return Ok(());
    }
    let (msgs, turn_count, current_saved, model_name, artifacts) =
        with_sessions(sessions, |store| {
            if let Some(entry) = store.get(&session_id) {
                (
                    entry.messages.clone(),
                    entry.turn_count,
                    entry.saved_name.clone(),
                    entry
                        .active_model
                        .clone()
                        .unwrap_or_else(|| "default".to_string()),
                    entry.artifacts_created.clone(),
                )
            } else {
                (Vec::new(), 0, None, "default".to_string(), Vec::new())
            }
        });

    match crate::session_store::save_session(crate::session_store::SaveSessionArgs {
        name: &name,
        current_saved_name: current_saved.as_deref(),
        description: &description,
        messages: &msgs,
        turn_count,
        model: &model_name,
        artifacts: &artifacts,
        force,
    }) {
        Ok(()) => {
            if current_saved.is_none() && !artifacts.is_empty() {
                let errs = crate::session_store::backfill_session_origin(&artifacts, &name);
                if !errs.is_empty() {
                    log::warn!(
                        "Session {}: backfill_session_origin failed for: {}",
                        session_id,
                        errs.join(", ")
                    );
                }
            }
            with_sessions(sessions, |store| {
                if let Some(entry) = store.get_mut(&session_id) {
                    entry.saved_name = Some(name.clone());
                    entry.dirty = false;
                }
            });
            log::info!("Session {}: saved as '{}'", session_id, name);
            send_response_split(tx, Response::SessionSaved { name }).await?;
        }
        Err(e) => {
            send_response_split(tx, Response::Error(e.to_string())).await?;
        }
    }
    Ok(())
}

pub(super) async fn handle_load_session<W>(
    tx: &mut W,
    sessions: &SessionStore,
    config: &Config,
    session_id: String,
    name: String,
    force: bool,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    if let Some(msg) = unknown_session_refusal(sessions, &session_id) {
        send_response_split(tx, Response::Error(msg)).await?;
        return Ok(());
    }
    let is_dirty = with_sessions(sessions, |store| {
        store.get(&session_id).is_some_and(|e| e.dirty)
    });
    if is_dirty && !force {
        send_response_split(
            tx,
            Response::Error(format!(
                "session has unsaved changes; run `/session save <name>` first, \
                 or use `/session load {} --force` to discard them",
                name
            )),
        )
        .await?;
        return Ok(());
    }

    if !crate::session_store::session_exists(&name) {
        send_response_split(
            tx,
            Response::Error(format!("no saved session named '{}'", name)),
        )
        .await?;
        return Ok(());
    }

    let load_count = config.sessions.load_recent_turns;
    match (
        crate::session_store::load_session_meta(&name),
        crate::session_store::load_session_messages(&name, load_count),
    ) {
        (Ok(meta), Ok(loaded_msgs)) => {
            let banner = crate::session_store::build_resumed_banner(&meta, loaded_msgs.len());
            let loaded_count = loaded_msgs.len();
            let turn_count = meta.turn_count;

            with_sessions(sessions, |store| {
                if let Some(entry) = store.get_mut(&session_id) {
                    entry.messages = loaded_msgs;
                    entry.saved_name = Some(name.clone());
                    entry.dirty = false;
                }
            });
            log::info!(
                "Session {}: loaded '{}' ({} messages)",
                session_id,
                name,
                loaded_count
            );
            send_response_split(
                tx,
                Response::SessionLoaded {
                    name,
                    message_count: loaded_count,
                    turn_count,
                    banner,
                },
            )
            .await?;
        }
        (Err(e), _) | (_, Err(e)) => {
            send_response_split(tx, Response::Error(e.to_string())).await?;
        }
    }
    Ok(())
}

pub(super) async fn handle_list_saved_sessions<W>(tx: &mut W) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    use crate::session_store::{list_sessions, load_session_meta};
    let sessions_list: Vec<crate::ipc::SessionSummary> = list_sessions()
        .into_iter()
        .map(|(name, idx)| {
            let (description, turn_count, message_count, artifact_count) = load_session_meta(&name)
                .map(|m| {
                    (
                        m.description,
                        m.turn_count,
                        m.message_count,
                        m.artifacts_created.len(),
                    )
                })
                .unwrap_or_default();
            crate::ipc::SessionSummary {
                name,
                description,
                created_at: idx.created_at,
                last_updated: idx.last_updated,
                turn_count,
                message_count,
                artifact_count,
            }
        })
        .collect();
    send_response_split(
        tx,
        Response::SavedSessionList {
            sessions: sessions_list,
        },
    )
    .await?;
    Ok(())
}

pub(super) async fn handle_delete_saved_session<W>(tx: &mut W, name: String) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    match crate::session_store::delete_session(&name) {
        Ok(()) => {
            log::info!("Saved session '{}' deleted", name);
            send_response_split(tx, Response::Ok).await?;
        }
        Err(e) => {
            send_response_split(tx, Response::Error(e.to_string())).await?;
        }
    }
    Ok(())
}

pub(super) async fn handle_rename_saved_session<W>(
    tx: &mut W,
    sessions: &SessionStore,
    old_name: String,
    new_name: String,
) -> Result<()>
where
    W: tokio::io::AsyncWriteExt + Unpin,
{
    match crate::session_store::rename_session(&old_name, &new_name) {
        Ok(()) => {
            with_sessions(sessions, |store| {
                for entry in store.values_mut() {
                    if entry.saved_name.as_deref() == Some(old_name.as_str()) {
                        entry.saved_name = Some(new_name.clone());
                    }
                }
            });
            log::info!("Saved session '{}' renamed to '{}'", old_name, new_name);
            send_response_split(tx, Response::Ok).await?;
        }
        Err(e) => {
            send_response_split(tx, Response::Error(e.to_string())).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{handle_load_session, handle_save_session};
    use crate::config::Config;
    use crate::daemon::session::SessionStore;
    use crate::ipc::Response;

    /// The single response frame a handler wrote to `tx`.
    fn reply(tx: &[u8]) -> Response {
        let line = std::str::from_utf8(tx).unwrap().lines().next().unwrap();
        serde_json::from_str(line).unwrap()
    }

    #[tokio::test]
    async fn save_of_an_unknown_session_is_an_error_and_saves_nothing() {
        let (_lock, _tmp) = crate::test_home();
        let sessions = SessionStore::new();
        let mut tx: Vec<u8> = Vec::new();

        handle_save_session(
            &mut tx,
            &sessions,
            "no-such-session".to_string(),
            "saved-name".to_string(),
            String::new(),
            false,
        )
        .await
        .unwrap();

        match reply(&tx) {
            Response::Error(msg) => assert!(
                msg.contains("no-such-session"),
                "the error must name the unknown session: {msg}"
            ),
            other => panic!("expected Response::Error, got {other:?}"),
        }
        assert!(
            !crate::session_store::session_exists("saved-name"),
            "nothing may be saved for an unknown session"
        );
    }

    #[tokio::test]
    async fn load_into_an_unknown_session_is_an_error() {
        let (_lock, _tmp) = crate::test_home();
        crate::session_store::save_session(crate::session_store::SaveSessionArgs {
            name: "saved-name",
            current_saved_name: None,
            description: "",
            messages: &[],
            turn_count: 0,
            model: "default",
            artifacts: &[],
            force: false,
        })
        .unwrap();
        let sessions = SessionStore::new();
        let mut tx: Vec<u8> = Vec::new();

        handle_load_session(
            &mut tx,
            &sessions,
            &Config::default(),
            "no-such-session".to_string(),
            "saved-name".to_string(),
            false,
        )
        .await
        .unwrap();

        match reply(&tx) {
            Response::Error(msg) => assert!(
                msg.contains("no-such-session"),
                "the error must name the unknown session: {msg}"
            ),
            other => panic!("expected Response::Error, got {other:?}"),
        }
    }
}
