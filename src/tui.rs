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

/// Entry point — call this from main() after spawning the axum server.
/// Blocks until the user presses 'q'.
pub fn run_tui(dashboard: SharedDashboard) -> io::Result<()> {
    // Set up terminal: switch to alternate screen (hides normal shell output)
    // and enable raw mode (keypresses go directly to us, not line-buffered)
    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    // Run the loop — cleanup happens whether we return Ok or Err
    let result = event_loop(&mut terminal, dashboard);

    // Always restore the terminal, even if the loop errored
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
                let count = dash.backends.len();

                match key.code {
                    KeyCode::Char('q') => return Ok(()),

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
                        dash.status_msg = "unpinned — back to round-robin".to_string();
                    }
                    _ => {}
                }
            }
        }
    }
}

fn render(frame: &mut Frame, dashboard: &SharedDashboard) {
    // Lock the dashboard to take a snapshot of the data for this frame
    // The lock is released at the end of this function
    let dash = dashboard.lock().unwrap();

    let area = frame.area();

    // ── Server boxes — grouped by route label ──────────────────────────────
    // Step 1: collect groups preserving config order (first-seen label wins order).
    // Each group is (label, Vec<(global_index, &BackendInfo)>).
    let mut groups: Vec<(String, Vec<(usize, &_)>)> = Vec::new();
    for (i, b) in dash.backends.iter().enumerate() {
        if let Some(group) = groups.iter_mut().find(|(lbl, _)| lbl == &b.route_label) {
            group.1.push((i, b));
        } else {
            groups.push((b.route_label.clone(), vec![(i, b)]));
        }
    }

    // Step 2: build vertical constraints — each group needs 1 (header) + 9 (boxes) = 10 rows.
    // Whatever remains goes to the request log.
    let num_groups = groups.len().max(1);
    let mut vert_constraints: Vec<Constraint> = vec![
        Constraint::Length(3),                            // title bar
    ];
    for _ in 0..num_groups {
        vert_constraints.push(Constraint::Length(1));     // route header label
        vert_constraints.push(Constraint::Length(9));     // backend box row
    }
    vert_constraints.push(Constraint::Min(5));            // request log
    vert_constraints.push(Constraint::Length(1));         // footer

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vert_constraints)
        .split(area);

    let title_area = sections[0];
    // groups occupy sections[1..1+num_groups*2], two slots each
    let log_area   = sections[1 + num_groups * 2];
    let hint_area  = sections[2 + num_groups * 2];

    // ── Title bar ──────────────────────────────────────────────────────────
    let title_text = if dash.status_msg.is_empty() {
        format!(
            " 🦀 ClawGate  |  Backends: {}  |  Total Requests: {}  |  Press 'q' to quit ",
            dash.backends.len(),
            dash.total_request,
        )
    } else {
        format!(
            " 🦀 ClawGate  |  Backends: {}  |  Total Requests: {}  |  {}  |  Press 'q' to quit ",
            dash.backends.len(),
            dash.total_request,
            dash.status_msg,
        )
    };
    let title = Paragraph::new(title_text)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(title, title_area);

    let selected = dash.selected_backend;
    let pinned   = dash.pinned_backend;

    // Step 3: render each group
    for (group_idx, (route_label, members)) in groups.iter().enumerate() {
        let header_area = sections[1 + group_idx * 2];
        let boxes_area  = sections[2 + group_idx * 2];

        // ── Route group header ─────────────────────────────────────────────
        let header_text = format!(" 🗂  {} ({} backend{}) ",
            route_label,
            members.len(),
            if members.len() == 1 { "" } else { "s" }
        );
        let group_header = Paragraph::new(header_text)
            .style(Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD));
        frame.render_widget(group_header, header_area);

        // ── Backend boxes in this group ────────────────────────────────────
        let n = members.len().max(1);
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![Constraint::Ratio(1, n as u32); n])
            .split(boxes_area);

        for (col, (global_idx, backend)) in members.iter().enumerate() {
            let i = *global_idx;
            let is_active  = backend.last_hit.map(|t| t.elapsed() < Duration::from_millis(300)).unwrap_or(false);
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
            let box_title = if backend.manually_disabled {
                format!(" ⛔ :{} ", port_label)
            } else if is_pinned {
                format!(" 📌 :{} ", port_label)
            } else {
                format!(" 🖥  :{} ", port_label)
            };

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
                .unwrap_or_else(|| "  not checked yet".to_string());

            let override_line = if is_pinned {
                Line::from(Span::styled("  📌 PINNED", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)))
            } else {
                Line::from("")
            };

            let weight_style = if backend.weight > 1 {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let active_conn = backend.active_connections.load(Ordering::Relaxed);
            let conn_style  = if active_conn > 0 {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let content = vec![
                Line::from(Span::styled(format!("  Weight: {}", backend.weight), weight_style)),
                Line::from(Span::styled(format!("  Active:  {}", active_conn), conn_style)),
                Line::from(format!("  Hits:   {}", backend.request_count)),
                Line::from(Span::styled(format!("  {}", status_text), Style::default().fg(status_color))),
                Line::from(Span::styled(checked_ago, Style::default().fg(Color::DarkGray))),
                override_line,
            ];

            let server_widget = Paragraph::new(content)
                .block(Block::default().title(box_title).borders(Borders::ALL).border_style(border_style));

            frame.render_widget(server_widget, columns[col]);
        }
    }

    // ── Request log ────────────────────────────────────────────────────────
    let header_cells = ["Method", "Path", "Backend", "Status", "Time (ms)"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1);

    let rows: Vec<Row> = dash
        .recent_request
        .iter()
        .map(|log| {
            // Colour the status code: 2xx green, 4xx yellow, 5xx red
            let status_color = match log.status {
                200..=299 => Color::Green,
                400..=499 => Color::Yellow,
                500..=599 => Color::Red,
                _ => Color::White,
            };

            Row::new([
                Cell::from(log.method.clone()),
                Cell::from(log.path.clone()),
                Cell::from(log.backends.clone()),
                Cell::from(log.status.to_string()).style(Style::default().fg(status_color)),
                Cell::from(format!("{}ms", log.duration_ms)),
            ])
        })
        .collect();

    let log_table = Table::new(
        rows,
        [
            Constraint::Length(8),    // Method
            Constraint::Min(20),      // Path  (stretches)
            Constraint::Length(22),   // Backend URL
            Constraint::Length(7),    // Status
            Constraint::Length(10),   // Time
        ],
    )
    .header(header)
    .block(
        Block::default()
            .title(" Recent Requests ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );

    frame.render_widget(log_table, log_area);

    // ── Key hint footer ────────────────────────────────────────────────────
    let hint = Paragraph::new(" ← → move  |  d disable  |  e enable  |  p pin  |  u unpin  |  q quit")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, hint_area);
}