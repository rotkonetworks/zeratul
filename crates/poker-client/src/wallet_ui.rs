//! wallet and deposit/withdraw ui
//!
//! provides egui panels for:
//! - viewing balances across chains
//! - depositing from asset hub / cosmos / hyperbridge
//! - withdrawing to various destinations

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

pub struct WalletUiPlugin;

impl Plugin for WalletUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WalletState>()
            .add_systems(Update, render_wallet_ui);
    }
}

/// wallet panel visibility
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WalletPanel {
    #[default]
    Closed,
    Overview,
    Deposit,
    Withdraw,
    History,
}

/// deposit source selection
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DepositSource {
    PolkadotAssetHub,
    KusamaAssetHub,
    Osmosis,
    // hyperbridge goes via kusama asset hub
}

impl DepositSource {
    fn name(&self) -> &'static str {
        match self {
            DepositSource::PolkadotAssetHub => "Polkadot Asset Hub",
            DepositSource::KusamaAssetHub => "Kusama Asset Hub",
            DepositSource::Osmosis => "Osmosis",
        }
    }

    fn description(&self) -> &'static str {
        match self {
            DepositSource::PolkadotAssetHub => "USDT, USDC, and DOT assets via XCM",
            DepositSource::KusamaAssetHub => "KSM, WETH (Hyperbridge), and foreign assets",
            DepositSource::Osmosis => "OSMO, ATOM and IBC tokens",
        }
    }

    fn estimated_time(&self) -> &'static str {
        match self {
            DepositSource::PolkadotAssetHub => "~1 minute",
            DepositSource::KusamaAssetHub => "~30 seconds",
            DepositSource::Osmosis => "~2 minutes",
        }
    }
}

/// withdraw destination
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WithdrawDestination {
    PolkadotAssetHub,
    KusamaAssetHub,
    Osmosis,
    Ethereum, // via kusama asset hub + hyperbridge
}

impl WithdrawDestination {
    fn name(&self) -> &'static str {
        match self {
            WithdrawDestination::PolkadotAssetHub => "Polkadot Asset Hub",
            WithdrawDestination::KusamaAssetHub => "Kusama Asset Hub",
            WithdrawDestination::Osmosis => "Osmosis",
            WithdrawDestination::Ethereum => "Ethereum (via Hyperbridge)",
        }
    }
}

/// asset for deposit/withdraw
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalletAsset {
    Native,
    Usdt,
    Usdc,
    Weth,
    Dot,
    Ksm,
    Osmo,
    Atom,
}

impl WalletAsset {
    fn symbol(&self) -> &'static str {
        match self {
            WalletAsset::Native => "GHETTO",
            WalletAsset::Usdt => "USDT",
            WalletAsset::Usdc => "USDC",
            WalletAsset::Weth => "WETH",
            WalletAsset::Dot => "DOT",
            WalletAsset::Ksm => "KSM",
            WalletAsset::Osmo => "OSMO",
            WalletAsset::Atom => "ATOM",
        }
    }

    fn decimals(&self) -> u8 {
        match self {
            WalletAsset::Native => 12,
            WalletAsset::Usdt | WalletAsset::Usdc => 6,
            WalletAsset::Weth => 18,
            WalletAsset::Dot => 10,
            WalletAsset::Ksm => 12,
            WalletAsset::Osmo | WalletAsset::Atom => 6,
        }
    }
}

/// balance info
#[derive(Clone, Debug, Default)]
pub struct BalanceInfo {
    pub on_chain: u128,    // main balance on ghettobox
    pub in_pool: u128,     // locked in poker pool
    pub pending: u128,     // pending deposits/withdrawals
}

/// pending transfer
#[derive(Clone, Debug)]
pub struct PendingTransfer {
    pub id: String,
    pub direction: TransferDirection,
    pub asset: WalletAsset,
    pub amount: u128,
    pub status: TransferStatus,
    pub timestamp: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferDirection {
    Deposit,
    Withdraw,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferStatus {
    Pending,
    InFlight,
    Completed,
    Failed(String),
}

/// wallet state resource
#[derive(Resource)]
pub struct WalletState {
    /// current panel
    pub panel: WalletPanel,
    /// native balance
    pub balance: BalanceInfo,
    /// usdt balance
    pub usdt_balance: BalanceInfo,
    /// usdc balance
    pub usdc_balance: BalanceInfo,
    /// pending transfers
    pub pending_transfers: Vec<PendingTransfer>,
    /// deposit source selection
    pub deposit_source: Option<DepositSource>,
    /// deposit asset selection
    pub deposit_asset: Option<WalletAsset>,
    /// deposit amount input
    pub deposit_amount: String,
    /// deposit address (source chain address)
    pub deposit_address: String,
    /// withdraw destination
    pub withdraw_destination: Option<WithdrawDestination>,
    /// withdraw asset
    pub withdraw_asset: Option<WalletAsset>,
    /// withdraw amount input
    pub withdraw_amount: String,
    /// withdraw address (destination address)
    pub withdraw_address: String,
    /// error message
    pub error: Option<String>,
    /// success message
    pub success: Option<String>,
}

impl Default for WalletState {
    fn default() -> Self {
        Self {
            panel: WalletPanel::Closed,
            balance: BalanceInfo {
                on_chain: 1_000_000_000_000, // 1.0 GHETTO
                in_pool: 500_000_000_000,     // 0.5 in pool
                pending: 0,
            },
            usdt_balance: BalanceInfo::default(),
            usdc_balance: BalanceInfo::default(),
            pending_transfers: Vec::new(),
            deposit_source: None,
            deposit_asset: None,
            deposit_amount: String::new(),
            deposit_address: String::new(),
            withdraw_destination: None,
            withdraw_asset: None,
            withdraw_amount: String::new(),
            withdraw_address: String::new(),
            error: None,
            success: None,
        }
    }
}

impl WalletState {
    fn format_balance(&self, amount: u128, decimals: u8) -> String {
        let divisor = 10u128.pow(decimals as u32);
        let whole = amount / divisor;
        let frac = amount % divisor;
        format!("{}.{:0>width$}", whole, frac / 10u128.pow((decimals - 4) as u32), width = 4)
    }
}

/// render wallet ui
fn render_wallet_ui(
    mut contexts: EguiContexts,
    mut wallet: ResMut<WalletState>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    // toggle wallet with W key
    if keys.just_pressed(KeyCode::KeyW) && !keys.pressed(KeyCode::ControlLeft) {
        wallet.panel = match wallet.panel {
            WalletPanel::Closed => WalletPanel::Overview,
            _ => WalletPanel::Closed,
        };
    }

    // close on escape
    if keys.just_pressed(KeyCode::Escape) && wallet.panel != WalletPanel::Closed {
        wallet.panel = WalletPanel::Closed;
    }

    if wallet.panel == WalletPanel::Closed {
        // just show wallet button
        egui::Area::new(egui::Id::new("wallet_button"))
            .fixed_pos(egui::pos2(10.0, 10.0))
            .show(contexts.ctx_mut(), |ui| {
                if ui.button("💰 Wallet [W]").clicked() {
                    wallet.panel = WalletPanel::Overview;
                }
            });
        return;
    }

    egui::Window::new("💰 Wallet")
        .collapsible(false)
        .resizable(true)
        .default_size([400.0, 500.0])
        .show(contexts.ctx_mut(), |ui| {
            // tab bar
            ui.horizontal(|ui| {
                if ui.selectable_label(wallet.panel == WalletPanel::Overview, "Overview").clicked() {
                    wallet.panel = WalletPanel::Overview;
                }
                if ui.selectable_label(wallet.panel == WalletPanel::Deposit, "Deposit").clicked() {
                    wallet.panel = WalletPanel::Deposit;
                    wallet.deposit_source = None;
                    wallet.deposit_asset = None;
                }
                if ui.selectable_label(wallet.panel == WalletPanel::Withdraw, "Withdraw").clicked() {
                    wallet.panel = WalletPanel::Withdraw;
                    wallet.withdraw_destination = None;
                    wallet.withdraw_asset = None;
                }
                if ui.selectable_label(wallet.panel == WalletPanel::History, "History").clicked() {
                    wallet.panel = WalletPanel::History;
                }
            });

            ui.separator();

            // clear messages
            if wallet.error.is_some() || wallet.success.is_some() {
                if ui.button("✕ Clear").clicked() {
                    wallet.error = None;
                    wallet.success = None;
                }
            }

            // show error/success
            if let Some(ref err) = wallet.error {
                ui.colored_label(egui::Color32::RED, format!("⚠ {}", err));
            }
            if let Some(ref msg) = wallet.success {
                ui.colored_label(egui::Color32::GREEN, format!("✓ {}", msg));
            }

            ui.separator();

            match wallet.panel {
                WalletPanel::Overview => render_overview(ui, &wallet),
                WalletPanel::Deposit => render_deposit(ui, &mut wallet),
                WalletPanel::Withdraw => render_withdraw(ui, &mut wallet),
                WalletPanel::History => render_history(ui, &wallet),
                WalletPanel::Closed => {}
            }
        });
}

fn render_overview(ui: &mut egui::Ui, wallet: &WalletState) {
    ui.heading("Balances");

    egui::Grid::new("balance_grid")
        .num_columns(4)
        .striped(true)
        .show(ui, |ui| {
            ui.label("Asset");
            ui.label("On-chain");
            ui.label("In Pool");
            ui.label("Pending");
            ui.end_row();

            // native
            ui.label("GHETTO");
            ui.label(wallet.format_balance(wallet.balance.on_chain, 12));
            ui.label(wallet.format_balance(wallet.balance.in_pool, 12));
            ui.label(wallet.format_balance(wallet.balance.pending, 12));
            ui.end_row();

            // usdt
            ui.label("USDT");
            ui.label(wallet.format_balance(wallet.usdt_balance.on_chain, 6));
            ui.label(wallet.format_balance(wallet.usdt_balance.in_pool, 6));
            ui.label(wallet.format_balance(wallet.usdt_balance.pending, 6));
            ui.end_row();

            // usdc
            ui.label("USDC");
            ui.label(wallet.format_balance(wallet.usdc_balance.on_chain, 6));
            ui.label(wallet.format_balance(wallet.usdc_balance.in_pool, 6));
            ui.label(wallet.format_balance(wallet.usdc_balance.pending, 6));
            ui.end_row();
        });

    ui.add_space(20.0);

    ui.heading("Quick Actions");
    ui.horizontal(|ui| {
        if ui.button("📥 Deposit").clicked() {
            // would set panel to deposit
        }
        if ui.button("📤 Withdraw").clicked() {
            // would set panel to withdraw
        }
        if ui.button("🎰 Add to Pool").clicked() {
            // deposit to poker pool
        }
        if ui.button("💵 Cash Out").clicked() {
            // withdraw from poker pool
        }
    });
}

fn render_deposit(ui: &mut egui::Ui, wallet: &mut WalletState) {
    ui.heading("Deposit Assets");

    // step 1: select source
    if wallet.deposit_source.is_none() {
        ui.label("Select deposit source:");
        ui.add_space(10.0);

        for source in [
            DepositSource::PolkadotAssetHub,
            DepositSource::KusamaAssetHub,
            DepositSource::Osmosis,
        ] {
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.strong(source.name());
                        ui.label(source.description());
                        ui.small(format!("Est. time: {}", source.estimated_time()));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Select →").clicked() {
                            wallet.deposit_source = Some(source.clone());
                        }
                    });
                });
            });
        }
        return;
    }

    let source = wallet.deposit_source.as_ref().unwrap();
    ui.label(format!("From: {}", source.name()));

    if ui.button("← Change source").clicked() {
        wallet.deposit_source = None;
        wallet.deposit_asset = None;
        return;
    }

    ui.separator();

    // step 2: select asset
    if wallet.deposit_asset.is_none() {
        ui.label("Select asset to deposit:");
        ui.add_space(10.0);

        let assets = match source {
            DepositSource::PolkadotAssetHub => vec![WalletAsset::Usdt, WalletAsset::Usdc, WalletAsset::Dot],
            DepositSource::KusamaAssetHub => vec![WalletAsset::Ksm, WalletAsset::Weth],
            DepositSource::Osmosis => vec![WalletAsset::Osmo, WalletAsset::Atom],
        };

        ui.horizontal_wrapped(|ui| {
            for asset in assets {
                if ui.button(asset.symbol()).clicked() {
                    wallet.deposit_asset = Some(asset);
                }
            }
        });
        return;
    }

    let asset = wallet.deposit_asset.as_ref().unwrap();
    ui.label(format!("Asset: {}", asset.symbol()));

    if ui.button("← Change asset").clicked() {
        wallet.deposit_asset = None;
        return;
    }

    ui.separator();

    // step 3: enter amount and address
    ui.label("Amount:");
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut wallet.deposit_amount);
        ui.label(asset.symbol());
    });

    ui.add_space(10.0);

    ui.label("Your address on source chain:");
    ui.text_edit_singleline(&mut wallet.deposit_address);
    ui.small("Enter your address to verify you control the funds");

    ui.add_space(20.0);

    // deposit instructions
    ui.group(|ui| {
        ui.heading("Deposit Instructions");
        match source {
            DepositSource::PolkadotAssetHub | DepositSource::KusamaAssetHub => {
                ui.label("1. Open your wallet on the source chain");
                ui.label("2. Initiate an XCM transfer to ghettobox");
                ui.label("3. Your ghettobox address will receive the funds automatically");
                ui.add_space(5.0);
                ui.label("Target address:");
                ui.monospace("5Ghettobox... (your derived address)");
            }
            DepositSource::Osmosis => {
                ui.label("1. Open your Osmosis wallet");
                ui.label("2. Use IBC transfer to ghettobox channel");
                ui.label("3. Funds arrive after ~2 minutes");
                ui.add_space(5.0);
                ui.label("IBC memo:");
                ui.monospace("channel-X/ghettobox-address");
            }
        }
    });

    ui.add_space(10.0);

    if ui.button("✓ I've initiated the deposit").clicked() {
        wallet.success = Some("Deposit pending - will appear in ~1-2 minutes".into());
        wallet.deposit_source = None;
        wallet.deposit_asset = None;
        wallet.deposit_amount.clear();
    }
}

fn render_withdraw(ui: &mut egui::Ui, wallet: &mut WalletState) {
    ui.heading("Withdraw Assets");

    // step 1: select destination
    if wallet.withdraw_destination.is_none() {
        ui.label("Select withdrawal destination:");
        ui.add_space(10.0);

        for dest in [
            WithdrawDestination::PolkadotAssetHub,
            WithdrawDestination::KusamaAssetHub,
            WithdrawDestination::Osmosis,
            WithdrawDestination::Ethereum,
        ] {
            ui.horizontal(|ui| {
                if ui.button(dest.name()).clicked() {
                    wallet.withdraw_destination = Some(dest);
                }
            });
        }
        return;
    }

    // extract values before any mutable borrows
    let dest = wallet.withdraw_destination.clone().unwrap();
    let dest_name = dest.name().to_string();
    let dest_time = match dest {
        WithdrawDestination::PolkadotAssetHub => "~1 minute",
        WithdrawDestination::KusamaAssetHub => "~30 seconds",
        WithdrawDestination::Osmosis => "~2 minutes",
        WithdrawDestination::Ethereum => "~15 minutes (via Hyperbridge)",
    };

    ui.label(format!("To: {}", dest_name));

    if ui.button("← Change destination").clicked() {
        wallet.withdraw_destination = None;
        wallet.withdraw_asset = None;
        return;
    }

    ui.separator();

    // step 2: asset and amount
    ui.label("Asset:");
    let current_asset = wallet.withdraw_asset.clone();
    ui.horizontal(|ui| {
        if ui.selectable_label(current_asset == Some(WalletAsset::Native), "GHETTO").clicked() {
            wallet.withdraw_asset = Some(WalletAsset::Native);
        }
        if ui.selectable_label(current_asset == Some(WalletAsset::Usdt), "USDT").clicked() {
            wallet.withdraw_asset = Some(WalletAsset::Usdt);
        }
        if ui.selectable_label(current_asset == Some(WalletAsset::Usdc), "USDC").clicked() {
            wallet.withdraw_asset = Some(WalletAsset::Usdc);
        }
    });

    if wallet.withdraw_asset.is_none() {
        return;
    }

    let asset = wallet.withdraw_asset.clone().unwrap();
    let asset_symbol = asset.symbol().to_string();
    let max_balance = wallet.format_balance(wallet.balance.on_chain, 12);

    ui.add_space(10.0);

    ui.label("Amount:");
    ui.horizontal(|ui| {
        ui.text_edit_singleline(&mut wallet.withdraw_amount);
        ui.label(&asset_symbol);
        if ui.button("Max").clicked() {
            wallet.withdraw_amount = max_balance.clone();
        }
    });

    ui.add_space(10.0);

    ui.label("Destination address:");
    ui.text_edit_singleline(&mut wallet.withdraw_address);

    ui.add_space(10.0);

    // fee estimate
    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.label("Estimated fee:");
            ui.label("~0.001 GHETTO + destination chain fee");
        });
        ui.horizontal(|ui| {
            ui.label("Estimated time:");
            ui.label(dest_time);
        });
    });

    ui.add_space(10.0);

    let can_withdraw = !wallet.withdraw_amount.is_empty() && !wallet.withdraw_address.is_empty();

    ui.add_enabled_ui(can_withdraw, |ui| {
        if ui.button("🔒 Withdraw").clicked() {
            wallet.success = Some(format!(
                "Withdrawal initiated: {} {} → {}",
                wallet.withdraw_amount,
                asset_symbol,
                dest_name
            ));
            wallet.withdraw_destination = None;
            wallet.withdraw_asset = None;
            wallet.withdraw_amount.clear();
            wallet.withdraw_address.clear();
        }
    });
}

fn render_history(ui: &mut egui::Ui, wallet: &WalletState) {
    ui.heading("Transfer History");

    if wallet.pending_transfers.is_empty() {
        ui.label("No recent transfers");
        return;
    }

    egui::Grid::new("history_grid")
        .num_columns(5)
        .striped(true)
        .show(ui, |ui| {
            ui.label("Type");
            ui.label("Asset");
            ui.label("Amount");
            ui.label("Status");
            ui.label("Time");
            ui.end_row();

            for transfer in &wallet.pending_transfers {
                let dir_icon = match transfer.direction {
                    TransferDirection::Deposit => "📥",
                    TransferDirection::Withdraw => "📤",
                };
                ui.label(dir_icon);
                ui.label(transfer.asset.symbol());
                ui.label(format!("{}", transfer.amount));
                let status = match &transfer.status {
                    TransferStatus::Pending => "⏳ Pending",
                    TransferStatus::InFlight => "🚀 In flight",
                    TransferStatus::Completed => "✓ Complete",
                    TransferStatus::Failed(e) => "⚠ Failed",
                };
                ui.label(status);
                ui.label("Just now");
                ui.end_row();
            }
        });
}
