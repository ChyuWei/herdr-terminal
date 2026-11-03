use super::*;
use crate::core::FileReadResult;

fn window(first: usize, last: usize, lines: usize, text: &str) -> Window {
    Window {
        text: text.to_owned(),
        first,
        last,
        lines,
    }
}

fn three_line_window() -> Window {
    window(1, 3, 30, "one\ntwo\nthree\n")
}

#[test]
fn a_pane_address_names_a_machine_and_keeps_its_cwd() {
    let local = Context::from_pane(Some("t7"), Some("/work/app"));
    assert_eq!(local.on, None, "a short address is local");
    assert_eq!(local.cwd.as_deref(), Some("/work/app"));
    assert_eq!(local.machine_label(), "local");
    assert_eq!(local.location("src/main.rs"), "src/main.rs");
    assert_eq!(local.prompt(), "/work/app/");

    let remote = Context::from_pane(Some("rohan/t7"), Some("/srv/app"));
    assert_eq!(remote.on.as_deref(), Some("rohan"));
    assert_eq!(remote.cwd.as_deref(), Some("/srv/app"));
    assert_eq!(remote.machine_label(), "rohan");
    assert_eq!(remote.location("src/main.rs"), "rohan:src/main.rs");
    assert_eq!(remote.prompt(), "/srv/app/");

    let none = Context::from_pane(None, None);
    assert_eq!(none, Context::default());
    assert!(none.prompt().is_empty());
    assert_eq!(none.file_at("notes.txt").on, None);
    assert_eq!(none.file_at("notes.txt").cwd, None);

    let mut at = remote.file_at("src/main.rs");
    assert_eq!(at.path, "src/main.rs");
    assert_eq!(at.on.as_deref(), Some("rohan"));
    assert_eq!(at.cwd.as_deref(), Some("/srv/app"));
    at.cwd = None;
    assert_eq!(at.cwd, None);
}

#[test]
fn a_cwd_prompt_never_doubles_the_separator() {
    let context = Context {
        on: None,
        cwd: Some("/work/".into()),
    };
    assert_eq!(context.prompt(), "/work/");
    let blank = Context::from_pane(None, Some("   "));
    assert!(blank.prompt().is_empty(), "a blank cwd is no cwd");
}

#[test]
fn a_window_reports_the_lines_core_gave_and_an_empty_range() {
    let view = three_line_window();
    assert_eq!(view.len(), 3);
    assert_eq!(view.rows(), vec!["one", "two", "three"]);
    assert_eq!(view.first, 1);
    assert_eq!(view.last, 3);
    assert_eq!(view.lines, 30);

    let none = window(1, 0, 0, "");
    assert!(none.is_empty());
    assert_eq!(none.len(), 0);
    assert!(none.rows().is_empty());
    assert_eq!(none.last, none.first.wrapping_sub(1));
    let empty_file = Window::from_result(FileReadResult {
        text: String::new(),
        first: 1,
        last: 0,
        lines: 0,
    });
    assert_eq!(empty_file, none);
}

#[test]
fn a_viewer_shows_the_window_and_describes_its_range() {
    let viewer = Viewer::new("notes.txt", Context::default(), three_line_window());
    assert_eq!(viewer.top, 1);
    assert_eq!(viewer.range_label(), "lines 1-3 of 30");
    assert_eq!(
        viewer.visible(2),
        vec![(1, "one".to_owned()), (2, "two".to_owned())],
        "the viewport limits visible rows"
    );

    let single = Viewer::new("notes.txt", Context::default(), window(4, 4, 9, "four\n"));
    assert_eq!(single.range_label(), "line 4 of 9");
    let empty = Viewer::new("notes.txt", Context::default(), window(1, 0, 0, ""));
    assert_eq!(empty.range_label(), "empty file");
}

#[test]
fn scrolling_clamps_inside_the_loaded_window() {
    let last = window(1, 3, 3, "one\ntwo\nthree\n");
    let mut viewer = Viewer::new("notes.txt", Context::default(), last);
    assert_eq!(viewer.max_top(2), 2, "the last row fills the viewport");
    assert_eq!(viewer.scroll(Move::Up, 2), None);
    assert_eq!(viewer.top, 1, "Up at the first loaded line stays there");
    assert_eq!(viewer.scroll(Move::Down, 2), None);
    assert_eq!(viewer.top, 2);
    assert_eq!(viewer.scroll(Move::Down, 2), None);
    assert_eq!(viewer.top, 2, "the file bottom is a hard stop");
    viewer.scroll(Move::Home, 2);
    assert_eq!(viewer.top, 1);
    viewer.scroll(Move::End, 2);
    assert_eq!(viewer.top, 2);
    viewer.scroll(Move::PageUp, 2);
    assert_eq!(viewer.top, 1);
    viewer.scroll(Move::PageDown, 2);
    assert_eq!(viewer.top, 2);

    let mut more = Viewer::new("notes.txt", Context::default(), three_line_window());
    assert_eq!(
        more.scroll(Move::Down, 3),
        Some(Fetch {
            offset: 4,
            limit: PAGE
        }),
        "down at the bottom of a non-final window asks for the next page"
    );
}

#[test]
fn a_page_that_leaves_the_window_is_a_fetch() {
    let mut first_page = Viewer::new("notes.txt", Context::default(), window(1, 400, 1000, "x\n"));
    first_page.top = first_page.max_top(10);
    assert_eq!(first_page.top, 391);
    assert_eq!(
        first_page.scroll(Move::Down, 10),
        Some(Fetch {
            offset: 401,
            limit: PAGE
        }),
        "moving past the loaded window asks for the next page"
    );
    assert_eq!(
        first_page.scroll(Move::PageDown, 10),
        Some(Fetch {
            offset: 401,
            limit: PAGE
        })
    );
    assert_eq!(
        first_page.scroll(Move::End, 10),
        Some(Fetch {
            offset: 601,
            limit: PAGE
        }),
        "End reads the last page, which starts 400 lines before the end"
    );

    let mut second_page = Viewer::new(
        "notes.txt",
        Context::default(),
        window(401, 800, 1000, "x\n"),
    );
    assert_eq!(
        second_page.scroll(Move::Up, 10),
        Some(Fetch {
            offset: 1,
            limit: 400
        }),
        "the page above ends right before the loaded first line"
    );
    assert_eq!(
        second_page.scroll(Move::PageUp, 10),
        Some(Fetch {
            offset: 1,
            limit: 400
        })
    );
    assert_eq!(
        second_page.scroll(Move::Home, 10),
        Some(Fetch {
            offset: 1,
            limit: PAGE
        })
    );
    let mut last_page = Viewer::new(
        "notes.txt",
        Context::default(),
        window(601, 1000, 1000, "x\n"),
    );
    assert_eq!(last_page.scroll(Move::End, 10), None);
    assert_eq!(last_page.top, 991);
    assert_eq!(last_page.range_label(), "lines 601-1000 of 1000");
    assert_eq!(last_page_start(1000, PAGE), 601);
    assert_eq!(last_page_start(10, PAGE), 1);
}

#[test]
fn an_applied_window_lands_at_the_edge_the_movement_asks_for() {
    let mut viewer = Viewer::new("notes.txt", Context::default(), three_line_window());
    viewer.apply(window(401, 800, 1000, "x\n"), Move::Down, 10);
    assert_eq!(viewer.top, 401, "a page down starts at its first line");
    assert_eq!(viewer.range_label(), "lines 401-800 of 1000");

    viewer.apply(window(1, 400, 1000, "x\n"), Move::Up, 10);
    assert_eq!(viewer.top, 391, "a page up shows its last full screen");

    viewer.apply(window(601, 1000, 1000, "x\n"), Move::End, 10);
    assert_eq!(viewer.top, 991);
    viewer.apply(window(1, 400, 1000, "x\n"), Move::Home, 10);
    assert_eq!(viewer.top, 1);

    viewer.apply(window(1, 0, 0, ""), Move::Down, 10);
    assert_eq!(viewer.top, 1, "an empty file has one stable top");
    assert!(viewer.visible(10).is_empty());
}

#[test]
fn a_line_diff_marks_added_and_removed_lines_and_summarizes_them() {
    let diff = diff("one\ntwo\nthree\n", "one\nTWO\nthree\nfour\n");
    let marks: Vec<(DiffTag, &str)> = diff
        .rows
        .iter()
        .map(|row| (row.tag, row.text.as_str()))
        .collect();
    assert_eq!(
        marks,
        vec![
            (DiffTag::Same, "one"),
            (DiffTag::Remove, "two"),
            (DiffTag::Add, "TWO"),
            (DiffTag::Same, "three"),
            (DiffTag::Add, "four"),
        ]
    );
    assert_eq!((diff.added, diff.removed), (2, 1));
    assert_eq!(diff.summary(), "2 added, 1 removed");
    assert!(!diff.truncated);
    let numbers: Vec<(Option<usize>, Option<usize>)> =
        diff.rows.iter().map(|row| (row.old, row.new)).collect();
    assert_eq!(
        numbers,
        vec![
            (Some(1), Some(1)),
            (Some(2), None),
            (None, Some(2)),
            (Some(3), Some(3)),
            (None, Some(4)),
        ]
    );
}

#[test]
fn identical_text_has_no_diff_and_line_ends_are_not_rows() {
    let equal = diff("a\r\nb\n", "a\r\nb\n");
    assert_eq!(equal.added, 0);
    assert_eq!(equal.removed, 0);
    assert_eq!(equal.summary(), "no changes");
    assert!(
        equal
            .rows
            .iter()
            .all(|row| row.tag == DiffTag::Same && !row.text.contains('\r'))
    );
    assert_eq!(
        diff("", "").summary(),
        "no changes",
        "two empty files are equal"
    );
    assert_eq!(diff("", "new\n").added, 1);
    assert_eq!(diff("old\n", "").removed, 1);
}

#[test]
fn a_huge_change_is_one_replaced_block_and_still_counted() {
    let old = "same\n".to_owned() + &"old\n".repeat(1_100);
    let new = "same\n".to_owned() + &"new\n".repeat(1_100);
    let diff = diff(&old, &new);
    assert!(diff.truncated, "over the alignment cap");
    assert_eq!(diff.removed, 1_100);
    assert_eq!(diff.added, 1_100);
    assert_eq!(diff.rows[0].tag, DiffTag::Same);
    assert_eq!(diff.rows[1].tag, DiffTag::Remove);
    assert_eq!(diff.rows[1 + 1_100].tag, DiffTag::Add);
}

#[test]
fn a_conflict_is_any_difference_in_the_text_read_again() {
    assert!(!changed_since("one\ntwo\n", "one\ntwo\n"));
    assert!(changed_since("one\ntwo\n", "one\nTWO\n"));
    assert!(
        changed_since("one\n", "one\n\n"),
        "even a missing final newline is a change"
    );
    assert!(changed_since("", "anything\n"));
}

#[test]
fn the_temp_name_keeps_the_source_name_and_extension() {
    assert_eq!(
        temp_file_name(4242, 7, "/srv/app/src/main.rs"),
        "ht-4242-7-main.rs"
    );
    assert_eq!(temp_file_name(1, 0, "notes.md"), "ht-1-0-notes.md");
    assert_eq!(
        temp_file_name(1, 0, "C:\\work\\app.toml"),
        "ht-1-0-app.toml"
    );
    assert_eq!(temp_file_name(1, 0, "/a/dir/"), "ht-1-0-file");
    assert_eq!(temp_file_name(1, 0, "a\u{7}b.txt"), "ht-1-0-a_b.txt");
}

#[test]
fn the_editor_prefers_visual_then_editor_and_has_no_fallback() {
    assert_eq!(
        editor(Some("nvim -u NONE"), Some("vi")),
        Some(vec!["nvim".into(), "-u".into(), "NONE".into()])
    );
    assert_eq!(
        editor(None, Some("code --wait")),
        Some(vec!["code".into(), "--wait".into()])
    );
    assert_eq!(editor(Some("   "), Some("vi")), Some(vec!["vi".into()]));
    assert_eq!(editor(None, None), None);
    assert_eq!(editor(Some(""), None), None);
}

#[test]
fn a_single_exact_replacement_is_an_edit_and_anything_else_is_not() {
    let replaced = single_replacement("one\ntwo\nthree\n", "one\nTWO\nthree\n")
        .expect("one contiguous replacement");
    assert_eq!(replaced.old, "two");
    assert_eq!(replaced.new, "TWO");

    let inserted = single_replacement("one\nthree\n", "one\ntwo\nthree\n");
    assert_eq!(
        inserted, None,
        "an insertion has an empty old text, which core refuses; the writer falls back"
    );

    assert_eq!(single_replacement("same\n", "same\n"), None);
    assert_eq!(
        single_replacement("a\nb\n", "A\nB\n"),
        Some(crate::core::FileEditOp {
            old: "a\nb".into(),
            new: "A\nB".into()
        }),
        "two adjacent changed lines are still one contiguous replacement"
    );
    assert_eq!(
        single_replacement("x\nx\n", "x\nX\n"),
        None,
        "old text that occurs more than once cannot be found exactly once"
    );
    assert_eq!(
        single_replacement("a", "b"),
        Some(crate::core::FileEditOp {
            old: "a".into(),
            new: "b".into()
        })
    );
    assert_eq!(
        single_replacement("", "new"),
        None,
        "there is no non-empty old text to find"
    );
}

#[test]
fn the_pane_titles_the_machine_and_limits_the_visible_diff() {
    let pane = Pane::viewing(
        "/srv/app/notes.txt",
        Context::from_pane(Some("rohan/t7"), None),
        three_line_window(),
    );
    assert_eq!(
        pane.title(),
        "file · rohan · /srv/app/notes.txt",
        "a remote file names its machine"
    );
    assert_eq!(
        pane.viewer.as_ref().expect("viewer").range_label(),
        "lines 1-3 of 30"
    );

    let mut pane = pane;
    pane.diff = Some(diff("a\nb\nc\n", "a\nB\nc\n"));
    assert_eq!(pane.visible_diff(2).len(), 2);
    assert!(scroll_diff(&mut pane, Move::Down, 2));
    assert_eq!(pane.diff_top, 1);
    scroll_diff(&mut pane, Move::End, 2);
    assert_eq!(pane.diff_top, 2, "the rows stop at the last full screen");
    scroll_diff(&mut pane, Move::Home, 2);
    assert_eq!(pane.diff_top, 0);
    pane.diff = None;
    assert!(!scroll_diff(&mut pane, Move::Down, 2));
}

#[test]
fn only_a_whole_read_that_reaches_the_last_line_is_complete() {
    let read = |text: &str, first: usize, last: usize, lines: usize| {
        is_complete(&FileReadResult {
            text: text.into(),
            first,
            last,
            lines,
        })
    };
    assert!(read("", 1, 0, 0), "an empty file is complete");
    assert!(read("one\ntwo\n", 1, 2, 2), "the last line is present");
    assert!(
        !read("one\ntwo\n", 1, 1, 2),
        "a prefix that stops before the last line is truncated"
    );
    assert!(!read("one\n", 1, 1, 5), "a capped reply is truncated");
    assert!(
        !read("", 1, 0, 5),
        "empty text is only complete for an empty file"
    );
    assert!(
        !read("x\n", 1, 1, 1000),
        "short text does not make a reply complete"
    );
    assert!(
        !read("x\n", 3, 2, 2),
        "a range with last < first carries nothing"
    );
    assert!(
        read("five\n", 2, 5, 5),
        "the documented predicate is used, not a guess from the text"
    );
    let empty_range = FileReadResult {
        text: String::new(),
        first: 1,
        last: 0,
        lines: 5,
    };
    assert_eq!(given_lines(&empty_range), 0);
}

#[test]
fn a_truncated_read_refusal_names_the_machine_file_and_counts() {
    let result = FileReadResult {
        text: "one\ntwo\n".into(),
        first: 1,
        last: 2,
        lines: 5,
    };
    assert_eq!(given_lines(&result), 2);
    assert_eq!(
        too_large_message(&Context::default(), "notes.txt", &result),
        "too large to edit through Core's inline read: local:notes.txt (2 of 5 lines read)"
    );
    let remote = Context::from_pane(Some("alpha/t7"), None);
    let message = too_large_message(&remote, "/srv/app/notes.txt", &result);
    assert!(message.contains("alpha:/srv/app/notes.txt"), "{message}");
    assert!(message.contains("2 of 5 lines read"), "{message}");
}
