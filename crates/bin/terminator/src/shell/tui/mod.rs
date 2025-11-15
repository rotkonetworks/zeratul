//! TUI Shell - Terminal UI implementation
//!
//! Maps terminal events to core Events and executes Effects

use anyhow::Result;
use crossterm::{
    event::{self, EnableMouseCapture, DisableMouseCapture, KeyboardEnhancementFlags,
            PushKeyboardEnhancementFlags, PopKeyboardEnhancementFlags},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    execute,
};
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    Terminal,
};
use std::io;
use std::time::Duration;

use crate::core::{AppCore, Event, Effect};
use crate::network::penumbra::grpc_client::PenumbraGrpcClient;
use crate::wallet::Wallet;

mod executor;
mod mapper;
mod renderer;

use executor::EffectExecutor;
use mapper::EventMapper;
use renderer::Renderer;

/// TUI Shell - manages terminal UI and core interaction
pub struct TuiShell {
    /// Core business logic
    core: AppCore,

    /// Terminal
    terminal: Terminal<CrosstermBackend<io::Stdout>>,

    /// Effect executor
    executor: EffectExecutor,

    /// Event mapper
    mapper: EventMapper,

    /// Renderer
    renderer: Renderer,

    /// Should exit
    should_exit: bool,
}

impl TuiShell {
    /// Create new TUI shell
    pub async fn new() -> Result<Self> {
        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            )
        )?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        
        // Create core
        let core = AppCore::new();
        
        // Try to load wallet
        let wallet = Wallet::load().await.ok();
        
        // Create executor with wallet
        let mut executor = EffectExecutor::new(wallet).await?;

        // Start streaming order book for default trading pair
        // TODO: Make trading pair configurable
        use penumbra_dex::TradingPair as PenumbraTradingPair;
        use penumbra_asset::asset;

        // For now, use placeholder asset IDs - these should be configured
        // We'll start the stream in the run loop instead

        Ok(Self {
            core,
            terminal,
            executor,
            mapper: EventMapper::new(),
            renderer: Renderer::new(),
            should_exit: false,
        })
    }
    
    /// Run the TUI shell
    pub async fn run(&mut self) -> Result<()> {
        loop {
            // Render current view model
            let view_model = self.core.view_model();
            self.terminal.draw(|f| self.renderer.render(f, &view_model))?;

            // Handle terminal events
            if event::poll(Duration::from_millis(100))? {
                let term_event = event::read()?;
                let mut core_event = self.mapper.map_event(term_event)?;

                // Convert MouseDown in chart area to ChartClicked with price
                if let Event::MouseDown { x, y } = core_event {
                    if let Some(chart_area) = self.renderer.last_chart_area {
                        if x >= chart_area.x && x < chart_area.x + chart_area.width &&
                           y >= chart_area.y && y < chart_area.y + chart_area.height {
                            // Mouse click is in chart area - calculate price from Y position
                            let price = self.calculate_price_from_y(y, chart_area, &view_model);
                            core_event = Event::ChartClicked { price, x, y };
                        }
                    }
                }

                // Update core
                let effects = self.core.update(core_event);

                // Execute effects
                for effect in effects {
                    self.execute_effect(effect).await?;
                }
            }
            
            // Poll for network updates
            self.executor.poll_updates(&mut self.core).await?;
            
            // Check exit
            if self.should_exit {
                break;
            }
        }
        
        // Cleanup
        self.cleanup()?;
        Ok(())
    }
    
    /// Execute a single effect
    async fn execute_effect(&mut self, effect: Effect) -> Result<()> {
        match &effect {
            Effect::Exit => {
                self.should_exit = true;
            }
            _ => {
                self.executor.execute(effect).await?;
            }
        }
        Ok(())
    }
    
    /// Calculate price from Y position in chart area
    fn calculate_price_from_y(&self, y: u16, chart_area: Rect, view_model: &crate::core::ViewModel) -> f64 {
        // Constants for chart configuration
        const PRICE_PADDING_RATIO: f64 = 0.5;  // 50% padding on each side of spread
        const DEFAULT_MIN_PRICE: f64 = 2500.0;
        const DEFAULT_MAX_PRICE: f64 = 3500.0;

        // Get relative position within chart (0.0 = top, 1.0 = bottom)
        let relative_y = (y.saturating_sub(chart_area.y)) as f64 / chart_area.height.max(1) as f64;

        // Get price range from order book
        let (min_price, max_price) = if let (Some(best_bid), Some(best_ask)) = (view_model.best_bid, view_model.best_ask) {
            // Use order book spread with configurable padding
            let spread = (best_ask - best_bid).max(0.01); // Ensure non-zero spread
            let padding = spread * PRICE_PADDING_RATIO;
            (best_bid - padding, best_ask + padding)
        } else {
            // No order book data - use default range
            (DEFAULT_MIN_PRICE, DEFAULT_MAX_PRICE)
        };

        // Map Y position to price (inverted: top = high price, bottom = low price)
        let price = max_price - (relative_y * (max_price - min_price));

        price
    }

    /// Cleanup terminal
    fn cleanup(&mut self) -> Result<()> {
        disable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            PopKeyboardEnhancementFlags,
        )?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

impl Drop for TuiShell {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}
