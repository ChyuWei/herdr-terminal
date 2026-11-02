//! The shared workspace document: its version-1 JSON shape, validation and
//! capture. Pure: no core, file or clock is touched.
//!
//! The document holds only what two screens share: spaces, tabs, split
//! geometry and pane terminal identities. Focus, zoom, history and the active
//! space/tab are local view, not document state.

use super::super::{Axis, Layout, PaneId, Workspace};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const DOC_NAME: &str = "herdr-terminal";
pub const SCHEMA_VERSION: u64 = 1;

pub const CLIENT_LABEL: &str = "herdr.client";
pub const CLIENT_NAME: &str = "herdr-terminal";
pub const SPACE_LABEL: &str = "herdr.terminal.space";
pub const TAB_LABEL: &str = "herdr.terminal.tab";
pub const PANE_LABEL: &str = "herdr.terminal.pane";
/// Shared attention labels: live terminal state, never document state.
pub const SEEN_LABEL: &str = "herdr.seen";
pub const PINNED_LABEL: &str = "herdr.pinned";

/// The labels the shared document owns: terminal identity only. Attention
/// labels are shared through core's terminal records and must never dirty the
/// layout document.
pub fn identity_labels(labels: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    labels
        .iter()
        .filter(|(key, _)| !is_attention_label(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

pub fn is_attention_label(key: &str) -> bool {
    key == SEEN_LABEL || key == PINNED_LABEL
}

/// A parsed version-1 workspace document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SharedDoc {
    pub spaces: Vec<DocSpace>,
}

impl SharedDoc {
    /// A document with no spaces, used as the merge base when the document was
    /// deleted while this screen was away.
    pub fn empty() -> SharedDoc {
        SharedDoc { spaces: Vec::new() }
    }

    pub fn to_value(&self) -> Value {
        json!({
            "version": SCHEMA_VERSION,
            "spaces": self.spaces.iter().map(DocSpace::to_value).collect::<Vec<_>>(),
        })
    }

    pub fn parse(value: &Value) -> Result<SharedDoc, DocError> {
        let object = value
            .as_object()
            .ok_or_else(|| DocError::Malformed("the value is not a JSON object".into()))?;
        let version = object
            .get("version")
            .ok_or_else(|| DocError::Malformed("missing version".into()))?
            .as_u64()
            .ok_or_else(|| DocError::Malformed("version is not an unsigned integer".into()))?;
        if version != SCHEMA_VERSION {
            return Err(DocError::UnsupportedVersion(version));
        }
        let spaces = object
            .get("spaces")
            .and_then(Value::as_array)
            .ok_or_else(|| DocError::Malformed("spaces is not an array".into()))?;
        if spaces.is_empty() {
            return Err(DocError::Malformed("the document has no spaces".into()));
        }
        let mut ids = BTreeSet::new();
        let mut parsed = Vec::with_capacity(spaces.len());
        for space in spaces {
            parsed.push(parse_space(space, &mut ids)?);
        }
        Ok(SharedDoc { spaces: parsed })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocSpace {
    pub id: u64,
    pub name: String,
    pub folder: String,
    pub tabs: Vec<DocTab>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocTab {
    pub id: u64,
    pub name: String,
    /// `None` is an empty tab.
    pub layout: Option<DocLayout>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocLayout {
    Leaf(DocPane),
    Split { axis: Axis, children: Vec<DocChild> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocChild {
    pub size: u32,
    pub layout: DocLayout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocPane {
    pub id: u64,
    pub address: String,
    pub name: String,
    pub labels: BTreeMap<String, String>,
}

impl DocLayout {
    pub fn leaves(&self) -> Vec<&DocPane> {
        let mut leaves = Vec::new();
        self.collect(&mut leaves);
        leaves
    }

    fn collect<'a>(&'a self, leaves: &mut Vec<&'a DocPane>) {
        match self {
            DocLayout::Leaf(pane) => leaves.push(pane),
            DocLayout::Split { children, .. } => {
                for child in children {
                    child.layout.collect(leaves);
                }
            }
        }
    }
}

impl DocSpace {
    pub fn tab(&self, id: u64) -> Option<&DocTab> {
        self.tabs.iter().find(|tab| tab.id == id)
    }
}

impl DocTab {
    pub fn leaves(&self) -> Vec<&DocPane> {
        self.layout
            .as_ref()
            .map(DocLayout::leaves)
            .unwrap_or_default()
    }
}

impl DocPane {
    pub fn to_info(&self) -> super::restore::TerminalInfo {
        super::restore::TerminalInfo {
            address: self.address.clone(),
            labels: self.labels.clone(),
        }
    }
}

impl DocSpace {
    fn to_value(&self) -> Value {
        json!({
            "id": self.id.to_string(),
            "name": self.name,
            "folder": self.folder,
            "tabs": self.tabs.iter().map(DocTab::to_value).collect::<Vec<_>>(),
        })
    }
}

impl DocTab {
    fn to_value(&self) -> Value {
        json!({
            "id": self.id.to_string(),
            "name": self.name,
            "layout": self.layout.as_ref().map(DocLayout::to_value).unwrap_or(Value::Null),
        })
    }
}

impl DocLayout {
    fn to_value(&self) -> Value {
        match self {
            DocLayout::Leaf(pane) => json!({"leaf": pane.to_value()}),
            DocLayout::Split { axis, children } => json!({
                "axis": match axis { Axis::Columns => "columns", Axis::Rows => "rows" },
                "children": children.iter().map(|child| json!({
                    "size": child.size,
                    "layout": child.layout.to_value(),
                })).collect::<Vec<_>>(),
            }),
        }
    }
}

impl DocPane {
    fn to_value(&self) -> Value {
        let labels: Map<String, Value> = self
            .labels
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect();
        json!({
            "id": self.id.to_string(),
            "address": self.address,
            "name": self.name,
            "labels": Value::Object(labels),
        })
    }
}

/// Captures the shared part of a workspace. Focus, zoom, history and the
/// active space/tab are deliberately absent.
pub fn capture(workspace: &Workspace) -> SharedDoc {
    let mut spaces = Vec::new();
    for space in &workspace.spaces {
        let mut tabs = Vec::new();
        for tab in &space.tabs {
            tabs.push(DocTab {
                id: tab.id.0,
                name: tab.name.clone(),
                layout: tab
                    .layout
                    .as_ref()
                    .map(|layout| capture_layout(workspace, layout)),
            });
        }
        spaces.push(DocSpace {
            id: space.id.0,
            name: space.name.clone(),
            folder: space.folder.clone(),
            tabs,
        });
    }
    SharedDoc { spaces }
}

fn capture_layout(workspace: &Workspace, layout: &Layout) -> DocLayout {
    match layout {
        Layout::Leaf(pane) => {
            let computed = workspace.panes.get(pane);
            DocLayout::Leaf(DocPane {
                id: pane.0,
                address: computed
                    .map(|pane| pane.address.clone())
                    .unwrap_or_default(),
                name: computed.map(|pane| pane.name.clone()).unwrap_or_default(),
                labels: computed
                    .map(|pane| identity_labels(&pane.labels))
                    .unwrap_or_default(),
            })
        }
        Layout::Split { axis, children } => DocLayout::Split {
            axis: *axis,
            children: children
                .iter()
                .map(|child| DocChild {
                    size: child.size,
                    layout: capture_layout(workspace, &child.layout),
                })
                .collect(),
        },
    }
}

fn parse_space(value: &Value, ids: &mut BTreeSet<u64>) -> Result<DocSpace, DocError> {
    let object = object(value, "space")?;
    let id = parse_id(object.get("id"), "space id", ids)?;
    let name = string(object.get("name"), "space name")?;
    let folder = match object.get("folder") {
        None | Some(Value::Null) => String::new(),
        Some(value) => string(Some(value), "space folder")?,
    };
    let tabs = object
        .get("tabs")
        .and_then(Value::as_array)
        .ok_or_else(|| DocError::Malformed(format!("space {id} tabs is not an array")))?;
    if tabs.is_empty() {
        return Err(DocError::Malformed(format!("space {id} has no tabs")));
    }
    let mut parsed = Vec::with_capacity(tabs.len());
    for tab in tabs {
        parsed.push(parse_tab(tab, ids)?);
    }
    Ok(DocSpace {
        id,
        name,
        folder,
        tabs: parsed,
    })
}

fn parse_tab(value: &Value, ids: &mut BTreeSet<u64>) -> Result<DocTab, DocError> {
    let object = object(value, "tab")?;
    let id = parse_id(object.get("id"), "tab id", ids)?;
    let name = string(object.get("name"), "tab name")?;
    let layout = match object.get("layout") {
        None | Some(Value::Null) => None,
        Some(value) => Some(parse_layout(value, ids)?),
    };
    Ok(DocTab { id, name, layout })
}

fn parse_layout(value: &Value, ids: &mut BTreeSet<u64>) -> Result<DocLayout, DocError> {
    let map = object(value, "layout")?;
    if let Some(leaf) = map.get("leaf") {
        if map.contains_key("axis") || map.contains_key("children") {
            return Err(DocError::Malformed(
                "a layout cannot be both a leaf and a split".into(),
            ));
        }
        return Ok(DocLayout::Leaf(parse_pane(leaf, ids)?));
    }
    let axis = match map.get("axis").and_then(Value::as_str) {
        Some("columns") => Axis::Columns,
        Some("rows") => Axis::Rows,
        Some(other) => {
            return Err(DocError::Malformed(format!(
                "unknown layout axis {other:?}"
            )));
        }
        None => return Err(DocError::Malformed("a split layout has no axis".into())),
    };
    let children = map
        .get("children")
        .and_then(Value::as_array)
        .ok_or_else(|| DocError::Malformed("a split layout has no children array".into()))?;
    if children.len() < 2 {
        return Err(DocError::Malformed(
            "a split layout needs at least two children".into(),
        ));
    }
    let mut parsed = Vec::with_capacity(children.len());
    for child in children {
        let child = object(child, "split child")?;
        let size = child
            .get("size")
            .and_then(Value::as_u64)
            .and_then(|size| u32::try_from(size).ok())
            .filter(|size| *size > 0)
            .ok_or_else(|| DocError::Malformed("a split child has no positive size".into()))?;
        let layout = child
            .get("layout")
            .ok_or_else(|| DocError::Malformed("a split child has no layout".into()))?;
        parsed.push(DocChild {
            size,
            layout: parse_layout(layout, ids)?,
        });
    }
    Ok(DocLayout::Split {
        axis,
        children: parsed,
    })
}

fn parse_pane(value: &Value, ids: &mut BTreeSet<u64>) -> Result<DocPane, DocError> {
    let object = object(value, "pane")?;
    let id = parse_id(object.get("id"), "pane id", ids)?;
    let address = string(object.get("address"), "pane address")?;
    if address.is_empty() {
        return Err(DocError::Malformed(format!(
            "pane {id} has an empty address"
        )));
    }
    let name = string(object.get("name"), "pane name")?;
    let labels = match object.get("labels") {
        None | Some(Value::Null) => BTreeMap::new(),
        Some(value) => {
            let map = value
                .as_object()
                .ok_or_else(|| DocError::Malformed(format!("pane {id} labels is not an object")))?;
            let mut labels = BTreeMap::new();
            for (key, value) in map {
                let value = value.as_str().ok_or_else(|| {
                    DocError::Malformed(format!("pane {id} label {key:?} is not a string"))
                })?;
                labels.insert(key.clone(), value.to_owned());
            }
            labels
        }
    };
    Ok(DocPane {
        id,
        address,
        name,
        labels,
    })
}

fn object<'a>(
    value: &'a Value,
    what: &str,
) -> Result<&'a serde_json::Map<String, Value>, DocError> {
    value
        .as_object()
        .ok_or_else(|| DocError::Malformed(format!("{what} is not an object")))
}

fn string(value: Option<&Value>, what: &str) -> Result<String, DocError> {
    let value = value.ok_or_else(|| DocError::Malformed(format!("missing {what}")))?;
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| DocError::Malformed(format!("{what} is not a string")))
}

fn parse_id(value: Option<&Value>, what: &str, ids: &mut BTreeSet<u64>) -> Result<u64, DocError> {
    let text = value
        .and_then(Value::as_str)
        .ok_or_else(|| DocError::Malformed(format!("{what} is not a decimal string")))?;
    let id: u64 = text
        .parse()
        .map_err(|_| DocError::Malformed(format!("{what} {text:?} is not a u64")))?;
    if !ids.insert(id) {
        return Err(DocError::Malformed(format!("duplicate id {id}")));
    }
    Ok(id)
}

/// Why a shared document is not usable. Neither error may be overwritten.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocError {
    UnsupportedVersion(u64),
    Malformed(String),
}

impl fmt::Display for DocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DocError::UnsupportedVersion(version) => write!(
                f,
                "has schema version {version}, but this client understands version {SCHEMA_VERSION}"
            ),
            DocError::Malformed(detail) => write!(f, "is malformed: {detail}"),
        }
    }
}

impl std::error::Error for DocError {}

/// The pane ids a document mentions, including any that a restore may prune.
pub fn pane_ids(doc: &SharedDoc) -> BTreeSet<PaneId> {
    let mut ids = BTreeSet::new();
    for space in &doc.spaces {
        for tab in &space.tabs {
            if let Some(layout) = &tab.layout {
                for pane in layout.leaves() {
                    ids.insert(PaneId(pane.id));
                }
            }
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::super::super::StartKind;
    use super::*;

    fn workspace() -> Workspace {
        let mut workspace = Workspace::new();
        let first = workspace.start_request(StartKind::Initial);
        workspace.insert_terminal(&first, "t1".into(), "t1".into());
        let second = workspace.start_request(StartKind::Split(Axis::Rows));
        workspace.insert_terminal(&second, "t2".into(), "t2".into());
        workspace
    }

    #[test]
    fn capture_round_trips_through_json() {
        let captured = workspace().capture_doc();
        let value = captured.to_value();
        assert_eq!(SharedDoc::parse(&value).expect("parse"), captured);
        assert_eq!(value["version"], json!(1));
        assert_eq!(value["spaces"][0]["id"], json!("4294967297"));
        assert_eq!(
            value["spaces"][0]["tabs"][0]["layout"]["axis"],
            json!("rows")
        );
    }

    #[test]
    fn the_shared_document_holds_identity_labels_only() {
        let mut workspace = workspace();
        let pane = workspace.panes().next().expect("pane").id;
        workspace.set_labels(
            pane,
            BTreeMap::from([
                (CLIENT_LABEL.to_owned(), CLIENT_NAME.to_owned()),
                (SPACE_LABEL.to_owned(), "1".into()),
                (SEEN_LABEL.to_owned(), "7".into()),
                (PINNED_LABEL.to_owned(), "1".into()),
            ]),
        );
        let value = workspace.capture_doc().to_value();
        let text = value.to_string();
        assert!(text.contains(CLIENT_LABEL), "{text}");
        assert!(text.contains(SPACE_LABEL), "{text}");
        assert!(
            !text.contains(SEEN_LABEL) && !text.contains(PINNED_LABEL),
            "attention labels must never dirty the layout document: {text}"
        );
        assert_eq!(
            workspace.capture_doc(),
            SharedDoc::parse(&value).expect("round trip")
        );
    }

    #[test]
    fn the_document_has_no_local_view() {
        let mut workspace = workspace();
        workspace.toggle_zoom();
        workspace.next_tab();
        let text = workspace.capture_doc().to_value().to_string();
        for local in [
            "focused",
            "zoomed",
            "history",
            "active_space",
            "active_tabs",
        ] {
            assert!(
                !text.contains(local),
                "the shared document must not contain {local}: {text}"
            );
        }
    }

    #[test]
    fn a_future_schema_version_is_refused_not_destroyed() {
        let error = SharedDoc::parse(&json!({"version": 2, "spaces": []})).unwrap_err();
        assert_eq!(error, DocError::UnsupportedVersion(2));
        assert!(error.to_string().contains("version 2"));
    }

    #[test]
    fn malformed_documents_are_refused() {
        let space = json!({"id": "1", "name": "s", "tabs": [
            {"id": "2", "name": "t", "layout": {"leaf": {
                "id": "3", "address": "t1", "name": "t1", "labels": {}
            }}}
        ]});
        let valid = json!({"version": 1, "spaces": [space]});
        let cases = [
            json!(null),
            json!({"version": 1, "spaces": []}),
            json!({"version": 1}),
            json!({"version": 1, "spaces": [
                valid["spaces"][0].clone(),
                valid["spaces"][0].clone(),
            ]}),
            json!({"version": 1, "spaces": [{"id": "1", "name": "s", "tabs": []}]}),
            json!({"version": 1, "spaces": [{"id": "1", "name": "s", "tabs": [
                {"id": "2", "name": "t", "layout": {"leaf": {
                    "id": "3", "address": "", "name": "t1", "labels": {}
                }}}
            ]}]}),
            json!({"version": 1, "spaces": [{"id": "1", "name": "s", "tabs": [
                {"id": "2", "name": "t", "layout": {"axis": "columns", "children": []}}
            ]}]}),
            json!({"version": 1, "spaces": [{"id": "1", "name": "s", "tabs": [
                {"id": "2", "name": "t", "layout": {"axis": "columns", "children": [
                    {"size": 0, "layout": {"leaf": {
                        "id": "3", "address": "t1", "name": "t1", "labels": {}
                    }}},
                    {"size": 1, "layout": {"leaf": {
                        "id": "4", "address": "t2", "name": "t2", "labels": {}
                    }}}
                ]}}
            ]}]}),
            json!({"version": 1, "spaces": [{"id": "1", "name": "s", "tabs": [
                {"id": "1", "name": "t", "layout": null}
            ]}]}),
        ];
        for case in cases {
            assert!(
                matches!(SharedDoc::parse(&case), Err(DocError::Malformed(_))),
                "this must be refused: {case}"
            );
        }
        // The same document, well formed, is accepted.
        SharedDoc::parse(&valid).expect("valid");
    }
}
