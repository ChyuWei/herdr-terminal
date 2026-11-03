//! The machine manager's IO: core's machine list, the add/connect/rename/
//! remove actions, and starting a new terminal on the selected machine.
//!
//! Core owns every ssh line, its state and its reconnect attempts. This file
//! never dials, retries or parses a target: it passes the typed target to
//! `machine.add` unchanged, shows the state core reports, and refreshes the
//! list on open, after each action, and on a bounded timer while open.

use super::Screen;
use crate::core::{MachineAdd, Result};
use crate::session;
use crate::workspace::agents;
use crate::workspace::machines::{self, MachineRow};
use crate::workspace::model::StartKind;
use crate::workspace::state::{MachinePrompt, MachinePromptKind};
use crate::workspace::ui;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How often the open machine manager asks core for fresh line states.
const MACHINE_REFRESH: Duration = Duration::from_secs(2);

impl Screen<'_> {
    // --- opening and rows -------------------------------------------------

    /// Prefix `m`: open the machine manager, or close it when it is already
    /// the modal overlay. Opening closes every other overlay; the list is
    /// fetched once immediately and then only while the overlay stays open.
    pub(super) fn toggle_machines(&mut self) {
        if self.ui.machines.open {
            self.close_machines();
            return;
        }
        self.ui.help = false;
        self.ui.prompt = None;
        self.ui.palette = None;
        self.ui.search = None;
        self.ui.selecting = None;
        self.ui.peek = None;
        self.ui.sidebar.open = false;
        self.ui.manager.open = false;
        self.ui.machines.open = true;
        self.ui.machines.filtering = false;
        self.ui.machines.prompt = None;
        self.ui.machines.confirm = None;
        self.ui.machines.error = None;
        self.machine_rows.clear();
        self.refresh_machines();
        self.machine_next_refresh = Some(Instant::now() + MACHINE_REFRESH);
        self.dirty = true;
    }

    pub(super) fn close_machines(&mut self) {
        if self.ui.machines.open {
            self.ui.machines.open = false;
            self.ui.machines.filtering = false;
            self.ui.machines.prompt = None;
            self.ui.machines.confirm = None;
            self.machine_rows.clear();
            self.machine_next_refresh = None;
            self.dirty = true;
        }
    }

    /// While the manager is open, refresh on its timer. Never calls core when
    /// the manager is closed.
    pub(super) fn refresh_machines_if_due(&mut self) {
        if !self.ui.machines.open {
            return;
        }
        if self
            .machine_next_refresh
            .is_some_and(|next| Instant::now() < next)
        {
            return;
        }
        self.refresh_machines();
        self.machine_next_refresh = Some(Instant::now() + MACHINE_REFRESH);
    }

    /// A `missed` snapshot may have changed machine state; refresh at the next
    /// loop step rather than waiting for the timer.
    pub(super) fn refresh_machines_soon(&mut self) {
        if self.ui.machines.open {
            self.machine_next_refresh = Some(Instant::now());
        }
    }

    /// Fetches `machine.list` and rebuilds the rows. A refusal is shown inside
    /// the overlay and leaves the previous rows in place; a lost connection
    /// starts the normal reconnect and the timer retries after it.
    fn refresh_machines(&mut self) {
        if !self.client.hello().has_method("machine.list") {
            self.ui.machines.error =
                Some("this core has no machine API (machine.list); upgrade core".into());
            self.dirty = true;
            return;
        }
        let machines = match self.client.machine_list() {
            Ok(machines) => machines,
            Err(error) if session::is_lost(&error) => {
                self.begin_reconnect();
                return;
            }
            Err(error) => {
                self.ui.machines.error = Some(format!("could not list machines: {error}"));
                self.dirty = true;
                return;
            }
        };
        self.machine_rows = machines::rows(&machines, now_ms());
        self.repair_machine_selection();
        self.dirty = true;
    }

    fn repair_machine_selection(&mut self) {
        let query = self.ui.machines.filter.text();
        let filtered = machines::filter(&self.machine_rows, &query);
        let selected = machines::ensure_selection(&filtered, self.ui.machines.selected.as_deref());
        let index = selected
            .as_deref()
            .and_then(|name| filtered.iter().position(|row| row.name == name));
        let height = ui::machine_list_height(self.area());
        self.ui.machines.offset =
            agents::scroll_to(self.ui.machines.offset, index, height, filtered.len());
        self.ui.machines.selected = selected;
    }

    fn selected_machine_row(&self) -> Option<MachineRow> {
        let query = self.ui.machines.filter.text();
        let filtered = machines::filter(&self.machine_rows, &query);
        machines::selected_row(&filtered, self.ui.machines.selected.as_deref()).cloned()
    }

    // --- keys --------------------------------------------------------------

    /// One key while the machine manager owns the keyboard. Nothing here is
    /// forwarded to a program; every key is consumed.
    pub(super) fn machine_key(&mut self, key: &KeyEvent) -> Result<()> {
        if key.kind == KeyEventKind::Release {
            return Ok(());
        }
        if self.ui.machines.prompt.is_some() {
            return self.machine_prompt_key(key);
        }
        if self.ui.machines.confirm.is_some() {
            return self.machine_confirm_key(key);
        }
        if self.ui.machines.filtering {
            match key.code {
                KeyCode::Esc => {
                    self.close_machines();
                    return Ok(());
                }
                KeyCode::Enter => {
                    self.ui.machines.filtering = false;
                    self.dirty = true;
                    return Ok(());
                }
                _ => {
                    if let Some(edit) = super::overlays::text_edit(key)
                        && self.ui.machines.filter.edit(edit)
                    {
                        self.repair_machine_selection();
                        self.dirty = true;
                    }
                    return Ok(());
                }
            }
        }
        let plain = !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => self.close_machines(),
            KeyCode::Down | KeyCode::Char('j') if plain => self.machine_move(1),
            KeyCode::Up | KeyCode::Char('k') if plain => self.machine_move(-1),
            KeyCode::Home | KeyCode::Char('g') if plain => self.machine_edge(false),
            KeyCode::End | KeyCode::Char('G') if plain => self.machine_edge(true),
            KeyCode::Enter => self.machine_connect()?,
            KeyCode::Char('a') if plain => self.begin_machine_add(),
            KeyCode::Char('r') if plain => self.begin_machine_rename(),
            KeyCode::Char('d') if plain => self.begin_machine_remove(),
            KeyCode::Char('n') if plain => self.machine_start()?,
            KeyCode::Char('/') if plain => {
                self.ui.machines.filtering = true;
                self.dirty = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn machine_move(&mut self, delta: isize) {
        let query = self.ui.machines.filter.text();
        let filtered = machines::filter(&self.machine_rows, &query);
        let next = machines::move_selection(&filtered, self.ui.machines.selected.as_deref(), delta);
        let index = next
            .as_deref()
            .and_then(|name| filtered.iter().position(|row| row.name == name));
        let height = ui::machine_list_height(self.area());
        self.ui.machines.offset =
            agents::scroll_to(self.ui.machines.offset, index, height, filtered.len());
        self.ui.machines.selected = next;
        self.dirty = true;
    }

    fn machine_edge(&mut self, last: bool) {
        let query = self.ui.machines.filter.text();
        let filtered = machines::filter(&self.machine_rows, &query);
        let next = machines::edge_selection(&filtered, last);
        let index = next
            .as_deref()
            .and_then(|name| filtered.iter().position(|row| row.name == name));
        let height = ui::machine_list_height(self.area());
        self.ui.machines.offset =
            agents::scroll_to(self.ui.machines.offset, index, height, filtered.len());
        self.ui.machines.selected = next;
        self.dirty = true;
    }

    fn machine_prompt_key(&mut self, key: &KeyEvent) -> Result<()> {
        let edit = super::overlays::text_edit(key);
        let Some(prompt) = self.ui.machines.prompt.as_mut() else {
            return Ok(());
        };
        match key.code {
            KeyCode::Esc => {
                self.ui.machines.prompt = None;
                self.dirty = true;
            }
            KeyCode::Enter => {
                let target = prompt.line.text().trim().to_owned();
                let kind = prompt.kind.clone();
                if target.is_empty() {
                    self.ui.machines.error = Some("a machine target is required".into());
                    self.dirty = true;
                    return Ok(());
                }
                self.ui.machines.prompt = None;
                match kind {
                    MachinePromptKind::Add => self.add_machine(&target),
                    MachinePromptKind::Rename { machine } => self.rename_machine(&machine, &target),
                }
            }
            _ => {
                if let Some(edit) = edit
                    && prompt.line.edit(edit)
                {
                    self.dirty = true;
                }
            }
        }
        Ok(())
    }

    fn machine_confirm_key(&mut self, key: &KeyEvent) -> Result<()> {
        let machine = self.ui.machines.confirm.clone().unwrap_or_default();
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                self.ui.machines.confirm = None;
                self.remove_machine(&machine);
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                self.ui.machines.confirm = None;
                self.dirty = true;
            }
            _ => {}
        }
        Ok(())
    }

    // --- actions -----------------------------------------------------------

    /// Enter: ask core to dial the selected machine now. Core owns the retry
    /// loop; this only reports the state it returns.
    fn machine_connect(&mut self) -> Result<()> {
        let Some(row) = self.selected_machine_row() else {
            self.ui.machines.error = Some("no machine selected".into());
            self.dirty = true;
            return Ok(());
        };
        match self.client.machine_connect(&row.name) {
            Ok(machine) => {
                self.ui.machines.error = None;
                self.ui.message = Some(machine_message(&machine));
            }
            Err(error) if session::is_lost(&error) => {
                self.begin_reconnect();
                return Ok(());
            }
            Err(error) => {
                self.ui.machines.error = Some(format!("could not connect: {error}"));
            }
        }
        self.refresh_machines();
        self.machine_next_refresh = Some(Instant::now() + MACHINE_REFRESH);
        self.dirty = true;
        Ok(())
    }

    fn begin_machine_add(&mut self) {
        self.ui.machines.prompt = Some(MachinePrompt::add());
        self.ui.machines.confirm = None;
        self.ui.machines.filtering = false;
        self.ui.machines.error = None;
        self.dirty = true;
    }

    fn begin_machine_rename(&mut self) {
        let Some(row) = self.selected_machine_row() else {
            self.ui.machines.error = Some("no machine selected".into());
            self.dirty = true;
            return;
        };
        self.ui.machines.prompt = Some(MachinePrompt::rename(&row.name));
        self.ui.machines.confirm = None;
        self.ui.machines.filtering = false;
        self.ui.machines.error = None;
        self.dirty = true;
    }

    fn begin_machine_remove(&mut self) {
        let Some(row) = self.selected_machine_row() else {
            self.ui.machines.error = Some("no machine selected".into());
            self.dirty = true;
            return;
        };
        self.ui.machines.confirm = Some(row.name);
        self.ui.machines.prompt = None;
        self.ui.machines.error = None;
        self.dirty = true;
    }

    /// Adds the typed target exactly as entered. Core parses, dials and names
    /// it; this client never rewrites `user@host:port` or a `--via` command.
    fn add_machine(&mut self, target: &str) {
        let params = MachineAdd::target(target);
        match self.client.machine_add(&params) {
            Ok(machine) => {
                let name = machine_name(&machine, target);
                self.ui.machines.error = None;
                self.ui.machines.selected = Some(name.clone());
                self.ui.message = Some(format!("added machine {name}"));
            }
            Err(error) if session::is_lost(&error) => {
                self.begin_reconnect();
                return;
            }
            Err(error) => {
                self.ui.machines.error = Some(format!("could not add {target}: {error}"));
                self.dirty = true;
                return;
            }
        }
        self.refresh_machines();
        self.machine_next_refresh = Some(Instant::now() + MACHINE_REFRESH);
        self.dirty = true;
    }

    /// Renames a machine in core and moves this workspace's qualified
    /// addresses before core's `removed`/`created` pair arrives, so the panes
    /// (and their attach streams) survive the rename.
    fn rename_machine(&mut self, machine: &str, new_name: &str) {
        if machine == new_name {
            self.dirty = true;
            return;
        }
        let renamed = match self.client.machine_rename(machine, new_name) {
            Ok(renamed) => renamed,
            Err(error) if session::is_lost(&error) => {
                self.begin_reconnect();
                return;
            }
            Err(error) => {
                self.ui.machines.prompt = Some(MachinePrompt {
                    kind: MachinePromptKind::Rename {
                        machine: machine.to_owned(),
                    },
                    line: crate::workspace::state::Line::new(new_name),
                });
                self.ui.machines.error =
                    Some(format!("could not rename {machine} to {new_name}: {error}"));
                self.dirty = true;
                return;
            }
        };
        let name = machine_name(&renamed, new_name);
        let changed = machines::rename_machine_addresses(&mut self.workspace, machine, &name);
        for pane in changed {
            let Some(address) = self.workspace.pane(pane).map(|pane| pane.address.clone()) else {
                continue;
            };
            if let Some(link) = self.links.get_mut(&pane) {
                link.session.set_address(address);
            }
        }
        self.ui.machines.selected = Some(name.clone());
        self.ui.machines.error = None;
        self.ui.machines.prompt = None;
        self.ui.message = Some(format!("renamed machine {machine} to {name}"));
        self.refresh_machines();
        self.machine_next_refresh = Some(Instant::now() + MACHINE_REFRESH);
        self.dirty = true;
    }

    /// Removes a machine after the explicit confirmation. Core aborts the line
    /// and emits `removed` for its terminals; the panes leave through those
    /// events, never through a local guess.
    fn remove_machine(&mut self, machine: &str) {
        match self.client.machine_rm(machine) {
            Ok(()) => {
                self.ui.machines.error = None;
                self.ui.message = Some(format!("removed machine {machine}"));
            }
            Err(error) if session::is_lost(&error) => {
                self.begin_reconnect();
                return;
            }
            Err(error) => {
                self.ui.machines.error = Some(format!("could not remove {machine}: {error}"));
            }
        }
        self.refresh_machines();
        self.machine_next_refresh = Some(Instant::now() + MACHINE_REFRESH);
        self.dirty = true;
    }

    /// `n`: start a new terminal on the selected machine. The run goes through
    /// this client's one core connection with `on`, and its labels stay this
    /// client's own. The manager closes so the new pane is visible.
    fn machine_start(&mut self) -> Result<()> {
        let Some(row) = self.selected_machine_row() else {
            self.ui.machines.error = Some("no machine selected".into());
            self.dirty = true;
            return Ok(());
        };
        let request = self.workspace.start_request(StartKind::Tab);
        self.close_machines();
        self.start_terminal_on(request, row.name)
    }
}

fn machine_name(machine: &crate::core::Machine, fallback: &str) -> String {
    if machine.name.is_empty() {
        fallback.to_owned()
    } else {
        machine.name.clone()
    }
}

fn machine_message(machine: &crate::core::Machine) -> String {
    let name = if machine.name.is_empty() {
        "machine"
    } else {
        machine.name.as_str()
    };
    let state = machines::state_word(&machine.state);
    match machine.why.as_deref().filter(|why| !why.is_empty()) {
        Some(why) => format!("{name}: {state} · {why}"),
        None => format!("{name}: {state}"),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}
