//! Terminator - High-performance TUI trading terminal
//!
//! Mouse-controlled, resizable panels for professional trading
//! Optimized for full-screen trading experience

mod command;
mod panels;
mod ui;
mod state;
mod network;
mod wallet;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    Terminal,
};
use std::io;

use crate::state::AppState;
use crate::ui::render_ui;

#[tokio::main]
async fn main() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut app = AppState::new();

    // Load wallet from pcli
    eprintln!("Loading wallet from pcli...");
    match wallet::Wallet::load().await {
        Ok(wallet) => {
            eprintln!("✓ Wallet loaded from {}", wallet.home);
            eprintln!("  FVK: {}", wallet.fvk());
            eprintln!("  gRPC: {}", wallet.grpc_url());

            // Connect to Penumbra using wallet's endpoint
            if let Err(e) = app.connect_penumbra_with_wallet(wallet).await {
                eprintln!("Warning: Failed to connect to Penumbra: {}", e);
                eprintln!("Running in offline mode with mock data...");
            } else {
                // Fetch initial balances
                eprintln!("Fetching balances...");
                app.update_balances().await;
                eprintln!("✓ {} assets found", app.balances.len());
            }
        }
        Err(e) => {
            eprintln!("Warning: Failed to load pcli wallet: {}", e);
            eprintln!("Running in offline mode with mock data...");
            eprintln!("Tip: Run 'pcli init' to set up your wallet");
        }
    }

    // Run the app
    let res = run_app(&mut terminal, &mut app).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {:?}", err);
    }

    Ok(())
}

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut AppState,
) -> Result<()> {
    loop {
        // Render UI
        terminal.draw(|f| render_ui(f, app))?;

        // Handle events
        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('r') => app.toggle_resize_mode(),
                        KeyCode::Tab => app.next_panel(),
                        _ => {}
                    }
                }
                Event::Mouse(mouse) => {
                    match mouse.kind {
                        MouseEventKind::Down(_) => {
                            app.handle_mouse_down(mouse.column, mouse.row);
                        }
                        MouseEventKind::Drag(_) => {
                            app.handle_mouse_drag(mouse.column, mouse.row);
                        }
                        MouseEventKind::Up(_) => {
                            app.handle_mouse_up();
                        }
                        _ => {}
                    }
                }
                Event::Resize(_, _) => {
                    // Terminal resized, will be handled on next render
                }
                _ => {}
            }
        }

        // Poll Penumbra updates
        app.poll_penumbra_updates().await;

        // Update market data (mock for now)
        app.update_market_data().await;
    }
}
