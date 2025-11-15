//! Wallet integration - uses pcli's wallet and view database
//!
//! Terminator shares the same wallet as pcli:
//! - Same home directory: ~/.local/share/pcli/
//! - Same view database: pcli-view.sqlite
//! - Same config: config.toml with FVK

use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use directories::ProjectDirs;
use penumbra_keys::FullViewingKey;
use penumbra_proto::{
    box_grpc_svc,
    view::v1::{
        view_service_client::ViewServiceClient, view_service_server::ViewServiceServer,
        BalancesRequest,
    },
};
use penumbra_view::{ViewClient, ViewServer};
use penumbra_asset::Value;
use serde::{Deserialize, Serialize};
use std::fs;
use url::Url;
use futures::StreamExt;

const CONFIG_FILE_NAME: &str = "config.toml";
const VIEW_FILE_NAME: &str = "pcli-view.sqlite";

/// Get pcli's default home directory
pub fn pcli_home() -> Utf8PathBuf {
    let path = ProjectDirs::from("zone", "penumbra", "pcli")
        .expect("Failed to get platform data dir")
        .data_dir()
        .to_path_buf();
    Utf8PathBuf::from_path_buf(path).expect("Platform default data dir was not UTF-8")
}

/// Minimal pcli config structure (just what we need)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PcliConfig {
    pub full_viewing_key: FullViewingKey,
    pub grpc_url: Url,
}

impl PcliConfig {
    /// Load from pcli's config file
    pub fn load(path: Utf8PathBuf) -> Result<Self> {
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {}", path))?;

        toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config file: {}", path))
    }
}

/// Wallet state - provides access to pcli's view database
pub struct Wallet {
    pub config: PcliConfig,
    pub view_client: ViewServiceClient<box_grpc_svc::BoxGrpcService>,
    pub home: Utf8PathBuf,
}

impl Wallet {
    /// Load wallet from pcli's home directory
    pub async fn load() -> Result<Self> {
        let home = pcli_home();

        // Ensure pcli is initialized
        if !home.exists() {
            anyhow::bail!(
                "pcli not initialized. Run 'pcli init' first.\n\
                 Expected home directory: {}",
                home
            );
        }

        let config_path = home.join(CONFIG_FILE_NAME);
        let config = PcliConfig::load(config_path)
            .context("Failed to load pcli config. Make sure pcli is initialized.")?;

        // Load view service from pcli's sqlite database
        let view_path = home.join(VIEW_FILE_NAME);

        tracing::info!(%view_path, "loading view database from pcli");

        let registry_path = home.join("registry.json");
        let registry_path = if registry_path.exists() {
            Some(registry_path)
        } else {
            None
        };

        let view_server = ViewServer::load_or_initialize(
            Some(view_path),
            registry_path,
            &config.full_viewing_key,
            config.grpc_url.clone(),
        )
        .await
        .context("Failed to load view database")?;

        // Wrap in gRPC client
        let view_svc = ViewServiceServer::new(view_server);
        let view_client = ViewServiceClient::new(box_grpc_svc::local(view_svc));

        Ok(Self {
            config,
            view_client,
            home,
        })
    }

    /// Get the full viewing key
    pub fn fvk(&self) -> &FullViewingKey {
        &self.config.full_viewing_key
    }

    /// Get the gRPC endpoint URL
    pub fn grpc_url(&self) -> &Url {
        &self.config.grpc_url
    }

    /// Check if wallet is initialized (pcli config exists)
    pub fn is_initialized() -> bool {
        let home = pcli_home();
        home.join(CONFIG_FILE_NAME).exists()
    }

    /// Query account balances
    pub async fn query_balances(&mut self) -> Result<Vec<Value>> {
        let request = BalancesRequest {
            // Query all balances
            ..Default::default()
        };

        let mut stream = self
            .view_client
            .balances(request)
            .await
            .context("Failed to query balances")?
            .into_inner();

        let mut balances = Vec::new();
        while let Some(response) = stream.next().await {
            let balance_response = response.context("Error in balance stream")?;
            if let Some(balance) = balance_response.balance {
                balances.push(
                    balance
                        .try_into()
                        .context("Failed to parse balance")?,
                );
            }
        }

        Ok(balances)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pcli_home_path() {
        let home = pcli_home();
        // Should end with pcli
        assert!(home.as_str().contains("pcli"));
        println!("pcli home: {}", home);
    }

    #[test]
    fn test_is_initialized() {
        // Just check that the function doesn't panic
        let _initialized = Wallet::is_initialized();
    }
}
