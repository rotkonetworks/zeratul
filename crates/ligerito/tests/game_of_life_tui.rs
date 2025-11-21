//! Full TUI Game of Life with Real-Time Proving
//!
//! Features:
//! - Mouse input to toggle cells
//! - Continuous execution (auto-step)
//! - Background proof generation
//! - Live proof stats display
//! - Two gliders battle initial state

#![cfg(feature = "polkavm-integration")]

use polkavm_pcvm::polkavm_constraints_v2::{ProvenTransition, InstructionProof};
use polkavm_pcvm::polkavm_adapter::PolkaVMRegisters;
use polkavm_pcvm::polkavm_prover::{prove_polkavm_execution, verify_polkavm_proof};
use ligerito::configs::{hardcoded_config_20, hardcoded_config_20_verifier};
use ligerito_binary_fields::{BinaryElem32, BinaryElem128};
use ligerito::transcript::{Sha256Transcript, Transcript};
use ligerito::data_structures::{ProverConfig, VerifierConfig};
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use polkavm::program::Instruction;
use polkavm_common::program::{RawReg, Reg};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame, Terminal,
};
use std::io;

fn raw_reg(r: Reg) -> RawReg {
    RawReg::from(r)
}

/// Game of Life grid (32x32 for better visualization)
#[derive(Debug, Clone)]
struct Grid {
    width: usize,
    height: usize,
    cells: Vec<u32>,
}

impl Grid {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            cells: vec![0u32; width * height],
        }
    }

    fn get(&self, x: usize, y: usize) -> u32 {
        if x < self.width && y < self.height {
            self.cells[y * self.width + x]
        } else {
            0
        }
    }

    fn set(&mut self, x: usize, y: usize, value: u32) {
        if x < self.width && y < self.height {
            self.cells[y * self.width + x] = value;
        }
    }

    fn toggle(&mut self, x: usize, y: usize) {
        if x < self.width && y < self.height {
            let idx = y * self.width + x;
            self.cells[idx] = 1 - self.cells[idx];
        }
    }

    fn count_neighbors(&self, x: usize, y: usize) -> u32 {
        let mut count = 0;
        for dy in [-1i32, 0, 1] {
            for dx in [-1i32, 0, 1] {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = (x as i32 + dx).rem_euclid(self.width as i32) as usize;
                let ny = (y as i32 + dy).rem_euclid(self.height as i32) as usize;
                count += self.get(nx, ny);
            }
        }
        count
    }

    fn step(&self) -> Self {
        let mut next = Self::new(self.width, self.height);
        for y in 0..self.height {
            for x in 0..self.width {
                let alive = self.get(x, y) == 1;
                let neighbors = self.count_neighbors(x, y);
                let next_alive = if alive {
                    neighbors == 2 || neighbors == 3
                } else {
                    neighbors == 3
                };
                next.set(x, y, if next_alive { 1 } else { 0 });
            }
        }
        next
    }

    /// Load two gliders on collision course
    fn load_glider_battle(&mut self) {
        // Glider 1 (top-left, moving down-right)
        self.set(5, 5, 1);
        self.set(6, 6, 1);
        self.set(4, 7, 1);
        self.set(5, 7, 1);
        self.set(6, 7, 1);

        // Glider 2 (bottom-right, moving up-left)
        self.set(26, 26, 1);
        self.set(25, 25, 1);
        self.set(27, 24, 1);
        self.set(26, 24, 1);
        self.set(25, 24, 1);
    }
}

/// Proof statistics
#[derive(Debug, Clone)]
struct ProofStats {
    total_proofs: usize,
    total_generations: usize,
    total_steps: usize,
    last_proof_time: Duration,
    last_verify_time: Duration,
    avg_proof_time: Duration,
    proving_in_progress: bool,
}

impl ProofStats {
    fn new() -> Self {
        Self {
            total_proofs: 0,
            total_generations: 0,
            total_steps: 0,
            last_proof_time: Duration::ZERO,
            last_verify_time: Duration::ZERO,
            avg_proof_time: Duration::ZERO,
            proving_in_progress: false,
        }
    }
}

/// Simulate PolkaVM execution trace for one generation
fn simulate_generation(
    grid_before: &Grid,
    _grid_after: &Grid,
    pc_start: u32,
    regs_start: [u32; 13],
    memory_root: [u8; 32],
) -> (Vec<(ProvenTransition, Instruction)>, [u32; 13]) {
    let mut trace = Vec::new();
    let mut pc = pc_start;
    let mut regs = regs_start;

    // Simulate reading each cell (simplified trace generation)
    for cell_idx in 0..(grid_before.width * grid_before.height) {
        let mut regs_after = regs;
        regs_after[7] = grid_before.cells[cell_idx];

        let step = (
            ProvenTransition {
                pc,
                next_pc: pc + 2,
                instruction_size: 2,
                regs_before: PolkaVMRegisters::from_array(regs),
                regs_after: PolkaVMRegisters::from_array(regs_after),
                memory_root_before: memory_root,
                memory_root_after: memory_root,
                memory_proof: None,
                instruction_proof: InstructionProof {
                    merkle_path: vec![],
                    position: 0,
                    opcode: 0,
                    operands: [0, 0, 0],
                },
            },
            Instruction::load_imm(raw_reg(Reg::A0), grid_before.cells[cell_idx]),
        );

        trace.push(step);
        regs = regs_after;
        pc += 2;
    }

    (trace, regs)
}

/// Application state
struct App {
    grid: Grid,
    generation: usize,
    trace: Vec<(ProvenTransition, Instruction)>,
    pc: u32,
    regs: [u32; 13],
    memory_root: [u8; 32],
    prover_config: ProverConfig<BinaryElem32, BinaryElem32>,
    verifier_config: VerifierConfig,
    stats: Arc<Mutex<ProofStats>>,
    auto_step: bool,
    step_delay_ms: u64,
    last_step_time: Instant,
    cursor_x: usize,
    cursor_y: usize,
    paused: bool,
}

impl App {
    fn new(width: usize, height: usize) -> Self {
        let mut grid = Grid::new(width, height);
        grid.load_glider_battle();

        Self {
            grid,
            generation: 0,
            trace: Vec::new(),
            pc: 0x1000,
            regs: [0u32; 13],
            memory_root: [0u8; 32],
            prover_config: hardcoded_config_20(PhantomData, PhantomData),
            verifier_config: hardcoded_config_20_verifier(),
            stats: Arc::new(Mutex::new(ProofStats::new())),
            auto_step: true,
            step_delay_ms: 100,
            last_step_time: Instant::now(),
            cursor_x: width / 2,
            cursor_y: height / 2,
            paused: false,
        }
    }

    fn execute_step(&mut self) {
        let grid_next = self.grid.step();

        let (gen_trace, final_regs) = simulate_generation(
            &self.grid,
            &grid_next,
            self.pc,
            self.regs,
            self.memory_root,
        );

        self.pc += (gen_trace.len() * 2) as u32;
        self.trace.extend(gen_trace);
        self.regs = final_regs;

        self.grid = grid_next;
        self.generation += 1;
    }

    fn prove_async(&mut self) {
        if self.trace.is_empty() {
            return;
        }

        let trace = self.trace.clone();
        let stats = Arc::clone(&self.stats);
        let prover_config = self.prover_config.clone();
        let verifier_config = self.verifier_config.clone();
        let memory_root = self.memory_root;
        let generation = self.generation;

        // Mark proving in progress
        {
            let mut stats = stats.lock().unwrap();
            stats.proving_in_progress = true;
        }

        // Clear trace (will be proven in background)
        self.trace.clear();
        self.regs = [0u32; 13];

        // Spawn background proving thread
        std::thread::spawn(move || {
            let program_commitment = [0x47u8; 32];
            let mut challenge_transcript = Sha256Transcript::new(42);
            let program_elems: Vec<BinaryElem32> = program_commitment
                .chunks(4)
                .map(|chunk| {
                    let mut bytes = [0u8; 4];
                    bytes.copy_from_slice(chunk);
                    BinaryElem32::from(u32::from_le_bytes(bytes))
                })
                .collect();
            challenge_transcript.absorb_elems(&program_elems);
            challenge_transcript.absorb_elem(BinaryElem32::from(trace.len() as u32));
            let batching_challenge = challenge_transcript.get_challenge::<BinaryElem128>();

            let transcript = Sha256Transcript::new(42);

            // Prove
            let prove_start = Instant::now();
            let proof = prove_polkavm_execution(
                &trace,
                program_commitment,
                batching_challenge,
                &prover_config,
                transcript,
            );
            let prove_time = prove_start.elapsed();

            if let Ok(proof) = proof {
                // Verify
                let verify_start = Instant::now();
                let _verified = verify_polkavm_proof(
                    &proof,
                    program_commitment,
                    memory_root,
                    memory_root,
                    &verifier_config,
                );
                let verify_time = verify_start.elapsed();

                // Update stats
                let mut stats = stats.lock().unwrap();
                stats.total_proofs += 1;
                stats.total_generations = generation;
                stats.total_steps += trace.len();
                stats.last_proof_time = prove_time;
                stats.last_verify_time = verify_time;
                stats.avg_proof_time = Duration::from_millis(
                    ((stats.avg_proof_time.as_millis() * (stats.total_proofs - 1) as u128
                        + prove_time.as_millis())
                        / stats.total_proofs as u128) as u64,
                );
                stats.proving_in_progress = false;
            } else {
                let mut stats = stats.lock().unwrap();
                stats.proving_in_progress = false;
            }
        });
    }
}

fn ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),     // Game grid
            Constraint::Length(12),  // Stats panel
        ])
        .split(f.area());

    // Game grid
    render_grid(f, chunks[0], app);

    // Stats panel
    render_stats(f, chunks[1], app);
}

fn render_grid(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            " Game of Life - Generation {} {} ",
            app.generation,
            if app.paused { "[PAUSED]" } else { "" }
        ))
        .title_alignment(Alignment::Center);

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Calculate cell size
    let cell_width = 2;
    let cell_height = 1;

    let grid_width = (inner.width as usize) / cell_width;
    let grid_height = (inner.height as usize) / cell_height;

    let visible_width = grid_width.min(app.grid.width);
    let visible_height = grid_height.min(app.grid.height);

    // Render cells
    for y in 0..visible_height {
        let mut line_spans = Vec::new();
        for x in 0..visible_width {
            let is_cursor = x == app.cursor_x && y == app.cursor_y;
            let is_alive = app.grid.get(x, y) == 1;

            let (symbol, style) = if is_cursor {
                if is_alive {
                    ("██", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
                } else {
                    ("··", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
                }
            } else if is_alive {
                ("██", Style::default().fg(Color::Cyan))
            } else {
                ("··", Style::default().fg(Color::DarkGray))
            };

            line_spans.push(Span::styled(symbol, style));
        }

        let line = Line::from(line_spans);
        let paragraph = Paragraph::new(line);
        let cell_area = Rect {
            x: inner.x,
            y: inner.y + y as u16,
            width: inner.width,
            height: 1,
        };
        f.render_widget(paragraph, cell_area);
    }
}

fn render_stats(f: &mut Frame, area: Rect, app: &App) {
    let stats = app.stats.lock().unwrap();

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Proof Statistics ")
        .title_alignment(Alignment::Center);

    let proving_status = if stats.proving_in_progress {
        Span::styled(" [PROVING...] ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
    } else {
        Span::styled(" [READY] ", Style::default().fg(Color::Green))
    };

    let text = vec![
        Line::from(vec![
            Span::raw("Status: "),
            proving_status,
        ]),
        Line::from(format!("Total Proofs: {}", stats.total_proofs)),
        Line::from(format!("Total Generations: {}", stats.total_generations)),
        Line::from(format!("Total Steps: {}", stats.total_steps)),
        Line::from(format!("Pending Steps: {}", app.trace.len())),
        Line::from(format!("Last Proof: {:?}", stats.last_proof_time)),
        Line::from(format!("Last Verify: {:?}", stats.last_verify_time)),
        Line::from(format!("Avg Proof Time: {:?}", stats.avg_proof_time)),
        Line::from(""),
        Line::from(vec![
            Span::styled("[Space]", Style::default().fg(Color::Cyan)),
            Span::raw(" Toggle Pause  "),
            Span::styled("[P]", Style::default().fg(Color::Cyan)),
            Span::raw(" Prove Now  "),
            Span::styled("[C]", Style::default().fg(Color::Cyan)),
            Span::raw(" Clear  "),
            Span::styled("[Q]", Style::default().fg(Color::Cyan)),
            Span::raw(" Quit"),
        ]),
    ];

    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, area);
}

fn run_tui(mut app: App) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut last_auto_prove = Instant::now();
    let auto_prove_interval = Duration::from_secs(3); // Prove every 3 seconds

    loop {
        terminal.draw(|f| ui(f, &app))?;

        // Auto-step if enabled and not paused
        if app.auto_step && !app.paused && app.last_step_time.elapsed() > Duration::from_millis(app.step_delay_ms) {
            app.execute_step();
            app.last_step_time = Instant::now();
        }

        // Auto-prove every 3 seconds if we have accumulated steps
        if !app.trace.is_empty() && last_auto_prove.elapsed() > auto_prove_interval {
            let stats = app.stats.lock().unwrap();
            if !stats.proving_in_progress {
                drop(stats);
                app.prove_async();
                last_auto_prove = Instant::now();
            }
        }

        // Handle events with timeout
        if event::poll(Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char(' ') => app.paused = !app.paused,
                    KeyCode::Char('p') | KeyCode::Char('P') => {
                        if !app.trace.is_empty() {
                            app.prove_async();
                        }
                    }
                    KeyCode::Char('c') | KeyCode::Char('C') => {
                        app.grid = Grid::new(app.grid.width, app.grid.height);
                        app.generation = 0;
                        app.trace.clear();
                        app.pc = 0x1000;
                        app.regs = [0u32; 13];
                    }
                    KeyCode::Char('g') | KeyCode::Char('G') => {
                        app.grid = Grid::new(app.grid.width, app.grid.height);
                        app.grid.load_glider_battle();
                        app.generation = 0;
                        app.trace.clear();
                        app.pc = 0x1000;
                        app.regs = [0u32; 13];
                    }
                    KeyCode::Up => app.cursor_y = app.cursor_y.saturating_sub(1),
                    KeyCode::Down => app.cursor_y = (app.cursor_y + 1).min(app.grid.height - 1),
                    KeyCode::Left => app.cursor_x = app.cursor_x.saturating_sub(1),
                    KeyCode::Right => app.cursor_x = (app.cursor_x + 1).min(app.grid.width - 1),
                    KeyCode::Enter => app.grid.toggle(app.cursor_x, app.cursor_y),
                    _ => {}
                }
            } else if let Event::Mouse(mouse) = event::read()? {
                match mouse.kind {
                    MouseEventKind::Down(_) => {
                        // Convert mouse position to grid coordinates
                        // Account for border (1) and cell size (2 chars wide)
                        if mouse.column > 0 && mouse.row > 0 {
                            let grid_x = ((mouse.column - 1) / 2) as usize;
                            let grid_y = (mouse.row - 1) as usize;
                            if grid_x < app.grid.width && grid_y < app.grid.height {
                                app.grid.toggle(grid_x, grid_y);
                                app.cursor_x = grid_x;
                                app.cursor_y = grid_y;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // Cleanup
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

#[test]
#[ignore] // Run with: cargo test --release --features polkavm-integration --test game_of_life_tui -- --ignored --nocapture
fn test_game_of_life_tui() {
    println!("🎮 Starting Full TUI Game of Life with Real-Time Proving...");
    println!("================================================\n");

    let app = App::new(32, 32);

    if let Err(e) = run_tui(app) {
        eprintln!("Error running TUI: {}", e);
    }

    println!("\n✅ TUI session complete!");
}
