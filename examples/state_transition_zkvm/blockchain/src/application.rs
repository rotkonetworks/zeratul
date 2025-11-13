//! Application layer for state transition blockchain
//!
//! This module implements the state machine that:
//! 1. Verifies AccidentalComputer proofs
//! 2. Updates NOMT state with new commitments
//! 3. Proposes new blocks with validated transactions

use crate::block::Block;
use anyhow::Result;
use commonware_consensus::marshal;
use commonware_consensus::types::Round;
use commonware_cryptography::{Digestible, Hasher, Sha256};
use commonware_macros::select;
use commonware_runtime::{spawn_cell, Clock, ContextCell, Handle, Metrics, Spawner};
use commonware_utils::SystemTimeExt;
use futures::{channel::mpsc, future, future::Either, StreamExt};
use nomt::{Blake3Hasher, KeyPath, Options, Session};
use rand::Rng;
use state_transition_circuit::{
    verify_accidental_computer, AccidentalComputerConfig, AccidentalComputerProof,
};
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, warn};

/// Genesis message to use during initialization
const GENESIS: &[u8] = b"zeratul-genesis";

/// Milliseconds in the future to allow for block timestamps
const SYNCHRONY_BOUND: u64 = 500;

/// Maximum number of proofs per block
const MAX_PROOFS_PER_BLOCK: usize = 100;

/// Configuration for the application
#[derive(Debug, Clone)]
pub struct Config {
    pub mailbox_size: usize,
    pub nomt_path: String,
    pub accidental_computer_config: AccidentalComputerConfig,
}

/// Messages sent to the application
pub enum Message {
    /// Get the genesis digest
    Genesis {
        response: tokio::sync::oneshot::Sender<commonware_cryptography::sha256::Digest>,
    },

    /// Propose a new block
    Propose {
        round: Round,
        parent: (u64, commonware_cryptography::sha256::Digest),
        response: tokio::sync::oneshot::Sender<commonware_cryptography::sha256::Digest>,
    },

    /// Broadcast a block
    Broadcast {
        payload: commonware_cryptography::sha256::Digest,
    },

    /// Verify a block
    Verify {
        round: Round,
        parent: (u64, commonware_cryptography::sha256::Digest),
        payload: commonware_cryptography::sha256::Digest,
        response: tokio::sync::oneshot::Sender<bool>,
    },

    /// A block has been finalized
    Finalized {
        block: commonware_cryptography::sha256::Digest,
    },

    /// Submit a proof to the mempool
    SubmitProof { proof: AccidentalComputerProof },
}

/// Mailbox for sending messages to the application
#[derive(Clone)]
pub struct Mailbox {
    sender: mpsc::Sender<Message>,
}

impl Mailbox {
    pub fn new(sender: mpsc::Sender<Message>) -> Self {
        Self { sender }
    }

    pub async fn genesis(
        &mut self,
    ) -> Result<commonware_cryptography::sha256::Digest, mpsc::SendError> {
        let (response, receiver) = tokio::sync::oneshot::channel();
        self.sender.send(Message::Genesis { response }).await?;
        Ok(receiver.await.unwrap())
    }

    pub async fn propose(
        &mut self,
        round: Round,
        parent: (u64, commonware_cryptography::sha256::Digest),
    ) -> Result<commonware_cryptography::sha256::Digest, mpsc::SendError> {
        let (response, receiver) = tokio::sync::oneshot::channel();
        self.sender
            .send(Message::Propose {
                round,
                parent,
                response,
            })
            .await?;
        Ok(receiver.await.unwrap())
    }

    pub async fn broadcast(
        &mut self,
        payload: commonware_cryptography::sha256::Digest,
    ) -> Result<(), mpsc::SendError> {
        self.sender.send(Message::Broadcast { payload }).await
    }

    pub async fn verify(
        &mut self,
        round: Round,
        parent: (u64, commonware_cryptography::sha256::Digest),
        payload: commonware_cryptography::sha256::Digest,
    ) -> Result<bool, mpsc::SendError> {
        let (response, receiver) = tokio::sync::oneshot::channel();
        self.sender
            .send(Message::Verify {
                round,
                parent,
                payload,
                response,
            })
            .await?;
        Ok(receiver.await.unwrap())
    }

    pub async fn finalized(
        &mut self,
        block: commonware_cryptography::sha256::Digest,
    ) -> Result<(), mpsc::SendError> {
        self.sender.send(Message::Finalized { block }).await
    }

    pub async fn submit_proof(
        &mut self,
        proof: AccidentalComputerProof,
    ) -> Result<(), mpsc::SendError> {
        self.sender.send(Message::SubmitProof { proof }).await
    }
}

/// Application actor
pub struct Actor<R: Rng + Spawner + Metrics + Clock> {
    context: ContextCell<R>,
    hasher: Sha256,
    mailbox: mpsc::Receiver<Message>,
    config: Config,

    /// NOMT session for state storage
    nomt: Arc<Mutex<Session>>,

    /// Mempool of pending proofs
    mempool: Arc<Mutex<Vec<AccidentalComputerProof>>>,

    /// Current state root
    state_root: Arc<Mutex<[u8; 32]>>,
}

impl<R: Rng + Spawner + Metrics + Clock> Actor<R> {
    /// Create a new application actor
    pub fn new(context: R, config: Config) -> Result<(Self, Mailbox)> {
        let (sender, mailbox) = mpsc::channel(config.mailbox_size);

        // Initialize NOMT
        let options = Options {
            path: config.nomt_path.clone().into(),
            cache_size_bytes: 1024 * 1024 * 1024, // 1GB cache
            ..Default::default()
        };
        let nomt_session = Session::new::<Blake3Hasher>(options)?;

        Ok((
            Self {
                context: ContextCell::new(context),
                hasher: Sha256::new(),
                mailbox,
                config,
                nomt: Arc::new(Mutex::new(nomt_session)),
                mempool: Arc::new(Mutex::new(Vec::new())),
                state_root: Arc::new(Mutex::new([0u8; 32])),
            },
            Mailbox::new(sender),
        ))
    }

    pub fn start<S: commonware_consensus::Scheme>(
        mut self,
        marshal: marshal::Mailbox<S, Block>,
    ) -> Handle<()> {
        spawn_cell!(self.context, self.run(marshal).await)
    }

    /// Run the application actor
    async fn run<S: commonware_consensus::Scheme>(
        mut self,
        mut marshal: marshal::Mailbox<S, Block>,
    ) {
        // Compute genesis digest
        self.hasher.update(GENESIS);
        let genesis_parent = self.hasher.finalize();
        let genesis = Block::genesis();
        let genesis_digest = genesis.digest();
        let built: Option<(Round, Block)> = None;
        let built = Arc::new(Mutex::new(built));

        while let Some(message) = self.mailbox.next().await {
            match message {
                Message::Genesis { response } => {
                    let _ = response.send(genesis_digest);
                }

                Message::SubmitProof { proof } => {
                    // Add proof to mempool
                    info!("Received proof for mempool");
                    self.mempool.lock().unwrap().push(proof);
                }

                Message::Propose {
                    round,
                    parent,
                    mut response,
                } => {
                    // Get the parent block
                    let parent_request = if parent.1 == genesis_digest {
                        Either::Left(future::ready(Ok(genesis.clone())))
                    } else {
                        Either::Right(
                            marshal
                                .subscribe(Some(Round::new(round.epoch(), parent.0)), parent.1)
                                .await,
                        )
                    };

                    // Spawn task to build block
                    self.context.with_label("propose").spawn({
                        let built = built.clone();
                        let mempool = self.mempool.clone();
                        let state_root = self.state_root.clone();
                        let nomt = self.nomt.clone();
                        let config = self.config.clone();

                        move |context| async move {
                            // Wait for parent
                            let parent = match parent_request.await {
                                Ok(p) => p,
                                Err(e) => {
                                    error!("Failed to get parent block: {:?}", e);
                                    return;
                                }
                            };

                            // Get proofs from mempool
                            let proofs: Vec<AccidentalComputerProof> = {
                                let mut mp = mempool.lock().unwrap();
                                mp.drain(..mp.len().min(MAX_PROOFS_PER_BLOCK))
                                    .collect()
                            };

                            info!(
                                ?round,
                                num_proofs = proofs.len(),
                                "Building block with proofs"
                            );

                            // Apply state transitions
                            let new_state_root = if !proofs.is_empty() {
                                match apply_state_transitions(
                                    &nomt,
                                    &proofs,
                                    &config.accidental_computer_config,
                                ) {
                                    Ok(root) => root,
                                    Err(e) => {
                                        error!("Failed to apply state transitions: {:?}", e);
                                        *state_root.lock().unwrap()
                                    }
                                }
                            } else {
                                *state_root.lock().unwrap()
                            };

                            // Update state root
                            *state_root.lock().unwrap() = new_state_root;

                            // Create new block
                            let mut current = context.current().epoch_millis();
                            if current <= parent.timestamp {
                                current = parent.timestamp + 1;
                            }

                            let block =
                                Block::new(parent.digest(), parent.height + 1, current, new_state_root, proofs);
                            let digest = block.digest();

                            {
                                let mut built = built.lock().unwrap();
                                *built = Some((round, block));
                            }

                            // Send digest to consensus
                            let result = response.send(digest);
                            info!(?round, ?digest, success = result.is_ok(), "Proposed new block");
                        }
                    });
                }

                Message::Broadcast { payload } => {
                    // Broadcast the last built block
                    let Some(built) = built.lock().unwrap().clone() else {
                        warn!(?payload, "missing block to broadcast");
                        continue;
                    };

                    debug!(
                        ?payload,
                        round = ?built.0,
                        height = built.1.height,
                        "broadcast requested"
                    );
                    marshal.broadcast(built.1.clone()).await;
                }

                Message::Verify {
                    round,
                    parent,
                    payload,
                    mut response,
                } => {
                    // Get parent and current block
                    let parent_request = if parent.1 == genesis_digest {
                        Either::Left(future::ready(Ok(genesis.clone())))
                    } else {
                        Either::Right(
                            marshal
                                .subscribe(Some(Round::new(round.epoch(), parent.0)), parent.1)
                                .await,
                        )
                    };

                    // Spawn verification task
                    self.context.with_label("verify").spawn({
                        let mut marshal = marshal.clone();
                        let config = self.config.clone();
                        let nomt = self.nomt.clone();

                        move |context| async move {
                            // Wait for blocks
                            let (parent, block) = match futures::future::try_join(
                                parent_request,
                                marshal.subscribe(None, payload).await,
                            )
                            .await
                            {
                                Ok(result) => result,
                                Err(e) => {
                                    error!("Failed to get blocks: {:?}", e);
                                    let _ = response.send(false);
                                    return;
                                }
                            };

                            // Verify block structure
                            if block.height != parent.height + 1 {
                                let _ = response.send(false);
                                return;
                            }
                            if block.parent != parent.digest() {
                                let _ = response.send(false);
                                return;
                            }
                            if block.timestamp <= parent.timestamp {
                                let _ = response.send(false);
                                return;
                            }
                            let current = context.current().epoch_millis();
                            if block.timestamp > current + SYNCHRONY_BOUND {
                                let _ = response.send(false);
                                return;
                            }

                            // Verify all proofs
                            for proof in &block.proofs {
                                match verify_accidental_computer(&config.accidental_computer_config, proof) {
                                    Ok(true) => continue,
                                    Ok(false) => {
                                        warn!("Proof verification failed");
                                        let _ = response.send(false);
                                        return;
                                    }
                                    Err(e) => {
                                        error!("Error verifying proof: {:?}", e);
                                        let _ = response.send(false);
                                        return;
                                    }
                                }
                            }

                            info!(
                                ?round,
                                num_proofs = block.proofs.len(),
                                "Block verified successfully"
                            );

                            // Persist the verified block
                            marshal.verified(round, block).await;

                            // Send verification result
                            let _ = response.send(true);
                        }
                    });
                }

                Message::Finalized { block } => {
                    info!(?block, "Block finalized - state transition complete");
                }
            }
        }
    }
}

/// Apply state transitions from proofs to NOMT
fn apply_state_transitions(
    nomt: &Arc<Mutex<Session>>,
    proofs: &[AccidentalComputerProof],
    config: &AccidentalComputerConfig,
) -> Result<[u8; 32]> {
    // Verify all proofs first
    for proof in proofs {
        if !verify_accidental_computer(config, proof)? {
            anyhow::bail!("Invalid proof in block");
        }
    }

    // Apply state changes to NOMT
    let mut session = nomt.lock().unwrap();
    let mut updates = Vec::new();

    for proof in proofs {
        // Update sender commitment
        let sender_key = KeyPath(proof.sender_commitment_old);
        updates.push((sender_key, Some(proof.sender_commitment_new.to_vec())));

        // Update receiver commitment
        let receiver_key = KeyPath(proof.receiver_commitment_old);
        updates.push((receiver_key, Some(proof.receiver_commitment_new.to_vec())));
    }

    // Commit updates
    let root = session.commit(&updates)?;

    // Convert root to [u8; 32]
    let mut state_root = [0u8; 32];
    state_root.copy_from_slice(&root.0);

    Ok(state_root)
}
