//! The screen's pure lifecycle rules: what a closed attach stream means and
//! how a failed reconnect is paced. No IO, no VT, no ratatui, so the rules can
//! be unit-tested without a core or a terminal.

use crate::core::{Error, Terminal};
use serde_json::Value;
use std::time::Duration;

/// The first reconnect delay; it doubles up to [`RECONNECT_MAX`].
pub(crate) const RECONNECT_BASE: Duration = Duration::from_millis(250);
pub(crate) const RECONNECT_MAX: Duration = Duration::from_secs(5);

/// What a closed attach stream means.
pub(crate) enum Disposition {
    Finished,
    Lost,
    Removed,
}

/// A kept terminal whose program was lost stays alive and waits; everything
/// else is the program ending (or the record being removed).
pub(crate) fn disposition(
    record: Option<&Terminal>,
    finished: bool,
    removed: bool,
    reason: &Option<Value>,
) -> Disposition {
    if removed {
        return Disposition::Removed;
    }
    // A `finished` event is authoritative even if the CLOSE reason is a loss
    // reason from a race.
    if finished {
        return Disposition::Finished;
    }
    let state = record.map(|record| record.state.as_str()).unwrap_or("");
    if state == "finished" {
        return Disposition::Finished;
    }
    // Every screen terminal is started with keep: true; when the model has
    // not seen the record yet (a fast exit), assume it is ours.
    let keep = record.map(|record| record.keep).unwrap_or(true);
    let name = reason
        .as_ref()
        .and_then(|reason| reason.get("reason"))
        .and_then(Value::as_str);
    match name {
        Some("exited") | Some("killed") => Disposition::Finished,
        Some("signaled" | "server_stop") if keep => Disposition::Lost,
        _ => {
            if keep && record.is_some_and(|record| record.pid.is_none()) {
                Disposition::Lost
            } else {
                // A live record with a pid whose stream closed: treat it as
                // the program ending (core sent a reason, or a detach
                // acknowledgement we did not ask for).
                Disposition::Finished
            }
        }
    }
}

/// Whether a failed reconnect attempt is worth retrying. A wrong core version
/// or a bad command line will not fix itself.
pub(crate) fn retryable(error: &Error) -> bool {
    matches!(
        error,
        Error::Io(_)
            | Error::Closed
            | Error::Timeout { .. }
            | Error::Spawn { .. }
            | Error::Json(_)
            | Error::Frame(_)
    )
}

pub(crate) fn backoff(attempt: u32) -> Duration {
    let shift = attempt.min(5);
    let millis = RECONNECT_BASE
        .as_millis()
        .saturating_mul(1u128 << shift)
        .min(RECONNECT_MAX.as_millis());
    Duration::from_millis(millis as u64)
}

pub(crate) fn reconnect_message(attempt: u32) -> String {
    if attempt == 0 {
        "reconnecting to core…".into()
    } else {
        format!("reconnecting to core… (attempt {attempt})")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_backoff_doubles_then_caps() {
        assert_eq!(backoff(0), RECONNECT_BASE);
        assert_eq!(backoff(1), Duration::from_millis(500));
        assert_eq!(backoff(2), Duration::from_millis(1000));
        assert_eq!(backoff(3), Duration::from_millis(2000));
        assert_eq!(backoff(4), Duration::from_millis(4000));
        assert_eq!(backoff(5), Duration::from_millis(5000));
        assert_eq!(backoff(100), Duration::from_millis(5000));
    }

    #[test]
    fn reconnect_messages_name_the_attempt() {
        assert_eq!(reconnect_message(0), "reconnecting to core…");
        assert_eq!(reconnect_message(3), "reconnecting to core… (attempt 3)");
    }

    #[test]
    fn retryable_failures_are_the_connection_kinds() {
        assert!(retryable(&Error::Closed));
        assert!(retryable(&Error::Io(std::io::Error::other("x"))));
        assert!(!retryable(&Error::NotFound));
        assert!(!retryable(&Error::Usage("x".into())));
        assert!(!retryable(&Error::Protocol("x".into())));
    }

    #[test]
    fn a_lost_kept_terminal_waits_and_everything_else_ends() {
        let live = Terminal {
            address: "t1".into(),
            state: "live".into(),
            keep: true,
            pid: Some(1),
            ..Terminal::default()
        };
        let parked = Terminal {
            pid: None,
            ..live.clone()
        };
        let finished = Terminal {
            state: "finished".into(),
            exit_code: Some(7),
            pid: None,
            ..live.clone()
        };
        let signal = Some(serde_json::json!({"reason": "signaled", "signal": 9}));
        let exited = Some(serde_json::json!({"reason": "exited", "exit_code": 7}));
        let killed = Some(serde_json::json!({"reason": "killed"}));

        assert!(matches!(
            disposition(Some(&parked), false, false, &signal),
            Disposition::Lost
        ));
        assert!(matches!(
            disposition(Some(&live), false, false, &exited),
            Disposition::Finished
        ));
        assert!(matches!(
            disposition(Some(&live), false, false, &killed),
            Disposition::Finished
        ));
        assert!(matches!(
            disposition(Some(&finished), false, false, &signal),
            Disposition::Finished
        ));
        assert!(matches!(
            disposition(None, false, false, &signal),
            Disposition::Lost
        ));
        assert!(matches!(
            disposition(Some(&live), false, true, &exited),
            Disposition::Removed
        ));
        assert!(matches!(
            disposition(Some(&parked), true, false, &signal),
            Disposition::Finished
        ));
    }
}
