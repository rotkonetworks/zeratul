# disputes

disputes resolve disagreements by taking them on-chain. the protocol ensures honest players are always protected.

## dispute triggers

```
when disputes happen:

1. player goes offline (timeout)
   - other player can't continue
   - submit latest state, start countdown

2. player sends invalid action
   - reject off-chain, revert to last valid state
   - if persistent, go on-chain

3. player tries to cheat
   - submits old state on-chain
   - other player challenges with newer state

4. player refuses to sign
   - valid action but won't counter-sign
   - force resolution on-chain
```

## dispute flow

```
                 ┌─────────────────────────────────────────┐
                 │          NORMAL GAMEPLAY                 │
                 │  (off-chain, instant, cooperative)       │
                 └─────────────────────────────────────────┘
                                  │
                                  │ problem detected
                                  ▼
    ┌─────────────────────────────────────────────────────────────┐
    │                    INITIATE DISPUTE                          │
    │  player submits: latest signed state + evidence              │
    └─────────────────────────────────────────────────────────────┘
                                  │
                                  │ {{DISPUTE_TIMEOUT_BLOCKS}} blocks
                                  ▼
    ┌─────────────────────────────────────────────────────────────┐
    │                   CHALLENGE PERIOD                           │
    │  other player can submit: newer state OR fraud proof         │
    └─────────────────────────────────────────────────────────────┘
                                  │
           ┌──────────────────────┼──────────────────────┐
           │                      │                      │
           ▼                      ▼                      ▼
    no challenge            newer state            fraud proof
           │                submitted              submitted
           │                      │                      │
           ▼                      ▼                      ▼
    original state          newer state            cheater
    wins                    wins                   penalized
```

## on-chain contract

```rust
struct DisputeContract {
    /// active challenges by channel
    challenges: HashMap<ChannelId, Challenge>,
}

struct Challenge {
    /// submitted state
    state: ChannelState,
    /// who submitted
    submitter: Address,
    /// when submitted
    submitted_at: BlockNumber,
    /// deadline for challenges
    deadline: BlockNumber,
    /// evidence (if fraud proof)
    evidence: Option<Evidence>,
}

impl DisputeContract {
    /// initiate dispute with latest state
    fn initiate(
        &mut self,
        channel_id: ChannelId,
        state: ChannelState,
    ) -> Result<(), DisputeError> {
        // verify caller is participant
        // verify signatures
        // set challenge with deadline

        self.challenges.insert(channel_id, Challenge {
            state,
            submitter: msg.sender,
            submitted_at: block.number,
            deadline: block.number + {{DISPUTE_TIMEOUT_BLOCKS}},
            evidence: None,
        });

        Ok(())
    }

    /// challenge with newer state
    fn challenge(
        &mut self,
        channel_id: ChannelId,
        newer_state: ChannelState,
    ) -> Result<(), DisputeError> {
        let challenge = self.challenges.get_mut(&channel_id)?;

        // verify still in challenge period
        require(block.number < challenge.deadline)?;

        // verify newer version
        require(newer_state.version > challenge.state.version)?;

        // verify signatures
        verify_signatures(&newer_state)?;

        // update challenge with newer state
        challenge.state = newer_state;
        challenge.submitter = msg.sender;

        Ok(())
    }

    /// resolve after deadline
    fn resolve(
        &mut self,
        channel_id: ChannelId,
    ) -> Result<(), DisputeError> {
        let challenge = self.challenges.remove(&channel_id)?;

        // verify past deadline
        require(block.number >= challenge.deadline)?;

        // distribute funds per final state
        let channel = channels.get(&channel_id)?;
        for (player, balance) in channel.players.iter()
            .zip(challenge.state.balances.iter())
        {
            transfer(player, *balance);
        }

        // charge dispute fee
        let fee = calculate_dispute_fee(&challenge);
        // fee comes from loser's bond

        emit(DisputeResolved {
            channel_id,
            winner: determine_winner(&challenge),
            final_balances: challenge.state.balances,
        });

        Ok(())
    }
}
```

## fraud proofs

prove specific cheating:

```rust
enum FraudProof {
    /// invalid shuffle (ZK proof failed)
    InvalidShuffle {
        state_version: u64,
        deck: EncryptedDeck,
        invalid_proof: ShuffleProof,
    },

    /// invalid card reveal
    InvalidReveal {
        state_version: u64,
        card_position: u8,
        bad_share: DecryptionShare,
    },

    /// impossible game state
    InvalidGameState {
        state_version: u64,
        reason: InvalidReason,
    },
}

fn verify_fraud_proof(
    channel: &Channel,
    state: &ChannelState,
    proof: &FraudProof,
) -> Result<Address, FraudError> {
    match proof {
        FraudProof::InvalidShuffle { deck, invalid_proof, .. } => {
            // verify shuffle proof is actually invalid
            if verify_shuffle(deck, invalid_proof).is_ok() {
                return Err(FraudError::ProofActuallyValid);
            }
            // determine who submitted invalid shuffle
            Ok(extract_shuffler(state))
        }

        // ... other fraud types
    }
}
```

## dispute fees

```
fee structure:
  - base fee: {{DISPUTE_FEE_PERCENT}}% of pot
  - covers: gas costs + resolution
  - paid by: dispute loser

bond system:
  - each player posts ${{BOND_AMOUNT_USD}} bond
  - winner: bond returned + winnings
  - loser: bond covers fees
  - net effect: winner made whole
```

## timeout disputes

most common dispute type:

```
scenario: player B stops responding

timeline:
  t=0:    last valid state v5, B's turn
  t=30s:  A sends reminder
  t=60s:  A sends final warning
  t=120s: A initiates dispute on-chain

  t=120s + {{DISPUTE_TIMEOUT_BLOCKS}} blocks:
    if B doesn't respond: A wins
    if B responds with v5: game continues
    if B responds with v6: v6 wins (B was just slow)
```

## double-spend prevention

can't use old states to steal:

```
attack: submit state v3 when v5 exists

defense:
  1. challenge period allows counter-submission
  2. v5 has higher version, automatically wins
  3. attacker loses dispute fee
  4. attacker's reputation damaged
```

## mid-game disputes

resolving with active game state:

```
if dispute during hand:
  option 1: void hand, return bets to players
  option 2: evaluate current state, best hand wins
  option 3: timeout player forfeits

default: timeout player forfeits
  - incentivizes responsive play
  - clear resolution
  - no subjective judgment
```

## dispute statistics

on-chain reputation includes:

```rust
struct PlayerDisputeHistory {
    /// disputes initiated by this player
    disputes_initiated: u64,
    /// disputes where this player was at fault
    disputes_lost: u64,
    /// total dispute fees paid
    fees_paid: u64,
}

// high dispute rate = bad actor
// tables can filter by dispute history
```
