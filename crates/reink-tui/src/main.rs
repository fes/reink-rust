#![forbid(unsafe_code)]

use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use reink_core::{ModelDatabase, PrinterIdentity};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
    }
}

fn run() -> Result<(), String> {
    let database = ModelDatabase::builtin().map_err(|error| error.to_string())?;
    let models = database.models().map(str::to_owned).collect();
    let mut application = Application::new(models);
    let mut terminal = TerminalSession::enter()?;

    let result = (|| -> Result<(), String> {
        loop {
            terminal
                .draw(|frame| application.draw(frame, &database))
                .map_err(|error| error.to_string())?;
            if !event::poll(Duration::from_millis(250)).map_err(|error| error.to_string())? {
                continue;
            }
            let Event::Key(key) = event::read().map_err(|error| error.to_string())? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if application.handle_key(key) == Navigation::Quit {
                return Ok(());
            }
        }
    })();

    let restore = terminal.restore();
    match (result, restore) {
        (Err(error), _) => Err(error),
        (Ok(()), result) => result,
    }
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restored: bool,
}

impl TerminalSession {
    fn enter() -> Result<Self, String> {
        enable_raw_mode().map_err(|error| format!("enable terminal raw mode: {error}"))?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(format!("enter terminal screen: {error}"));
        }
        let backend = CrosstermBackend::new(stdout);
        let terminal =
            Terminal::new(backend).map_err(|error| format!("initialize terminal UI: {error}"))?;
        Ok(Self {
            terminal,
            restored: false,
        })
    }

    fn draw(&mut self, render: impl FnOnce(&mut ratatui::Frame<'_>)) -> Result<(), io::Error> {
        self.terminal.draw(render).map(|_| ())
    }

    fn restore(&mut self) -> Result<(), String> {
        if self.restored {
            return Ok(());
        }
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)
            .map_err(|error| format!("leave terminal screen: {error}"))?;
        disable_raw_mode().map_err(|error| format!("disable terminal raw mode: {error}"))?;
        self.terminal
            .show_cursor()
            .map_err(|error| format!("restore terminal cursor: {error}"))?;
        self.restored = true;
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if !self.restored {
            let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
            let _ = disable_raw_mode();
            let _ = self.terminal.show_cursor();
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum View {
    Home,
    Models,
    ModelDetail,
    IdentityInspection,
    ReadOnlyWorkflows,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Navigation {
    Continue,
    Quit,
}

struct Application {
    view: View,
    models: Vec<String>,
    selected_model: usize,
    identity_input: String,
}

impl Application {
    fn new(models: Vec<String>) -> Self {
        Self {
            view: View::Home,
            models,
            selected_model: 0,
            identity_input: String::new(),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Navigation {
        if matches!(
            key.code,
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc
        ) && self.view == View::Home
        {
            return Navigation::Quit;
        }
        match self.view {
            View::Home if matches!(key.code, KeyCode::Enter | KeyCode::Char('m')) => {
                self.view = View::Models;
            }
            View::Home if matches!(key.code, KeyCode::Char('i') | KeyCode::Char('I')) => {
                self.view = View::IdentityInspection;
            }
            View::Home if matches!(key.code, KeyCode::Char('h') | KeyCode::Char('H')) => {
                self.view = View::ReadOnlyWorkflows;
            }
            View::Models => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
                KeyCode::Enter => self.view = View::ModelDetail,
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => self.view = View::Home,
                _ => {}
            },
            View::ModelDetail
                if matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q')
                ) =>
            {
                self.view = View::Models;
            }
            View::IdentityInspection => match key.code {
                KeyCode::Esc => self.view = View::Home,
                KeyCode::Backspace => {
                    self.identity_input.pop();
                }
                KeyCode::Char(character) if self.identity_input.len() < 1024 => {
                    self.identity_input.push(character);
                }
                _ => {}
            },
            View::ReadOnlyWorkflows
                if matches!(
                    key.code,
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q')
                ) =>
            {
                self.view = View::Home;
            }
            _ => {}
        }
        Navigation::Continue
    }

    fn move_selection(&mut self, delta: isize) {
        if self.models.is_empty() {
            return;
        }
        self.selected_model = self
            .selected_model
            .saturating_add_signed(delta)
            .min(self.models.len() - 1);
    }

    fn draw(&self, frame: &mut ratatui::Frame<'_>, database: &ModelDatabase) {
        match self.view {
            View::Home => self.draw_home(frame),
            View::Models => self.draw_models(frame),
            View::ModelDetail => self.draw_model_detail(frame, database),
            View::IdentityInspection => self.draw_identity_inspection(frame, database),
            View::ReadOnlyWorkflows => self.draw_read_only_workflows(frame),
        }
    }

    fn draw_home(&self, frame: &mut ratatui::Frame<'_>) {
        let area = centered_area(frame.area(), 64, 14);
        let text = Text::from(vec![
            Line::from("ReInk").style(Style::default().add_modifier(Modifier::BOLD)),
            Line::from("Read-only printer inspection"),
            Line::from(""),
            Line::from("Browse the built-in Epson model database."),
            Line::from("No device is opened and no printer state can be changed."),
            Line::from(""),
            Line::from("Enter/M: models  I: inspect ID  H: read-only workflows  Q/Esc: quit"),
        ]);
        frame.render_widget(
            Paragraph::new(text)
                .block(Block::default().borders(Borders::ALL).title(" ReInk "))
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            area,
        );
    }

    fn draw_models(&self, frame: &mut ratatui::Frame<'_>) {
        let items = self
            .models
            .iter()
            .map(|model| ListItem::new(model.as_str()))
            .collect::<Vec<_>>();
        let mut state = ListState::default();
        state.select((!self.models.is_empty()).then_some(self.selected_model));
        frame.render_stateful_widget(
            List::new(items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Epson models (Enter: details, Esc: back) "),
                )
                .highlight_style(
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("> "),
            frame.area(),
            &mut state,
        );
    }

    fn draw_model_detail(&self, frame: &mut ratatui::Frame<'_>, database: &ModelDatabase) {
        let Some(name) = self.models.get(self.selected_model) else {
            return;
        };
        let Some(spec) = database.get(name) else {
            return;
        };
        let mut lines = vec![
            Line::from(format!("Model: {}", spec.model)),
            Line::from(format!("Read key: {:04X}", spec.read_key)),
            Line::from(format!(
                "Address widths: read={} write={}",
                spec.read_address_width.byte_len(),
                spec.write_address_width.byte_len()
            )),
            Line::from(format!(
                "Memory range: {:04X}-{:04X}",
                spec.memory_low, spec.memory_high
            )),
            Line::from(""),
            Line::from("Configured operations:"),
        ];
        lines.extend(
            spec.memory_operations
                .iter()
                .map(|operation| Line::from(format!("- {}", operation.description))),
        );
        lines.push(Line::from(""));
        lines.push(Line::from("Esc or Q: back to models"));
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Model details "),
                )
                .wrap(Wrap { trim: true }),
            frame.area(),
        );
    }

    fn draw_identity_inspection(&self, frame: &mut ratatui::Frame<'_>, database: &ModelDatabase) {
        let lines = identity_inspection_lines(&self.identity_input, database)
            .into_iter()
            .map(Line::from)
            .collect::<Vec<_>>();
        frame.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Local IEEE 1284 identity inspection "),
                )
                .wrap(Wrap { trim: true }),
            frame.area(),
        );
    }

    fn draw_read_only_workflows(&self, frame: &mut ratatui::Frame<'_>) {
        let text = Text::from(vec![
            Line::from("CLI discovery and hardware validation are separate opt-in commands.")
                .style(Style::default().add_modifier(Modifier::BOLD)),
            Line::from(""),
            Line::from("Network discovery: reink discover --timeout-seconds 3"),
            Line::from("Linux device candidates: reink local-devices"),
            Line::from(
                "Both commands are read-only; device-file discovery does not open a device.",
            ),
            Line::from(""),
            Line::from("Linux hardware preflight: reink-hardware-test read-sequence"),
            Line::from(
                "It requires an exact vendor/product/interface selection and may communicate",
            ),
            Line::from("only with the explicitly selected printer. It never writes or resets."),
            Line::from(""),
            Line::from("This UI does not run discovery or hardware commands."),
            Line::from("Esc or Q: back"),
        ]);
        frame.render_widget(
            Paragraph::new(text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Read-only workflows "),
                )
                .wrap(Wrap { trim: true }),
            frame.area(),
        );
    }
}

fn identity_inspection_lines(input: &str, database: &ModelDatabase) -> Vec<String> {
    let mut lines = vec![
        "Type an IEEE 1284 device ID to inspect local model metadata.".to_owned(),
        "No device is opened and no printer state can be changed.".to_owned(),
        format!("Input: {input}"),
        String::new(),
    ];
    if input.is_empty() {
        lines.push("Example: MFG:EPSON;MDL:C90;".to_owned());
        lines.push("Esc: back".to_owned());
        return lines;
    }

    match PrinterIdentity::parse(input) {
        Ok(identity) => {
            lines.push("Parsed fields:".to_owned());
            lines.extend(
                identity
                    .fields()
                    .iter()
                    .map(|(name, value)| format!("{name}: {value}")),
            );
            lines.push(format!(
                "Detected model: {}",
                identity.detected_model().unwrap_or("unavailable")
            ));
            lines.push(format!(
                "Built-in match: {}",
                database
                    .resolve_identity(&identity)
                    .map(|spec| spec.model.as_str())
                    .unwrap_or("no built-in match")
            ));
        }
        Err(error) => lines.push(format!("Device ID parse error: {error}")),
    }
    lines.push(String::new());
    lines.push("Esc: back     Backspace: edit".to_owned());
    lines
}

fn centered_area(area: ratatui::layout::Rect, width: u16, height: u16) -> ratatui::layout::Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(height.min(area.height)),
            Constraint::Fill(1),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Fill(1),
            Constraint::Length(width.min(area.width)),
            Constraint::Fill(1),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use reink_core::ModelDatabase;

    use super::{Application, Navigation, View, identity_inspection_lines};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn model_browser_navigates_without_exposing_device_actions() {
        let mut application = Application::new(vec!["A".to_owned(), "B".to_owned()]);

        assert_eq!(
            application.handle_key(key(KeyCode::Enter)),
            Navigation::Continue
        );
        assert_eq!(application.view, View::Models);
        assert_eq!(
            application.handle_key(key(KeyCode::Down)),
            Navigation::Continue
        );
        assert_eq!(application.selected_model, 1);
        assert_eq!(
            application.handle_key(key(KeyCode::Enter)),
            Navigation::Continue
        );
        assert_eq!(application.view, View::ModelDetail);
        assert_eq!(
            application.handle_key(key(KeyCode::Esc)),
            Navigation::Continue
        );
        assert_eq!(application.view, View::Models);
        assert_eq!(
            application.handle_key(key(KeyCode::Esc)),
            Navigation::Continue
        );
        assert_eq!(application.view, View::Home);
        assert_eq!(
            application.handle_key(key(KeyCode::Char('q'))),
            Navigation::Quit
        );
    }

    #[test]
    fn identity_inspection_is_local_and_resolves_model_metadata() {
        let database = ModelDatabase::builtin().unwrap();
        let mut application = Application::new(vec!["C90".to_owned()]);

        application.handle_key(key(KeyCode::Char('i')));
        assert_eq!(application.view, View::IdentityInspection);
        for character in "MFG:EPSON;MDL:C90;".chars() {
            application.handle_key(key(KeyCode::Char(character)));
        }
        assert_eq!(application.identity_input, "MFG:EPSON;MDL:C90;");
        assert!(
            identity_inspection_lines(&application.identity_input, &database)
                .iter()
                .any(|line| line == "Built-in match: C90")
        );
        application.handle_key(key(KeyCode::Esc));
        assert_eq!(application.view, View::Home);
    }

    #[test]
    fn workflow_view_only_exposes_guidance_and_returns_home() {
        let mut application = Application::new(vec![]);

        application.handle_key(key(KeyCode::Char('h')));
        assert_eq!(application.view, View::ReadOnlyWorkflows);
        assert_eq!(
            application.handle_key(key(KeyCode::Char('q'))),
            Navigation::Continue
        );
        assert_eq!(application.view, View::Home);
    }
}
