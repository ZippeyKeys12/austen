use anstyle::{AnsiColor, Color, Style};

pub const HEADING: Style = Style::new().bold();
pub const PHASE: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
pub const FINDING: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Red)))
    .bold();
pub const WARN: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
pub const DIM: Style = Style::new().dimmed();
