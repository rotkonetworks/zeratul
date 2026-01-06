//! ghettobox authentication module
//!
//! extension-free web3 identity using email + PIN
//! - login: PIN stretched → vault shares → seed → signing key
//! - signing key lives in memory for session (no prompts during gameplay)
//! - confirmations for withdrawals/transfers (configurable)

use bevy::prelude::*;
use ed25519_dalek::{Signature, Signer as Ed25519Signer, SigningKey, VerifyingKey};
use ghettobox::{Account, Client as GhettoboxClient, vss};
use std::sync::Arc;
use tokio::sync::mpsc;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// auth plugin for bevy
pub struct AuthPlugin;

impl Plugin for AuthPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AuthState>()
            .init_resource::<SigningPolicy>()
            .init_resource::<VaultConfig>()
            .add_event::<AuthEvent>()
            .add_event::<SigningRequest>()
            .add_event::<SigningResponse>()
            .add_systems(Update, (handle_auth_events, poll_auth_result, handle_signing_requests));
    }
}

/// vault node configuration
#[derive(Resource, Clone)]
pub struct VaultConfig {
    /// vault node URLs (need 2 of 3)
    pub nodes: Vec<String>,
    /// timeout for vault requests (ms)
    pub timeout_ms: u64,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            // default to localhost for development
            nodes: vec![
                "http://127.0.0.1:4200".into(),
                "http://127.0.0.1:4201".into(),
                "http://127.0.0.1:4202".into(),
            ],
            timeout_ms: 10_000,
        }
    }
}

/// signing policy - what requires confirmation
#[derive(Resource, Clone)]
pub struct SigningPolicy {
    /// auto-sign channel state updates (recommended: true)
    pub auto_channel_updates: bool,
    /// auto-sign poker actions (recommended: true)
    pub auto_poker_actions: bool,
    /// auto-sign shuffle proofs (recommended: true)
    pub auto_shuffle_proofs: bool,
    /// confirm withdrawals (recommended: true)
    pub confirm_withdrawals: bool,
    /// confirm transfers to other users (recommended: true)
    pub confirm_transfers: bool,
    /// require PIN re-entry for amounts above threshold (None = disabled)
    pub pin_threshold_usd: Option<u64>,
    /// session timeout in seconds (re-login required after)
    pub session_timeout_secs: u64,
}

impl Default for SigningPolicy {
    fn default() -> Self {
        Self {
            auto_channel_updates: true,
            auto_poker_actions: true,
            auto_shuffle_proofs: true,
            confirm_withdrawals: true,
            confirm_transfers: true,
            pin_threshold_usd: None, // disabled by default
            session_timeout_secs: 3600 * 4, // 4 hours
        }
    }
}

/// authentication mode
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AuthMode {
    /// test mode - local deterministic keys, no vault network
    #[default]
    Test,
    /// production mode - uses ghettobox vaults
    Production,
}

/// authentication state
#[derive(Resource)]
pub struct AuthState {
    /// current auth status
    pub status: AuthStatus,
    /// auth mode (test or production)
    pub mode: AuthMode,
    /// user's email (for display)
    pub email: Option<String>,
    /// derived account address (hex)
    pub account_address: Option<String>,
    /// session start timestamp
    pub session_started: Option<u64>,
    /// login form state
    pub login_form: LoginForm,
    /// signing key (in memory after login)
    signing_key: Option<SigningKey>,
    /// async auth result channel
    auth_rx: Option<mpsc::Receiver<AuthResult>>,
    /// pending confirmation dialog
    pub pending_confirmation: Option<ConfirmationRequest>,
}

impl Default for AuthState {
    fn default() -> Self {
        Self {
            status: AuthStatus::NotLoggedIn,
            mode: AuthMode::Test,
            email: None,
            account_address: None,
            session_started: None,
            login_form: LoginForm::default(),
            signing_key: None,
            auth_rx: None,
            pending_confirmation: None,
        }
    }
}

impl AuthState {
    /// check if logged in and session valid
    pub fn is_logged_in(&self) -> bool {
        self.status == AuthStatus::LoggedIn && self.signing_key.is_some()
    }

    /// get public key bytes
    pub fn public_key(&self) -> Option<[u8; 32]> {
        self.signing_key.as_ref().map(|k| k.verifying_key().to_bytes())
    }

    /// sign a message (returns None if not logged in)
    pub fn sign(&self, message: &[u8]) -> Option<[u8; 64]> {
        self.signing_key.as_ref().map(|k| k.sign(message).to_bytes())
    }

    /// sign a message, returns Signature type
    pub fn sign_signature(&self, message: &[u8]) -> Option<Signature> {
        self.signing_key.as_ref().map(|k| k.sign(message))
    }
}

/// login form state
#[derive(Default, Clone)]
pub struct LoginForm {
    pub email: String,
    pub pin: String,
    pub pin_confirm: String,
    pub error: Option<String>,
    pub is_registering: bool,
}

/// auth status
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum AuthStatus {
    #[default]
    NotLoggedIn,
    LoggingIn,
    Registering,
    LoggedIn,
    Error,
}

/// auth events
#[derive(Event)]
pub enum AuthEvent {
    /// user submitted login form
    Login { email: String, pin: String },
    /// user submitted registration form
    Register { email: String, pin: String },
    /// logout
    Logout,
    /// switch auth mode
    SetMode(AuthMode),
}

/// async auth result
enum AuthResult {
    Success {
        email: String,
        address: String,
        signing_key: SigningKey,
    },
    Failed {
        error: String,
    },
}

/// signing request for confirmation dialogs
#[derive(Event, Clone)]
pub struct SigningRequest {
    pub id: u64,
    pub kind: SigningKind,
    pub message: Vec<u8>,
    pub display_info: String,
}

/// what kind of signing operation
#[derive(Clone, Debug)]
pub enum SigningKind {
    /// channel state update (auto-sign)
    ChannelUpdate,
    /// poker action (auto-sign)
    PokerAction,
    /// shuffle proof (auto-sign)
    ShuffleProof,
    /// withdrawal (may require confirmation)
    Withdrawal { amount_usd: u64 },
    /// transfer to another user (may require confirmation)
    Transfer { recipient: String, amount_usd: u64 },
}

/// signing response (after confirmation if needed)
#[derive(Event)]
pub struct SigningResponse {
    pub id: u64,
    pub signature: Option<[u8; 64]>,
    pub approved: bool,
}

/// pending confirmation dialog
#[derive(Clone)]
pub struct ConfirmationRequest {
    pub id: u64,
    pub title: String,
    pub message: String,
    pub amount: Option<u64>,
    pub requires_pin: bool,
    pub signing_message: Vec<u8>,
}

fn handle_auth_events(
    mut events: EventReader<AuthEvent>,
    mut auth_state: ResMut<AuthState>,
    vault_config: Res<VaultConfig>,
) {
    for event in events.read() {
        match event {
            AuthEvent::Login { email, pin } => {
                info!("auth: login {} (mode: {:?})", email, auth_state.mode);
                auth_state.status = AuthStatus::LoggingIn;
                auth_state.email = Some(email.clone());
                auth_state.login_form.error = None;

                match auth_state.mode {
                    AuthMode::Test => {
                        // test mode: derive keys locally
                        let (address, key) = derive_test_identity(email, pin);
                        info!("test mode login: {}", address);
                        auth_state.status = AuthStatus::LoggedIn;
                        auth_state.account_address = Some(address);
                        auth_state.signing_key = Some(key);
                        auth_state.session_started = Some(now_timestamp());
                    }
                    AuthMode::Production => {
                        // production: async vault recovery
                        let (tx, rx) = mpsc::channel(1);
                        auth_state.auth_rx = Some(rx);

                        let email = email.clone();
                        let pin = pin.clone();
                        let nodes = vault_config.nodes.clone();

                        // spawn async login task
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            std::thread::spawn(move || {
                                let rt = tokio::runtime::Runtime::new().unwrap();
                                rt.block_on(async {
                                    let result = do_vault_login(&email, &pin, &nodes).await;
                                    let _ = tx.blocking_send(result);
                                });
                            });
                        }

                        #[cfg(target_arch = "wasm32")]
                        {
                            wasm_bindgen_futures::spawn_local(async move {
                                let result = do_vault_login(&email, &pin, &nodes).await;
                                let _ = tx.send(result).await;
                            });
                        }
                    }
                }
            }
            AuthEvent::Register { email, pin } => {
                info!("auth: register {} (mode: {:?})", email, auth_state.mode);
                auth_state.status = AuthStatus::Registering;
                auth_state.email = Some(email.clone());
                auth_state.login_form.error = None;

                match auth_state.mode {
                    AuthMode::Test => {
                        // test mode: just derive and "register"
                        let (address, key) = derive_test_identity(email, pin);
                        info!("test mode register: {}", address);
                        auth_state.status = AuthStatus::LoggedIn;
                        auth_state.account_address = Some(address);
                        auth_state.signing_key = Some(key);
                        auth_state.session_started = Some(now_timestamp());
                    }
                    AuthMode::Production => {
                        // production: async vault registration
                        let (tx, rx) = mpsc::channel(1);
                        auth_state.auth_rx = Some(rx);

                        let email = email.clone();
                        let pin = pin.clone();
                        let nodes = vault_config.nodes.clone();

                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            std::thread::spawn(move || {
                                let rt = tokio::runtime::Runtime::new().unwrap();
                                rt.block_on(async {
                                    let result = do_vault_register(&email, &pin, &nodes).await;
                                    let _ = tx.blocking_send(result);
                                });
                            });
                        }

                        #[cfg(target_arch = "wasm32")]
                        {
                            wasm_bindgen_futures::spawn_local(async move {
                                let result = do_vault_register(&email, &pin, &nodes).await;
                                let _ = tx.send(result).await;
                            });
                        }
                    }
                }
            }
            AuthEvent::Logout => {
                info!("auth: logout");
                auth_state.status = AuthStatus::NotLoggedIn;
                auth_state.email = None;
                auth_state.account_address = None;
                auth_state.session_started = None;
                auth_state.signing_key = None;
                auth_state.pending_confirmation = None;
            }
            AuthEvent::SetMode(mode) => {
                info!("auth: set mode {:?}", mode);
                auth_state.mode = *mode;
            }
        }
    }
}

fn poll_auth_result(mut auth_state: ResMut<AuthState>) {
    if let Some(ref mut rx) = auth_state.auth_rx {
        match rx.try_recv() {
            Ok(AuthResult::Success { email, address, signing_key }) => {
                info!("auth success: {}", address);
                auth_state.status = AuthStatus::LoggedIn;
                auth_state.email = Some(email);
                auth_state.account_address = Some(address);
                auth_state.signing_key = Some(signing_key);
                auth_state.session_started = Some(now_timestamp());
                auth_state.login_form.error = None;
                auth_state.auth_rx = None;
            }
            Ok(AuthResult::Failed { error }) => {
                warn!("auth failed: {}", error);
                auth_state.status = AuthStatus::Error;
                auth_state.login_form.error = Some(error);
                auth_state.auth_rx = None;
            }
            Err(mpsc::error::TryRecvError::Empty) => {
                // still waiting
            }
            Err(mpsc::error::TryRecvError::Disconnected) => {
                auth_state.status = AuthStatus::Error;
                auth_state.login_form.error = Some("auth channel disconnected".into());
                auth_state.auth_rx = None;
            }
        }
    }
}

fn handle_signing_requests(
    mut requests: EventReader<SigningRequest>,
    mut responses: EventWriter<SigningResponse>,
    auth_state: Res<AuthState>,
    policy: Res<SigningPolicy>,
) {
    for request in requests.read() {
        let should_auto_sign = match &request.kind {
            SigningKind::ChannelUpdate => policy.auto_channel_updates,
            SigningKind::PokerAction => policy.auto_poker_actions,
            SigningKind::ShuffleProof => policy.auto_shuffle_proofs,
            SigningKind::Withdrawal { amount_usd } => {
                !policy.confirm_withdrawals &&
                policy.pin_threshold_usd.map_or(true, |t| *amount_usd < t)
            }
            SigningKind::Transfer { amount_usd, .. } => {
                !policy.confirm_transfers &&
                policy.pin_threshold_usd.map_or(true, |t| *amount_usd < t)
            }
        };

        if should_auto_sign {
            // auto-sign immediately
            let signature = auth_state.sign(&request.message);
            responses.send(SigningResponse {
                id: request.id,
                signature,
                approved: signature.is_some(),
            });
        } else {
            // need confirmation - handled by UI
            // the UI should check auth_state.pending_confirmation
            info!("signing request {} requires confirmation: {}", request.id, request.display_info);
        }
    }
}

/// perform vault login (async)
async fn do_vault_login(email: &str, pin: &str, nodes: &[String]) -> AuthResult {
    // create ghettobox client
    let client = GhettoboxClient::offline();

    // for production, we'd use:
    // let client = GhettoboxClient::new(nodes).await?;

    // try to recover from vaults
    // this is simplified - real impl would:
    // 1. compute unlock_tag from PIN
    // 2. request shares from 2+ vault nodes
    // 3. VSS combine shares to get seed
    // 4. derive account from seed

    // for now, use offline mode (same as test but with ghettobox)
    let pin_bytes = pin.as_bytes();

    // derive deterministic seed for demo
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"ghettobox-demo-seed-v1");
    hasher.update(email.as_bytes());
    hasher.update(pin_bytes);
    let seed: [u8; 32] = *hasher.finalize().as_bytes();

    match Account::from_seed(&seed) {
        Ok(account) => {
            let address = account.address_hex();

            // extract signing key from account
            // note: Account doesn't expose signing_key directly,
            // so we re-derive it the same way ghettobox does
            use hkdf::Hkdf;
            use sha2::Sha256;
            let hk = Hkdf::<Sha256>::new(None, &seed);
            let mut signing_bytes = [0u8; 32];
            hk.expand(b"ghettobox:ed25519:v1", &mut signing_bytes).unwrap();
            let signing_key = SigningKey::from_bytes(&signing_bytes);

            AuthResult::Success {
                email: email.to_string(),
                address,
                signing_key,
            }
        }
        Err(e) => AuthResult::Failed {
            error: format!("failed to derive account: {}", e),
        },
    }
}

/// perform vault registration (async)
async fn do_vault_register(email: &str, pin: &str, nodes: &[String]) -> AuthResult {
    // create account and register with vaults
    let client = GhettoboxClient::offline();

    let pin_bytes = pin.as_bytes();

    // create new account
    match client.create_account(email, pin_bytes) {
        Ok(result) => {
            let address = result.account.address_hex();

            // in production, we'd distribute shares to vaults:
            // for (i, node) in nodes.iter().enumerate() {
            //     register_share(node, &result.user_share, &result.vss_shares[i]).await?;
            // }

            // extract signing key (same derivation as login)
            let mut hasher = blake3::Hasher::new();
            hasher.update(b"ghettobox-demo-seed-v1");
            hasher.update(email.as_bytes());
            hasher.update(pin_bytes);
            let seed: [u8; 32] = *hasher.finalize().as_bytes();

            use hkdf::Hkdf;
            use sha2::Sha256;
            let hk = Hkdf::<Sha256>::new(None, &seed);
            let mut signing_bytes = [0u8; 32];
            hk.expand(b"ghettobox:ed25519:v1", &mut signing_bytes).unwrap();
            let signing_key = SigningKey::from_bytes(&signing_bytes);

            info!("registered new account: {}", address);

            AuthResult::Success {
                email: email.to_string(),
                address,
                signing_key,
            }
        }
        Err(e) => AuthResult::Failed {
            error: format!("registration failed: {}", e),
        },
    }
}

/// derive test identity from email + pin using blake3
fn derive_test_identity(email: &str, pin: &str) -> (String, SigningKey) {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"poker-client.test-auth.v1");
    hasher.update(email.as_bytes());
    hasher.update(pin.as_bytes());
    let seed: [u8; 32] = *hasher.finalize().as_bytes();

    let signing_key = SigningKey::from_bytes(&seed);
    let pubkey = signing_key.verifying_key();

    // format as hex address
    let address = format!("0x{}", hex::encode(&pubkey.to_bytes()[..20]));

    (address, signing_key)
}

fn now_timestamp() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        let window = web_sys::window().unwrap();
        let performance = window.performance().unwrap();
        (performance.now() / 1000.0) as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }
}

/// helper to check session expiry
pub fn is_session_valid(auth_state: &AuthState, policy: &SigningPolicy) -> bool {
    if let Some(started) = auth_state.session_started {
        let elapsed = now_timestamp().saturating_sub(started);
        elapsed < policy.session_timeout_secs
    } else {
        false
    }
}
