use super::*;
use crate::terminal_palette::rgb_color;
use pretty_assertions::assert_eq;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ColorMode {
    Enabled,
    Disabled,
}

/// Crossterm memoizes NO_COLOR, so exercise both contracts in isolated child processes.
fn isolated_color_mode(test_name: &str) -> Option<ColorMode> {
    const CHILD_ENV: &str = "CODEX_CURSOR_COLOR_TEST_CHILD";
    const OUTPUT_LIMIT: u64 = 64 * 1024;
    if let Ok(child) = std::env::var(CHILD_ENV) {
        let (child_test, mode) = child.rsplit_once(':').expect("child fixture mode");
        assert_eq!(child_test, test_name);
        let mode = match mode {
            "enabled" => ColorMode::Enabled,
            "disabled" => ColorMode::Disabled,
            _ => panic!("unknown color fixture mode: {mode}"),
        };
        assert_eq!(
            Colored::ansi_color_disabled_memoized(),
            mode == ColorMode::Disabled
        );
        return Some(mode);
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("color fixture runtime");
    for mode in ["enabled", "disabled"] {
        runtime.block_on(async {
            use tokio::io::AsyncReadExt;

            let mut command =
                tokio::process::Command::new(std::env::current_exe().expect("current test binary"));
            command
                .args(["--exact", test_name, "--nocapture", "--color", "never"])
                .env(CHILD_ENV, format!("{test_name}:{mode}"))
                .env_remove("FORCE_COLOR")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true);
            if mode == "enabled" {
                command.env_remove("NO_COLOR");
            } else {
                command.env("NO_COLOR", "1");
            }
            let mut child = command.spawn().expect("spawn color fixture child");
            let mut stdout = child
                .stdout
                .take()
                .expect("child stdout")
                .take(OUTPUT_LIMIT + 1);
            let mut stderr = child
                .stderr
                .take()
                .expect("child stderr")
                .take(OUTPUT_LIMIT + 1);
            let mut out = Vec::new();
            let mut err = Vec::new();
            let result = tokio::time::timeout(std::time::Duration::from_secs(/*secs*/ 20), async {
                tokio::try_join!(
                    stdout.read_to_end(&mut out),
                    stderr.read_to_end(&mut err),
                    child.wait(),
                )
            })
            .await;
            let status = match result {
                Ok(Ok((_, _, status))) => status,
                failure => {
                    child
                        .kill()
                        .await
                        .expect("stop and reap color fixture child");
                    panic!("color fixture {test_name} ({mode}) did not finish: {failure:?}");
                }
            };
            assert!(
                out.len() <= OUTPUT_LIMIT as usize && err.len() <= OUTPUT_LIMIT as usize,
                "color fixture {test_name} ({mode}) exceeded its output limit"
            );
            let stdout = String::from_utf8_lossy(&out);
            let stderr = String::from_utf8_lossy(&err);
            assert!(
                status.success() && stdout.contains("test result: ok. 1 passed; 0 failed;"),
                "color fixture {test_name} ({mode}): {status}\n{stdout}\n{stderr}"
            );
        });
    }
    None
}

#[test]
fn terminal_draw_keeps_intermediate_cursor_positions_hidden() {
    let mut terminal =
        Terminal::with_options(CaptureBackend::new(/*width*/ 12, /*height*/ 2)).expect("terminal");
    let area = Rect::new(
        /*x*/ 0, /*y*/ 0, /*width*/ 12, /*height*/ 2,
    );
    let cursor = (1, 4);
    terminal.set_viewport_area(area);
    let mut parser =
        vt100::Parser::new(/*rows*/ 2, /*cols*/ 12, /*scrollback_len*/ 0);

    for (index, (marker, show_cursor)) in [
        ("x", true),
        ("y", true),
        ("y", true),
        ("", false),
        ("z", true),
    ]
    .into_iter()
    .enumerate()
    {
        terminal.backend_mut().output.clear();
        terminal
            .draw(|frame| {
                let buffer = frame.buffer_mut();
                buffer.set_string(0, 0, "anchor", Style::default());
                if !marker.is_empty() {
                    buffer.set_string(10, 1, marker, Style::default());
                }
                if show_cursor {
                    frame.set_cursor_position((cursor.1, cursor.0));
                }
            })
            .expect("draw");

        // Process one byte at a time to expose cursor motion even if a terminal displays a
        // synchronized frame before all writes have arrived.
        let output = terminal.backend().output();
        for byte in output.bytes() {
            parser.process(&[byte]);
            if index > 0 && !parser.screen().hide_cursor() {
                assert_eq!(parser.screen().cursor_position(), cursor, "frame {index}");
            }
        }
        assert_eq!(parser.screen().hide_cursor(), !show_cursor, "frame {index}");
        assert_eq!(
            parser.screen().cell(1, 10).expect("marker cell").contents(),
            marker,
            "frame {index}"
        );
    }
}

#[test]
fn terminal_draw_repaints_the_cursor_anchor_only_when_its_style_needs_restoring() {
    let mut terminal =
        Terminal::with_options(CaptureBackend::new(/*width*/ 12, /*height*/ 2)).expect("terminal");
    let area = Rect::new(
        /*x*/ 0, /*y*/ 0, /*width*/ 12, /*height*/ 2,
    );
    terminal.set_viewport_area(area);
    let render = |terminal: &mut Terminal<CaptureBackend>, marker| {
        terminal.backend_mut().output.clear();
        terminal
            .draw(|frame| {
                let buffer = frame.buffer_mut();
                buffer.set_string(0, 0, "transcript", Style::default());
                buffer.set_string(10, 1, marker, Style::default());
                frame.set_cursor_position((4, 1));
            })
            .expect("draw");
        terminal.backend().output()
    };

    let first = render(&mut terminal, "x");
    assert!(first.contains("\x1b[1;1H"));
    let next = render(&mut terminal, "y");
    assert!(next.contains("\x1b[2;11H"));
    assert!(!next.contains("\x1b[1;1H"));

    terminal.invalidate_cursor_state();
    let restored = render(&mut terminal, "y");
    assert!(restored.contains("\x1b[1;1H"));
    let unchanged = render(&mut terminal, "y");
    assert!(!unchanged.contains("\x1b[1;1H"));
}

#[test]
fn terminal_draw_repairs_styled_anchor_on_cursor_only_frames() {
    let Some(color_mode) = isolated_color_mode(
        "custom_terminal::tests::cursor::terminal_draw_repairs_styled_anchor_on_cursor_only_frames",
    ) else {
        return;
    };
    let mut terminal =
        Terminal::with_options(CaptureBackend::new(/*width*/ 12, /*height*/ 2)).expect("terminal");
    let area = Rect::new(
        /*x*/ 0, /*y*/ 1, /*width*/ 12, /*height*/ 1,
    );
    terminal.set_viewport_area(area);
    let mut frames = Vec::new();
    let mut previous_style = None;
    let mut parser =
        vt100::Parser::new(/*rows*/ 2, /*cols*/ 12, /*scrollback_len*/ 0);

    for (x, style) in [
        (1, SetCursorStyle::DefaultUserShape),
        (4, SetCursorStyle::DefaultUserShape),
        (3, SetCursorStyle::SteadyBar),
        (1, SetCursorStyle::SteadyBlock),
    ] {
        terminal.backend_mut().output.clear();
        terminal
            .draw(|frame| {
                Paragraph::new("ab  cd  ef")
                    .style(Style::default().bg(rgb_color((80, 80, 80))).bold())
                    .render(area, frame.buffer_mut());
                frame.set_cursor_style(style);
                frame.set_cursor_position((x, 1));
            })
            .expect("draw");

        let mut style_command = Vec::new();
        queue!(style_command, style).expect("expected cursor style");
        let style_command = String::from_utf8(style_command).expect("cursor style ANSI");
        assert_eq!(
            terminal
                .backend()
                .output()
                .matches(style_command.as_str())
                .count(),
            usize::from(previous_style != Some(style))
        );
        previous_style = Some(style);
        parser.process(&terminal.backend().output);
        assert_eq!(parser.screen().contents(), "\nab  cd  ef  ");
        assert_eq!(parser.screen().cursor_position(), (1, x));
        for column in 0..area.width {
            let cell = parser.screen().cell(1, column).expect("viewport cell");
            assert_eq!(
                cell.bgcolor(),
                match color_mode {
                    ColorMode::Enabled => vt100::Color::Rgb(80, 80, 80),
                    ColorMode::Disabled => vt100::Color::Default,
                }
            );
            assert!(
                cell.bold(),
                "lost anchor or trailing-cell modifier at {column}"
            );
        }
        frames.push(terminal.backend().output().escape_debug().to_string());
    }
    if color_mode == ColorMode::Enabled {
        assert_snapshot!("cursor_style_styled_frames", frames.join("\n"));
    }
}

#[test]
fn terminal_draw_repairs_owned_wide_hyperlink_after_skipped_glyphs() {
    let Some(color_mode) = isolated_color_mode(
        "custom_terminal::tests::cursor::terminal_draw_repairs_owned_wide_hyperlink_after_skipped_glyphs",
    ) else {
        return;
    };
    let mut terminal =
        Terminal::with_options(CaptureBackend::new(/*width*/ 8, /*height*/ 1)).expect("terminal");
    let area = Rect::new(
        /*x*/ 0, /*y*/ 0, /*width*/ 8, /*height*/ 1,
    );
    terminal.set_viewport_area(area);
    let mut frames = Vec::new();
    let mut parser =
        vt100::Parser::new(/*rows*/ 1, /*cols*/ 8, /*scrollback_len*/ 0);
    for index in 0..2 {
        terminal.backend_mut().output.clear();
        terminal
            .draw(|frame| {
                let buffer = frame.buffer_mut();
                buffer.set_string(0, 0, "中x界", Style::default().bg(Color::Blue));
                buffer[(0, 0)].diff_option = CellDiffOption::Skip;
                buffer[(2, 0)].diff_option = CellDiffOption::Skip;
                buffer[(3, 0)].set_symbol("\x1b]8;;https://example.com\x07界\x1b]8;;\x07");
                buffer[(3, 0)].diff_option =
                    CellDiffOption::ForcedWidth(NonZeroU16::new(/*n*/ 2).expect("wide glyph"));
                frame.set_cursor_position((5, 0));
            })
            .expect("draw");
        parser.process(&terminal.backend().output);
        assert_eq!(parser.screen().cursor_position(), (0, 5));
        assert!(!parser.screen().hide_cursor());
        for column in 0..3 {
            assert_eq!(
                parser
                    .screen()
                    .cell(0, column)
                    .expect("skipped cell")
                    .contents(),
                ""
            );
        }
        assert_eq!(
            parser.screen().cell(0, 3).expect("wide anchor").contents(),
            "界"
        );
        assert_eq!(
            parser.screen().cell(0, 3).expect("wide anchor").bgcolor(),
            match color_mode {
                ColorMode::Enabled => vt100::Color::Idx(4),
                ColorMode::Disabled => vt100::Color::Default,
            }
        );
        let output = terminal.backend().output();
        assert_eq!(output.matches("\x1b[0 q").count(), usize::from(index == 0));
        assert_eq!(
            output
                .matches("\x1b]8;;https://example.com\x07界\x1b]8;;\x07")
                .count(),
            if index == 0 { 2 } else { 0 },
            "both initial draw and cursor repair must preserve the hyperlink"
        );
        frames.push(terminal.backend().output().escape_debug().to_string());
    }
    if color_mode == ColorMode::Enabled {
        assert_snapshot!("cursor_style_owned_wide_frames", frames.join("\n"));
    }
}

#[test]
fn terminal_draw_repairs_single_column_without_scrolling() {
    let Some(color_mode) = isolated_color_mode(
        "custom_terminal::tests::cursor::terminal_draw_repairs_single_column_without_scrolling",
    ) else {
        return;
    };
    let mut terminal =
        Terminal::with_options(CaptureBackend::new(/*width*/ 1, /*height*/ 1)).expect("terminal");
    let area = Rect::new(
        /*x*/ 0, /*y*/ 0, /*width*/ 1, /*height*/ 1,
    );
    terminal.set_viewport_area(area);
    let mut frames = Vec::new();
    let mut parser =
        vt100::Parser::new(/*rows*/ 1, /*cols*/ 1, /*scrollback_len*/ 1);
    for index in 0..3 {
        terminal.backend_mut().output.clear();
        terminal
            .draw(|frame| {
                Paragraph::new("x")
                    .style(Style::default().bg(Color::Blue))
                    .render(area, frame.buffer_mut());
                frame.set_cursor_position((0, 0));
            })
            .expect("draw");
        parser.process(&terminal.backend().output);
        assert_eq!(parser.screen().contents(), "x");
        assert_eq!(parser.screen().cursor_position(), (0, 0));
        assert_eq!(
            terminal.backend().output().matches("\x1b[0 q").count(),
            usize::from(index == 0)
        );
        assert_eq!(
            parser.screen().cell(0, 0).expect("single column").bgcolor(),
            match color_mode {
                ColorMode::Enabled => vt100::Color::Idx(4),
                ColorMode::Disabled => vt100::Color::Default,
            }
        );
        frames.push(terminal.backend().output().escape_debug().to_string());
    }
    parser.screen_mut().set_scrollback(/*rows*/ 1);
    assert_eq!(parser.screen().scrollback(), 0);
    if color_mode == ColorMode::Enabled {
        assert_snapshot!("cursor_style_single_column_frames", frames.join("\n"));
    }
}

#[test]
fn terminal_draw_omits_cursor_style_without_an_owned_glyph() {
    let Some(color_mode) = isolated_color_mode(
        "custom_terminal::tests::cursor::terminal_draw_omits_cursor_style_without_an_owned_glyph",
    ) else {
        return;
    };
    let reset = match color_mode {
        ColorMode::Enabled => "\x1b[39m\x1b[49m\x1b[0m",
        ColorMode::Disabled => "\x1b[0m",
    };
    let mut terminal =
        Terminal::with_options(CaptureBackend::new(/*width*/ 2, /*height*/ 1)).expect("terminal");
    for width in [0, 2] {
        terminal.set_viewport_area(Rect::new(
            /*x*/ 0, /*y*/ 0, width, /*height*/ 1,
        ));
        for buffer in &mut terminal.buffers {
            for cell in &mut buffer.content {
                cell.diff_option = CellDiffOption::Skip;
            }
        }
        terminal.backend_mut().output.clear();
        terminal
            .draw(|frame| {
                for cell in &mut frame.buffer_mut().content {
                    cell.diff_option = CellDiffOption::Skip;
                }
                frame.set_cursor_style(SetCursorStyle::SteadyBar);
                frame.set_cursor_position((1, 0));
            })
            .expect("draw");
        assert_eq!(
            terminal.backend().output(),
            format!("{reset}\x1b[1;2H\x1b[?25h")
        );
    }
    terminal.set_viewport_area(Rect::default());
    terminal.backend_mut().output.clear();
    terminal.draw(|_| {}).expect("hide cursor");
    assert_eq!(terminal.backend().output(), format!("{reset}\x1b[?25l"));
}
