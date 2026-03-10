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
        // Draw one frame — ratatui clears and redraws the whole screen
        terminal.draw(|frame| render(frame, &dashboard))?;

        // Poll for input with a 50ms timeout → ~20 FPS refresh rate
        // If no key in 50ms, we loop back and redraw (so stats update live)
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                // Only react to actual key presses (not releases)
                if key.kind == KeyEventKind::Press && key.code == KeyCode::Char('q') {
                    return Ok(());
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

    // ── Outer layout: title bar | server boxes | request log ──────────────
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // title bar
            Constraint::Length(7),  // server boxes
            Constraint::Min(5),     // request log (fills remaining space)
        ])
        .split(area);

    let title_area   = sections[0];
    let servers_area = sections[1];
    let log_area     = sections[2];

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

    // ── Server boxes ───────────────────────────────────────────────────────
    // Split the servers row into N equal columns, one per backend
    let num_backends = dash.backends.len().max(1); // avoid divide-by-zero
    let server_columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, num_backends as u32); num_backends])
        .split(servers_area);

    for (i, backend) in dash.backends.iter().enumerate() {
        // "Active" = a request hit this backend in the last 300ms
        let is_active = backend
            .last_hit
            .map(|t| t.elapsed() < Duration::from_millis(300))
            .unwrap_or(false);

        // Three states: DOWN (red) > ACTIVE (green) > idle (white)
        let border_style = match (backend.is_healthy, is_active) {
            (false, _)   => Style::default().fg(Color::Red),
            (true, true) => Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            (true, false)=> Style::default().fg(Color::White),
        };

        // Extract just the port for a clean label
        let label = backend.url.split(':').last().unwrap_or(&backend.url).to_string();

        let (status_text, status_color) = match (backend.is_healthy, is_active) {
            (false, _)   => ("🔴 DOWN",   Color::Red),
            (true, true) => ("🟢 ACTIVE", Color::Green),
            (true, false)=> ("⬜ idle",   Color::DarkGray),
        };

        // Show last-checked age so user can see how fresh the health info is
        let checked_ago = backend.last_checked
            .map(|t| format!("  checked {}s ago", t.elapsed().as_secs()))
            .unwrap_or_else(|| "  not checked yet".to_string());

        let content = vec![
            Line::from(format!("  Hits: {}", backend.request_count)),
            Line::from(Span::styled(
                format!("  {}", status_text),
                Style::default().fg(status_color),
            )),
            Line::from(Span::styled(
                checked_ago,
                Style::default().fg(Color::DarkGray),
            )),
        ];

        let server_widget = Paragraph::new(content)
            .block(
                Block::default()
                    .title(format!(" 🖥  :{} ", label))
                    .borders(Borders::ALL)
                    .border_style(border_style),
            );

        frame.render_widget(server_widget, server_columns[i]);
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
}