//! zidecar gRPC client

use anyhow::Result;
use crate::zidecar::{zidecar_client::ZidecarClient as GrpcClient, Empty, ProofRequest, BlockRange};
use tonic::transport::Channel;
use tracing::{info, debug};

pub struct ZidecarClient {
    client: GrpcClient<Channel>,
}

impl ZidecarClient {
    pub async fn connect(url: &str) -> Result<Self> {
        info!("connecting to zidecar at {}", url);
        let client = GrpcClient::connect(url.to_string()).await?;
        Ok(Self { client })
    }

    /// get gigaproof + tip proof for full chain
    pub async fn get_header_proof(&mut self) -> Result<(Vec<u8>, u32, u32)> {
        let request = tonic::Request::new(ProofRequest {
            from_height: 0,
            to_height: 0, // 0 = tip
        });

        let response = self.client.get_header_proof(request).await?;
        let proof = response.into_inner();

        debug!(
            "received proof: {} -> {} ({} bytes)",
            proof.from_height,
            proof.to_height,
            proof.ligerito_proof.len()
        );

        Ok((proof.ligerito_proof, proof.from_height, proof.to_height))
    }

    /// get current chain tip
    pub async fn get_tip(&mut self) -> Result<(u32, Vec<u8>)> {
        let request = tonic::Request::new(Empty {});
        let response = self.client.get_tip(request).await?;
        let tip = response.into_inner();

        Ok((tip.height, tip.hash))
    }

    /// stream compact blocks for scanning
    pub async fn get_compact_blocks(
        &mut self,
        start_height: u32,
        end_height: u32,
    ) -> Result<tonic::Streaming<crate::zidecar::CompactBlock>> {
        let request = tonic::Request::new(BlockRange {
            start_height,
            end_height,
        });

        let response = self.client.get_compact_blocks(request).await?;
        Ok(response.into_inner())
    }
}
