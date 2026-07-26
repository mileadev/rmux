use super::Screen;
use crate::grid::{GridLine, GridLineFlags, GridStringState};
use crate::input::{InputParser, COLOUR_DEFAULT};
use crate::{GridRenderOptions, ScreenCaptureRange};
use rmux_proto::TerminalSize;

fn parse(screen: &mut Screen, bytes: &[u8]) {
    let mut parser = InputParser::new();
    parser.parse(bytes, screen);
}

fn screen(cols: u16, rows: u16, history: usize, payload: &[u8]) -> Screen {
    let mut screen = Screen::new(TerminalSize { cols, rows }, history);
    parse(&mut screen, payload);
    screen
}

fn full_range() -> ScreenCaptureRange {
    ScreenCaptureRange {
        start_is_absolute: true,
        end_is_absolute: true,
        ..ScreenCaptureRange::default()
    }
}

fn raw_capture(screen: &Screen) -> Vec<u8> {
    screen.capture_transcript(
        full_range(),
        GridRenderOptions {
            join_wrapped: false,
            include_empty_cells: true,
            trim_spaces: true,
            ..GridRenderOptions::default()
        },
    )
}

fn joined_capture(screen: &Screen) -> Vec<u8> {
    screen.capture_transcript(
        full_range(),
        GridRenderOptions {
            join_wrapped: true,
            include_empty_cells: false,
            trim_spaces: false,
            ..GridRenderOptions::default()
        },
    )
}

#[test]
fn joined_capture_restores_compact_ascii_boundary_spaces() {
    let cases = [
        (
            "one-space-width-8",
            8,
            4,
            20,
            b"abc def gh".as_slice(),
            b"abc def\ngh\n\n\n".as_slice(),
            b"abc def gh\n\n\n".as_slice(),
        ),
        (
            "two-spaces-width-12",
            12,
            4,
            20,
            b"alpha beta  42".as_slice(),
            b"alpha beta\n42\n\n\n".as_slice(),
            b"alpha beta  42\n\n\n".as_slice(),
        ),
        (
            "one-space-width-20",
            20,
            4,
            20,
            b"hello world foo bar baz".as_slice(),
            b"hello world foo bar\nbaz\n\n\n".as_slice(),
            b"hello world foo bar baz\n\n\n".as_slice(),
        ),
        (
            "utf8-suffix",
            8,
            4,
            20,
            "abc def 界".as_bytes(),
            "abc def\n界\n\n\n".as_bytes(),
            "abc def 界\n\n\n".as_bytes(),
        ),
        (
            "viewport-without-history",
            8,
            4,
            0,
            b"abc def gh".as_slice(),
            b"abc def\ngh\n\n\n".as_slice(),
            b"abc def gh\n\n\n".as_slice(),
        ),
    ];

    for (label, cols, rows, history, payload, expected_raw, expected_joined) in cases {
        let screen = screen(cols, rows, history, payload);
        assert_eq!(raw_capture(&screen), expected_raw, "{label}: raw capture");
        assert_eq!(
            joined_capture(&screen),
            expected_joined,
            "{label}: first joined capture"
        );
        assert_eq!(
            joined_capture(&screen),
            expected_joined,
            "{label}: repeated joined capture"
        );
    }
}

#[test]
fn joined_capture_restores_compact_ascii_space_in_scrollback() {
    let screen = screen(8, 3, 20, b"abc def gh\r\nHARDONE\r\nHARDTWO\r\nTAIL");
    assert!(screen.history_size() > 0, "fixture must reach scrollback");
    assert_eq!(
        raw_capture(&screen),
        b"abc def\ngh\nHARDONE\nHARDTWO\nTAIL\n"
    );
    assert_eq!(
        joined_capture(&screen),
        b"abc def gh\nHARDONE\nHARDTWO\nTAIL\n"
    );
}

#[test]
fn joined_capture_restores_repeated_compact_ascii_boundaries() {
    let screen = screen(8, 5, 20, b"abc def gh\r\nabc def gh");
    assert_eq!(raw_capture(&screen), b"abc def\ngh\nabc def\ngh\n\n");
    assert_eq!(joined_capture(&screen), b"abc def gh\nabc def gh\n\n");
}

#[test]
fn joined_capture_keeps_exact_utf8_and_sgr_sentinels() {
    let cases = [
        (
            "exact-wrap",
            8,
            b"ABCDEFGHIJKLMNOP".as_slice(),
            b"ABCDEFGH\nIJKLMNOP\n\n\n".as_slice(),
            b"ABCDEFGHIJKLMNOP\n\n\n".as_slice(),
        ),
        (
            "utf8-line",
            12,
            "été café ab çà".as_bytes(),
            "été café ab\nçà\n\n\n".as_bytes(),
            "été café ab çà\n\n\n".as_bytes(),
        ),
        (
            "sgr-line",
            8,
            b"\x1b[31mabc def \x1b[0mgh".as_slice(),
            b"abc def\ngh\n\n\n".as_slice(),
            b"abc def gh\n\n\n".as_slice(),
        ),
    ];

    for (label, cols, payload, expected_raw, expected_joined) in cases {
        let screen = screen(cols, 4, 20, payload);
        assert_eq!(raw_capture(&screen), expected_raw, "{label}: raw capture");
        assert_eq!(
            joined_capture(&screen),
            expected_joined,
            "{label}: joined capture"
        );
    }
}

#[test]
fn joined_capture_hard_and_terminal_spaces_product_divergence() {
    let hard_break = screen(8, 4, 20, b"abc   \r\nnext");
    assert_eq!(raw_capture(&hard_break), b"abc\nnext\n\n\n");
    assert_eq!(joined_capture(&hard_break), b"abc\nnext\n\n\n");

    let terminal = screen(8, 4, 20, b"abc def ");
    assert_eq!(raw_capture(&terminal), b"abc def\n\n\n\n");
    assert_eq!(joined_capture(&terminal), b"abc def\n\n\n\n");
}

#[test]
fn compact_ascii_boundary_space_respects_include_empty_cells() {
    let screen = screen(8, 4, 20, b"abc def gh");
    let line = screen.grid().visible_line(0).expect("first line exists");
    assert_eq!(line.plain_text(), Some("abc def"));
    assert!(line.cells().is_empty(), "fixture must use compact storage");
    assert!(line.flags().contains(GridLineFlags::WRAPPED));

    let render = |options| {
        screen
            .grid()
            .render_absolute_line(0, options, &mut GridStringState::default(), None)
            .expect("first line renders")
    };
    assert_eq!(
        render(GridRenderOptions {
            join_wrapped: true,
            include_empty_cells: false,
            trim_spaces: false,
            ..GridRenderOptions::default()
        }),
        "abc def "
    );
    assert_eq!(
        render(GridRenderOptions {
            join_wrapped: false,
            include_empty_cells: false,
            trim_spaces: false,
            ..GridRenderOptions::default()
        }),
        "abc def"
    );
    assert_eq!(
        render(GridRenderOptions {
            join_wrapped: false,
            include_empty_cells: true,
            trim_spaces: false,
            ..GridRenderOptions::default()
        }),
        "abc def "
    );
    assert_eq!(
        render(GridRenderOptions {
            join_wrapped: false,
            include_empty_cells: true,
            trim_spaces: true,
            ..GridRenderOptions::default()
        }),
        "abc def"
    );
}

#[test]
fn compact_ascii_grid_capture_joins_boundary_space() {
    let screen = screen(8, 4, 20, b"abc def gh");
    assert_eq!(screen.capture_grid(false).lines[..2], ["abc def", "gh"]);
    assert_eq!(screen.capture_grid(true).lines[0], "abc def gh");
}

#[test]
fn compact_ascii_data_end_survives_materialization_without_absorbing_padding() {
    let mut line = GridLine::new(8);
    assert!(line.write_plain_ascii_run(0, b"A  "));
    assert_eq!(line.plain_text(), Some("A"));
    assert!(!line.cell(1).expect("first data space").is_blank());
    assert!(!line.cell(2).expect("second data space").is_blank());
    assert!(line.cell(3).expect("cleared suffix").is_blank());

    line.materialize_for_cell_mutation();
    assert!(line.plain_text().is_none());
    assert!(!line.cell(1).expect("materialized data space").is_blank());
    assert!(!line.cell(2).expect("materialized data space").is_blank());
    assert!(line
        .cell(3)
        .expect("materialized cleared suffix")
        .is_blank());

    line.clear(COLOUR_DEFAULT);
    assert_eq!(line.plain_text(), Some(""));
    assert!(line.cell(0).expect("cleared cell").is_blank());
}
