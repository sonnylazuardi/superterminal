//! Turning one CONTROL line into one response — `02-protocol.md` §3.
//!
//! Parsing and envelope construction live here; the *semantics* of every
//! request live in the Workspace actor, because it is the only writer
//! (`03-server.md` §3). This module therefore has exactly three jobs:
//!
//! 1. decode a line into a [`Req`], answering `bad_request` when it will not
//!    decode — with the request's `id` when one can still be salvaged;
//! 2. hand the request to the actor and wrap the answer in `ok`/`err`;
//! 3. tell the connection when a request armed the event stream
//!    (`workspace.subscribe`).

use serde_json::Value;
use st_proto::control::{AnyRes, ErrorBody, ErrorCode, ReqId, Res, Revision};
use st_proto::Req;

use crate::workspace::ClientId;
use crate::ServerContext;

/// What the connection task must do with one handled request.
#[derive(Debug)]
pub struct Handled {
    /// The response to write back.
    pub res: AnyRes,
    /// `Some(revision)` when this request was `workspace.subscribe`: the
    /// connection starts forwarding events published *after* that revision,
    /// because everything up to it is already in the response.
    pub subscribe_at: Option<Revision>,
}

/// Decodes one NDJSON line into a request.
///
/// A line that is not a request at all still deserves a well-formed `err`
/// envelope, so the `id` is dug out of the raw JSON when the typed decode
/// fails (`02-protocol.md` §3.1: exactly one response per request).
pub fn parse_request(line: &[u8]) -> Result<Req, AnyRes> {
    match serde_json::from_slice::<Req>(line) {
        Ok(req) => Ok(req),
        Err(err) => {
            let raw: Option<Value> = serde_json::from_slice(line).ok();
            let id = raw
                .as_ref()
                .and_then(|v| v.get("id"))
                .and_then(Value::as_u64)
                .and_then(|id| ReqId::try_from(id).ok())
                .unwrap_or(0);
            let tag = raw
                .as_ref()
                .and_then(|v| v.get("t"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let message = match tag {
                Some(tag) => format!("cannot handle `{tag}`: {err}"),
                None => format!("not a control-plane request: {err}"),
            };
            Err(Res::Err {
                id,
                error: ErrorBody::new(ErrorCode::BadRequest, message),
            })
        }
    }
}

/// Applies one request and builds its response.
pub async fn handle(ctx: &ServerContext, client: ClientId, req: Req) -> Handled {
    let id = req.id();
    let subscribing = matches!(req, Req::WorkspaceSubscribe { .. });

    ctx.metrics.requests.inc();
    let answer = ctx.workspace.request(req, Some(client)).await;

    match answer {
        Ok(result) => {
            let subscribe_at = subscribing.then(|| revision_of(&result)).flatten();
            Handled {
                res: Res::Ok { id, result },
                subscribe_at,
            }
        }
        Err(error) => {
            ctx.metrics.request_errors.inc();
            Handled {
                res: Res::Err { id, error },
                subscribe_at: None,
            }
        }
    }
}

/// The revision inside a `WorkspaceSnapshot` result.
fn revision_of(result: &Value) -> Option<Revision> {
    result
        .get("workspace")
        .and_then(|w| w.get("revision"))
        .and_then(Value::as_u64)
}

/// Builds an `ev.workspace` line out of a `workspace.get` result.
///
/// Used when a connection's broadcast receiver lagged: rather than replaying,
/// the server re-sends the whole document, which is exactly what the event
/// carries anyway (`02-protocol.md` §3.3).
#[must_use]
pub fn workspace_event_from_snapshot(snapshot: &Value) -> Value {
    let revision = revision_of(snapshot).unwrap_or_default();
    serde_json::json!({
        "t": "ev.workspace",
        "revision": revision,
        "workspace": snapshot.get("workspace").cloned().unwrap_or(Value::Null),
        "surfaces": snapshot.get("surfaces").cloned().unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_good_line_decodes() {
        let req = parse_request(br#"{"t":"workspace.get","id":3}"#).unwrap();
        assert_eq!(req.id(), 3);
        assert_eq!(req.tag(), "workspace.get");
    }

    #[test]
    fn an_unknown_tag_keeps_the_request_id() {
        let err = parse_request(br#"{"t":"nope.nope","id":9}"#).unwrap_err();
        let Res::Err { id, error } = err else {
            panic!("expected an err envelope");
        };
        assert_eq!(id, 9);
        assert_eq!(error.code, ErrorCode::BadRequest);
        assert!(error.message.contains("nope.nope"));
    }

    #[test]
    fn garbage_still_answers_with_id_zero() {
        let err = parse_request(b"not json at all").unwrap_err();
        let Res::Err { id, error } = err else {
            panic!("expected an err envelope");
        };
        assert_eq!(id, 0);
        assert_eq!(error.code, ErrorCode::BadRequest);
    }

    #[test]
    fn a_snapshot_becomes_an_event() {
        let snapshot = serde_json::json!({
            "workspace": { "revision": 12, "active_session": 1, "sessions": [] },
            "surfaces": [],
        });
        let ev = workspace_event_from_snapshot(&snapshot);
        assert_eq!(ev["t"], "ev.workspace");
        assert_eq!(ev["revision"], 12);
        assert_eq!(ev["workspace"]["revision"], 12);
    }
}
