# Zeratul Design Philosophy: Removing Middlemen, Not Becoming One

**Last Updated**: 2025-11-13

## The Core Paradox

> "Technology that cuts the middle man shouldn't act like a middle man"

### The Problem with Fee-Per-Service Models

Many blockchain protocols claim to remove intermediaries but then reintroduce them in the architecture:

```
Traditional Finance:
User → Pay Bank Fee → Get Service → Bank Extracts Value

Many Blockchain "Solutions":
User → Pay Transaction Fee → Pay DA Fee → Pay Sequencer Fee → Pay Relayer Fee
     ↑ Still extracting value at every step!
```

**This isn't disintermediation - it's just different intermediaries!**

## Two Approaches to Blockchain Economics

### Approach A: Fee-Per-Service (Becoming the Middleman)

```rust
// User pays separately for each service
let tx_fee = 0.01;           // Base transaction
let da_fee = 0.005;          // Data availability
let sequencer_fee = 0.002;   // Ordering
let relayer_fee = 0.001;     // Submission
// Total: $0.018 with 4 separate "middlemen"
```

**Examples**: Many L2 rollups, data availability layers

**Problems**:
- ❌ User must understand multiple fee markets
- ❌ Each service provider extracts rent
- ❌ Complexity grows with each new service
- ❌ No alignment between service providers
- ❌ **Acts like middleman, just decentralized**

### Approach B: Protocol-Native Treasury (Zeratul's Model)

```rust
// User pays ONE fee
let tx_fee = 0.01;           // Everything included!

// Protocol allocates internally
let distribution = FeeDistribution {
    validators: 0.007,       // 70% - secure the network
    treasury: 0.002,         // 20% - fund public goods (DA, archives)
    burn: 0.001,             // 10% - deflationary
};

// Treasury funds services WITHOUT charging users
treasury.pay_da_nodes(epoch);
treasury.pay_archive_nodes(epoch);
treasury.fund_public_goods();
```

**Examples**: Early Bitcoin, Ethereum (pre-MEV), **Zeratul**

**Benefits**:
- ✅ Simple UX (one fee)
- ✅ Protocol funds its own infrastructure
- ✅ Aligned incentives (everyone benefits from treasury)
- ✅ No rent extraction per-service
- ✅ **Truly removes middlemen**

## The Philosophical Difference

### Middleman Model (What We're Avoiding)

```
User needs data stored
    ↓
User negotiates with DA provider
    ↓
User pays DA provider directly
    ↓
DA provider extracts profit margin
    ↓
Repeat for every service
```

**Result**: Decentralized middlemen. Still extracting value at each step.

### Protocol Treasury Model (Zeratul)

```
User wants to transact
    ↓
User pays single transaction fee
    ↓
Protocol ensures all services happen
    ↓
No individual extraction per service
    ↓
Surplus returns to community treasury
```

**Result**: True disintermediation. Protocol IS the coordination layer.

## Real-World Analogies

### The Internet vs Telecommunication

**Old Model (Telecom)**:
- Pay per minute for calls
- Pay per SMS
- Pay per MB of data
- Each service priced separately
- Telecom companies extract value at each step

**Internet Model**:
- Pay flat monthly fee
- Infrastructure funded from that pool
- Use as much as you need
- Service providers compete on quality, not rent extraction

**Zeratul = Internet model, not Telecom model**

### Public Infrastructure vs Toll Roads

**Toll Road Model (Fee-Per-Service)**:
```
Every bridge: $5
Every tunnel: $3
Every highway entrance: $2
Every service: separate payment
```

**Public Infrastructure Model (Treasury)**:
```
Pay taxes once
Government builds/maintains all roads
Use freely
Surplus improves infrastructure
```

**Zeratul = Public infrastructure model**

## Why This Matters for Zeratul

### Case Study: Data Availability

#### The Middleman Approach ❌
```rust
// User's perspective
let proof = generate_proof(witness);

// Now I need to pay ANOTHER party for DA
let da_provider = find_cheapest_da_provider()?;
let da_fee = da_provider.quote_price(proof.size())?;

// Pay the DA middleman
da_provider.store(proof, da_fee)?;

// Still need to pay transaction fee!
network.submit(proof, tx_fee)?;

// Total: tx_fee + da_fee
// User dealt with 2 separate parties
```

**Problems**:
- User must find DA provider
- User must negotiate price
- User trusts DA provider not to lose data
- DA provider extracts profit
- **DA provider is a middleman**

#### The Treasury Approach ✅
```rust
// User's perspective
let proof = generate_proof(witness);

// Just submit with single fee
network.submit(proof, tx_fee)?;

// Done! Protocol handles DA internally
// No middleman, no negotiation, no extra fees
```

**Behind the scenes** (invisible to user):
```rust
// Protocol handles everything
impl Protocol {
    fn process_transaction(&mut self, tx: Transaction) -> Result<()> {
        // 1. Validate and execute
        self.execute(tx)?;

        // 2. Split fee
        let distribution = self.split_fee(tx.fee);

        // 3. Pay validators
        self.pay_validators(distribution.validators)?;

        // 4. Treasury funds DA (not user!)
        self.treasury.deposit(distribution.treasury)?;

        // 5. Treasury pays DA nodes later
        // (from accumulated funds, not per-transaction)
        if self.is_epoch_end() {
            self.treasury.pay_da_nodes(self.current_epoch())?;
        }

        Ok(())
    }
}
```

**Benefits**:
- User doesn't know DA exists
- User doesn't negotiate with DA providers
- User pays once
- Protocol ensures data availability
- **No middleman**

## The Governance Question

**With fee-per-service**: Each service provider sets their own prices
- DA providers compete on price
- Users must comparison shop
- Race to the bottom on quality
- Extractive behavior incentivized

**With treasury model**: Community governs collectively
- Validators vote on treasury allocation
- Predictable, stable funding
- Quality incentivized (good services get more funding)
- Cooperative, not competitive

```rust
// Governance example
pub enum TreasuryProposal {
    IncreaseDAFunding {
        from_percent: u8,
        to_percent: u8,
        reason: String,
    },
    FundNewArchiveNode {
        operator: PublicKey,
        monthly_amount: Amount,
        duration_months: u32,
    },
}

// Validators vote via FROST threshold signatures
impl Validator {
    pub fn vote(&self, proposal: TreasuryProposal) -> Result<()> {
        // Democratic decision making
        // Not "whoever charges least wins"
        self.frost_sign_vote(proposal)
    }
}
```

## Addressing Common Criticisms

### "Treasury model is centralized!"

**Response**: No more centralized than fee markets

**Fee market**:
- DA providers compete
- Largest providers win (economies of scale)
- Tends toward oligopoly
- Users have no say in pricing

**Treasury model**:
- Validators vote on allocation
- Anyone can become DA node
- Democratic governance
- Users influence via validator selection

**Both have governance, treasury is more transparent**

### "What if treasury runs out?"

**Response**: Sustainable by design

**Math** (at 1000 TPS, $0.01/tx):
- Treasury income: $5,184/month
- DA costs: $33/month (30-day retention, 5 nodes)
- Archive costs: $60/month (3 nodes, year 3)
- **Surplus: $5,091/month**

**Treasury income scales with usage:**
- 10x traffic = 10x treasury income = 10x budget for infrastructure
- Self-sustaining growth

### "What if a service becomes expensive?"

**Response**: Governance adjusts

```rust
// If storage costs spike
let proposal = TreasuryProposal::IncreaseDAFunding {
    from_percent: 40,  // Old allocation
    to_percent: 60,    // New allocation
    reason: "Storage costs increased 50%",
};

// Validators vote
// If approved, DA nodes get more funding automatically
```

**Alternative in fee-per-service model**:
- Users pay more
- No protection from price spikes
- Services can hold users hostage

## Examples in the Wild

### Bitcoin: Pure Protocol Treasury
- Block rewards fund miners
- Transaction fees supplement
- No separate "DA fee" or "consensus fee"
- **Works because protocol funds infrastructure**

### Ethereum (pre-MEV): Protocol Treasury
- Gas fees went to miners
- Miners secured network + stored state
- No separate payments for services
- **Broke when MEV introduced middlemen**

### Modern L2s: Reintroduced Middlemen
- Transaction fees
- Data availability fees
- Sequencer fees
- Prover fees
- **Each layer extracts rent**

### Zeratul: Return to Protocol Treasury
- One transaction fee
- Treasury funds all infrastructure
- No per-service extraction
- **True disintermediation**

## The Lesson

**Building decentralized systems isn't enough.**

**We must also:**
- Avoid recreating rent-seeking structures
- Internalize infrastructure costs to the protocol
- Use treasury models, not fee-per-service
- Govern collectively, not through markets

## Zeratul's Commitment

We commit to:

1. **Single-fee model**: Users pay ONE transaction fee
2. **Treasury-funded infrastructure**: DA, archives, public goods funded from treasury
3. **No rent extraction**: Service providers earn from treasury, not per-transaction
4. **Democratic governance**: Validators vote on treasury allocation
5. **Sustainable economics**: Treasury income grows with usage

**The goal**: Make blockchain infrastructure as invisible and natural as the Internet itself.

**The method**: Remove middlemen, don't become them.

## Conclusion

> "Technology should serve users, not extract from them"

Traditional finance has banks, payment processors, custodians - each extracting fees.

Many blockchains replace these with sequencers, DA providers, relayers - still extracting fees.

**Zeratul internalizes all infrastructure to the protocol:**
- Users interact with protocol directly
- Protocol funds its own operation
- No intermediaries, no rent seeking
- True peer-to-peer

**This is the promise of blockchain, fulfilled.**

---

*"The best technology is invisible. The best economics is inclusive. The best governance is democratic."*

*- Zeratul Design Principles*
