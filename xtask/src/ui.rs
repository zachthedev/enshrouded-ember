//! Terminal output.
//!
//! Every line a command prints goes through here, so the color policy and the
//! column math live in one place. Color is an accent: a run with `--quiet`, a
//! run with `NO_COLOR` set, and a run whose stdout is not a terminal all still
//! carry the glyph and the words.

use std::io::{IsTerminal, Write};

use comfy_table::{Cell, CellAlignment, ContentArrangement, Table, presets};
use owo_colors::OwoColorize;

/// The glyph a result row leads with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// The step passed.
    Ok,
    /// The step failed.
    Fail,
    /// The step needs attention without having failed.
    Warn,
    /// The row reports a fact rather than a result.
    Note,
    /// A diff row for something that appeared.
    Added,
    /// A diff row for something that went away.
    Removed,
    /// A diff row for something that moved or changed shape.
    Changed,
}

impl Mark {
    /// The single character this mark prints as.
    fn glyph(self) -> &'static str {
        match self {
            Mark::Ok => "\u{2713}",
            Mark::Fail => "\u{2717}",
            Mark::Warn => "!",
            Mark::Note => "\u{b7}",
            Mark::Added => "+",
            Mark::Removed => "-",
            Mark::Changed => "~",
        }
    }
}

/// One row of a result block.
pub struct Row {
    mark: Mark,
    name: String,
    value: String,
    note: String,
}

impl Row {
    /// A row carrying a mark, a name and a value.
    pub fn new(mark: Mark, name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            mark,
            name: name.into(),
            value: value.into(),
            note: String::new(),
        }
    }

    /// Add the trailing explanation column.
    #[must_use]
    pub fn note(mut self, note: impl Into<String>) -> Self {
        self.note = note.into();
        self
    }
}

/// The output sink, holding the color decision for the whole run.
pub struct Ui {
    color: bool,
    quiet: bool,
}

impl Ui {
    /// Decide once whether this run emits color.
    pub fn new(quiet: bool) -> Self {
        let color =
            !quiet && std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal();
        Self { color, quiet }
    }

    /// Whether this run suppresses everything but failures.
    pub fn quiet(&self) -> bool {
        self.quiet
    }

    /// A blank line and one lowercase label opening a block.
    pub fn section(&self, label: &str) {
        if self.quiet {
            return;
        }
        if self.color {
            println!("\n{}", label.dimmed());
        } else {
            println!("\n{label}");
        }
    }

    /// A line of plain prose, indented into the current block.
    pub fn line(&self, text: &str) {
        if self.quiet {
            return;
        }
        println!("  {text}");
    }

    /// A line of prose printed whether or not the run is quiet.
    #[expect(
        clippy::unused_self,
        reason = "every printed line goes through the sink, so the indent has one home"
    )]
    pub fn always(&self, text: &str) {
        println!("  {text}");
    }

    /// One result row outside a block, for a command with a single outcome.
    pub fn row(&self, mark: Mark, name: &str, value: &str) {
        self.rows(std::slice::from_ref(&Row::new(mark, name, value)));
    }

    /// A block of result rows, aligned as one table.
    pub fn rows(&self, rows: &[Row]) {
        if rows.is_empty() || self.quiet {
            return;
        }
        let mut table = Table::new();
        table
            .load_style(presets::NOTHING)
            .set_content_arrangement(ContentArrangement::Disabled);
        for row in rows {
            table.add_row(vec![
                Cell::new(self.paint(row.mark, row.mark.glyph())),
                Cell::new(&row.name),
                Cell::new(&row.value),
                Cell::new(&row.note),
            ]);
        }
        if let Some(column) = table.column_mut(2) {
            column.set_cell_alignment(CellAlignment::Right);
        }
        for line in table.lines() {
            println!("  {}", line.trim_end());
        }
    }

    /// The dim rule that closes a block. One per run at most.
    pub fn divider(&self, width: usize) {
        if self.quiet {
            return;
        }
        let rule = "\u{2500}".repeat(width.clamp(8, 72));
        if self.color {
            println!("  {}", rule.dimmed());
        } else {
            println!("  {rule}");
        }
    }

    /// The closing summary line of a block.
    pub fn summary(&self, text: &str, failed: bool) {
        if self.quiet && !failed {
            return;
        }
        if self.color && failed {
            println!("  {}", text.red());
        } else {
            println!("  {text}");
        }
    }

    /// A failure that stopped the command, printed to stderr.
    pub fn error(&self, text: &str) {
        let label = "error";
        let mut err = std::io::stderr();
        let _ = if self.color {
            writeln!(err, "{}: {text}", label.red())
        } else {
            writeln!(err, "{label}: {text}")
        };
    }

    /// Apply the mark's accent to a string, or leave it plain.
    fn paint(&self, mark: Mark, text: &str) -> String {
        if !self.color {
            return text.to_string();
        }
        match mark {
            Mark::Ok | Mark::Added => text.green().to_string(),
            Mark::Fail | Mark::Removed => text.red().to_string(),
            Mark::Warn | Mark::Changed => text.yellow().to_string(),
            Mark::Note => text.dimmed().to_string(),
        }
    }
}

/// Render a byte count the way a person reads it.
pub fn bytes(count: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    #[expect(
        clippy::cast_precision_loss,
        reason = "a display string does not need every bit of a byte count"
    )]
    let mut value = count as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{count} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::bytes;

    #[test]
    fn byte_counts_carry_one_decimal_past_a_kilobyte() {
        let cases = [
            (0_u64, "0 B"),
            (999, "999 B"),
            (1000, "1.0 kB"),
            (22_557_696, "22.6 MB"),
            (9_016_000_000, "9.0 GB"),
        ];
        for (input, want) in cases {
            assert_eq!(bytes(input), want, "input {input}");
        }
    }
}
