use crossterm::terminal::{enable_raw_mode, disable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;

enable_raw_mode()?;
std::io::stdout().execute(EnterAlternateScreen)?;
let mut terminal = ratatui::Terminal::new(CrosstermBackend::new(std::io::stdout()))?;

loop {
    // 1. Draw a frame
    terminal.draw(|frame| render(frame, &dashboard))?;
    
    // 2. Check for input with a 50ms timeout (20 FPS)
    if crossterm::event::poll(Duration::from_millis(50))? {
        if let Event::Key(key) = crossterm::event::read()? {
            if key.code == KeyCode::Char('q') {
                break;  // exit the TUI
            }
        }
    }
}