//! Small adapter around inquire. It owns rendering; the wizard owns the questions.
use anyhow::{Context, Result};
use inquire::{Confirm, InquireError, MultiSelect, Password, PasswordDisplayMode, Select, Text};
use std::{fmt, future::Future, io::Write};
use tokio_util::sync::CancellationToken;

mod picker;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Choice {
    pub id: String,
    pub label: String,
}
impl Choice {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}
impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&clean(&self.label))
    }
}

/// Strip terminal commands and directional/invisible formatting from untrusted labels.
pub fn clean(value: &str) -> String {
    value
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(*c,
        '\u{00ad}' | '\u{061c}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
        })
        .collect()
}

#[derive(Debug)]
pub struct Cancelled;
impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("setup cancelled; no installation requested")
    }
}
impl std::error::Error for Cancelled {}

#[derive(Debug)]
pub struct Back;
impl fmt::Display for Back {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("discard current draft")
    }
}
impl std::error::Error for Back {}

#[derive(Debug)]
pub struct OpenMenu;
impl fmt::Display for OpenMenu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("open discovery menu")
    }
}
impl std::error::Error for OpenMenu {}

/// Scriptable boundary: no discovery, configuration or installer logic here.
pub trait Prompts {
    fn local_cancel(&mut self, _enabled: bool) {}
    fn say(&mut self, message: &str);
    fn emphasize(&mut self, message: &str) {
        self.say(message);
    }
    fn heading(&mut self, message: &str) {
        self.say(message);
    }
    fn text(&mut self, message: &str, default: &str) -> impl Future<Output = Result<String>>;
    fn password(&mut self, saved: bool) -> impl Future<Output = Result<String>>;
    fn confirm(&mut self, message: &str, default: bool) -> impl Future<Output = Result<bool>>;
    fn select(
        &mut self,
        message: &str,
        choices: &[Choice],
        default: usize,
    ) -> impl Future<Output = Result<usize>>;
    fn multi(
        &mut self,
        message: &str,
        choices: &[Choice],
        defaults: &[usize],
    ) -> impl Future<Output = Result<Vec<usize>>>;
    /// Discovery pickers support Ctrl-O without changing ordinary action menus.
    fn pick_one(
        &mut self,
        message: &str,
        choices: &[Choice],
        default: usize,
    ) -> impl Future<Output = Result<usize>> {
        self.select(message, choices, default)
    }
    fn pick_many(
        &mut self,
        message: &str,
        choices: &[Choice],
        defaults: &[usize],
    ) -> impl Future<Output = Result<Vec<usize>>> {
        self.multi(message, choices, defaults)
    }
}

pub struct TerminalPrompts {
    no_color: bool,
    cancel: CancellationToken,
    local_cancel: bool,
}
impl TerminalPrompts {
    pub fn new(no_color: bool, cancel: CancellationToken) -> Self {
        inquire::set_global_render_config(if no_color {
            inquire::ui::RenderConfig::empty()
        } else {
            inquire::ui::RenderConfig::default_colored()
        });
        Self {
            no_color,
            cancel,
            local_cancel: false,
        }
    }

    async fn ask<T: Send + 'static>(
        &self,
        ask: impl FnOnce() -> std::result::Result<T, InquireError> + Send + 'static,
    ) -> Result<T> {
        if self.cancel.is_cancelled() {
            return Err(Cancelled.into());
        }
        // Arm crossterm synchronously, before a worker can be scheduled. Its raw
        // mode is idempotent until disable_raw_mode clears the saved mode.
        let original =
            rustix::termios::tcgetattr(std::io::stdin()).context("terminal mode unavailable")?;
        crossterm::terminal::enable_raw_mode().context("terminal raw mode unavailable")?;
        // Ordinary threads, not spawn_blocking: a blocked terminal must never starve
        // Tokio's signal task or keep its runtime alive on termination.
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let _ = tx.send(ask());
        });
        let result = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => {
                // Serialize all inquire output (including startup's EnablePaste)
                // with cleanup. Restore termios WITHOUT clearing crossterm's saved
                // mode: a worker not yet initialized must see raw mode already
                // armed and cannot enable it again after this restoration. If the
                // worker has already dropped its terminal, it cannot initialize
                // again. Keep this output lock and the saved mode until exit; do
                // not wait for a worker that may be blocked reading input.
                let stderr = std::io::stderr();
                let mut output = stderr.lock();
                let _ = rustix::termios::tcsetattr(std::io::stdin(), rustix::termios::OptionalActions::Now, &original);
                let _ = crossterm::execute!(output, crossterm::event::DisableBracketedPaste, crossterm::cursor::Show);
                let _ = writeln!(output, "\nPullBoard setup cancelled; no files changed.");
                let _ = output.flush();
                std::process::exit(0);
            }
            answer = rx => match answer {
                Ok(Ok(value)) => Ok(value),
                Ok(Err(InquireError::OperationCanceled)) if self.local_cancel => Err(Back.into()),
                Ok(Err(InquireError::Custom(error))) if error.is::<OpenMenu>() => Err(OpenMenu.into()),
                Ok(Err(InquireError::OperationInterrupted | InquireError::OperationCanceled)) => {
                    self.cancel.cancel();
                    Err(Cancelled.into())
                }
                _ => Err(anyhow::anyhow!("terminal prompt failed; no installation requested")),
            }
        };
        // A received answer means inquire has returned and dropped its terminal;
        // only then is it safe to clear the synchronously armed saved mode.
        restore();
        result
    }
}

/// Also used by the panic hook. No alternate screen is ever entered.
pub fn restore() {
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = crossterm::execute!(
        std::io::stderr(),
        crossterm::event::DisableBracketedPaste,
        crossterm::cursor::Show
    );
}

/// Register all launcher/terminal termination signals before asking any question.
pub fn cancellation_signals(cancel: CancellationToken) -> Result<tokio::task::JoinHandle<()>> {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut hup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    Ok(tokio::spawn(async move {
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {}, _ = hup.recv() => {} }
        cancel.cancel();
    }))
}

fn emphasized(message: &str, no_color: bool) -> String {
    let message = clean(message);
    if no_color {
        message
    } else {
        format!("\x1b[1m{message}\x1b[0m")
    }
}

impl Prompts for TerminalPrompts {
    fn local_cancel(&mut self, enabled: bool) {
        self.local_cancel = enabled;
    }
    fn say(&mut self, message: &str) {
        eprintln!("{}", clean(message));
    }
    fn emphasize(&mut self, message: &str) {
        eprintln!("{}", emphasized(message, self.no_color));
    }
    fn heading(&mut self, message: &str) {
        eprintln!("\n{}", emphasized(message, self.no_color));
    }
    async fn text(&mut self, message: &str, default: &str) -> Result<String> {
        let message = clean(message);
        let default = clean(default);
        let help = if self.local_cancel {
            "Esc discard forge draft · Ctrl-C exit setup"
        } else {
            "Ctrl-C exit setup"
        };
        self.ask(move || {
            Text::new(&message)
                .with_default(&default)
                .with_help_message(help)
                .prompt()
        })
        .await
    }
    async fn password(&mut self, saved: bool) -> Result<String> {
        let help = if self.local_cancel {
            "Esc discard forge draft · Ctrl-C exit setup"
        } else {
            "Ctrl-C exit setup"
        };
        self.ask(move || {
            Password::new(if saved {
                "Token (Enter keeps saved token):"
            } else {
                "Token:"
            })
            .without_confirmation()
            .with_display_mode(PasswordDisplayMode::Masked)
            // Never allow accepted-answer scrollback to contain the secret.
            .with_formatter(&|_| "[hidden]".into())
            .with_help_message(help)
            .prompt()
        })
        .await
    }
    async fn confirm(&mut self, message: &str, default: bool) -> Result<bool> {
        let message = clean(message);
        let help = if self.local_cancel {
            "Esc discard forge draft · Ctrl-C exit setup"
        } else {
            "Ctrl-C exit setup"
        };
        self.ask(move || {
            Confirm::new(&message)
                .with_default(default)
                .with_help_message(help)
                .prompt()
        })
        .await
    }
    async fn select(&mut self, message: &str, choices: &[Choice], default: usize) -> Result<usize> {
        let message = clean(message);
        let choices = choices.to_vec();
        let help = if self.local_cancel {
            "↑↓ move · type to filter · Enter select · Esc discard forge draft · Ctrl-C exit setup"
        } else {
            "↑↓ move · type to filter · Enter select · Ctrl-C exit setup"
        };
        self.ask(move || {
            Select::new(&message, choices)
                .with_starting_cursor(default)
                .with_page_size(8)
                .with_help_message(help)
                .raw_prompt()
                .map(|answer| answer.index)
        })
        .await
    }
    async fn multi(
        &mut self,
        message: &str,
        choices: &[Choice],
        defaults: &[usize],
    ) -> Result<Vec<usize>> {
        let message = clean(message);
        let choices = choices.to_vec();
        let defaults = defaults.to_vec();
        self.ask(move || {
            MultiSelect::new(&message, choices)
                .with_default(&defaults)
                .with_page_size(8)
                .with_help_message(
                    "↑↓ move · Space toggle · type to filter · Enter accept · Ctrl-C cancel",
                )
                .raw_prompt()
                .map(|answers| answers.into_iter().map(|a| a.index).collect())
        })
        .await
    }

    async fn pick_one(
        &mut self,
        message: &str,
        choices: &[Choice],
        default: usize,
    ) -> Result<usize> {
        let message = clean(message);
        let choices = choices.to_vec();
        let no_color = self.no_color;
        self.ask(move || picker::prompt(&message, &choices, &[default], false, no_color))
            .await?
            .into_iter()
            .next()
            .context("no choice selected")
    }

    async fn pick_many(
        &mut self,
        message: &str,
        choices: &[Choice],
        defaults: &[usize],
    ) -> Result<Vec<usize>> {
        let message = clean(message);
        let choices = choices.to_vec();
        let defaults = defaults.to_vec();
        let no_color = self.no_color;
        self.ask(move || picker::prompt(&message, &choices, &defaults, true, no_color))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emphasis_sanitizes_before_adding_bold_styling() {
        let message = "https://example.test/\x1b[0m\n\u{202e}path";
        assert_eq!(
            emphasized(message, false),
            "\x1b[1mhttps://example.test/[0mpath\x1b[0m"
        );
    }

    #[test]
    fn no_color_emphasis_is_plain_and_sanitized() {
        let message = "https://example.test/\x1b[1m\r\u{200b}path";
        let rendered = emphasized(message, true);
        assert_eq!(rendered, "https://example.test/[1mpath");
        assert!(!rendered.contains('\x1b'));
    }
}
