//! Interactive view: track and manage every change and workspace.

use crate::accept::{self, Outcome};
use crate::api::{self, Overview};
use crate::git::{Oid, Repo};
use crate::{change, evidence, view, workspace, Result};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pane {
    Changes,
    Workspaces,
}

/// What a key press asks for. The UI itself never touches the graph.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    Refresh,
    Accept(Oid),
    Check(Oid),
    Retry(Oid),
    Discard(Oid),
    Dispose(String),
}

pub struct App {
    overview: Overview,
    focus: Pane,
    change: usize,
    workspace: usize,
    /// Result of the last action.
    pub message: String,
}

const HELP: &str = "a accept  c check  t retry  d discard/dispose  r refresh  tab switch pane  q quit";

impl App {
    pub fn new(overview: Overview) -> App {
        App { overview, focus: Pane::Changes, change: 0, workspace: 0, message: String::new() }
    }

    /// Swap in fresh data, keeping the selection in bounds.
    pub fn update(&mut self, overview: Overview) {
        self.overview = overview;
        self.change = self.change.min(self.overview.changes.len().saturating_sub(1));
        self.workspace = self.workspace.min(self.overview.workspaces.len().saturating_sub(1));
    }

    fn selected_change(&self) -> Option<Oid> {
        (self.focus == Pane::Changes)
            .then(|| self.overview.changes.get(self.change))
            .flatten()
            .map(|r| r.change.id.clone())
    }

    fn selected_workspace(&self) -> Option<String> {
        (self.focus == Pane::Workspaces)
            .then(|| self.overview.workspaces.get(self.workspace))
            .flatten()
            .map(|r| r.workspace.id.clone())
    }

    fn step(&mut self, delta: isize) {
        let (index, len) = match self.focus {
            Pane::Changes => (&mut self.change, self.overview.changes.len()),
            Pane::Workspaces => (&mut self.workspace, self.overview.workspaces.len()),
        };
        *index = index.saturating_add_signed(delta).min(len.saturating_sub(1));
    }

    /// A key press with its modifiers. In raw mode Ctrl-C is a key, not a
    /// signal: it quits rather than running `c`.
    pub fn on_event(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
            code => self.on_key(code),
        }
    }

    pub fn on_key(&mut self, key: KeyCode) -> Action {
        let on_change = |f: fn(Oid) -> Action, id: Option<Oid>| id.map_or(Action::None, f);
        match key {
            KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
            KeyCode::Char('r') => Action::Refresh,
            KeyCode::Tab => {
                self.focus = if self.focus == Pane::Changes { Pane::Workspaces } else { Pane::Changes };
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.step(1);
                Action::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.step(-1);
                Action::None
            }
            KeyCode::Char('a') => on_change(Action::Accept, self.selected_change()),
            KeyCode::Char('c') => on_change(Action::Check, self.selected_change()),
            KeyCode::Char('t') => on_change(Action::Retry, self.selected_change()),
            KeyCode::Char('d') => match self.focus {
                Pane::Changes => on_change(Action::Discard, self.selected_change()),
                Pane::Workspaces => self.selected_workspace().map_or(Action::None, Action::Dispose),
            },
            _ => Action::None,
        }
    }

    pub fn draw(&self, frame: &mut Frame) {
        let o = &self.overview;
        let [head, changes, workspaces, foot] =
            Layout::vertical([Constraint::Length(1), Constraint::Fill(3), Constraint::Fill(2), Constraint::Length(2)])
                .areas(frame.area());

        let c = &o.current;
        let current = format!(" current {}  {}  {}  {}", c.id.short(), c.agent, view::age(c.time), view::subject(c));
        frame.render_widget(Paragraph::new(current).style(Style::new().add_modifier(Modifier::BOLD)), head);

        let pane = |title: String, focused: bool| {
            let style = if focused { Style::new().fg(Color::Cyan) } else { Style::new() };
            Block::bordered().title(title).border_style(style)
        };
        let highlight = Style::new().add_modifier(Modifier::REVERSED);

        let rows: Vec<Row> = match o.changes.is_empty() {
            true => vec![Row::new(["", "", "", "", "no speculative changes"].map(Cell::from))],
            false => o
                .changes
                .iter()
                .map(|row| {
                    let color = match view::status_label(&row.status).as_str() {
                        "verified" => Color::Green,
                        "speculative" => Color::Yellow,
                        _ => Color::Red,
                    };
                    let why = view::reasons(&row.status).first().map(|r| format!("  ({r})")).unwrap_or_default();
                    Row::new(vec![
                        Cell::from(row.change.id.short().to_string()),
                        Cell::from(view::status_label(&row.status)).style(Style::new().fg(color)),
                        Cell::from(row.change.agent.clone()),
                        Cell::from(view::age(row.change.time)),
                        Cell::from(format!("{}{why}", view::subject(&row.change))),
                    ])
                })
                .collect(),
        };
        let widths = [
            Constraint::Length(10),
            Constraint::Length(17),
            Constraint::Length(12),
            Constraint::Length(4),
            Constraint::Fill(1),
        ];
        let table = Table::new(rows, widths)
            .block(pane(format!(" changes ({}) ", o.changes.len()), self.focus == Pane::Changes))
            .row_highlight_style(highlight);
        let mut state = TableState::default().with_selected((!o.changes.is_empty()).then_some(self.change));
        frame.render_stateful_widget(table, changes, &mut state);

        let rows: Vec<Row> = o
            .workspaces
            .iter()
            .map(|row| {
                let w = &row.workspace;
                Row::new(vec![
                    Cell::from(w.id.clone()),
                    Cell::from(w.base.short().to_string()),
                    Cell::from(w.agent.clone()),
                    Cell::from(view::age(w.created)),
                    Cell::from(view::workspace_state(row)),
                    Cell::from(w.path().display().to_string()),
                ])
            })
            .collect();
        let widths = [
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(4),
            Constraint::Length(18),
            Constraint::Fill(1),
        ];
        let table = Table::new(rows, widths)
            .block(pane(format!(" workspaces ({}) ", o.workspaces.len()), self.focus == Pane::Workspaces))
            .row_highlight_style(highlight);
        let mut state = TableState::default().with_selected((!o.workspaces.is_empty()).then_some(self.workspace));
        frame.render_stateful_widget(table, workspaces, &mut state);

        // One line for the message (git errors span several), one for the keys.
        let message = self.message.lines().collect::<Vec<_>>().join(" ");
        frame.render_widget(Paragraph::new(format!(" {message}\n {HELP}")), foot);
    }
}

/// Carry out an action against the graph; the returned line is shown to the user.
fn perform(repo: &Repo, action: &Action) -> Result<String> {
    Ok(match action {
        Action::Accept(id) => match accept::accept(repo, id)? {
            Outcome::Accepted { current, .. } => format!("accepted; current is {}", current.short()),
            Outcome::AlreadyAccepted => "already accepted".into(),
            Outcome::Rejected(invalid) => {
                let status = accept::Status::Invalid(invalid);
                format!("rejected: {}", view::reasons(&status).join("; "))
            }
        },
        Action::Check(id) => {
            let verdicts = evidence::verify(repo, id, false)?;
            let passed = verdicts.iter().filter(|v| v.evidence.passed).count();
            format!("{passed}/{} checks pass", verdicts.len())
        }
        Action::Retry(id) => format!("retry workspace at {}", accept::retry(repo, id)?.path().display()),
        Action::Discard(id) => {
            change::discard(repo, id)?;
            format!("discarded {}", id.short())
        }
        Action::Dispose(id) => {
            workspace::dispose(repo, id)?;
            format!("disposed {id}")
        }
        Action::None | Action::Quit | Action::Refresh => String::new(),
    })
}

const REFRESH: Duration = Duration::from_secs(2);

pub fn run(repo: &Repo) -> Result<()> {
    let mut app = App::new(api::overview(repo)?);
    let mut terminal = ratatui::try_init().map_err(|e| crate::Error::Other(format!("zit ui needs a terminal: {e}")))?;
    let result = (|| -> Result<()> {
        loop {
            terminal.draw(|frame| app.draw(frame))?;
            let action = match event::poll(REFRESH)? {
                true => match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => app.on_event(key),
                    _ => Action::None,
                },
                false => Action::Refresh,
            };
            match action {
                Action::Quit => return Ok(()),
                Action::None => continue,
                action => {
                    match perform(repo, &action) {
                        Ok(message) if action == Action::Refresh && message.is_empty() => {}
                        Ok(message) => app.message = message,
                        Err(e) => app.message = format!("error: {e}"),
                    }
                    app.update(api::overview(repo)?);
                }
            }
        }
    })();
    ratatui::restore();
    result
}
