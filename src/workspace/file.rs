//! The file workflow's pure half: where a request points, which lines are in
//! view, what changed, and which editor to run.
//!
//! Nothing here opens a file. Core resolves the path and does the IO on this
//! machine or another; these decisions are testable without either.

use crate::core::{FileAt, FileReadResult};

/// Lines one `file.read` window asks for. A large file is never loaded whole
/// into the viewer or a frame.
pub const PAGE: usize = 400;

/// Above these, the line diff stops being worth its memory and reports a
/// whole-block replacement instead.
const MAX_DIFF_CELLS: usize = 1_000_000;
const MAX_DIFF_ROWS: usize = 4_000;

/// Where a file lives: the machine core should use and the directory a
/// relative path starts from. Both are absent when no pane is focused, so
/// core's own default applies.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Context {
    pub on: Option<String>,
    pub cwd: Option<String>,
}

impl Context {
    /// The focused pane's context. A qualified address (`machine/t7`) names a
    /// machine; a local one (`t7`) does not. The terminal's cwd is passed
    /// through; an empty one is no cwd at all.
    pub fn from_pane(address: Option<&str>, cwd: Option<&str>) -> Context {
        let on = address
            .and_then(|address| address.split_once('/'))
            .map(|(machine, _)| machine)
            .filter(|machine| !machine.is_empty())
            .map(str::to_owned);
        let cwd = cwd
            .map(str::trim)
            .filter(|cwd| !cwd.is_empty())
            .map(str::to_owned);
        Context { on, cwd }
    }

    /// The machine name, or `None` for this core.
    pub fn machine(&self) -> Option<&str> {
        self.on.as_deref()
    }

    /// The word a header or error uses for this place.
    pub fn machine_label(&self) -> &str {
        self.on.as_deref().unwrap_or("local")
    }

    /// The path prompt's starting text: the pane's cwd with a separator, so a
    /// relative name can be typed straight after it.
    pub fn prompt(&self) -> String {
        match &self.cwd {
            Some(cwd) if cwd.ends_with('/') => cwd.clone(),
            Some(cwd) => format!("{cwd}/"),
            None => String::new(),
        }
    }

    /// The request context for `path`, unchanged. Local requests omit `on`;
    /// remote requests carry the machine and its remote cwd.
    pub fn file_at(&self, path: &str) -> FileAt {
        FileAt {
            path: path.to_owned(),
            on: self.on.clone(),
            cwd: self.cwd.clone(),
        }
    }

    /// `machine:path` for a header or an error; a local path stands alone.
    pub fn location(&self, path: &str) -> String {
        match self.machine() {
            Some(machine) => format!("{machine}:{path}"),
            None => path.to_owned(),
        }
    }
}

/// One `file.read` reply, kept as core gave it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Window {
    pub text: String,
    /// First line in this window, from 1.
    pub first: usize,
    /// Last line in this window; `last < first` means none.
    pub last: usize,
    /// Lines in the whole file.
    pub lines: usize,
}

impl Window {
    pub fn from_result(result: FileReadResult) -> Window {
        Window {
            text: result.text,
            first: result.first,
            last: result.last,
            lines: result.lines,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.last < self.first || self.text.is_empty()
    }

    pub fn len(&self) -> usize {
        if self.is_empty() {
            0
        } else {
            self.last.saturating_sub(self.first) + 1
        }
    }

    /// The window's lines with their ends stripped, at most `len()` of them.
    pub fn rows(&self) -> Vec<&str> {
        if self.is_empty() {
            return Vec::new();
        }
        self.text
            .split_inclusive('\n')
            .take(self.len())
            .map(display_line)
            .collect()
    }
}

/// A line as it is shown: its line end is not part of the row.
fn display_line(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

/// A read-only view of one file window and its scroll position.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Viewer {
    pub path: String,
    pub context: Context,
    pub window: Window,
    /// First visible line, from 1. It stays inside the loaded window.
    pub top: usize,
}

/// One scrolling key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}

/// The next window to read, because the movement left the loaded one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fetch {
    pub offset: usize,
    pub limit: usize,
}

impl Viewer {
    pub fn new(path: impl Into<String>, context: Context, window: Window) -> Viewer {
        let top = window.first.max(1);
        Viewer {
            path: path.into(),
            context,
            window,
            top,
        }
    }

    /// The lowest top that still fills `height` rows, or the window's first
    /// line when the window is shorter.
    pub fn max_top(&self, height: usize) -> usize {
        if self.window.is_empty() {
            return self.window.first.max(1);
        }
        let height = height.max(1);
        self.window
            .last
            .saturating_sub(height - 1)
            .max(self.window.first)
    }

    /// The visible rows with their 1-based file line numbers.
    pub fn visible(&self, height: usize) -> Vec<(usize, String)> {
        let start = self.top.max(self.window.first);
        self.window
            .rows()
            .into_iter()
            .enumerate()
            .filter_map(|(index, text)| {
                let number = self.window.first + index;
                (number >= start).then(|| (number, text.to_owned()))
            })
            .take(height.max(1))
            .collect()
    }

    /// The range line under a header. It names the loaded window and the
    /// file's total, never the scroll position alone.
    pub fn range_label(&self) -> String {
        if self.window.is_empty() {
            return if self.window.lines == 0 {
                "empty file".to_owned()
            } else {
                format!("no lines · {} in file", self.window.lines)
            };
        }
        if self.window.first == self.window.last {
            format!("line {} of {}", self.window.first, self.window.lines)
        } else {
            format!(
                "lines {}-{} of {}",
                self.window.first, self.window.last, self.window.lines
            )
        }
    }

    /// Applies one movement inside the loaded window. When the movement needs
    /// a line outside it, returns the window to read instead.
    pub fn scroll(&mut self, movement: Move, height: usize) -> Option<Fetch> {
        let height = height.max(1);
        if self.window.is_empty() {
            self.top = self.window.first.max(1);
            return None;
        }
        match movement {
            Move::Up => {
                if self.top > self.window.first {
                    self.top -= 1;
                    None
                } else {
                    self.previous_page()
                }
            }
            Move::Down => {
                if self.top < self.max_top(height) {
                    self.top += 1;
                    None
                } else if self.window.last < self.window.lines {
                    Some(Fetch {
                        offset: self.window.last + 1,
                        limit: PAGE,
                    })
                } else {
                    self.top = self.max_top(height);
                    None
                }
            }
            Move::PageUp => {
                let target = self.top.saturating_sub(height).max(self.window.first);
                if target < self.top {
                    self.top = target;
                    None
                } else {
                    self.previous_page()
                }
            }
            Move::PageDown => {
                let target = self.top.saturating_add(height).min(self.max_top(height));
                if target > self.top {
                    self.top = target;
                    None
                } else if self.window.last < self.window.lines {
                    Some(Fetch {
                        offset: self.window.last + 1,
                        limit: PAGE,
                    })
                } else {
                    self.top = self.max_top(height);
                    None
                }
            }
            Move::Home => {
                if self.window.first > 1 {
                    Some(Fetch {
                        offset: 1,
                        limit: PAGE,
                    })
                } else {
                    self.top = 1;
                    None
                }
            }
            Move::End => {
                if self.window.last < self.window.lines {
                    Some(Fetch {
                        offset: last_page_start(self.window.lines, PAGE),
                        limit: PAGE,
                    })
                } else {
                    self.top = self.max_top(height);
                    None
                }
            }
        }
    }

    fn previous_page(&mut self) -> Option<Fetch> {
        if self.window.first <= 1 {
            self.top = self.window.first.max(1);
            return None;
        }
        let offset = self.window.first.saturating_sub(PAGE).max(1);
        Some(Fetch {
            offset,
            limit: self.window.first - offset,
        })
    }

    /// Installs the window a fetch produced and places the view where that
    /// movement should land: the top of a page moving down, the bottom of a
    /// page moving up, and the documented edge for Home and End.
    pub fn apply(&mut self, window: Window, movement: Move, height: usize) {
        self.window = window;
        let height = height.max(1);
        self.top = match movement {
            Move::Up | Move::PageUp => self.max_top(height).max(self.window.first),
            Move::End => self.max_top(height),
            Move::Down | Move::PageDown | Move::Home => self.window.first.max(1),
        };
        if self.window.is_empty() {
            self.top = self.window.first.max(1);
        }
    }
}

/// The offset whose page ends at the file's last line.
pub fn last_page_start(lines: usize, page: usize) -> usize {
    if lines <= page { 1 } else { lines + 1 - page }
}

/// What the file overlay is showing: the read-only viewer, or a pending
/// editor buffer's diff. The contents live in the runtime; nothing here is
/// part of the workspace document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pane {
    pub path: String,
    pub context: Context,
    pub viewer: Option<Viewer>,
    pub diff: Option<Diff>,
    /// Core's file changed between the first read and the write check. The
    /// write waits for an explicit overwrite or a discard.
    pub conflict: bool,
    /// First diff row on screen.
    pub diff_top: usize,
}

impl Pane {
    pub fn viewing(path: impl Into<String>, context: Context, window: Window) -> Pane {
        let path = path.into();
        let viewer = Viewer::new(path.clone(), context.clone(), window);
        Pane {
            path,
            context,
            viewer: Some(viewer),
            diff: None,
            conflict: false,
            diff_top: 0,
        }
    }

    /// The overlay's heading: the path and the machine it was read through.
    pub fn title(&self) -> String {
        format!("file · {} · {}", self.context.machine_label(), self.path)
    }

    /// The diff rows to draw from the current offset, bounded by `height`.
    pub fn visible_diff(&self, height: usize) -> Vec<&DiffRow> {
        let Some(diff) = &self.diff else {
            return Vec::new();
        };
        diff.rows
            .iter()
            .skip(self.diff_top)
            .take(height.max(1))
            .collect()
    }
}

/// Moves the diff offset inside its rows and returns whether it changed.
pub fn scroll_diff(pane: &mut Pane, movement: Move, height: usize) -> bool {
    let Some(diff) = &pane.diff else {
        return false;
    };
    let height = height.max(1);
    let max_top = diff.rows.len().saturating_sub(height);
    let before = pane.diff_top;
    pane.diff_top = match movement {
        Move::Up => pane.diff_top.saturating_sub(1),
        Move::Down => (pane.diff_top + 1).min(max_top),
        Move::PageUp => pane.diff_top.saturating_sub(height),
        Move::PageDown => (pane.diff_top + height).min(max_top),
        Move::Home => 0,
        Move::End => max_top,
    };
    pane.diff_top != before
}

/// A single contiguous replacement between two texts, when one fits exactly:
/// the same prefix and suffix, one changed span. Used for `file.edit`, which
/// finds the old text exactly once; anything more complex is written whole.
pub fn single_replacement(original: &str, edited: &str) -> Option<crate::core::FileEditOp> {
    if original == edited {
        return None;
    }
    let prefix = common_prefix_len(original, edited);
    let suffix = common_suffix_len(&original[prefix..], &edited[prefix..]);
    let old = &original[prefix..original.len() - suffix];
    let new = &edited[prefix..edited.len() - suffix];
    if old.is_empty() || occurrences(original, old) != 1 {
        return None;
    }
    Some(crate::core::FileEditOp {
        old: old.to_owned(),
        new: new.to_owned(),
    })
}

fn common_prefix_len(left: &str, right: &str) -> usize {
    left.char_indices()
        .zip(right.chars())
        .take_while(|((_, a), b)| a == b)
        .map(|((at, a), _)| at + a.len_utf8())
        .last()
        .unwrap_or(0)
}

fn common_suffix_len(left: &str, right: &str) -> usize {
    left.chars()
        .rev()
        .zip(right.chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum()
}

/// How many times `needle` occurs in `haystack`, overlapping finds included,
/// matching core's `file.edit` rule (`aa` is twice in `aaa`).
fn occurrences(haystack: &str, needle: &str) -> usize {
    let step = needle.chars().next().map_or(1, char::len_utf8);
    let mut count = 0;
    let mut from = 0;
    while let Some(at) = haystack[from..].find(needle) {
        count += 1;
        from += at + step;
    }
    count
}

/// What one diff row means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffTag {
    Same,
    Add,
    Remove,
}

/// One displayed diff row. Add rows have no old line number; Remove rows no
/// new one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffRow {
    pub tag: DiffTag,
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub text: String,
}

/// A line diff and its summary counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diff {
    pub rows: Vec<DiffRow>,
    pub added: usize,
    pub removed: usize,
    /// The diff was too large to align line by line; the changed block is
    /// shown whole.
    pub truncated: bool,
}

impl Diff {
    pub fn summary(&self) -> String {
        match (self.added, self.removed) {
            (0, 0) => "no changes".to_owned(),
            (0, removed) => format!("{removed} removed"),
            (added, 0) => format!("{added} added"),
            (added, removed) => format!("{added} added, {removed} removed"),
        }
    }
}

/// A line diff of `original` against `edited`, by line content. A small LCS
/// keeps adjacent edits aligned; beyond the caps the middle is one replaced
/// block. No merge or three-way resolution is implied.
pub fn diff(original: &str, edited: &str) -> Diff {
    let old = lines(original);
    let new = lines(edited);
    let mut same_prefix = 0;
    while same_prefix < old.len() && same_prefix < new.len() && old[same_prefix] == new[same_prefix]
    {
        same_prefix += 1;
    }
    let mut same_suffix = 0;
    while same_suffix < old.len() - same_prefix
        && same_suffix < new.len() - same_prefix
        && old[old.len() - 1 - same_suffix] == new[new.len() - 1 - same_suffix]
    {
        same_suffix += 1;
    }
    let old_mid = &old[same_prefix..old.len() - same_suffix];
    let new_mid = &new[same_prefix..new.len() - same_suffix];

    let cells = (old_mid.len() + 1).saturating_mul(new_mid.len() + 1);
    let mut out = Diff::default();
    let mut old_no = 1;
    let mut new_no = 1;
    for line in &old[..same_prefix] {
        push_same(&mut out, &mut old_no, &mut new_no, line);
    }
    if cells > MAX_DIFF_CELLS {
        out.truncated = true;
        for line in old_mid {
            push_removed(&mut out, &mut old_no, line);
        }
        for line in new_mid {
            push_added(&mut out, &mut new_no, line);
        }
    } else {
        for (tag, old_index, new_index) in align(old_mid, new_mid) {
            match tag {
                DiffTag::Same => push_same(&mut out, &mut old_no, &mut new_no, old_mid[old_index]),
                DiffTag::Remove => push_removed(&mut out, &mut old_no, old_mid[old_index]),
                DiffTag::Add => push_added(&mut out, &mut new_no, new_mid[new_index]),
            }
        }
    }
    for line in &old[old.len() - same_suffix..] {
        push_same(&mut out, &mut old_no, &mut new_no, line);
    }
    if out.rows.len() > MAX_DIFF_ROWS {
        out.rows.truncate(MAX_DIFF_ROWS);
        out.truncated = true;
    }
    out
}

fn lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        Vec::new()
    } else {
        text.split_inclusive('\n').collect()
    }
}

/// LCS alignment of two line slices. On a tie the removal is emitted before
/// the addition, so a replaced line appears as `-` then `+`.
fn align(old: &[&str], new: &[&str]) -> Vec<(DiffTag, usize, usize)> {
    let (n, m) = (old.len(), new.len());
    let mut table = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if old[i] == new[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0usize, 0usize);
    let mut ops = Vec::with_capacity(n + m);
    while i < n || j < m {
        if i < n && j < m && old[i] == new[j] {
            ops.push((DiffTag::Same, i, j));
            i += 1;
            j += 1;
        } else if i < n && (j == m || table[i + 1][j] >= table[i][j + 1]) {
            ops.push((DiffTag::Remove, i, j));
            i += 1;
        } else {
            ops.push((DiffTag::Add, i, j));
            j += 1;
        }
    }
    ops
}

fn push_same(out: &mut Diff, old_no: &mut usize, new_no: &mut usize, line: &str) {
    out.rows.push(DiffRow {
        tag: DiffTag::Same,
        old: Some(*old_no),
        new: Some(*new_no),
        text: display_line(line).to_owned(),
    });
    *old_no += 1;
    *new_no += 1;
}

fn push_removed(out: &mut Diff, old_no: &mut usize, line: &str) {
    out.rows.push(DiffRow {
        tag: DiffTag::Remove,
        old: Some(*old_no),
        new: None,
        text: display_line(line).to_owned(),
    });
    *old_no += 1;
    out.removed += 1;
}

fn push_added(out: &mut Diff, new_no: &mut usize, line: &str) {
    out.rows.push(DiffRow {
        tag: DiffTag::Add,
        old: None,
        new: Some(*new_no),
        text: display_line(line).to_owned(),
    });
    *new_no += 1;
    out.added += 1;
}

/// The changed-file check. Core exposes no version or etag, so the text read
/// again before a write is compared with the text first read: any difference
/// means another writer changed it and the write must not proceed silently.
pub fn changed_since(original: &str, reread: &str) -> bool {
    original != reread
}

/// Whether a whole-file `file.read` reply covers the whole file. Core caps an
/// unbounded read at about 8 MB of JSON and answers a whole-line prefix with
/// `last < lines` and no error; such a reply must never be edited, compared
/// or written back. Paged (offset/limit) reads may be partial by design and
/// are not classified here.
pub fn is_complete(result: &FileReadResult) -> bool {
    (result.text.is_empty() && result.lines == 0)
        || (result.first <= result.last && result.last >= result.lines)
}

/// How many lines a reply actually carries.
pub fn given_lines(result: &FileReadResult) -> usize {
    if result.last < result.first {
        0
    } else {
        result.last - result.first + 1
    }
}

/// The refusal for a capped whole read: where the file is, why it cannot be
/// edited, and how much was readable.
pub fn too_large_message(context: &Context, path: &str, result: &FileReadResult) -> String {
    format!(
        "too large to edit through Core's inline read: {}:{} ({} of {} lines read)",
        context.machine_label(),
        path,
        given_lines(result),
        result.lines
    )
}

/// The local temp file for an editor session. It keeps the source file's
/// name and extension so the editor's syntax mode still applies.
pub fn temp_file_name(pid: u32, serial: u64, path: &str) -> String {
    let base = path
        .rsplit(['/', '\\'])
        .next()
        .filter(|base| !base.is_empty())
        .unwrap_or("file");
    let base: String = base
        .chars()
        .map(|c| if c.is_control() { '_' } else { c })
        .collect();
    format!("ht-{pid}-{serial}-{base}")
}

/// The editor to run: `$VISUAL` first, then `$EDITOR`, split on whitespace.
/// There is no fallback editor; a client with neither reports it.
pub fn editor(visual: Option<&str>, editor: Option<&str>) -> Option<Vec<String>> {
    visual
        .and_then(parse_command)
        .or_else(|| editor.and_then(parse_command))
}

fn parse_command(command: &str) -> Option<Vec<String>> {
    let mut parts = command.split_whitespace();
    let program = parts.next()?;
    let mut argv = vec![program.to_owned()];
    argv.extend(parts.map(str::to_owned));
    Some(argv)
}

#[cfg(test)]
mod tests;
