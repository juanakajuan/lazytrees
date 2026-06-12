use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap,
};

use crate::error::AppResult;
use crate::git::{
    NewOptions, Repo, Worktree, build_new_worktree_plan, create_worktree, default_worktree_path,
    list_worktrees, open_tmux_session, prune_worktrees, remove_worktree,
};

type TerminalBackend = Terminal<CrosstermBackend<io::Stdout>>;

/// Cohesive color palette (Gruvbox dark). Foreground-only so it adapts to the
/// user's terminal background; selection is the only filled surface.
mod theme {
    use ratatui::style::Color;

    pub const ACCENT: Color = Color::Rgb(0x83, 0xA5, 0x98); // blue
    pub const TEXT: Color = Color::Rgb(0xEB, 0xDB, 0xB2); // fg1
    pub const MUTED: Color = Color::Rgb(0xA8, 0x99, 0x84); // fg4
    pub const FAINT: Color = Color::Rgb(0x66, 0x5C, 0x54); // bg3
    pub const GOOD: Color = Color::Rgb(0xB8, 0xBB, 0x26); // green
    pub const WARN: Color = Color::Rgb(0xFA, 0xBD, 0x2F); // yellow
    pub const BAD: Color = Color::Rgb(0xFB, 0x49, 0x34); // red
    pub const SELECT_BG: Color = Color::Rgb(0x50, 0x49, 0x45); // bg2
}

/// Rounded, padded panel with an accent-styled title and muted border.
fn panel(title: &str, focused: bool) -> Block<'static> {
    let border = if focused { theme::ACCENT } else { theme::FAINT };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ))
}

/// A `[key] label` hint used in the footer.
fn hint<'a>(key: &'a str, label: &'a str) -> Vec<Span<'a>> {
    vec![
        Span::styled(
            key,
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {label}"), Style::default().fg(theme::MUTED)),
        Span::styled("   ", Style::default()),
    ]
}

pub fn run(repo: &Repo) -> AppResult<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(repo, &mut terminal);
    let cleanup = restore_terminal(&mut terminal);

    result.and(cleanup)
}

fn run_loop(repo: &Repo, terminal: &mut TerminalBackend) -> AppResult<()> {
    let mut app = TuiApp::new(repo)?;

    loop {
        terminal.draw(|frame| render(frame, &app))?;

        if app.should_quit {
            return Ok(());
        }

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };

        if key.kind != KeyEventKind::Press {
            continue;
        }

        match app.handle_key(key)? {
            Some(TuiAction::Open(path)) => {
                let result = open_from_tui(terminal, &path);
                app.refresh()?;
                match result {
                    Ok(()) => app.set_info(format!("tmux session opened: {}", path.display())),
                    Err(error) => app.set_error(error.to_string()),
                }
            }
            None => {}
        }
    }
}

fn restore_terminal(terminal: &mut TerminalBackend) -> AppResult<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn open_from_tui(terminal: &mut TerminalBackend, path: &Path) -> AppResult<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    println!("lazytrees opening tmux session in {}", path.display());
    let result = open_tmux_session(path);

    execute!(terminal.backend_mut(), EnterAlternateScreen)?;
    enable_raw_mode()?;
    terminal.clear()?;

    result
}

struct TuiApp<'repo> {
    repo: &'repo Repo,
    worktrees: Vec<Worktree>,
    selected: usize,
    mode: Mode,
    status: StatusMessage,
    should_quit: bool,
}

enum TuiAction {
    Open(PathBuf),
}

enum Mode {
    Browse,
    NewBranch {
        input: String,
    },
    NewBase {
        branch: String,
        input: String,
    },
    NewPath {
        branch: String,
        base: String,
        input: String,
    },
    ConfirmRemove {
        path: PathBuf,
        branch: String,
    },
    ConfirmPrune,
}

struct StatusMessage {
    text: String,
    kind: StatusKind,
}

enum StatusKind {
    Info,
    Error,
}

impl<'repo> TuiApp<'repo> {
    fn new(repo: &'repo Repo) -> AppResult<Self> {
        let mut app = Self {
            repo,
            worktrees: Vec::new(),
            selected: 0,
            mode: Mode::Browse,
            status: StatusMessage {
                text: "ready".to_owned(),
                kind: StatusKind::Info,
            },
            should_quit: false,
        };
        app.refresh()?;
        Ok(app)
    }

    fn refresh(&mut self) -> AppResult<()> {
        self.worktrees = list_worktrees(self.repo)?;
        if self.worktrees.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.worktrees.len() {
            self.selected = self.worktrees.len() - 1;
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> AppResult<Option<TuiAction>> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return Ok(None);
        }

        let mode = std::mem::replace(&mut self.mode, Mode::Browse);
        match mode {
            Mode::Browse => self.handle_browse_key(key),
            Mode::NewBranch { input } => self.handle_new_branch_key(key, input),
            Mode::NewBase { branch, input } => self.handle_new_base_key(key, branch, input),
            Mode::NewPath {
                branch,
                base,
                input,
            } => self.handle_new_path_key(key, branch, base, input),
            Mode::ConfirmRemove { path, branch } => {
                self.handle_remove_confirmation(key, path, branch)
            }
            Mode::ConfirmPrune => self.handle_prune_confirmation(key),
        }
    }

    fn handle_new_branch_key(
        &mut self,
        key: KeyEvent,
        mut input: String,
    ) -> AppResult<Option<TuiAction>> {
        match prompt_key(&mut input, key) {
            PromptResult::Submit(branch) => self.submit_branch(branch),
            PromptResult::Cancel => self.cancel_new_worktree(),
            PromptResult::Continue => {
                self.mode = Mode::NewBranch { input };
                Ok(None)
            }
        }
    }

    fn handle_new_base_key(
        &mut self,
        key: KeyEvent,
        branch: String,
        mut input: String,
    ) -> AppResult<Option<TuiAction>> {
        match prompt_key(&mut input, key) {
            PromptResult::Submit(base) => {
                let base = if base.is_empty() {
                    "HEAD".to_owned()
                } else {
                    base
                };
                self.mode = Mode::NewPath {
                    branch,
                    base,
                    input: String::new(),
                };
                Ok(None)
            }
            PromptResult::Cancel => self.cancel_new_worktree(),
            PromptResult::Continue => {
                self.mode = Mode::NewBase { branch, input };
                Ok(None)
            }
        }
    }

    fn handle_new_path_key(
        &mut self,
        key: KeyEvent,
        branch: String,
        base: String,
        mut input: String,
    ) -> AppResult<Option<TuiAction>> {
        match prompt_key(&mut input, key) {
            PromptResult::Submit(path) => {
                let path = if path.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(path))
                };
                self.create_from_prompt(branch, base, path)
            }
            PromptResult::Cancel => self.cancel_new_worktree(),
            PromptResult::Continue => {
                self.mode = Mode::NewPath {
                    branch,
                    base,
                    input,
                };
                Ok(None)
            }
        }
    }

    fn handle_browse_key(&mut self, key: KeyEvent) -> AppResult<Option<TuiAction>> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.select_next(),
            KeyCode::Char('k') | KeyCode::Up => self.select_previous(),
            KeyCode::Char('g') | KeyCode::Home => self.selected = 0,
            KeyCode::Char('G') | KeyCode::End if !self.worktrees.is_empty() => {
                self.selected = self.worktrees.len() - 1;
            }
            KeyCode::Char('r') => {
                self.refresh()?;
                self.set_info("refreshed");
            }
            KeyCode::Char('n') => {
                self.mode = Mode::NewBranch {
                    input: String::new(),
                };
            }
            KeyCode::Char('p') => {
                self.mode = Mode::ConfirmPrune;
            }
            KeyCode::Char('d') => self.confirm_remove_selected(),
            KeyCode::Enter | KeyCode::Char('o') => {
                if let Some(worktree) = self.selected_worktree() {
                    return Ok(Some(TuiAction::Open(worktree.path.clone())));
                }
                self.set_error("no worktree selected");
            }
            _ => {}
        }

        Ok(None)
    }

    fn confirm_remove_selected(&mut self) {
        let Some((path, branch, prunable)) = self.selected_worktree().map(|worktree| {
            (
                worktree.path.clone(),
                worktree.branch_label().to_owned(),
                worktree.prunable,
            )
        }) else {
            self.set_error("no worktree selected");
            return;
        };

        if prunable {
            self.set_error("use prune for stale worktree metadata");
            return;
        }

        if path == self.repo.root {
            self.set_error("cannot remove current worktree");
            return;
        }

        self.mode = Mode::ConfirmRemove { path, branch };
    }

    fn submit_branch(&mut self, branch: String) -> AppResult<Option<TuiAction>> {
        if branch.is_empty() {
            self.set_error("branch name is required");
            return Ok(None);
        }

        self.mode = Mode::NewBase {
            branch,
            input: String::new(),
        };
        Ok(None)
    }

    fn create_from_prompt(
        &mut self,
        branch: String,
        base: String,
        path: Option<PathBuf>,
    ) -> AppResult<Option<TuiAction>> {
        let options = NewOptions {
            branch: Some(branch),
            base: Some(base),
            path,
        };

        let plan = match build_new_worktree_plan(self.repo, options) {
            Ok(plan) => plan,
            Err(error) => {
                self.mode = Mode::Browse;
                self.set_error(error.to_string());
                return Ok(None);
            }
        };

        if let Err(error) = create_worktree(self.repo, &plan) {
            self.mode = Mode::Browse;
            self.set_error(error.to_string());
            return Ok(None);
        }

        self.refresh()?;
        self.select_path(&plan.path);
        self.mode = Mode::Browse;
        self.set_info(format!("created {}", plan.path.display()));

        Ok(Some(TuiAction::Open(plan.path.clone())))
    }

    fn handle_remove_confirmation(
        &mut self,
        key: KeyEvent,
        path: PathBuf,
        branch: String,
    ) -> AppResult<Option<TuiAction>> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => match remove_worktree(self.repo, &path) {
                Ok(()) => {
                    self.refresh()?;
                    self.mode = Mode::Browse;
                    self.set_info(format!("removed {}", path.display()));
                }
                Err(error) => {
                    self.mode = Mode::Browse;
                    self.set_error(error.to_string());
                }
            },
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.mode = Mode::Browse;
                self.set_info("remove cancelled");
            }
            _ => self.mode = Mode::ConfirmRemove { path, branch },
        }

        Ok(None)
    }

    fn handle_prune_confirmation(&mut self, key: KeyEvent) -> AppResult<Option<TuiAction>> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                prune_worktrees(self.repo)?;
                self.refresh()?;
                self.mode = Mode::Browse;
                self.set_info("pruned stale metadata");
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.mode = Mode::Browse;
                self.set_info("prune cancelled");
            }
            _ => self.mode = Mode::ConfirmPrune,
        }

        Ok(None)
    }

    fn select_next(&mut self) {
        if self.worktrees.is_empty() {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected + 1).min(self.worktrees.len() - 1);
    }

    fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn select_path(&mut self, path: &PathBuf) {
        if let Some(index) = self
            .worktrees
            .iter()
            .position(|worktree| worktree.path == *path)
        {
            self.selected = index;
        }
    }

    fn selected_worktree(&self) -> Option<&Worktree> {
        self.worktrees.get(self.selected)
    }

    fn set_info(&mut self, text: impl Into<String>) {
        self.status = StatusMessage {
            text: text.into(),
            kind: StatusKind::Info,
        };
    }

    fn cancel_new_worktree(&mut self) -> AppResult<Option<TuiAction>> {
        self.mode = Mode::Browse;
        self.set_info("new worktree cancelled");
        Ok(None)
    }

    fn set_error(&mut self, text: impl Into<String>) {
        self.status = StatusMessage {
            text: text.into(),
            kind: StatusKind::Error,
        };
    }
}

enum PromptResult {
    Submit(String),
    Cancel,
    Continue,
}

fn prompt_key(input: &mut String, key: KeyEvent) -> PromptResult {
    match key.code {
        KeyCode::Enter => PromptResult::Submit(input.trim().to_owned()),
        KeyCode::Esc => PromptResult::Cancel,
        KeyCode::Backspace => {
            input.pop();
            PromptResult::Continue
        }
        KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            input.push(character);
            PromptResult::Continue
        }
        _ => PromptResult::Continue,
    }
}

fn render(frame: &mut ratatui::Frame<'_>, app: &TuiApp<'_>) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(4),
        ])
        .split(area);

    render_header(frame, chunks[0], app);
    render_body(frame, chunks[1], app);
    render_footer(frame, chunks[2], app);
    render_mode(frame, area, app);
}

fn render_header(frame: &mut ratatui::Frame<'_>, area: Rect, app: &TuiApp<'_>) {
    let count = app.worktrees.len();
    let title = Line::from(vec![
        Span::styled(
            " 🌳 lazytrees ",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("│ ", Style::default().fg(theme::FAINT)),
        Span::styled(
            format!("{count}"),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if count == 1 {
                " worktree "
            } else {
                " worktrees "
            },
            Style::default().fg(theme::MUTED),
        ),
        Span::styled("│ ", Style::default().fg(theme::FAINT)),
        Span::styled(
            app.repo.root.display().to_string(),
            Style::default().fg(theme::TEXT),
        ),
    ]);
    let header = Paragraph::new(title).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme::FAINT)),
    );
    frame.render_widget(header, area);
}

fn render_body(frame: &mut ratatui::Frame<'_>, area: Rect, app: &TuiApp<'_>) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
        .split(area);

    render_worktree_list(frame, chunks[0], app);
    render_worktree_details(frame, chunks[1], app);
}

fn render_worktree_list(frame: &mut ratatui::Frame<'_>, area: Rect, app: &TuiApp<'_>) {
    if app.worktrees.is_empty() {
        let empty = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "No worktrees yet",
                Style::default().fg(theme::MUTED),
            )),
            Line::from(Span::styled(
                "Press n to create one",
                Style::default().fg(theme::FAINT),
            )),
        ])
        .alignment(Alignment::Center)
        .block(panel("Worktrees", true));
        frame.render_widget(empty, area);
        return;
    }

    let items: Vec<ListItem<'_>> = app
        .worktrees
        .iter()
        .map(|worktree| {
            let (glyph, glyph_style) = worktree_glyph(worktree);
            let branch_style = if worktree.prunable {
                Style::default().fg(theme::WARN)
            } else {
                Style::default()
                    .fg(theme::TEXT)
                    .add_modifier(Modifier::BOLD)
            };
            let dir = worktree
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| worktree.path.display().to_string());
            ListItem::new(Line::from(vec![
                Span::styled(format!("{glyph} "), glyph_style),
                Span::styled(worktree.branch_label().to_owned(), branch_style),
                Span::styled(format!("  {dir}"), Style::default().fg(theme::FAINT)),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(panel("Worktrees", true))
        .highlight_style(
            Style::default()
                .fg(theme::ACCENT)
                .bg(theme::SELECT_BG)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▎");
    let mut state = ListState::default();
    state.select(Some(app.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_worktree_details(frame: &mut ratatui::Frame<'_>, area: Rect, app: &TuiApp<'_>) {
    let lines = match app.selected_worktree() {
        Some(worktree) => vec![
            Line::from(""),
            detail_row("", "branch", worktree.branch_label(), theme::TEXT),
            detail_row("", "head", worktree.short_head(), theme::ACCENT),
            detail_row(
                "",
                "path",
                &worktree.path.display().to_string(),
                theme::TEXT,
            ),
            detail_row(
                "",
                "state",
                &worktree_state(worktree),
                state_color(worktree),
            ),
            Line::from(""),
            Line::from(Span::styled(
                "─".repeat(area.width.saturating_sub(4) as usize),
                Style::default().fg(theme::FAINT),
            )),
            Line::from(""),
            detail_row("", "open", "tmux", theme::GOOD),
        ],
        None => vec![
            Line::from(""),
            Line::from(Span::styled(
                "No worktree selected.",
                Style::default().fg(theme::MUTED),
            )),
        ],
    };

    let details = Paragraph::new(lines)
        .block(panel("Details", false))
        .wrap(Wrap { trim: false });
    frame.render_widget(details, area);
}

/// Aligned `icon label  value` row with a muted label and colored value.
fn detail_row(icon: &str, label: &str, value: &str, value_color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{icon}{label:<7}"),
            Style::default().fg(theme::MUTED),
        ),
        Span::styled(value.to_owned(), Style::default().fg(value_color)),
    ])
}

/// Leading status glyph and its color for a worktree list entry.
fn worktree_glyph(worktree: &Worktree) -> (&'static str, Style) {
    if worktree.prunable {
        ("⚠", Style::default().fg(theme::WARN))
    } else if worktree.bare {
        ("◇", Style::default().fg(theme::MUTED))
    } else if worktree.detached {
        ("⚲", Style::default().fg(theme::ACCENT))
    } else {
        ("●", Style::default().fg(theme::GOOD))
    }
}

fn state_color(worktree: &Worktree) -> Color {
    if worktree.prunable {
        theme::WARN
    } else if worktree.detached || worktree.bare {
        theme::ACCENT
    } else {
        theme::GOOD
    }
}

fn render_footer(frame: &mut ratatui::Frame<'_>, area: Rect, app: &TuiApp<'_>) {
    let (status_icon, status_style) = match app.status.kind {
        StatusKind::Info => ("✓", Style::default().fg(theme::GOOD)),
        StatusKind::Error => ("✗", Style::default().fg(theme::BAD)),
    };

    let mut status_line = vec![
        Span::styled(
            format!("{status_icon} "),
            status_style.add_modifier(Modifier::BOLD),
        ),
        Span::styled(app.status.text.clone(), status_style),
        Span::styled("    ", Style::default()),
    ];
    status_line.extend(hint("↵", "open"));
    status_line.extend(hint("n", "new"));
    status_line.extend(hint("d", "remove"));
    status_line.extend(hint("p", "prune"));
    status_line.extend(hint("r", "refresh"));
    status_line.extend(hint("q", "quit"));

    let footer = Paragraph::new(vec![
        Line::from(status_line),
        Line::from(vec![
            Span::styled("default parent  ", Style::default().fg(theme::FAINT)),
            Span::styled(
                app.repo.default_worktree_parent.display().to_string(),
                Style::default().fg(theme::MUTED),
            ),
        ]),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme::FAINT))
            .padding(Padding::horizontal(1)),
    );
    frame.render_widget(footer, area);
}

fn render_mode(frame: &mut ratatui::Frame<'_>, area: Rect, app: &TuiApp<'_>) {
    match &app.mode {
        Mode::Browse => {}
        Mode::NewBranch { input } => {
            render_prompt(frame, area, "New worktree branch", input, None);
        }
        Mode::NewBase { input, .. } => {
            render_prompt(frame, area, "Base ref", input, Some("HEAD"));
        }
        Mode::NewPath { branch, input, .. } => {
            let default_path = default_worktree_path(app.repo, branch);
            render_prompt(
                frame,
                area,
                "Worktree path",
                input,
                Some(&default_path.display().to_string()),
            );
        }
        Mode::ConfirmRemove { path, branch } => {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Remove selected worktree?",
                    Style::default()
                        .fg(theme::TEXT)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(vec![
                    Span::styled("branch  ", Style::default().fg(theme::FAINT)),
                    Span::styled(branch.to_owned(), Style::default().fg(theme::TEXT)),
                ]),
                Line::from(vec![
                    Span::styled("path    ", Style::default().fg(theme::FAINT)),
                    Span::styled(
                        path.display().to_string(),
                        Style::default().fg(theme::MUTED),
                    ),
                ]),
                Line::from(""),
                confirm_action_line(),
            ];
            render_confirmation_popup(
                frame,
                area,
                ConfirmationPopup {
                    width_percent: 64,
                    height: 9,
                    title: "Confirm remove",
                    border_color: theme::BAD,
                    lines,
                    wrap: true,
                },
            );
        }
        Mode::ConfirmPrune => {
            let lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "Run git worktree prune?",
                    Style::default()
                        .fg(theme::TEXT)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                confirm_action_line(),
            ];
            render_confirmation_popup(
                frame,
                area,
                ConfirmationPopup {
                    width_percent: 56,
                    height: 7,
                    title: "Confirm prune",
                    border_color: theme::WARN,
                    lines,
                    wrap: false,
                },
            );
        }
    }
}

struct ConfirmationPopup {
    width_percent: u16,
    height: u16,
    title: &'static str,
    border_color: Color,
    lines: Vec<Line<'static>>,
    wrap: bool,
}

fn render_confirmation_popup(frame: &mut ratatui::Frame<'_>, area: Rect, popup: ConfirmationPopup) {
    let popup_area = clear_popup_area(frame, area, popup.width_percent, popup.height);
    let paragraph = Paragraph::new(popup.lines)
        .alignment(Alignment::Center)
        .block(popup_block(popup.title, popup.border_color));
    let paragraph = if popup.wrap {
        paragraph.wrap(Wrap { trim: false })
    } else {
        paragraph
    };
    frame.render_widget(paragraph, popup_area);
}

fn render_prompt(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    title: &str,
    input: &str,
    default: Option<&str>,
) {
    let popup = clear_popup_area(frame, area, 70, 8);

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("❯ ", Style::default().fg(theme::ACCENT)),
            Span::styled(
                input.to_owned(),
                Style::default()
                    .fg(theme::TEXT)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("█", Style::default().fg(theme::ACCENT)),
        ]),
    ];
    if let Some(default) = default {
        lines.push(Line::from(vec![
            Span::styled("  default  ", Style::default().fg(theme::FAINT)),
            Span::styled(default.to_owned(), Style::default().fg(theme::MUTED)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(
            "↵",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" accept    ", Style::default().fg(theme::MUTED)),
        Span::styled(
            "esc",
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" cancel", Style::default().fg(theme::MUTED)),
    ]));

    let paragraph = Paragraph::new(lines)
        .block(popup_block(title, theme::ACCENT).padding(Padding::horizontal(1)))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup);
}

fn clear_popup_area(
    frame: &mut ratatui::Frame<'_>,
    area: Rect,
    percent_x: u16,
    height: u16,
) -> Rect {
    let popup = centered_rect(percent_x, height, area);
    render_shadow(frame, popup);
    frame.render_widget(Clear, popup);
    popup
}

fn popup_block(title: &str, border_color: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ))
}

fn confirm_action_line() -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "y",
            Style::default()
                .fg(theme::GOOD)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" confirm    ", Style::default().fg(theme::MUTED)),
        Span::styled(
            "n",
            Style::default().fg(theme::BAD).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" cancel", Style::default().fg(theme::MUTED)),
    ])
}

/// Draw a soft drop-shadow one cell down-right of the popup for depth.
fn render_shadow(frame: &mut ratatui::Frame<'_>, popup: Rect) {
    let area = frame.area();
    let shadow = Rect {
        x: (popup.x + 1).min(area.width.saturating_sub(1)),
        y: (popup.y + 1).min(area.height.saturating_sub(1)),
        width: popup.width.min(area.width.saturating_sub(popup.x + 1)),
        height: popup.height.min(area.height.saturating_sub(popup.y + 1)),
    };
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(0x1D, 0x20, 0x21))),
        shadow,
    );
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(height.min(area.height)),
            Constraint::Min(0),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1]);
    horizontal[1]
}

fn worktree_state(worktree: &Worktree) -> String {
    let mut states = Vec::new();
    if worktree.bare {
        states.push("bare");
    }
    if worktree.detached {
        states.push("detached");
    }
    if worktree.prunable {
        states.push("prunable");
    }
    if states.is_empty() {
        "normal".to_owned()
    } else {
        states.join(", ")
    }
}
