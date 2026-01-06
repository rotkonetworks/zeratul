//! chat system for poker table
//!
//! table chat (visible to all), private messages, typing indicators

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use std::collections::VecDeque;

use poker_p2p::{ChatMessage, PrivateMessage, TypingIndicator};

/// max messages to keep in history
const MAX_CHAT_HISTORY: usize = 100;

/// max messages visible in chat panel
const VISIBLE_MESSAGES: usize = 20;

pub struct ChatPlugin;

impl Plugin for ChatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChatState>()
            .add_event::<ChatEvent>()
            .add_systems(
                Update,
                (handle_chat_input, process_chat_events, render_chat_panel).chain(),
            );
    }
}

/// chat events for network send/receive
#[derive(Event, Clone, Debug)]
pub enum ChatEvent {
    /// send table chat
    SendTable(String),
    /// send private message
    SendPrivate { to: String, content: String },
    /// received table chat from network
    ReceivedTable(ChatMessage),
    /// received private message from network
    ReceivedPrivate(PrivateMessage),
    /// player typing status changed
    TypingChanged(TypingIndicator),
}

/// local chat message for display
#[derive(Clone, Debug)]
pub struct DisplayMessage {
    pub seat: u8,
    pub sender: String,
    pub content: String,
    pub timestamp: u64,
    pub is_private: bool,
    pub is_system: bool,
}

impl From<&ChatMessage> for DisplayMessage {
    fn from(msg: &ChatMessage) -> Self {
        Self {
            seat: msg.seat,
            sender: msg.sender.clone(),
            content: msg.content.clone(),
            timestamp: msg.timestamp,
            is_private: false,
            is_system: false,
        }
    }
}

/// chat state resource
#[derive(Resource)]
pub struct ChatState {
    /// table chat history (ring buffer)
    pub table_messages: VecDeque<DisplayMessage>,
    /// private message history per player
    pub private_messages: Vec<(String, VecDeque<DisplayMessage>)>,
    /// current input text
    pub input: String,
    /// is chat panel visible
    pub visible: bool,
    /// is chat panel minimized (just shows last msg)
    pub minimized: bool,
    /// is input focused
    pub input_focused: bool,
    /// current PM target (None = table chat)
    pub pm_target: Option<String>,
    /// players currently typing
    pub typing: Vec<(u8, String)>,
    /// scroll to bottom on next frame
    pub scroll_to_bottom: bool,
    /// local player seat
    pub local_seat: u8,
    /// local player name
    pub local_name: String,
    /// unread message count (when minimized)
    pub unread_count: u32,
}

impl Default for ChatState {
    fn default() -> Self {
        Self {
            table_messages: VecDeque::with_capacity(MAX_CHAT_HISTORY),
            private_messages: Vec::new(),
            input: String::new(),
            visible: true,
            minimized: false,
            input_focused: false,
            pm_target: None,
            typing: Vec::new(),
            scroll_to_bottom: false,
            local_seat: 0,
            local_name: "You".to_string(),
            unread_count: 0,
        }
    }
}

impl ChatState {
    /// add a message to table chat
    pub fn add_table_message(&mut self, msg: DisplayMessage) {
        if self.table_messages.len() >= MAX_CHAT_HISTORY {
            self.table_messages.pop_front();
        }
        self.table_messages.push_back(msg);
        self.scroll_to_bottom = true;
        if self.minimized {
            self.unread_count += 1;
        }
    }

    /// add a system message (join/leave, game events)
    pub fn add_system_message(&mut self, content: String) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        self.add_table_message(DisplayMessage {
            seat: 255,
            sender: "".to_string(),
            content,
            timestamp,
            is_private: false,
            is_system: true,
        });
    }

    /// set typing indicator for a player
    pub fn set_typing(&mut self, seat: u8, name: String, is_typing: bool) {
        self.typing.retain(|(s, _)| *s != seat);
        if is_typing {
            self.typing.push((seat, name));
        }
    }

    /// get visible messages (last N)
    pub fn visible_messages(&self) -> impl Iterator<Item = &DisplayMessage> {
        let start = self.table_messages.len().saturating_sub(VISIBLE_MESSAGES);
        self.table_messages.iter().skip(start)
    }

    /// parse /pm command from input
    pub fn parse_pm_command(&self) -> Option<(String, String)> {
        if self.input.starts_with("/pm ") || self.input.starts_with("/msg ") {
            let rest = if self.input.starts_with("/pm ") {
                &self.input[4..]
            } else {
                &self.input[5..]
            };
            let mut parts = rest.splitn(2, ' ');
            if let (Some(target), Some(content)) = (parts.next(), parts.next()) {
                return Some((target.to_string(), content.to_string()));
            }
        }
        None
    }
}

/// handle keyboard input for chat
fn handle_chat_input(keyboard: Res<ButtonInput<KeyCode>>, mut chat_state: ResMut<ChatState>) {
    // toggle chat with Enter when not focused
    if keyboard.just_pressed(KeyCode::Enter) && !chat_state.input_focused {
        chat_state.visible = !chat_state.visible;
        if chat_state.visible {
            chat_state.minimized = false;
            chat_state.unread_count = 0;
        }
    }

    // toggle minimize with Tab
    if keyboard.just_pressed(KeyCode::Tab) && chat_state.visible {
        chat_state.minimized = !chat_state.minimized;
        if !chat_state.minimized {
            chat_state.unread_count = 0;
        }
    }

    // escape to close/unfocus
    if keyboard.just_pressed(KeyCode::Escape) {
        if chat_state.input_focused {
            chat_state.input_focused = false;
        } else if chat_state.visible {
            chat_state.minimized = true;
        }
    }
}

/// process chat events from network
fn process_chat_events(mut events: EventReader<ChatEvent>, mut chat_state: ResMut<ChatState>) {
    for event in events.read() {
        match event {
            ChatEvent::ReceivedTable(msg) => {
                chat_state.add_table_message(msg.into());
            }
            ChatEvent::TypingChanged(indicator) => {
                // would need player name lookup
                let name = format!("Player {}", indicator.seat);
                chat_state.set_typing(indicator.seat, name, indicator.is_typing);
            }
            _ => {}
        }
    }
}

/// render chat panel UI
fn render_chat_panel(
    mut contexts: EguiContexts,
    mut chat_state: ResMut<ChatState>,
    mut chat_events: EventWriter<ChatEvent>,
) {
    if !chat_state.visible {
        return;
    }

    let ctx = contexts.ctx_mut();

    // position in bottom-left
    egui::Window::new("chat")
        .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(10.0, -10.0))
        .resizable(true)
        .collapsible(false)
        .title_bar(false)
        .default_width(300.0)
        .default_height(if chat_state.minimized { 40.0 } else { 200.0 })
        .show(ctx, |ui| {
            if chat_state.minimized {
                render_minimized_chat(ui, &mut chat_state);
            } else {
                render_full_chat(ui, &mut chat_state, &mut chat_events);
            }
        });
}

fn render_minimized_chat(ui: &mut egui::Ui, chat_state: &mut ChatState) {
    ui.horizontal(|ui| {
        // show last message preview
        if let Some(last) = chat_state.table_messages.back() {
            let preview = if last.is_system {
                format!("[{}]", truncate(&last.content, 30))
            } else {
                format!("{}: {}", last.sender, truncate(&last.content, 25))
            };
            ui.label(egui::RichText::new(preview).small().weak());
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if chat_state.unread_count > 0 {
                ui.label(
                    egui::RichText::new(format!("({})", chat_state.unread_count))
                        .small()
                        .color(egui::Color32::YELLOW),
                );
            }
        });
    });
}

fn render_full_chat(
    ui: &mut egui::Ui,
    chat_state: &mut ChatState,
    chat_events: &mut EventWriter<ChatEvent>,
) {
    // header with close/minimize buttons
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("chat").strong());

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("_").clicked() {
                chat_state.minimized = true;
            }
        });
    });

    ui.separator();

    // message list (scrollable)
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .max_height(150.0)
        .show(ui, |ui| {
            for msg in chat_state.visible_messages() {
                render_message(ui, msg);
            }
        });

    // typing indicators
    if !chat_state.typing.is_empty() {
        let typing_text = chat_state
            .typing
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        ui.label(
            egui::RichText::new(format!("{} typing...", typing_text))
                .small()
                .weak()
                .italics(),
        );
    }

    ui.separator();

    // input field
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut chat_state.input)
                .hint_text(if chat_state.pm_target.is_some() {
                    format!("pm to {}...", chat_state.pm_target.as_ref().unwrap())
                } else {
                    "type message...".to_string()
                })
                .desired_width(ui.available_width() - 50.0),
        );

        chat_state.input_focused = response.has_focus();

        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            send_message(chat_state, chat_events);
        }

        if ui.button("send").clicked() {
            send_message(chat_state, chat_events);
        }
    });
}

fn render_message(ui: &mut egui::Ui, msg: &DisplayMessage) {
    ui.horizontal(|ui| {
        if msg.is_system {
            ui.label(
                egui::RichText::new(&msg.content)
                    .small()
                    .italics()
                    .color(egui::Color32::GRAY),
            );
        } else if msg.is_private {
            ui.label(
                egui::RichText::new(format!("[PM] {}", msg.sender))
                    .small()
                    .strong()
                    .color(egui::Color32::from_rgb(200, 100, 200)),
            );
            ui.label(egui::RichText::new(&msg.content).small());
        } else {
            // seat color based on position
            let color = seat_color(msg.seat);
            ui.label(
                egui::RichText::new(format!("{}:", msg.sender))
                    .small()
                    .strong()
                    .color(color),
            );
            ui.label(egui::RichText::new(&msg.content).small());
        }
    });
}

fn send_message(chat_state: &mut ChatState, chat_events: &mut EventWriter<ChatEvent>) {
    let input = chat_state.input.trim().to_string();
    if input.is_empty() {
        return;
    }

    // check for /pm command
    if let Some((target, content)) = chat_state.parse_pm_command() {
        chat_events.send(ChatEvent::SendPrivate {
            to: target,
            content,
        });
    } else if let Some(target) = &chat_state.pm_target {
        // sending in PM mode
        chat_events.send(ChatEvent::SendPrivate {
            to: target.clone(),
            content: input.clone(),
        });
    } else {
        // regular table chat
        chat_events.send(ChatEvent::SendTable(input.clone()));

        // add to local history immediately (optimistic)
        let msg = DisplayMessage {
            seat: chat_state.local_seat,
            sender: chat_state.local_name.clone(),
            content: input,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            is_private: false,
            is_system: false,
        };
        chat_state.add_table_message(msg);
    }

    chat_state.input.clear();
}

fn seat_color(seat: u8) -> egui::Color32 {
    match seat % 9 {
        0 => egui::Color32::from_rgb(100, 200, 100), // green (you)
        1 => egui::Color32::from_rgb(100, 150, 255), // blue
        2 => egui::Color32::from_rgb(255, 150, 100), // orange
        3 => egui::Color32::from_rgb(255, 100, 150), // pink
        4 => egui::Color32::from_rgb(150, 100, 255), // purple
        5 => egui::Color32::from_rgb(255, 255, 100), // yellow
        6 => egui::Color32::from_rgb(100, 255, 255), // cyan
        7 => egui::Color32::from_rgb(255, 200, 150), // peach
        _ => egui::Color32::from_rgb(200, 200, 200), // gray (spectators)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max.saturating_sub(3)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pm_command() {
        let mut state = ChatState::default();

        state.input = "/pm Alice hello there".to_string();
        let result = state.parse_pm_command();
        assert_eq!(
            result,
            Some(("Alice".to_string(), "hello there".to_string()))
        );

        state.input = "/msg Bob how are you".to_string();
        let result = state.parse_pm_command();
        assert_eq!(result, Some(("Bob".to_string(), "how are you".to_string())));

        state.input = "regular message".to_string();
        assert!(state.parse_pm_command().is_none());
    }

    #[test]
    fn test_add_messages() {
        let mut state = ChatState::default();

        for i in 0..150 {
            state.add_table_message(DisplayMessage {
                seat: 0,
                sender: "test".to_string(),
                content: format!("msg {}", i),
                timestamp: i as u64,
                is_private: false,
                is_system: false,
            });
        }

        // should cap at MAX_CHAT_HISTORY
        assert_eq!(state.table_messages.len(), MAX_CHAT_HISTORY);
        // oldest messages should be dropped
        assert!(state
            .table_messages
            .front()
            .unwrap()
            .content
            .ends_with("50"));
    }

    #[test]
    fn test_typing_indicator() {
        let mut state = ChatState::default();

        state.set_typing(1, "Alice".to_string(), true);
        state.set_typing(2, "Bob".to_string(), true);
        assert_eq!(state.typing.len(), 2);

        state.set_typing(1, "Alice".to_string(), false);
        assert_eq!(state.typing.len(), 1);
        assert_eq!(state.typing[0].1, "Bob");
    }
}
