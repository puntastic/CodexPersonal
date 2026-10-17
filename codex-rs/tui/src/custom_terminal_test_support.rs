//! Inspect the completed frame without adding test helpers to the terminal implementation.

use std::io;
use std::io::Write;

use ratatui::backend::Backend;
use ratatui::buffer::Buffer;
use ratatui::layout::Size;

use super::Terminal;

/// Resize the simulated backend as well as the terminal's cached geometry.
pub(crate) fn set_screen_size<B>(terminal: &mut Terminal<B>, size: Size) -> io::Result<()>
where
    B: Backend<Error = io::Error> + Write,
{
    assert!(
        terminal.screen_size_override.is_some(),
        "expected a fake terminal"
    );
    terminal.screen_size_override = Some(size);
    terminal.resize(size)
}

pub(crate) fn last_rendered_buffer<B>(terminal: &Terminal<B>) -> &Buffer
where
    B: Backend<Error = io::Error> + Write,
{
    terminal.previous_buffer()
}
