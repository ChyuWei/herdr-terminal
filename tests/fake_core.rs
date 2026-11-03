//! Deterministic lifecycle regressions with a scripted stand-in for `hc`.
//!
//! This file is both the test and the fake core: cargo runs it with no
//! arguments (the tests below), while `ht` spawns the same executable as
//! `--hc PATH`, which answers `--version` and `connect`. The scripted core
//! forces the lifecycle events a real core only shows intermittently:
//!
//! * `terminal.attach` reports `finished` because the program exited first;
//! * a local stdout write fails after attach while the program keeps running;
//! * DATA and CLOSE arrive in one burst;
//! * a kept terminal is lost and restarted, the subscriber falls behind, a
//!   terminal is removed, and the connection drops;
//! * Ctrl-] detaches without killing or removing the program.
//!
//! The fake core logs every call, frame and scripted reply to `FAKE_LOG`, so
//! the tests can see what the client sent: whether it detached, removed the
//! record, killed the program, or started a duplicate terminal.

use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

mod common;

use common::{Capture, HT, Probe, secs};
#[cfg(unix)]
use herdr_terminal::core::{Attached, CoreClient, Error, StreamItem};

const DATA: u8 = 0;
const CLOSE: u8 = 2;
const EVENTS_CHANNEL: u32 = 3;
const ATTACH_CHANNEL: u32 = 5;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => println!("hc 9.9.9-fake"),
        Some("connect") => {
            if let Err(error) = fake_connect() {
                eprintln!("fake core: {error}");
                std::process::exit(1);
            }
        }
        _ => run_tests(),
    }
}

fn run_tests() {
    a_program_that_exits_before_attach_is_still_shown_and_removed();
    #[cfg(unix)]
    a_local_write_error_stops_and_removes_the_started_terminal();
    #[cfg(unix)]
    screen_draws_the_final_frame_when_data_and_close_arrive_together();
    #[cfg(unix)]
    workspace_keeps_the_final_screen_and_status_when_the_program_exited_before_attach();
    #[cfg(unix)]
    workspace_draws_the_final_frame_and_status_when_data_and_close_arrive_together();
    #[cfg(unix)]
    workspace_reclaims_a_start_whose_run_reply_was_lost();
    #[cfg(unix)]
    workspace_restores_a_document_without_starting_terminals();
    #[cfg(unix)]
    workspace_applies_the_local_view();
    #[cfg(unix)]
    workspace_focus_and_zoom_do_not_write_the_document();
    #[cfg(unix)]
    workspace_repairs_a_rediscovered_terminal();
    #[cfg(unix)]
    workspace_prunes_a_pane_whose_terminal_is_gone();
    #[cfg(unix)]
    workspace_keeps_a_pane_missing_from_the_snapshot();
    #[cfg(unix)]
    workspace_never_adopts_a_terminal_it_did_not_label();
    #[cfg(unix)]
    workspace_claims_a_new_document_and_labels_its_terminal();
    #[cfg(unix)]
    workspace_sends_each_new_terminals_attach_size_to_run();
    #[cfg(unix)]
    workspace_an_explicit_program_gets_the_new_tab_attach_size();
    #[cfg(unix)]
    workspace_reclaims_a_deleted_document_at_its_version();
    #[cfg(unix)]
    workspace_recovers_labeled_terminals_when_the_document_is_deleted();
    #[cfg(unix)]
    workspace_losing_the_deleted_document_claim_starts_no_shell();
    #[cfg(unix)]
    workspace_recovers_a_labeled_orphan_at_startup();
    #[cfg(unix)]
    workspace_ignores_an_unlabeled_orphan_at_startup();
    #[cfg(unix)]
    workspace_refetches_after_a_doc_event();
    #[cfg(unix)]
    workspace_keeps_local_focus_after_a_doc_event();
    #[cfg(unix)]
    workspace_removes_a_pane_deleted_by_another_screen();
    #[cfg(unix)]
    workspace_refetches_the_document_after_reconnect();
    #[cfg(unix)]
    workspace_refetches_the_document_after_missed();
    #[cfg(unix)]
    workspace_recreates_a_deleted_document();
    #[cfg(unix)]
    workspace_merges_a_concurrent_change_in_another_space();
    #[cfg(unix)]
    workspace_keeps_local_on_a_same_tab_conflict_and_recovers_the_other_pane();
    #[cfg(unix)]
    workspace_flushes_a_merged_change_on_detach_after_conflicts();
    #[cfg(unix)]
    workspace_reattaches_every_pane_after_missed();
    #[cfg(unix)]
    screen_detaches_without_killing_the_program();
    #[cfg(unix)]
    screen_waits_for_a_restarted_terminal();
    #[cfg(unix)]
    screen_resnapshots_after_missed_events();
    #[cfg(unix)]
    screen_repaints_without_stale_cells();
    #[cfg(unix)]
    screen_exits_when_the_terminal_is_removed();
    #[cfg(unix)]
    screen_reconnects_to_the_same_terminal();
    #[cfg(unix)]
    screen_sends_the_viewport_size_when_it_starts_and_replaces_a_terminal();
    #[cfg(unix)]
    workspace_marks_visible_turns_seen_and_alerts_for_hidden_finishes();
    #[cfg(unix)]
    workspace_alerts_once_for_a_hidden_finish_and_keeps_the_turn_unseen();
    #[cfg(unix)]
    workspace_honors_another_clients_pin_label();
    #[cfg(unix)]
    workspace_pins_unpins_and_marks_unread_with_exact_labels();
    #[cfg(unix)]
    workspace_sidebar_toggle_and_close_do_not_write_the_document();
    #[cfg(unix)]
    workspace_sidebar_consumes_unhandled_keys();
    #[cfg(unix)]
    workspace_jumps_to_an_agent_and_marks_it_seen();
    #[cfg(unix)]
    workspace_peeks_an_agent_without_reading_its_screen();
    #[cfg(unix)]
    workspace_sidebar_and_peek_do_not_resize_panes();
    #[cfg(unix)]
    workspace_retries_a_failed_seen_write_with_a_bounded_delay();
    #[cfg(unix)]
    workspace_gives_up_after_a_timed_out_seen_write();
    #[cfg(unix)]
    workspace_reconnects_after_a_lost_label_write();
    #[cfg(unix)]
    workspace_startup_baseline_does_not_alert_for_blocked_agents();
    #[cfg(unix)]
    workspace_reconnect_baseline_does_not_replay_an_alert();
    #[cfg(unix)]
    workspace_missed_baseline_does_not_replay_an_alert();
    #[cfg(unix)]
    workspace_restores_an_open_sidebar_from_the_local_view();
    #[cfg(unix)]
    workspace_mouse_clicks_focus_panes_and_select_tabs();
    #[cfg(unix)]
    workspace_a_fresh_press_leaves_a_selection_and_routes_normally();
    #[cfg(unix)]
    workspace_an_ordinary_key_leaves_selection_and_reaches_the_program();
    #[cfg(unix)]
    workspace_a_selection_exit_key_forwards_its_release();
    #[cfg(unix)]
    workspace_a_drag_selection_extends_and_copies_the_range();
    #[cfg(unix)]
    workspace_dragging_a_split_sends_exact_resize_frames();
    #[cfg(unix)]
    workspace_wheel_scrolls_local_history_and_any_key_returns_live();
    #[cfg(unix)]
    workspace_the_live_cursor_is_hidden_in_history_and_returns_at_the_bottom();
    #[cfg(unix)]
    workspace_forwards_mouse_to_a_tracking_program_and_consumes_chrome();
    #[cfg(unix)]
    workspace_shift_drag_selects_locally_over_a_tracking_program();
    #[cfg(unix)]
    workspace_shift_click_is_local_over_a_tracking_program();
    #[cfg(unix)]
    workspace_overlays_consume_mouse_without_input_or_resize();
    #[cfg(unix)]
    workspace_palette_runs_a_command_and_view_commands_do_not_write();
    #[cfg(unix)]
    workspace_search_is_local_and_jumps_to_a_match();
    #[cfg(unix)]
    workspace_copy_emits_osc52_and_empty_selection_copies_nothing();
    #[cfg(unix)]
    workspace_paste_is_bracketed_and_overlays_consume_it();
    #[cfg(unix)]
    workspace_paste_frames_only_when_the_program_enabled_it();
    #[cfg(unix)]
    workspace_config_sets_theme_and_keys_and_malformed_config_warns();
    #[cfg(unix)]
    workspace_processes_a_large_output_burst();
    #[cfg(unix)]
    observer_attach_negotiates_resize_notifications_and_applies_them_in_order();
    #[cfg(unix)]
    observer_attach_is_unavailable_without_the_feature_and_normal_attach_is_unchanged();
    #[cfg(unix)]
    observer_attach_requires_a_confirmed_reply_and_leaves_no_channel();
    #[cfg(unix)]
    a_malformed_resize_ends_the_connection_without_a_panic();
    #[cfg(unix)]
    screen_applies_a_resize_before_its_data_and_sends_none_back();
    #[cfg(unix)]
    workspace_applies_a_resize_before_its_data_and_sends_none_back();
    #[cfg(unix)]
    an_early_resize_before_the_reply_keeps_the_live_route();
    #[cfg(unix)]
    a_screen_observer_sends_no_input_and_no_echo();
    #[cfg(unix)]
    a_workspace_observer_sends_no_keyboard_or_mouse_input();
    println!("fake-core tests passed");
}

// --- scripted screen tests -------------------------------------------------

/// One `ht screen` under a PTY, against the scripted core.
#[cfg(unix)]
struct FakeScreen {
    pty: common::pty::Pty,
    probe: Probe,
    output: Capture,
    stderr: Capture,
    input: std::fs::File,
    cols: u16,
    rows: u16,
}

#[cfg(unix)]
impl FakeScreen {
    fn spawn(scratch: &Scratch, scenario: &str, args: &[&str]) -> FakeScreen {
        Self::spawn_parts(scratch, scenario, &["screen"], args, None)
    }

    /// `ht` with no subcommand: the default workspace, against the same
    /// scripted core.
    fn spawn_workspace(scratch: &Scratch, scenario: &str) -> FakeScreen {
        Self::spawn_parts(scratch, scenario, &[], &[], None)
    }

    /// A second screen on the same core, with its own local view.
    fn spawn_workspace_as(scratch: &Scratch, scenario: &str, screen: &str) -> FakeScreen {
        Self::spawn_parts(scratch, scenario, &[], &[], Some(screen))
    }

    /// The default workspace at a size wide enough for untruncated notices.
    fn spawn_workspace_wide(scratch: &Scratch, scenario: &str, cols: u16, rows: u16) -> FakeScreen {
        Self::spawn_parts_size(scratch, scenario, &[], &[], None, cols, rows)
    }

    fn spawn_parts(
        scratch: &Scratch,
        scenario: &str,
        leading: &[&str],
        args: &[&str],
        screen: Option<&str>,
    ) -> FakeScreen {
        Self::spawn_parts_size(scratch, scenario, leading, args, screen, 80, 24)
    }

    fn spawn_parts_size(
        scratch: &Scratch,
        scenario: &str,
        leading: &[&str],
        args: &[&str],
        screen: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> FakeScreen {
        let exe = std::env::current_exe().expect("test executable");
        let pty = common::pty::Pty::open(cols, rows);
        let (stdin, stdout) = pty.stdio();
        let mut command = Command::new(HT);
        command
            .args(leading)
            .arg("--hc")
            .arg(&exe)
            .args(["--cols", &cols.to_string(), "--rows", &rows.to_string()])
            .args(args)
            .env("FAKE_SCENARIO", scenario)
            .env("FAKE_LOG", scratch.log())
            .env("HOME", &scratch.dir)
            .env("XDG_STATE_HOME", scratch.state())
            .env("XDG_CONFIG_HOME", scratch.config_root())
            // Deterministic chrome colors in the captured frames.
            .env("COLORTERM", "truecolor")
            .env("TERM", "xterm-256color");
        if let Some(screen) = screen {
            command.env("HERDR_TERMINAL_SCREEN", screen);
        }
        let mut child = command
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn ht");
        let output = Capture::spawn(pty.reader());
        let stderr = Capture::spawn(child.stderr.take().expect("stderr"));
        let input = pty.writer();
        let probe = Probe::new(child);
        FakeScreen {
            pty,
            probe,
            output,
            stderr,
            input,
            cols,
            rows,
        }
    }

    fn wait_text(&self, needle: &str, timeout: std::time::Duration) -> bool {
        common::wait_until(timeout, || self.rendered().contains(needle))
    }

    fn rendered(&self) -> String {
        let mut vt = herdr_terminal::vt::Vt::new(self.cols, self.rows).expect("local VT");
        vt.feed(&self.output.bytes());
        vt.snapshot().expect("snapshot").lines().join("\n")
    }

    /// The outer terminal's cursor after replaying everything the client drew.
    fn cursor(&self) -> herdr_terminal::vt::Cursor {
        let mut vt = herdr_terminal::vt::Vt::new(self.cols, self.rows).expect("local VT");
        vt.feed(&self.output.bytes());
        vt.snapshot().expect("snapshot").cursor
    }

    fn send(&mut self, bytes: &[u8]) {
        self.input.write_all(bytes).expect("write to the screen");
        self.input.flush().expect("flush the screen");
    }

    /// The workspace prefix (Ctrl-b) followed by one command key.
    fn command(&mut self, key: u8) {
        self.send(&[0x02, key]);
    }

    fn wait_exit(&mut self, timeout: std::time::Duration) -> Option<std::process::ExitStatus> {
        self.probe.wait_timeout(timeout)
    }

    fn tail(&self) -> String {
        let raw = self.output.bytes();
        String::from_utf8_lossy(&raw[raw.len().saturating_sub(400)..]).into_owned()
    }

    /// The raw outer-terminal bytes, including OSC sequences such as OSC 52.
    fn raw(&self) -> String {
        String::from_utf8_lossy(&self.output.bytes()).into_owned()
    }

    fn errors(&self) -> String {
        self.stderr.text()
    }
}

/// DATA and CLOSE in one attach burst: the screen must draw the final frame
/// (past its frame throttle) before it leaves the alternate screen.
#[cfg(unix)]
fn screen_draws_the_final_frame_when_data_and_close_arrive_together() {
    let scratch = Scratch::new("screen-final");
    let mut screen = FakeScreen::spawn(&scratch, "screen-final", &["--", "sh", "-c", "sleep 30"]);

    let status = screen
        .wait_exit(secs(10))
        .expect("ht screen exits after the scripted CLOSE");
    assert_eq!(
        status.code(),
        Some(0),
        "ht screen exit {status}; stderr={:?}",
        screen.stderr.text()
    );
    let raw = screen.output.bytes();
    assert!(
        screen.output.contains(b"READY"),
        "the final frame is missing READY; raw={:?}",
        String::from_utf8_lossy(&raw)
    );
    assert!(
        screen.output.contains(b"END"),
        "the final frame is missing END; raw={:?}",
        String::from_utf8_lossy(&raw)
    );
    let end = rfind(&raw, b"END").expect("END was drawn");
    let leave = rfind(&raw, b"\x1b[?1049l").expect("the alternate screen was left");
    assert!(
        end < leave,
        "the final frame must be drawn before the screen is restored; raw={:?}",
        String::from_utf8_lossy(&raw)
    );
    assert!(
        screen.pty.is_cooked(),
        "ht screen did not restore the terminal after the program ended"
    );
    let log = scratch.entries();
    assert!(
        log.iter().any(|entry| entry["call"] == "terminal.rm"),
        "the finished record must be removed; log={log:?}"
    );
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.kill"),
        "a program that ended by itself must not be killed; log={log:?}"
    );
}

/// A program that exited before the workspace's first attach: the pane must
/// show core's kept final screen and the finished record's exact status, stay
/// on screen, and only remove the record when the person closes it.
#[cfg(unix)]
fn workspace_keeps_the_final_screen_and_status_when_the_program_exited_before_attach() {
    let scratch = Scratch::new("workspace-finished");
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "finished");

    assert!(
        workspace.wait_text("FAST", secs(10)),
        "the recovered final screen was not drawn; raw={:?} stderr={:?}",
        workspace.output.text(),
        workspace.stderr.text()
    );
    assert!(
        workspace.wait_text("exited (7)", secs(5)),
        "the finished record's status was not shown; screen={:?} raw={:?}",
        workspace.rendered(),
        workspace.output.text()
    );
    assert!(
        !workspace.rendered().contains("no terminals in this tab"),
        "the finished pane must stay visible; screen={:?}",
        workspace.rendered()
    );

    let log = scratch.entries();
    assert!(
        log.iter().any(|entry| entry["call"] == "terminal.get"),
        "the finished record must be read; log={log:?}"
    );
    assert!(
        log.iter().any(|entry| entry["call"] == "terminal.read"),
        "the final screen must be read; log={log:?}"
    );
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.rm"),
        "the record must not be removed before the screen has been shown; log={log:?}"
    );

    // x closes the finished view; only now is its record removed.
    workspace.command(b'x');
    assert!(
        common::wait_until(secs(5), || scratch
            .entries()
            .iter()
            .any(|entry| entry["call"] == "terminal.rm")),
        "closing the finished pane must remove its record; log={:?}",
        scratch.entries()
    );
    assert!(
        workspace.wait_text("no terminals in this tab", secs(5)),
        "the empty state did not follow the close; screen={:?}",
        workspace.rendered()
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
    assert!(workspace.pty.is_cooked(), "the terminal was not restored");
}

/// DATA and CLOSE in one attach burst: the workspace must draw the final text
/// and the exact status before the record is removed.
#[cfg(unix)]
fn workspace_draws_the_final_frame_and_status_when_data_and_close_arrive_together() {
    let scratch = Scratch::new("workspace-final");
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "workspace-final");

    assert!(
        workspace.wait_text("READY", secs(10)),
        "the final frame is missing READY; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    assert!(
        workspace.wait_text("END", secs(5)),
        "the final frame is missing END; screen={:?}",
        workspace.rendered()
    );
    assert!(
        workspace.wait_text("exited (7)", secs(5)),
        "the close reason's status was not shown; screen={:?}",
        workspace.rendered()
    );
    assert!(
        !workspace.rendered().contains("no terminals in this tab"),
        "the finished pane was removed before its final frame; screen={:?}",
        workspace.rendered()
    );
    let log = scratch.entries();
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.rm"),
        "the record must not be removed before the final frame is shown; log={log:?}"
    );

    workspace.command(b'x');
    assert!(
        common::wait_until(secs(5), || scratch
            .entries()
            .iter()
            .any(|entry| entry["call"] == "terminal.rm")),
        "closing the finished pane must remove its record; log={:?}",
        scratch.entries()
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
    assert!(workspace.pty.is_cooked(), "the terminal was not restored");
}

/// The connection dies after core creates a split's terminal but before the
/// `terminal.run` reply reaches the client. Reconnect must reclaim that
/// terminal by its unique name, not start a duplicate or lose the pane.
#[cfg(unix)]
fn workspace_reclaims_a_start_whose_run_reply_was_lost() {
    let scratch = Scratch::new("workspace-pending");
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "workspace-pending");
    assert!(
        workspace.wait_text("PANE-ONE", secs(10)),
        "the first pane was not drawn; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );

    // The split's terminal.run is answered by core (it exists in the next
    // snapshot) but the reply never arrives; this is the race the test at
    // tests/workspace.rs:reconnecting... used to hit.
    workspace.command(b'v');
    assert!(
        workspace.wait_text("reconnected to core", secs(20)),
        "the workspace did not reconnect; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    assert!(
        workspace.wait_text("PANE-TWO", secs(10)),
        "the created pane was not reclaimed; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );

    let log = scratch.entries();
    let runs: Vec<&serde_json::Value> = log
        .iter()
        .filter(|entry| entry["call"] == "terminal.run")
        .collect();
    assert_eq!(
        runs.len(),
        2,
        "the interrupted start must not run a second terminal; log={log:?}"
    );
    let attaches: Vec<&serde_json::Value> = log
        .iter()
        .filter(|entry| entry.get("terminal").is_some())
        .collect();
    assert_eq!(
        attaches
            .iter()
            .filter(|entry| entry["terminal"] == "t2")
            .count(),
        1,
        "the reclaimed t2 must be attached once; log={log:?}"
    );
    assert_eq!(
        attaches
            .iter()
            .filter(|entry| entry["terminal"] == "t1")
            .count(),
        2,
        "t1 is attached once per connection; log={log:?}"
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
    assert!(workspace.pty.is_cooked(), "the terminal was not restored");
}

/// `missed` carries a fresh snapshot with both panes: each pane reattaches and
/// no terminal is started again.
#[cfg(unix)]
fn workspace_reattaches_every_pane_after_missed() {
    let scratch = Scratch::new("workspace-missed");
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "workspace-missed");

    assert!(
        workspace.wait_text("PANE-ONE", secs(10)),
        "the first pane was not drawn; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    workspace.command(b'v');
    // `missed` follows the second attach, so the split pane's first paint may
    // be replaced by its reattach paint before a poll can see it.
    assert!(
        workspace.wait_text("ONE-AGAIN", secs(10)),
        "the first pane did not reattach after missed; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    assert!(
        workspace.wait_text("TWO-AGAIN", secs(10)),
        "the second pane did not reattach after missed; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );

    let log = scratch.entries();
    assert_eq!(
        log.iter()
            .filter(|entry| entry["call"] == "terminal.run")
            .count(),
        2,
        "a resnapshot must not start terminals; log={log:?}"
    );
    for target in ["t1", "t2"] {
        assert_eq!(
            log.iter()
                .filter(|entry| entry.get("attached").is_some() && entry["terminal"] == target)
                .count(),
            2,
            "{target} must attach once per snapshot; log={log:?}"
        );
    }

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
    assert!(workspace.pty.is_cooked(), "the terminal was not restored");
}

/// Ctrl-] detaches: the attach stream is closed, the program keeps running,
/// and the record is neither killed nor removed.
#[cfg(unix)]
fn screen_detaches_without_killing_the_program() {
    let scratch = Scratch::new("screen-detach");
    let mut screen = FakeScreen::spawn(&scratch, "screen-detach", &["--", "sh", "-c", "sleep 30"]);
    assert!(
        screen.wait_text("READY", secs(10)),
        "the first frame was not drawn; raw={:?} stderr={:?}",
        screen.output.text(),
        screen.stderr.text()
    );

    screen.send(&[0x1d]);
    let status = screen.wait_exit(secs(10)).expect("ht exits after Ctrl-]");
    assert_eq!(
        status.code(),
        Some(0),
        "a detach is not an error; stderr={:?}",
        screen.stderr.text()
    );
    assert!(
        screen.pty.is_cooked(),
        "the terminal was not restored after a detach"
    );

    let log = scratch.entries();
    // The client may exit before the fake reads the CLOSE frame it wrote: the
    // connection closing is what core sees either way. What must not happen is
    // a kill or a removal.
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.kill"),
        "a detach must leave the program running; log={log:?}"
    );
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.rm"),
        "a detach must leave the record in place; log={log:?}"
    );
    let run = log
        .iter()
        .find(|entry| entry["call"] == "terminal.run")
        .expect("the screen started a terminal");
    assert_eq!(
        run["params"]["keep"],
        json!(true),
        "the screen's terminal must be kept"
    );
    assert_eq!(run["params"]["rm"], json!(false));
    assert_eq!(
        run["params"]["labels"]["herdr.client"],
        json!("herdr-terminal"),
        "the terminal must carry the client label"
    );
    assert_eq!(
        run["params"]["size"],
        json!({"cols": 80, "rows": 24}),
        "the screen must start the program at the grid it attaches at"
    );
}

/// A kept terminal is lost, stays unattachable until core restarts it, and
/// the client reattaches to the same address instead of starting a duplicate.
#[cfg(unix)]
fn screen_waits_for_a_restarted_terminal() {
    let scratch = Scratch::new("screen-lost-restarted");
    let mut screen = FakeScreen::spawn(&scratch, "screen-lost-restarted", &[]);
    assert!(
        screen.wait_text("BEFORE", secs(10)),
        "the adopted terminal was not drawn; raw={:?} stderr={:?}",
        screen.output.text(),
        screen.stderr.text()
    );

    // A key on the stream triggers the scripted loss: core reports `lost`,
    // closes the stream, and only restarts the terminal after an attach
    // finds it parked.
    screen.send(b"g");
    assert!(
        screen.wait_text("RESTARTED", secs(10)),
        "the client did not reattach after `restarted`; raw tail={:?} stderr={:?}",
        screen.tail(),
        screen.stderr.text()
    );

    let log = scratch.entries();
    let attached: Vec<&serde_json::Value> = log
        .iter()
        .filter(|entry| entry.get("attached").is_some())
        .collect();
    assert_eq!(
        attached.len(),
        2,
        "one attach before the loss and one after the restart; log={log:?}"
    );
    assert_eq!(attached[0]["attached"], json!(5));
    assert_eq!(attached[1]["attached"], json!(6));
    for entry in &attached {
        assert_eq!(
            entry["terminal"],
            json!("t1"),
            "the client must reattach the same address; log={log:?}"
        );
    }
    assert!(
        log.iter()
            .any(|entry| entry.get("attach_unavailable").is_some()),
        "a parked terminal must not be attachable before `restarted`; log={log:?}"
    );
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.run"),
        "an adopted terminal must never be started again; log={log:?}"
    );

    screen.send(&[0x1d]);
    let status = screen.wait_exit(secs(10)).expect("ht exits after Ctrl-]");
    assert_eq!(status.code(), Some(0), "stderr={:?}", screen.stderr.text());
    assert!(screen.pty.is_cooked());
    let log = scratch.entries();
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.kill"));
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.rm"));
}

/// `missed` carries a fresh snapshot: the client replaces its state and
/// reattaches the live terminal rather than trusting the old stream.
#[cfg(unix)]
fn screen_resnapshots_after_missed_events() {
    let scratch = Scratch::new("screen-missed");
    let mut screen = FakeScreen::spawn(&scratch, "screen-missed", &[]);
    assert!(
        screen.wait_text("BEFORE", secs(10)),
        "the adopted terminal was not drawn; raw={:?} stderr={:?}",
        screen.output.text(),
        screen.stderr.text()
    );

    screen.send(b"g");
    assert!(
        screen.wait_text("RESYNCED", secs(10)),
        "the client did not reattach after `missed`; raw tail={:?} stderr={:?}",
        screen.tail(),
        screen.stderr.text()
    );

    let log = scratch.entries();
    let attached: Vec<&serde_json::Value> = log
        .iter()
        .filter(|entry| entry.get("attached").is_some())
        .collect();
    assert_eq!(
        attached.len(),
        2,
        "the missed snapshot must lead to a fresh attach; log={log:?}"
    );
    assert_eq!(attached[1]["attached"], json!(6));
    assert_eq!(attached[1]["terminal"], json!("t1"));
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.run"),
        "a resnapshot must not start a duplicate terminal; log={log:?}"
    );
    assert!(
        log.iter()
            .any(|entry| { entry["frame"]["channel"] == 5 && entry["frame"]["kind"] == CLOSE }),
        "the replaced stream must be closed before the fresh attach; log={log:?}"
    );

    screen.send(&[0x1d]);
    let status = screen.wait_exit(secs(10)).expect("ht exits after Ctrl-]");
    assert_eq!(status.code(), Some(0), "stderr={:?}", screen.stderr.text());
    assert!(screen.pty.is_cooked());
    let log = scratch.entries();
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.kill"));
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.rm"));
}

/// A fresh repaint after the viewer fell behind starts with a soft reset and
/// a clear; the old screen must be gone, not merely overdrawn.
#[cfg(unix)]
fn screen_repaints_without_stale_cells() {
    let scratch = Scratch::new("screen-repaint");
    let mut screen = FakeScreen::spawn(&scratch, "screen-repaint", &[]);
    assert!(
        screen.wait_text("BEFORE", secs(10)),
        "the adopted terminal was not drawn; raw={:?} stderr={:?}",
        screen.output.text(),
        screen.stderr.text()
    );

    screen.send(b"g");
    assert!(
        screen.wait_text("REPAINTED", secs(10)),
        "the repaint was not drawn; raw tail={:?} stderr={:?}",
        screen.tail(),
        screen.stderr.text()
    );
    assert!(
        !screen.rendered().contains("BEFORE"),
        "a fresh repaint must clear the stale screen; grid={:?}",
        screen.rendered()
    );

    screen.send(&[0x1d]);
    let status = screen.wait_exit(secs(10)).expect("ht exits after Ctrl-]");
    assert_eq!(status.code(), Some(0), "stderr={:?}", screen.stderr.text());
    assert!(screen.pty.is_cooked());
    let log = scratch.entries();
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.kill"));
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.rm"));
}

/// A `removed` event ends the screen with a clear failure; there is nothing
/// left to kill or remove.
#[cfg(unix)]
fn screen_exits_when_the_terminal_is_removed() {
    let scratch = Scratch::new("screen-removed");
    let mut screen = FakeScreen::spawn(&scratch, "screen-removed", &[]);
    assert!(
        screen.wait_text("BEFORE", secs(10)),
        "the adopted terminal was not drawn; raw={:?} stderr={:?}",
        screen.output.text(),
        screen.stderr.text()
    );

    screen.send(b"g");
    let status = screen
        .wait_exit(secs(10))
        .expect("ht exits when its terminal is removed");
    assert_eq!(
        status.code(),
        Some(1),
        "a removed terminal is a failure; stderr={:?}",
        screen.stderr.text()
    );
    assert!(
        screen.stderr.wait_for(b"was removed", secs(2)),
        "the failure must name what happened; stderr={:?}",
        screen.stderr.text()
    );
    assert!(
        screen.pty.is_cooked(),
        "the terminal was not restored after removal"
    );
    let log = scratch.entries();
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.kill"),
        "a removed terminal must not be killed; log={log:?}"
    );
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.rm"),
        "a removed terminal must not be removed again; log={log:?}"
    );
}

/// When the connection dies, a new `hc connect` is made, a new snapshot is
/// taken, and the same terminal is reattached: the program never restarts.
#[cfg(unix)]
fn screen_reconnects_to_the_same_terminal() {
    let scratch = Scratch::new("screen-reconnect");
    let mut screen = FakeScreen::spawn(&scratch, "screen-reconnect", &[]);
    assert!(
        screen.wait_text("BEFORE", secs(10)),
        "the adopted terminal was not drawn; raw={:?} stderr={:?}",
        screen.output.text(),
        screen.stderr.text()
    );

    // The key makes generation 1 drop the connection; generation 2 answers
    // with a snapshot that still has t1 live.
    screen.send(b"g");
    assert!(
        screen.wait_text("RECONNECTED", secs(15)),
        "the client did not reconnect; raw tail={:?} stderr={:?}",
        screen.tail(),
        screen.stderr.text()
    );

    let log = scratch.entries();
    let generations: Vec<u64> = log
        .iter()
        .filter_map(|entry| entry["generation"].as_u64())
        .collect();
    assert!(
        generations.contains(&1) && generations.contains(&2),
        "both connections must have run; log={log:?}"
    );
    let attached: Vec<&serde_json::Value> = log
        .iter()
        .filter(|entry| entry.get("attached").is_some())
        .collect();
    assert_eq!(attached.len(), 2, "one attach per connection; log={log:?}");
    for entry in &attached {
        assert_eq!(
            entry["terminal"],
            json!("t1"),
            "the reconnect must adopt the same address; log={log:?}"
        );
    }
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.run"),
        "a reconnect must never start a duplicate terminal; log={log:?}"
    );

    screen.send(&[0x1d]);
    let status = screen.wait_exit(secs(10)).expect("ht exits after Ctrl-]");
    assert_eq!(status.code(), Some(0), "stderr={:?}", screen.stderr.text());
    assert!(screen.pty.is_cooked());
    let log = scratch.entries();
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.kill"));
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.rm"));
}

/// Both the first `ht screen` start and the terminal it starts after a
/// reconnect finds none tell core the viewport they will attach at.
#[cfg(unix)]
fn screen_sends_the_viewport_size_when_it_starts_and_replaces_a_terminal() {
    let scratch = Scratch::new("screen-reconnect-gone");
    let mut screen = FakeScreen::spawn(&scratch, "screen-reconnect-gone", &[]);
    assert!(
        screen.wait_text("BEFORE", secs(10)),
        "the first frame was not drawn; raw={:?} stderr={:?}",
        screen.output.text(),
        screen.stderr.text()
    );

    let runs = || {
        scratch
            .entries()
            .into_iter()
            .filter(|entry| entry["call"] == "terminal.run")
            .collect::<Vec<_>>()
    };
    let first = runs();
    assert_eq!(first.len(), 1, "the screen must start one terminal");
    assert_eq!(
        first[0]["params"]["size"],
        json!({"cols": 80, "rows": 24}),
        "the first terminal must start at the viewport"
    );

    // Generation 1 drops the connection; generation 2 has no terminal, so the
    // screen starts a replacement at the size it will attach at.
    screen.send(b"g");
    assert!(
        common::wait_until(secs(15), || runs().len() == 2),
        "the screen did not start a replacement; log={:?}",
        scratch.entries()
    );
    let runs = runs();
    assert_eq!(
        runs[1]["params"]["size"],
        json!({"cols": 80, "rows": 24}),
        "the replacement terminal must start at the viewport"
    );

    screen.send(&[0x1d]);
    let status = screen.wait_exit(secs(10)).expect("ht exits after Ctrl-]");
    assert_eq!(status.code(), Some(0), "stderr={:?}", screen.stderr.text());
}

/// Restores the shared document's spaces, tabs, split layout and panes, and
/// adopts the named terminals instead of starting new ones. A clean restore
/// writes nothing, and detaching leaves the document and the programs alone.
#[cfg(unix)]
fn workspace_restores_a_document_without_starting_terminals() {
    let scratch = Scratch::new("doc-restore");
    scratch.seed_doc(doc_same_tab(), 7);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-restore");

    assert!(
        workspace.wait_text("PANE-ONE", secs(10)) && workspace.wait_text("PANE-TWO", secs(10)),
        "both documented panes must be restored and drawn; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    assert!(
        workspace.rendered().contains("shared") && workspace.rendered().contains("main"),
        "the space and tab names come from the document; screen={:?}",
        workspace.rendered()
    );

    let log = scratch.entries();
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.run"),
        "a restored pane must never start a replacement; log={log:?}"
    );
    for target in ["t1", "t2"] {
        assert_eq!(
            log.iter()
                .filter(|entry| entry.get("attached").is_some() && entry["terminal"] == target)
                .count(),
            1,
            "{target} must be attached once; log={log:?}"
        );
    }
    assert_eq!(
        scratch.doc_version(),
        7,
        "an unchanged restore must not write the document"
    );

    // x on a restored pane ends it like any workspace-owned, non-kept pane,
    // even though the Session itself is adopted (a local error only detaches).
    workspace.command(b'x');
    assert!(
        common::wait_until(secs(10), || {
            doc_addresses(&scratch.doc_value()) == vec!["t2".to_owned()]
        }),
        "closing a restored pane must update the document; doc={:?}",
        scratch.doc_value()
    );
    assert!(
        common::wait_until(secs(10), || {
            scratch.entries().iter().any(|entry| {
                entry["call"] == "terminal.kill" && entry["params"]["terminal"] == "t1"
            })
        }),
        "closing a restored pane must end its program; log={:?}",
        scratch.entries()
    );
    assert!(
        common::wait_until(secs(10), || {
            scratch
                .entries()
                .iter()
                .any(|entry| entry["call"] == "terminal.rm" && entry["params"]["terminal"] == "t1")
        }),
        "closing a restored pane must remove its record; log={:?}",
        scratch.entries()
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
    assert!(workspace.pty.is_cooked());
    let log = scratch.entries();
    assert!(
        !log.iter().any(|entry| {
            entry["params"]["terminal"] == "t2"
                && (entry["call"] == "terminal.kill" || entry["call"] == "terminal.rm")
        }),
        "detaching must leave the remaining program alone; log={log:?}"
    );
    assert_eq!(
        doc_addresses(&scratch.doc_value()),
        vec!["t2".to_owned()],
        "detaching must leave the remaining pane in the document"
    );
}

/// Focus, zoom and the active tab are local: the second screen opens the tab
/// its view names, while the shared document is untouched.
#[cfg(unix)]
fn workspace_applies_the_local_view() {
    let scratch = Scratch::new("doc-view");
    scratch.seed_doc(doc_two_tabs(), 3);
    scratch.seed_view(
        "work",
        json!({
            "version": 1,
            "active_space": "1",
            "active_tabs": {"1": "31"},
            "tabs": {
                "30": {"focused": "3", "zoomed": null, "history": ["3"]},
                "31": {"focused": "4", "zoomed": null, "history": ["4"]},
            },
        }),
    );
    let mut workspace = FakeScreen::spawn_workspace_as(&scratch, "doc-view", "work");

    assert!(
        workspace.wait_text("TAB-TWO", secs(10)),
        "the view's active tab must be shown; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    assert!(
        !workspace.rendered().contains("TAB-ONE"),
        "a pane in a background tab is not drawn; screen={:?}",
        workspace.rendered()
    );

    workspace.command(b'1');
    assert!(
        workspace.wait_text("TAB-ONE", secs(5)),
        "selecting the first tab shows its pane; screen={:?}",
        workspace.rendered()
    );
    assert_eq!(
        scratch.doc_version(),
        3,
        "the local view must not be written to the shared document"
    );
    assert!(
        !scratch
            .entries()
            .iter()
            .any(|entry| entry["call"] == "terminal.run"),
        "the local view must not start terminals"
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// Focus and zoom change only the local view: no document write, no version
/// bump, including the final detach flush.
#[cfg(unix)]
fn workspace_focus_and_zoom_do_not_write_the_document() {
    let scratch = Scratch::new("doc-focus");
    scratch.seed_doc(doc_same_tab(), 7);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-focus");
    assert!(
        workspace.wait_text("PANE-ONE", secs(10)) && workspace.wait_text("PANE-TWO", secs(10)),
        "both panes must be restored; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );

    workspace.command(b'h');
    workspace.command(b'z');
    assert!(
        workspace.wait_text("PANE-ONE", secs(5))
            && common::wait_until(secs(5), || !workspace.rendered().contains("PANE-TWO")),
        "zoom must show only the focused pane; screen={:?}",
        workspace.rendered()
    );
    workspace.command(b'z');
    assert!(
        workspace.wait_text("PANE-TWO", secs(5)),
        "unzooming must show both panes; screen={:?}",
        workspace.rendered()
    );

    let log = scratch.entries();
    assert!(
        !log.iter().any(|entry| entry["call"] == "doc.set"),
        "focus and zoom must not write the shared document; log={log:?}"
    );
    assert_eq!(scratch.doc_version(), 7);

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
    assert_eq!(
        scratch.doc_version(),
        7,
        "the detach flush must not write a view-only change"
    );
    assert!(
        !scratch
            .entries()
            .iter()
            .any(|entry| entry["call"] == "doc.set"),
        "no document write for focus or zoom"
    );
}

/// A terminal rediscovered under a new address is matched by its unique pane
/// label; the document is repaired with the new address.
#[cfg(unix)]
fn workspace_repairs_a_rediscovered_terminal() {
    let scratch = Scratch::new("doc-mismatch");
    scratch.seed_doc(one_tab_doc(&[(3, "t9")]), 2);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-mismatch");

    assert!(
        workspace.wait_text("PANE-ONE", secs(10)),
        "the terminal found by label must be drawn; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    assert!(
        common::wait_until(secs(5), || {
            let text = scratch.doc_value().to_string();
            text.contains("\"t1\"") && !text.contains("\"t9\"")
        }),
        "the repaired address must be written back; doc={:?}",
        scratch.doc_value()
    );
    assert_eq!(scratch.doc_version(), 3, "one repaired write");
    let log = scratch.entries();
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.run"));
    assert!(
        log.iter()
            .any(|entry| entry.get("attached").is_some() && entry["terminal"] == "t1"),
        "the rediscovered terminal is attached by its current address; log={log:?}"
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// A pane whose terminal no longer exists is pruned; the tab stays and the
/// pruned pane is removed from the document.
#[cfg(unix)]
fn workspace_prunes_a_pane_whose_terminal_is_gone() {
    let scratch = Scratch::new("doc-missing");
    scratch.seed_doc(one_tab_doc(&[(3, "t1"), (4, "t9")]), 2);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-missing");

    assert!(
        workspace.wait_text("PANE-ONE", secs(10)),
        "the live pane must be drawn; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    assert!(
        common::wait_until(secs(5), || !scratch.doc_value().to_string().contains("t9")),
        "the pruned pane must leave the document; doc={:?}",
        scratch.doc_value()
    );
    assert_eq!(scratch.doc_version(), 3, "one repair write");
    assert_eq!(
        doc_addresses(&scratch.doc_value()),
        vec!["t1".to_owned()],
        "only the live pane remains"
    );
    let log = scratch.entries();
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.run"));
    assert!(
        log.iter().any(|entry| {
            entry["call"] == "terminal.get" && entry["params"]["terminal"] == "t9"
        }),
        "a missing address must be checked with terminal.get before pruning; log={log:?}"
    );
    assert!(
        !log.iter()
            .any(|entry| entry.get("attached").is_some() && entry["terminal"] == "t9"),
        "a missing terminal must not be attached; log={log:?}"
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// The document names a terminal that the subscription snapshot predates
/// (another screen created it just after this screen subscribed). Startup
/// must check the address with terminal.get, keep the live pane, and never
/// write a prune over the shared document.
#[cfg(unix)]
fn workspace_keeps_a_pane_missing_from_the_snapshot() {
    let scratch = Scratch::new("doc-stale-snapshot");
    scratch.seed_doc(doc_same_tab(), 7);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-stale-snapshot");

    assert!(
        workspace.wait_text("PANE-ONE", secs(10)) && workspace.wait_text("PANE-TWO", secs(10)),
        "the pane newer than the snapshot must be restored; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    let log = scratch.entries();
    assert!(
        log.iter().any(|entry| {
            entry["call"] == "terminal.get" && entry["params"]["terminal"] == "t2"
        }),
        "the document address absent from the snapshot must be looked up; log={log:?}"
    );
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.run"),
        "a live documented pane must never be replaced; log={log:?}"
    );
    assert!(
        log.iter()
            .any(|entry| { entry.get("attached").is_some() && entry["terminal"] == "t2" }),
        "the pane restored from the lookup must be attached; log={log:?}"
    );
    assert_eq!(
        scratch.doc_version(),
        7,
        "a clean restore must not prune the pane the snapshot missed; doc={:?}",
        scratch.doc_value()
    );
    assert_eq!(
        doc_addresses(&scratch.doc_value()),
        vec!["t1".to_owned(), "t2".to_owned()]
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// A terminal at the same address but without this client's pane label is not
/// adopted; the pane is pruned rather than claiming a stranger's program.
#[cfg(unix)]
fn workspace_never_adopts_a_terminal_it_did_not_label() {
    let scratch = Scratch::new("doc-foreign");
    scratch.seed_doc(one_tab_doc(&[(3, "t1")]), 2);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-foreign");

    assert!(
        workspace.wait_text("no terminals in this tab", secs(10)),
        "the foreign terminal was not adopted; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    assert!(
        common::wait_until(secs(5), || {
            scratch.doc_value()["spaces"][0]["tabs"][0]["layout"] == json!(null)
        }),
        "the pruned pane must leave the document; doc={:?}",
        scratch.doc_value()
    );
    let log = scratch.entries();
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.run"));
    assert!(
        !log.iter().any(|entry| entry.get("attached").is_some()),
        "a foreign terminal must not be attached; log={log:?}"
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// With no document, the first screen claims an empty one at version 0, starts
/// its shell with the pane label, and saves the layout.
#[cfg(unix)]
fn workspace_claims_a_new_document_and_labels_its_terminal() {
    let scratch = Scratch::new("doc-fresh");
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-fresh");

    assert!(
        workspace.wait_text("PANE-ONE", secs(10)),
        "the first shell was not started; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    assert!(
        common::wait_until(secs(5), || doc_addresses(&scratch.doc_value())
            == vec!["t1"]),
        "the claimed document must gain the first pane; doc={:?}",
        scratch.doc_value()
    );
    let run = scratch
        .entries()
        .into_iter()
        .find(|entry| entry["call"] == "terminal.run")
        .expect("one terminal was started");
    let label = run["params"]["labels"]["herdr.terminal.pane"]
        .as_str()
        .expect("the pane label was sent with terminal.run")
        .to_owned();
    assert_eq!(
        scratch.doc_value()["spaces"][0]["tabs"][0]["layout"]["leaf"]["id"],
        json!(label),
        "the document pane id is the label the terminal carries"
    );
    assert_eq!(
        scratch.doc_value()["spaces"][0]["tabs"][0]["layout"]["leaf"]["labels"]["herdr.terminal.pane"],
        json!(label)
    );
    assert_eq!(
        scratch
            .entries()
            .iter()
            .filter(|entry| entry["call"] == "terminal.run")
            .count(),
        1,
        "the initial shell must start once"
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
    let log = scratch.entries();
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.kill"));
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.rm"));
    assert_eq!(
        doc_addresses(&scratch.doc_value()),
        vec!["t1".to_owned()],
        "detaching leaves the program and the document intact"
    );
}

/// Every new workspace terminal tells core the grid it will attach at: the
/// first shell fills the viewport's pane, and a split gets its half before
/// its first attach. The fake core ignores the field, like older cores.
#[cfg(unix)]
fn workspace_sends_each_new_terminals_attach_size_to_run() {
    let scratch = Scratch::new("workspace-run-size");
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "m6-copy");
    assert!(
        wait_for_text(&workspace, "hello"),
        "the first pane did not paint; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );

    let runs = || {
        scratch
            .entries()
            .into_iter()
            .filter(|entry| entry["call"] == "terminal.run")
            .collect::<Vec<_>>()
    };
    let initial = runs();
    assert_eq!(initial.len(), 1, "only the initial shell has run");
    assert_eq!(
        initial[0]["params"]["size"],
        json!({"cols": 78, "rows": 20}),
        "the initial shell must fill the pane inside the 80x24 viewport"
    );

    workspace.command(b'v');
    assert!(
        common::wait_until(secs(10), || runs().len() == 2),
        "the split did not start a terminal; log={:?}",
        scratch.entries()
    );
    let runs = runs();
    assert_eq!(
        runs[1]["params"]["size"],
        json!({"cols": 37, "rows": 20}),
        "the split's right-hand pane is 37 cells wide"
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// An explicit PROGRAM after a restore runs in a new tab and is told that
/// tab's attach size before the tab is inserted.
#[cfg(unix)]
fn workspace_an_explicit_program_gets_the_new_tab_attach_size() {
    let scratch = Scratch::new("workspace-program-size");
    scratch.seed_doc(doc_same_tab(), 7);
    let mut workspace = FakeScreen::spawn_parts(
        &scratch,
        "doc-program",
        &[],
        &["--", "sh", "-c", "sleep 30"],
        None,
    );
    assert!(
        wait_for_text(&workspace, "PAINT"),
        "the explicit program's tab did not paint; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );

    let runs: Vec<_> = scratch
        .entries()
        .into_iter()
        .filter(|entry| entry["call"] == "terminal.run")
        .collect();
    assert_eq!(runs.len(), 1, "the explicit program must start once");
    assert_eq!(
        runs[0]["params"]["size"],
        json!({"cols": 78, "rows": 20}),
        "the new tab must start at the pane it will attach at"
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// A document deleted at a retained version (for example by a failed claim)
/// is claimed again at that version instead of being treated as malformed.
#[cfg(unix)]
fn workspace_reclaims_a_deleted_document_at_its_version() {
    let scratch = Scratch::new("doc-deleted-start");
    scratch.seed_doc(json!(null), 5);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-deleted-start");

    assert!(
        workspace.wait_text("PAINT", secs(10)),
        "the first shell was not started; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    assert!(
        common::wait_until(secs(5), || {
            doc_addresses(&scratch.doc_value()) == vec!["t1".to_owned()]
        }),
        "the reclaimed document must gain the first pane; doc={:?}",
        scratch.doc_value()
    );
    let versions: Vec<u64> = scratch
        .entries()
        .iter()
        .filter(|entry| entry["call"] == "doc.set")
        .filter_map(|entry| entry["params"]["version"].as_u64())
        .collect();
    assert_eq!(
        versions,
        vec![5, 6],
        "the claim uses the deleted document's version, then the pane write uses the claim's"
    );
    assert_eq!(
        scratch
            .entries()
            .iter()
            .filter(|entry| entry["call"] == "terminal.run")
            .count(),
        1,
        "the reclaiming screen starts exactly one shell"
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// A document deleted at a retained version is rebuilt from the live labeled
/// terminals: both panes come back, the local view is applied, and no new
/// shell is started.
#[cfg(unix)]
fn workspace_recovers_labeled_terminals_when_the_document_is_deleted() {
    let scratch = Scratch::new("doc-deleted-recover");
    scratch.seed_doc(json!(null), 5);
    scratch.seed_view(
        "default",
        json!({
            "version": 1,
            "active_space": "1",
            "active_tabs": {"1": "2"},
            "tabs": {"2": {"focused": "3", "zoomed": "3", "history": ["3"]}},
        }),
    );
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-deleted-recover");

    assert!(
        workspace.wait_text("PANE-ONE", secs(10)),
        "the recovered pane must be drawn; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    assert!(
        !workspace.rendered().contains("PANE-TWO"),
        "the local view's zoom must hide the other recovered pane; screen={:?}",
        workspace.rendered()
    );
    assert!(
        common::wait_until(secs(5), || {
            doc_addresses(&scratch.doc_value()) == vec!["t1".to_owned(), "t2".to_owned()]
        }),
        "the recreated document must contain both recovered panes; doc={:?}",
        scratch.doc_value()
    );
    assert_eq!(
        scratch.doc_version(),
        6,
        "the document is recreated once at the retained version"
    );
    let log = scratch.entries();
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.run"),
        "a recovered terminal must never be replaced by a new shell; log={log:?}"
    );
    assert!(
        log.iter()
            .any(|entry| { entry.get("attached").is_some() && entry["terminal"] == "t1" }),
        "the recovered pane is attached; log={log:?}"
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// When no labeled terminal survives a deleted document, the claim winner
/// starts the initial shell; a screen that loses the recreate race starts
/// nothing and restores the winner's empty document.
#[cfg(unix)]
fn workspace_losing_the_deleted_document_claim_starts_no_shell() {
    let scratch = Scratch::new("doc-deleted-claim-lost");
    scratch.seed_doc(json!(null), 5);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-deleted-claim-lost");

    assert!(
        workspace.wait_text("no terminals in this tab", secs(10)),
        "the lost claim must restore the winner's empty document; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    assert_eq!(
        scratch.doc_version(),
        6,
        "the winner's document is version 6; doc={:?}",
        scratch.doc_value()
    );
    assert!(
        scratch.doc_value()["spaces"][0]["tabs"][0]["layout"] == json!(null),
        "the recreated document is the empty one; doc={:?}",
        scratch.doc_value()
    );
    let log = scratch.entries();
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.run"),
        "a screen that lost the claim must not start a shell; log={log:?}"
    );
    let sets: Vec<u64> = log
        .iter()
        .filter(|entry| entry["call"] == "doc.set")
        .filter_map(|entry| entry["params"]["version"].as_u64())
        .collect();
    assert_eq!(
        sets,
        vec![5],
        "the loser makes one CAS attempt at the retained version"
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// A labeled terminal absent from the document is recovered into its own
/// recovered space and tab at startup, without starting a program.
#[cfg(unix)]
fn workspace_recovers_a_labeled_orphan_at_startup() {
    let scratch = Scratch::new("doc-orphan");
    scratch.seed_doc(doc_one_pane(), 4);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-orphan");

    assert!(
        workspace.wait_text("PANE-ONE", secs(10)),
        "the documented pane must be drawn; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    assert!(
        common::wait_until(secs(5), || {
            doc_addresses(&scratch.doc_value()) == vec!["t1".to_owned(), "t7".to_owned()]
        }),
        "the recovered pane must be written to the document; doc={:?}",
        scratch.doc_value()
    );
    // The orphan lands in a recovered space; switch to it to see it.
    workspace.command(b'N');
    assert!(
        workspace.wait_text("ORPHAN-PANE", secs(10)),
        "the recovered orphan must be drawn in its recovered space; screen={:?}",
        workspace.rendered()
    );
    let log = scratch.entries();
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.run"),
        "a recovered terminal must never be started; log={log:?}"
    );
    assert!(
        log.iter()
            .any(|entry| { entry.get("attached").is_some() && entry["terminal"] == "t7" }),
        "the recovered terminal is attached; log={log:?}"
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// A live terminal without this client's labels is never adopted as an
/// orphan, even at startup.
#[cfg(unix)]
fn workspace_ignores_an_unlabeled_orphan_at_startup() {
    let scratch = Scratch::new("doc-orphan-invalid");
    scratch.seed_doc(doc_one_pane(), 4);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-orphan-invalid");

    assert!(
        workspace.wait_text("PANE-ONE", secs(10)),
        "the documented pane must be drawn; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    assert_eq!(
        doc_addresses(&scratch.doc_value()),
        vec!["t1".to_owned()],
        "an unlabeled terminal is not added to the document; doc={:?}",
        scratch.doc_value()
    );
    assert_eq!(
        scratch.doc_version(),
        4,
        "a clean restore of the documented pane writes nothing"
    );
    let log = scratch.entries();
    assert!(
        !log.iter()
            .any(|entry| { entry.get("attached").is_some() && entry["terminal"] == "t9" }),
        "an unlabeled terminal must not be attached; log={log:?}"
    );
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.run"));

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// A doc event carries no value: the client refetches, adopts the pane the
/// other screen added and attaches it; nothing is started.
#[cfg(unix)]
fn workspace_refetches_after_a_doc_event() {
    let scratch = Scratch::new("doc-event");
    scratch.seed_doc(doc_one_pane(), 5);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-event");
    assert!(
        workspace.wait_text("PANE-ONE", secs(10)),
        "the restored pane was not drawn; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );

    workspace.send(b"g");
    assert!(
        workspace.wait_text("PANE-TWO", secs(10)),
        "the pane added by the doc event was not adopted; screen={:?}",
        workspace.rendered()
    );
    assert_eq!(
        doc_addresses(&scratch.doc_value()),
        vec!["t1".to_owned(), "t2".to_owned()]
    );
    let log = scratch.entries();
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.run"),
        "a doc event must never start a terminal; log={log:?}"
    );
    assert!(
        log.iter()
            .any(|entry| entry.get("attached").is_some() && entry["terminal"] == "t2"),
        "the added pane's existing terminal is attached; log={log:?}"
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// A doc event applies a document from another screen without disturbing
/// this screen's local view: the focused, zoomed pane stays in front.
#[cfg(unix)]
fn workspace_keeps_local_focus_after_a_doc_event() {
    let scratch = Scratch::new("doc-event-focus");
    scratch.seed_doc(doc_same_tab(), 5);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-event-focus");
    assert!(
        workspace.wait_text("PANE-ONE", secs(10)) && workspace.wait_text("PANE-TWO", secs(10)),
        "both panes must be restored; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );

    workspace.command(b'l');
    workspace.command(b'z');
    assert!(
        workspace.wait_text("PANE-TWO", secs(5))
            && common::wait_until(secs(5), || !workspace.rendered().contains("PANE-ONE")),
        "the zoomed right pane must fill the tab; screen={:?}",
        workspace.rendered()
    );

    // A key to the focused pane makes another screen add a third pane.
    workspace.send(b"g");
    assert!(
        common::wait_until(secs(10), || {
            let log = scratch.entries();
            log.iter()
                .any(|entry| entry.get("attached").is_some() && entry["terminal"] == "t3")
        }),
        "the pane added by the doc event must be adopted; screen={:?} log={:?}",
        workspace.rendered(),
        scratch.entries()
    );
    assert_eq!(
        doc_addresses(&scratch.doc_value()),
        vec!["t1".to_owned(), "t2".to_owned(), "t3".to_owned()]
    );
    assert!(
        workspace.rendered().contains("PANE-TWO")
            && !workspace.rendered().contains("PANE-ONE")
            && !workspace.rendered().contains("SPLIT-PANE"),
        "the local focus and zoom must survive the doc event; screen={:?}",
        workspace.rendered()
    );
    assert!(
        !scratch
            .entries()
            .iter()
            .any(|entry| entry["call"] == "terminal.run"),
        "a doc event must never start a terminal"
    );

    workspace.command(b'z');
    assert!(
        workspace.wait_text("PANE-ONE", secs(5)) && workspace.wait_text("SPLIT-PANE", secs(5)),
        "unzooming shows the adopted pane; screen={:?}",
        workspace.rendered()
    );

    // Closing the focused pane proves focus survived the document change:
    // the right pane (t2) leaves, not t1 or the adopted t3.
    workspace.command(b'x');
    assert!(
        common::wait_until(secs(10), || {
            doc_addresses(&scratch.doc_value()) == vec!["t1".to_owned(), "t3".to_owned()]
        }),
        "the focused pane must still be t2 after the doc event; doc={:?}",
        scratch.doc_value()
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// A pane another screen closed leaves this workspace; its program is only
/// detached, never killed or removed.
#[cfg(unix)]
fn workspace_removes_a_pane_deleted_by_another_screen() {
    let scratch = Scratch::new("doc-remove");
    scratch.seed_doc(doc_same_tab(), 5);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-remove");
    assert!(
        workspace.wait_text("PANE-ONE", secs(10)) && workspace.wait_text("PANE-TWO", secs(10)),
        "both restored panes were not drawn; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );

    workspace.send(b"g");
    assert!(
        common::wait_until(secs(10), || !workspace.rendered().contains("PANE-TWO")),
        "the removed pane must leave the screen; screen={:?}",
        workspace.rendered()
    );
    assert!(
        common::wait_until(secs(5), || {
            doc_addresses(&scratch.doc_value()) == vec!["t1".to_owned()]
        }),
        "the document must lose the pane; doc={:?}",
        scratch.doc_value()
    );
    let log = scratch.entries();
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.kill"));
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.rm"));

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// The connection drops, another screen changes the document meanwhile, and
/// the fresh connection refetches it before reattaching; no duplicate starts.
#[cfg(unix)]
fn workspace_refetches_the_document_after_reconnect() {
    let scratch = Scratch::new("doc-reconnect");
    scratch.seed_doc(doc_one_pane(), 5);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-reconnect");
    assert!(
        workspace.wait_text("PANE-ONE", secs(10)),
        "the restored pane was not drawn; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );

    workspace.send(b"g");
    assert!(
        workspace.wait_text("PANE-TWO", secs(25)),
        "the document changed while away was not applied after reconnect; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );
    let log = scratch.entries();
    let generations: Vec<u64> = log
        .iter()
        .filter_map(|entry| entry["generation"].as_u64())
        .collect();
    assert!(
        generations.contains(&1) && generations.contains(&2),
        "the client must reconnect; log={log:?}"
    );
    assert!(
        !log.iter().any(|entry| entry["call"] == "terminal.run"),
        "a reconnect must never start a terminal; log={log:?}"
    );
    assert!(
        log.iter()
            .filter(|entry| entry["call"] == "doc.get")
            .count()
            >= 2,
        "the document is read again after the reconnect; log={log:?}"
    );
    assert_eq!(
        doc_addresses(&scratch.doc_value()),
        vec!["t1".to_owned(), "t2".to_owned()]
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// `missed` also means the document may have changed: the client refetches and
/// adopts the pane the fresh snapshot and document name.
#[cfg(unix)]
fn workspace_refetches_the_document_after_missed() {
    let scratch = Scratch::new("doc-missed");
    scratch.seed_doc(doc_one_pane(), 5);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-missed");
    assert!(
        workspace.wait_text("PANE-ONE", secs(10)),
        "the restored pane was not drawn; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );

    workspace.send(b"g");
    assert!(
        workspace.wait_text("PANE-TWO", secs(10)),
        "the document was not refetched after missed; screen={:?}",
        workspace.rendered()
    );
    assert!(
        !scratch
            .entries()
            .iter()
            .any(|entry| entry["call"] == "terminal.run"),
        "a resnapshot must not start a terminal"
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// A document deleted by another screen is not overwritten blindly: the
/// client recreates its own capture at the version core kept.
#[cfg(unix)]
fn workspace_recreates_a_deleted_document() {
    let scratch = Scratch::new("doc-deleted");
    scratch.seed_doc(doc_one_pane(), 5);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-deleted");
    assert!(
        workspace.wait_text("PANE-ONE", secs(10)),
        "the restored pane was not drawn; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );

    workspace.send(b"g");
    assert!(
        common::wait_until(secs(10), || {
            let (value, version) = scratch.doc();
            !value.is_null() && version >= 7
        }),
        "the deleted document was not recreated; store={:?}",
        scratch.doc()
    );
    assert_eq!(
        doc_addresses(&scratch.doc_value()),
        vec!["t1".to_owned()],
        "the recreated document keeps the local pane"
    );
    assert!(
        !scratch
            .entries()
            .iter()
            .any(|entry| entry["call"] == "terminal.run"),
        "recreating a document must not start a terminal"
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// A CAS conflict on a change in a different space merges both: this screen's
/// split and the other screen's new space and terminal both survive.
#[cfg(unix)]
fn workspace_merges_a_concurrent_change_in_another_space() {
    let scratch = Scratch::new("doc-conflict-merge");
    scratch.seed_doc(doc_one_pane(), 5);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-conflict-merge");
    assert!(
        workspace.wait_text("PANE-ONE", secs(10)),
        "the restored pane was not drawn; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );

    // The split's reply arrives after another screen has already written, so
    // this client's next save is a stale CAS write.
    workspace.command(b'v');
    assert!(
        workspace.wait_text("SPLIT-PANE", secs(10)),
        "this screen's split was not drawn; screen={:?}",
        workspace.rendered()
    );
    // The other screen's pane lives in its own space; both survive and the
    // remote space can be visited.
    workspace.command(b'N');
    assert!(
        workspace.wait_text("PANE-TWO", secs(10)),
        "the other screen's pane was not adopted; screen={:?}",
        workspace.rendered()
    );
    assert!(
        common::wait_until(secs(5), || {
            let addresses = doc_addresses(&scratch.doc_value());
            addresses.len() == 3
                && addresses.contains(&"t1".to_owned())
                && addresses.contains(&"t2".to_owned())
                && addresses.contains(&"t3".to_owned())
        }),
        "the merged document must keep both screens' work; doc={:?}",
        scratch.doc_value()
    );
    assert!(
        scratch.doc_value()["spaces"].as_array().unwrap().len() == 2,
        "the other screen's space must survive; doc={:?}",
        scratch.doc_value()
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// Two changes to the same tab conflict: this screen keeps its layout, says
/// so, and the other screen's terminal is recovered rather than lost.
#[cfg(unix)]
fn workspace_keeps_local_on_a_same_tab_conflict_and_recovers_the_other_pane() {
    let scratch = Scratch::new("doc-conflict-tab");
    scratch.seed_doc(doc_one_pane(), 5);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-conflict-tab");
    assert!(
        workspace.wait_text("PANE-ONE", secs(10)),
        "the restored pane was not drawn; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );

    workspace.command(b'v');
    assert!(
        workspace.wait_text("changed on another screen", secs(10)),
        "a same-tab conflict must be reported; screen={:?}",
        workspace.rendered()
    );
    assert!(
        workspace.wait_text("SPLIT-PANE", secs(10)),
        "this screen's layout must win; screen={:?}",
        workspace.rendered()
    );
    assert!(
        workspace.wait_text("PANE-TWO", secs(10)),
        "the other screen's terminal must be recovered; screen={:?}",
        workspace.rendered()
    );
    assert!(
        common::wait_until(secs(5), || doc_addresses(&scratch.doc_value()).len() == 3),
        "the recovered pane must be written back; doc={:?}",
        scratch.doc_value()
    );
    let log = scratch.entries();
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.kill"));
    assert!(!log.iter().any(|entry| entry["call"] == "terminal.rm"));

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );
}

/// The normal loop exhausts its three conflict rounds, leaving a local change
/// dirty. The detach flush then loses one more CAS race, merges the other
/// screen's work, retries, and writes the local change; a one-round flush
/// would stop after the conflict and lose it.
#[cfg(unix)]
fn workspace_flushes_a_merged_change_on_detach_after_conflicts() {
    let scratch = Scratch::new("doc-conflict-flush");
    scratch.seed_doc(doc_one_pane(), 5);
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "doc-conflict-flush");
    assert!(
        workspace.wait_text("PANE-ONE", secs(10)),
        "the restored pane must be drawn; screen={:?} stderr={:?}",
        workspace.rendered(),
        workspace.stderr.text()
    );

    // The split is a local change; each CAS round loses to another screen,
    // so the normal loop cannot write it and leaves it dirty.
    workspace.command(b'v');
    assert!(
        workspace.wait_text("SPLIT-PANE", secs(10)),
        "the local split must be drawn; screen={:?}",
        workspace.rendered()
    );
    assert!(
        common::wait_until(secs(10), || {
            scratch
                .entries()
                .iter()
                .filter(|entry| entry["call"] == "doc.set")
                .count()
                >= 3
        }),
        "the loop must use its conflict rounds; log={:?}",
        scratch.entries()
    );
    let before_detach = scratch
        .entries()
        .iter()
        .filter(|entry| entry["call"] == "doc.set")
        .count();
    assert_eq!(
        before_detach, 3,
        "three CAS rounds before detaching, leaving the split dirty"
    );

    workspace.send(&[0x1d]);
    let status = workspace.wait_exit(secs(5)).expect("detach");
    assert_eq!(
        status.code(),
        Some(0),
        "stderr={:?}",
        workspace.stderr.text()
    );

    let addresses = doc_addresses(&scratch.doc_value());
    assert!(
        addresses.contains(&"t2".to_owned()),
        "the other screen's terminal must be preserved; doc={:?}",
        scratch.doc_value()
    );
    assert!(
        addresses.contains(&"t3".to_owned()),
        "the local split must be flushed on detach; doc={:?}",
        scratch.doc_value()
    );
    let attempts = scratch
        .entries()
        .iter()
        .filter(|entry| entry["call"] == "doc.set")
        .count();
    assert!(
        attempts > before_detach,
        "the detach flush must attempt another write"
    );
    assert_eq!(
        attempts, 5,
        "the flush's first CAS loses, then the merge retries and writes"
    );
    assert_eq!(
        scratch.doc_version(),
        10,
        "the flush conflict is re-read, merged and retried at version 10"
    );
}

/// Every document pane address, in document order.
fn doc_addresses(doc: &serde_json::Value) -> Vec<String> {
    fn leaves(layout: &serde_json::Value, out: &mut Vec<String>) {
        if let Some(leaf) = layout.get("leaf") {
            if let Some(address) = leaf.get("address").and_then(|value| value.as_str()) {
                out.push(address.to_owned());
            }
            return;
        }
        if let Some(children) = layout.get("children").and_then(|value| value.as_array()) {
            for child in children {
                if let Some(layout) = child.get("layout") {
                    leaves(layout, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    let spaces = doc.get("spaces").and_then(|value| value.as_array());
    for space in spaces.into_iter().flatten() {
        let tabs = space.get("tabs").and_then(|value| value.as_array());
        for tab in tabs.into_iter().flatten() {
            if let Some(layout) = tab.get("layout") {
                leaves(layout, &mut out);
            }
        }
    }
    out
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

/// A program that exited before attach: the probe must read the final screen,
/// report the exit status, and remove the record it kept with `rm: false`.
fn a_program_that_exits_before_attach_is_still_shown_and_removed() {
    let scratch = Scratch::new("finished");
    let exe = std::env::current_exe().expect("test executable");
    let mut child = Command::new(HT)
        .arg("probe")
        .arg("--hc")
        .arg(&exe)
        .arg("--")
        .env("FAKE_SCENARIO", "finished")
        .env("FAKE_LOG", scratch.log())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ht probe");
    let stdout = Capture::spawn(child.stdout.take().expect("stdout"));
    let stderr = Capture::spawn(child.stderr.take().expect("stderr"));
    let mut probe = Probe::new(child);

    let status = probe
        .wait_timeout(secs(10))
        .expect("the probe exits after recovering the program");
    assert_eq!(
        status.code(),
        Some(7),
        "the recovered program's exit status must be reported; stderr={:?}",
        stderr.text()
    );
    assert!(
        stdout.wait_for(b"FAST", secs(2)),
        "the recovered screen must reach stdout; output={:?}",
        stdout.text()
    );

    let log = scratch.entries();
    assert!(
        log.iter().any(|entry| entry["call"] == "terminal.rm"),
        "the finished record must be removed; log={log:?}"
    );
    let run = log
        .iter()
        .find(|entry| entry["call"] == "terminal.run")
        .expect("the probe ran a terminal");
    assert_eq!(
        run["params"]["rm"],
        json!(false),
        "the run must keep the record until the probe reads it"
    );
    assert_eq!(
        run["params"]["size"],
        json!({"cols": 80, "rows": 24}),
        "the probe must start the program at the grid it attaches at"
    );
}

/// A local stdout failure while the program is alive: the probe must detach,
/// end the program it started, and remove the record.
#[cfg(unix)]
fn a_local_write_error_stops_and_removes_the_started_terminal() {
    use std::fs::File;
    use std::os::fd::FromRawFd;

    let scratch = Scratch::new("write-error");
    let exe = std::env::current_exe().expect("test executable");
    // A stdout pipe with no reader: every write fails with EPIPE.
    let mut fds = [0i32; 2];
    // SAFETY: fds is a two-element array that pipe fills on success.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe");
    // SAFETY: both descriptors came from the pipe above.
    unsafe { libc::close(fds[0]) };
    // SAFETY: fds[1] is the pipe's write end and is owned from here on.
    let stdout = unsafe { Stdio::from(File::from_raw_fd(fds[1])) };

    let mut child = Command::new(HT)
        .arg("probe")
        .arg("--hc")
        .arg(&exe)
        .arg("--")
        .env("FAKE_SCENARIO", "live")
        .env("FAKE_LOG", scratch.log())
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ht probe");
    let stderr = Capture::spawn(child.stderr.take().expect("stderr"));
    let mut probe = Probe::new(child);

    let status = probe.wait_timeout(secs(10)).expect("the probe exits");
    assert_eq!(
        status.code(),
        Some(1),
        "a broken stdout must fail the probe; stderr={:?}",
        stderr.text()
    );
    assert!(
        stderr.wait_for(b"cannot write terminal output", secs(2)),
        "stderr={:?}",
        stderr.text()
    );

    let log = scratch.entries();
    assert!(
        log.iter().any(|entry| entry["call"] == "terminal.kill"),
        "the started program must be ended; log={log:?}"
    );
    assert!(
        log.iter().any(|entry| entry["call"] == "terminal.rm"),
        "the finished record must be removed; log={log:?}"
    );
    assert!(
        log.iter().any(|entry| {
            entry["frame"]["channel"] == ATTACH_CHANNEL && entry["frame"]["kind"] == CLOSE
        }),
        "the probe must detach from its stream; log={log:?}"
    );
}

// --- fake core -------------------------------------------------------------

// --- M5 agent scenarios ----------------------------------------------------

#[cfg(unix)]
fn label_calls(scratch: &Scratch) -> Vec<Value> {
    scratch
        .entries()
        .into_iter()
        .filter(|entry| entry["call"] == "terminal.label")
        .collect()
}

#[cfg(unix)]
fn seen_writes(scratch: &Scratch, address: &str, turn: u64) -> usize {
    label_calls(scratch)
        .iter()
        .filter(|entry| {
            entry["params"]["terminal"] == json!(address)
                && entry["params"]["labels"]["herdr.seen"] == json!(turn.to_string())
        })
        .count()
}

#[cfg(unix)]
fn wait_for_seen(scratch: &Scratch, address: &str, turn: u64) -> bool {
    common::wait_until(secs(8), || seen_writes(scratch, address, turn) > 0)
}

#[cfg(unix)]
fn wait_for_text(screen: &FakeScreen, needle: &str) -> bool {
    screen.wait_text(needle, secs(10))
}

#[cfg(unix)]
fn doc_set_calls(scratch: &Scratch) -> usize {
    scratch
        .entries()
        .iter()
        .filter(|entry| entry["call"] == "doc.set")
        .count()
}

/// Asserts a local-only action neither changed the shared document version
/// nor sent a `doc.set`.
#[cfg(unix)]
fn assert_doc_untouched(scratch: &Scratch, version: u64, writes: usize, what: &str) {
    assert_eq!(
        scratch.doc_version(),
        version,
        "{what} changed the layout document: {:?}",
        scratch.doc_value()
    );
    assert_eq!(
        doc_set_calls(scratch),
        writes,
        "{what} sent doc.set: {:?}",
        scratch.entries()
    );
}

#[cfg(unix)]
fn wait_doc_has(scratch: &Scratch, address: &str) -> bool {
    common::wait_until(secs(10), || {
        doc_addresses(&scratch.doc_value())
            .iter()
            .any(|known| known == address)
    })
}

#[cfg(unix)]
fn attach_keys(scratch: &Scratch) -> Vec<String> {
    scratch
        .entries()
        .iter()
        .filter_map(|entry| entry["attach_data"]["text"].as_str().map(str::to_owned))
        .collect()
}

#[cfg(unix)]
fn resize_frames(scratch: &Scratch) -> usize {
    scratch
        .entries()
        .iter()
        .filter(|entry| entry["frame"]["kind"] == json!(3))
        .count()
}

/// The exact `(cols, rows)` of every RESIZE the client sent.
#[cfg(unix)]
fn resize_payloads(scratch: &Scratch) -> Vec<(u16, u16)> {
    scratch
        .entries()
        .iter()
        .filter(|entry| entry["frame"]["kind"] == json!(3))
        .filter_map(|entry| {
            Some((
                u16::try_from(entry["frame"]["cols"].as_u64()?).ok()?,
                u16::try_from(entry["frame"]["rows"].as_u64()?).ok()?,
            ))
        })
        .collect()
}

/// Every byte the client sent to a program, keyed by the terminal address the
/// attach channel belongs to.
#[cfg(unix)]
fn program_input(scratch: &Scratch) -> Vec<(String, String)> {
    let mut channels: BTreeMap<u64, String> = BTreeMap::new();
    for entry in scratch.entries() {
        if let (Some(channel), Some(target)) =
            (entry["attached"].as_u64(), entry["terminal"].as_str())
        {
            channels.insert(channel, target.to_owned());
        }
    }
    scratch
        .entries()
        .iter()
        .filter_map(|entry| {
            let channel = entry["attach_data"]["channel"].as_u64()?;
            let text = entry["attach_data"]["text"].as_str()?;
            Some((
                channels.get(&channel).cloned().unwrap_or_default(),
                text.to_owned(),
            ))
        })
        .collect()
}

#[cfg(unix)]
fn terminal_read_calls(scratch: &Scratch) -> usize {
    scratch
        .entries()
        .iter()
        .filter(|entry| entry["call"] == "terminal.read")
        .count()
}

/// SGR mouse input as the outer terminal would send it (cells are 0-based in
/// the test, 1-based on the wire).
#[cfg(unix)]
fn sgr(button: u8, release: bool, x: u16, y: u16) -> String {
    format!(
        "\x1b[<{button};{};{}{}",
        x + 1,
        y + 1,
        if release { 'm' } else { 'M' }
    )
}

#[cfg(unix)]
fn click(x: u16, y: u16) -> String {
    format!("{}{}", sgr(0, false, x, y), sgr(0, true, x, y))
}

#[cfg(unix)]
fn wheel_up(x: u16, y: u16) -> String {
    sgr(64, false, x, y)
}

#[cfg(unix)]
fn generation_count(scratch: &Scratch) -> usize {
    scratch
        .entries()
        .iter()
        .filter(|entry| entry.get("generation").is_some())
        .count()
}

#[cfg(unix)]
fn attach_count(scratch: &Scratch, address: &str) -> usize {
    scratch
        .entries()
        .iter()
        .filter(|entry| entry.get("attached").is_some() && entry["terminal"] == json!(address))
        .count()
}

/// A visible turn is marked seen once; a hidden busy→non-busy transition is a
/// notice and one BEL, and never writes a seen label.
#[cfg(unix)]
fn workspace_marks_visible_turns_seen_and_alerts_for_hidden_finishes() {
    let scratch = Scratch::new("agents-turn");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "agents-turn");
    assert!(
        wait_for_text(&screen, "[idle]"),
        "the reported agent was not shown: {:?}",
        screen.rendered()
    );

    // A key makes the agent work, then finish at turn 1 while visible.
    screen.send(b"x");
    assert!(
        wait_for_text(&screen, "[working]"),
        "working status was not shown: {:?}",
        screen.rendered()
    );
    let version = scratch.doc_version();
    let writes = doc_set_calls(&scratch);
    screen.send(b"x");
    assert!(
        wait_for_seen(&scratch, "t1", 1),
        "visible turn 1 was not marked seen: log={:?}",
        scratch.entries()
    );
    assert!(
        wait_for_text(&screen, "[idle]"),
        "the seen agent was not idle: {:?}",
        screen.rendered()
    );
    assert_doc_untouched(&scratch, version, writes, "the automatic seen write");
    assert_eq!(
        screen.output.count(7),
        0,
        "a finish while looking must not beep"
    );

    // Same turn, focus away and back: the seen write is not repeated.
    screen.command(b'a');
    assert!(wait_for_text(&screen, "agents"), "sidebar did not open");
    screen.send(b"\x1b");
    assert!(
        common::wait_until(secs(5), || !screen.rendered().contains("agents")),
        "sidebar did not close: {:?}",
        screen.rendered()
    );
    // Another event at the same turn gives the refreshed row a visible change
    // to wait on before counting the seen writes again.
    screen.send(b"x");
    assert!(
        wait_for_text(&screen, "claude3"),
        "the same-turn event was not applied: {:?}",
        screen.rendered()
    );
    assert_eq!(
        seen_writes(&scratch, "t1", 1),
        1,
        "re-focusing the same turn rewrote seen: {:?}",
        label_calls(&scratch)
    );
}

/// While the sidebar covers the panes, a busy agent finishing is one notice and
/// one BEL, and its new turn stays unseen.
#[cfg(unix)]
fn workspace_alerts_once_for_a_hidden_finish_and_keeps_the_turn_unseen() {
    let scratch = Scratch::new("agents-hidden");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "agents-hidden-done");
    assert!(wait_for_text(&screen, "[blocked]"));

    // A new tab makes t1 hidden and gives the working t2 the whole tab.
    screen.command(b'c');
    assert!(
        wait_for_text(&screen, "PANE-TWO"),
        "the second agent did not attach: {:?}",
        screen.rendered()
    );
    assert!(
        wait_for_seen(&scratch, "t2", 1),
        "visible t2 turn 1 not seen"
    );

    // Back to the first tab; the sidebar then covers both panes.
    screen.command(b'1');
    assert!(wait_for_text(&screen, "Build"));
    screen.command(b'a');
    assert!(wait_for_text(&screen, "agents"));
    let version = scratch.doc_version();
    let writes = doc_set_calls(&scratch);

    // A pin write is the deterministic moment the sidebar is open; the fake
    // then finishes t2 at turn 2 while nobody can see it.
    screen.send(b"p");
    assert!(
        screen.output.wait_count(7, 1, secs(10)),
        "the hidden finish did not beep: {:?}",
        screen.rendered()
    );
    assert!(
        wait_for_text(&screen, "finished"),
        "the hidden finish had no notice: {:?}",
        screen.rendered()
    );
    assert!(
        wait_for_text(&screen, "unseen"),
        "the hidden turn was marked seen: {:?}",
        screen.rendered()
    );
    assert!(
        wait_for_text(&screen, "1 blocked · 1 done"),
        "attention count is wrong: {:?}",
        screen.rendered()
    );
    assert_eq!(
        seen_writes(&scratch, "t2", 2),
        0,
        "a hidden turn must not be marked seen: {:?}",
        label_calls(&scratch)
    );
    assert_doc_untouched(&scratch, version, writes, "the hidden finish and pin");
}

/// Another client's pin label re-sorts the sidebar without this client writing
/// a pin label itself.
#[cfg(unix)]
fn workspace_honors_another_clients_pin_label() {
    let scratch = Scratch::new("agents-pin-event");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "agents-pin-event");
    assert!(wait_for_text(&screen, "[blocked]"));
    screen.command(b'v');
    assert!(wait_for_text(&screen, "PANE-TWO"));

    screen.command(b'a');
    assert!(wait_for_text(&screen, "agents"));
    let version = scratch.doc_version();
    assert!(
        common::wait_until(secs(10), || {
            let text = screen.rendered();
            match (text.find("Review"), text.find("Build")) {
                (Some(review), Some(build)) => review < build,
                _ => false,
            }
        }),
        "the pinned agent did not move first: {:?}",
        screen.rendered()
    );
    assert!(
        screen.rendered().contains("* idle"),
        "the pin marker is missing: {:?}",
        screen.rendered()
    );
    assert!(
        label_calls(&scratch)
            .iter()
            .all(|call| call["params"]["labels"]["herdr.pinned"].is_null()),
        "this client wrote a pin label instead of honoring the event: {:?}",
        label_calls(&scratch)
    );
    assert_eq!(scratch.doc_version(), version);
}

/// `p` and `u` send exactly the documented merged label maps, and nothing
/// else about the document changes.
#[cfg(unix)]
fn workspace_pins_unpins_and_marks_unread_with_exact_labels() {
    let scratch = Scratch::new("agents-pin");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "agents-pin");
    assert!(wait_for_text(&screen, "[blocked]"));
    assert!(
        wait_for_seen(&scratch, "t1", 2),
        "the visible turn was not marked seen: {:?}",
        scratch.entries()
    );
    let version = scratch.doc_version();
    let writes = doc_set_calls(&scratch);
    screen.command(b'a');
    assert!(wait_for_text(&screen, "agents"));
    assert_doc_untouched(&scratch, version, writes, "opening the sidebar");
    let version = scratch.doc_version();
    let writes = doc_set_calls(&scratch);

    screen.send(b"p");
    assert!(
        common::wait_until(secs(5), || {
            label_calls(&scratch).iter().any(|call| {
                call["params"]["terminal"] == json!("t1")
                    && call["params"]["labels"] == json!({"herdr.pinned": "1"})
            })
        }),
        "the pin map was not exact: {:?}",
        label_calls(&scratch)
    );
    assert_doc_untouched(&scratch, version, writes, "pin");

    screen.send(b"p");
    assert!(
        common::wait_until(secs(5), || {
            label_calls(&scratch).iter().any(|call| {
                call["params"]["terminal"] == json!("t1")
                    && call["params"]["labels"] == json!({"herdr.pinned": ""})
            })
        }),
        "the unpin map was not exact: {:?}",
        label_calls(&scratch)
    );
    assert_doc_untouched(&scratch, version, writes, "unpin");

    screen.send(b"u");
    assert!(
        common::wait_until(secs(5), || {
            label_calls(&scratch).iter().any(|call| {
                call["params"]["terminal"] == json!("t1")
                    && call["params"]["labels"] == json!({"herdr.seen": "1"})
            })
        }),
        "unread did not write turn-1: {:?}",
        label_calls(&scratch)
    );
    assert_doc_untouched(&scratch, version, writes, "unread");
}

/// Opening and closing the sidebar is local view state: it never writes the
/// layout document, whether `a` or Esc closes it.
#[cfg(unix)]
fn workspace_sidebar_toggle_and_close_do_not_write_the_document() {
    let scratch = Scratch::new("agents-sidebar-doc");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "agents-turn");
    assert!(wait_for_text(&screen, "[idle]"));
    assert!(
        wait_doc_has(&scratch, "t1"),
        "the initial layout was not saved: {:?}",
        scratch.doc_value()
    );
    let version = scratch.doc_version();
    let writes = doc_set_calls(&scratch);

    screen.command(b'a');
    assert!(wait_for_text(&screen, "agents"));
    assert_doc_untouched(&scratch, version, writes, "opening the sidebar");

    screen.command(b'a');
    assert!(
        common::wait_until(secs(5), || !screen.rendered().contains("agents")),
        "the sidebar did not close with a: {:?}",
        screen.rendered()
    );
    assert_doc_untouched(&scratch, version, writes, "closing the sidebar with a");

    screen.command(b'a');
    assert!(wait_for_text(&screen, "agents"));
    screen.send(b"\x1b");
    assert!(
        common::wait_until(secs(5), || !screen.rendered().contains("agents")),
        "the sidebar did not close with Esc: {:?}",
        screen.rendered()
    );
    assert_doc_untouched(&scratch, version, writes, "closing the sidebar with Esc");
}

/// Sidebar keys are modal: an unhandled key is consumed, the sidebar stays
/// open, and the same key reaches the pane once the sidebar closes.
#[cfg(unix)]
fn workspace_sidebar_consumes_unhandled_keys() {
    let scratch = Scratch::new("agents-sidebar-keys");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "agents-turn");
    assert!(wait_for_text(&screen, "[idle]"));
    screen.command(b'a');
    assert!(wait_for_text(&screen, "agents"));

    let before = attach_keys(&scratch).len();
    screen.send(b"z");
    // Pinning is a handled sidebar key and writes a label; its log entry is a
    // barrier proving the client has processed both keys.
    screen.send(b"p");
    assert!(
        common::wait_until(secs(5), || {
            label_calls(&scratch).iter().any(|call| {
                call["params"]["terminal"] == json!("t1")
                    && call["params"]["labels"] == json!({"herdr.pinned": "1"})
            })
        }),
        "the pin barrier was not reached: {:?}",
        scratch.entries()
    );
    assert!(wait_for_text(&screen, "pinned"));
    assert_eq!(
        attach_keys(&scratch).len(),
        before,
        "the sidebar forwarded an unhandled key: {:?}",
        attach_keys(&scratch)
    );
    assert!(
        screen.rendered().contains("agents"),
        "the sidebar closed on an unhandled key: {:?}",
        screen.rendered()
    );
    assert!(
        !screen.rendered().contains("working"),
        "the unhandled key changed the agent status: {:?}",
        screen.rendered()
    );

    screen.send(b"\x1b");
    assert!(
        common::wait_until(secs(5), || !screen.rendered().contains("agents")),
        "the sidebar did not close: {:?}",
        screen.rendered()
    );
    assert!(
        !screen.rendered().contains("working"),
        "the agent changed while the sidebar was open: {:?}",
        screen.rendered()
    );
    screen.send(b"z");
    assert!(
        common::wait_until(secs(5), || attach_keys(&scratch).contains(&"z".to_owned())),
        "the same key did not reach the pane after closing: {:?}",
        attach_keys(&scratch)
    );
    assert!(
        wait_for_text(&screen, "[working]"),
        "the forwarded key did not drive the agent: {:?}",
        screen.rendered()
    );
}

/// Sidebar and peek are overlays: they never send RESIZE after the initial
/// attach sizing.
#[cfg(unix)]
fn workspace_sidebar_and_peek_do_not_resize_panes() {
    let scratch = Scratch::new("agents-resize");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "agents-two");
    assert!(wait_for_text(&screen, "[blocked]"));
    screen.command(b'v');
    assert!(wait_for_text(&screen, "PANE-TWO"));
    assert!(
        common::wait_until(secs(5), || resize_frames(&scratch) >= 1),
        "the split never sent its initial RESIZE: {:?}",
        scratch.entries()
    );
    let resizes = resize_frames(&scratch);

    screen.command(b'a');
    assert!(wait_for_text(&screen, "agents"));
    assert_eq!(
        resize_frames(&scratch),
        resizes,
        "opening the sidebar resized a pane: {:?}",
        scratch.entries()
    );
    screen.send(b"\x1b");
    assert!(
        common::wait_until(secs(5), || !screen.rendered().contains("agents")),
        "the sidebar did not close: {:?}",
        screen.rendered()
    );
    assert_eq!(
        resize_frames(&scratch),
        resizes,
        "closing the sidebar resized a pane: {:?}",
        scratch.entries()
    );

    screen.command(b'i');
    assert!(wait_for_text(&screen, "peek · any key closes"));
    assert_eq!(
        resize_frames(&scratch),
        resizes,
        "opening peek resized a pane: {:?}",
        scratch.entries()
    );
    screen.send(b"z");
    assert!(
        common::wait_until(secs(5), || {
            !screen.rendered().contains("peek · any key closes")
        }),
        "peek did not close: {:?}",
        screen.rendered()
    );
    assert_eq!(
        resize_frames(&scratch),
        resizes,
        "closing peek resized a pane: {:?}",
        scratch.entries()
    );
}

/// The first snapshot only sets the alert baseline: an agent already blocked
/// when this screen subscribes gets an attention count but no notice and no BEL.
#[cfg(unix)]
fn workspace_startup_baseline_does_not_alert_for_blocked_agents() {
    let scratch = Scratch::new("agents-startup-baseline");
    scratch.seed_doc(one_tab_doc(&[(3, "t1")]), 5);
    let screen = FakeScreen::spawn_workspace(&scratch, "agents-snapshot-blocked");
    assert!(
        wait_for_text(&screen, "[blocked]"),
        "the startup agent was not shown: {:?}",
        screen.rendered()
    );
    assert!(
        wait_for_text(&screen, "1 blocked"),
        "the startup attention count is missing: {:?}",
        screen.rendered()
    );
    assert_eq!(
        screen.output.count(7),
        0,
        "startup replayed an alert: {:?}",
        screen.rendered()
    );
    assert!(
        !screen.rendered().contains("needs you"),
        "startup showed a needs-you notice: {:?}",
        screen.rendered()
    );
    assert_eq!(
        scratch.doc_version(),
        5,
        "a restored blocked state must not write the document"
    );
    assert!(
        !scratch
            .entries()
            .iter()
            .any(|entry| entry["call"] == "terminal.run"),
        "the restored agent must not be started again"
    );
}

/// A reconnect rebuilds the alert baseline: a blocked state present in the
/// fresh snapshot is counted, never replayed as a new alert.
#[cfg(unix)]
fn workspace_reconnect_baseline_does_not_replay_an_alert() {
    let scratch = Scratch::new("agents-reconnect-baseline");
    let screen = FakeScreen::spawn_workspace(&scratch, "agents-reconnect-blocked");
    assert!(
        common::wait_until(secs(10), || generation_count(&scratch) >= 2),
        "the client did not reconnect: {:?}",
        scratch.entries()
    );
    assert!(
        wait_for_text(&screen, "1 blocked"),
        "the reconnected attention count is missing: {:?}",
        screen.rendered()
    );
    assert_eq!(
        screen.output.count(7),
        0,
        "the reconnect replayed an alert: {:?}",
        screen.rendered()
    );
    assert!(
        !screen.rendered().contains("needs you"),
        "the reconnect showed a needs-you notice: {:?}",
        screen.rendered()
    );
}

/// The same baseline rule after `missed`: a blocked snapshot is counted, not
/// replayed as a new alert.
#[cfg(unix)]
fn workspace_missed_baseline_does_not_replay_an_alert() {
    let scratch = Scratch::new("agents-missed-baseline");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "agents-missed-blocked");
    assert!(wait_for_text(&screen, "[blocked]"));
    screen.send(b"x");
    assert!(
        common::wait_until(secs(10), || attach_count(&scratch, "t1") >= 2),
        "the terminal was not reattached after missed: {:?}",
        scratch.entries()
    );
    assert!(
        wait_for_text(&screen, "1 blocked"),
        "the missed attention count is missing: {:?}",
        screen.rendered()
    );
    assert_eq!(
        screen.output.count(7),
        0,
        "missed replayed an alert: {:?}",
        screen.rendered()
    );
    assert!(
        !screen.rendered().contains("needs you"),
        "missed showed a needs-you notice: {:?}",
        screen.rendered()
    );
}

/// Jumping to a hidden agent focuses it and marks its turn seen; returning to
/// the same turn later does not write again.
#[cfg(unix)]
fn workspace_jumps_to_an_agent_and_marks_it_seen() {
    let scratch = Scratch::new("agents-jump");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "agents-jump");
    assert!(wait_for_text(&screen, "[blocked]"));
    screen.command(b'c');
    assert!(wait_for_text(&screen, "PANE-TWO"));
    assert!(
        screen.output.wait_count(7, 1, secs(10)),
        "the hidden finish did not alert: {:?}",
        screen.rendered()
    );
    assert!(wait_for_seen(&scratch, "t2", 1));

    // Jumping is local: it must not write the layout document.
    let version = scratch.doc_version();
    let writes = doc_set_calls(&scratch);

    // Sidebar order puts the done, unseen t1 before idle t2; g cycles to it.
    screen.command(b'g');
    assert!(
        wait_for_text(&screen, "/ t1"),
        "jump did not focus t1: {:?}",
        screen.rendered()
    );
    assert!(
        wait_for_seen(&scratch, "t1", 1),
        "the jumped-to turn was not marked seen: {:?}",
        label_calls(&scratch)
    );
    assert_doc_untouched(&scratch, version, writes, "jump");

    // Leave and return at the same turn: no second seen write.
    screen.command(b'2');
    assert!(wait_for_text(&screen, "/ t2"));
    screen.command(b'1');
    assert!(wait_for_text(&screen, "/ t1"));
    assert_eq!(
        seen_writes(&scratch, "t1", 1),
        1,
        "the same turn was rewritten: {:?}",
        label_calls(&scratch)
    );
}

/// Peek draws the existing local VT plus core facts and never calls
/// `terminal.read`.
#[cfg(unix)]
fn workspace_peeks_an_agent_without_reading_its_screen() {
    let scratch = Scratch::new("agents-peek");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "agents-two");
    assert!(wait_for_text(&screen, "[blocked]"));
    screen.command(b'v');
    assert!(wait_for_text(&screen, "PANE-TWO"));
    assert!(
        wait_for_seen(&scratch, "t2", 1),
        "the visible split pane was not marked seen"
    );
    let version = scratch.doc_version();
    let writes = doc_set_calls(&scratch);
    screen.command(b'i');
    assert!(
        wait_for_text(&screen, "peek · any key closes"),
        "peek did not open: {:?}",
        screen.rendered()
    );
    assert_doc_untouched(&scratch, version, writes, "peek");
    let text = screen.rendered();
    assert!(text.contains("Review") && text.contains("t2"), "{text}");
    assert!(text.contains("PANE-TWO"), "peek lost the local VT: {text}");
    assert!(
        scratch
            .entries()
            .iter()
            .all(|entry| entry["call"] != "terminal.read"),
        "peek must never read the screen from core"
    );

    // Any key closes it and is not forwarded.
    screen.send(b"z");
    assert!(
        common::wait_until(secs(5), || {
            !screen.rendered().contains("peek · any key closes")
        }),
        "peek did not close: {:?}",
        screen.rendered()
    );
}

/// A failed seen write is reported, retried with a delay, and stops at the
/// bounded count.
#[cfg(unix)]
fn workspace_retries_a_failed_seen_write_with_a_bounded_delay() {
    let scratch = Scratch::new("agents-fail");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "agents-fail");
    assert!(
        wait_for_text(&screen, "could not mark t1 seen"),
        "no failure was reported: {:?}",
        screen.rendered()
    );
    assert!(
        common::wait_until(secs(10), || label_calls(&scratch).len() >= 3),
        "the seen write was not retried: {:?}",
        label_calls(&scratch)
    );
    assert!(
        wait_for_seen(&scratch, "t1", 2),
        "the retry never succeeded: {:?}",
        label_calls(&scratch)
    );
    // Opening the sidebar is a later observable action; by then the bounded
    // retries must have stopped at three attempts.
    screen.command(b'a');
    assert!(wait_for_text(&screen, "agents"));
    assert_eq!(
        label_calls(&scratch).len(),
        3,
        "the retry loop was not bounded: {:?}",
        label_calls(&scratch)
    );
}

/// A label reply delayed past the 2 s timeout, followed by persistent failures,
/// ends after exactly three attempts with a give-up notice and a live
/// connection: the 2 s timeout and the total attempt count are both observable.
#[cfg(unix)]
fn workspace_gives_up_after_a_timed_out_seen_write() {
    let scratch = Scratch::new("agents-slow-fail");
    let mut screen = FakeScreen::spawn_workspace_wide(&scratch, "agents-slow-fail", 160, 30);
    assert!(
        wait_for_text(&screen, "could not mark t1 seen"),
        "the 2 s label timeout was not reported: {:?}",
        screen.rendered()
    );
    assert!(
        wait_for_text(&screen, "retrying"),
        "the failure did not schedule a retry: {:?}",
        screen.rendered()
    );
    assert!(
        wait_for_text(&screen, "giving up"),
        "persistent failures never gave up: {:?}",
        screen.rendered()
    );
    assert_eq!(
        label_calls(&scratch).len(),
        3,
        "the give-up must stop after exactly three attempts: {:?}",
        label_calls(&scratch)
    );
    // No fourth attempt in the next three retry periods.
    assert!(
        !common::wait_until(secs(3), || label_calls(&scratch).len() > 3),
        "the failed write kept retrying after giving up: {:?}",
        label_calls(&scratch)
    );
    // The connection is still live: a key reaches the pane.
    let keys = attach_keys(&scratch).len();
    screen.send(b"x");
    assert!(
        common::wait_until(secs(5), || attach_keys(&scratch).len() > keys),
        "the connection or pane input died after giving up: {:?}",
        scratch.entries()
    );
    assert_eq!(label_calls(&scratch).len(), 3);
}

/// A lost label write takes the normal reconnect path, and the fresh snapshot
/// makes the seen write again.
#[cfg(unix)]
fn workspace_reconnects_after_a_lost_label_write() {
    let scratch = Scratch::new("agents-lost");
    let screen = FakeScreen::spawn_workspace(&scratch, "agents-lost");
    assert!(
        wait_for_text(&screen, "reconnected to core"),
        "the client did not recover: {:?}",
        screen.rendered()
    );
    assert!(
        common::wait_until(secs(10), || seen_writes(&scratch, "t1", 2) >= 2),
        "the write was not retried after reconnecting: {:?}",
        label_calls(&scratch)
    );
    let generations = scratch
        .entries()
        .iter()
        .filter(|entry| entry.get("generation").is_some())
        .count();
    assert!(generations >= 2, "the client did not reconnect");
}

/// A persisted `sidebar_open` is applied before any key, and the rest of the
/// local view (the active tab) is still honored.
#[cfg(unix)]
fn workspace_restores_an_open_sidebar_from_the_local_view() {
    let scratch = Scratch::new("agents-sidebar-restart");
    scratch.seed_doc(doc_two_tabs(), 3);
    scratch.seed_view(
        "work",
        json!({
            "version": 1,
            "active_space": "1",
            "active_tabs": {"1": "31"},
            "tabs": {
                "30": {"focused": "3", "zoomed": null, "history": ["3"]},
                "31": {"focused": "4", "zoomed": null, "history": ["4"]},
            },
            "sidebar_open": true,
        }),
    );
    let mut screen = FakeScreen::spawn_workspace_as(&scratch, "doc-view", "work");
    assert!(
        screen.wait_text("no agents yet", secs(10)),
        "the persisted sidebar was not drawn open before any key: {:?}",
        screen.rendered()
    );
    assert_eq!(
        scratch.doc_version(),
        3,
        "restoring the sidebar flag must not write the document"
    );

    // Closing it reveals the tab the rest of the local view selected.
    screen.command(b'a');
    assert!(
        screen.wait_text("TAB-TWO", secs(5)),
        "the rest of the local view was lost: {:?}",
        screen.rendered()
    );
    assert!(
        !screen.rendered().contains("TAB-ONE"),
        "a background tab was drawn: {:?}",
        screen.rendered()
    );

    screen.send(&[0x1d]);
    let status = screen.wait_exit(secs(5)).expect("detach");
    assert_eq!(status.code(), Some(0), "stderr={:?}", screen.stderr.text());
}

/// Mouse clicks focus the pane under the pointer and select tabs in the bar.
/// Chrome clicks never reach a program.
#[cfg(unix)]
fn workspace_mouse_clicks_focus_panes_and_select_tabs() {
    let scratch = Scratch::new("m6-click");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-multi");
    assert!(
        wait_for_text(&screen, "PANE-ONE"),
        "the first pane did not paint: {:?}",
        screen.rendered()
    );
    screen.command(b'v');
    assert!(
        wait_for_text(&screen, "PANE-TWO"),
        "the split did not paint: {:?}",
        screen.rendered()
    );
    assert!(
        wait_for_text(&screen, "/ t2"),
        "the new split should hold the focus: {:?}",
        screen.rendered()
    );

    // A grid click moves the focus back to the first pane. It is a plain
    // focus click: no selection mode, no Esc, and typing reaches the program
    // immediately.
    screen.send(click(10, 5).as_bytes());
    assert!(
        wait_for_text(&screen, "/ t1"),
        "clicking the left pane did not focus it: {:?}",
        screen.rendered()
    );
    screen.send(b"x");
    assert!(
        common::wait_until(secs(10), || {
            program_input(&scratch)
                .iter()
                .any(|(target, text)| target == "t1" && text == "x")
        }),
        "typing after a plain click did not reach t1: {:?}",
        program_input(&scratch)
    );
    assert!(
        !screen.rendered().contains("selection:"),
        "a plain click entered selection mode: {:?}",
        screen.rendered()
    );

    // The tab bar selects tabs; the workspace keeps them in the document, but
    // which tab this screen shows is local.
    screen.command(b'c');
    assert!(
        wait_for_text(&screen, "PANE-THREE"),
        "the new tab did not paint: {:?}",
        screen.rendered()
    );
    assert!(wait_for_text(&screen, "/ t3"), "{:?}", screen.rendered());
    assert!(
        wait_doc_has(&scratch, "t3"),
        "the tab was not written to the document: {:?}",
        scratch.doc_value()
    );
    let version = scratch.doc_version();
    let writes = doc_set_calls(&scratch);

    screen.send(click(10, 0).as_bytes());
    assert!(
        wait_for_text(&screen, "PANE-ONE"),
        "clicking the first tab did not show it: {:?} stderr={:?}",
        screen.rendered(),
        screen.errors()
    );
    assert!(wait_for_text(&screen, "/ t1"), "{:?}", screen.rendered());
    screen.send(click(19, 0).as_bytes());
    assert!(
        wait_for_text(&screen, "PANE-THREE"),
        "clicking the second tab did not show it: {:?}",
        screen.rendered()
    );
    assert!(wait_for_text(&screen, "/ t3"), "{:?}", screen.rendered());
    assert_doc_untouched(&scratch, version, writes, "tab selection");
}

/// A selection never makes the mouse modal: a fresh press on the same pane,
/// another pane or a tab leaves it and is routed normally.
#[cfg(unix)]
fn workspace_a_fresh_press_leaves_a_selection_and_routes_normally() {
    let scratch = Scratch::new("m6-selection-click");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-multi");
    assert!(
        wait_for_text(&screen, "PANE-ONE"),
        "{:?}",
        screen.rendered()
    );
    screen.command(b'v');
    assert!(
        wait_for_text(&screen, "PANE-TWO"),
        "{:?}",
        screen.rendered()
    );
    assert!(wait_for_text(&screen, "/ t2"), "{:?}", screen.rendered());

    // Focus the first pane and drag a local selection there.
    screen.send(click(10, 5).as_bytes());
    assert!(
        wait_for_text(&screen, "/ t1"),
        "the click did not focus t1: {:?}",
        screen.rendered()
    );
    screen.send(sgr(0, false, 10, 5).as_bytes());
    screen.send(sgr(32, false, 14, 5).as_bytes());
    screen.send(sgr(0, true, 14, 5).as_bytes());
    assert!(
        wait_for_text(&screen, "selection:"),
        "the drag did not start a selection: {:?}",
        screen.rendered()
    );

    // A fresh click in the same pane ends the selection; the click and the
    // next key are both processed.
    screen.send(click(10, 6).as_bytes());
    assert!(
        common::wait_until(secs(10), || !screen.rendered().contains("selection:")),
        "a fresh click did not leave selection mode: {:?}",
        screen.rendered()
    );
    screen.send(b"w");
    assert!(
        common::wait_until(secs(10), || {
            program_input(&scratch)
                .iter()
                .any(|(target, text)| target == "t1" && text == "w")
        }),
        "typing after the fresh click did not reach t1: {:?}",
        program_input(&scratch)
    );

    // The same holds for a tab click: create a second tab, select in the
    // first, then click the second tab and type.
    screen.command(b'c');
    assert!(
        wait_for_text(&screen, "PANE-THREE"),
        "the new tab did not paint: {:?}",
        screen.rendered()
    );
    assert!(wait_for_text(&screen, "/ t3"), "{:?}", screen.rendered());
    screen.send(click(10, 0).as_bytes());
    assert!(
        wait_for_text(&screen, "PANE-ONE"),
        "the first tab was not selected: {:?}",
        screen.rendered()
    );
    screen.send(click(10, 5).as_bytes());
    assert!(
        wait_for_text(&screen, "/ t1"),
        "t1 was not focused: {:?}",
        screen.rendered()
    );
    screen.send(sgr(0, false, 10, 5).as_bytes());
    screen.send(sgr(32, false, 14, 5).as_bytes());
    screen.send(sgr(0, true, 14, 5).as_bytes());
    assert!(
        wait_for_text(&screen, "selection:"),
        "the second drag did not start a selection: {:?}",
        screen.rendered()
    );
    screen.send(click(19, 0).as_bytes());
    assert!(
        wait_for_text(&screen, "PANE-THREE"),
        "a tab click during selection did not select the tab: {:?}",
        screen.rendered()
    );
    assert!(
        common::wait_until(secs(10), || !screen.rendered().contains("selection:")),
        "the tab click did not leave selection mode: {:?}",
        screen.rendered()
    );
    screen.send(b"q");
    assert!(
        common::wait_until(secs(10), || {
            program_input(&scratch)
                .iter()
                .any(|(target, text)| target == "t3" && text == "q")
        }),
        "typing after the tab click did not reach t3: {:?}",
        program_input(&scratch)
    );
}

/// Dragging the mouse extends the selection across cells and copies the whole
/// range, not only the cell under the initial press.
#[cfg(unix)]
fn workspace_a_drag_selection_extends_and_copies_the_range() {
    let scratch = Scratch::new("m6-drag-copy");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-copy");
    assert!(
        wait_for_text(&screen, "hello"),
        "the copy fixture did not paint: {:?}",
        screen.rendered()
    );

    // "hello" starts at the first inner cell, screen (1,2) in the single
    // pane; drag from h to o in two steps, so the first drag starts the
    // selection and the second one must extend it.
    screen.send(sgr(0, false, 1, 2).as_bytes());
    screen.send(sgr(32, false, 3, 2).as_bytes());
    screen.send(sgr(32, false, 5, 2).as_bytes());
    screen.send(sgr(0, true, 5, 2).as_bytes());
    assert!(
        wait_for_text(&screen, "selection:"),
        "the drag did not start a selection: {:?}",
        screen.rendered()
    );
    screen.send(b"y");
    let expected = format!(
        "\x1b]52;c;{}\x07",
        herdr_terminal::clipboard::base64(b"hello")
    );
    assert!(
        common::wait_until(secs(10), || screen.raw().contains(&expected)),
        "the drag selection did not copy the whole range: {:?}",
        screen.tail()
    );
    assert!(
        wait_for_text(&screen, "copied 5 characters"),
        "the drag selection copied the wrong number of characters: {:?}",
        screen.rendered()
    );
}

/// Dragging a split gap resizes them exactly and sends RESIZE frames with
/// the new inner grid sizes.
#[cfg(unix)]
fn workspace_dragging_a_split_sends_exact_resize_frames() {
    let scratch = Scratch::new("m6-drag");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-multi");
    assert!(
        wait_for_text(&screen, "PANE-ONE"),
        "{:?}",
        screen.rendered()
    );
    screen.command(b'v');
    assert!(
        wait_for_text(&screen, "PANE-TWO"),
        "{:?}",
        screen.rendered()
    );
    assert!(
        common::wait_until(secs(10), || {
            resize_payloads(&scratch).contains(&(38, 20))
        }),
        "the split did not resize the left pane to its inner size: {:?}",
        resize_payloads(&scratch)
    );
    let before = resize_payloads(&scratch).len();

    // The gap is the single column between the 40- and 39-cell panes.
    screen.send(sgr(0, false, 40, 10).as_bytes());
    screen.send(sgr(32, false, 44, 10).as_bytes());
    screen.send(sgr(0, true, 44, 10).as_bytes());
    assert!(
        common::wait_until(secs(10), || {
            let payloads = resize_payloads(&scratch);
            payloads.contains(&(42, 20)) && payloads.contains(&(33, 20))
        }),
        "the drag did not resize both panes: {:?}",
        resize_payloads(&scratch)
    );
    assert_eq!(
        resize_payloads(&scratch).len(),
        before + 2,
        "a 4-cell drag sent extra RESIZE frames: {:?}",
        resize_payloads(&scratch)
    );
}

/// The wheel scrolls a non-tracking program's local history and any key
/// returns to the live bottom. Neither touches core or the document.
#[cfg(unix)]
fn workspace_wheel_scrolls_local_history_and_any_key_returns_live() {
    let scratch = Scratch::new("m6-wheel");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-scroll");
    assert!(
        wait_for_text(&screen, "LIVE-BOTTOM"),
        "the scrollback fixture did not paint: {:?}",
        screen.rendered()
    );
    assert!(wait_doc_has(&scratch, "t1"), "{:?}", scratch.doc_value());
    let attach = scratch
        .entries()
        .into_iter()
        .find(|entry| entry["call"] == "terminal.attach")
        .expect("the pane attached");
    assert_eq!(
        attach["params"]["history"],
        json!(10_000),
        "the default scrollback was not requested from core: {attach}"
    );
    let version = scratch.doc_version();
    let writes = doc_set_calls(&scratch);
    let resizes = resize_frames(&scratch);

    screen.send(wheel_up(10, 5).as_bytes());
    assert!(
        common::wait_until(secs(10), || {
            let rendered = screen.rendered();
            rendered.contains("scroll ") && !rendered.contains("LIVE-BOTTOM")
        }),
        "the wheel did not scroll local history: {:?}",
        screen.rendered()
    );
    assert!(
        program_input(&scratch).is_empty(),
        "the wheel reached a non-tracking program: {:?}",
        program_input(&scratch)
    );
    assert_eq!(
        resize_frames(&scratch),
        resizes,
        "local scrolling resized a pane"
    );
    assert_eq!(
        terminal_read_calls(&scratch),
        0,
        "local scrolling read a terminal"
    );
    assert_doc_untouched(&scratch, version, writes, "wheel scrolling");

    // Any ordinary key snaps the pane back to live before it is forwarded.
    screen.send(b"x");
    assert!(
        common::wait_until(secs(10), || {
            program_input(&scratch)
                .iter()
                .any(|(target, text)| target == "t1" && text == "x")
                && screen.rendered().contains("LIVE-BOTTOM")
        }),
        "the key did not return the pane to live: {:?}",
        screen.rendered()
    );
    assert_doc_untouched(&scratch, version, writes, "returning to live");
}

/// The cursor is drawn only at the live bottom: scrolling the focused pane
/// into history hides it, and returning live shows it again.
#[cfg(unix)]
fn workspace_the_live_cursor_is_hidden_in_history_and_returns_at_the_bottom() {
    let scratch = Scratch::new("m6-cursor");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-scroll");
    assert!(
        wait_for_text(&screen, "LIVE-BOTTOM"),
        "the scrollback fixture did not paint: {:?}",
        screen.rendered()
    );
    assert!(
        screen.cursor().visible,
        "the live cursor was not drawn: {:?}",
        screen.cursor()
    );

    // The fixture leaves the cursor three rows above the bottom, so after a
    // three-line wheel scroll the terminal's own cursor is still inside the
    // viewport. Only the live-bottom rule hides it.
    screen.send(wheel_up(10, 5).as_bytes());
    assert!(
        wait_for_text(&screen, "scroll "),
        "the wheel did not scroll local history: {:?}",
        screen.rendered()
    );
    assert!(
        common::wait_until(secs(10), || !screen.cursor().visible),
        "the cursor stayed visible while the pane was scrolled: {:?}",
        screen.cursor()
    );

    screen.send(b"\x1b[1;2F");
    assert!(
        common::wait_until(secs(10), || {
            screen.rendered().contains("LIVE-BOTTOM") && screen.cursor().visible
        }),
        "the live cursor did not return at the bottom: {:?} {:?}",
        screen.rendered(),
        screen.cursor()
    );
}

/// A program that tracks the mouse receives pane-local events on its grid.
/// Clicks and wheels on chrome are consumed, never forwarded.
#[cfg(unix)]
fn workspace_forwards_mouse_to_a_tracking_program_and_consumes_chrome() {
    let scratch = Scratch::new("m6-mouse");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-mouse");
    assert!(
        wait_for_text(&screen, "MOUSE-READY"),
        "the mouse program did not paint: {:?}",
        screen.rendered()
    );

    screen.send(wheel_up(10, 5).as_bytes());
    assert!(
        common::wait_until(secs(10), || {
            program_input(&scratch)
                .iter()
                .any(|(_, text)| text.contains("\x1b[<64;"))
        }),
        "the wheel was not forwarded to the tracking program: {:?}",
        program_input(&scratch)
    );
    screen.send(click(10, 6).as_bytes());
    assert!(
        common::wait_until(secs(10), || {
            program_input(&scratch)
                .iter()
                .any(|(_, text)| text.contains("\x1b[<0;"))
        }),
        "the grid click was not forwarded: {:?}",
        program_input(&scratch)
    );
    let after_grid = program_input(&scratch).len();

    // Tab bar, status bar and pane border are chrome: focus or selection only.
    screen.send(click(10, 0).as_bytes());
    screen.send(click(10, 23).as_bytes());
    screen.send(click(0, 5).as_bytes());
    screen.send(b"z");
    assert!(
        common::wait_until(secs(10), || {
            program_input(&scratch)
                .iter()
                .any(|(target, text)| target == "t1" && text == "z")
        }),
        "the client stalled after chrome clicks: {:?}",
        program_input(&scratch)
    );
    let forwarded: Vec<String> = program_input(&scratch)
        .into_iter()
        .skip(after_grid)
        .map(|(_, text)| text)
        .collect();
    assert!(
        forwarded.iter().all(|text| !text.contains("\x1b[<")),
        "a chrome click reached the tracking program: {forwarded:?}"
    );
}

/// Shift-drag is always local: even over a program that tracks the mouse it
/// starts a selection and sends no mouse report to the program.
#[cfg(unix)]
fn workspace_shift_drag_selects_locally_over_a_tracking_program() {
    let scratch = Scratch::new("m6-shift-select");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-mouse");
    assert!(
        wait_for_text(&screen, "MOUSE-READY"),
        "the mouse program did not paint: {:?}",
        screen.rendered()
    );
    let before = program_input(&scratch).len();

    screen.send(sgr(4, false, 10, 6).as_bytes());
    screen.send(sgr(36, false, 14, 6).as_bytes());
    screen.send(sgr(4, true, 14, 6).as_bytes());
    assert!(
        wait_for_text(&screen, "selection:"),
        "Shift-drag did not start a local selection: {:?}",
        screen.rendered()
    );
    assert_eq!(
        program_input(&scratch).len(),
        before,
        "a Shift-drag reached the tracking program: {:?}",
        program_input(&scratch)
    );

    screen.send(&[0x1b]);
    assert!(
        common::wait_until(secs(10), || !screen.rendered().contains("selection:")),
        "Esc did not leave the selection: {:?}",
        screen.rendered()
    );
    assert_eq!(
        program_input(&scratch).len(),
        before,
        "Esc from a local selection reached the tracking program: {:?}",
        program_input(&scratch)
    );
}

/// A Shift-click is a local click: over a tracking program neither its press
/// nor its release reaches the program. A later plain click is the barrier
/// that proves the program's input stream is still live.
#[cfg(unix)]
fn workspace_shift_click_is_local_over_a_tracking_program() {
    let scratch = Scratch::new("m6-shift-click");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-mouse");
    assert!(
        wait_for_text(&screen, "MOUSE-READY"),
        "the mouse program did not paint: {:?}",
        screen.rendered()
    );

    screen.send(sgr(4, false, 10, 6).as_bytes());
    screen.send(sgr(4, true, 10, 6).as_bytes());
    screen.send(click(10, 7).as_bytes());
    assert!(
        common::wait_until(secs(10), || {
            program_input(&scratch)
                .iter()
                .any(|(target, text)| target == "t1" && text.contains("\x1b[<0;"))
        }),
        "the barrier click was not forwarded: {:?}",
        program_input(&scratch)
    );
    let forwarded: Vec<String> = program_input(&scratch)
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    assert!(
        forwarded
            .iter()
            .all(|text| !text.contains("\x1b[<4;") && !text.contains("\x1b[<36;")),
        "the Shift-click reached the tracking program: {forwarded:?}"
    );
    assert!(
        !screen.rendered().contains("selection:"),
        "a Shift-click entered selection mode: {:?}",
        screen.rendered()
    );
}

/// An ordinary key during a selection leaves it, snaps the pane back to the
/// live bottom and reaches the program exactly once.
#[cfg(unix)]
fn workspace_an_ordinary_key_leaves_selection_and_reaches_the_program() {
    let scratch = Scratch::new("m6-key-exit");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-scroll");
    assert!(
        wait_for_text(&screen, "LIVE-BOTTOM"),
        "the scrollback fixture did not paint: {:?}",
        screen.rendered()
    );

    screen.command(b'[');
    assert!(
        wait_for_text(&screen, "selection:"),
        "selection mode did not open: {:?}",
        screen.rendered()
    );
    screen.send(wheel_up(10, 5).as_bytes());
    assert!(
        common::wait_until(secs(10), || {
            let rendered = screen.rendered();
            rendered.contains("scroll ") && !rendered.contains("LIVE-BOTTOM")
        }),
        "the wheel did not scroll the selected pane: {:?}",
        screen.rendered()
    );
    assert!(
        screen.rendered().contains("selection:"),
        "scrolling ended the selection: {:?}",
        screen.rendered()
    );

    screen.send(b"w");
    assert!(
        common::wait_until(secs(10), || {
            let rendered = screen.rendered();
            program_input(&scratch)
                .iter()
                .any(|(target, text)| target == "t1" && text == "w")
                && rendered.contains("LIVE-BOTTOM")
                && !rendered.contains("selection:")
                && !rendered.contains("scroll ")
        }),
        "the ordinary key did not leave selection at the live bottom: {:?} input={:?}",
        screen.rendered(),
        program_input(&scratch)
    );
    let forwarded: Vec<String> = program_input(&scratch)
        .into_iter()
        .filter(|(target, _)| target == "t1")
        .map(|(_, text)| text)
        .collect();
    assert_eq!(
        forwarded,
        vec!["w".to_string()],
        "the ordinary key was not forwarded exactly once: {forwarded:?}"
    );
    assert!(
        screen.cursor().visible,
        "the live cursor did not return: {:?}",
        screen.cursor()
    );
}

/// When a program requests kitty key events, the key that leaves a selection
/// is forwarded with both its press and its release.
#[cfg(unix)]
fn workspace_a_selection_exit_key_forwards_its_release() {
    let scratch = Scratch::new("m6-kitty-exit");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-kitty");
    assert!(
        wait_for_text(&screen, "KITTY-READY"),
        "the kitty fixture did not paint: {:?}",
        screen.rendered()
    );
    screen.command(b'[');
    assert!(
        wait_for_text(&screen, "selection:"),
        "selection mode did not open: {:?}",
        screen.rendered()
    );

    // Kitty CSI u: 'w' press (event type 1), then its release (type 3). A
    // plain printable press stays legacy text; the release is the CSI u form.
    screen.send(b"\x1b[119;1:1u");
    assert!(
        common::wait_until(secs(10), || {
            program_input(&scratch)
                .iter()
                .any(|(target, text)| target == "t1" && text == "w")
        }),
        "the kitty press was not forwarded: {:?}",
        program_input(&scratch)
    );
    screen.send(b"\x1b[119;1:3u");
    assert!(
        common::wait_until(secs(10), || {
            program_input(&scratch)
                .iter()
                .any(|(target, text)| target == "t1" && text == "\x1b[119;1:3u")
        }),
        "the kitty release was not forwarded: {:?}",
        program_input(&scratch)
    );
    assert!(
        common::wait_until(secs(10), || !screen.rendered().contains("selection:")),
        "the kitty key did not leave selection mode: {:?}",
        screen.rendered()
    );
}

/// An open overlay consumes every mouse event: no input reaches a program and
/// no pane is resized.
#[cfg(unix)]
fn workspace_overlays_consume_mouse_without_input_or_resize() {
    let scratch = Scratch::new("m6-overlay-mouse");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-multi");
    assert!(
        wait_for_text(&screen, "PANE-ONE"),
        "{:?}",
        screen.rendered()
    );
    assert!(wait_doc_has(&scratch, "t1"), "{:?}", scratch.doc_value());
    let version = scratch.doc_version();
    let writes = doc_set_calls(&scratch);
    let resizes = resize_frames(&scratch);

    screen.command(b':');
    assert!(
        wait_for_text(&screen, "palette · Enter run · Esc close"),
        "the palette did not open: {:?}",
        screen.rendered()
    );
    screen.send(click(10, 5).as_bytes());
    screen.send(wheel_up(10, 5).as_bytes());
    screen.send(click(10, 0).as_bytes());
    screen.send(&[0x1b]);
    assert!(
        common::wait_until(secs(10), || !screen.rendered().contains("palette ·")),
        "the palette did not close: {:?}",
        screen.rendered()
    );

    screen.command(b'f');
    assert!(
        wait_for_text(&screen, "search:"),
        "the search bar did not open: {:?}",
        screen.rendered()
    );
    screen.send(click(10, 5).as_bytes());
    screen.send(wheel_up(10, 5).as_bytes());
    screen.send(&[0x1b]);
    assert!(
        common::wait_until(secs(10), || !screen.rendered().contains("search:")),
        "the search bar did not close: {:?}",
        screen.rendered()
    );

    assert!(
        program_input(&scratch).is_empty(),
        "an overlay mouse event reached the program: {:?}",
        program_input(&scratch)
    );
    assert_eq!(
        resize_frames(&scratch),
        resizes,
        "an overlay mouse event resized a pane"
    );
    assert_doc_untouched(&scratch, version, writes, "overlay mouse events");
}

/// The palette reuses the command table: a view command writes nothing, and a
/// layout command does exactly what its key would.
#[cfg(unix)]
fn workspace_palette_runs_a_command_and_view_commands_do_not_write() {
    let scratch = Scratch::new("m6-palette");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-palette");
    assert!(
        wait_for_text(&screen, "PANE-ONE"),
        "{:?}",
        screen.rendered()
    );
    assert!(wait_doc_has(&scratch, "t1"), "{:?}", scratch.doc_value());
    let version = scratch.doc_version();
    let writes = doc_set_calls(&scratch);

    screen.command(b':');
    assert!(
        wait_for_text(&screen, "palette · Enter run · Esc close"),
        "the palette did not open: {:?}",
        screen.rendered()
    );
    screen.send(b"help");
    assert!(
        wait_for_text(&screen, "› help"),
        "the palette did not filter: {:?}",
        screen.rendered()
    );
    screen.send(b"\r");
    assert!(
        wait_for_text(&screen, "any key closes help"),
        "Enter did not run help: {:?}",
        screen.rendered()
    );
    assert_doc_untouched(&scratch, version, writes, "palette help");
    assert_eq!(
        terminal_read_calls(&scratch),
        0,
        "the palette read a terminal"
    );

    screen.send(&[0x1b]);
    assert!(
        common::wait_until(secs(10), || !screen
            .rendered()
            .contains("any key closes help")),
        "help did not close: {:?}",
        screen.rendered()
    );

    screen.command(b':');
    assert!(wait_for_text(&screen, "palette · Enter run · Esc close"));
    screen.send(b"split top");
    assert!(
        wait_for_text(&screen, "› split top"),
        "the palette did not filter the split command: {:?}",
        screen.rendered()
    );
    screen.send(b"\r");
    assert!(
        wait_for_text(&screen, "PANE-TWO"),
        "the palette split did not run: {:?}",
        screen.rendered()
    );
    assert!(
        wait_doc_has(&scratch, "t2"),
        "the palette split was not persisted: {:?}",
        scratch.doc_value()
    );
    assert!(
        scratch.doc_version() > version || doc_set_calls(&scratch) > writes,
        "the palette split did not write the layout"
    );
    assert_eq!(
        terminal_read_calls(&scratch),
        0,
        "the palette read a terminal"
    );
}

/// Search is local: it never calls `terminal.read` or writes the document, and
/// Enter jumps the viewport to the matching history line.
#[cfg(unix)]
fn workspace_search_is_local_and_jumps_to_a_match() {
    let scratch = Scratch::new("m6-search");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-scroll");
    assert!(
        wait_for_text(&screen, "LIVE-BOTTOM"),
        "the scrollback fixture did not paint: {:?}",
        screen.rendered()
    );
    assert!(wait_doc_has(&scratch, "t1"), "{:?}", scratch.doc_value());
    let version = scratch.doc_version();
    let writes = doc_set_calls(&scratch);
    let resizes = resize_frames(&scratch);

    screen.command(b'f');
    assert!(
        wait_for_text(&screen, "search:"),
        "the search bar did not open: {:?}",
        screen.rendered()
    );
    screen.send(b"HIST-250");
    assert!(
        wait_for_text(&screen, "matches · Enter next"),
        "the search did not count the match: {:?}",
        screen.rendered()
    );
    assert!(
        screen.rendered().contains("0/1 matches"),
        "the search found the wrong number of matches: {:?}",
        screen.rendered()
    );
    screen.send(b"\r");
    assert!(
        common::wait_until(secs(10), || {
            let rendered = screen.rendered();
            rendered.contains("HIST-250") && rendered.contains("1/1 matches")
        }),
        "Enter did not jump to the match: {:?}",
        screen.rendered()
    );
    assert_eq!(terminal_read_calls(&scratch), 0, "search read a terminal");
    assert_doc_untouched(&scratch, version, writes, "scrollback search");
    assert_eq!(resize_frames(&scratch), resizes, "search resized a pane");

    screen.send(&[0x1b]);
    assert!(
        common::wait_until(secs(10), || !screen.rendered().contains("search:")),
        "Esc did not close the search bar: {:?}",
        screen.rendered()
    );
}

/// A selection copied with `y` reaches the outer terminal as OSC 52, without
/// padding. An empty selection never emits a clipboard sequence.
#[cfg(unix)]
fn workspace_copy_emits_osc52_and_empty_selection_copies_nothing() {
    let scratch = Scratch::new("m6-copy");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-copy");
    assert!(
        wait_for_text(&screen, "hello"),
        "the copy fixture did not paint: {:?}",
        screen.rendered()
    );

    screen.command(b'[');
    assert!(
        wait_for_text(&screen, "selection:"),
        "selection mode did not open: {:?}",
        screen.rendered()
    );
    screen.send(b"hhhhh");
    screen.send(b"y");
    let expected = format!(
        "\x1b]52;c;{}\x07",
        herdr_terminal::clipboard::base64(b"hello")
    );
    assert!(
        common::wait_until(secs(10), || screen.raw().contains(&expected)),
        "the copy did not emit OSC 52 for \"hello\": {:?}",
        screen.tail()
    );
    assert!(
        wait_for_text(&screen, "copied 5 characters"),
        "the copy did not report five characters: {:?}",
        screen.rendered()
    );
    assert_eq!(terminal_read_calls(&scratch), 0, "copy read a terminal");

    // At the cursor, a fresh selection covers one blank cell: nothing to copy.
    let sequences = screen.raw().matches("\x1b]52;c;").count();
    screen.command(b'[');
    screen.send(b"y");
    assert!(
        wait_for_text(&screen, "nothing selected"),
        "the empty copy was not reported: {:?}",
        screen.rendered()
    );
    assert_eq!(
        screen.raw().matches("\x1b]52;c;").count(),
        sequences,
        "an empty selection emitted a clipboard sequence: {:?}",
        screen.tail()
    );
}

/// A paste is framed only when the focused program enabled bracketed paste,
/// and an open overlay consumes it instead.
#[cfg(unix)]
fn workspace_paste_is_bracketed_and_overlays_consume_it() {
    let scratch = Scratch::new("m6-paste");
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-paste");
    assert!(
        wait_for_text(&screen, "PASTE-READY"),
        "the paste fixture did not paint: {:?}",
        screen.rendered()
    );
    let paste = "\x1b[200~hello paste\x1b[201~";
    screen.send(paste.as_bytes());
    assert!(
        common::wait_until(secs(10), || {
            program_input(&scratch)
                .iter()
                .any(|(target, text)| target == "t1" && text == paste)
        }),
        "the paste was not bracketed to the focused program: {:?}",
        program_input(&scratch)
    );
    assert!(
        screen.raw().contains("\x1b[?2004h"),
        "the outer terminal was not put in bracketed-paste mode"
    );

    let after_paste = program_input(&scratch).len();
    screen.command(b':');
    assert!(
        wait_for_text(&screen, "palette · Enter run · Esc close"),
        "the palette did not open: {:?}",
        screen.rendered()
    );
    screen.send(b"second paste");
    screen.send(&[0x1b]);
    assert!(
        common::wait_until(secs(10), || !screen.rendered().contains("palette ·")),
        "the palette did not close: {:?}",
        screen.rendered()
    );
    screen.send(b"w");
    assert!(
        common::wait_until(secs(10), || {
            program_input(&scratch).iter().any(|(_, text)| text == "w")
        }),
        "the client stalled after the overlay paste: {:?}",
        program_input(&scratch)
    );
    let extra: Vec<String> = program_input(&scratch)
        .into_iter()
        .skip(after_paste)
        .map(|(_, text)| text)
        .collect();
    assert!(
        extra.iter().all(|text| !text.contains("second paste")),
        "a paste reached the program through an overlay: {extra:?}"
    );
}

/// The inner bracketed-paste framing follows the focused program's DEC 2004
/// mode: without it the text is forwarded unframed.
#[cfg(unix)]
fn workspace_paste_frames_only_when_the_program_enabled_it() {
    let plain = Scratch::new("m6-paste-plain");
    let mut screen = FakeScreen::spawn_workspace(&plain, "m6-multi");
    assert!(
        wait_for_text(&screen, "PANE-ONE"),
        "the plain program did not paint: {:?}",
        screen.rendered()
    );
    screen.send(b"\x1b[200~plain paste\x1b[201~");
    assert!(
        common::wait_until(secs(10), || {
            program_input(&plain)
                .iter()
                .any(|(target, text)| target == "t1" && text == "plain paste")
        }),
        "the paste was not forwarded unframed: {:?}",
        program_input(&plain)
    );
    assert!(
        program_input(&plain)
            .iter()
            .all(|(_, text)| !text.contains("\x1b[200~")),
        "a program without DEC 2004 received bracketed-paste framing: {:?}",
        program_input(&plain)
    );

    let framed = Scratch::new("m6-paste-framed");
    let mut screen = FakeScreen::spawn_workspace(&framed, "m6-paste");
    assert!(
        wait_for_text(&screen, "PASTE-READY"),
        "the bracketed program did not paint: {:?}",
        screen.rendered()
    );
    let expected = "\x1b[200~plain paste\x1b[201~";
    screen.send(b"\x1b[200~plain paste\x1b[201~");
    assert!(
        common::wait_until(secs(10), || {
            program_input(&framed)
                .iter()
                .any(|(target, text)| target == "t1" && text == expected)
        }),
        "a program with DEC 2004 did not receive the framed paste: {:?}",
        program_input(&framed)
    );
}

/// Config is read before the UI starts: theme and rebound keys apply, and a
/// malformed file warns on stderr while the UI still runs.
#[cfg(unix)]
fn workspace_config_sets_theme_and_keys_and_malformed_config_warns() {
    let scratch = Scratch::new("m6-config");
    scratch.seed_config(r#"{"theme":"light","keys":{"new_tab":"t"}}"#);
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-multi");
    assert!(
        wait_for_text(&screen, "PANE-ONE"),
        "{:?}",
        screen.rendered()
    );
    assert!(
        screen.raw().contains("38;2;30;102;245"),
        "the light theme accent was not drawn in truecolor: {:?}",
        screen.tail()
    );
    assert!(
        screen.raw().contains("\x1b[?1006h"),
        "mouse reporting was not enabled by default"
    );
    assert!(
        !screen.errors().contains("warning"),
        "a valid config warned: {:?}",
        screen.errors()
    );

    screen.command(b't');
    assert!(
        wait_for_text(&screen, "PANE-TWO"),
        "the rebound new-tab key did not run: {:?}",
        screen.rendered()
    );
    screen.command(b':');
    assert!(
        wait_for_text(&screen, "palette · Enter run · Esc close"),
        "the palette did not open: {:?}",
        screen.rendered()
    );
    assert!(
        wait_for_text(&screen, "^b t"),
        "the palette did not show the configured key hint: {:?}",
        screen.rendered()
    );
    screen.send(&[0x1b]);

    let bad = Scratch::new("m6-config-bad");
    bad.seed_config("{not json");
    let screen = FakeScreen::spawn_workspace(&bad, "m6-multi");
    assert!(
        wait_for_text(&screen, "PANE-ONE"),
        "a malformed config stopped the UI: stderr={:?} rendered={:?}",
        screen.errors(),
        screen.rendered()
    );
    assert!(
        screen.errors().contains("ht: warning:") && screen.errors().contains("not valid JSON"),
        "a malformed config was not reported: {:?}",
        screen.errors()
    );

    // `mouse: false` keeps the outer terminal out of mouse reporting.
    let no_mouse = Scratch::new("m6-config-no-mouse");
    no_mouse.seed_config(r#"{"mouse":false}"#);
    let screen = FakeScreen::spawn_workspace(&no_mouse, "m6-multi");
    assert!(
        wait_for_text(&screen, "PANE-ONE"),
        "the workspace did not start with mouse off: {:?}",
        screen.rendered()
    );
    assert!(
        !screen.raw().contains("\x1b[?1006h"),
        "mouse off still asked the outer terminal for mouse events: {:?}",
        screen.tail()
    );
}

/// Four MiB of program output is consumed with credit and leaves the client
/// responsive; the history is bounded rather than growing without limit.
#[cfg(unix)]
fn workspace_processes_a_large_output_burst() {
    let scratch = Scratch::new("m6-large");
    let started = std::time::Instant::now();
    let mut screen = FakeScreen::spawn_workspace(&scratch, "m6-large");
    assert!(
        wait_for_text(&screen, "LARGE-END"),
        "the large burst did not paint within the bound: {:?}",
        screen.tail()
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed < secs(15),
        "4 MiB of output took {elapsed:?}, above the 15s bound"
    );
    assert!(
        scratch
            .entries()
            .iter()
            .any(|entry| entry["frame"]["kind"] == json!(1)),
        "the client never sent a credit frame: {:?}",
        scratch.entries()
    );
    assert_eq!(
        terminal_read_calls(&scratch),
        0,
        "the burst read a terminal"
    );

    // Still interactive after the burst.
    screen.command(b':');
    assert!(
        wait_for_text(&screen, "palette · Enter run · Esc close"),
        "the client was unresponsive after the burst: {:?}",
        screen.rendered()
    );
}

// --- observer attach -------------------------------------------------------

/// A client connected straight to the scripted core, without `ht` in the way.
/// The wrapper is a small shell script so the scenario reaches the child
/// `hc connect` without touching this process's environment.
#[cfg(unix)]
fn observer_client(scratch: &Scratch, scenario: &str) -> CoreClient {
    use std::os::unix::fs::PermissionsExt;
    let exe = std::env::current_exe().expect("test executable");
    let wrapper = scratch.dir.join("hc");
    let script = format!(
        "#!/bin/sh\nFAKE_SCENARIO='{scenario}'\nFAKE_LOG='{}'\nexport FAKE_SCENARIO FAKE_LOG\nexec '{}' \"$@\"\n",
        scratch.log().display(),
        exe.display()
    );
    std::fs::write(&wrapper, script).expect("write hc wrapper");
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))
        .expect("make wrapper executable");
    CoreClient::connect(Some(&wrapper)).expect("connect to the scripted core")
}

#[cfg(unix)]
fn next_item(attached: &Attached) -> StreamItem {
    attached
        .stream
        .recv_timeout(secs(10))
        .expect("an attach stream item")
}

/// The fake core's JSON log, one record per line. A line still being written
/// by the child is skipped; wait conditions decide when enough has arrived.
#[cfg(unix)]
fn log_records(scratch: &Scratch) -> Vec<Value> {
    std::fs::read_to_string(scratch.log())
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

#[cfg(unix)]
fn logged_frames(scratch: &Scratch) -> Vec<Value> {
    log_records(scratch)
        .into_iter()
        .filter_map(|record| record.get("frame").cloned())
        .collect()
}

#[cfg(unix)]
fn logged_credits(scratch: &Scratch) -> Vec<u64> {
    logged_frames(scratch)
        .iter()
        .filter(|frame| frame["kind"] == json!(1))
        .filter_map(|frame| frame["credit"].as_u64())
        .collect()
}

/// The channel the fake core handed out for the first attach.
#[cfg(unix)]
fn attach_channel(scratch: &Scratch) -> u64 {
    log_records(scratch)
        .iter()
        .find_map(|record| record["attached"].as_u64())
        .expect("the fake core logged an attach")
}

/// A confirmed observer receives RESIZE before each paint; the grid is
/// applied before the bytes it describes, and only DATA is credited.
#[cfg(unix)]
fn observer_attach_negotiates_resize_notifications_and_applies_them_in_order() {
    let scratch = Scratch::new("observe-new");
    let client = observer_client(&scratch, "observe-new");
    let attached = client
        .attach_observer("t1", 90, 30, 0)
        .expect("observer attach is supported");
    assert!(
        attached.resize_notifications,
        "the reply confirmed resize notifications"
    );

    let mut vt = herdr_terminal::vt::Vt::new(90, 30).expect("local VT");
    let mut order = Vec::new();
    loop {
        match next_item(&attached) {
            StreamItem::Resize { cols, rows } => {
                vt.resize(cols, rows).expect("resize the local VT");
                order.push(format!("resize {cols}x{rows}"));
            }
            StreamItem::Data(bytes) => {
                vt.feed(&bytes);
                client
                    .credit(attached.channel, bytes.len())
                    .expect("pay credit for data only");
                order.push(format!("data {}", bytes.len()));
            }
            StreamItem::Closed(_) => break,
            StreamItem::Lost => panic!("the stream was lost"),
        }
    }
    assert_eq!(
        order,
        ["resize 40x12", "data 32", "resize 20x6", "data 20"],
        "each RESIZE precedes the paint it describes"
    );
    let screen = vt.snapshot().expect("snapshot");
    assert_eq!(
        (screen.cols, screen.rows),
        (20, 6),
        "the VT ends at the new grid"
    );
    assert!(
        screen.lines().join("\n").contains("BBBB"),
        "the second paint was laid out for the new grid: {:?}",
        screen.lines()
    );

    // The attach asked as an observer and the connection sent nothing back
    // except DATA credit: no keys, RESIZE, FOCUS or FACTS.
    let records = log_records(&scratch);
    let attach = records
        .iter()
        .find(|record| record["call"] == "terminal.attach")
        .expect("terminal.attach was called");
    assert_eq!(attach["params"]["observe"], json!(true));
    assert_eq!(attach["params"]["resize_notifications"], json!(true));
    assert!(
        common::wait_until(secs(5), || {
            let credits = logged_credits(&scratch);
            credits.contains(&32) && credits.contains(&20)
        }),
        "the client credited both DATA frames"
    );
    let frames = logged_frames(&scratch);
    let channel = json!(attached.channel);
    for kind in [0, 3, 4, 5] {
        assert!(
            !frames
                .iter()
                .any(|frame| frame["channel"] == channel && frame["kind"] == json!(kind)),
            "an observer sent frame kind {kind} on its stream: {frames:?}"
        );
    }
    let mut credits = logged_credits(&scratch);
    credits.sort_unstable();
    assert_eq!(
        credits,
        [20, 32],
        "RESIZE pays no credit; only the two DATA frames do"
    );
}

/// Without the feature there is no observer path, and a normal attach still
/// carries no observer fields.
#[cfg(unix)]
fn observer_attach_is_unavailable_without_the_feature_and_normal_attach_is_unchanged() {
    let scratch = Scratch::new("observe-missing");
    let client = observer_client(&scratch, "observe-missing");
    assert!(!client.hello().supports("resize_notifications"));
    let Err(error) = client.attach_observer("t1", 80, 24, 0) else {
        panic!("an observer attach needs the feature");
    };
    assert!(
        matches!(error, Error::Protocol(_)),
        "clear protocol error, got {error:?}"
    );
    assert!(
        error.to_string().contains("resize_notifications"),
        "the error names the missing feature: {error}"
    );
    assert!(
        !log_records(&scratch)
            .iter()
            .any(|record| record["call"] == "terminal.attach"),
        "the client must not attach as a normal viewer instead"
    );

    let attached = client
        .attach("t1", 80, 24, 0)
        .expect("a normal attach still works");
    assert!(!attached.resize_notifications);
    assert!(
        matches!(next_item(&attached), StreamItem::Data(bytes) if bytes.as_slice() == b"NORMAL\n")
    );
    assert!(matches!(next_item(&attached), StreamItem::Closed(_)));

    let records = log_records(&scratch);
    let attach = records
        .iter()
        .find(|record| record["call"] == "terminal.attach")
        .expect("terminal.attach was called");
    let params = attach["params"].as_object().expect("params");
    assert!(
        !params.contains_key("observe"),
        "normal attach params are unchanged: {params:?}"
    );
    assert!(
        !params.contains_key("resize_notifications"),
        "normal attach params are unchanged: {params:?}"
    );
}

/// A reply that does not confirm the ordering is refused, and the channel is
/// closed instead of being left registered.
#[cfg(unix)]
fn observer_attach_requires_a_confirmed_reply_and_leaves_no_channel() {
    let scratch = Scratch::new("observe-unconfirmed");
    let client = observer_client(&scratch, "observe-unconfirmed");
    let Err(error) = client.attach_observer("t1", 80, 24, 0) else {
        panic!("an unconfirmed reply cannot be used");
    };
    assert!(
        matches!(error, Error::Protocol(_)),
        "clear protocol error, got {error:?}"
    );
    assert!(
        error.to_string().contains("confirm"),
        "the error says the reply did not confirm: {error}"
    );
    assert!(
        common::wait_until(secs(5), || {
            logged_frames(&scratch)
                .iter()
                .any(|frame| frame["kind"] == json!(2))
        }),
        "the abandoned channel was closed"
    );
    // The connection is still usable and nothing was registered for it.
    assert!(
        client.get("t1").is_ok(),
        "the connection survived abandoning the channel"
    );
}

/// A RESIZE that is not four bytes is a protocol error: the connection ends
/// and the stream reports Lost, without a panic.
#[cfg(unix)]
fn a_malformed_resize_ends_the_connection_without_a_panic() {
    let scratch = Scratch::new("observe-bad-resize");
    let client = observer_client(&scratch, "observe-bad-resize");
    let attached = client
        .attach_observer("t1", 80, 24, 0)
        .expect("the attach reply confirmed");
    assert!(
        matches!(next_item(&attached), StreamItem::Data(bytes) if bytes.as_slice() == b"READY\n")
    );
    client
        .credit(attached.channel, 6)
        .expect("credit the paint; this asks for the malformed frame");
    match next_item(&attached) {
        StreamItem::Lost => {}
        other => panic!("expected the stream to end as lost, got {other:?}"),
    }
    assert!(
        matches!(
            client.call("terminal.get", json!({"terminal": "t1"})),
            Err(Error::Closed)
        ),
        "a malformed frame ends the connection"
    );
}

/// A RESIZE on an attach stream is applied before the DATA after it: 30
/// cells wrap at 20, and the client never echoes the frame or credits it.
#[cfg(unix)]
fn screen_applies_a_resize_before_its_data_and_sends_none_back() {
    let scratch = Scratch::new("screen-resize-frames");
    let screen = FakeScreen::spawn(
        &scratch,
        "screen-resize-frames",
        &["--", "sh", "-c", "sleep 30"],
    );
    assert!(
        screen.wait_text("AFTER", secs(10)),
        "the paint arrived; log={:?}",
        log_records(&scratch)
    );
    let rendered = screen.rendered();
    assert!(
        rendered.contains("xxxxxxxxxxxxxxxxxxxx\nxxxxxxxxxx\nAFTER"),
        "the data was laid out for the RESIZE grid, not the outer one: {rendered:?}"
    );
    assert!(
        common::wait_until(secs(5), || logged_credits(&scratch).contains(&38)),
        "the screen credited the DATA bytes"
    );
    let frames = logged_frames(&scratch);
    assert!(
        !frames.iter().any(|frame| frame["kind"] == json!(3)),
        "the screen echoed a RESIZE: {frames:?}"
    );
    assert!(
        !logged_credits(&scratch).contains(&4),
        "the RESIZE was credited: {:?}",
        logged_credits(&scratch)
    );
}

/// The workspace's pane drain applies a RESIZE to its pane VT before the
/// following DATA, without sending RESIZE or credit for it.
#[cfg(unix)]
fn workspace_applies_a_resize_before_its_data_and_sends_none_back() {
    let scratch = Scratch::new("workspace-resize-frames");
    let workspace = FakeScreen::spawn_workspace(&scratch, "workspace-resize-frames");
    assert!(
        workspace.wait_text("AFTER", secs(10)),
        "the pane painted; log={:?}",
        log_records(&scratch)
    );
    let rendered = workspace.rendered();
    let lines: Vec<&str> = rendered.lines().collect();
    let first = lines
        .iter()
        .position(|line| line.contains('x'))
        .expect("the paint is visible");
    assert_eq!(
        lines[first].matches('x').count(),
        20,
        "the first row wraps at the RESIZE grid: {rendered:?}"
    );
    assert_eq!(
        lines[first + 1].matches('x').count(),
        10,
        "the remainder is on the next row: {rendered:?}"
    );
    assert!(
        lines[first + 2].contains("AFTER"),
        "the data after the wrap is present: {rendered:?}"
    );
    assert!(
        common::wait_until(secs(5), || logged_credits(&scratch).contains(&38)),
        "the pane credited the DATA bytes"
    );
    let frames = logged_frames(&scratch);
    assert!(
        !frames.iter().any(|frame| frame["kind"] == json!(3)),
        "the pane echoed a RESIZE: {frames:?}"
    );
    assert!(
        !logged_credits(&scratch).contains(&4),
        "the RESIZE was credited: {:?}",
        logged_credits(&scratch)
    );
}

/// A resize and its paint that arrive before the attach reply are queued in
/// order; once registered, the live route keeps delivering. The later pair is
/// sent only after the client credits the early paint, so a client that
/// treated the early RESIZE as an ending would never see it.
#[cfg(unix)]
fn an_early_resize_before_the_reply_keeps_the_live_route() {
    let scratch = Scratch::new("observe-early-frames");
    let client = observer_client(&scratch, "observe-early-frames");
    let attached = client
        .attach_observer("t1", 90, 30, 0)
        .expect("observer attach");
    assert!(attached.resize_notifications);

    let mut vt = herdr_terminal::vt::Vt::new(90, 30).expect("local VT");
    let mut order = Vec::new();
    loop {
        match next_item(&attached) {
            StreamItem::Resize { cols, rows } => {
                vt.resize(cols, rows).expect("resize the local VT");
                order.push(format!("resize {cols}x{rows}"));
            }
            StreamItem::Data(bytes) => {
                vt.feed(&bytes);
                client
                    .credit(attached.channel, bytes.len())
                    .expect("credit the paint");
                order.push(format!("data {}", bytes.len()));
            }
            StreamItem::Closed(_) => break,
            StreamItem::Lost => panic!("the early RESIZE ended the stream"),
        }
    }
    assert_eq!(
        order,
        ["resize 40x12", "data 6", "resize 20x6", "data 5"],
        "early items keep their order and the live route keeps delivering"
    );
    let screen = vt.snapshot().expect("snapshot");
    assert_eq!((screen.cols, screen.rows), (20, 6));
}

/// Once a workspace pane is observed, neither keyboard nor mouse input reaches
/// it; a purely local command still works afterwards, which proves the input
/// was processed.
#[cfg(unix)]
fn a_workspace_observer_sends_no_keyboard_or_mouse_input() {
    let scratch = Scratch::new("workspace-resize-mouse");
    let mut workspace = FakeScreen::spawn_workspace(&scratch, "workspace-resize-mouse");
    assert!(
        wait_for_text(&workspace, "MOUSE-READY"),
        "the tracking program painted; log={:?}",
        log_records(&scratch)
    );
    let rendered = workspace.rendered();
    let lines: Vec<&str> = rendered.lines().collect();
    let first = lines
        .iter()
        .position(|line| line.contains('x'))
        .expect("the paint is visible");
    assert_eq!(lines[first].matches('x').count(), 20, "{rendered:?}");
    assert_eq!(lines[first + 1].matches('x').count(), 10, "{rendered:?}");
    // The paint's credit proves the RESIZE was drained and the link is
    // observed before the input below. Frames already logged are startup
    // traffic, not a response to it.
    assert!(
        common::wait_until(secs(5), || logged_credits(&scratch).contains(&68)),
        "the pane credited its paint: {:?}",
        logged_credits(&scratch)
    );
    let marker = logged_frames(&scratch).len();

    // Wheel, press, drag with the button held, and release over the pane.
    workspace.send(wheel_up(10, 5).as_bytes());
    workspace.send(sgr(0, false, 10, 5).as_bytes());
    workspace.send(sgr(32, false, 14, 5).as_bytes());
    workspace.send(sgr(0, true, 14, 5).as_bytes());
    workspace.send(b"k");
    // The help overlay is handled locally, so seeing it proves the input
    // above went through the loop before this point.
    workspace.command(b'?');
    assert!(
        wait_for_text(&workspace, "any key closes help"),
        "the local help overlay did not open: {:?}",
        workspace.rendered()
    );

    assert!(
        program_input(&scratch).is_empty(),
        "an observed pane received input: {:?}",
        program_input(&scratch)
    );
    let channel = attach_channel(&scratch);
    let frames = logged_frames(&scratch);
    for kind in [0, 3, 4, 5] {
        assert!(
            !frames[marker..].iter().any(|frame| {
                frame["channel"] == json!(channel) && frame["kind"] == json!(kind)
            }),
            "an observed pane sent frame kind {kind} after the input: {:?}",
            &frames[marker..]
        );
    }
}

/// The single-pane screen is observed after its RESIZE; a key and a mouse
/// event must not be sent, and nothing is echoed back. Ctrl-] still detaches.
#[cfg(unix)]
fn a_screen_observer_sends_no_input_and_no_echo() {
    let scratch = Scratch::new("screen-resize-input");
    let mut screen = FakeScreen::spawn(
        &scratch,
        "screen-resize-input",
        &["--", "sh", "-c", "sleep 30"],
    );
    assert!(
        screen.wait_text("AFTER", secs(10)),
        "the paint arrived; log={:?}",
        log_records(&scratch)
    );
    assert!(
        screen
            .rendered()
            .contains("xxxxxxxxxxxxxxxxxxxx\nxxxxxxxxxx\nAFTER"),
        "the RESIZE grid was applied: {:?}",
        screen.rendered()
    );
    // The paint's credit proves the RESIZE was drained and the screen is
    // observed before the input below.
    assert!(
        common::wait_until(secs(5), || logged_credits(&scratch).contains(&62)),
        "the screen credited its paint: {:?}",
        logged_credits(&scratch)
    );
    let marker = logged_frames(&scratch).len();

    screen.send(b"k");
    screen.send(click(10, 5).as_bytes());
    screen.send(&[0x1d]);
    let status = screen
        .wait_exit(secs(10))
        .expect("ht screen exits after Ctrl-]");
    assert_eq!(status.code(), Some(0), "stderr={:?}", screen.stderr.text());

    assert!(
        program_input(&scratch).is_empty(),
        "an observed screen received input: {:?}",
        program_input(&scratch)
    );
    let channel = attach_channel(&scratch);
    let frames = logged_frames(&scratch);
    for kind in [0, 3, 4, 5] {
        assert!(
            !frames[marker..].iter().any(|frame| {
                frame["channel"] == json!(channel) && frame["kind"] == json!(kind)
            }),
            "an observed screen sent frame kind {kind} after the input: {:?}",
            &frames[marker..]
        );
    }
}

fn fake_connect() -> io::Result<()> {
    let scenario = std::env::var("FAKE_SCENARIO").unwrap_or_else(|_| "finished".into());
    let log_path = std::env::var_os("FAKE_LOG").map(PathBuf::from);
    let log_path = log_path.as_deref();
    let doc_path = log_path.map(|path| path.with_file_name("docs.json"));
    let doc_path = doc_path.as_deref();
    let label_path = log_path.map(|path| path.with_file_name("labels.json"));
    let label_path = label_path.as_deref();
    let generation = bump_generation(log_path);
    log(log_path, json!({"generation": generation}));
    // The name of the start whose reply will be dropped, so the next
    // connection can present the terminal it created.
    let pending_name = last_start_name(log_path);
    // Every run so far, with the labels the client sent: a realistic snapshot
    // reports the labels the terminal actually carries.
    let started = started_terminals(log_path);

    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut output = stdout.lock();

    // Plain line mode: one hello, one line back, then framed for good.
    let mut line = String::new();
    input.read_line(&mut line)?;
    let hello: serde_json::Value =
        serde_json::from_str(line.trim()).map_err(|error| invalid(error.to_string()))?;
    assert_eq!(hello["method"], "hello");
    let mut features = vec!["framed", "attach", "events"];
    if scenario.starts_with("observe") && scenario != "observe-missing" {
        // A relay path can lose support; an old core simply never had it.
        features.push("resize_notifications");
    }
    let result = json!({
        "generation": 1,
        "version": "9.9.9-fake",
        "core": {"id": "fake", "name": "fake"},
        "framed": true,
        "methods": [
            "events.subscribe", "terminal.run", "terminal.attach",
            "terminal.get", "terminal.read", "terminal.rm", "terminal.kill",
            "terminal.label", "doc.get", "doc.set"
        ],
        "features": features,
    });
    writeln!(
        output,
        "{}",
        json!({"jsonrpc": "2.0", "id": hello["id"], "result": result})
    )?;
    output.flush()?;

    let mut next_channel = ATTACH_CHANNEL;
    let mut attached_channels: BTreeSet<u32> = BTreeSet::new();
    let mut attached_once = false;
    // The last start's labels and address, so later attaches and terminal.get
    // can present the terminal the client started.
    let mut last_run_labels: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut last_run_address = "t1".to_owned();
    // Every start's address and labels, for merging label calls realistically.
    let mut run_labels: BTreeMap<String, serde_json::Map<String, Value>> = BTreeMap::new();
    // Sequence numbers for scripted agent events, above the snapshot's.
    let mut event_seq = 100u64;
    // How far a scripted agent sequence has progressed on attach-stream keys.
    let mut agent_step = 0u32;
    // `agents-label-fail`: fail this many label writes before accepting one.
    let mut label_failures = 2u32;
    // `agents-slow-fail`: the first label reply is delayed past the client's
    // timeout; every later call fails. Count every call.
    let mut label_calls_seen = 0u32;
    // A kept terminal whose program was lost and not yet replaced.
    let mut parked = false;
    let mut triggered = false;
    let mut runs = 0u32;
    // Two-pane `missed` script: attach counts drive both the first paint and
    // the resnapshot once both panes have attached.
    let mut t1_attaches = 0u32;
    let mut t2_attaches = 0u32;
    let mut missed_sent = false;
    // `doc-conflict-flush`: the first four CAS writes lose to another screen:
    // three exhaust one loop's retry budget, the fourth lands on the detach
    // flush so its merge-and-retry path is what writes the local change.
    let mut flush_conflicts = 4u32;
    // `observe-bad-resize`: the malformed frame waits for the client to pay
    // credit for the paint, which proves the attach stream is registered.
    let mut bad_resize_sent = false;
    // `observe-early-frames`: the later pair is sent only after the client
    // credits the early paint, which proves the live route was registered.
    let mut late_frames_sent = false;

    while let Some((channel, kind, payload)) = read_frame(&mut input)? {
        let mut frame = json!({"channel": channel, "kind": kind, "len": payload.len()});
        // RESIZE carries exactly the two u16s the client asked for, so tests
        // can assert the grid size after a drag without guessing.
        if kind == 3 && payload.len() >= 4 {
            frame["cols"] = json!(u16::from_le_bytes([payload[0], payload[1]]));
            frame["rows"] = json!(u16::from_le_bytes([payload[2], payload[3]]));
        }
        // CREDIT says how many stream bytes the client consumed, which is how
        // a test can tell RESIZE (never credited) from DATA.
        if kind == 1 && payload.len() == 4 {
            frame["credit"] = json!(u32::from_le_bytes([
                payload[0], payload[1], payload[2], payload[3]
            ]));
        }
        log(log_path, json!({"frame": frame}));

        // The client closing its stream: forget the channel, so a later
        // attach is treated as a fresh paint.
        if kind == CLOSE {
            attached_channels.remove(&channel);
            continue;
        }

        if kind == 1
            && scenario == "observe-bad-resize"
            && attached_channels.contains(&channel)
            && !bad_resize_sent
        {
            bad_resize_sent = true;
            write_frame(&mut output, channel, 3, &[1, 2])?;
            write_frame(&mut output, channel, DATA, b"AFTER-BAD\n")?;
            continue;
        }

        if kind == 1
            && scenario == "observe-early-frames"
            && attached_channels.contains(&channel)
            && !late_frames_sent
        {
            late_frames_sent = true;
            write_frame(&mut output, channel, 3, &resize_payload(20, 6))?;
            write_frame(&mut output, channel, DATA, b"LATE\n")?;
            write_frame(
                &mut output,
                channel,
                CLOSE,
                br#"{"reason":"exited","exit_code":0}"#,
            )?;
            continue;
        }

        // A key on an attach stream drives the scripted lifecycle. Ctrl-]
        // never arrives here: the client handles it locally.
        if kind == DATA && attached_channels.contains(&channel) {
            log(
                log_path,
                json!({
                    "attach_data": {
                        "channel": channel,
                        "text": String::from_utf8_lossy(&payload),
                    }
                }),
            );
            if scenario.starts_with("agents") {
                agent_step += 1;
                agent_key_script(&scenario, agent_step, &mut output, &mut event_seq)?;
                continue;
            }
            if !triggered {
                triggered = true;
                match scenario.as_str() {
                    "screen-lost-restarted" => {
                        parked = true;
                        notify(&mut output, lost_event())?;
                        write_frame(
                            &mut output,
                            channel,
                            CLOSE,
                            br#"{"reason":"signaled","signal":9}"#,
                        )?;
                    }
                    "screen-missed" => notify(&mut output, missed_event())?,
                    "screen-removed" => notify(&mut output, removed_event())?,
                    // Core's fresh paint after a viewer fell behind: a soft
                    // reset, mode resets and a clear before the screen.
                    "screen-repaint" => write_frame(
                        &mut output,
                        channel,
                        DATA,
                        b"\x1b[!p\x1b[?1004l\x1b[H\x1b[2JREPAINTED\n",
                    )?,
                    // Drop the connection: the client must make a new one.
                    "screen-reconnect" | "screen-reconnect-gone" if generation == 1 => {
                        return Ok(());
                    }
                    // Another screen added a pane: the doc event carries no
                    // value, the client must refetch and adopt it.
                    "doc-event" | "doc-missed" | "doc-event-focus" => {
                        let doc = if scenario == "doc-event-focus" {
                            one_tab_doc(&[(3, "t1"), (4, "t2"), (5, "t3")])
                        } else {
                            doc_same_tab()
                        };
                        let version = replace_doc(doc_path, doc);
                        if scenario == "doc-missed" {
                            notify(&mut output, missed_two_panes_event())?;
                        } else {
                            notify(&mut output, doc_event_message(version))?;
                        }
                    }
                    // Another screen closed a pane.
                    "doc-remove" => {
                        let version = replace_doc(doc_path, doc_one_pane());
                        notify(&mut output, doc_event_message(version))?;
                    }
                    // The connection drops after another screen changed the
                    // document; the fresh connection must refetch it.
                    "doc-reconnect" => {
                        replace_doc(doc_path, doc_same_tab());
                        return Ok(());
                    }
                    // Another screen deleted the document. Core keeps the
                    // version, so it is recreated at that version.
                    "doc-deleted" => {
                        let version = replace_doc(doc_path, Value::Null);
                        notify(&mut output, doc_event_message(version))?;
                    }
                    _ => {}
                }
            }
            continue;
        }
        if channel != 0 || kind != DATA {
            continue;
        }
        let Ok(message) = serde_json::from_slice::<serde_json::Value>(&payload) else {
            continue;
        };
        let id = message["id"].clone();
        let method = message["method"].as_str().unwrap_or_default();
        log(
            log_path,
            json!({"call": method, "params": message.get("params")}),
        );
        match method {
            "events.subscribe" => {
                let snapshot =
                    snapshot_for(&scenario, generation, pending_name.as_deref(), &started);
                reply(&mut output, &id, snapshot)?;
            }
            "terminal.run" => {
                runs += 1;
                if scenario == "workspace-pending" && generation == 1 && runs == 2 {
                    // The terminal exists from now on, but the client never
                    // sees the reply: the whole connection ends here.
                    return Ok(());
                }
                let name = message["params"]["name"].as_str().unwrap_or_default();
                last_run_labels = message["params"]["labels"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                let record = if scenario == "workspace-pending" {
                    if runs == 1 {
                        named_record("t1", name, 4242)
                    } else {
                        // If the client repeats the run, the duplicate is
                        // visible in the log and gets a different address.
                        named_record("t3", name, 6161)
                    }
                } else if scenario.starts_with("agents") {
                    let address = if runs > 1 { "t2" } else { "t1" };
                    let mut record = live_record(4200 + runs);
                    record["address"] = json!(address);
                    record["name"] = json!(name);
                    record["labels"] = Value::Object(last_run_labels.clone());
                    record["reported"] = json!(true);
                    record["turn"] = json!(if address == "t2" && scenario != "agents-pin-event" {
                        1
                    } else if matches!(
                        scenario.as_str(),
                        "agents-pin"
                            | "agents-lost"
                            | "agents-fail"
                            | "agents-reconnect-blocked"
                            | "agents-missed-blocked"
                            | "agents-slow-fail"
                    ) {
                        2
                    } else {
                        0
                    });
                    record["title"] = json!(if address == "t2" { "Review" } else { "Build" });
                    record["status"] = if address == "t2" {
                        if scenario == "agents-hidden-done" {
                            agent_status("working", "codex", "Review", Some("test"), None, "")
                        } else if scenario == "agents-pin-event" {
                            agent_status("idle", "codex", "Review", None, None, "")
                        } else {
                            agent_status("done", "codex", "Review", None, None, "")
                        }
                    } else if scenario == "agents-turn" {
                        agent_status("idle", "claude", "Build", None, None, "")
                    } else {
                        agent_status(
                            "blocked",
                            "claude",
                            "Build",
                            Some("permission"),
                            None,
                            "allow?",
                        )
                    };
                    record["foreground"] = json!({
                        "pid": 77,
                        "name": if address == "t2" { "codex" } else { "claude" },
                        "argv": [if address == "t2" { "codex" } else { "claude" }, "run"],
                        "cwd": "/work",
                    });
                    record
                } else if scenario == "finished" {
                    record("finished")
                } else if scenario == "workspace-missed" && runs > 1 {
                    named_record("t2", name, 4444)
                } else if scenario.starts_with("m6-") {
                    // Interactive UI scenarios: every start gets its own
                    // address so a click can be traced to one terminal.
                    let address = match runs {
                        1 => "t1",
                        2 => "t2",
                        _ => "t3",
                    };
                    let mut record = live_record(4300 + runs);
                    record["address"] = json!(address);
                    record["name"] = json!(name);
                    record["labels"] = Value::Object(last_run_labels.clone());
                    record
                } else if doc_scenario(&scenario) {
                    // A split's terminal, with the labels the client sent.
                    let address =
                        if scenario.starts_with("doc-conflict") || scenario == "doc-program" {
                            "t3"
                        } else {
                            "t1"
                        };
                    let mut record = live_record(7000 + runs);
                    record["address"] = json!(address);
                    record["name"] = json!(name);
                    record["labels"] = Value::Object(last_run_labels.clone());
                    if scenario == "doc-conflict-merge" {
                        // Another screen wrote while this client was starting
                        // the split: its next write sees a stale version.
                        replace_doc(doc_path, doc_remote_space());
                    } else if scenario == "doc-conflict-tab" {
                        replace_doc(doc_path, doc_same_tab());
                    }
                    record
                } else {
                    live_record(4242)
                };
                last_run_address = record["address"].as_str().unwrap_or("t1").to_owned();
                run_labels.insert(last_run_address.clone(), last_run_labels.clone());
                let created = record.clone();
                reply(&mut output, &id, record)?;
                if scenario.starts_with("agents") {
                    // A real subscription sees the new terminal as `created`
                    // with its whole record; the run reply itself is only the
                    // session the client starts.
                    event_seq += 1;
                    notify(
                        &mut output,
                        agent_event(event_seq, "created", &last_run_address, created),
                    )?;
                }
            }
            "terminal.attach" => {
                let target = message["params"]["terminal"]
                    .as_str()
                    .unwrap_or("t1")
                    .to_owned();
                if scenario == "finished" {
                    reply_error(
                        &mut output,
                        &id,
                        "finished",
                        "t1 has finished",
                        "read what it left: hc read t1",
                    )?;
                } else if parked {
                    // The kept terminal has no program until core restarts
                    // it; the `restarted` event follows immediately.
                    parked = false;
                    log(log_path, json!({"attach_unavailable": "t1"}));
                    reply_error(
                        &mut output,
                        &id,
                        "unavailable",
                        "t1 is waiting for a new shell",
                        "it will restart soon",
                    )?;
                    notify(
                        &mut output,
                        event_message(3, "restarted", live_record(4343)),
                    )?;
                } else {
                    let first = !attached_once;
                    attached_once = true;
                    let channel = next_channel;
                    next_channel += 1;
                    attached_channels.insert(channel);
                    log(log_path, json!({"attached": channel, "terminal": target}));
                    let second_pane = target == "t2"
                        && matches!(scenario.as_str(), "workspace-pending" | "workspace-missed");
                    let attached_record = if second_pane {
                        named_record("t2", pending_name.as_deref().unwrap_or("ht-0-2"), 5151)
                    } else if scenario.starts_with("m6-") {
                        // Interactive scenarios attach several panes; the
                        // record must name the terminal that was attached.
                        let mut record = live_record(4242);
                        record["address"] = json!(target);
                        record
                    } else if doc_scenario(&scenario)
                        && target == last_run_address
                        && target != "t1"
                    {
                        let mut record = live_record(8000);
                        record["address"] = json!(target);
                        record["labels"] = Value::Object(last_run_labels.clone());
                        record
                    } else if doc_scenario(&scenario) {
                        doc_record_for(&scenario, &target)
                    } else {
                        live_record(4242)
                    };
                    let observe = message["params"]["observe"].as_bool().unwrap_or(false);
                    let asked = message["params"]["resize_notifications"]
                        .as_bool()
                        .unwrap_or(false);
                    let confirmed = observe && asked && scenario != "observe-unconfirmed";
                    if scenario == "observe-early-frames" {
                        // These reach the client before the attach reply does:
                        // the reader must queue them and keep the stream live.
                        write_frame(&mut output, channel, 3, &resize_payload(40, 12))?;
                        write_frame(&mut output, channel, DATA, b"EARLY\n")?;
                    }
                    reply(
                        &mut output,
                        &id,
                        json!({
                            "channel": channel,
                            "terminal": attached_record,
                            "resize_notifications": confirmed,
                        }),
                    )?;
                    match scenario.as_str() {
                        // The early pair already went out before the reply;
                        // the live pair waits for credit in the frame loop.
                        "observe-early-frames" => {}
                        // A confirmed observer gets the grid before each
                        // paint: the initial one, then a changed one whose
                        // paint supersedes pending output.
                        "observe-new" => {
                            write_frame(&mut output, channel, 3, &resize_payload(40, 12))?;
                            write_frame(
                                &mut output,
                                channel,
                                DATA,
                                b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
                            )?;
                            write_frame(&mut output, channel, 3, &resize_payload(20, 6))?;
                            write_frame(&mut output, channel, DATA, b"BBBBBBBBBBBBBBBBBBBB")?;
                            write_frame(
                                &mut output,
                                channel,
                                CLOSE,
                                br#"{"reason":"exited","exit_code":0}"#,
                            )?;
                        }
                        // The reply neither confirms nor supports the
                        // ordering, so the client must not use the stream.
                        "observe-unconfirmed" => {
                            write_frame(&mut output, channel, DATA, b"LEAK\n")?;
                        }
                        // A RESIZE that is not two u16s is a frame error. It
                        // waits for credit on the paint so the attach is
                        // registered before the frame arrives.
                        "observe-bad-resize" => {
                            write_frame(&mut output, channel, DATA, b"READY\n")?;
                        }
                        // A normal attach on a core without the feature.
                        "observe-missing" => {
                            write_frame(&mut output, channel, DATA, b"NORMAL\n")?;
                            write_frame(
                                &mut output,
                                channel,
                                CLOSE,
                                br#"{"reason":"exited","exit_code":0}"#,
                            )?;
                        }
                        // RESIZE on an attach stream, then output that wraps
                        // at the new grid. A viewer that ignores it would draw
                        // one 30-cell line instead. The `-input`/`-mouse`
                        // variants also enable mouse tracking so the client
                        // has input it must still refuse for an observer.
                        "screen-resize-frames" | "workspace-resize-frames" => {
                            write_frame(&mut output, channel, 3, &resize_payload(20, 5))?;
                            write_frame(
                                &mut output,
                                channel,
                                DATA,
                                b"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\r\nAFTER\n",
                            )?;
                        }
                        "screen-resize-input" => {
                            write_frame(&mut output, channel, 3, &resize_payload(20, 5))?;
                            write_frame(
                                &mut output,
                                channel,
                                DATA,
                                b"\x1b[?1000h\x1b[?1002h\x1b[?1006h\
                                  xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\r\nAFTER\n",
                            )?;
                        }
                        "workspace-resize-mouse" => {
                            write_frame(&mut output, channel, 3, &resize_payload(20, 5))?;
                            write_frame(
                                &mut output,
                                channel,
                                DATA,
                                b"\x1b[?1000h\x1b[?1002h\x1b[?1006h\
                                  xxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\r\nMOUSE-READY\n",
                            )?;
                        }
                        // The whole ending arrives as one burst: two DATA
                        // frames and CLOSE, so the last text is only in the
                        // VT when the close is seen.
                        "screen-final" => {
                            write_frame(&mut output, channel, DATA, b"\x1b[38;2;1;2;3mREADY\n")?;
                            write_frame(&mut output, channel, DATA, b"END\n")?;
                            write_frame(
                                &mut output,
                                channel,
                                CLOSE,
                                br#"{"reason":"exited","exit_code":0}"#,
                            )?;
                        }
                        // The same simultaneous DATA+CLOSE ending for the
                        // default workspace, with a non-zero status.
                        "workspace-final" => {
                            write_frame(&mut output, channel, DATA, b"\x1b[38;2;1;2;3mREADY\n")?;
                            write_frame(&mut output, channel, DATA, b"END\n")?;
                            write_frame(
                                &mut output,
                                channel,
                                CLOSE,
                                br#"{"reason":"exited","exit_code":7}"#,
                            )?;
                        }
                        "workspace-pending" => {
                            let text: &[u8] = if target == "t2" {
                                b"PANE-TWO\n"
                            } else {
                                b"PANE-ONE\n"
                            };
                            write_frame(&mut output, channel, DATA, text)?;
                        }
                        // Restored panes paint from core, and a terminal the
                        // document added later paints when it is adopted.
                        "doc-restore"
                        | "doc-remove"
                        | "doc-stale-snapshot"
                        | "doc-deleted-recover"
                        | "doc-focus" => {
                            let text: &[u8] = if target == "t2" {
                                b"PANE-TWO\n"
                            } else {
                                b"PANE-ONE\n"
                            };
                            write_frame(&mut output, channel, DATA, text)?;
                        }
                        "doc-view" => {
                            let text: &[u8] = if target == "t2" {
                                b"TAB-TWO\n"
                            } else {
                                b"TAB-ONE\n"
                            };
                            write_frame(&mut output, channel, DATA, text)?;
                        }
                        "doc-orphan" => {
                            let text: &[u8] = if target == "t7" {
                                b"ORPHAN-PANE\n"
                            } else {
                                b"PANE-ONE\n"
                            };
                            write_frame(&mut output, channel, DATA, text)?;
                        }
                        "doc-event" | "doc-event-focus" | "doc-missed" | "doc-reconnect"
                        | "doc-conflict-merge" | "doc-conflict-tab" | "doc-conflict-flush" => {
                            let text: &[u8] = match target.as_str() {
                                "t2" => b"PANE-TWO\n",
                                "t3" => b"SPLIT-PANE\n",
                                _ => b"PANE-ONE\n",
                            };
                            write_frame(&mut output, channel, DATA, text)?;
                        }
                        "doc-mismatch" | "doc-missing" | "doc-foreign" | "doc-fresh"
                        | "doc-deleted" | "doc-orphan-invalid" => {
                            write_frame(&mut output, channel, DATA, b"PANE-ONE\n")?;
                        }
                        // Agent scenarios paint their whole pane, and some
                        // emit the events the scenario is about once both
                        // panes are attached.
                        scenario if scenario.starts_with("agents") => {
                            let text: &[u8] = if target == "t2" {
                                b"PANE-TWO\n"
                            } else {
                                b"PANE-ONE\n"
                            };
                            write_frame(&mut output, channel, DATA, text)?;
                            if scenario == "agents-jump" && target == "t2" {
                                event_seq += 1;
                                notify(
                                    &mut output,
                                    agent_event(
                                        event_seq,
                                        "status",
                                        "t1",
                                        json!({
                                            "status": agent_status(
                                                "done", "claude", "Build", None, None, "ready"
                                            ),
                                            "records": {},
                                            "reported": true,
                                        }),
                                    ),
                                )?;
                                event_seq += 1;
                                notify(
                                    &mut output,
                                    agent_event(
                                        event_seq,
                                        "turn",
                                        "t1",
                                        json!({"turn": 1, "state": "done"}),
                                    ),
                                )?;
                            }
                            if scenario == "agents-pin-event" && target == "t2" {
                                let mut labels = base_labels(scenario, "t2", &run_labels);
                                labels.insert("herdr.pinned".into(), json!("1"));
                                event_seq += 1;
                                notify(
                                    &mut output,
                                    agent_event(
                                        event_seq,
                                        "labels",
                                        "t2",
                                        json!({"labels": labels}),
                                    ),
                                )?;
                            }
                        }
                        // Two panes attach, then the subscriber falls behind.
                        // The `missed` snapshot must reattach both in place.
                        "workspace-missed" => {
                            let nth = if target == "t2" {
                                t2_attaches += 1;
                                t2_attaches
                            } else {
                                t1_attaches += 1;
                                t1_attaches
                            };
                            let text: &[u8] = match (target.as_str(), nth) {
                                ("t2", 1) => b"PANE-TWO\n",
                                ("t2", _) => b"TWO-AGAIN\n",
                                (_, 1) => b"PANE-ONE\n",
                                (_, _) => b"ONE-AGAIN\n",
                            };
                            write_frame(&mut output, channel, DATA, text)?;
                            if !missed_sent && t1_attaches > 0 && t2_attaches > 0 {
                                missed_sent = true;
                                let name = last_start_name(log_path);
                                notify(
                                    &mut output,
                                    missed_workspace_event(name.as_deref(), &started),
                                )?;
                            }
                        }
                        "screen-detach" => {
                            write_frame(&mut output, channel, DATA, b"READY\n")?;
                        }
                        "m6-scroll" => {
                            // Enough history to scroll and to search.
                            let mut text = Vec::new();
                            for index in 0..300 {
                                text.extend_from_slice(format!("HIST-{index:03}\r\n").as_bytes());
                            }
                            text.extend_from_slice(b"LIVE-BOTTOM\r\n");
                            // Leave the cursor three rows above the bottom, so
                            // a three-line wheel scroll still has it inside
                            // the viewport and only the live-bottom rule can
                            // hide it.
                            text.extend_from_slice(b"\x1b[3A");
                            write_frame(&mut output, channel, DATA, &text)?;
                        }
                        "m6-large" => {
                            // Four MiB of program output, ended by a marker.
                            let line = b"0123456789abcdefghijklmnopqrstuvwxyz\r\n";
                            let mut text = Vec::with_capacity(4 * 1024 * 1024 + 16);
                            while text.len() < 4 * 1024 * 1024 {
                                text.extend_from_slice(line);
                            }
                            text.extend_from_slice(b"LARGE-END\r\n");
                            write_frame(&mut output, channel, DATA, &text)?;
                        }
                        "m6-mouse" => {
                            // Any-event tracking with SGR coordinates.
                            write_frame(
                                &mut output,
                                channel,
                                DATA,
                                b"\x1b[?1000h\x1b[?1002h\x1b[?1006hMOUSE-READY\n",
                            )?;
                        }
                        "m6-paste" => {
                            write_frame(&mut output, channel, DATA, b"\x1b[?2004hPASTE-READY\n")?;
                        }
                        "m6-kitty" => {
                            // Push disambiguate + report-event-types, so the
                            // program's key encoder emits press and release.
                            write_frame(&mut output, channel, DATA, b"\x1b[>3uKITTY-READY\n")?;
                        }
                        "m6-copy" => {
                            write_frame(&mut output, channel, DATA, b"hello")?;
                        }
                        "m6-multi" | "m6-palette" => {
                            let text: &[u8] = match target.as_str() {
                                "t2" => b"PANE-TWO\n",
                                "t3" => b"PANE-THREE\n",
                                _ => b"PANE-ONE\n",
                            };
                            write_frame(&mut output, channel, DATA, text)?;
                        }
                        "screen-lost-restarted" if first => {
                            write_frame(&mut output, channel, DATA, b"BEFORE\n")?;
                        }
                        "screen-lost-restarted" => {
                            write_frame(&mut output, channel, DATA, b"RESTARTED\n")?;
                        }
                        "screen-missed" if first => {
                            write_frame(&mut output, channel, DATA, b"BEFORE\n")?;
                        }
                        "screen-missed" => {
                            write_frame(&mut output, channel, DATA, b"RESYNCED\n")?;
                        }
                        "screen-removed" => {
                            write_frame(&mut output, channel, DATA, b"BEFORE\n")?;
                        }
                        "screen-repaint" => {
                            write_frame(&mut output, channel, DATA, b"BEFORE\n")?;
                        }
                        "screen-reconnect" | "screen-reconnect-gone" if generation == 1 => {
                            write_frame(&mut output, channel, DATA, b"BEFORE\n")?;
                        }
                        "screen-reconnect" => {
                            write_frame(&mut output, channel, DATA, b"RECONNECTED\n")?;
                        }
                        _ => {
                            // The probe cannot write this anywhere: its stdout
                            // is broken.
                            write_frame(&mut output, channel, DATA, b"PAINT\n")?;
                        }
                    }
                }
            }
            "terminal.get" => {
                let target = message["params"]["terminal"].as_str().unwrap_or("t1");
                if scenario == "doc-missing" && target == "t9" {
                    reply_error(
                        &mut output,
                        &id,
                        "not_found",
                        "no terminal t9",
                        "it is gone",
                    )?;
                    continue;
                }
                let record = if doc_scenario(&scenario) {
                    if target == last_run_address && target != "t1" {
                        let mut record = live_record(8000);
                        record["address"] = json!(target);
                        record["labels"] = Value::Object(last_run_labels.clone());
                        record
                    } else {
                        doc_record_for(&scenario, target)
                    }
                } else {
                    record("finished")
                };
                reply(&mut output, &id, record)?;
            }
            "terminal.read" => reply(
                &mut output,
                &id,
                json!({"text": "FAST", "complete": true, "how": "screen"}),
            )?,
            "terminal.rm" => reply(&mut output, &id, json!({"removed": "t1"}))?,
            "terminal.kill" => reply(&mut output, &id, record("finished"))?,
            "terminal.label" => {
                let target = message["params"]["terminal"]
                    .as_str()
                    .unwrap_or("t1")
                    .to_owned();
                label_calls_seen += 1;
                if scenario == "agents-slow-fail" {
                    if label_calls_seen == 1 {
                        // Past the client's 2 s label timeout, so the first
                        // call reports a timeout; a 10 s timeout would absorb
                        // this reply and silently succeed. Do not emit a
                        // labels event: the client's route has already timed
                        // out, and a late event would cancel the retry the
                        // persistent-failure path is meant to exercise.
                        std::thread::sleep(std::time::Duration::from_secs(3));
                        let mut record = live_record(6000);
                        record["address"] = json!(target);
                        record["labels"] = json!({"herdr.seen": "2"});
                        reply(&mut output, &id, record)?;
                    } else {
                        reply_error(
                            &mut output,
                            &id,
                            "io",
                            "the terminal store is busy",
                            "retry later",
                        )?;
                    }
                    continue;
                }
                if scenario == "agents-fail" && label_failures > 0 {
                    // A transient non-connection failure: the client must
                    // report it and retry with a bounded delay.
                    label_failures -= 1;
                    reply_error(
                        &mut output,
                        &id,
                        "io",
                        "the terminal store is busy",
                        "retry later",
                    )?;
                    continue;
                }
                if matches!(
                    scenario.as_str(),
                    "agents-lost" | "agents-reconnect-blocked"
                ) && generation == 1
                {
                    // The connection fails under a label write; the client
                    // must take its normal reconnect path.
                    reply_error(
                        &mut output,
                        &id,
                        "lost",
                        "the connection was lost",
                        "reconnect",
                    )?;
                    continue;
                }
                let mut store = read_labels(label_path);
                let mut current = store
                    .get(&target)
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_else(|| base_labels(&scenario, &target, &run_labels));
                for (key, value) in message["params"]["labels"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
                {
                    let text = value.as_str().unwrap_or_default();
                    if text.is_empty() {
                        current.remove(&key);
                    } else {
                        current.insert(key, value);
                    }
                }
                let labels = Value::Object(current.clone());
                store.insert(target.clone(), labels.clone());
                write_labels(label_path, &store);
                let mut record = live_record(6000);
                record["address"] = json!(target);
                record["labels"] = labels.clone();
                reply(&mut output, &id, record)?;
                event_seq += 1;
                notify(
                    &mut output,
                    agent_event(event_seq, "labels", &target, json!({"labels": labels})),
                )?;
                // The hidden-finish script: the label call is the deterministic
                // moment the sidebar is open, so the terminal can finish while
                // the person is not looking at it.
                if scenario == "agents-hidden-done"
                    && agent_step == 0
                    && message["params"]["labels"]["herdr.pinned"].is_string()
                {
                    agent_step += 1;
                    event_seq += 1;
                    notify(
                        &mut output,
                        agent_event(
                            event_seq,
                            "status",
                            "t2",
                            json!({
                                "status": agent_status("done", "codex", "Review", None, None, ""),
                                "records": {},
                                "reported": true,
                            }),
                        ),
                    )?;
                    event_seq += 1;
                    notify(
                        &mut output,
                        agent_event(event_seq, "turn", "t2", json!({"turn": 2, "state": "done"})),
                    )?;
                }
            }
            // Documents live in one file next to the log so every connection
            // and every screen sees the same store, like core's state file.
            "doc.get" => {
                let (value, version) = read_doc(doc_path);
                reply(
                    &mut output,
                    &id,
                    json!({"value": value, "version": version}),
                )?;
            }
            "doc.set" => {
                let requested = message["params"]["version"].as_u64().unwrap_or(0);
                let (stored, current) = read_doc(doc_path);
                if requested != current {
                    reply_error(
                        &mut output,
                        &id,
                        "conflict",
                        &format!("document is at version {current}, not {requested}"),
                        "read doc.get and retry with its version",
                    )?;
                } else if scenario == "doc-deleted-claim-lost"
                    && stored.is_null()
                    && requested == current
                {
                    // Another screen recreates the deleted document first and
                    // wins the CAS; this screen must restore it without a run.
                    let version = current + 1;
                    write_doc(doc_path, &empty_doc(), version);
                    reply_error(
                        &mut output,
                        &id,
                        "conflict",
                        &format!("document is at version {version}, not {requested}"),
                        "read doc.get and retry with its version",
                    )?;
                } else if scenario == "doc-conflict-flush" && flush_conflicts > 0 {
                    // Another screen writes before the next CAS attempt: the
                    // first three exhaust the loop's rounds, the fourth
                    // makes the detach flush merge and retry.
                    flush_conflicts -= 1;
                    replace_doc(doc_path, doc_remote_space());
                    reply_error(
                        &mut output,
                        &id,
                        "conflict",
                        "document changed while writing",
                        "read doc.get and retry with its version",
                    )?;
                } else {
                    let value = message["params"]["value"].clone();
                    let version = current + 1;
                    write_doc(doc_path, &value, version);
                    reply(&mut output, &id, json!({"version": version}))?;
                    notify(&mut output, doc_event_message(version))?;
                }
            }
            _ => reply(&mut output, &id, json!({}))?,
        }
    }
    Ok(())
}

/// Scenarios whose snapshot already contains the terminal to adopt, or whose
/// document names one.
fn scripted_terminal(scenario: &str) -> bool {
    doc_scenario(scenario)
        || matches!(
            scenario,
            "screen-lost-restarted"
                | "screen-missed"
                | "screen-removed"
                | "screen-reconnect"
                | "screen-repaint"
        )
}

/// M4 scenarios: a workspace document drives what is restored.
fn doc_scenario(scenario: &str) -> bool {
    matches!(
        scenario,
        "doc-restore"
            | "doc-view"
            | "doc-mismatch"
            | "doc-missing"
            | "doc-foreign"
            | "doc-event"
            | "doc-remove"
            | "doc-conflict-merge"
            | "doc-conflict-tab"
            | "doc-reconnect"
            | "doc-missed"
            | "doc-deleted"
            | "doc-stale-snapshot"
            | "doc-deleted-recover"
            | "doc-deleted-claim-lost"
            | "doc-conflict-flush"
            | "doc-focus"
            | "doc-event-focus"
            | "doc-orphan"
            | "doc-orphan-invalid"
            | "doc-program"
    )
}

fn snapshot_for(
    scenario: &str,
    generation: u32,
    pending_name: Option<&str>,
    started: &[(String, serde_json::Map<String, Value>)],
) -> Value {
    if scenario == "workspace-pending" && generation > 1 {
        // The interrupted start did reach core: the fresh snapshot has both
        // terminals, the second under the requested name.
        let t1 = started.first().map_or_else(
            || live_record(4242),
            |(name, labels)| started_record("t1", name, labels, 4242),
        );
        let t2 = started.get(1).map_or_else(
            || named_record("t2", pending_name.unwrap_or("ht-0-2"), 5151),
            |(name, labels)| started_record("t2", name, labels, 5151),
        );
        return json!({
            "seq": 1,
            "terminals": [t1, t2],
            "channel": EVENTS_CHANNEL,
        });
    }
    let terminals: Vec<Value> = match scenario {
        "doc-restore"
        | "doc-remove"
        | "doc-deleted-recover"
        | "doc-focus"
        | "doc-event-focus"
        | "doc-program" => {
            vec![
                doc_record_for(scenario, "t1"),
                doc_record_for(scenario, "t2"),
            ]
        }
        "doc-view" => vec![
            doc_record_for(scenario, "t1"),
            doc_record_for(scenario, "t2"),
        ],
        "doc-reconnect" if generation > 1 => {
            vec![
                doc_record_for(scenario, "t1"),
                doc_record_for(scenario, "t2"),
            ]
        }
        "doc-mismatch" | "doc-missing" | "doc-event" | "doc-conflict-merge"
        | "doc-conflict-tab" | "doc-reconnect" | "doc-missed" | "doc-deleted"
        | "doc-stale-snapshot" | "doc-conflict-flush" => {
            vec![doc_record_for(scenario, "t1")]
        }
        "doc-deleted-claim-lost" => Vec::new(),
        "doc-orphan" => vec![doc_record_for(scenario, "t1"), orphan_record("t7", 5, 6, 9)],
        "doc-orphan-invalid" => vec![doc_record_for(scenario, "t1"), unlabeled_record("t9")],
        "doc-foreign" => vec![foreign_record("t1")],
        "agents-lost" if generation > 1 => vec![agent_snapshot_record("t1")],
        "agents-reconnect-blocked" if generation > 1 => {
            vec![agent_blocked_snapshot_record("t1")]
        }
        "agents-snapshot-blocked" => vec![agent_blocked_snapshot_record("t1")],
        scenario if scripted_terminal(scenario) => vec![live_record(4242)],
        _ => Vec::new(),
    };
    json!({"seq": 1, "terminals": terminals, "channel": EVENTS_CHANNEL})
}

fn client_labels(space: u64, tab: u64, pane: u64) -> Value {
    json!({
        "herdr.client": "herdr-terminal",
        "herdr.terminal.space": space.to_string(),
        "herdr.terminal.tab": tab.to_string(),
        "herdr.terminal.pane": pane.to_string(),
    })
}

/// A live terminal carrying the labels a document pane expects.
fn doc_terminal(address: &str, space: u64, tab: u64, pane: u64) -> Value {
    let mut record = live_record(9000 + pane as u32);
    record["address"] = json!(address);
    record["labels"] = client_labels(space, tab, pane);
    record
}

fn doc_record_for(scenario: &str, address: &str) -> Value {
    match (scenario, address) {
        ("doc-view", "t1") => doc_terminal("t1", 1, 30, 3),
        ("doc-view", "t2") => doc_terminal("t2", 1, 31, 4),
        ("doc-conflict-merge", "t2") | ("doc-conflict-flush", "t2") => {
            doc_terminal("t2", 20, 21, 22)
        }
        ("doc-event-focus", "t3") => doc_terminal("t3", 1, 2, 5),
        (_, "t2") => doc_terminal("t2", 1, 2, 4),
        (_, "t1") => doc_terminal("t1", 1, 2, 3),
        _ => {
            let mut record = live_record(9999);
            record["address"] = json!(address);
            record
        }
    }
}

/// A reported terminal in a fresh snapshot, as the second connection of an
/// agent scenario sees it: turn 2, seen 0, still working.
fn agent_snapshot_record(address: &str) -> Value {
    let mut labels = client_labels(1, 2, 3)
        .as_object()
        .cloned()
        .unwrap_or_default();
    labels.insert("herdr.seen".into(), json!("0"));
    let mut record = live_record(4242);
    record["address"] = json!(address);
    record["labels"] = Value::Object(labels);
    record["reported"] = json!(true);
    record["turn"] = json!(2);
    record["title"] = json!("Build");
    record["status"] = agent_status("working", "claude", "Build", Some("edit"), None, "");
    record
}

/// A blocked reported terminal in a fresh snapshot, already seen at turn 2.
/// Used to prove reconnect/missed rebuild the alert baseline.
fn agent_blocked_snapshot_record(address: &str) -> Value {
    let mut labels = client_labels(1, 2, 3)
        .as_object()
        .cloned()
        .unwrap_or_default();
    labels.insert("herdr.seen".into(), json!("2"));
    let mut record = live_record(4343);
    record["address"] = json!(address);
    record["labels"] = Value::Object(labels);
    record["reported"] = json!(true);
    record["turn"] = json!(2);
    record["title"] = json!("Build");
    record["status"] = agent_status(
        "blocked",
        "claude",
        "Build",
        Some("permission"),
        None,
        "allow?",
    );
    record["foreground"] = json!({
        "pid": 78,
        "name": "claude",
        "argv": ["claude", "run"],
        "cwd": "/work",
    });
    record
}

/// A terminal without the pane label: the same address is not enough.
fn foreign_record(address: &str) -> Value {
    let mut record = live_record(9191);
    record["address"] = json!(address);
    record["labels"] = json!({"herdr.client": "herdr-terminal"});
    record
}

/// A labeled terminal that belongs to no document pane yet.
fn orphan_record(address: &str, space: u64, tab: u64, pane: u64) -> Value {
    let mut record = live_record(7373);
    record["address"] = json!(address);
    record["labels"] = client_labels(space, tab, pane);
    record
}

/// A live terminal carrying none of this client's labels.
fn unlabeled_record(address: &str) -> Value {
    let mut record = live_record(9292);
    record["address"] = json!(address);
    record["labels"] = json!({"herdr.client": "someone-else"});
    record
}

fn pane_leaf(pane: u64, address: &str, space: u64, tab: u64) -> Value {
    json!({"leaf": {
        "id": pane.to_string(),
        "address": address,
        "name": address,
        "labels": client_labels(space, tab, pane),
    }})
}

fn tab_doc(id: u64, name: &str, layout: Value) -> Value {
    json!({"id": id.to_string(), "name": name, "layout": layout})
}

fn space_doc(id: u64, name: &str, tabs: Vec<Value>) -> Value {
    json!({"id": id.to_string(), "name": name, "folder": "", "tabs": tabs})
}

fn split_layout(leaves: Vec<Value>) -> Value {
    if leaves.len() == 1 {
        return leaves.into_iter().next().expect("one leaf");
    }
    json!({
        "axis": "columns",
        "children": leaves
            .into_iter()
            .map(|layout| json!({"size": 1, "layout": layout}))
            .collect::<Vec<_>>(),
    })
}

/// One space, one tab, the given panes side by side.
fn one_tab_doc(panes: &[(u64, &str)]) -> Value {
    let layout = split_layout(
        panes
            .iter()
            .map(|(pane, address)| pane_leaf(*pane, address, 1, 2))
            .collect(),
    );
    doc_value(vec![space_doc(
        1,
        "shared",
        vec![tab_doc(2, "main", layout)],
    )])
}

fn doc_value(spaces: Vec<Value>) -> Value {
    json!({"version": 1, "spaces": spaces})
}

fn doc_one_pane() -> Value {
    one_tab_doc(&[(3, "t1")])
}

/// The document a screen claims when it has no panes.
fn empty_doc() -> Value {
    doc_value(vec![space_doc(
        1,
        "space 1",
        vec![tab_doc(2, "tab 1", Value::Null)],
    )])
}

fn doc_same_tab() -> Value {
    one_tab_doc(&[(3, "t1"), (4, "t2")])
}

/// Another screen added a pane in a space of its own.
fn doc_remote_space() -> Value {
    doc_value(vec![
        space_doc(
            1,
            "shared",
            vec![tab_doc(2, "main", pane_leaf(3, "t1", 1, 2))],
        ),
        space_doc(
            20,
            "remote",
            vec![tab_doc(21, "remote tab", pane_leaf(22, "t2", 20, 21))],
        ),
    ])
}

/// Two tabs, one pane each; the local view picks the second.
fn doc_two_tabs() -> Value {
    doc_value(vec![space_doc(
        1,
        "shared",
        vec![
            tab_doc(30, "one", pane_leaf(3, "t1", 1, 30)),
            tab_doc(31, "two", pane_leaf(4, "t2", 1, 31)),
        ],
    )])
}

// --- document store --------------------------------------------------------

fn read_doc(path: Option<&Path>) -> (Value, u64) {
    let Some(path) = path else {
        return (Value::Null, 0);
    };
    let Ok(bytes) = std::fs::read(path) else {
        return (Value::Null, 0);
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(store) => (
            store.get("value").cloned().unwrap_or(Value::Null),
            store.get("version").and_then(Value::as_u64).unwrap_or(0),
        ),
        Err(_) => (Value::Null, 0),
    }
}

fn write_doc(path: Option<&Path>, value: &Value, version: u64) {
    let Some(path) = path else { return };
    let store = json!({"version": version, "value": value});
    let _ = std::fs::write(path, store.to_string());
}

/// Replaces the store as another screen would, returning the new version.
fn replace_doc(path: Option<&Path>, value: Value) -> u64 {
    let (_, current) = read_doc(path);
    let version = current + 1;
    write_doc(path, &value, version);
    version
}

fn doc_event_message(version: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "event",
        "params": {
            "seq": 50 + version,
            "type": "doc",
            "data": {"name": "herdr-terminal", "version": version},
        },
    })
}

/// A fresh snapshot with both restored panes, as core sends after `missed`.
fn missed_two_panes_event() -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "missed",
        "params": {"seq": 9, "terminals": [doc_terminal("t1", 1, 2, 3), doc_terminal("t2", 1, 2, 4)]},
    })
}

/// A live record, or a finished one with exit status 7.
fn record(state: &str) -> serde_json::Value {
    let finished = state == "finished";
    json!({
        "address": "t1",
        "name": null,
        "labels": {"herdr.client": "herdr-terminal"},
        "argv": [],
        "cwd": "/",
        "keep": true,
        "state": state,
        "cols": 80,
        "rows": 24,
        "pid": if finished { serde_json::Value::Null } else { json!(4242) },
        "title": "",
        "parent": null,
        "reason": if finished { Some("exited") } else { None },
        "exit_code": if finished { Some(7) } else { None },
        "signal": null,
    })
}

fn live_record(pid: u32) -> serde_json::Value {
    let mut record = record("live");
    record["pid"] = json!(pid);
    record
}

/// A live record with its own address and the client's unique start name.
fn named_record(address: &str, name: &str, pid: u32) -> serde_json::Value {
    let mut record = live_record(pid);
    record["address"] = json!(address);
    record["name"] = json!(name);
    record
}

/// The name sent by the most recent `terminal.run` in the log. On the second
/// connection this is the start whose reply the first connection dropped.
fn last_start_name(path: Option<&Path>) -> Option<String> {
    let text = std::fs::read_to_string(path?).ok()?;
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|entry| entry["call"] == "terminal.run")
        .filter_map(|entry| entry["params"]["name"].as_str().map(str::to_owned))
        .next_back()
}

/// Every `terminal.run` in the log, in order, with the labels it carried.
fn started_terminals(path: Option<&Path>) -> Vec<(String, serde_json::Map<String, Value>)> {
    let Some(path) = path else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|entry| entry["call"] == "terminal.run")
        .filter_map(|entry| {
            let name = entry["params"]["name"].as_str()?.to_owned();
            let labels = entry["params"]["labels"].as_object()?.clone();
            Some((name, labels))
        })
        .collect()
}

/// A live record carrying the labels the start sent.
fn started_record(
    address: &str,
    name: &str,
    labels: &serde_json::Map<String, Value>,
    pid: u32,
) -> Value {
    let mut record = live_record(pid);
    record["address"] = json!(address);
    record["name"] = json!(name);
    record["labels"] = Value::Object(labels.clone());
    record
}

/// One `event` notification, on the events stream.
fn event_message(seq: u64, kind: &str, data: serde_json::Value) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "method": "event",
        "params": {"seq": seq, "type": kind, "terminal": "t1", "data": data},
    })
}

/// One agent event for a chosen terminal, as core emits it.
fn agent_event(seq: u64, kind: &str, terminal: &str, data: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "event",
        "params": {"seq": seq, "type": kind, "terminal": terminal, "data": data},
    })
}

fn agent_status(
    state: &str,
    app: &str,
    title: &str,
    kind: Option<&str>,
    progress: Option<u8>,
    msg: &str,
) -> Value {
    json!({
        "state": state,
        "kind": kind,
        "progress": progress,
        "app": app,
        "title": title,
        "msg": msg,
    })
}

/// The script an agent scenario runs for each key forwarded to its program.
fn agent_key_script(
    scenario: &str,
    step: u32,
    out: &mut impl Write,
    seq: &mut u64,
) -> io::Result<()> {
    if scenario == "agents-missed-blocked" {
        if step == 1 {
            notify(out, missed_agent_event())?;
        }
        return Ok(());
    }
    if scenario != "agents-turn" {
        return Ok(());
    }
    match step {
        1 => {
            *seq += 1;
            notify(
                out,
                agent_event(*seq, "title", "t1", json!({"title": "Build"})),
            )?;
            *seq += 1;
            notify(
                out,
                agent_event(
                    *seq,
                    "foreground",
                    "t1",
                    json!({"pid": 77, "name": "claude", "argv": ["claude"], "cwd": "/work"}),
                ),
            )?;
            *seq += 1;
            notify(
                out,
                agent_event(
                    *seq,
                    "status",
                    "t1",
                    json!({
                        "status": agent_status(
                            "working", "claude", "Build", Some("edit"), Some(42), "writing"
                        ),
                        "records": {},
                        "reported": true,
                    }),
                ),
            )?;
        }
        2 => {
            *seq += 1;
            notify(
                out,
                agent_event(
                    *seq,
                    "status",
                    "t1",
                    json!({
                        "status": agent_status("done", "claude", "Build", None, None, "ready"),
                        "records": {},
                        "reported": true,
                    }),
                ),
            )?;
            *seq += 1;
            notify(
                out,
                agent_event(*seq, "turn", "t1", json!({"turn": 1, "state": "done"})),
            )?;
        }
        _ => {
            // A same-turn event with no word change: a redraw whose seen
            // write decision must not repeat.
            *seq += 1;
            notify(
                out,
                agent_event(
                    *seq,
                    "status",
                    "t1",
                    json!({
                        "status": agent_status("idle", "claude3", "Build", None, None, ""),
                        "records": {},
                        "reported": true,
                    }),
                ),
            )?;
        }
    }
    Ok(())
}

fn read_labels(path: Option<&Path>) -> serde_json::Map<String, Value> {
    path.and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_labels(path: Option<&Path>, store: &serde_json::Map<String, Value>) {
    let Some(path) = path else { return };
    let _ = std::fs::write(path, Value::Object(store.clone()).to_string());
}

/// The labels a terminal already carries when its first label call arrives.
fn base_labels(
    _scenario: &str,
    target: &str,
    run_labels: &BTreeMap<String, serde_json::Map<String, Value>>,
) -> serde_json::Map<String, Value> {
    if let Some(labels) = run_labels.get(target) {
        return labels.clone();
    }
    let pane = if target == "t2" { 4 } else { 3 };
    let mut labels = client_labels(1, 2, pane)
        .as_object()
        .cloned()
        .unwrap_or_default();
    labels.insert("herdr.seen".into(), json!("0"));
    labels
}

fn lost_event() -> serde_json::Value {
    event_message(2, "lost", json!({"reason": "signaled", "signal": 9}))
}

fn removed_event() -> serde_json::Value {
    event_message(2, "removed", json!({}))
}

/// A fresh snapshot, as core sends when the subscriber fell behind.
fn missed_event() -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "method": "missed",
        "params": {"seq": 9, "terminals": [live_record(5555)]},
    })
}

/// A fresh blocked snapshot for the missed-baseline scenario.
fn missed_agent_event() -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "missed",
        "params": {"seq": 9, "terminals": [agent_blocked_snapshot_record("t1")]},
    })
}

/// A fresh snapshot with both workspace panes, as core sends after `missed`.
fn missed_workspace_event(
    name: Option<&str>,
    started: &[(String, serde_json::Map<String, Value>)],
) -> serde_json::Value {
    let t1 = started.first().map_or_else(
        || live_record(4242),
        |(name, labels)| started_record("t1", name, labels, 4242),
    );
    let t2 = started.get(1).map_or_else(
        || named_record("t2", name.unwrap_or("ht-0-2"), 5151),
        |(name, labels)| started_record("t2", name, labels, 5151),
    );
    json!({
        "jsonrpc": "2.0",
        "method": "missed",
        "params": {"seq": 9, "terminals": [t1, t2]},
    })
}

/// A notification from core: a newline-terminated JSON line on the events
/// channel, as core sends it.
fn notify(out: &mut impl Write, message: serde_json::Value) -> io::Result<()> {
    let mut line = message.to_string();
    line.push('\n');
    write_frame(out, EVENTS_CHANNEL, DATA, line.as_bytes())
}

fn bump_generation(log_path: Option<&Path>) -> u32 {
    let Some(log_path) = log_path else { return 1 };
    let path = log_path.with_extension("generation");
    let current: u32 = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0);
    let next = current + 1;
    let _ = std::fs::write(&path, next.to_string());
    next
}

fn reply(
    out: &mut impl Write,
    id: &serde_json::Value,
    result: serde_json::Value,
) -> io::Result<()> {
    write_json(out, json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn reply_error(
    out: &mut impl Write,
    id: &serde_json::Value,
    kind: &str,
    message: &str,
    hint: &str,
) -> io::Result<()> {
    write_json(
        out,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32000,
                "message": message,
                "data": {"kind": kind, "hint": hint},
            },
        }),
    )
}

fn write_json(out: &mut impl Write, value: serde_json::Value) -> io::Result<()> {
    write_frame(out, 0, DATA, value.to_string().as_bytes())
}

fn resize_payload(cols: u16, rows: u16) -> Vec<u8> {
    let mut payload = Vec::with_capacity(4);
    payload.extend_from_slice(&cols.to_le_bytes());
    payload.extend_from_slice(&rows.to_le_bytes());
    payload
}

fn write_frame(out: &mut impl Write, channel: u32, kind: u8, payload: &[u8]) -> io::Result<()> {
    let mut wire = Vec::with_capacity(9 + payload.len());
    wire.extend_from_slice(&channel.to_le_bytes());
    wire.push(kind);
    wire.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    wire.extend_from_slice(payload);
    out.write_all(&wire)?;
    out.flush()
}

fn read_frame(input: &mut impl Read) -> io::Result<Option<(u32, u8, Vec<u8>)>> {
    let mut header = [0u8; 9];
    match input.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let channel = u32::from_le_bytes(header[0..4].try_into().expect("four bytes"));
    let kind = header[4];
    let len = u32::from_le_bytes(header[5..9].try_into().expect("four bytes")) as usize;
    let mut payload = vec![0u8; len];
    input.read_exact(&mut payload)?;
    Ok(Some((channel, kind, payload)))
}

fn log(path: Option<&Path>, record: serde_json::Value) {
    let Some(path) = path else { return };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        // One write per record: a client that exits by killing this process
        // must not leave a half-written line behind.
        let mut line = record.to_string();
        line.push('\n');
        let _ = file.write_all(line.as_bytes());
    }
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

// --- scratch ---------------------------------------------------------------

struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = PathBuf::from(format!("/var/tmp/ht-fake-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch { dir }
    }

    fn log(&self) -> PathBuf {
        self.dir.join("log")
    }

    fn state(&self) -> PathBuf {
        self.dir.join("state")
    }

    fn config_root(&self) -> PathBuf {
        self.dir.join("config")
    }

    /// Writes this client's config where XDG_CONFIG_HOME points.
    #[cfg(unix)]
    fn seed_config(&self, text: &str) {
        let dir = self.config_root().join("herdr-terminal");
        std::fs::create_dir_all(&dir).expect("config dir");
        std::fs::write(dir.join("config.json"), text).expect("seed config");
    }

    fn docs(&self) -> PathBuf {
        self.dir.join("docs.json")
    }

    /// Writes the document store as if another screen had saved it.
    fn seed_doc(&self, value: serde_json::Value, version: u64) {
        write_doc(Some(&self.docs()), &value, version);
    }

    fn doc(&self) -> (serde_json::Value, u64) {
        read_doc(Some(&self.docs()))
    }

    fn doc_value(&self) -> serde_json::Value {
        self.doc().0
    }

    fn doc_version(&self) -> u64 {
        self.doc().1
    }

    /// Writes the per-screen local view where the client looks for it.
    #[cfg(unix)]
    fn seed_view(&self, screen: &str, view: serde_json::Value) {
        let dir = self.state().join("herdr-terminal");
        std::fs::create_dir_all(&dir).expect("local view dir");
        std::fs::write(dir.join(format!("view-{screen}.json")), view.to_string())
            .expect("seed local view");
    }

    fn entries(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.log())
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if std::env::var_os("KEEP_FAKE_SCRATCH").is_none() {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}
