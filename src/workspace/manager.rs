//! The Core-wide terminal inventory: every live terminal in the connected
//! Core, its facts, and the local selection/filter rules for the manager
//! overlay. Pure: no IO and no clock. The runtime supplies the current time.
//!
//! A terminal is listed from [`Model`] alone. Nothing here calls
//! `terminal.read`, looks at screen text or infers a status: the word is the
//! program's own OSC 7501 report, or `"unknown"` when it never reported. The
//! agent sidebar is a different view (only workspace agents); this module is
//! the inventory of everything Core runs.

use super::model::{
    CLIENT_LABEL, CLIENT_NAME, PANE_LABEL, PaneId, SPACE_LABEL, TAB_LABEL, Workspace,
};
use crate::core::Terminal;
use crate::model::Model;
use std::collections::BTreeMap;

/// One row of the Core-wide terminal inventory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagerRow {
    /// The full terminal address (`t7`, `machine/t7`).
    pub address: String,
    /// The machine when the address has the `machine/t7` form.
    pub machine: Option<String>,
    /// The Core record's name, or the address when it has none.
    pub name: String,
    /// The Core record's terminal title (OSC 0/2), if any.
    pub title: String,
    /// The terminal that started this one, from Core's record.
    pub parent: Option<String>,
    /// The program's own OSC 7501 word; `"unknown"` when it never reported.
    pub word: String,
    /// The Core record's working directory, else its foreground cwd.
    pub cwd: String,
    /// A compact age from Core's `created_ms`, or `"-"` when absent.
    pub age: String,
    /// The raw creation time, used only for stable ordering.
    pub created_ms: Option<u64>,
    /// This terminal is in the current workspace.
    pub shown: bool,
}

/// The display name a Core record gives a pane: its name, else its address.
pub fn display_name(terminal: &Terminal) -> String {
    terminal
        .name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| terminal.address.clone())
}

/// The status word a row shows. Only the program's OSC 7501 root word is
/// used; no status is `"unknown"`.
pub fn word(terminal: &Terminal) -> String {
    terminal
        .status
        .as_ref()
        .map(|status| status.state.as_str())
        .filter(|state| !state.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}

/// The machine prefix of a `machine/t7` address.
pub fn machine(address: &str) -> Option<String> {
    address
        .split_once('/')
        .map(|(machine, _)| machine)
        .filter(|machine| !machine.is_empty())
        .map(str::to_owned)
}

fn cwd(terminal: &Terminal) -> String {
    if !terminal.cwd.is_empty() {
        return terminal.cwd.clone();
    }
    terminal
        .foreground
        .as_ref()
        .map(|foreground| foreground.cwd.clone())
        .unwrap_or_default()
}

/// A compact age: seconds, minutes, hours or days. Core sends `created_ms`
/// with a snapshot record; a run reply may omit it.
pub fn age_text(now_ms: u64, created_ms: Option<u64>) -> String {
    let Some(created_ms) = created_ms else {
        return "-".to_owned();
    };
    let seconds = now_ms.saturating_sub(created_ms) / 1000;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 60 * 60 {
        format!("{}m", seconds / 60)
    } else if seconds < 24 * 60 * 60 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / (24 * 60 * 60))
    }
}

/// The sort group for a displayed word. Blocked (and the program's own
/// `error`) first, then working, done, idle, and anything else last.
fn priority(word: &str) -> u8 {
    match word.to_ascii_lowercase().as_str() {
        "blocked" | "error" => 0,
        "working" => 1,
        "done" => 2,
        "idle" => 3,
        _ => 4,
    }
}

/// Every live terminal in the connection, sorted blocked > working > done >
/// idle > unknown, then oldest first, then address. Finished terminals are
/// out of scope.
pub fn rows(model: &Model, workspace: &Workspace, now_ms: u64) -> Vec<ManagerRow> {
    let mut rows: Vec<ManagerRow> = model
        .terminals()
        .filter(|terminal| terminal.is_live() && !terminal.address.is_empty())
        .map(|terminal| ManagerRow {
            address: terminal.address.clone(),
            machine: machine(&terminal.address),
            name: display_name(terminal),
            title: terminal.title.clone(),
            parent: terminal.parent.clone().filter(|parent| !parent.is_empty()),
            word: word(terminal),
            cwd: cwd(terminal),
            age: age_text(now_ms, terminal.created_ms),
            created_ms: terminal.created_ms,
            shown: workspace.pane_by_address(&terminal.address).is_some(),
        })
        .collect();
    rows.sort_by(|left, right| {
        priority(&left.word)
            .cmp(&priority(&right.word))
            .then_with(|| match (left.created_ms, right.created_ms) {
                (Some(left), Some(right)) => left.cmp(&right),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| left.address.cmp(&right.address))
    });
    rows
}

/// The rows matching a case-insensitive substring filter over the facts the
/// row shows. An empty query matches everything.
pub fn filter<'a>(rows: &'a [ManagerRow], query: &str) -> Vec<&'a ManagerRow> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return rows.iter().collect();
    }
    rows.iter()
        .filter(|row| {
            let fields = [
                row.address.as_str(),
                row.machine.as_deref().unwrap_or(""),
                row.name.as_str(),
                row.title.as_str(),
                row.parent.as_deref().unwrap_or(""),
                row.word.as_str(),
                row.cwd.as_str(),
                row.age.as_str(),
            ];
            fields
                .iter()
                .any(|field| field.to_ascii_lowercase().contains(&query))
        })
        .collect()
}

/// The selected row, or the first row when the saved address no longer
/// matches the filter. `None` only when there are no rows.
pub fn selected_row<'a>(rows: &[&'a ManagerRow], selected: Option<&str>) -> Option<&'a ManagerRow> {
    let selected =
        selected.and_then(|address| rows.iter().copied().find(|row| row.address == address));
    selected.or_else(|| rows.first().copied())
}

/// Keeps a selection when its address is still in the filtered rows; otherwise
/// selects the first row.
pub fn ensure_selection(rows: &[&ManagerRow], selected: Option<&str>) -> Option<String> {
    if let Some(selected) = selected
        && rows.iter().any(|row| row.address == selected)
    {
        return Some(selected.to_owned());
    }
    rows.first().map(|row| row.address.clone())
}

/// Moves the selection by `delta`, clamped at the first and last row. With no
/// selection, forward selects the first and backward the last.
pub fn move_selection(
    rows: &[&ManagerRow],
    selected: Option<&str>,
    delta: isize,
) -> Option<String> {
    if rows.is_empty() {
        return None;
    }
    let current = selected.and_then(|address| rows.iter().position(|row| row.address == address));
    let index = match (current, delta.signum()) {
        (Some(index), 1) => (index + 1).min(rows.len() - 1),
        (Some(index), -1) => index.saturating_sub(1),
        (None, 1) => 0,
        (None, -1) => rows.len() - 1,
        (Some(index), _) => index,
        (None, _) => 0,
    };
    rows.get(index).map(|row| row.address.clone())
}

/// The first or last filtered row.
pub fn edge_selection(rows: &[&ManagerRow], last: bool) -> Option<String> {
    if last { rows.last() } else { rows.first() }.map(|row| row.address.clone())
}

/// The label removal for a terminal this screen owns. A hidden terminal must
/// not be recovered as an orphan on the next start, so its identity labels
/// are cleared. Only this screen's own labels are touched: the client name
/// alone is not enough, because every HerdR Terminal screen writes the same
/// `herdr.client` value but allocates ids in its own namespace. The label's
/// high 32 bits name the screen that wrote it; it must equal `namespace`
/// (this workspace's allocator namespace). A missing or unparseable pane
/// label cannot prove the screen, so nothing is cleared.
pub fn release_labels(
    labels: &BTreeMap<String, String>,
    namespace: u64,
) -> Option<BTreeMap<String, String>> {
    if labels.get(CLIENT_LABEL).map(String::as_str) != Some(CLIENT_NAME) {
        return None;
    }
    let pane: u64 = labels.get(PANE_LABEL)?.parse().ok()?;
    if pane >> 32 != namespace {
        return None;
    }
    Some(
        [CLIENT_LABEL, SPACE_LABEL, TAB_LABEL, PANE_LABEL]
            .into_iter()
            .map(|label| (label.to_owned(), String::new()))
            .collect(),
    )
}

/// The pane id a terminal's `herdr.terminal.pane` label names when it parses
/// and differs from `pane`, i.e. a label left over from an earlier pane.
pub fn stale_pane_label(labels: &BTreeMap<String, String>, pane: PaneId) -> Option<PaneId> {
    labels
        .get(PANE_LABEL)?
        .parse::<u64>()
        .ok()
        .map(PaneId)
        .filter(|labeled| *labeled != pane)
}

/// One manager line fitted to `width` characters. Width is a character count,
/// not a display width: a wide glyph costs one column here, which keeps the
/// rule dependency-free. When the full line fits, the wide format is used
/// exactly. Otherwise the line keeps the marker, status word and address, then
/// appends the secondary facts in priority order (name, title, parent,
/// machine, cwd, age) only while each fits, so a narrow terminal drops facts
/// instead of clipping the address. If even the base does not fit, the address
/// is shortened to a recognizable prefix plus `…` rather than wrapping.
pub fn row_line(row: &ManagerRow, width: usize) -> String {
    let full = full_row_line(row);
    if full.chars().count() <= width {
        return full;
    }
    let marker = if row.shown { "*" } else { " " };
    let head = format!("{marker} {} ", row.word);
    let mut text = fit_base(&head, &row.address, width);
    for fact in compact_facts(row) {
        if !push_fact(&mut text, &fact, width) {
            break;
        }
    }
    text
}

/// The wide format: marker, word, age and address, then every present fact.
fn full_row_line(row: &ManagerRow) -> String {
    let marker = if row.shown { "*" } else { " " };
    let mut text = format!("{marker} {:<8} {:<4} {}", row.word, row.age, row.address);
    if row.name != row.address {
        text.push_str(" · ");
        text.push_str(&row.name);
    }
    if !row.title.is_empty() && row.title != row.name {
        text.push_str(" · ");
        text.push_str(&row.title);
    }
    if let Some(parent) = &row.parent {
        text.push_str(" · parent ");
        text.push_str(parent);
    }
    if let Some(machine) = &row.machine {
        text.push_str(" · machine ");
        text.push_str(machine);
    }
    if !row.cwd.is_empty() {
        text.push_str(" · ");
        text.push_str(&row.cwd);
    }
    text
}

/// The compact line's facts, highest priority first. Age is last so it is the
/// first secondary fact dropped, after machine and cwd.
fn compact_facts(row: &ManagerRow) -> Vec<String> {
    let mut facts = Vec::new();
    if row.name != row.address {
        facts.push(row.name.clone());
    }
    if !row.title.is_empty() && row.title != row.name {
        facts.push(row.title.clone());
    }
    if let Some(parent) = &row.parent {
        facts.push(format!("parent {parent}"));
    }
    if let Some(machine) = &row.machine {
        facts.push(format!("machine {machine}"));
    }
    if !row.cwd.is_empty() {
        facts.push(row.cwd.clone());
    }
    facts.push(row.age.clone());
    facts
}

/// The marker, status word and address. The address is shortened with `…` when
/// the base would exceed `width`; at widths too small for even the head, the
/// head itself is clipped as the only alternative to wrapping. Shared with the
/// machine manager, whose rows follow the same width rule.
pub(super) fn fit_base(head: &str, address: &str, width: usize) -> String {
    let head_len = head.chars().count();
    if head_len + address.chars().count() <= width {
        return format!("{head}{address}");
    }
    if head_len >= width {
        return head.chars().take(width).collect();
    }
    let room = width - head_len;
    let mut text = head.to_owned();
    if room == 1 {
        text.push('…');
        return text;
    }
    text.extend(address.chars().take(room - 1));
    text.push('…');
    text
}

/// Appends ` · fact` when it fits; false when it does not. Shared with the
/// machine manager.
pub(super) fn push_fact(text: &mut String, fact: &str, width: usize) -> bool {
    let extra = format!(" · {fact}");
    if text.chars().count() + extra.chars().count() > width {
        return false;
    }
    text.push_str(&extra);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Snapshot, Status};
    use crate::workspace::model::IdAllocator;

    fn terminal(address: &str, state: &str) -> Terminal {
        Terminal {
            address: address.to_owned(),
            state: state.to_owned(),
            ..Terminal::default()
        }
    }

    fn with_status(mut terminal: Terminal, state: &str, created_ms: Option<u64>) -> Terminal {
        terminal.status = Some(Status {
            state: state.to_owned(),
            ..Status::default()
        });
        terminal.created_ms = created_ms;
        terminal
    }

    fn model(terminals: Vec<Terminal>) -> Model {
        Model::from_snapshot(&Snapshot {
            seq: 1,
            terminals,
            ..Snapshot::default()
        })
    }

    fn adopt(workspace: &mut Workspace, address: &str) {
        let request = workspace.adopt_request();
        workspace
            .insert_adopted(
                &request,
                super::super::model::AdoptedTerminal {
                    address: address.to_owned(),
                    name: address.to_owned(),
                    labels: BTreeMap::new(),
                    title: String::new(),
                    status: None,
                    keep: false,
                },
            )
            .expect("adopt");
    }

    #[test]
    fn only_live_terminals_are_listed_with_core_facts() {
        let mut t7 = with_status(terminal("t7", "live"), "blocked", Some(10_000));
        t7.name = Some("worker".into());
        t7.title = "Build".into();
        t7.cwd = "/work".into();
        t7.parent = Some("t1".into());
        let t8 = with_status(terminal("m1/t8", "live"), "working", Some(20_000));
        let finished = with_status(terminal("t9", "finished"), "done", Some(5_000));
        let model = model(vec![t7, t8, finished]);
        let workspace = Workspace::new();
        let rows = rows(&model, &workspace, 40_000);
        assert_eq!(rows.len(), 2, "{rows:?}");
        let worker = &rows[0];
        assert_eq!(worker.address, "t7");
        assert_eq!(worker.machine, None);
        assert_eq!(worker.name, "worker");
        assert_eq!(worker.title, "Build");
        assert_eq!(worker.parent.as_deref(), Some("t1"));
        assert_eq!(worker.word, "blocked");
        assert_eq!(worker.cwd, "/work");
        assert_eq!(worker.age, "30s");
        assert!(!worker.shown, "nothing is in this workspace");
        assert_eq!(rows[1].machine.as_deref(), Some("m1"));
        assert_eq!(rows[1].name, "m1/t8", "no name falls back to the address");
        assert_eq!(rows[1].word, "working");
    }

    #[test]
    fn a_missing_status_is_unknown_and_a_missing_age_is_a_dash() {
        let rows = rows(
            &model(vec![terminal("t7", "live")]),
            &Workspace::new(),
            1_000,
        );
        assert_eq!(rows[0].word, "unknown");
        assert_eq!(rows[0].age, "-");
        assert_eq!(rows[0].cwd, "");
        assert_eq!(rows[0].parent, None);
    }

    #[test]
    fn a_foreground_cwd_is_used_when_the_record_has_none() {
        let mut record = terminal("t7", "live");
        record.foreground = Some(crate::core::Foreground {
            cwd: "/foreground".into(),
            ..crate::core::Foreground::default()
        });
        let rows = rows(&model(vec![record]), &Workspace::new(), 1_000);
        assert_eq!(rows[0].cwd, "/foreground");
    }

    #[test]
    fn shown_is_exactly_a_workspace_pane_with_the_address() {
        let mut workspace = Workspace::new();
        adopt(&mut workspace, "t7");
        let rows = rows(
            &model(vec![terminal("t7", "live"), terminal("t8", "live")]),
            &workspace,
            1_000,
        );
        assert!(rows.iter().find(|row| row.address == "t7").unwrap().shown);
        assert!(!rows.iter().find(|row| row.address == "t8").unwrap().shown);
    }

    #[test]
    fn rows_sort_by_word_group_then_age_then_address() {
        let records = vec![
            with_status(terminal("unknown-new", "live"), "", Some(40_000)),
            with_status(terminal("idle", "live"), "idle", Some(30_000)),
            with_status(terminal("done", "live"), "done", Some(20_000)),
            with_status(terminal("working-new", "live"), "working", Some(25_000)),
            with_status(terminal("working-old", "live"), "working", Some(10_000)),
            with_status(terminal("blocked", "live"), "blocked", Some(50_000)),
            with_status(terminal("error", "live"), "error", Some(60_000)),
        ];
        let rows = rows(&model(records), &Workspace::new(), 100_000);
        let addresses: Vec<&str> = rows.iter().map(|row| row.address.as_str()).collect();
        assert_eq!(
            addresses,
            [
                "blocked",
                "error",
                "working-old",
                "working-new",
                "done",
                "idle",
                "unknown-new"
            ],
            "blocked/error, working oldest first, done, idle, unknown"
        );
    }

    #[test]
    fn the_filter_is_case_insensitive_over_every_shown_fact() {
        let mut t7 = with_status(terminal("m1/t7", "live"), "working", Some(10_000));
        t7.name = Some("Worker".into());
        t7.title = "Build the thing".into();
        t7.parent = Some("t1".into());
        t7.cwd = "/srv/project".into();
        let rows = self::rows(&model(vec![t7]), &Workspace::new(), 20_000);
        for query in [
            "WORKER", "m1", "build", "T1", "srv", "working", "10s", "m1/t7",
        ] {
            assert_eq!(filter(&rows, query).len(), 1, "query {query:?}");
        }
        assert!(filter(&rows, "no such terminal").is_empty());
        assert_eq!(filter(&rows, "  ").len(), 1);
    }

    #[test]
    fn selection_follows_the_address_and_clamps_at_the_ends() {
        let rows = self::rows(
            &model(vec![
                with_status(terminal("t7", "live"), "blocked", Some(1_000)),
                with_status(terminal("t8", "live"), "working", Some(2_000)),
                with_status(terminal("t9", "live"), "idle", Some(3_000)),
            ]),
            &Workspace::new(),
            4_000,
        );
        let filtered = filter(&rows, "");
        assert_eq!(ensure_selection(&filtered, None).as_deref(), Some("t7"));
        assert_eq!(
            ensure_selection(&filtered, Some("t8")).as_deref(),
            Some("t8")
        );
        assert_eq!(
            ensure_selection(&filtered, Some("gone")).as_deref(),
            Some("t7"),
            "a stale address selects the first filtered row"
        );
        assert_eq!(
            move_selection(&filtered, Some("t7"), 1).as_deref(),
            Some("t8")
        );
        assert_eq!(
            move_selection(&filtered, Some("t9"), 1).as_deref(),
            Some("t9"),
            "the last row is a clamp, not a wrap"
        );
        assert_eq!(move_selection(&filtered, None, -1).as_deref(), Some("t9"));
        assert_eq!(edge_selection(&filtered, true).as_deref(), Some("t9"));
        assert_eq!(edge_selection(&filtered, false).as_deref(), Some("t7"));
    }

    #[test]
    fn age_formatting_is_compact_and_unknown_is_dash() {
        assert_eq!(age_text(10_000, Some(10_000)), "0s");
        assert_eq!(age_text(69_999, Some(10_000)), "59s");
        assert_eq!(age_text(70_000, Some(10_000)), "1m");
        assert_eq!(age_text(3_610_000, Some(10_000)), "1h");
        assert_eq!(age_text(86_410_000, Some(10_000)), "1d");
        assert_eq!(age_text(10_000, None), "-");
        assert_eq!(age_text(10_000, Some(20_000)), "0s", "a future time is 0s");
    }

    #[test]
    fn a_row_line_shows_the_marker_and_facts_in_a_readable_order() {
        let mut t7 = with_status(terminal("m1/t7", "live"), "blocked", Some(10_000));
        t7.name = Some("worker".into());
        t7.title = "Build".into();
        t7.parent = Some("t1".into());
        t7.cwd = "/work".into();
        let rows = self::rows(&model(vec![t7]), &Workspace::new(), 20_000);
        let line = row_line(&rows[0], 200);
        assert!(line.starts_with("  blocked"), "{line:?}");
        assert!(line.contains("10s"), "{line:?}");
        assert!(line.contains("m1/t7"), "{line:?}");
        assert!(line.contains("worker"), "{line:?}");
        assert!(line.contains("Build"), "{line:?}");
        assert!(line.contains("parent t1"), "{line:?}");
        assert!(line.contains("machine m1"), "{line:?}");
        assert!(line.contains("/work"), "{line:?}");

        let mut workspace = Workspace::new();
        adopt(&mut workspace, "m1/t7");
        let rows = self::rows(
            &model(vec![{
                let mut record = with_status(terminal("m1/t7", "live"), "blocked", Some(10_000));
                record.name = Some("worker".into());
                record
            }]),
            &workspace,
            20_000,
        );
        assert!(row_line(&rows[0], 200).starts_with("* blocked"));
    }

    #[test]
    fn a_narrow_row_keeps_the_status_and_address_and_drops_secondary_facts() {
        let mut t7 = with_status(terminal("m1/t7", "live"), "blocked", Some(10_000));
        t7.name = Some("worker".into());
        t7.title = "Build".into();
        t7.parent = Some("t1".into());
        t7.cwd = "/work".into();
        let rows = self::rows(&model(vec![t7]), &Workspace::new(), 20_000);
        let row = &rows[0];

        let wide = row_line(row, 200);
        assert!(wide.chars().count() <= 200);
        for fact in [
            "blocked",
            "m1/t7",
            "10s",
            "worker",
            "Build",
            "parent t1",
            "machine m1",
            "/work",
        ] {
            assert!(wide.contains(fact), "{fact}: {wide:?}");
        }

        let medium = row_line(row, 40);
        assert!(medium.chars().count() <= 40, "{medium:?}");
        assert!(medium.contains("blocked"), "{medium:?}");
        assert!(medium.contains("m1/t7"), "{medium:?}");
        assert!(
            medium.contains("worker"),
            "name survives when it fits: {medium:?}"
        );
        assert!(
            medium.contains("Build"),
            "title survives until the width runs out: {medium:?}"
        );
        assert!(
            !medium.contains("machine m1") && !medium.contains("/work") && !medium.contains("10s"),
            "age, machine and cwd are dropped first: {medium:?}"
        );

        let very_narrow = row_line(row, 14);
        assert!(very_narrow.chars().count() <= 14, "{very_narrow:?}");
        assert_eq!(very_narrow, "  blocked m1/…");
        assert!(!very_narrow.contains("worker"), "{very_narrow:?}");
        assert!(!very_narrow.contains("/work"), "{very_narrow:?}");

        for width in [0, 1, 5, 9, 10, 12, 14, 20, 40, 80] {
            assert!(
                row_line(row, width).chars().count() <= width,
                "width {width}: {:?}",
                row_line(row, width)
            );
        }
    }

    fn identity_labels(pane: u64) -> BTreeMap<String, String> {
        BTreeMap::from([
            (CLIENT_LABEL.to_owned(), CLIENT_NAME.to_owned()),
            (SPACE_LABEL.to_owned(), "1".to_owned()),
            (TAB_LABEL.to_owned(), "2".to_owned()),
            (PANE_LABEL.to_owned(), pane.to_string()),
            ("herdr.seen".to_owned(), "4".to_owned()),
        ])
    }

    #[test]
    fn only_our_own_identity_labels_are_cleared_on_hide() {
        let screen = IdAllocator::new("default").namespace();
        let ours = identity_labels((screen << 32) | 3);
        let cleared = release_labels(&ours, screen).expect("our labels");
        for label in [CLIENT_LABEL, SPACE_LABEL, TAB_LABEL, PANE_LABEL] {
            assert_eq!(cleared.get(label).map(String::as_str), Some(""), "{label}");
        }
        assert_eq!(cleared.len(), 4);
        assert!(
            !cleared.contains_key("herdr.seen"),
            "shared attention labels are never touched"
        );

        let another_client = BTreeMap::from([(CLIENT_LABEL.to_owned(), "herdr-app".to_owned())]);
        assert_eq!(release_labels(&another_client, screen), None);
        assert_eq!(release_labels(&BTreeMap::new(), screen), None);
    }

    #[test]
    fn a_hidden_pane_may_clear_labels_written_by_its_own_screen() {
        // A Show-then-Hide of a terminal this screen created in an older pane
        // still clears it: the label's namespace is this screen's, even though
        // the pane id differs from the one being hidden.
        let screen = IdAllocator::new("default").namespace();
        let ours = identity_labels((screen << 32) | 11);
        assert!(release_labels(&ours, screen).is_some());
    }

    #[test]
    fn labels_from_another_screen_namespace_are_never_cleared() {
        let screen = IdAllocator::new("default").namespace();
        let other = IdAllocator::new("another-screen").namespace();
        assert_ne!(screen, other);
        let labels = identity_labels((other << 32) | 7);
        // The hidden pane itself may carry the label's namespace; that cannot
        // identify who wrote the labels, so only the creating screen matters.
        let hidden_pane = PaneId((other << 32) | 7);
        assert_eq!(hidden_pane.0 >> 32, other);
        assert_eq!(release_labels(&labels, screen), None);
    }

    #[test]
    fn a_missing_or_unparseable_pane_label_is_never_cleared() {
        let screen = IdAllocator::new("default").namespace();
        let mut labels = identity_labels((screen << 32) | 3);
        labels.remove(PANE_LABEL);
        assert_eq!(release_labels(&labels, screen), None);

        let mut unparseable = identity_labels((screen << 32) | 3);
        unparseable.insert(PANE_LABEL.to_owned(), "not-a-pane".to_owned());
        assert_eq!(release_labels(&unparseable, screen), None);
    }

    #[test]
    fn a_stale_pane_label_is_reported_only_when_it_names_another_pane() {
        let screen = IdAllocator::new("default").namespace();
        let pane = PaneId((screen << 32) | 3);
        let labels = identity_labels(pane.0);
        assert_eq!(stale_pane_label(&labels, pane), None);
        let stale = PaneId((screen << 32) | 11);
        let labels = identity_labels(stale.0);
        assert_eq!(stale_pane_label(&labels, pane), Some(stale));
        assert_eq!(stale_pane_label(&BTreeMap::new(), pane), None);
    }
}
