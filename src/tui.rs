use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
    Frame, Terminal,
};

use std::sync::atomic::Ordering;
use crate::dashboard::SharedDashboard;

// The 8 unicode block characters used for sparklines.
// Index 0 = shortest bar, 7 = tallest. We map normalised [0..1] latency → one of these.
const SPARK_CHARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Build a sparkline string from a latency history slice.
/// Each character represents one sample, normalised relative to the max in the window.
fn sparkline(history: &std::collections::VecDeque<u128>) -> String {
    if history.is_empty() {
        return "  no data".to_string();
    }
    let max = *history.iter().max().unwrap_or(&1);
    let max = max.max(1); // avoid divide-by-zero
    history.iter().map(|&v| {
        let idx = ((v as f64 / max as f64) * 7.0).round() as usize;
        SPARK_CHARS[idx.min(7)]
    }).collect()
}

/// Build an error-rate bar: e.g. "[████░░░░░░] 40%"
/// bar_width is the number of cells available for the filled/empty part.
fn error_bar(request_count: u64, error_count: u64, bar_width: usize) -> (String, Color) {
    if request_count == 0 {
        return (format!("[{}] n/a", "░".repeat(bar_width)), Color::DarkGray);
    }
    let pct = (error_count * 100) / request_count;
    let filled = ((pct as usize) * bar_width) / 100;
    let empty = bar_width - filled;
    let bar = format!("[{}{}] {}%", "█".repeat(filled), "░".repeat(empty), pct);
    let color = match pct {
        0 => Color::DarkGray,
        1..=10 => Color::Yellow,
        _ => Color::Red,
    };
    (bar, color)
}

/// Blocks until the user presses 'q'.
pub fn run_tui(dashboard: SharedDashboard) -> io::Result<()> {
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, dashboard);

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    dashboard: SharedDashboard,
) -> io::Result<()> {
    loop {
        terminal.draw(|frame| render(frame, &dashboard))?;

        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press { continue; }

                let mut dash = dashboard.lock().unwrap();

                // ── Search mode: route most keys to the query string ──────────
                // When search_mode is true, character keys append to the query,
                // Backspace removes the last char, Esc exits search mode.
                // Only '/' and 'q' are handled normally outside search mode.
                if dash.search_mode {
                    match key.code {
                        KeyCode::Esc => {
                            dash.search_mode = false;
                            dash.search_query.clear();
                        }
                        KeyCode::Backspace => { dash.search_query.pop(); }
                        KeyCode::Char(c) => { dash.search_query.push(c); }
                        _ => {}
                    }
                    continue;
                }

                // ── Normal mode ────────────────────────────────────────────────
                let count = dash.backends.len();
                match key.code {
                    KeyCode::Char('q') => return Ok(()),

                    // Tab cycles through the three views.
                    // Modulo 3 wraps back to 0 after tab 2.
                    KeyCode::Tab => {
                        dash.current_tab = (dash.current_tab + 1) % 3;
                    }

                    // '/' enters search mode — only meaningful on the log tab
                    KeyCode::Char('/') => {
                        dash.search_mode = true;
                        dash.search_query.clear();
                        dash.current_tab = 1; // jump to log tab so user sees the filter
                    }

                    // Backend navigation (overview tab)
                    KeyCode::Left => {
                        dash.selected_backend = dash.selected_backend.saturating_sub(1);
                    }
                    KeyCode::Right => {
                        if count > 0 {
                            dash.selected_backend = (dash.selected_backend + 1).min(count - 1);
                        }
                    }
                    KeyCode::Char('d') => {
                        let sel = dash.selected_backend;
                        if let Some(b) = dash.backends.get_mut(sel) {
                            b.manually_disabled = true;
                            let port = b.url.split(':').last().unwrap_or("?").to_string();
                            dash.status_msg = format!("⛔ :{} disabled", port);
                        }
                    }
                    KeyCode::Char('e') => {
                        let sel = dash.selected_backend;
                        if let Some(b) = dash.backends.get_mut(sel) {
                            b.manually_disabled = false;
                            let port = b.url.split(':').last().unwrap_or("?").to_string();
                            dash.status_msg = format!("✅ :{} re-enabled", port);
                        }
                    }
                    KeyCode::Char('p') => {
                        let sel = dash.selected_backend;
                        dash.pinned_backend = Some(sel);
                        let port = dash.backends.get(sel)
                            .and_then(|b| b.url.split(':').last().map(|s| s.to_string()))
                            .unwrap_or_else(|| "?".to_string());
                        dash.status_msg = format!("📌 pinned to :{}", port);
                    }
                    KeyCode::Char('u') => {
                        dash.pinned_backend = None;
                        dash.status_msg = "unpinned - back to round-robin".to_string();
                    }
                    _ => {}
                }
            }
        }
    }
}

fn render(frame: &mut Frame, dashboard: &SharedDashboard) {
    let dash = dashboard.lock().unwrap();
    let area = frame.area();

    // ── Title bar (always visible) ────────────────────────────────────────────
    let tabs_label = format!(
        " [1] Overview{}  [2] Log{}  [3] Config{} ",
        if dash.current_tab == 0 { " ◀" } else { "" },
        if dash.current_tab == 1 { " ◀" } else { "" },
        if dash.current_tab == 2 { " ◀" } else { "" },
    );
    let blocked = dash.blocked_requests.load(std::sync::atomic::Ordering::Relaxed);
    let blocked_label = if blocked > 0 { format!("  |  Blocked: {}", blocked) } else { String::new() };
    let title_text = if dash.status_msg.is_empty() {
        format!(
            " 🦀 ClawGate  |  Backends: {}  |  Req: {}{}  |{}",
            dash.backends.len(), dash.total_request, blocked_label, tabs_label,
        )
    } else {
        format!(
            " 🦀 ClawGate  |  Req: {}{}  |  {}  |{}",
            dash.total_request, blocked_label, dash.status_msg, tabs_label,
        )
    };

    // Split screen: title (3) + content (rest) + hint (1)
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(1)])
        .split(area);

    let title_area   = outer[0];
    let content_area = outer[1];
    let hint_area    = outer[2];

    let title = Paragraph::new(title_text)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, title_area);

    // ── Tab dispatch ──────────────────────────────────────────────────────────
    match dash.current_tab {
        0 => render_overview(frame, &dash, content_area),
        1 => render_log(frame, &dash, content_area),
        2 => render_config(frame, &dash, content_area),
        _ => {}
    }

    // ── Hint bar (always visible, context-sensitive) ───────────────────────
    let hint_text = match dash.current_tab {
        0 => " Tab: switch view  |  ← → move  |  d disable  |  e enable  |  p pin  |  u unpin  |  q quit",
        1 if dash.search_mode => &format!(" ESC: cancel  |  Backspace: delete  |  Searching: {}_", dash.search_query),
        1 => " Tab: switch view  |  /: search  |  q quit",
        2 => " Tab: switch view  |  q quit",
        _ => "",
    };
    let hint = Paragraph::new(hint_text.to_string())
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, hint_area);
}

// ── Tab 1: Server Overview ────────────────────────────────────────────────────
fn render_overview(frame: &mut Frame, dash: &crate::dashboard::DashboardState, area: ratatui::layout::Rect) {
    // Group backends by route label (same logic as before)
    let mut groups: Vec<(String, Vec<(usize, &crate::dashboard::BackendInfo)>)> = Vec::new();
    for (i, b) in dash.backends.iter().enumerate() {
        if let Some(group) = groups.iter_mut().find(|(lbl, _)| lbl == &b.route_label) {
            group.1.push((i, b));
        } else {
            groups.push((b.route_label.clone(), vec![(i, b)]));
        }
    }

    let num_groups = groups.len().max(1);
    // Each group: 1-line header + 11-line box (extra rows for sparkline + error bar)
    let mut vert_constraints: Vec<Constraint> = Vec::new();
    for _ in 0..num_groups {
        vert_constraints.push(Constraint::Length(1));
        vert_constraints.push(Constraint::Length(11));
    }
    vert_constraints.push(Constraint::Min(0)); // absorb slack

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vert_constraints)
        .split(area);

    let selected = dash.selected_backend;
    let pinned   = dash.pinned_backend;

    for (group_idx, (route_label, members)) in groups.iter().enumerate() {
        let header_area = sections[group_idx * 2];
        let boxes_area  = sections[group_idx * 2 + 1];

        let header_text = format!(" 🗂  {} ({} backend{}) ",
            route_label, members.len(),
            if members.len() == 1 { "" } else { "s" }
        );
        let group_header = Paragraph::new(header_text)
            .style(Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD));
        frame.render_widget(group_header, header_area);

        let n = members.len().max(1);
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![Constraint::Ratio(1, n as u32); n])
            .split(boxes_area);

        for (col, (global_idx, backend)) in members.iter().enumerate() {
            let i = *global_idx;
            let is_active   = backend.last_hit.map(|t| t.elapsed() < Duration::from_millis(300)).unwrap_or(false);
            let is_selected = i == selected;
            let is_pinned   = pinned == Some(i);

            let border_style = if is_selected && is_pinned {
                Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)
            } else if is_selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else if backend.manually_disabled {
                Style::default().fg(Color::DarkGray)
            } else {
                match (backend.is_healthy, is_active) {
                    (false, _)    => Style::default().fg(Color::Red),
                    (true, true)  => Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                    (true, false) => Style::default().fg(Color::White),
                }
            };

            let port_label = backend.url.split(':').last().unwrap_or(&backend.url).to_string();
            let box_title = if backend.manually_disabled { format!(" ⛔ :{} ", port_label) }
                else if is_pinned                        { format!(" 📌 :{} ", port_label) }
                else                                     { format!(" 🖥  :{} ", port_label) };

            let (status_text, status_color) = if backend.manually_disabled {
                ("⛔ disabled", Color::DarkGray)
            } else {
                match (backend.is_healthy, is_active) {
                    (false, _)    => ("🔴 DOWN",   Color::Red),
                    (true, true)  => ("🟢 ACTIVE", Color::Green),
                    (true, false) => ("⬜ idle",   Color::DarkGray),
                }
            };

            let checked_ago = backend.last_checked
                .map(|t| format!("  checked {}s ago", t.elapsed().as_secs()))
                .unwrap_or_else(|| "  not checked".to_string());

            let active_conn = backend.active_connections.load(Ordering::Relaxed);
            let conn_style  = if active_conn > 0 {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            // Sparkline: normalise history and render as unicode bar characters
            let spark = sparkline(&backend.latency_history);
            let spark_style = Style::default().fg(Color::Green);

            // Error rate bar: 10 cells wide
            let (err_bar, err_color) = error_bar(backend.request_count, backend.error_count, 10);

            let override_line = if is_pinned {
                Line::from(Span::styled("  📌 PINNED", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)))
            } else { Line::from("") };

            let content = vec![
                Line::from(Span::styled(format!("  Active: {}", active_conn), conn_style)),
                Line::from(format!("  Hits:   {}", backend.request_count)),
                Line::from(Span::styled(format!("  {}", status_text), Style::default().fg(status_color))),
                Line::from(Span::styled(checked_ago, Style::default().fg(Color::DarkGray))),
                // Sparkline row — shows latency trend over last 30 requests
                Line::from(vec![
                    Span::raw("  ms "),
                    Span::styled(spark, spark_style),
                ]),
                // Error rate bar row
                Line::from(Span::styled(format!("  5xx {}", err_bar), Style::default().fg(err_color))),
                override_line,
            ];

            let server_widget = Paragraph::new(content)
                .block(Block::default().title(box_title).borders(Borders::ALL).border_style(border_style));
            frame.render_widget(server_widget, columns[col]);
        }
    }
}

// ── Tab 2: Request Log (with search/filter) ───────────────────────────────────
fn render_log(frame: &mut Frame, dash: &crate::dashboard::DashboardState, area: ratatui::layout::Rect) {
    let query = dash.search_query.to_lowercase();

    // Filter rows if a search query is active.
    // We match against path OR backend URL — both are common things to grep for.
    let rows: Vec<Row> = dash.recent_request.iter()
        .filter(|log| {
            if query.is_empty() { return true; }
            log.path.to_lowercase().contains(&query)
                || log.backends.to_lowercase().contains(&query)
                || log.method.to_lowercase().contains(&query)
        })
        .map(|log| {
            let status_color = match log.status {
                200..=299 => Color::Green,
                400..=499 => Color::Yellow,
                500..=599 => Color::Red,
                _         => Color::White,
            };
            let short_id = log.request_id.chars().take(8).collect::<String>();
            Row::new([
                Cell::from(short_id).style(Style::default().fg(Color::DarkGray)),
                Cell::from(log.method.clone()),
                Cell::from(log.path.clone()),
                Cell::from(log.backends.clone()),
                Cell::from(log.status.to_string()).style(Style::default().fg(status_color)),
                Cell::from(format!("{}ms", log.duration_ms)),
            ])
        })
        .collect();

    let header_cells = ["Req-ID", "Method", "Path", "Backend", "Status", "Time (ms)"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1);

    // Build the block title — show search status when filtering
    let block_title = if dash.search_mode {
        format!(" Recent Requests — filter: {}_ ", dash.search_query)
    } else if !query.is_empty() {
        format!(" Recent Requests — filtered: \"{}\" (ESC to clear) ", query)
    } else {
        " Recent Requests ".to_string()
    };

    let log_table = Table::new(
        rows,
        [
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Min(20),
            Constraint::Length(22),
            Constraint::Length(7),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(block_title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );

    frame.render_widget(log_table, area);
}

// ── Tab 3: Config View ────────────────────────────────────────────────────────
// Shows current runtime state — balancing mode, backend count, health interval.
// Read-only: no config.yaml parsing here, just what's in DashboardState.
fn render_config(frame: &mut Frame, dash: &crate::dashboard::DashboardState, area: ratatui::layout::Rect) {
    let lines: Vec<Line> = vec![
        Line::from(Span::styled("  Runtime Configuration", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(format!("  Backends registered : {}", dash.backends.len())),
        Line::from(format!("  Health check interval: {}s", dash.health_check_interval_secs)),
        Line::from(format!("  Total requests served: {}", dash.total_request)),
        Line::from(""),
        Line::from(Span::styled("  Backend URLs:", Style::default().fg(Color::Yellow))),
    ]
    .into_iter()
    .chain(dash.backends.iter().map(|b| {
        let state = if b.manually_disabled { "⛔ disabled" }
            else if !b.is_healthy { "🔴 down" }
            else { "🟢 up" };
        Line::from(format!("    {} — {} (hits: {})", b.url, state, b.request_count))
    }))
    .collect();

    let config_widget = Paragraph::new(lines)
        .block(Block::default()
            .title(" Config View ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)));
    frame.render_widget(config_widget, area);
}