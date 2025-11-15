//! Event Mapper - converts terminal events to core events

use anyhow::Result;
use crossterm::event::{Event as TermEvent, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};

use crate::core::Event;

/// Maps terminal events to core events
pub struct EventMapper {
    // Track state for multi-event interactions
}

impl EventMapper {
    pub fn new() -> Self {
        Self {}
    }
    
    /// Map terminal event to core event
    pub fn map_event(&mut self, term_event: TermEvent) -> Result<Event> {
        match term_event {
            // ===== Keyboard Events =====
            TermEvent::Key(key) => {
                // Only process Press events, ignore Repeat and Release
                if key.kind != KeyEventKind::Press {
                    return Ok(Event::Ignored);
                }

                // Ctrl+C should always quit
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    return Ok(Event::Quit);
                }

                // Ctrl+hjkl for vim-style navigation
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    return match key.code {
                        KeyCode::Char('h') => Ok(Event::FocusLeft),
                        KeyCode::Char('j') => Ok(Event::FocusDown),
                        KeyCode::Char('k') => Ok(Event::FocusUp),
                        KeyCode::Char('l') => Ok(Event::FocusRight),
                        _ => Ok(Event::Ignored),
                    };
                }

                match key.code {
                    KeyCode::Char('q') => Ok(Event::Quit),
                    KeyCode::Char('l') => Ok(Event::LimitOrderAtCursor),
                    KeyCode::Char('m') => Ok(Event::MarketOrder),
                    KeyCode::Char('r') => Ok(Event::ResizeModeToggled),
                    KeyCode::Char('v') => Ok(Event::ToggleRightPanelView),
                    KeyCode::Tab => Ok(Event::NextPanel),
                    KeyCode::Esc => Ok(Event::CancelAction),
                    _ => Ok(Event::Ignored),
                }
            }
            
            // ===== Mouse Events =====
            TermEvent::Mouse(mouse) => {
                match mouse.kind {
                    MouseEventKind::Moved => {
                        Ok(Event::MouseMove {
                            x: mouse.column,
                            y: mouse.row,
                        })
                    }

                    MouseEventKind::Down(MouseButton::Left) => {
                        Ok(Event::MouseDown {
                            x: mouse.column,
                            y: mouse.row,
                        })
                    }

                    MouseEventKind::Down(MouseButton::Right) => {
                        // Right-click opens context menu
                        Ok(Event::RightClick {
                            x: mouse.column,
                            y: mouse.row,
                        })
                    }

                    MouseEventKind::Drag(MouseButton::Left) => {
                        Ok(Event::MouseDrag {
                            x: mouse.column,
                            y: mouse.row,
                        })
                    }

                    MouseEventKind::Up(MouseButton::Left) => {
                        Ok(Event::MouseUp {
                            x: mouse.column,
                            y: mouse.row,
                        })
                    }

                    _ => Ok(Event::Ignored),
                }
            }
            
            // Resize
            TermEvent::Resize(_, _) => Ok(Event::Ignored),
            
            _ => Ok(Event::Ignored),
        }
    }
}
