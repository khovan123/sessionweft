use std::{io, time::Duration};

use anyhow::Context;
use crossterm::{
    event::{self, Event as TerminalEvent, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use serde_json::Value;

const DEFAULT_TITLE: &str = "Shared coding session";

#[derive(Debug)]
pub(crate) enum PickerResult {
    Create(String),
    Resume(String),
    Quit,
}

#[derive(Debug, Clone)]
struct SessionRow {
    id: String,
    title: String,
    version: u64,
    messages: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Browse,
    Create,
}

struct App {
    agent: String,
    mode: Mode,
    selected: usize,
    sessions: Vec<SessionRow>,
    title: String,
}

impl App {
    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn move_down(&mut self) {
        self.selected = (self.selected + 1).min(self.sessions.len());
    }

    fn selected_session(&self) -> Option<&SessionRow> {
        self.selected
            .checked_sub(1)
            .and_then(|index| self.sessions.get(index))
    }
}

pub(crate) fn pick_session(value: &Value, agent: &str) -> anyhow::Result<PickerResult> {
    let sessions = parse_sessions(value)?;
    let mut app = App {
        agent: agent.to_owned(),
        mode: Mode::Browse,
        selected: 0,
        sessions,
        title: String::new(),
    };

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = run(&mut terminal, &mut app);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> anyhow::Result<PickerResult> {
    loop {
        terminal.draw(|frame| render(frame, app))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let TerminalEvent::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match app.mode {
            Mode::Browse => match key.code {
                KeyCode::Up | KeyCode::Char('k') => app.move_up(),
                KeyCode::Down | KeyCode::Char('j') => app.move_down(),
                KeyCode::Enter if app.selected == 0 => app.mode = Mode::Create,
                KeyCode::Enter => {
                    if let Some(session) = app.selected_session() {
                        return Ok(PickerResult::Resume(session.id.clone()));
                    }
                }
                KeyCode::Esc | KeyCode::Char('q') => return Ok(PickerResult::Quit),
                _ => {}
            },
            Mode::Create => match key.code {
                KeyCode::Enter => {
                    let title = app.title.trim();
                    return Ok(PickerResult::Create(if title.is_empty() {
                        DEFAULT_TITLE.to_owned()
                    } else {
                        title.to_owned()
                    }));
                }
                KeyCode::Esc => {
                    app.title.clear();
                    app.mode = Mode::Browse;
                }
                KeyCode::Backspace => {
                    app.title.pop();
                }
                KeyCode::Char(value) => app.title.push(value),
                _ => {}
            },
        }
    }
}

fn render(frame: &mut Frame<'_>, app: &App) {
    let area = centered_rect(82, 82, frame.area());
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(Line::from(vec![
                Span::styled(
                    " SessionWeft ",
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("{} ", app.agent)),
            ])),
        area,
    );

    let inner = area.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    });
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(match app.mode {
            Mode::Browse => "Create or continue a shared Session",
            Mode::Create => "Create a new shared Session",
        })
        .alignment(Alignment::Center),
        chunks[0],
    );

    match app.mode {
        Mode::Browse => render_sessions(frame, app, chunks[1]),
        Mode::Create => render_create(frame, app, chunks[1]),
    }

    let help = match app.mode {
        Mode::Browse => "↑/↓ Navigate   Enter Select   Esc Quit",
        Mode::Create => "Type a title   Enter Create   Esc Back",
    };
    frame.render_widget(
        Paragraph::new(help)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true }),
        chunks[2],
    );
}

fn render_sessions(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let mut items = vec![ListItem::new(Line::from(vec![Span::styled(
        "+ Create new Session",
        Style::default().add_modifier(Modifier::BOLD),
    )]))];
    items.extend(app.sessions.iter().map(|row| {
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("{:<32}", truncate(&row.title, 32)),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                " v{:<3} {:>3} msg  {}",
                row.version,
                row.messages,
                short_id(&row.id)
            )),
        ]))
    }));

    let mut state = ListState::default();
    state.select(Some(app.selected));
    frame.render_stateful_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" Sessions ({}) ", app.sessions.len())),
            )
            .highlight_symbol("› ")
            .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)),
        area,
        &mut state,
    );
}

fn render_create(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let input = if app.title.is_empty() {
        format!("{DEFAULT_TITLE}▏")
    } else {
        format!("{}▏", app.title)
    };
    frame.render_widget(
        Paragraph::new(input).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Session title "),
        ),
        area,
    );
}

fn parse_sessions(value: &Value) -> anyhow::Result<Vec<SessionRow>> {
    let sessions = value.as_array().context("Session list is not an array")?;
    Ok(sessions
        .iter()
        .filter_map(|session| {
            Some(SessionRow {
                id: session.get("id")?.as_str()?.to_owned(),
                title: session
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Untitled Session")
                    .to_owned(),
                version: session.get("version").and_then(Value::as_u64).unwrap_or(0),
                messages: session
                    .get("messages")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len),
            })
        })
        .collect())
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn short_id(value: &str) -> &str {
    value.get(..8).unwrap_or(value)
}

fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_owned();
    }
    value
        .chars()
        .take(max.saturating_sub(1))
        .collect::<String>()
        + "…"
}
