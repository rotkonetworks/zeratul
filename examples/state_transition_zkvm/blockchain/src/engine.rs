//! Blockchain engine
//!
//! Coordinates all components:
//! - Application (state transitions + proof verification)
//! - Consensus (Byzantine fault tolerance)
//! - Marshal (block storage and synchronization)
//! - Broadcast (efficient block dissemination)
//! - P2P (encrypted peer communication)

use crate::{application, Block, StaticSchemeProvider};
use commonware_broadcast::buffered;
use commonware_consensus::{
    marshal,
    simplex::{self, Engine as Consensus},
    Reporters,
};
use commonware_cryptography::{
    bls12381::primitives::{group, poly::Poly},
    ed25519::PublicKey,
    sha256::Digest,
};
use commonware_p2p::{Blocker, Receiver, Sender};
use commonware_resolver::Resolver;
use commonware_runtime::{
    buffer::PoolRef, spawn_cell, Clock, ContextCell, Handle, Metrics, Spawner, Storage,
};
use commonware_utils::set::Ordered;
use commonware_utils::{NZUsize, NZU64};
use futures::{channel::mpsc, future::try_join_all};
use governor::clock::Clock as GClock;
use governor::Quota;
use rand::{CryptoRng, Rng};
use std::marker::PhantomData;
use std::{num::NonZero, time::Duration};
use tracing::{error, warn};

/// Reporter type for consensus
type Reporter = Reporters<
    commonware_consensus::simplex::Activity,
    marshal::Mailbox<commonware_consensus::simplex::Scheme<crate::Scheme, PublicKey>, Block>,
    Option<()>, // No indexer for now
>;

// Storage constants
const SYNCER_ACTIVITY_TIMEOUT_MULTIPLIER: u64 = 10;
const PRUNABLE_ITEMS_PER_SECTION: NonZero<u64> = NZU64!(4_096);
const IMMUTABLE_ITEMS_PER_SECTION: NonZero<u64> = NZU64!(262_144);
const FREEZER_TABLE_RESIZE_FREQUENCY: u8 = 4;
const FREEZER_TABLE_RESIZE_CHUNK_SIZE: u32 = 2u32.pow(16); // 3MB
const FREEZER_JOURNAL_TARGET_SIZE: u64 = 1024 * 1024 * 1024; // 1GB
const FREEZER_JOURNAL_COMPRESSION: Option<u8> = Some(3);
const REPLAY_BUFFER: NonZero<usize> = NZUsize!(8 * 1024 * 1024); // 8MB
const WRITE_BUFFER: NonZero<usize> = NZUsize!(1024 * 1024); // 1MB
const BUFFER_POOL_PAGE_SIZE: NonZero<usize> = NZUsize!(4_096); // 4KB
const BUFFER_POOL_CAPACITY: NonZero<usize> = NZUsize!(8_192); // 32MB
const MAX_REPAIR: u64 = 20;

const EPOCH_LENGTH: u64 = 100;
const NAMESPACE: &[u8] = b"zeratul";

/// Configuration for the blockchain engine
pub struct Config<B: Blocker<PublicKey = PublicKey>> {
    pub blocker: B,
    pub partition_prefix: String,
    pub blocks_freezer_table_initial_size: u32,
    pub finalized_freezer_table_initial_size: u32,
    pub me: PublicKey,
    pub polynomial: Poly<group::G1>,
    pub share: group::Share,
    pub participants: Ordered<PublicKey>,
    pub mailbox_size: usize,
    pub deque_size: usize,

    pub leader_timeout: Duration,
    pub notarization_timeout: Duration,
    pub nullify_retry: Duration,
    pub fetch_timeout: Duration,
    pub activity_timeout: u64,
    pub skip_timeout: u64,
    pub max_fetch_count: usize,
    pub max_fetch_size: usize,
    pub fetch_concurrent: usize,
    pub fetch_rate_per_peer: Quota,

    pub application_config: application::Config,
}

/// The blockchain engine
pub struct Engine<
    E: Clock + GClock + Rng + CryptoRng + Spawner + Storage + Metrics,
    B: Blocker<PublicKey = PublicKey>,
> {
    context: ContextCell<E>,

    application: application::Actor<E>,
    application_mailbox: application::Mailbox,
    buffer: buffered::Engine<E, PublicKey, Block>,
    buffer_mailbox: buffered::Mailbox<PublicKey, Block>,
    marshal: marshal::Actor<E, Block, StaticSchemeProvider, commonware_consensus::simplex::Scheme<crate::Scheme, PublicKey>>,
    marshal_mailbox: marshal::Mailbox<commonware_consensus::simplex::Scheme<crate::Scheme, PublicKey>, Block>,

    consensus: Consensus<
        E,
        PublicKey,
        commonware_consensus::simplex::Scheme<crate::Scheme, PublicKey>,
        B,
        Digest,
        application::Mailbox,
        application::Mailbox,
        Reporter,
    >,
}

impl<
        E: Clock + GClock + Rng + CryptoRng + Spawner + Storage + Metrics,
        B: Blocker<PublicKey = PublicKey>,
    > Engine<E, B>
{
    /// Create a new blockchain engine
    pub async fn new(context: E, cfg: Config<B>) -> anyhow::Result<Self> {
        // Create the application
        let (application, application_mailbox) =
            application::Actor::new(context.with_label("application"), cfg.application_config)?;

        // Create the buffer for broadcast
        let (buffer, buffer_mailbox) = buffered::Engine::new(
            context.with_label("buffer"),
            buffered::Config {
                public_key: cfg.me,
                mailbox_size: cfg.mailbox_size,
                deque_size: cfg.deque_size,
                priority: true,
                codec_config: (),
            },
        );

        // Create buffer pool for efficient memory management
        let buffer_pool = PoolRef::new(BUFFER_POOL_PAGE_SIZE, BUFFER_POOL_CAPACITY);

        // Create the signing scheme for consensus
        let scheme = commonware_consensus::simplex::Scheme::new(cfg.participants, &cfg.polynomial, cfg.share);

        // Create marshal for block storage and sync
        let (marshal, marshal_mailbox) = marshal::Actor::init(
            context.with_label("marshal"),
            marshal::Config {
                scheme_provider: scheme.clone().into(),
                epoch_length: EPOCH_LENGTH,
                partition_prefix: cfg.partition_prefix.clone(),
                mailbox_size: cfg.mailbox_size,
                view_retention_timeout: cfg
                    .activity_timeout
                    .saturating_mul(SYNCER_ACTIVITY_TIMEOUT_MULTIPLIER),
                namespace: NAMESPACE.to_vec(),
                prunable_items_per_section: PRUNABLE_ITEMS_PER_SECTION,
                immutable_items_per_section: IMMUTABLE_ITEMS_PER_SECTION,
                freezer_table_initial_size: cfg.blocks_freezer_table_initial_size,
                freezer_table_resize_frequency: FREEZER_TABLE_RESIZE_FREQUENCY,
                freezer_table_resize_chunk_size: FREEZER_TABLE_RESIZE_CHUNK_SIZE,
                freezer_journal_target_size: FREEZER_JOURNAL_TARGET_SIZE,
                freezer_journal_compression: FREEZER_JOURNAL_COMPRESSION,
                replay_buffer: REPLAY_BUFFER,
                write_buffer: WRITE_BUFFER,
                finalized_partition_prefix: format!("{}-finalized", cfg.partition_prefix),
                finalized_table_initial_size: cfg.finalized_freezer_table_initial_size,
                finalized_table_resize_frequency: FREEZER_TABLE_RESIZE_FREQUENCY,
                finalized_table_resize_chunk_size: FREEZER_TABLE_RESIZE_CHUNK_SIZE,
                finalized_journal_target_size: FREEZER_JOURNAL_TARGET_SIZE,
                finalized_journal_compression: FREEZER_JOURNAL_COMPRESSION,
                max_repair: MAX_REPAIR,
                buffer_pool,
            },
        )
        .await?;

        // Create reporters for monitoring
        let reporters = Reporters {
            activities: simplex::Activity::EPOCH,
            marshal: marshal_mailbox.clone(),
            indexer: None,
        };

        // Create consensus engine
        let consensus = Consensus::init(
            context.with_label("consensus"),
            simplex::Config {
                blocker: cfg.blocker,
                public_key: cfg.me,
                scheme: scheme.clone(),
                mailbox_size: cfg.mailbox_size,
                deque_size: cfg.deque_size,
                activity_timeout: cfg.activity_timeout,
                leader_timeout: cfg.leader_timeout,
                notarization_timeout: cfg.notarization_timeout,
                nullify_retry: cfg.nullify_retry,
                skip_timeout: cfg.skip_timeout,
            },
            application_mailbox.clone(),
            application_mailbox.clone(),
            reporters,
        )
        .await?;

        Ok(Self {
            context: ContextCell::new(context),
            application,
            application_mailbox,
            buffer,
            buffer_mailbox,
            marshal,
            marshal_mailbox,
            consensus,
        })
    }

    /// Start the blockchain engine
    pub fn start(
        self,
        pending: (Sender<PublicKey>, Receiver<PublicKey>),
        recovered: (Sender<PublicKey>, Receiver<PublicKey>),
        resolver: (Sender<PublicKey>, Receiver<PublicKey>),
        broadcast: (Sender<PublicKey>, Receiver<PublicKey>),
        marshal_resolver: impl Resolver<Digest, Block> + 'static,
    ) {
        // Start application
        self.application.start(self.marshal_mailbox.clone());

        // Start buffer
        self.buffer
            .start(broadcast.0, broadcast.1, self.marshal_mailbox.clone());

        // Start marshal
        self.marshal.start(
            resolver.0,
            resolver.1,
            self.buffer_mailbox.clone(),
            marshal_resolver,
        );

        // Start consensus
        self.consensus.start(pending, recovered);
    }
}
