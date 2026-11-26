//! The log of which session was selected when, for callers that follow the
//! user's attention, such as an assistant running a focus block.

use super::*;
use crate::persist::{SessionVisit, now_millis};

/// Long enough to look back over a few weeks of work.
const VISITS_KEPT: Duration = Duration::from_secs(30 * 24 * 60 * 60);

impl Workspace {
    /// Notes that the main area now shows this session.
    pub(super) fn record_session_visit(&self, record: &SessionRecord) {
        let Some(store) = self.store.borrow().clone() else {
            return;
        };
        let visit = SessionVisit {
            at: now_millis(),
            session_id: record.id.clone(),
            name: record.name.clone(),
        };
        let keep_since = visit
            .at
            .saturating_sub(VISITS_KEPT.as_millis().try_into().unwrap_or(u64::MAX));
        self.run_io(
            move || store.save_session_visit(&visit, keep_since),
            |_, result| {
                // The log is a convenience: losing a visit is not worth a toast.
                if let Err(error) = result {
                    eprintln!("agtk: could not record a session visit: {error}");
                }
            },
        );
    }

    pub(super) fn handle_visits_control(&self, pending: PendingRequest) {
        let ControlCommand::SessionVisits(params) = pending.request.command.clone() else {
            return;
        };
        let id = pending.request.id.clone();
        let Some(store) = self.store.borrow().clone() else {
            let _ = pending.respond(ControlResponse::failure(
                id,
                ErrorCode::InternalError,
                "Workspace is loading: the session visits are not available yet",
                None,
            ));
            return;
        };
        // The same queue as the writes, so a visit recorded just before is in the answer.
        self.run_io(
            move || store.session_visits(params.since_ms, params.limit),
            move |_, result| {
                let response = match result {
                    Ok(visits) => ControlResponse::success(
                        id,
                        serde_json::json!({ "visits": visits, "now": now_millis() }),
                    ),
                    Err(error) => ControlResponse::failure(
                        id,
                        ErrorCode::InternalError,
                        format!("Could not read the session visits: {error}"),
                        None,
                    ),
                };
                let _ = pending.respond(response);
            },
        );
    }
}
