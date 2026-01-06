//! p2p table discovery and joining
//!
//! interfaces with poker-p2p crate for:
//! - creating tables with word codes
//! - joining tables by code
//! - managing peer connections

#![allow(dead_code)]

use bevy::prelude::*;
use parity_scale_codec::{Decode, Encode};

/// P2P networking plugin
pub struct P2PPlugin;

impl Plugin for P2PPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<P2PManager>()
            .add_event::<P2PCommand>()
            .add_event::<P2PNotification>()
            .add_systems(Update, process_p2p_commands);
    }
}

/// P2P manager resource
#[derive(Resource, Default)]
pub struct P2PManager {
    inner: Option<TableManager>,
}

impl P2PManager {
    pub fn new() -> Self {
        // generate random pubkey for now
        let pubkey: [u8; 32] = rand::random();
        Self {
            inner: Some(TableManager::new(pubkey)),
        }
    }

    pub fn state(&self) -> &P2pState {
        self.inner.as_ref().map(|m| m.state()).unwrap_or(&P2pState::Disconnected)
    }

    pub fn player_count(&self) -> usize {
        self.inner.as_ref().map(|m| m.player_count()).unwrap_or(0)
    }

    pub fn table_code(&self) -> Option<String> {
        match self.state() {
            P2pState::Publishing { code } => Some(code.to_string()),
            P2pState::WaitingForPlayers { code, .. } => Some(code.to_string()),
            P2pState::Connected { .. } => self.inner.as_ref().and_then(|m| {
                m.rules.as_ref().map(|_| "connected".to_string())
            }),
            _ => None,
        }
    }
}

/// commands to the P2P system
#[derive(Event, Clone, Debug)]
pub enum P2PCommand {
    CreateTable { rules: TableRules },
    JoinTable { code: String },
    Leave,
}

/// notifications from the P2P system
#[derive(Event, Clone, Debug)]
pub enum P2PNotification {
    TableCreated { code: String },
    PlayerJoined { seat: u8 },
    JoinedTable { seat: u8 },
    ReadyToStart,
    Error { message: String },
}

fn process_p2p_commands(
    mut commands: EventReader<P2PCommand>,
    mut manager: ResMut<P2PManager>,
    mut notifications: EventWriter<P2PNotification>,
) {
    for cmd in commands.read() {
        match cmd {
            P2PCommand::CreateTable { rules } => {
                if let Some(m) = &mut manager.inner {
                    let code = m.create_table(rules.clone());
                    // simulate instant table creation for demo
                    m.handle_event(TableEvent::TableCreated { code: code.clone() });
                    notifications.send(P2PNotification::TableCreated {
                        code: code.to_string(),
                    });
                    info!("p2p: created table with code {}", code);
                }
            }
            P2PCommand::JoinTable { code } => {
                if let Some(m) = &mut manager.inner {
                    m.join_table(code, Role::Player);
                    // simulate instant join for demo
                    m.handle_event(TableEvent::JoinedTable {
                        role: Role::Player,
                        seat: Some(2),
                    });
                    notifications.send(P2PNotification::JoinedTable { seat: 2 });
                    info!("p2p: joined table {}", code);
                }
            }
            P2PCommand::Leave => {
                if let Some(m) = &mut manager.inner {
                    m.leave_table();
                    info!("p2p: left table");
                }
            }
        }
    }
}

/// table code (like "42-bison-lamp")
#[derive(Clone, Debug)]
pub struct TableCode(pub String);

impl TableCode {
    pub fn new(code: String) -> Self {
        Self(code)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TableCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// table rules for p2p negotiation
#[derive(Clone, Debug, Encode, Decode)]
pub struct TableRules {
    pub seats: u8,
    pub small_blind: u128,
    pub big_blind: u128,
    pub min_buy_in: u128,
    pub max_buy_in: u128,
    pub ante: u128,
    pub allow_spectators: bool,
    pub max_spectators: u8,
}

impl Default for TableRules {
    fn default() -> Self {
        Self {
            seats: 9,
            small_blind: 5_000_000_000,      // 0.005
            big_blind: 10_000_000_000,       // 0.01
            min_buy_in: 100_000_000_000,     // 0.1
            max_buy_in: 1_000_000_000_000,   // 1.0
            ante: 0,
            allow_spectators: true,
            max_spectators: 10,
        }
    }
}

impl TableRules {
    /// training mode - free play
    pub fn training() -> Self {
        Self {
            seats: 2,
            small_blind: 5,
            big_blind: 10,
            min_buy_in: 100,
            max_buy_in: 1000,
            ante: 0,
            allow_spectators: true,
            max_spectators: 10,
        }
    }

    /// heads-up cash game
    pub fn heads_up(big_blind: u128) -> Self {
        Self {
            seats: 2,
            small_blind: big_blind / 2,
            big_blind,
            min_buy_in: big_blind * 20,
            max_buy_in: big_blind * 100,
            ..Default::default()
        }
    }
}

/// participant role
#[derive(Clone, Copy, Debug, PartialEq, Eq, Encode, Decode)]
pub enum Role {
    Player,
    Spectator,
}

/// p2p connection state
#[derive(Clone, Debug)]
pub enum P2pState {
    Disconnected,
    Publishing { code: TableCode },
    WaitingForPlayers { code: TableCode, count: u8 },
    Resolving { code: TableCode },
    Connecting,
    Authenticating,
    Connected { role: Role, seat: Option<u8> },
    Error(String),
}

impl Default for P2pState {
    fn default() -> Self {
        Self::Disconnected
    }
}

/// p2p events
#[derive(Clone, Debug)]
pub enum TableEvent {
    /// table created, share this code
    TableCreated { code: TableCode },
    /// player joined the table
    PlayerJoined { seat: u8, pubkey: [u8; 32] },
    /// spectator joined
    SpectatorJoined { pubkey: [u8; 32] },
    /// successfully joined table
    JoinedTable { role: Role, seat: Option<u8> },
    /// table is ready to start
    ReadyToStart,
    /// participant disconnected
    ParticipantLeft { pubkey: [u8; 32] },
    /// error occurred
    Error(String),
}

/// p2p table manager
pub struct TableManager {
    state: P2pState,
    rules: Option<TableRules>,
    my_pubkey: [u8; 32],
    participants: Vec<ParticipantInfo>,
}

#[derive(Clone, Debug)]
pub struct ParticipantInfo {
    pub pubkey: [u8; 32],
    pub role: Role,
    pub seat: Option<u8>,
    pub is_ready: bool,
}

impl TableManager {
    pub fn new(my_pubkey: [u8; 32]) -> Self {
        Self {
            state: P2pState::Disconnected,
            rules: None,
            my_pubkey,
            participants: Vec::new(),
        }
    }

    pub fn state(&self) -> &P2pState {
        &self.state
    }

    pub fn rules(&self) -> Option<&TableRules> {
        self.rules.as_ref()
    }

    pub fn participants(&self) -> &[ParticipantInfo] {
        &self.participants
    }

    pub fn player_count(&self) -> usize {
        self.participants.iter().filter(|p| p.role == Role::Player).count()
    }

    pub fn spectator_count(&self) -> usize {
        self.participants.iter().filter(|p| p.role == Role::Spectator).count()
    }

    /// create a new table and get a code
    pub fn create_table(&mut self, rules: TableRules) -> TableCode {
        // generate word code
        let code = generate_code();
        self.rules = Some(rules);
        self.state = P2pState::Publishing { code: code.clone() };
        code
    }

    /// join existing table by code
    pub fn join_table(&mut self, code: &str, role: Role) {
        let code = TableCode::new(code.to_string());
        self.state = P2pState::Resolving { code };
        // actual P2P connection would happen here
    }

    /// leave current table
    pub fn leave_table(&mut self) {
        self.state = P2pState::Disconnected;
        self.rules = None;
        self.participants.clear();
    }

    /// process incoming event
    pub fn handle_event(&mut self, event: TableEvent) {
        match event {
            TableEvent::TableCreated { code } => {
                self.state = P2pState::WaitingForPlayers { code, count: 0 };
            }
            TableEvent::PlayerJoined { seat, pubkey } => {
                self.participants.push(ParticipantInfo {
                    pubkey,
                    role: Role::Player,
                    seat: Some(seat),
                    is_ready: false,
                });
                if let P2pState::WaitingForPlayers { code, count } = &self.state {
                    self.state = P2pState::WaitingForPlayers {
                        code: code.clone(),
                        count: count + 1,
                    };
                }
            }
            TableEvent::SpectatorJoined { pubkey } => {
                self.participants.push(ParticipantInfo {
                    pubkey,
                    role: Role::Spectator,
                    seat: None,
                    is_ready: true,
                });
            }
            TableEvent::JoinedTable { role, seat } => {
                self.state = P2pState::Connected { role, seat };
            }
            TableEvent::ParticipantLeft { pubkey } => {
                self.participants.retain(|p| p.pubkey != pubkey);
            }
            TableEvent::Error(e) => {
                self.state = P2pState::Error(e);
            }
            TableEvent::ReadyToStart => {
                // mark all ready
                for p in &mut self.participants {
                    p.is_ready = true;
                }
            }
        }
    }

    /// check if table is ready to start
    pub fn is_ready(&self) -> bool {
        self.player_count() >= 2 && self.participants.iter().all(|p| p.is_ready)
    }
}

/// generate random table code
fn generate_code() -> TableCode {
    // simplified version - real impl uses PGP wordlist
    use std::time::{SystemTime, UNIX_EPOCH};

    let words = [
        "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel",
        "india", "juliet", "kilo", "lima", "mike", "november", "oscar", "papa",
        "quebec", "romeo", "sierra", "tango", "uniform", "victor", "whiskey", "xray",
        "yankee", "zulu", "zero", "one", "two", "three", "four", "five",
    ];

    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let n = (seed % 100) as u8;
    let w1 = words[(seed / 100 % 32) as usize];
    let w2 = words[(seed / 3200 % 32) as usize];

    TableCode(format!("{}-{}-{}", n, w1, w2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_table_manager_create() {
        let mut manager = TableManager::new([0u8; 32]);
        let code = manager.create_table(TableRules::training());

        assert!(matches!(manager.state(), P2pState::Publishing { .. }));
        assert!(!code.as_str().is_empty());
    }

    #[test]
    fn test_table_manager_join_events() {
        let mut manager = TableManager::new([0u8; 32]);
        manager.create_table(TableRules::training());

        // simulate table created
        manager.handle_event(TableEvent::TableCreated {
            code: TableCode::new("42-alpha-bravo".to_string()),
        });

        assert!(matches!(manager.state(), P2pState::WaitingForPlayers { .. }));

        // player joins
        manager.handle_event(TableEvent::PlayerJoined {
            seat: 1,
            pubkey: [1u8; 32],
        });

        assert_eq!(manager.player_count(), 1);

        // another player joins
        manager.handle_event(TableEvent::PlayerJoined {
            seat: 2,
            pubkey: [2u8; 32],
        });

        assert_eq!(manager.player_count(), 2);
    }

    #[test]
    fn test_code_generation() {
        let code1 = generate_code();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let code2 = generate_code();

        // codes should be different (most of the time)
        assert!(!code1.as_str().is_empty());
        assert!(!code2.as_str().is_empty());
    }
}
