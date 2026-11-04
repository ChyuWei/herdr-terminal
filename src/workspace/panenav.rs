//! The narrow pane-nav: one row of compact chips above the pane area.
//!
//! A narrow window draws only the focused pane, so this row is the map of the
//! tab. Every chip is `NN name`, in split-tree order; numbers do not change
//! when panes are hidden. When the chips do not fit, a window of whole chips
//! scrolls to keep the focused pane visible, so keyboard focus always reaches
//! every pane and every visible chip stays clickable. The row never overflows.

use super::agents::AgentRow;
use super::model::{PaneId, Workspace};
use ratatui::layout::Rect;
use ratatui::text::Span;

/// The smallest useful chip: a two-digit number and one name cell.
pub const MIN_CHIP: u16 = 4;
/// The widest one chip grows; longer names are truncated.
pub const MAX_CHIP: u16 = 24;
/// The cell between two chips.
const GAP: u16 = 1;

/// One pane as the row shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub pane: PaneId,
    pub name: String,
    /// The status word, shown only when the chip has room.
    pub word: Option<String>,
}

/// The active tab's chips, built from the one shared word rule. Drawing and
/// mouse hit testing must both call this: a different word changes a chip's
/// width, and with it every later chip's rectangle.
pub fn entries(workspace: &Workspace, rows: &[AgentRow]) -> Vec<Entry> {
    let Some(tab) = workspace.active_tab() else {
        return Vec::new();
    };
    tab.panes()
        .into_iter()
        .filter_map(|pane| {
            let record = workspace.pane(pane)?;
            let word = match record.exit {
                Some(code) => Some(format!("exited {code}")),
                None => word(&record.status, rows, pane),
            };
            Some(Entry {
                pane,
                name: record.name.clone(),
                word,
            })
        })
        .collect()
}

/// The one status word a pane shows in its identity row and its chip. The
/// agent index already derives the display word (`done` becomes `idle` once
/// seen, `error` becomes `blocked`); the pane chrome uses that same word so
/// the rail, the chip and the header never disagree. Without a row the pane's
/// own OSC 7501 word is used as-is; nothing is invented.
pub fn word(status: &Option<String>, rows: &[AgentRow], pane: PaneId) -> Option<String> {
    rows.iter()
        .find(|row| row.pane == pane)
        .map(|row| row.word.clone())
        .or_else(|| status.clone().filter(|word| !word.is_empty()))
}

/// One drawn chip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chip {
    pub pane: PaneId,
    pub rect: Rect,
    /// The chip's text, already truncated to `rect.width`.
    pub label: String,
    /// The room left for the word inside the chip, when it was shown.
    pub word: Option<String>,
    pub focused: bool,
}

/// Lays out the chips for `area` and `focused`. The result never overlaps or
/// leaves `area`.
pub fn layout(entries: &[Entry], focused: Option<PaneId>, area: Rect) -> Vec<Chip> {
    if entries.is_empty() || area.width == 0 || area.height == 0 {
        return Vec::new();
    }
    let base: Vec<String> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| format!("{:02} {}", index + 1, entry.name))
        .collect();
    let desired: Vec<u16> = base.iter().map(|text| width(text)).collect();
    let count = entries.len() as u16;

    // Every chip at its natural width (bounded) fits: show them all and add
    // the word where it still fits.
    let base_total: u16 = desired
        .iter()
        .map(|wanted| (*wanted).min(MAX_CHIP))
        .sum::<u16>()
        .saturating_add(GAP.saturating_mul(count - 1));
    if base_total <= area.width {
        let mut widths: Vec<u16> = desired
            .iter()
            .map(|wanted| (*wanted).min(MAX_CHIP).max(MIN_CHIP.min(*wanted)))
            .collect();
        // A label longer than the capped chip must be ellipsized, not left for
        // the renderer to hard-cut at the rect edge.
        let mut labels: Vec<String> = base
            .iter()
            .zip(&widths)
            .map(|(text, width)| truncate(text, usize::from(*width)))
            .collect();
        let mut words: Vec<Option<String>> = vec![None; entries.len()];
        let mut left = area.width.saturating_sub(base_total);
        for (index, entry) in entries.iter().enumerate() {
            let Some(word) = entry.word.as_deref().filter(|word| !word.is_empty()) else {
                continue;
            };
            let extra = GAP.saturating_add(width(word));
            if left >= extra && widths[index].saturating_add(extra) <= MAX_CHIP {
                widths[index] += extra;
                labels[index].push(' ');
                labels[index].push_str(word);
                words[index] = Some(word.to_owned());
                left -= extra;
            }
        }
        return place(entries, &labels, &words, &widths, focused, area);
    }

    // Even minimum chips fit: truncate every name into equal slots.
    let minimum_total = MIN_CHIP
        .saturating_mul(count)
        .saturating_add(GAP.saturating_mul(count - 1));
    if minimum_total <= area.width {
        let slot = (area.width.saturating_sub(GAP.saturating_mul(count - 1)) / count)
            .clamp(MIN_CHIP, MAX_CHIP);
        let widths = vec![slot; entries.len()];
        let labels: Vec<String> = base
            .iter()
            .map(|text| truncate(text, slot as usize))
            .collect();
        let words = vec![None; entries.len()];
        return place(entries, &labels, &words, &widths, focused, area);
    }

    // Too many panes for minimum chips: show a window around the focused one.
    let visible = (area.width / (MIN_CHIP + GAP)).max(1) as usize;
    let visible = visible.min(entries.len());
    let focus_index = focused
        .and_then(|pane| entries.iter().position(|entry| entry.pane == pane))
        .unwrap_or(0);
    let mut start = if focus_index < visible {
        0
    } else {
        focus_index + 1 - visible
    };
    start = start.min(entries.len() - visible);
    if focus_index < start {
        start = focus_index;
    } else if focus_index >= start + visible {
        start = focus_index + 1 - visible;
    }
    let slot = (area
        .width
        .saturating_sub(GAP.saturating_mul(visible as u16 - 1))
        / visible as u16)
        .clamp(MIN_CHIP, MAX_CHIP);
    let widths = vec![slot; visible];
    let labels: Vec<String> = base[start..start + visible]
        .iter()
        .map(|text| truncate(text, slot as usize))
        .collect();
    let words = vec![None; visible];
    place(
        &entries[start..start + visible],
        &labels,
        &words,
        &widths,
        focused,
        area,
    )
}

fn place(
    entries: &[Entry],
    labels: &[String],
    words: &[Option<String>],
    widths: &[u16],
    focused: Option<PaneId>,
    area: Rect,
) -> Vec<Chip> {
    let mut chips = Vec::with_capacity(entries.len());
    let mut x = area.x;
    for (index, entry) in entries.iter().enumerate() {
        let width = widths[index];
        if width == 0 || x >= area.x.saturating_add(area.width) {
            break;
        }
        chips.push(Chip {
            pane: entry.pane,
            rect: Rect::new(x, area.y, width, 1),
            label: labels[index].clone(),
            word: words[index].clone(),
            focused: focused == Some(entry.pane),
        });
        x = x.saturating_add(width).saturating_add(GAP);
    }
    chips
}

/// The chip under a cell.
pub fn at(chips: &[Chip], x: u16, y: u16) -> Option<PaneId> {
    chips
        .iter()
        .find(|chip| {
            let rect = chip.rect;
            x >= rect.x
                && x < rect.x.saturating_add(rect.width)
                && y >= rect.y
                && y < rect.y.saturating_add(rect.height)
        })
        .map(|chip| chip.pane)
}

/// Display width, not character count.
fn width(text: &str) -> u16 {
    u16::try_from(Span::raw(text).width()).unwrap_or(u16::MAX)
}

/// Truncates to `room` display columns, reserving one for an ellipsis. Wide
/// characters are never split.
pub fn truncate(text: &str, room: usize) -> String {
    if Span::raw(text).width() <= room {
        return text.to_owned();
    }
    if room == 0 {
        return String::new();
    }
    let budget = room - 1;
    let mut used = 0;
    let mut shortened = String::new();
    for ch in text.chars() {
        let cell = Span::raw(ch.to_string()).width();
        if used + cell > budget {
            break;
        }
        used += cell;
        shortened.push(ch);
    }
    shortened.push('…');
    shortened
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pane: u64, name: &str) -> Entry {
        Entry {
            pane: PaneId(pane),
            name: name.to_owned(),
            word: None,
        }
    }

    #[test]
    fn entries_use_the_agents_word_and_the_exit_fact_at_the_chip_edge() {
        use crate::workspace::agents::Priority;
        use crate::workspace::model::{Axis, StartKind};

        let mut workspace = Workspace::new();
        let request = workspace.start_request(StartKind::Initial);
        let first = workspace.insert_terminal(&request, "t1".into(), "t1".into());
        let request = workspace.start_request(StartKind::Split(Axis::Columns));
        let second = workspace.insert_terminal(&request, "t2".into(), "t2".into());
        workspace.set_status(first, Some("error".into()));
        let row = AgentRow {
            pane: first,
            address: "t1".into(),
            title: "Build".into(),
            name: "t1".into(),
            word: "blocked".into(),
            priority: Priority::Blocked,
            turn: 1,
            pinned: false,
            seen: true,
            seen_turn: Some(1),
            status: None,
            foreground: None,
            state: "live".into(),
            exit: None,
            order: (0, 0, 0),
        };
        let listed = entries(&workspace, &[row]);
        assert_eq!(
            listed[0].word.as_deref(),
            Some("blocked"),
            "a raw error is the index's blocked"
        );
        assert_eq!(listed[1].word, None, "a plain shell has no word");
        let chips = layout(&listed, Some(second), Rect::new(0, 0, 80, 1));
        assert!(chips[0].label.contains("blocked"), "{chips:?}");
        let edge = chips[0].rect.x + chips[0].rect.width - 1;
        assert_eq!(at(&chips, edge, 0), Some(first), "the first chip's edge");
        assert_eq!(
            at(&chips, chips[1].rect.x, 0),
            Some(second),
            "the next chip starts after the derived word"
        );

        workspace.mark_finished(first, 3);
        let listed = entries(&workspace, &[]);
        assert_eq!(listed[0].word.as_deref(), Some("exited 3"));
        let chips = layout(&listed, Some(first), Rect::new(0, 0, 80, 1));
        assert!(chips[0].label.contains("exited 3"), "{chips:?}");
        assert_eq!(
            at(&chips, chips[0].rect.x + chips[0].rect.width - 1, 0),
            Some(first)
        );
    }

    #[test]
    fn a_long_name_in_a_wide_row_is_ellipsized_to_the_chip_cap() {
        let entries = vec![entry(1, "feature-authentication-session")];
        let area = Rect::new(0, 0, 89, 1);
        let chips = layout(&entries, Some(PaneId(1)), area);
        assert_eq!(chips.len(), 1);
        assert_eq!(
            chips[0].rect.width, MAX_CHIP,
            "the chip stops at the cap: {chips:?}"
        );
        assert!(
            chips[0].label.ends_with('…'),
            "the label is ellipsized, not hard-cut: {:?}",
            chips[0].label
        );
        assert!(
            Span::raw(chips[0].label.as_str()).width() <= usize::from(chips[0].rect.width),
            "the label fits its rect: {chips:?}"
        );
        assert_eq!(
            at(&chips, chips[0].rect.x + chips[0].rect.width - 1, 0),
            Some(PaneId(1)),
            "the hit rect is the drawn chip"
        );
    }

    #[test]
    fn chips_are_numbered_in_tree_order_and_every_chip_is_hit_testable() {
        let entries = vec![entry(7, "build"), entry(8, "review"), entry(9, "deploy")];
        let chips = layout(&entries, Some(PaneId(8)), Rect::new(0, 0, 60, 1));
        assert_eq!(chips.len(), 3);
        assert_eq!(chips[0].label, "01 build");
        assert_eq!(chips[1].label, "02 review");
        assert!(chips[1].focused);
        for chip in &chips {
            assert_eq!(
                at(&chips, chip.rect.x, 0),
                Some(chip.pane),
                "chip {:?}",
                chip.label
            );
            assert!(chip.rect.x + chip.rect.width <= 60);
        }
        assert_eq!(at(&chips, 59, 0), None, "past the last chip");
    }

    #[test]
    fn a_status_word_appears_only_while_the_chip_has_room() {
        let mut entries = vec![entry(1, "build")];
        entries[0].word = Some("working".into());
        let chips = layout(&entries, Some(PaneId(1)), Rect::new(0, 0, 40, 1));
        assert_eq!(chips[0].label, "01 build working");
        assert_eq!(chips[0].word.as_deref(), Some("working"));
        let narrow = layout(&entries, Some(PaneId(1)), Rect::new(0, 0, 7, 1));
        assert_eq!(narrow[0].label, "01 bui…");
        assert_eq!(narrow[0].word, None);
    }

    #[test]
    fn long_names_truncate_without_overflowing_the_row() {
        let entries = vec![
            entry(1, "a-very-long-pane-name"),
            entry(2, "another-very-long-name"),
        ];
        let chips = layout(&entries, Some(PaneId(2)), Rect::new(0, 0, 20, 1));
        assert_eq!(chips.len(), 2);
        let mut x = 0;
        for chip in &chips {
            assert_eq!(chip.rect.x, x);
            assert!(chip.rect.x + chip.rect.width <= 20, "{chips:?}");
            assert!(chip.label.contains('…'), "{chip:?}");
            x += chip.rect.width + 1;
        }
    }

    #[test]
    fn many_panes_scroll_a_whole_chip_window_around_the_focused_pane() {
        let entries: Vec<Entry> = (1..=20)
            .map(|index| entry(index, &format!("pane{index}")))
            .collect();
        let area = Rect::new(0, 0, 40, 1);
        let chips = layout(&entries, Some(PaneId(18)), area);
        assert!(!chips.is_empty());
        assert!(
            chips
                .iter()
                .any(|chip| chip.pane == PaneId(18) && chip.focused),
            "the focused chip scrolled out: {chips:?}"
        );
        let mut end = 0;
        for chip in &chips {
            assert_eq!(chip.rect.x, end, "chips are adjacent: {chips:?}");
            assert!(chip.rect.x + chip.rect.width <= area.width);
            end = chip.rect.x + chip.rect.width + 1;
        }
        // The window keeps the focused pane at its right edge and shows an
        // earlier pane, so the row never renumbers or jumps.
        let last = chips.last().expect("a chip");
        assert_eq!(last.pane, PaneId(18));
        assert!(chips.len() > 1);

        let first = layout(&entries, None, area);
        assert_eq!(first.first().map(|chip| chip.pane), Some(PaneId(1)));
    }
}
