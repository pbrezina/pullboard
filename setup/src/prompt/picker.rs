//! Inline discovery picker. Inquire does not expose custom key bindings; keep
//! ordinary prompts there, and own only the Ctrl-O-enabled inventory lists here.
use super::{Choice, OpenMenu, clean};
use crossterm::{
    cursor::{MoveToColumn, MoveUp},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    terminal::{self, Clear, ClearType},
};
use inquire::{InquireError, error::InquireResult};
use std::{collections::BTreeSet, io::Write};
use unicode_width::UnicodeWidthChar;

struct Picker {
    labels: Vec<String>,
    visible: Vec<usize>,
    checked: BTreeSet<usize>,
    cursor: usize,
    filter: String,
    multi: bool,
}
impl Picker {
    fn new(choices: &[Choice], defaults: &[usize], multi: bool) -> Self {
        Self {
            labels: choices.iter().map(|c| clean(&c.label)).collect(),
            visible: (0..choices.len()).collect(),
            checked: defaults
                .iter()
                .copied()
                .filter(|i| *i < choices.len())
                .collect(),
            cursor: if multi {
                0
            } else {
                defaults
                    .first()
                    .copied()
                    .unwrap_or(0)
                    .min(choices.len().saturating_sub(1))
            },
            filter: String::new(),
            multi,
        }
    }
    fn filter(&mut self) {
        let query = self.filter.to_lowercase();
        self.visible = self
            .labels
            .iter()
            .enumerate()
            .filter(|(_, label)| label.to_lowercase().contains(&query))
            .map(|(i, _)| i)
            .collect();
        self.cursor = 0;
    }
    fn toggle(&mut self) {
        if let Some(&i) = self.visible.get(self.cursor)
            && !self.checked.remove(&i)
        {
            self.checked.insert(i);
        }
    }
    fn answer(&self) -> Option<Vec<usize>> {
        if self.multi {
            (!self.checked.is_empty()).then(|| self.checked.iter().copied().collect())
        } else {
            self.visible.get(self.cursor).map(|&i| vec![i])
        }
    }

    fn frame(&self, message: &str, width: u16, page_size: usize, no_color: bool) -> Vec<String> {
        let start = self.cursor / page_size * page_size;
        let mut frame = vec![render_line(
            &format!("? {message} {}", self.filter),
            width,
            Style::Bold,
            no_color,
        )];
        for (at, &i) in self.visible.iter().enumerate().skip(start).take(page_size) {
            let checked = self.multi && self.checked.contains(&i);
            let mark = if self.multi {
                if checked { "[x] " } else { "[ ] " }
            } else {
                ""
            };
            let focused = at == self.cursor;
            frame.push(render_line(
                &format!(
                    "{} {mark}{}",
                    if focused { ">" } else { " " },
                    self.labels[i]
                ),
                width,
                if focused || checked {
                    Style::Cyan
                } else {
                    Style::Plain
                },
                no_color,
            ));
        }
        if self.visible.is_empty() {
            frame.push(render_line(
                "  No matches; edit filter or press Ctrl-O for other discovery methods.",
                width,
                Style::Plain,
                no_color,
            ));
        }
        let help = if self.multi {
            format!(
                "Ctrl-O More options · Enter accept · Space toggle · ↑↓ move · type to filter · {} selected · Ctrl-C exit",
                self.checked.len()
            )
        } else {
            "Ctrl-O More options · Enter select · ↑↓ move · type to filter · Ctrl-C exit".into()
        };
        frame.push(render_line(&help, width, Style::Blue, no_color));
        frame
    }
}

enum Style {
    Plain,
    Bold,
    Cyan,
    Blue,
}

fn render_line(text: &str, width: u16, style: Style, no_color: bool) -> String {
    // Clip sanitized visible text, never the escape sequences we add ourselves.
    let text = clipped(&clean(text), width);
    if no_color {
        return text;
    }
    let code = match style {
        Style::Plain => return text,
        Style::Bold => "1",
        Style::Cyan => "36",
        Style::Blue => "34",
    };
    format!("\x1b[{code}m{text}\x1b[0m")
}

fn clipped(text: &str, columns: u16) -> String {
    let mut width = 0;
    text.chars()
        .take_while(|c| {
            width += c.width().unwrap_or(0);
            width < usize::from(columns)
        })
        .collect()
}

pub(super) fn prompt(
    message: &str,
    choices: &[Choice],
    defaults: &[usize],
    multi: bool,
    no_color: bool,
) -> InquireResult<Vec<usize>> {
    if choices.is_empty() {
        return Err(InquireError::InvalidConfiguration(
            "no discovered choices".into(),
        ));
    }
    let mut picker = Picker::new(choices, defaults, multi);
    let mut lines = 0u16;
    crossterm::execute!(std::io::stderr(), event::EnableBracketedPaste)?;
    loop {
        let (width, height) = terminal::size()?;
        let page_size = usize::from(height.saturating_sub(4).clamp(1, 8));
        let frame = picker.frame(message, width, page_size, no_color);
        {
            let stderr = std::io::stderr();
            let mut output = stderr.lock();
            if lines > 0 {
                crossterm::queue!(
                    output,
                    MoveUp(lines),
                    MoveToColumn(0),
                    Clear(ClearType::FromCursorDown)
                )?;
            }
            for line in &frame {
                write!(output, "{line}\r\n")?;
            }
            output.flush()?;
            lines = u16::try_from(frame.len()).unwrap_or(u16::MAX);
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match (key.code, key.modifiers) {
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    return Err(InquireError::OperationInterrupted);
                }
                (KeyCode::Char('o'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => {
                    return Err(InquireError::Custom(Box::new(OpenMenu)));
                }
                (KeyCode::Enter, _) => {
                    if let Some(answer) = picker.answer() {
                        return Ok(answer);
                    }
                }
                (KeyCode::Up, _) => {
                    picker.cursor = if picker.cursor == 0 {
                        picker.visible.len().saturating_sub(1)
                    } else {
                        picker.cursor - 1
                    }
                }
                (KeyCode::Down, _) => {
                    picker.cursor = if picker.visible.is_empty() {
                        0
                    } else {
                        (picker.cursor + 1) % picker.visible.len()
                    }
                }
                (KeyCode::PageUp, _) => picker.cursor = picker.cursor.saturating_sub(page_size),
                (KeyCode::PageDown, _) => {
                    picker.cursor = picker
                        .cursor
                        .saturating_add(page_size)
                        .min(picker.visible.len().saturating_sub(1))
                }
                (KeyCode::Home, _) => picker.cursor = 0,
                (KeyCode::End, _) => picker.cursor = picker.visible.len().saturating_sub(1),
                (KeyCode::Char(' '), KeyModifiers::NONE) if multi => picker.toggle(),
                (KeyCode::Backspace, _) => {
                    picker.filter.pop();
                    picker.filter();
                }
                (KeyCode::Char(c), modifiers)
                    if !c.is_control()
                        && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    picker.filter.push(c);
                    picker.filter();
                }
                _ => {}
            },
            Event::Paste(text) => {
                picker.filter.push_str(&clean(&text));
                picker.filter();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtered_toggles_keep_original_indices_and_hidden_selections() {
        let choices = [Choice::new("a", "Alpha"), Choice::new("b", "Beta")];
        let mut picker = Picker::new(&choices, &[0], true);
        picker.filter = "BETA".into();
        picker.filter();
        picker.toggle();
        assert_eq!(picker.answer(), Some(vec![0, 1]));
        picker.toggle();
        assert_eq!(picker.answer(), Some(vec![0]));
        picker.filter.clear();
        picker.filter();
        picker.toggle();
        assert_eq!(picker.answer(), None);
    }

    #[test]
    fn single_picker_preserves_default_and_clips_wide_terminal_labels() {
        let choices = [Choice::new("a", "Alpha"), Choice::new("b", "Beta")];
        let picker = Picker::new(&choices, &[1], false);
        assert_eq!(picker.answer(), Some(vec![1]));
        assert_eq!(clipped("ab界cd", 5), "ab界");
    }

    #[test]
    fn colored_frame_has_cyan_focus_and_selection_with_blue_keyboard_help() {
        let choices = [
            Choice::new("a", "Alpha"),
            Choice::new("b", "Beta"),
            Choice::new("c", "Gamma"),
        ];
        let picker = Picker::new(&choices, &[1], true);
        let frame = picker.frame("Projects", 200, 8, false);

        assert_eq!(frame[0], "\x1b[1m? Projects \x1b[0m");
        assert_eq!(frame[1], "\x1b[36m> [ ] Alpha\x1b[0m");
        assert_eq!(frame[2], "\x1b[36m  [x] Beta\x1b[0m");
        assert_eq!(frame[3], "  [ ] Gamma");
        assert_eq!(
            frame[4],
            "\x1b[34mCtrl-O More options · Enter accept · Space toggle · ↑↓ move · type to filter · 1 selected · Ctrl-C exit\x1b[0m"
        );
    }

    #[test]
    fn colored_single_picker_keeps_focus_on_the_visible_page() {
        let choices = [Choice::new("a", "Alpha"), Choice::new("b", "Beta")];
        let picker = Picker::new(&choices, &[1], false);
        let frame = picker.frame("Projects", 200, 1, false);

        assert_eq!(frame.len(), 3);
        assert_eq!(frame[1], "\x1b[36m> Beta\x1b[0m");
        assert_eq!(
            frame[2],
            "\x1b[34mCtrl-O More options · Enter select · ↑↓ move · type to filter · Ctrl-C exit\x1b[0m"
        );
        assert_eq!(picker.answer(), Some(vec![1]));
    }

    #[test]
    fn no_color_frames_are_sgr_free_and_sanitize_all_untrusted_text() {
        let choices = [Choice::new("a", "Al\x1b[31m\u{202e}pha")];
        for multi in [false, true] {
            let mut picker = Picker::new(&choices, &[0], multi);
            picker.filter = "fi\x1b[1m\n\u{200b}lter".into();
            let frame = picker.frame("Pro\x1b[0m\rjects", 200, 8, true);

            assert_eq!(frame[0], "? Pro[0mjects fi[1mlter");
            assert!(frame[1].ends_with("Al[31mpha"));
            assert!(frame.last().unwrap().starts_with("Ctrl-O More options · "));
            assert!(frame.iter().all(|line| !line.chars().any(char::is_control)));

            picker.filter();
            let empty_frame = picker.frame("Projects", 200, 8, true);
            assert!(empty_frame[1].starts_with("  No matches;"));
            assert!(
                empty_frame
                    .iter()
                    .all(|line| !line.chars().any(char::is_control))
            );
        }
    }

    #[test]
    fn color_is_added_only_after_sanitizing_and_clipping_visible_columns() {
        assert_eq!(
            render_line("ab\x1b\n\u{202e}界cd", 5, Style::Cyan, false),
            "\x1b[36mab界\x1b[0m"
        );
    }
}
