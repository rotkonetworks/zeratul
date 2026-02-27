//! friends and playmate history system
//!
//! tracks players you've played with for easy invites and social features:
//! - automatic tracking when you play at a table
//! - friend list with nicknames and notes
//! - game history and stats per playmate
//! - quick invite to new tables

use bevy::prelude::*;
use parity_scale_codec::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[cfg(not(target_arch = "wasm32"))]
use dirs;

pub struct FriendsPlugin;

impl Plugin for FriendsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FriendsState>()
            .init_resource::<PlaymateStorageHandle>()
            .add_event::<FriendsEvent>()
            .add_systems(Startup, load_playmates_on_startup)
            .add_systems(Update, (
                handle_friends_events,
                auto_track_playmates,
                auto_save_playmates,
            ));
    }
}

/// handle to playmate storage (initialized after auth)
#[derive(Resource, Default)]
pub struct PlaymateStorageHandle {
    #[cfg(not(target_arch = "wasm32"))]
    pub storage: Option<crate::storage::FilePlaymateStorage>,
    pub loaded: bool,
}

/// unique identifier for a player (derived from their account)
#[derive(Clone, Debug, PartialEq, Eq, Hash, Encode, Decode, Serialize, Deserialize)]
pub struct PlayerId(pub String);

impl PlayerId {
    pub fn from_pubkey(pubkey: &[u8; 32]) -> Self {
        Self(hex::encode(&pubkey[..16])) // use first 16 bytes for shorter id
    }

    pub fn from_address(address: &str) -> Self {
        // normalize address (remove 0x prefix, lowercase)
        let normalized = address
            .strip_prefix("0x")
            .unwrap_or(address)
            .to_lowercase();
        Self(normalized[..32.min(normalized.len())].to_string())
    }

    pub fn short(&self) -> String {
        if self.0.len() > 8 {
            format!("{}...", &self.0[..8])
        } else {
            self.0.clone()
        }
    }
}

/// a player you've played with
#[derive(Clone, Debug, Encode, Decode, Serialize, Deserialize)]
pub struct Playmate {
    /// unique identifier
    pub id: PlayerId,
    /// display name (from chat or user-set nickname)
    pub display_name: String,
    /// user-set nickname (overrides display_name in UI)
    pub nickname: Option<String>,
    /// user notes about this player
    pub notes: String,
    /// is this player a friend (favorited)?
    pub is_friend: bool,
    /// is this player blocked?
    pub is_blocked: bool,
    /// their session pubkey (most recent)
    pub last_pubkey: Option<[u8; 32]>,
    /// first time played together (unix timestamp)
    pub first_met: u64,
    /// last time played together
    pub last_played: u64,
    /// total games played together
    pub games_together: u32,
    /// hands won against this player
    pub hands_won: u32,
    /// hands lost to this player
    pub hands_lost: u32,
    /// total chips won from this player (can be negative)
    pub chips_balance: i64,
    /// recent table encounters
    pub encounters: Vec<TableEncounter>,
}

impl Playmate {
    pub fn new(id: PlayerId, display_name: String) -> Self {
        let now = current_timestamp();
        Self {
            id,
            display_name,
            nickname: None,
            notes: String::new(),
            is_friend: false,
            is_blocked: false,
            last_pubkey: None,
            first_met: now,
            last_played: now,
            games_together: 0,
            hands_won: 0,
            hands_lost: 0,
            chips_balance: 0,
            encounters: Vec::new(),
        }
    }

    /// get the name to display (nickname if set, otherwise display_name)
    pub fn name(&self) -> &str {
        self.nickname.as_deref().unwrap_or(&self.display_name)
    }

    /// win rate against this player (0.0 - 1.0)
    pub fn win_rate(&self) -> f32 {
        let total = self.hands_won + self.hands_lost;
        if total == 0 {
            0.5
        } else {
            self.hands_won as f32 / total as f32
        }
    }

    /// record a new encounter at a table
    pub fn record_encounter(&mut self, table_code: String, stakes: String) {
        self.last_played = current_timestamp();
        self.games_together += 1;

        // keep last 20 encounters
        if self.encounters.len() >= 20 {
            self.encounters.remove(0);
        }

        self.encounters.push(TableEncounter {
            table_code,
            stakes,
            timestamp: self.last_played,
            result_chips: 0,
            hands_played: 0,
        });
    }

    /// update the most recent encounter with game results
    pub fn update_last_encounter(&mut self, hands: u32, chips: i64, won: u32, lost: u32) {
        if let Some(enc) = self.encounters.last_mut() {
            enc.hands_played = hands;
            enc.result_chips = chips;
        }
        self.hands_won += won;
        self.hands_lost += lost;
        self.chips_balance += chips;
    }
}

/// record of playing at a table together
#[derive(Clone, Debug, Encode, Decode, Serialize, Deserialize)]
pub struct TableEncounter {
    /// table code (e.g. "42-alpha-bravo")
    pub table_code: String,
    /// stakes (e.g. "1/2")
    pub stakes: String,
    /// when the game started
    pub timestamp: u64,
    /// chips won/lost this session
    pub result_chips: i64,
    /// hands played together
    pub hands_played: u32,
}

/// friends and playmate state
#[derive(Resource, Default)]
pub struct FriendsState {
    /// all known playmates indexed by id
    pub playmates: HashMap<PlayerId, Playmate>,
    /// current view in friends panel
    pub view: FriendsView,
    /// selected playmate for details
    pub selected: Option<PlayerId>,
    /// search/filter text
    pub search: String,
    /// pending invite (playmate id, table code)
    pub pending_invite: Option<(PlayerId, String)>,
    /// is friends panel open?
    pub panel_open: bool,
    /// dirty flag for saving
    pub needs_save: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FriendsView {
    #[default]
    RecentPlaymates,
    Friends,
    Blocked,
    Stats,
}

impl FriendsState {
    /// get friends (favorited playmates)
    pub fn friends(&self) -> Vec<&Playmate> {
        let mut friends: Vec<_> = self.playmates.values()
            .filter(|p| p.is_friend && !p.is_blocked)
            .collect();
        friends.sort_by(|a, b| b.last_played.cmp(&a.last_played));
        friends
    }

    /// get recent playmates (sorted by last played)
    pub fn recent(&self) -> Vec<&Playmate> {
        let mut recent: Vec<_> = self.playmates.values()
            .filter(|p| !p.is_blocked)
            .collect();
        recent.sort_by(|a, b| b.last_played.cmp(&a.last_played));
        recent.truncate(50); // limit to 50 recent
        recent
    }

    /// get blocked players
    pub fn blocked(&self) -> Vec<&Playmate> {
        self.playmates.values()
            .filter(|p| p.is_blocked)
            .collect()
    }

    /// search playmates by name
    pub fn search_playmates(&self, query: &str) -> Vec<&Playmate> {
        let query = query.to_lowercase();
        self.playmates.values()
            .filter(|p| {
                p.name().to_lowercase().contains(&query) ||
                p.display_name.to_lowercase().contains(&query) ||
                p.id.0.contains(&query)
            })
            .collect()
    }

    /// get or create a playmate
    pub fn get_or_create(&mut self, id: PlayerId, name: String) -> &mut Playmate {
        if !self.playmates.contains_key(&id) {
            self.playmates.insert(id.clone(), Playmate::new(id.clone(), name));
            self.needs_save = true;
        }
        self.playmates.get_mut(&id).unwrap()
    }

    /// add a friend
    pub fn add_friend(&mut self, id: &PlayerId) {
        if let Some(p) = self.playmates.get_mut(id) {
            p.is_friend = true;
            p.is_blocked = false;
            self.needs_save = true;
        }
    }

    /// remove friend status
    pub fn remove_friend(&mut self, id: &PlayerId) {
        if let Some(p) = self.playmates.get_mut(id) {
            p.is_friend = false;
            self.needs_save = true;
        }
    }

    /// block a player
    pub fn block_player(&mut self, id: &PlayerId) {
        if let Some(p) = self.playmates.get_mut(id) {
            p.is_blocked = true;
            p.is_friend = false;
            self.needs_save = true;
        }
    }

    /// unblock a player
    pub fn unblock_player(&mut self, id: &PlayerId) {
        if let Some(p) = self.playmates.get_mut(id) {
            p.is_blocked = false;
            self.needs_save = true;
        }
    }

    /// set nickname for a playmate
    pub fn set_nickname(&mut self, id: &PlayerId, nickname: Option<String>) {
        if let Some(p) = self.playmates.get_mut(id) {
            p.nickname = nickname;
            self.needs_save = true;
        }
    }

    /// set notes for a playmate
    pub fn set_notes(&mut self, id: &PlayerId, notes: String) {
        if let Some(p) = self.playmates.get_mut(id) {
            p.notes = notes;
            self.needs_save = true;
        }
    }

    /// total unique players played with
    pub fn total_playmates(&self) -> usize {
        self.playmates.len()
    }

    /// total games played
    pub fn total_games(&self) -> u32 {
        self.playmates.values().map(|p| p.games_together).sum()
    }

    /// overall win rate
    pub fn overall_win_rate(&self) -> f32 {
        let won: u32 = self.playmates.values().map(|p| p.hands_won).sum();
        let lost: u32 = self.playmates.values().map(|p| p.hands_lost).sum();
        let total = won + lost;
        if total == 0 {
            0.5
        } else {
            won as f32 / total as f32
        }
    }

    /// check if a player with given pubkey is a friend
    pub fn is_friend_by_pubkey(&self, pubkey: &[u8; 32]) -> bool {
        self.playmates.values().any(|p| {
            p.is_friend && !p.is_blocked && p.last_pubkey.as_ref() == Some(pubkey)
        })
    }

    /// serialize for storage
    pub fn serialize(&self) -> Vec<u8> {
        let records: Vec<&Playmate> = self.playmates.values().collect();
        bincode::serialize(&records).unwrap_or_default()
    }

    /// deserialize from storage
    pub fn deserialize(data: &[u8]) -> Option<HashMap<PlayerId, Playmate>> {
        let records: Vec<Playmate> = bincode::deserialize(data).ok()?;
        let map = records.into_iter()
            .map(|p| (p.id.clone(), p))
            .collect();
        Some(map)
    }
}

/// events for the friends system
#[derive(Event)]
pub enum FriendsEvent {
    /// player joined our table
    PlayerJoined {
        pubkey: [u8; 32],
        name: String,
        seat: u8,
    },
    /// player left our table
    PlayerLeft {
        pubkey: [u8; 32],
    },
    /// game ended - update stats
    GameEnded {
        results: Vec<(PlayerId, i64)>, // (player, chips delta)
    },
    /// hand completed
    HandCompleted {
        winner: PlayerId,
        losers: Vec<PlayerId>,
    },
    /// toggle friend status
    ToggleFriend {
        id: PlayerId,
    },
    /// block/unblock player
    ToggleBlock {
        id: PlayerId,
    },
    /// invite player to table
    InviteToTable {
        id: PlayerId,
        table_code: String,
    },
    /// save playmate data
    Save,
    /// load playmate data
    Load,
}

/// handle friends events
fn handle_friends_events(
    mut friends: ResMut<FriendsState>,
    mut events: EventReader<FriendsEvent>,
) {
    for event in events.read() {
        match event {
            FriendsEvent::PlayerJoined { pubkey, name, seat: _ } => {
                let id = PlayerId::from_pubkey(pubkey);
                let playmate = friends.get_or_create(id, name.clone());
                playmate.last_pubkey = Some(*pubkey);
                playmate.display_name = name.clone();
                info!("friends: tracking playmate {} ({})", name, playmate.id.short());
            }

            FriendsEvent::PlayerLeft { pubkey } => {
                let id = PlayerId::from_pubkey(pubkey);
                if let Some(p) = friends.playmates.get(&id) {
                    info!("friends: {} left the table", p.name());
                }
            }

            FriendsEvent::GameEnded { results } => {
                let mut updated = false;
                for (id, chips) in results {
                    if let Some(p) = friends.playmates.get_mut(id) {
                        p.chips_balance += chips;
                        updated = true;
                    }
                }
                if updated {
                    friends.needs_save = true;
                }
            }

            FriendsEvent::HandCompleted { winner, losers } => {
                let mut updated = false;
                if let Some(p) = friends.playmates.get_mut(winner) {
                    p.hands_won += 1;
                    updated = true;
                }
                for loser in losers {
                    if let Some(p) = friends.playmates.get_mut(loser) {
                        p.hands_lost += 1;
                        updated = true;
                    }
                }
                if updated {
                    friends.needs_save = true;
                }
            }

            FriendsEvent::ToggleFriend { id } => {
                let (name, is_friend) = if let Some(p) = friends.playmates.get_mut(id) {
                    p.is_friend = !p.is_friend;
                    if p.is_friend {
                        p.is_blocked = false;
                    }
                    (p.name().to_string(), p.is_friend)
                } else {
                    continue;
                };
                friends.needs_save = true;
                info!("friends: {} friend status: {}", name, is_friend);
            }

            FriendsEvent::ToggleBlock { id } => {
                let (name, is_blocked) = if let Some(p) = friends.playmates.get_mut(id) {
                    p.is_blocked = !p.is_blocked;
                    if p.is_blocked {
                        p.is_friend = false;
                    }
                    (p.name().to_string(), p.is_blocked)
                } else {
                    continue;
                };
                friends.needs_save = true;
                info!("friends: {} blocked: {}", name, is_blocked);
            }

            FriendsEvent::InviteToTable { id, table_code } => {
                friends.pending_invite = Some((id.clone(), table_code.clone()));
                if let Some(p) = friends.playmates.get(id) {
                    info!("friends: inviting {} to table {}", p.name(), table_code);
                }
            }

            FriendsEvent::Save => {
                // handled by storage system
                friends.needs_save = false;
            }

            FriendsEvent::Load => {
                // handled by storage system
            }
        }
    }
}

/// auto-track playmates from P2P events
fn auto_track_playmates(
    mut friends: ResMut<FriendsState>,
    mut p2p_events: EventReader<crate::p2p::P2PNotification>,
    lobby: Res<crate::lobby::LobbyState>,
) {
    for event in p2p_events.read() {
        match event {
            crate::p2p::P2PNotification::PlayerJoined { seat } => {
                // record encounter when player joins
                if let Some(ref code) = lobby.created_code {
                    // note: we'd need the actual pubkey/name from P2P here
                    // for now just log the event
                    info!("friends: player joined at seat {} (table {})", seat, code);
                }
            }
            _ => {}
        }
    }
}

/// load playmates on startup (after auth provides encryption key)
fn load_playmates_on_startup(
    mut friends: ResMut<FriendsState>,
    mut storage_handle: ResMut<PlaymateStorageHandle>,
    auth: Res<crate::auth::AuthState>,
) {
    // only load once when we have auth key
    if storage_handle.loaded {
        return;
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        // derive encryption key from account address (or use default before login)
        let encryption_key = if let Some(ref addr) = auth.account_address {
            blake3::derive_key("playmate-storage-v1", addr.as_bytes())
        } else {
            // use default key for now (before login)
            [0u8; 32]
        };

        // create storage
        let data_dir = dirs::data_local_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join("ghettobox-poker");

        let storage = crate::storage::FilePlaymateStorage::new(
            data_dir.to_str().unwrap_or("."),
            &encryption_key,
        );

        // try to load
        match crate::storage::PlaymateStorage::load_playmates(&storage) {
            Ok(playmates) => {
                friends.playmates = playmates;
                info!("friends: loaded {} playmates from storage", friends.playmates.len());
            }
            Err(e) => {
                warn!("friends: failed to load playmates: {}", e);
            }
        }

        storage_handle.storage = Some(storage);
        storage_handle.loaded = true;
    }

    #[cfg(target_arch = "wasm32")]
    {
        storage_handle.loaded = true;
        // TODO: implement wasm storage for playmates
    }
}

/// auto-save playmates when dirty
fn auto_save_playmates(
    mut friends: ResMut<FriendsState>,
    storage_handle: Res<PlaymateStorageHandle>,
    mut last_save: Local<f64>,
    time: Res<Time>,
) {
    // only save every 5 seconds at most
    let now = time.elapsed().as_secs_f64();
    if now - *last_save < 5.0 {
        return;
    }

    if !friends.needs_save {
        return;
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(ref storage) = storage_handle.storage {
            let playmates = friends.playmates.clone();
            match crate::storage::PlaymateStorage::save_playmates(storage, &playmates) {
                Ok(()) => {
                    friends.needs_save = false;
                    *last_save = now;
                    info!("friends: saved {} playmates", playmates.len());
                }
                Err(e) => {
                    warn!("friends: failed to save playmates: {}", e);
                }
            }
        }
    }
}

/// get current unix timestamp
fn current_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_playmate_tracking() {
        let mut state = FriendsState::default();

        let id = PlayerId::from_address("0x1234567890abcdef");
        let playmate = state.get_or_create(id.clone(), "Alice".to_string());

        assert_eq!(playmate.name(), "Alice");
        assert_eq!(playmate.games_together, 0);

        playmate.record_encounter("42-alpha".to_string(), "1/2".to_string());
        assert_eq!(playmate.games_together, 1);

        state.add_friend(&id);
        assert!(state.playmates.get(&id).unwrap().is_friend);

        let friends = state.friends();
        assert_eq!(friends.len(), 1);
    }

    #[test]
    fn test_serialization() {
        let mut state = FriendsState::default();

        let id1 = PlayerId::from_address("0xabc");
        let id2 = PlayerId::from_address("0xdef");

        state.get_or_create(id1.clone(), "Bob".to_string());
        state.get_or_create(id2.clone(), "Charlie".to_string());
        state.add_friend(&id1);

        let data = state.serialize();
        let loaded = FriendsState::deserialize(&data).unwrap();

        assert_eq!(loaded.len(), 2);
        assert!(loaded.get(&id1).unwrap().is_friend);
    }
}
