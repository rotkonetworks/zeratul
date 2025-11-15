//! IBC Integration for Asset Transfers
//!
//! Handles bidirectional asset transfers between Penumbra and Zeratul:
//! - User locks assets on Penumbra → credited on Zeratul
//! - User withdraws from Zeratul → unlocked on Penumbra
//!
//! Uses standard IBC fungible token transfer protocol (ICS-20).

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::super::lending::types::{AssetId, Amount};
use super::light_client::EmbeddedPenumbraClient;

/// IBC transfer packet (simplified ICS-20)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IBCPacket {
    /// Sequence number
    pub sequence: u64,

    /// Source chain channel
    pub source_channel: String,

    /// Destination chain channel
    pub destination_channel: String,

    /// Transfer data
    pub data: FungibleTokenPacketData,

    /// Timeout height (0 = no timeout)
    pub timeout_height: u64,

    /// Timeout timestamp (unix seconds)
    pub timeout_timestamp: u64,
}

/// IBC fungible token transfer data (ICS-20)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FungibleTokenPacketData {
    /// Asset denomination (e.g., "upenumbra", "penumbra.core.asset.v1.Asset/...")
    pub denom: String,

    /// Amount to transfer
    pub amount: String,

    /// Sender address on source chain
    pub sender: String,

    /// Receiver address on destination chain
    pub receiver: String,
}

/// IBC packet with Merkle proof
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IBCPacketWithProof {
    /// The packet
    pub packet: IBCPacket,

    /// Penumbra block height where packet was committed
    pub proof_height: u64,

    /// Merkle proof of packet commitment
    pub merkle_proof: Vec<u8>,

    /// App hash at proof height
    pub app_hash: [u8; 32],
}

/// IBC transfer processed on Zeratul
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IBCTransfer {
    /// Sender (Penumbra address)
    pub sender: String,

    /// Receiver (Zeratul account ID)
    pub receiver: [u8; 32],

    /// Asset ID
    pub asset_id: AssetId,

    /// Amount transferred
    pub amount: Amount,

    /// IBC packet sequence
    pub sequence: u64,
}

/// IBC handler for Penumbra ↔ Zeratul transfers
pub struct IBCHandler {
    /// Embedded Penumbra light client (for proof verification)
    penumbra_client: EmbeddedPenumbraClient,

    /// Our IBC channel to Penumbra
    our_channel: String,

    /// Penumbra's IBC channel to us
    penumbra_channel: String,

    /// Next outgoing packet sequence
    next_sequence: u64,

    /// Asset ID mappings (Penumbra denom → Zeratul AssetId)
    asset_mappings: HashMap<String, AssetId>,
}

impl IBCHandler {
    pub fn new(
        penumbra_client: EmbeddedPenumbraClient,
        our_channel: String,
        penumbra_channel: String,
    ) -> Self {
        Self {
            penumbra_client,
            our_channel,
            penumbra_channel,
            next_sequence: 1,
            asset_mappings: HashMap::new(),
        }
    }

    /// Register asset mapping (Penumbra denom → Zeratul AssetId)
    pub fn register_asset(&mut self, penumbra_denom: String, asset_id: AssetId) {
        self.asset_mappings.insert(penumbra_denom, asset_id);
    }

    /// Process incoming IBC packet from Penumbra
    ///
    /// Steps:
    /// 1. Verify IBC proof against Penumbra light client
    /// 2. Decode transfer data
    /// 3. Credit user on Zeratul
    pub async fn handle_incoming_packet(
        &mut self,
        packet_with_proof: IBCPacketWithProof,
    ) -> Result<IBCTransfer> {
        // 1. Verify packet proof using light client
        let verified = self
            .penumbra_client
            .verify_ibc_packet(
                &self.compute_packet_commitment(&packet_with_proof.packet),
                packet_with_proof.proof_height,
                &packet_with_proof.merkle_proof,
            )
            .await?;

        if !verified {
            bail!("IBC packet proof verification failed");
        }

        // 2. Check packet is for our channel
        if packet_with_proof.packet.destination_channel != self.our_channel {
            bail!("packet not for our channel");
        }

        // 3. Decode transfer data
        let data = &packet_with_proof.packet.data;

        // 4. Map Penumbra denom to our AssetId
        let asset_id = self
            .asset_mappings
            .get(&data.denom)
            .ok_or_else(|| anyhow::anyhow!("unknown asset: {}", data.denom))?;

        // 5. Parse amount
        let amount = Amount(data.amount.parse()?);

        // 6. Parse receiver (Zeratul account ID)
        let receiver = self.parse_receiver_address(&data.receiver)?;

        Ok(IBCTransfer {
            sender: data.sender.clone(),
            receiver,
            asset_id: *asset_id,
            amount,
            sequence: packet_with_proof.packet.sequence,
        })
    }

    /// Send IBC packet to Penumbra (unlock assets)
    ///
    /// User wants to withdraw assets back to Penumbra.
    /// Creates IBC transfer packet that will be relayed.
    pub fn create_outgoing_packet(
        &mut self,
        receiver: String, // Penumbra address
        asset_id: AssetId,
        amount: Amount,
    ) -> Result<IBCPacket> {
        // Find Penumbra denom for this asset
        let denom = self
            .asset_mappings
            .iter()
            .find(|(_, &id)| id == asset_id)
            .map(|(denom, _)| denom.clone())
            .ok_or_else(|| anyhow::anyhow!("asset not mapped to Penumbra denom"))?;

        // Create packet
        let packet = IBCPacket {
            sequence: self.next_sequence,
            source_channel: self.our_channel.clone(),
            destination_channel: self.penumbra_channel.clone(),
            data: FungibleTokenPacketData {
                denom,
                amount: amount.0.to_string(),
                sender: "zeratul".to_string(), // Our chain identifier
                receiver,
            },
            timeout_height: 0, // No timeout
            timeout_timestamp: 0,
        };

        self.next_sequence += 1;

        Ok(packet)
    }

    /// Compute packet commitment (hash of packet data)
    fn compute_packet_commitment(&self, packet: &IBCPacket) -> Vec<u8> {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(&packet.sequence.to_le_bytes());
        hasher.update(packet.source_channel.as_bytes());
        hasher.update(packet.destination_channel.as_bytes());
        hasher.update(serde_json::to_vec(&packet.data).unwrap());

        hasher.finalize().to_vec()
    }

    /// Parse receiver address (Zeratul account ID from string)
    fn parse_receiver_address(&self, address: &str) -> Result<[u8; 32]> {
        // In real implementation: Parse bech32 or hex address
        // For now, parse hex string
        if address.len() != 64 {
            bail!("invalid receiver address length");
        }

        let mut bytes = [0u8; 32];
        for i in 0..32 {
            bytes[i] = u8::from_str_radix(&address[i * 2..i * 2 + 2], 16)?;
        }

        Ok(bytes)
    }
}

/// IBC relayer integration
///
/// Coordinates with Hermes relayer to:
/// - Submit outgoing packets to Penumbra
/// - Receive incoming packets from Penumbra
/// - Handle acknowledgments and timeouts
pub struct IBCRelayerInterface {
    /// Path to Hermes config
    hermes_config: String,

    /// Pending outgoing packets
    pending_packets: Vec<IBCPacket>,
}

impl IBCRelayerInterface {
    pub fn new(hermes_config: String) -> Self {
        Self {
            hermes_config,
            pending_packets: Vec::new(),
        }
    }

    /// Submit packet for relaying to Penumbra
    pub fn submit_packet(&mut self, packet: IBCPacket) {
        self.pending_packets.push(packet);
    }

    /// Trigger Hermes to relay pending packets
    pub async fn relay_packets(&mut self) -> Result<()> {
        if self.pending_packets.is_empty() {
            return Ok(());
        }

        // In real implementation:
        // 1. Write packets to shared state
        // 2. Signal Hermes relayer
        // 3. Wait for acknowledgments

        // For now, just clear pending
        self.pending_packets.clear();

        Ok(())
    }
}

/// Settlement helper for closing positions and sending back to Penumbra
pub struct PenumbraSettlement;

impl PenumbraSettlement {
    /// Settle position profits back to Penumbra
    ///
    /// User wants to close leveraged position and withdraw to Penumbra.
    pub fn settle_position_to_penumbra(
        position_value: Vec<(AssetId, Amount)>,
        penumbra_address: String,
        ibc_handler: &mut IBCHandler,
    ) -> Result<Vec<IBCPacket>> {
        let mut packets = Vec::new();

        // Create IBC transfer for each asset
        for (asset_id, amount) in position_value {
            let packet = ibc_handler.create_outgoing_packet(
                penumbra_address.clone(),
                asset_id,
                amount,
            )?;

            packets.push(packet);
        }

        Ok(packets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::light_client::PenumbraClientConfig;

    #[tokio::test]
    async fn test_ibc_packet_creation() {
        // Create mock client
        let config = PenumbraClientConfig::default();
        let client = EmbeddedPenumbraClient::start(config).await.unwrap();

        let mut handler = IBCHandler::new(
            client,
            "zeratul-channel-0".to_string(),
            "penumbra-channel-5".to_string(),
        );

        // Register asset mapping
        let asset_id = AssetId([1; 32]);
        handler.register_asset("upenumbra".to_string(), asset_id);

        // Create outgoing packet
        let packet = handler
            .create_outgoing_packet(
                "penumbra1abc...".to_string(),
                asset_id,
                Amount(1000),
            )
            .unwrap();

        assert_eq!(packet.sequence, 1);
        assert_eq!(packet.source_channel, "zeratul-channel-0");
        assert_eq!(packet.destination_channel, "penumbra-channel-5");
        assert_eq!(packet.data.denom, "upenumbra");
        assert_eq!(packet.data.amount, "1000");
        assert_eq!(packet.data.receiver, "penumbra1abc...");
    }
}
