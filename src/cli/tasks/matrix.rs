//! Human-only Cartesian view of the existing task snapshot.
use std::io::Write;

use chrono::NaiveDate;
use hs::{
    Result,
    domain::tasks::{Quadrant, Task},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::Metrics;

const URGENCY_AXIS: &str = "--> URGENCY";
const GRID_MIN_COLUMNS: usize = 60;

struct Section {
    quadrant: Quadrant,
    heading: &'static str,
    meaning: &'static str,
}

// Positive x is urgency; positive y is importance, like Cartesian axes.
const SECTIONS: [Section; 4] = [
    Section {
        quadrant: Quadrant::Plan,
        heading: "II PLAN",
        meaning: "important / not urgent",
    },
    Section {
        quadrant: Quadrant::Do,
        heading: "I DO",
        meaning: "important / urgent",
    },
    Section {
        quadrant: Quadrant::Drop,
        heading: "III DROP",
        meaning: "not important / not urgent",
    },
    Section {
        quadrant: Quadrant::Delegate,
        heading: "IV DELEGATE",
        meaning: "not important / urgent",
    },
];

pub(super) fn render(
    out: &mut impl Write,
    tasks: &[Task],
    metrics: &Metrics<'_>,
    today: NaiveDate,
    columns: usize,
) -> Result<()> {
    let columns = columns.max(1);
    if columns >= GRID_MIN_COLUMNS {
        // Include both borders, cell padding and the horizontal axis label.
        let cell = (columns - 3 - URGENCY_AXIS.len()) / 2;
        let content = cell - 2;
        let sections: Vec<_> = SECTIONS
            .iter()
            .map(|section| section_lines(section, tasks, metrics, today, content))
            .collect();
        axes_top(out, cell + 1)?;
        let border = format!("+{}+{}+", "-".repeat(cell), "-".repeat(cell));
        writeln!(out, "{border}")?;
        pair(out, &sections[0], &sections[1], content)?;
        writeln!(out, "{border}{URGENCY_AXIS}")?;
        pair(out, &sections[2], &sections[3], content)?;
        writeln!(out, "{border}")?;
    } else {
        // Keep the spatial key in small panes, then give each task the full
        // available width instead of squeezing its title into a tiny cell.
        if columns >= 24 {
            axes_top(out, 7)?;
            writeln!(
                out,
                "+------+------+\n|  II  |  I   |\n+------+------+> URGENCY\n| III  |  IV  |\n+------+------+"
            )?;
        } else {
            lines(out, &wrap("Importance: up", columns))?;
            lines(out, &wrap("Urgency: right", columns))?;
        }
        for section in &SECTIONS {
            writeln!(out)?;
            lines(out, &section_lines(section, tasks, metrics, today, columns))?;
        }
    }
    writeln!(out)?;
    let inbox: Vec<_> = tasks
        .iter()
        .filter(|task| task.quadrant.is_none())
        .collect();
    lines(
        out,
        &wrap(&format!("INBOX ({}) - unclassified", inbox.len()), columns),
    )?;
    lines(out, &task_lines(&inbox, metrics, today, columns))?;
    Ok(())
}

fn axes_top(out: &mut impl Write, center: usize) -> Result<()> {
    writeln!(out, "{}IMPORTANCE", " ".repeat(center.saturating_sub(5)))?;
    writeln!(out, "{}^", " ".repeat(center))?;
    Ok(())
}

fn section_lines(
    section: &Section,
    tasks: &[Task],
    metrics: &Metrics<'_>,
    today: NaiveDate,
    width: usize,
) -> Vec<String> {
    let tasks: Vec<_> = tasks
        .iter()
        .filter(|task| task.quadrant == Some(section.quadrant))
        .collect();
    let mut result = wrap(&format!("{} ({})", section.heading, tasks.len()), width);
    result.extend(wrap(section.meaning, width));
    result.push(String::new());
    result.extend(task_lines(&tasks, metrics, today, width));
    result
}

fn task_lines(
    tasks: &[&Task],
    metrics: &Metrics<'_>,
    today: NaiveDate,
    width: usize,
) -> Vec<String> {
    if tasks.is_empty() {
        return wrap("(empty)", width);
    }
    let mut result = Vec::new();
    for (index, task) in tasks.iter().enumerate() {
        if index > 0 {
            result.push(String::new());
        }
        result.extend(wrap(
            &format!("#{} {}", task.id, crate::cli::visible(&task.title)),
            width,
        ));
        if let Some(due) = task.due_date {
            detail(
                &mut result,
                &format!("due {due}{}", if due < today { " overdue" } else { "" }),
                width,
            );
        }
        let seconds = metrics.seconds.get(&task.id).copied().unwrap_or(0);
        if seconds > 0 {
            detail(
                &mut result,
                &format!("tracked {}", crate::cli::duration_seconds(seconds)),
                width,
            );
        }
        if let Some(timer) = metrics.timers.get(&task.id) {
            detail(&mut result, &format!("timer: {timer}"), width);
        }
    }
    result
}

fn detail(lines: &mut Vec<String>, text: &str, width: usize) {
    let indent = if width > 4 { 2 } else { 0 };
    lines.extend(
        wrap(text, width - indent)
            .into_iter()
            .map(|line| format!("{}{line}", " ".repeat(indent))),
    );
}

fn pair(out: &mut impl Write, left: &[String], right: &[String], width: usize) -> Result<()> {
    for index in 0..left.len().max(right.len()) {
        let left = left.get(index).map(String::as_str).unwrap_or("");
        let right = right.get(index).map(String::as_str).unwrap_or("");
        writeln!(
            out,
            "| {}{} | {}{} |",
            left,
            padding(left, width),
            right,
            padding(right, width)
        )?;
    }
    Ok(())
}

fn padding(text: &str, width: usize) -> String {
    " ".repeat(width.saturating_sub(UnicodeWidthStr::width(text)))
}

fn lines(out: &mut impl Write, lines: &[String]) -> Result<()> {
    for line in lines {
        writeln!(out, "{line}")?;
    }
    Ok(())
}

/// Wrap at word boundaries and split long words only between graphemes. No
/// truncation: task IDs, complete titles, and metadata all remain available.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut result = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for word in text.split_whitespace() {
        let word_width = UnicodeWidthStr::width(word);
        if word_width <= width {
            let space = usize::from(!line.is_empty());
            if used + space + word_width > width {
                result.push(std::mem::take(&mut line));
                used = 0;
            }
            if !line.is_empty() {
                line.push(' ');
                used += 1;
            }
            line.push_str(word);
            used += word_width;
        } else {
            if !line.is_empty() {
                result.push(std::mem::take(&mut line));
                used = 0;
            }
            for grapheme in word.graphemes(true) {
                // A two-column character cannot fit a one-column terminal.
                // Show its code points as ASCII instead of losing the text.
                let escaped;
                let units: Vec<&str> = if UnicodeWidthStr::width(grapheme) > width {
                    escaped = grapheme
                        .chars()
                        .flat_map(char::escape_unicode)
                        .collect::<String>();
                    escaped.graphemes(true).collect()
                } else {
                    vec![grapheme]
                };
                for unit in units {
                    let size = UnicodeWidthStr::width(unit);
                    if used + size > width {
                        result.push(std::mem::take(&mut line));
                        used = 0;
                    }
                    line.push_str(unit);
                    used += size;
                }
            }
        }
    }
    if !line.is_empty() {
        result.push(line);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_preserves_graphemes_and_all_long_word_content() {
        let text = "Cafe\u{301} 你好 👩‍💻 extraordinarilylongword";
        let lines = wrap(text, 8);
        assert!(
            lines
                .iter()
                .all(|line| UnicodeWidthStr::width(line.as_str()) <= 8)
        );
        assert_eq!(lines.join("").replace(' ', ""), text.replace(' ', ""));
        assert!(lines.iter().any(|line| line.contains("e\u{301}")));
        assert!(lines.iter().any(|line| line.contains("👩‍💻")));
        assert_eq!(wrap("你", 1).join(""), "\\u{4f60}");
    }
}
