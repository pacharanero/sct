//! `sct tui` — Keyboard-driven terminal UI for SNOMED CT exploration.
//!
//! Layout: three panels — hierarchy list (top-left), search/results (bottom-left),
//! concept detail (right). Navigate with Tab/←→, search with /, quit with q.
//!
//! Requires the `tui` Cargo feature: `cargo build --features tui`

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use rusqlite::{params, Connection, OpenFlags};
use serde_json::Value;
use std::{io, path::PathBuf, time::Duration};

#[derive(Parser, Debug)]
pub struct Args {
    /// Path to the SNOMED CT SQLite database produced by `sct sqlite`.
    /// Falls back to ./snomed.db then $SCT_DB.
    #[arg(long)]
    pub db: Option<PathBuf>,
}

pub fn run(args: Args) -> Result<()> {
    let db_path = resolve_db_path(args.db)?;
    let conn = Connection::open_with_flags(
        &db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening database {}", db_path.display()))?;
    conn.execute_batch("PRAGMA cache_size = -32768;")?;

    let mut app = App::new(conn)?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    result
}

// ---------------------------------------------------------------------------
// DB path resolution
// ---------------------------------------------------------------------------

fn resolve_db_path(arg: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = arg {
        return Ok(p);
    }
    let default = PathBuf::from("snomed.db");
    if default.exists() {
        return Ok(default);
    }
    if let Ok(env_path) = std::env::var("SCT_DB") {
        let p = PathBuf::from(env_path);
        if p.exists() {
            return Ok(p);
        }
    }
    anyhow::bail!(
        "No database found. Specify --db <path>, place snomed.db in the current directory, \
         or set $SCT_DB.\nBuild a database first with: sct sqlite --input snomed.ndjson"
    )
}

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ConceptSummary {
    id: String,
    preferred_term: String,
}

#[derive(Clone)]
struct Concept {
    id: String,
    fsn: String,
    preferred_term: String,
    synonyms: Vec<String>,
    hierarchy: String,
    hierarchy_path: Vec<String>,
    parents: Vec<(String, String)>,   // (id, fsn)
    children_count: i64,
    attributes: Vec<(String, Vec<(String, String)>)>, // (attr_name, [(id, fsn)])
}

#[derive(PartialEq, Clone, Copy)]
enum Focus {
    Hierarchy,
    Search,
    Detail,
}

struct App {
    conn: Connection,
    // Hierarchy panel
    hierarchies: Vec<String>,
    hierarchy_state: ListState,
    // Search panel
    search_query: String,
    search_results: Vec<ConceptSummary>,
    results_state: ListState,
    searching: bool,
    last_queried: String,
    // Detail panel
    current_concept: Option<Concept>,
    detail_scroll: u16,
    // Navigation
    history: Vec<String>,
    focus: Focus,
    should_quit: bool,
}

impl App {
    fn new(conn: Connection) -> Result<Self> {
        let hierarchies = load_hierarchies(&conn)?;
        let mut app = App {
            conn,
            hierarchies,
            hierarchy_state: ListState::default(),
            search_query: String::new(),
            search_results: Vec::new(),
            results_state: ListState::default(),
            searching: false,
            last_queried: String::new(),
            current_concept: None,
            detail_scroll: 0,
            history: Vec::new(),
            focus: Focus::Hierarchy,
            should_quit: false,
        };
        if !app.hierarchies.is_empty() {
            app.hierarchy_state.select(Some(0));
        }
        Ok(app)
    }

    fn run_search(&mut self) {
        if self.search_query == self.last_queried {
            return;
        }
        self.last_queried = self.search_query.clone();
        if self.search_query.trim().is_empty() {
            self.search_results.clear();
            self.results_state.select(None);
            return;
        }
        match search_concepts(&self.conn, &self.search_query, 50) {
            Ok(results) => {
                self.search_results = results;
                if !self.search_results.is_empty() {
                    self.results_state.select(Some(0));
                } else {
                    self.results_state.select(None);
                }
            }
            Err(_) => {
                self.search_results.clear();
                self.results_state.select(None);
            }
        }
    }

    fn load_hierarchy_concepts(&mut self, hierarchy: String) {
        self.search_query.clear();
        self.last_queried.clear();
        match fetch_hierarchy_concepts(&self.conn, &hierarchy, 200) {
            Ok(results) => {
                self.search_results = results;
                if !self.search_results.is_empty() {
                    self.results_state.select(Some(0));
                } else {
                    self.results_state.select(None);
                }
            }
            Err(_) => {}
        }
    }

    fn load_concept(&mut self, id: String) {
        if let Some(current) = &self.current_concept {
            if current.id != id {
                if self.history.len() >= 20 {
                    self.history.remove(0);
                }
                self.history.push(current.id.clone());
            }
        }
        if let Ok(Some(c)) = fetch_concept(&self.conn, &id) {
            self.current_concept = Some(c);
            self.detail_scroll = 0;
        }
    }

    fn go_back(&mut self) {
        if let Some(id) = self.history.pop() {
            if let Ok(Some(c)) = fetch_concept(&self.conn, &id) {
                self.current_concept = Some(c);
                self.detail_scroll = 0;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// DB queries
// ---------------------------------------------------------------------------

fn load_hierarchies(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT hierarchy FROM concepts \
         WHERE hierarchy IS NOT NULL AND hierarchy != '' \
         ORDER BY hierarchy",
    )?;
    let hierarchies = stmt
        .query_map([], |row| row.get(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(hierarchies)
}

fn sanitise_fts(q: &str) -> String {
    let trimmed = q.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Pass through if caller already wrote an FTS5 expression
    if trimmed.contains(" OR ")
        || trimmed.contains(" AND ")
        || trimmed.contains('*')
        || trimmed.contains('"')
    {
        return trimmed.to_string();
    }
    if trimmed.contains(' ') {
        format!("\"{}\"", trimmed)
    } else {
        format!("{}*", trimmed)
    }
}

fn search_concepts(conn: &Connection, query: &str, limit: usize) -> Result<Vec<ConceptSummary>> {
    let safe = sanitise_fts(query);
    if safe.is_empty() {
        return Ok(vec![]);
    }
    let mut stmt = conn.prepare(
        "SELECT id, preferred_term FROM concepts_fts \
         WHERE concepts_fts MATCH ?1 ORDER BY rank LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![safe, limit as i64], |row| {
            Ok(ConceptSummary {
                id: row.get(0)?,
                preferred_term: row.get(1)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

fn fetch_hierarchy_concepts(
    conn: &Connection,
    hierarchy: &str,
    limit: usize,
) -> Result<Vec<ConceptSummary>> {
    let mut stmt = conn.prepare(
        "SELECT id, preferred_term FROM concepts \
         WHERE hierarchy = ?1 ORDER BY preferred_term LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![hierarchy, limit as i64], |row| {
            Ok(ConceptSummary {
                id: row.get(0)?,
                preferred_term: row.get(1)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

fn fetch_concept(conn: &Connection, id: &str) -> Result<Option<Concept>> {
    let result = conn.query_row(
        "SELECT id, fsn, preferred_term, synonyms, hierarchy, hierarchy_path, \
                parents, children_count, attributes \
         FROM concepts WHERE id = ?1",
        params![id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                row.get::<_, i64>(7).unwrap_or(0),
                row.get::<_, Option<String>>(8)?.unwrap_or_default(),
            ))
        },
    );

    match result {
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
        Ok((id, fsn, preferred_term, syn_raw, hier, path_raw, par_raw, children_count, attr_raw)) => {
            let synonyms: Vec<String> =
                serde_json::from_str(&syn_raw).unwrap_or_default();
            let hierarchy_path: Vec<String> =
                serde_json::from_str(&path_raw).unwrap_or_default();

            let parents_val: Value =
                serde_json::from_str(&par_raw).unwrap_or(Value::Array(vec![]));
            let parents: Vec<(String, String)> = parents_val
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| {
                            Some((
                                v["id"].as_str()?.to_string(),
                                v["fsn"].as_str().unwrap_or("").to_string(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();

            let attrs_val: Value = serde_json::from_str(&attr_raw)
                .unwrap_or(Value::Object(serde_json::Map::new()));
            let attributes: Vec<(String, Vec<(String, String)>)> =
                if let Some(obj) = attrs_val.as_object() {
                    obj.iter()
                        .map(|(k, v)| {
                            let vals = v
                                .as_array()
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|item| {
                                            Some((
                                                item["id"].as_str()?.to_string(),
                                                item["fsn"].as_str().unwrap_or("").to_string(),
                                            ))
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            (k.clone(), vals)
                        })
                        .collect()
                } else {
                    vec![]
                };

            Ok(Some(Concept {
                id,
                fsn,
                preferred_term,
                synonyms,
                hierarchy: hier,
                hierarchy_path,
                parents,
                children_count,
                attributes,
            }))
        }
    }
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    loop {
        terminal.draw(|f| render(f, app))?;

        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(key) = event::read()? {
                handle_key(app, key.code, key.modifiers);
            }
        } else {
            // Debounce: fire search after 150 ms idle
            app.run_search();
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    // Ctrl-C always quits
    if modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('c') {
        app.should_quit = true;
        return;
    }

    if app.searching {
        match code {
            KeyCode::Esc => {
                app.searching = false;
                app.focus = Focus::Search;
            }
            KeyCode::Enter => {
                app.searching = false;
                app.run_search();
                app.focus = Focus::Search;
            }
            KeyCode::Char(c) => app.search_query.push(c),
            KeyCode::Backspace => {
                app.search_query.pop();
            }
            _ => {}
        }
        return;
    }

    match code {
        KeyCode::Char('q') | KeyCode::Char('Q') => app.should_quit = true,

        KeyCode::Char('/') => {
            app.searching = true;
            app.focus = Focus::Search;
        }

        KeyCode::Tab => {
            app.focus = match app.focus {
                Focus::Hierarchy => Focus::Search,
                Focus::Search => Focus::Detail,
                Focus::Detail => Focus::Hierarchy,
            };
        }

        KeyCode::Left => {
            app.focus = match app.focus {
                Focus::Detail => Focus::Search,
                _ => Focus::Hierarchy,
            };
        }
        KeyCode::Right => {
            app.focus = match app.focus {
                Focus::Hierarchy => Focus::Search,
                _ => Focus::Detail,
            };
        }

        KeyCode::Up => match app.focus {
            Focus::Hierarchy => list_up(&mut app.hierarchy_state, app.hierarchies.len()),
            Focus::Search => list_up(&mut app.results_state, app.search_results.len()),
            Focus::Detail => app.detail_scroll = app.detail_scroll.saturating_sub(1),
        },
        KeyCode::Down => match app.focus {
            Focus::Hierarchy => list_down(&mut app.hierarchy_state, app.hierarchies.len()),
            Focus::Search => list_down(&mut app.results_state, app.search_results.len()),
            Focus::Detail => app.detail_scroll += 1,
        },
        KeyCode::PageUp => {
            if app.focus == Focus::Detail {
                app.detail_scroll = app.detail_scroll.saturating_sub(10);
            }
        }
        KeyCode::PageDown => {
            if app.focus == Focus::Detail {
                app.detail_scroll += 10;
            }
        }

        KeyCode::Enter => match app.focus {
            Focus::Hierarchy => {
                if let Some(i) = app.hierarchy_state.selected() {
                    if let Some(h) = app.hierarchies.get(i).cloned() {
                        app.load_hierarchy_concepts(h);
                        app.focus = Focus::Search;
                    }
                }
            }
            Focus::Search => {
                if let Some(i) = app.results_state.selected() {
                    if let Some(r) = app.search_results.get(i).cloned() {
                        app.load_concept(r.id);
                        app.focus = Focus::Detail;
                    }
                }
            }
            Focus::Detail => {}
        },

        KeyCode::Char('b') => app.go_back(),
        KeyCode::Char('h') => app.focus = Focus::Hierarchy,

        _ => {}
    }
}

fn list_up(state: &mut ListState, len: usize) {
    if len == 0 {
        return;
    }
    let i = state.selected().unwrap_or(0);
    state.select(Some(if i == 0 { len - 1 } else { i - 1 }));
}

fn list_down(state: &mut ListState, len: usize) {
    if len == 0 {
        return;
    }
    let i = state.selected().unwrap_or(0);
    state.select(Some((i + 1) % len));
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

const NHS_BLUE: Color = Color::Rgb(0, 48, 135);
const DIM: Color = Color::Rgb(140, 140, 140);
const CYAN: Color = Color::Cyan;
const YELLOW: Color = Color::Yellow;

fn focused_border(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::White)
    } else {
        Style::default().fg(DIM)
    }
}

fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    // Title bar
    let title = Paragraph::new(
        "  sct — SNOMED CT Explorer        [/] search  [Tab] switch panel  [q] quit",
    )
    .style(Style::default().fg(Color::White).bg(NHS_BLUE));
    frame.render_widget(title, outer[0]);

    // Main: left 30 % + right 70 %
    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(outer[1]);

    // Left column: hierarchy (45 %) + search (55 %)
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(main[0]);

    render_hierarchy(frame, app, left[0]);
    render_search(frame, app, left[1]);
    render_detail(frame, app, main[1]);

    // Status bar
    let status = Paragraph::new(
        " [/] search  [↑↓] navigate  [Enter] select  [Tab] panels  [b] back  [q] quit",
    )
    .style(Style::default().fg(DIM).bg(Color::Rgb(20, 20, 30)));
    frame.render_widget(status, outer[2]);
}

fn render_hierarchy(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Hierarchy;
    let block = Block::default()
        .title(" HIERARCHY ")
        .borders(Borders::ALL)
        .border_style(focused_border(focused));

    let items: Vec<ListItem> = app
        .hierarchies
        .iter()
        .map(|h| ListItem::new(format!("  {}", h)))
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .fg(Color::White)
                .bg(NHS_BLUE)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(list, area, &mut app.hierarchy_state);
}

fn render_search(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Search;
    let searching = app.searching;

    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    // Input box
    let input_display = if searching {
        format!(" {}_", app.search_query)
    } else if app.search_query.is_empty() {
        "  press / to search...".to_string()
    } else {
        format!(" {}", app.search_query)
    };

    let input_block = Block::default()
        .title(" SEARCH ")
        .borders(Borders::ALL)
        .border_style(if searching {
            Style::default().fg(CYAN)
        } else {
            focused_border(focused)
        });

    frame.render_widget(
        Paragraph::new(input_display)
            .block(input_block)
            .style(if searching {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(DIM)
            }),
        parts[0],
    );

    // Results
    let title = if app.search_results.is_empty() {
        " RESULTS ".to_string()
    } else {
        format!(" RESULTS ({}) ", app.search_results.len())
    };
    let results_block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(focused_border(focused));

    let items: Vec<ListItem> = app
        .search_results
        .iter()
        .map(|r| {
            ListItem::new(Line::from(vec![
                Span::raw("  "),
                Span::styled(r.preferred_term.clone(), Style::default().fg(Color::White)),
                Span::raw(" "),
                Span::styled(
                    format!("[{}]", r.id),
                    Style::default().fg(CYAN),
                ),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(results_block)
        .highlight_style(
            Style::default()
                .fg(Color::White)
                .bg(NHS_BLUE)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(list, parts[1], &mut app.results_state);
}

fn render_detail(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Detail;
    let block = Block::default()
        .title(" CONCEPT DETAIL ")
        .borders(Borders::ALL)
        .border_style(focused_border(focused));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let concept = match &app.current_concept {
        None => {
            let hint = Paragraph::new(
                "\n  Select a concept from the results panel.\n\n  \
                 [/] to search  ·  select a hierarchy on the left to browse",
            )
            .style(Style::default().fg(DIM))
            .wrap(Wrap { trim: false });
            frame.render_widget(hint, inner);
            return;
        }
        Some(c) => c.clone(),
    };

    let width = inner.width as usize;
    let rule = "─".repeat(width.min(60));

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            concept.preferred_term.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(rule.clone(), Style::default().fg(DIM))),
        Line::from(""),
        Line::from(vec![
            Span::styled("SCTID:     ", Style::default().fg(DIM)),
            Span::styled(
                concept.id.clone(),
                Style::default().fg(CYAN).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("FSN:       ", Style::default().fg(DIM)),
            Span::raw(concept.fsn.clone()),
        ]),
        Line::from(vec![
            Span::styled("Hierarchy: ", Style::default().fg(DIM)),
            Span::styled(concept.hierarchy.clone(), Style::default().fg(YELLOW)),
        ]),
        Line::from(vec![
            Span::styled("Children:  ", Style::default().fg(DIM)),
            Span::raw(concept.children_count.to_string()),
        ]),
        Line::from(""),
    ];

    if !concept.synonyms.is_empty() {
        lines.push(Line::from(Span::styled(
            "SYNONYMS",
            Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            "────────────────────",
            Style::default().fg(DIM),
        )));
        for s in &concept.synonyms {
            lines.push(Line::from(vec![
                Span::styled("  • ", Style::default().fg(DIM)),
                Span::styled(s.clone(), Style::default().fg(YELLOW)),
            ]));
        }
        lines.push(Line::from(""));
    }

    if !concept.hierarchy_path.is_empty() {
        lines.push(Line::from(Span::styled(
            "HIERARCHY PATH",
            Style::default().fg(DIM).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            "────────────────────",
            Style::default().fg(DIM),
        )));
        let last = concept.hierarchy_path.len() - 1;
        for (i, item) in concept.hierarchy_path.iter().enumerate() {
            let indent = "  ".repeat(i);
            let connector = if i == last { "└─ " } else { "├─ " };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}{}", indent, connector),
                    Style::default().fg(DIM),
                ),
                Span::styled(
                    item.clone(),
                    if i == last {
                        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(DIM)
                    },
                ),
            ]));
        }
        lines.push(Line::from(""));
    }

    if !concept.parents.is_empty() {
        lines.push(Line::from(Span::styled(
            "PARENTS",
            Style::default().fg(DIM).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            "────────────────────",
            Style::default().fg(DIM),
        )));
        for (id, fsn) in &concept.parents {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::raw(fsn.clone()),
                Span::raw("  "),
                Span::styled(format!("[{}]", id), Style::default().fg(CYAN)),
            ]));
        }
        lines.push(Line::from(""));
    }

    if !concept.attributes.is_empty() {
        lines.push(Line::from(Span::styled(
            "ATTRIBUTES",
            Style::default().fg(DIM).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(Span::styled(
            "────────────────────",
            Style::default().fg(DIM),
        )));
        for (attr_name, values) in &concept.attributes {
            let label = attr_name.replace('_', " ");
            for (id, fsn) in values {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {}: ", label), Style::default().fg(DIM)),
                    Span::raw(fsn.clone()),
                    Span::raw("  "),
                    Span::styled(format!("[{}]", id), Style::default().fg(CYAN)),
                ]));
            }
        }
    }

    let para = Paragraph::new(Text::from(lines))
        .scroll((app.detail_scroll, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(para, inner);
}
