//! Crux App - cross-platform wallet core
//!
//! This is the shared business logic that works across all platforms:
//! - Desktop (egui shell)
//! - Android (Jetpack Compose shell)
//! - iOS (SwiftUI shell)
//! - Web (WASM)

use crux_core::{render::{render, RenderOperation}, App, Command, Request};
use serde::{Deserialize, Serialize};

/// Events from shell to core
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum Event {
    // Lifecycle
    Init,

    // Auth
    CreateWallet { password: String },
    RestoreWallet { seed_phrase: String, password: String, birthday: u32 },
    Login { password: String },
    Logout,

    // Sync
    StartSync,
    SyncProgress { height: u32, total: u32 },
    SyncComplete { height: u32 },
    SyncError { message: String },

    // Wallet
    RefreshBalance,
    BalanceUpdated { balance: u64 },

    // Send
    PrepareSend { address: String, amount: u64, memo: Option<String> },
    ConfirmSend,
    SendComplete { txid: String },
    SendError { message: String },

    // Receive
    GenerateAddress,
    AddressGenerated { address: String },

    // Contacts
    AddContact { name: String, address: String },
    DeleteContact { id: String },

    // Chat
    SendMessage { contact_id: String, message: String },
    MessageReceived { contact_id: String, message: String, timestamp: u64 },

    // Settings
    SetServer { url: String },
    SetInsecureMode { enabled: bool },
}

/// App state (owned by core)
#[derive(Debug, Default)]
pub struct Model {
    // Auth state
    pub is_logged_in: bool,
    pub wallet_exists: bool,

    // Wallet data
    pub seed_phrase: Option<String>,
    pub viewing_key: Option<String>,
    pub balance: u64,
    pub birthday_height: u32,

    // Sync state
    pub is_syncing: bool,
    pub sync_height: u32,
    pub chain_height: u32,
    pub gigaproof_verified: bool,

    // Server
    pub server_url: String,
    pub insecure_mode: bool,

    // Pending tx
    pub pending_address: Option<String>,
    pub pending_amount: u64,
    pub pending_memo: Option<String>,

    // Contacts
    pub contacts: Vec<Contact>,

    // Chat
    pub messages: Vec<ChatMessage>,

    // Error state
    pub error: Option<String>,
}

/// Contact entry
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Contact {
    pub id: String,
    pub name: String,
    pub address: String,
    pub last_message: Option<u64>,
    pub unread: u32,
}

/// Chat message
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub id: String,
    pub contact_id: String,
    pub content: String,
    pub timestamp: u64,
    pub is_outgoing: bool,
    pub txid: Option<String>,
}

/// ViewModel sent to shell for rendering
#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq)]
pub struct ViewModel {
    // Auth
    pub is_logged_in: bool,
    pub wallet_exists: bool,

    // Balance
    pub balance_zat: u64,
    pub balance_zec: String,

    // Sync
    pub is_syncing: bool,
    pub sync_progress: f32,
    pub sync_height: u32,
    pub chain_height: u32,
    pub is_verified: bool,

    // Server
    pub server_url: String,
    pub insecure_mode: bool,

    // Address
    pub receive_address: Option<String>,

    // Pending tx
    pub has_pending_tx: bool,
    pub pending_address: Option<String>,
    pub pending_amount: u64,

    // Contacts
    pub contacts: Vec<Contact>,
    pub total_unread: u32,

    // Error
    pub error: Option<String>,
}

/// Effect enum for capabilities
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Effect {
    Render(RenderOperation),
}

// Effect must be Send + 'static
impl crux_core::Effect for Effect {}

impl From<Request<RenderOperation>> for Effect {
    fn from(req: Request<RenderOperation>) -> Self {
        Effect::Render(req.operation)
    }
}

/// The Crux App
#[derive(Default)]
pub struct ZafuCore;

impl App for ZafuCore {
    type Event = Event;
    type Model = Model;
    type ViewModel = ViewModel;
    type Capabilities = ();
    type Effect = Effect;

    fn update(
        &self,
        event: Self::Event,
        model: &mut Self::Model,
        _caps: &Self::Capabilities,
    ) -> Command<Self::Effect, Self::Event> {
        match event {
            Event::Init => {
                // initial setup
            }

            Event::CreateWallet { password: _ } => {
                // generate new seed phrase
                let mnemonic = bip39::Mnemonic::generate(24)
                    .expect("failed to generate mnemonic");
                model.seed_phrase = Some(mnemonic.to_string());
                model.wallet_exists = true;
                model.is_logged_in = true;
            }

            Event::RestoreWallet { seed_phrase, password: _, birthday } => {
                // validate seed phrase
                match bip39::Mnemonic::parse(&seed_phrase) {
                    Ok(_) => {
                        model.seed_phrase = Some(seed_phrase);
                        model.birthday_height = birthday;
                        model.wallet_exists = true;
                        model.is_logged_in = true;
                        model.error = None;
                    }
                    Err(e) => {
                        model.error = Some(format!("invalid seed phrase: {}", e));
                    }
                }
            }

            Event::Login { password: _ } => {
                model.is_logged_in = true;
            }

            Event::Logout => {
                model.is_logged_in = false;
                model.seed_phrase = None;
                model.viewing_key = None;
            }

            Event::StartSync => {
                model.is_syncing = true;
                model.error = None;
            }

            Event::SyncProgress { height, total } => {
                model.sync_height = height;
                model.chain_height = total;
            }

            Event::SyncComplete { height } => {
                model.is_syncing = false;
                model.sync_height = height;
                model.gigaproof_verified = true;
            }

            Event::SyncError { message } => {
                model.is_syncing = false;
                model.error = Some(message);
            }

            Event::RefreshBalance => {
                // recalculate from scanned notes
            }

            Event::BalanceUpdated { balance } => {
                model.balance = balance;
            }

            Event::PrepareSend { address, amount, memo } => {
                model.pending_address = Some(address);
                model.pending_amount = amount;
                model.pending_memo = memo;
            }

            Event::ConfirmSend => {
                // build and broadcast transaction
            }

            Event::SendComplete { txid: _ } => {
                model.pending_address = None;
                model.pending_amount = 0;
                model.pending_memo = None;
            }

            Event::SendError { message } => {
                model.error = Some(message);
            }

            Event::GenerateAddress => {
                // derive next address from viewing key
            }

            Event::AddressGenerated { address: _ } => {
                // address generated
            }

            Event::AddContact { name, address } => {
                let id = format!("{:x}", rand::random::<u64>());
                model.contacts.push(Contact {
                    id,
                    name,
                    address,
                    last_message: None,
                    unread: 0,
                });
            }

            Event::DeleteContact { id } => {
                model.contacts.retain(|c| c.id != id);
            }

            Event::SendMessage { contact_id, message } => {
                let msg = ChatMessage {
                    id: format!("{:x}", rand::random::<u64>()),
                    contact_id,
                    content: message,
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    is_outgoing: true,
                    txid: None,
                };
                model.messages.push(msg);
            }

            Event::MessageReceived { contact_id, message, timestamp } => {
                let msg = ChatMessage {
                    id: format!("{:x}", rand::random::<u64>()),
                    contact_id: contact_id.clone(),
                    content: message,
                    timestamp,
                    is_outgoing: false,
                    txid: None,
                };
                model.messages.push(msg);

                // update unread count
                if let Some(contact) = model.contacts.iter_mut().find(|c| c.id == contact_id) {
                    contact.unread += 1;
                    contact.last_message = Some(timestamp);
                }
            }

            Event::SetServer { url } => {
                model.server_url = url;
            }

            Event::SetInsecureMode { enabled } => {
                model.insecure_mode = enabled;
            }
        }

        // always render after state change
        render()
    }

    fn view(&self, model: &Self::Model) -> Self::ViewModel {
        let sync_progress = if model.chain_height > 0 {
            model.sync_height as f32 / model.chain_height as f32
        } else {
            0.0
        };

        ViewModel {
            is_logged_in: model.is_logged_in,
            wallet_exists: model.wallet_exists,
            balance_zat: model.balance,
            balance_zec: format!("{:.8}", model.balance as f64 / 100_000_000.0),
            is_syncing: model.is_syncing,
            sync_progress,
            sync_height: model.sync_height,
            chain_height: model.chain_height,
            is_verified: model.gigaproof_verified,
            server_url: model.server_url.clone(),
            insecure_mode: model.insecure_mode,
            receive_address: model.viewing_key.clone(),
            has_pending_tx: model.pending_address.is_some(),
            pending_address: model.pending_address.clone(),
            pending_amount: model.pending_amount,
            contacts: model.contacts.clone(),
            total_unread: model.contacts.iter().map(|c| c.unread).sum(),
            error: model.error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crux_core::testing::AppTester;

    #[test]
    fn test_create_wallet() {
        let app = AppTester::<ZafuCore>::default();
        let mut model = Model::default();

        let event = Event::CreateWallet {
            password: "test123".to_string(),
        };

        let _effects = app.update(event, &mut model);

        assert!(model.is_logged_in);
        assert!(model.wallet_exists);
        assert!(model.seed_phrase.is_some());
    }

    #[test]
    fn test_restore_wallet_invalid() {
        let app = AppTester::<ZafuCore>::default();
        let mut model = Model::default();

        let event = Event::RestoreWallet {
            seed_phrase: "invalid seed".to_string(),
            password: "test".to_string(),
            birthday: 1000000,
        };

        let _effects = app.update(event, &mut model);

        assert!(!model.is_logged_in);
        assert!(model.error.is_some());
    }
}
