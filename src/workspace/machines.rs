//! The machine manager's pure half: rows from `machine.list`, their order,
//! the filter and selection, one line per row, and the remote address rules.
//!
//! Core owns the ssh lines and their state. This module never dials, retries
//! or infers a state: it formats the five words core reports (`connecting`,
//! `connected`, `needs_login`, `needs_update`, `unreachable`), `since_ms` and
//! `why`, and leaves every action to the runtime.

use super::manager;
use super::model::{PaneId, Workspace};
use crate::core::Machine;

/// One row of the machine manager.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineRow {
    /// The name core uses in `machine/t7` addresses.
    pub name: String,
    /// The ssh target, or the `via` command, unchanged.
    pub target: String,
    /// The target is a command rather than an ssh target.
    pub via: bool,
    /// Short-lived, shown apart from machines that stay.
    pub temporary: bool,
    /// The hc session there, when not the default.
    pub session: Option<String>,
    /// What the other core calls itself.
    pub host: String,
    /// Its hc version when last seen.
    pub version: String,
    /// `connected`, `connecting`, `needs_login`, `needs_update`,
    /// `unreachable`, or `unknown` when the profile reply carried no state.
    pub state: String,
    /// How long it has been disconnected, or `-`.
    pub since: String,
    /// Core's reason for a failed line, when it gives one.
    pub why: Option<String>,
}

/// The display word for a machine state. A profile reply from `machine.add`
/// or `machine.rename` carries no state; a list or connect view does.
pub fn state_word(state: &str) -> &str {
    if state.is_empty() { "unknown" } else { state }
}

/// Usable machines first, then ones still being dialed, then the states a
/// person can repair (login, update), then unreachable, then anything a
/// future core adds. Permanent machines precede temporary ones in a group.
fn priority(state: &str) -> u8 {
    match state {
        "connected" => 0,
        "connecting" => 1,
        "needs_login" => 2,
        "needs_update" => 3,
        "unreachable" => 4,
        _ => 5,
    }
}

/// Every machine core listed, sorted by [`priority`], then name.
pub fn rows(machines: &[Machine], now_ms: u64) -> Vec<MachineRow> {
    let mut rows: Vec<MachineRow> = machines
        .iter()
        .map(|machine| {
            let name = if machine.name.is_empty() {
                machine.target.clone()
            } else {
                machine.name.clone()
            };
            MachineRow {
                name,
                target: machine.target.clone(),
                via: machine.via,
                temporary: machine.temporary,
                session: machine
                    .session
                    .clone()
                    .filter(|session| !session.is_empty()),
                host: machine.host.clone(),
                version: machine.version.clone(),
                state: state_word(&machine.state).to_owned(),
                since: manager::age_text(now_ms, machine.since_ms),
                why: machine.why.clone().filter(|why| !why.is_empty()),
            }
        })
        .collect();
    rows.sort_by(|left, right| {
        priority(&left.state)
            .cmp(&priority(&right.state))
            .then_with(|| left.temporary.cmp(&right.temporary))
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
            .then_with(|| left.name.cmp(&right.name))
    });
    rows
}

/// The rows matching a case-insensitive substring filter over every fact the
/// row shows. An empty query matches everything.
pub fn filter<'a>(rows: &'a [MachineRow], query: &str) -> Vec<&'a MachineRow> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return rows.iter().collect();
    }
    rows.iter()
        .filter(|row| {
            let mut fields = vec![
                row.name.as_str(),
                row.target.as_str(),
                row.host.as_str(),
                row.version.as_str(),
                row.state.as_str(),
                row.since.as_str(),
                row.session.as_deref().unwrap_or(""),
                row.why.as_deref().unwrap_or(""),
            ];
            if row.temporary {
                fields.push("temporary");
            }
            if row.via {
                fields.push("via");
            }
            fields
                .iter()
                .any(|field| field.to_ascii_lowercase().contains(&query))
        })
        .collect()
}

/// The selected row, or the first row when the saved name no longer matches
/// the filter. `None` only when there are no rows.
pub fn selected_row<'a>(rows: &[&'a MachineRow], selected: Option<&str>) -> Option<&'a MachineRow> {
    let selected = selected.and_then(|name| rows.iter().copied().find(|row| row.name == name));
    selected.or_else(|| rows.first().copied())
}

/// Keeps a selection when its name is still in the filtered rows; otherwise
/// selects the first row.
pub fn ensure_selection(rows: &[&MachineRow], selected: Option<&str>) -> Option<String> {
    if let Some(selected) = selected
        && rows.iter().any(|row| row.name == selected)
    {
        return Some(selected.to_owned());
    }
    rows.first().map(|row| row.name.clone())
}

/// Moves the selection by `delta`, clamped at the first and last row. With no
/// selection, forward selects the first and backward the last.
pub fn move_selection(
    rows: &[&MachineRow],
    selected: Option<&str>,
    delta: isize,
) -> Option<String> {
    if rows.is_empty() {
        return None;
    }
    let current = selected.and_then(|name| rows.iter().position(|row| row.name == name));
    let index = match (current, delta.signum()) {
        (Some(index), 1) => (index + 1).min(rows.len() - 1),
        (Some(index), -1) => index.saturating_sub(1),
        (None, 1) => 0,
        (None, -1) => rows.len() - 1,
        (Some(index), _) => index,
        (None, _) => 0,
    };
    rows.get(index).map(|row| row.name.clone())
}

/// The first or last filtered row.
pub fn edge_selection(rows: &[&MachineRow], last: bool) -> Option<String> {
    if last { rows.last() } else { rows.first() }.map(|row| row.name.clone())
}

/// The machine prefix of a `machine/t7` address, or `None` for a local `t7`.
/// A short address is never assumed unique across machines.
pub fn machine(address: &str) -> Option<String> {
    address
        .split_once('/')
        .map(|(machine, _)| machine)
        .filter(|machine| !machine.is_empty())
        .map(str::to_owned)
}

/// A pane title that makes its machine clear when the address is qualified.
/// `m1/t7` stays as it is; a human name such as `worker` becomes `m1/worker`.
pub fn qualified_name(address: &str, name: &str) -> String {
    match machine(address) {
        Some(machine) if !name.starts_with(&format!("{machine}/")) => {
            format!("{machine}/{name}")
        }
        _ => name.to_owned(),
    }
}

/// One machine line fitted to `width` characters, following the terminal
/// manager's width rule: the wide format when it fits, else name, state and
/// target with the secondary facts appended only while they fit.
pub fn row_line(row: &MachineRow, width: usize) -> String {
    let full = full_row_line(row);
    if full.chars().count() <= width {
        return full;
    }
    let head = format!("{} {} ", row.name, row.state);
    let mut text = manager::fit_base(&head, &row.target, width);
    for fact in compact_facts(row) {
        if !manager::push_fact(&mut text, &fact, width) {
            break;
        }
    }
    text
}

fn since_fact(row: &MachineRow) -> String {
    if row.since == "-" {
        "-".to_owned()
    } else {
        format!("since {}", row.since)
    }
}

fn host_version(row: &MachineRow) -> String {
    match (row.host.is_empty(), row.version.is_empty()) {
        (false, false) => format!("{} {}", row.host, row.version),
        (false, true) => row.host.clone(),
        (true, false) => format!("hc {}", row.version),
        (true, true) => String::new(),
    }
}

/// The wide format: name, state, since, host/version and target, then the
/// flags, session and core's reason.
fn full_row_line(row: &MachineRow) -> String {
    let mut text = format!(
        "{:<12} {:<12} {:<10} {}",
        row.name,
        row.state,
        since_fact(row),
        row.target
    );
    let host = host_version(row);
    if !host.is_empty() {
        text.push_str(" · ");
        text.push_str(&host);
    }
    if let Some(session) = &row.session {
        text.push_str(" · session ");
        text.push_str(session);
    }
    if row.temporary {
        text.push_str(" · temporary");
    }
    if row.via {
        text.push_str(" · via");
    }
    if let Some(why) = &row.why {
        text.push_str(" · ");
        text.push_str(why);
    }
    text
}

/// The compact line's facts, highest priority first.
fn compact_facts(row: &MachineRow) -> Vec<String> {
    let mut facts = Vec::new();
    let host = host_version(row);
    if !host.is_empty() {
        facts.push(host);
    }
    facts.push(since_fact(row));
    if row.temporary {
        facts.push("temporary".to_owned());
    }
    if row.via {
        facts.push("via".to_owned());
    }
    if let Some(session) = &row.session {
        facts.push(format!("session {session}"));
    }
    if let Some(why) = &row.why {
        facts.push(why.clone());
    }
    facts
}

/// Gives every pane whose address is `old/t7` the new prefix `new/t7`. Core
/// renames a machine by removing the old mirrored records and creating new
/// ones; updating the workspace first keeps the pane (and its attach stream)
/// instead of losing it between the two events. A target address already in
/// the workspace is never created twice. Returns the changed pane ids.
pub fn rename_machine_addresses(workspace: &mut Workspace, old: &str, new: &str) -> Vec<PaneId> {
    if old.is_empty() || new.is_empty() || old == new {
        return Vec::new();
    }
    let existing: std::collections::BTreeSet<String> =
        workspace.panes().map(|pane| pane.address.clone()).collect();
    let mut changed = Vec::new();
    for (pane, address) in workspace
        .panes()
        .map(|pane| (pane.id, pane.address.clone()))
        .collect::<Vec<_>>()
    {
        let Some((machine, rest)) = address.split_once('/') else {
            continue;
        };
        if machine != old {
            continue;
        }
        let next = format!("{new}/{rest}");
        if existing.contains(&next) {
            continue;
        }
        if workspace.rename_pane_address(pane, next.clone()) {
            changed.push(pane);
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::model::{AdoptedTerminal, StartKind};

    fn record(name: &str, state: &str) -> Machine {
        Machine {
            name: name.into(),
            target: format!("{name}.example"),
            host: format!("{name}-host"),
            version: "0.1.18".into(),
            state: state.into(),
            ..Machine::default()
        }
    }

    fn adopt(workspace: &mut Workspace, address: &str) {
        let request = workspace.start_request(StartKind::Initial);
        workspace
            .insert_adopted(
                &request,
                AdoptedTerminal {
                    address: address.into(),
                    name: address.into(),
                    labels: Default::default(),
                    title: String::new(),
                    status: None,
                    keep: false,
                },
            )
            .expect("adopt");
    }

    #[test]
    fn rows_show_core_facts_and_sort_usable_machines_first() {
        let mut alpha = record("alpha", "unreachable");
        alpha.target = "can@10.0.0.7:2222".into();
        alpha.temporary = true;
        alpha.session = Some("work".into());
        alpha.since_ms = Some(10_000);
        alpha.why = Some("connection refused".into());
        let mut beta = record("beta", "connected");
        beta.via = true;
        let gamma = record("gamma", "connecting");
        let delta = record("delta", "needs_login");
        let rows = rows(&[alpha, beta, gamma, delta], 70_000);
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["beta", "gamma", "delta", "alpha"]);
        let alpha = rows.iter().find(|row| row.name == "alpha").unwrap();
        assert_eq!(alpha.target, "can@10.0.0.7:2222");
        assert_eq!(alpha.since, "1m");
        assert_eq!(alpha.why.as_deref(), Some("connection refused"));
        assert!(alpha.temporary);
        assert_eq!(alpha.session.as_deref(), Some("work"));
        assert!(rows.iter().find(|row| row.name == "beta").unwrap().via);
    }

    #[test]
    fn a_missing_state_is_unknown_and_a_missing_since_is_a_dash() {
        let rows = rows(&[record("one", "")], 1_000);
        assert_eq!(rows[0].state, "unknown");
        assert_eq!(rows[0].since, "-");
        assert_eq!(state_word("needs_update"), "needs_update");
    }

    #[test]
    fn permanent_machines_precede_temporary_ones_in_the_same_state() {
        let mut temp = record("a-temp", "connected");
        temp.temporary = true;
        let permanent = record("z-perm", "connected");
        let rows = rows(&[temp, permanent], 1_000);
        assert_eq!(rows[0].name, "z-perm");
    }

    #[test]
    fn the_filter_is_case_insensitive_over_every_shown_fact() {
        let mut one = record("rohan", "needs_login");
        one.target = "can@10.0.0.7:2222".into();
        one.session = Some("work".into());
        one.temporary = true;
        one.why = Some("login required".into());
        let rows = rows(&[one], 1_000);
        for query in [
            "ROHAN",
            "can@10",
            "2222",
            "needs",
            "login required",
            "WORK",
            "TEMP",
        ] {
            assert_eq!(filter(&rows, query).len(), 1, "query {query:?}");
        }
        assert!(filter(&rows, "no such machine").is_empty());
        assert_eq!(filter(&rows, "  ").len(), 1);
    }

    #[test]
    fn selection_follows_the_name_and_clamps_at_the_ends() {
        let machines = [
            record("a", "connected"),
            record("b", "connected"),
            record("c", "connected"),
        ];
        let rows = rows(&machines, 1_000);
        let filtered = filter(&rows, "");
        assert_eq!(ensure_selection(&filtered, None).as_deref(), Some("a"));
        assert_eq!(ensure_selection(&filtered, Some("b")).as_deref(), Some("b"));
        assert_eq!(
            ensure_selection(&filtered, Some("gone")).as_deref(),
            Some("a")
        );
        assert_eq!(
            move_selection(&filtered, Some("a"), 1).as_deref(),
            Some("b")
        );
        assert_eq!(
            move_selection(&filtered, Some("c"), 1).as_deref(),
            Some("c")
        );
        assert_eq!(move_selection(&filtered, None, -1).as_deref(), Some("c"));
        assert_eq!(edge_selection(&filtered, true).as_deref(), Some("c"));
        assert_eq!(edge_selection(&filtered, false).as_deref(), Some("a"));
    }

    #[test]
    fn a_wide_row_shows_every_fact_and_a_narrow_one_keeps_name_state_and_target() {
        let mut one = record("rohan", "needs_login");
        one.target = "can@10.0.0.7:2222".into();
        one.session = Some("work".into());
        one.temporary = true;
        one.via = true;
        one.since_ms = Some(10_000);
        one.why = Some("login required".into());
        let rows = rows(&[one], 70_000);
        let row = &rows[0];
        let wide = row_line(row, 200);
        for fact in [
            "rohan",
            "needs_login",
            "since 1m",
            "can@10.0.0.7:2222",
            "rohan-host",
            "0.1.18",
            "session work",
            "temporary",
            "via",
            "login required",
        ] {
            assert!(wide.contains(fact), "{fact}: {wide:?}");
        }

        let medium = row_line(row, 40);
        assert!(medium.chars().count() <= 40, "{medium:?}");
        assert!(medium.contains("rohan"), "{medium:?}");
        assert!(medium.contains("needs_login"), "{medium:?}");
        assert!(medium.contains("can@10.0.0.7:2222"), "{medium:?}");

        let narrow = row_line(row, 12);
        assert!(narrow.chars().count() <= 12, "{narrow:?}");
        assert!(narrow.contains("rohan"), "{narrow:?}");
        for width in [0, 1, 5, 9, 12, 20, 40, 80] {
            assert!(
                row_line(row, width).chars().count() <= width,
                "width {width}: {:?}",
                row_line(row, width)
            );
        }
    }

    #[test]
    fn a_qualified_address_names_its_machine_and_a_short_one_does_not() {
        assert_eq!(machine("m1/t7").as_deref(), Some("m1"));
        assert_eq!(machine("t7"), None);
        assert_eq!(machine("/t7"), None);
        assert_eq!(qualified_name("m1/t7", "t7"), "m1/t7");
        assert_eq!(qualified_name("m1/t7", "worker"), "m1/worker");
        assert_eq!(qualified_name("m1/t7", "m1/worker"), "m1/worker");
        assert_eq!(qualified_name("t7", "worker"), "worker");

        // Identity is the whole address: a local t7 is not a remote m1/t7.
        let mut workspace = Workspace::new();
        adopt(&mut workspace, "t7");
        adopt(&mut workspace, "m1/t7");
        assert_eq!(workspace.pane_count(), 2);
        assert!(workspace.pane_by_address("t7").is_some());
        assert!(workspace.pane_by_address("m1/t7").is_some());
    }

    #[test]
    fn renaming_a_machine_moves_every_qualified_pane_and_never_duplicates_one() {
        let mut workspace = Workspace::new();
        adopt(&mut workspace, "m1/t7");
        adopt(&mut workspace, "m1/t8");
        adopt(&mut workspace, "t9");
        adopt(&mut workspace, "m2/t7");
        let changed = rename_machine_addresses(&mut workspace, "m1", "rohan");
        assert_eq!(changed.len(), 2);
        assert!(workspace.pane_by_address("rohan/t7").is_some());
        assert!(workspace.pane_by_address("rohan/t8").is_some());
        assert!(workspace.pane_by_address("t9").is_some());
        assert!(workspace.pane_by_address("m2/t7").is_some());
        assert_eq!(workspace.pane_count(), 4);
        assert!(workspace.pane_by_address("m1/t7").is_none());

        // A target address that is already present is left alone rather than
        // merging two programs into one pane or making a duplicate.
        let mut workspace = Workspace::new();
        adopt(&mut workspace, "m1/t7");
        adopt(&mut workspace, "rohan/t7");
        let changed = rename_machine_addresses(&mut workspace, "m1", "rohan");
        assert!(changed.is_empty());
        assert_eq!(workspace.pane_count(), 2);

        assert!(rename_machine_addresses(&mut workspace, "m1", "m1").is_empty());
        assert!(rename_machine_addresses(&mut workspace, "", "x").is_empty());
    }
}
