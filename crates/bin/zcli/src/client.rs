// gRPC-web client for zidecar
//
// the server runs tonic with tonic_web::enable(), but the reverse proxy
// only forwards grpc-web content types. so we speak grpc-web over reqwest
// instead of native grpc over tonic channels.
//
// grpc-web wire format:
//   request:  [0x00][4-byte big-endian len][protobuf]
//   response: [0x00][4-byte big-endian len][protobuf]  (data frame)
//             [0x80][4-byte big-endian len][trailers]   (trailer frame)

use prost::Message;
use crate::error::Error;

pub mod zidecar_proto {
    tonic::include_proto!("zidecar.v1");
}

pub mod lightwalletd_proto {
    tonic::include_proto!("cash.z.wallet.sdk.rpc");
}

// -- shared types --

#[derive(Debug, Clone)]
pub struct Utxo {
    pub address: String,
    pub txid: [u8; 32],
    pub output_index: u32,
    pub script: Vec<u8>,
    pub value_zat: u64,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub struct CompactBlock {
    pub height: u32,
    pub hash: Vec<u8>,
    pub actions: Vec<CompactAction>,
}

#[derive(Debug, Clone)]
pub struct CompactAction {
    pub cmx: [u8; 32],
    pub ephemeral_key: [u8; 32],
    pub ciphertext: Vec<u8>,
    pub nullifier: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct SendResult {
    pub txid: String,
    pub error_code: i32,
    pub error_message: String,
}

impl SendResult {
    pub fn is_success(&self) -> bool {
        self.error_code == 0
    }
}

// -- grpc-web transport --

fn grpc_web_encode(msg: &impl Message) -> Vec<u8> {
    let payload = msg.encode_to_vec();
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.push(0x00); // data frame
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&payload);
    frame
}

fn grpc_web_decode(body: &[u8]) -> Result<(Vec<u8>, u8), Error> {
    if body.len() < 5 {
        return Err(Error::Network("grpc-web: response too short".into()));
    }
    let frame_type = body[0];
    let len = u32::from_be_bytes([body[1], body[2], body[3], body[4]]) as usize;
    if body.len() < 5 + len {
        return Err(Error::Network(format!(
            "grpc-web: truncated frame (expected {} got {})", len, body.len() - 5
        )));
    }
    Ok((body[5..5 + len].to_vec(), frame_type))
}

/// decode all data frames from a grpc-web response, return concatenated payloads
fn grpc_web_decode_stream(body: &[u8]) -> Result<Vec<Vec<u8>>, Error> {
    let mut messages = Vec::new();
    let mut offset = 0;
    while offset + 5 <= body.len() {
        let frame_type = body[offset];
        let len = u32::from_be_bytes([
            body[offset + 1], body[offset + 2],
            body[offset + 3], body[offset + 4],
        ]) as usize;
        if offset + 5 + len > body.len() {
            return Err(Error::Network("grpc-web: truncated stream frame".into()));
        }
        if frame_type == 0x00 {
            messages.push(body[offset + 5..offset + 5 + len].to_vec());
        }
        // 0x80 = trailer frame, skip
        offset += 5 + len;
    }
    Ok(messages)
}

fn check_grpc_status(headers: &reqwest::header::HeaderMap, body: &[u8]) -> Result<(), Error> {
    // check header-level grpc-status first
    if let Some(status) = headers.get("grpc-status") {
        let code: i32 = status.to_str().unwrap_or("0").parse().unwrap_or(0);
        if code != 0 {
            let msg = headers.get("grpc-message")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("unknown error");
            let msg = urlencoding::decode(msg).unwrap_or_else(|_| msg.into());
            return Err(Error::Network(format!("grpc status {}: {}", code, msg)));
        }
    }

    // check trailer frame in body for grpc-status
    let mut offset = 0;
    while offset + 5 <= body.len() {
        let frame_type = body[offset];
        let len = u32::from_be_bytes([
            body[offset + 1], body[offset + 2],
            body[offset + 3], body[offset + 4],
        ]) as usize;
        if offset + 5 + len > body.len() { break; }
        if frame_type == 0x80 {
            let trailers = String::from_utf8_lossy(&body[offset + 5..offset + 5 + len]);
            for line in trailers.lines() {
                if let Some(code) = line.strip_prefix("grpc-status:") {
                    let code: i32 = code.trim().parse().unwrap_or(0);
                    if code != 0 {
                        let msg = trailers.lines()
                            .find_map(|l| l.strip_prefix("grpc-message:"))
                            .unwrap_or("unknown error")
                            .trim();
                        return Err(Error::Network(format!("grpc status {}: {}", code, msg)));
                    }
                }
            }
        }
        offset += 5 + len;
    }
    Ok(())
}

// -- zidecar client --

pub struct ZidecarClient {
    http: reqwest::Client,
    base_url: String,
}

impl ZidecarClient {
    pub async fn connect(url: &str) -> Result<Self, Error> {
        let base_url = url.trim_end_matches('/').to_string();
        let http = reqwest::Client::builder()
            .build()
            .map_err(|e| Error::Network(format!("http client: {}", e)))?;
        Ok(Self { http, base_url })
    }

    async fn call_unary<Req: Message, Resp: Message + Default>(
        &self, method: &str, req: &Req,
    ) -> Result<Resp, Error> {
        let url = format!("{}/{}", self.base_url, method);
        let body = grpc_web_encode(req);

        let resp = self.http.post(&url)
            .header("content-type", "application/grpc-web+proto")
            .header("x-grpc-web", "1")
            .body(body)
            .send()
            .await
            .map_err(|e| Error::Network(format!("{}: {}", method, e)))?;

        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.bytes().await
            .map_err(|e| Error::Network(format!("{}: read body: {}", method, e)))?;

        if !status.is_success() {
            return Err(Error::Network(format!("{}: HTTP {}", method, status)));
        }

        check_grpc_status(&headers, &bytes)?;

        let (payload, _) = grpc_web_decode(&bytes)?;
        Resp::decode(payload.as_slice())
            .map_err(|e| Error::Network(format!("{}: decode: {}", method, e)))
    }

    async fn call_server_stream<Req: Message, Resp: Message + Default>(
        &self, method: &str, req: &Req,
    ) -> Result<Vec<Resp>, Error> {
        let url = format!("{}/{}", self.base_url, method);
        let body = grpc_web_encode(req);

        let resp = self.http.post(&url)
            .header("content-type", "application/grpc-web+proto")
            .header("x-grpc-web", "1")
            .timeout(std::time::Duration::from_secs(120))
            .body(body)
            .send()
            .await
            .map_err(|e| Error::Network(format!("{}: {}", method, e)))?;

        let status = resp.status();
        let headers = resp.headers().clone();

        if !status.is_success() {
            return Err(Error::Network(format!("{}: HTTP {}", method, status)));
        }

        check_grpc_status(&headers, &[])?;

        // read full response body (grpc-web buffers server-stream in single response)
        let bytes = resp.bytes().await
            .map_err(|e| Error::Network(format!("{}: read body: {}", method, e)))?;

        let frames = grpc_web_decode_stream(&bytes)?;
        frames.iter().map(|f| {
            Resp::decode(f.as_slice())
                .map_err(|e| Error::Network(format!("{}: decode: {}", method, e)))
        }).collect()
    }

    pub async fn get_tip(&self) -> Result<(u32, Vec<u8>), Error> {
        let tip: zidecar_proto::BlockId = self.call_unary(
            "zidecar.v1.Zidecar/GetTip",
            &zidecar_proto::Empty {},
        ).await?;
        Ok((tip.height, tip.hash))
    }

    pub async fn get_address_utxos(&self, addresses: Vec<String>) -> Result<Vec<Utxo>, Error> {
        let resp: zidecar_proto::UtxoList = self.call_unary(
            "zidecar.v1.Zidecar/GetAddressUtxos",
            &zidecar_proto::TransparentAddressFilter {
                addresses,
                start_height: 0,
                max_entries: 0,
            },
        ).await?;

        Ok(resp.utxos.into_iter().map(|u| {
            let mut txid = [0u8; 32];
            if u.txid.len() == 32 { txid.copy_from_slice(&u.txid); }
            Utxo {
                address: u.address,
                txid,
                output_index: u.output_index,
                script: u.script,
                value_zat: u.value_zat,
                height: u.height,
            }
        }).collect())
    }

    pub async fn get_compact_blocks(
        &self,
        start_height: u32,
        end_height: u32,
    ) -> Result<Vec<CompactBlock>, Error> {
        let protos: Vec<zidecar_proto::CompactBlock> = self.call_server_stream(
            "zidecar.v1.Zidecar/GetCompactBlocks",
            &zidecar_proto::BlockRange { start_height, end_height },
        ).await?;

        Ok(protos.into_iter().map(|block| {
            let actions = block.actions.into_iter().filter_map(|a| {
                if a.cmx.len() != 32 || a.ephemeral_key.len() != 32 || a.nullifier.len() != 32 {
                    return None;
                }
                let mut cmx = [0u8; 32];
                let mut ek = [0u8; 32];
                let mut nf = [0u8; 32];
                cmx.copy_from_slice(&a.cmx);
                ek.copy_from_slice(&a.ephemeral_key);
                nf.copy_from_slice(&a.nullifier);
                Some(CompactAction {
                    cmx,
                    ephemeral_key: ek,
                    ciphertext: a.ciphertext,
                    nullifier: nf,
                })
            }).collect();

            CompactBlock {
                height: block.height,
                hash: block.hash,
                actions,
            }
        }).collect())
    }

    pub async fn get_tree_state(&self, height: u32) -> Result<(String, u32), Error> {
        let state: zidecar_proto::TreeState = self.call_unary(
            "zidecar.v1.Zidecar/GetTreeState",
            &zidecar_proto::BlockId { height, hash: vec![] },
        ).await?;
        Ok((state.orchard_tree, state.height))
    }

    pub async fn send_transaction(&self, tx_data: Vec<u8>) -> Result<SendResult, Error> {
        let r: zidecar_proto::SendResponse = self.call_unary(
            "zidecar.v1.Zidecar/SendTransaction",
            &zidecar_proto::RawTransaction { data: tx_data, height: 0 },
        ).await?;
        Ok(SendResult {
            txid: r.txid,
            error_code: r.error_code,
            error_message: r.error_message,
        })
    }
}
