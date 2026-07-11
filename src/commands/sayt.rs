// SPDX-FileCopyrightText: 2026 Marcus Baw and Baw Medical Ltd
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sct sayt` - search-as-you-type over the mmap'd FST index.
//!
//! Two surfaces, one shared core ([`Index::search_typeahead`]):
//!   - **default** - an interactive terminal UI that repaints on every
//!     keystroke (requires the `tui` Cargo feature).
//!   - **`--stdio`** - a language-agnostic line protocol for embedding in
//!     another app: one query per line in, one JSON line of ranked hits out.
//!
//! The third surface, an HTTP `/autocomplete` endpoint, lives in `sct serve`.
//! All three call `search_typeahead` on the same index, so results match.

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

use crate::index::query::Index;

#[derive(Parser, Debug)]
pub struct Args {
    /// FST index produced by `sct fst build`.
    #[arg(long, default_value = "snomed.fst", value_parser = crate::paths::tilde_pathbuf)]
    pub index: PathBuf,

    /// Maximum number of results shown / returned.
    #[arg(long, short, default_value = "10")]
    pub limit: usize,

    /// Minimum query length before results are computed.
    #[arg(long, default_value = "1")]
    pub min_chars: usize,

    /// Enable typo-tolerant fuzzy fallback (a little slower, broader).
    #[arg(long)]
    pub fuzzy: bool,

    /// Machine mode: read one query per line on stdin and write one line of
    /// JSON (`{"query":..,"hits":[{id,display,score,tag}..]}`) per query on
    /// stdout. For embedding `sct` as a search backend in another program. No
    /// terminal UI is started, so this works in any build.
    #[arg(long)]
    pub stdio: bool,
}

pub fn run(args: Args) -> Result<()> {
    let index = Index::open(&args.index).with_context(|| {
        format!(
            "opening FST index {} - build one with `sct fst build`",
            args.index.display()
        )
    })?;

    if args.stdio {
        run_stdio(&index, &args)
    } else {
        run_interactive(&index, &args)
    }
}

/// Line protocol: each line of stdin is a query; each response is one line of
/// JSON. Output is flushed per query so a consumer gets results immediately.
/// Queries are processed in order and each is sub-millisecond, so there is no
/// in-flight request to cancel - the consumer simply reads the latest line.
fn run_stdio(index: &Index, args: &Args) -> Result<()> {
    use std::io::{BufRead, Write};

    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    let mut buf = String::new();
    let mut reader = stdin.lock();

    loop {
        buf.clear();
        let n = reader.read_line(&mut buf).context("reading stdin")?;
        if n == 0 {
            break; // EOF
        }
        let query = buf.trim_end_matches(['\n', '\r']);
        let hits = if query.trim().chars().count() >= args.min_chars {
            index.search_typeahead(query, args.limit, args.fuzzy)
        } else {
            Vec::new()
        };
        let payload = serde_json::json!({
            "query": query,
            "hits": hits.iter().map(|h| h.to_json()).collect::<Vec<_>>(),
        });
        writeln!(out, "{}", serde_json::to_string(&payload)?)?;
        out.flush()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Interactive TUI (requires the `tui` feature)
// ---------------------------------------------------------------------------

#[cfg(not(feature = "tui"))]
fn run_interactive(_index: &Index, _args: &Args) -> Result<()> {
    anyhow::bail!(
        "interactive search-as-you-type needs a build with the `tui` feature.\n\
         Rebuild with `cargo install sct-rs --features tui` (or `s/install --full`),\n\
         or use `sct sayt --stdio` for the machine-readable line protocol."
    )
}

#[cfg(feature = "tui")]
#[derive(Default)]
struct TuiState {
    query: String,
    hits: Vec<crate::index::query::Hit>,
    selected: usize,
    last_micros: u128,
}

#[cfg(feature = "tui")]
impl TuiState {
    /// Re-run the search for the current query, timing it for the latency
    /// readout. Called on every keystroke - it is sub-millisecond.
    fn research(&mut self, index: &Index, args: &Args) {
        self.selected = 0;
        if self.query.trim().chars().count() < args.min_chars {
            self.hits.clear();
            self.last_micros = 0;
            return;
        }
        let t0 = std::time::Instant::now();
        self.hits = index.search_typeahead(&self.query, args.limit, args.fuzzy);
        self.last_micros = t0.elapsed().as_micros();
    }
}

#[cfg(feature = "tui")]
fn run_interactive(index: &Index, args: &Args) -> Result<()> {
    use crossterm::{
        event::{self, Event, KeyCode, KeyModifiers},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    };
    use ratatui::{backend::CrosstermBackend, Terminal};
    use std::io;
    use std::time::Duration;

    let mut state = TuiState::default();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Loop returns the concept the user selected with Enter, if any.
    let selected: Option<(u64, String)> = loop {
        terminal.draw(|f| render_tui(f, index, &state))?;

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            break None;
        }
        match key.code {
            KeyCode::Esc => break None,
            KeyCode::Enter => {
                break state
                    .hits
                    .get(state.selected)
                    .map(|h| (h.concept_id, h.term.clone()));
            }
            KeyCode::Up => state.selected = state.selected.saturating_sub(1),
            KeyCode::Down => {
                if state.selected + 1 < state.hits.len() {
                    state.selected += 1;
                }
            }
            KeyCode::Backspace => {
                state.query.pop();
                state.research(index, args);
            }
            KeyCode::Char(c) => {
                state.query.push(c);
                state.research(index, args);
            }
            _ => {}
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Emit the selection to stdout so `sct sayt` can feed a pipe, e.g.
    //   sct sayt | cut -f1 | sct codelist add mylist.codelist -
    if let Some((id, term)) = selected {
        println!("{id}\t{term}");
    }
    Ok(())
}

#[cfg(feature = "tui")]
fn render_tui(frame: &mut ratatui::Frame, index: &Index, state: &TuiState) {
    use ratatui::{
        layout::{Constraint, Direction, Layout},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    };

    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // search box
            Constraint::Min(1),    // results
            Constraint::Length(1), // status bar
        ])
        .split(area);

    // Search box, with a fake block-cursor at the end of the query.
    let search = Paragraph::new(Line::from(vec![
        Span::styled("› ", Style::default().fg(Color::Cyan)),
        Span::raw(state.query.as_str()),
        Span::styled("▏", Style::default().fg(Color::DarkGray)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" search-as-you-type "),
    );
    frame.render_widget(search, rows[0]);

    // Results list.
    let items: Vec<ListItem> = state
        .hits
        .iter()
        .map(|h| {
            let tag = h
                .semantic_tag
                .as_deref()
                .map(|t| format!("  ({t})"))
                .unwrap_or_default();
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:>17}", h.concept_id),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw("  "),
                Span::raw(h.term.clone()),
                Span::styled(tag, Style::default().fg(Color::Green)),
            ]))
        })
        .collect();
    let mut list_state = ListState::default();
    if !state.hits.is_empty() {
        list_state.select(Some(state.selected));
    }
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} results ", state.hits.len())),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(list, rows[1], &mut list_state);

    // Status bar: loaded edition, last search latency, and key hints.
    let latency = if state.last_micros > 0 {
        format!("{:.3} ms", state.last_micros as f64 / 1000.0)
    } else {
        "—".to_string()
    };
    let edition = index
        .provenance()
        .map(|p| p.edition_label.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "FST index".to_string());
    let status = Line::from(vec![
        Span::styled(
            format!(" {edition} "),
            Style::default().bg(Color::Blue).fg(Color::White),
        ),
        Span::raw(format!("  {latency}  ")),
        Span::styled(
            "↑↓ select · Enter emit · Esc quit",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(status), rows[2]);
}
