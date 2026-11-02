//! The per-screen local view: active space/tab, per-tab focus, zoom and focus
//! history. Pure capture/apply and JSON encoding; no IO and no layout.
//!
//! Applying a view never changes a layout or an address. Ids that no longer
//! exist are ignored, and every tab without a usable focus falls back to its
//! first pane.

use super::{PaneId, SpaceId, TabId, Workspace};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub const VIEW_VERSION: u64 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalView {
    pub active_space: Option<SpaceId>,
    pub active_tabs: BTreeMap<SpaceId, TabId>,
    pub tabs: BTreeMap<TabId, TabView>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabView {
    pub focused: Option<PaneId>,
    pub zoomed: Option<PaneId>,
    pub history: Vec<PaneId>,
}

impl LocalView {
    pub fn capture(workspace: &Workspace) -> LocalView {
        let mut view = LocalView {
            active_space: Some(workspace.active),
            active_tabs: BTreeMap::new(),
            tabs: BTreeMap::new(),
        };
        for space in &workspace.spaces {
            view.active_tabs.insert(space.id, space.active);
            for tab in &space.tabs {
                view.tabs.insert(
                    tab.id,
                    TabView {
                        focused: tab.focused,
                        zoomed: tab.zoomed,
                        history: tab.history.clone(),
                    },
                );
            }
        }
        view
    }

    /// Restores the view where its ids still exist. Never creates ids.
    pub fn apply(&self, workspace: &mut Workspace) {
        if let Some(active) = self.active_space
            && workspace.spaces.iter().any(|space| space.id == active)
        {
            workspace.active = active;
        }
        for space in &mut workspace.spaces {
            if let Some(tab) = self.active_tabs.get(&space.id)
                && space.tabs.iter().any(|existing| existing.id == *tab)
            {
                space.active = *tab;
            }
            for tab in &mut space.tabs {
                let saved = self.tabs.get(&tab.id);
                if let Some(saved) = saved {
                    let mut history = Vec::new();
                    for pane in &saved.history {
                        if tab.contains(*pane) && !history.contains(pane) {
                            history.push(*pane);
                        }
                    }
                    tab.history = history;
                    tab.focused = saved.focused.filter(|pane| tab.contains(*pane));
                    tab.zoomed = saved.zoomed.filter(|pane| tab.contains(*pane));
                }
                if tab.focused.is_none() {
                    let fallback = tab
                        .history
                        .iter()
                        .rev()
                        .copied()
                        .find(|pane| tab.contains(*pane))
                        .or_else(|| tab.layout.as_ref().and_then(|layout| layout.first_leaf()));
                    if let Some(pane) = fallback {
                        tab.history.retain(|existing| *existing != pane);
                        tab.history.push(pane);
                        tab.focused = Some(pane);
                    } else {
                        tab.focused = None;
                        tab.zoomed = None;
                        tab.history.clear();
                    }
                }
            }
        }
        if !workspace
            .spaces
            .iter()
            .any(|space| space.id == workspace.active)
            && let Some(first) = workspace.spaces.first()
        {
            workspace.active = first.id;
        }
        for space in &mut workspace.spaces {
            if !space.tabs.iter().any(|tab| tab.id == space.active)
                && let Some(first) = space.tabs.first()
            {
                space.active = first.id;
            }
        }
    }

    pub fn to_value(&self) -> Value {
        let active_tabs: Map<String, Value> = self
            .active_tabs
            .iter()
            .map(|(space, tab)| (space.0.to_string(), json!(tab.0.to_string())))
            .collect();
        let tabs: Map<String, Value> = self
            .tabs
            .iter()
            .map(|(tab, view)| {
                (
                    tab.0.to_string(),
                    json!({
                        "focused": view.focused.map(|pane| pane.0.to_string()),
                        "zoomed": view.zoomed.map(|pane| pane.0.to_string()),
                        "history": view.history.iter().map(|pane| pane.0.to_string()).collect::<Vec<_>>(),
                    }),
                )
            })
            .collect();
        json!({
            "version": VIEW_VERSION,
            "active_space": self.active_space.map(|space| space.0.to_string()),
            "active_tabs": Value::Object(active_tabs),
            "tabs": Value::Object(tabs),
        })
    }

    pub fn parse(value: &Value) -> Result<LocalView, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "the local view is not a JSON object".to_owned())?;
        let version = object
            .get("version")
            .and_then(Value::as_u64)
            .ok_or_else(|| "the local view has no version".to_owned())?;
        if version != VIEW_VERSION {
            return Err(format!(
                "the local view has version {version}, not {VIEW_VERSION}"
            ));
        }
        let active_space = optional_id(object.get("active_space"), "active_space")?.map(SpaceId);
        let mut active_tabs = BTreeMap::new();
        if let Some(value) = object.get("active_tabs") {
            let map = value
                .as_object()
                .ok_or_else(|| "active_tabs is not an object".to_owned())?;
            for (space, tab) in map {
                let space = id(space, "active_tabs id")?;
                let tab = id(
                    tab.as_str()
                        .ok_or_else(|| "active tab id is not a string".to_owned())?,
                    "active tab id",
                )?;
                active_tabs.insert(SpaceId(space), TabId(tab));
            }
        }
        let mut tabs = BTreeMap::new();
        if let Some(value) = object.get("tabs") {
            let map = value
                .as_object()
                .ok_or_else(|| "tabs is not an object".to_owned())?;
            for (tab, view) in map {
                let tab = id(tab, "tab id")?;
                let view = view
                    .as_object()
                    .ok_or_else(|| "a tab view is not an object".to_owned())?;
                let focused = optional_id(view.get("focused"), "focused")?.map(PaneId);
                let zoomed = optional_id(view.get("zoomed"), "zoomed")?.map(PaneId);
                let mut history = Vec::new();
                if let Some(value) = view.get("history") {
                    let list = value
                        .as_array()
                        .ok_or_else(|| "history is not an array".to_owned())?;
                    for pane in list {
                        let pane = pane
                            .as_str()
                            .ok_or_else(|| "history id is not a string".to_owned())?;
                        history.push(PaneId(id(pane, "history id")?));
                    }
                }
                tabs.insert(
                    TabId(tab),
                    TabView {
                        focused,
                        zoomed,
                        history,
                    },
                );
            }
        }
        Ok(LocalView {
            active_space,
            active_tabs,
            tabs,
        })
    }
}

fn optional_id(value: Option<&Value>, field: &str) -> Result<Option<u64>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            let value = value
                .as_str()
                .ok_or_else(|| format!("{field} is not a string"))?;
            Ok(Some(id(value, field)?))
        }
    }
}

fn id(value: &str, field: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("{field} {value:?} is not a u64"))
}

#[cfg(test)]
mod tests {
    use super::super::{StartKind, TerminalInfo, restore};
    use super::*;

    fn populated() -> Workspace {
        let mut workspace = Workspace::new();
        let first = workspace.start_request(StartKind::Initial);
        workspace.insert_terminal(&first, "t1".into(), "t1".into());
        let tab = workspace.start_request(StartKind::Tab);
        workspace.insert_terminal(&tab, "t2".into(), "t2".into());
        let space = workspace.start_request(StartKind::Space);
        workspace.insert_terminal(&space, "t3".into(), "t3".into());
        workspace
    }

    fn restore_workspace(workspace: &Workspace) -> Workspace {
        let infos: Vec<TerminalInfo> = workspace
            .panes()
            .map(|pane| TerminalInfo::new(pane.address.clone(), pane.labels.clone()))
            .collect();
        restore(
            &workspace.capture_doc(),
            workspace.allocator().clone(),
            workspace.name_counters(),
            &infos,
        )
        .workspace
    }

    #[test]
    fn the_view_restores_active_space_tab_focus_zoom_and_history() {
        let mut source = populated();
        let space = source.spaces()[0].id;
        let tab = source.spaces()[0].tabs[1].id;
        let zoomed = source.panes().find(|pane| pane.address == "t2").unwrap().id;
        assert!(source.prev_space());
        assert_eq!(source.active_tab_id(), Some(tab));
        assert_eq!(source.focused_pane(), Some(zoomed));
        assert!(source.toggle_zoom());
        let view = LocalView::capture(&source);

        let mut target = restore_workspace(&source);
        view.apply(&mut target);
        assert_eq!(target.active_space_id(), space);
        assert_eq!(target.active_tab_id(), Some(tab));
        assert_eq!(target.focused_pane(), Some(zoomed));
        assert_eq!(target.active_tab().unwrap().zoomed, Some(zoomed));
        assert_eq!(target.active_tab().unwrap().history.last(), Some(&zoomed));
    }

    #[test]
    fn a_view_for_missing_ids_never_creates_them() {
        let mut workspace = Workspace::new();
        let present = workspace.start_request(StartKind::Initial);
        let pane = workspace.insert_terminal(&present, "t1".into(), "t1".into());
        let view = LocalView {
            active_space: Some(SpaceId(99)),
            active_tabs: BTreeMap::from([(SpaceId(99), TabId(98))]),
            tabs: BTreeMap::from([(
                TabId(98),
                TabView {
                    focused: Some(PaneId(97)),
                    zoomed: Some(PaneId(97)),
                    history: vec![PaneId(97), pane],
                },
            )]),
        };
        view.apply(&mut workspace);
        assert_eq!(workspace.spaces().len(), 1);
        assert_eq!(workspace.focused_pane(), Some(pane), "focus falls back");
        assert_eq!(workspace.active_tab().unwrap().zoomed, None);
        assert_eq!(workspace.active_tab().unwrap().history, vec![pane]);
        assert!(workspace.pane(PaneId(97)).is_none());
    }

    #[test]
    fn the_view_round_trips_through_json() {
        let source = populated();
        let view = LocalView::capture(&source);
        let value = view.to_value();
        assert_eq!(value["version"], json!(1));
        assert_eq!(LocalView::parse(&value), Ok(view));
        assert!(LocalView::parse(&json!({"version": 2})).is_err());
        assert!(LocalView::parse(&json!({"version": 1, "tabs": {"x": {}}})).is_err());
        assert!(LocalView::parse(&json!({"version": 1, "tabs": {"7": {"history": [7]}}})).is_err());
    }
}
