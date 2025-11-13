# Treasury Surplus: Not Extraction, But Recycling

**Last Updated**: 2025-11-13

## The Fair Criticism

> "If treasury earns 170x more than costs, isn't it just a middleman extracting 170x profit?"

**YES - if the surplus just accumulates!**

The difference between **extraction** and **coordination** is what happens to the surplus.

## Two Models of Surplus

### ❌ Middleman Extraction Model
```
Profit = Revenue - Costs
Profit → Shareholders/Owners
Users get nothing back
```

**Example**: Traditional bank
- Charges $10 service fee
- Costs $0.50 to provide service
- $9.50 profit → Bank shareholders
- Customer pays $10, gets nothing back

### ✅ Protocol Recycling Model
```
Surplus = Income - Operating Costs
Surplus → Returns to ecosystem participants
Creates positive feedback loop
```

**Example**: Zeratul treasury
- Income: $5,184/month
- Costs: $93/month (DA + archives)
- Surplus: $5,091/month → ???

**The question is: Where does the $5,091 go?**

## Zeratul's Surplus Distribution

### Option 1: User Rebates (Direct Return)

```rust
pub struct FeeRefund {
    refund_rate: f64,  // How much of surplus to return
}

impl Protocol {
    pub fn calculate_fee_with_rebate(&self, base_fee: Amount) -> Amount {
        // User pays base fee
        let paid = base_fee;

        // At end of epoch, treasury calculates surplus
        let surplus = self.treasury.calculate_surplus()?;

        // Return portion to recent users proportionally
        let refund = self.distribute_surplus_to_users(surplus)?;

        // Effective fee = base_fee - refund
        paid - refund
    }
}
```

**Example**:
- User pays $0.01 transaction fee
- End of month, treasury has $5,091 surplus
- Treasury distributes 50% back to users = $2,545
- User's effective fee = $0.01 - (their share of $2,545)

**Pros:**
- ✅ Directly returns value to users
- ✅ Makes fees cheaper over time
- ✅ Not extraction if users get money back!

**Cons:**
- ⚠️ Complex to calculate rebates
- ⚠️ Might incentivize spam (to get rebates)
- ⚠️ Doesn't fund public goods

### Option 2: Burn the Surplus (Deflationary)

```rust
impl Treasury {
    pub fn process_surplus(&mut self, epoch: u64) -> Result<()> {
        let operating_costs = self.calculate_costs(epoch)?;
        let income = self.calculate_income(epoch)?;
        let surplus = income - operating_costs;

        // Burn 100% of surplus
        self.burn(surplus)?;

        Ok(())
    }
}
```

**Example**:
- Treasury income: $5,184
- Operating costs: $93
- Surplus: $5,091
- **Burn $5,091** → Reduces token supply

**Pros:**
- ✅ Benefits all token holders (scarcity)
- ✅ Simple mechanism
- ✅ No extraction (value goes to everyone)

**Cons:**
- ❌ Doesn't fund public goods
- ❌ Over-deflationary might limit supply

### Option 3: Ecosystem Grants (Value Creation) ⭐ Recommended

```rust
pub struct SurplusAllocation {
    burn: u8,              // % to burn (scarcity)
    grants: u8,            // % to ecosystem grants
    reserves: u8,          // % to reserves (rainy day fund)
}

impl Treasury {
    pub fn allocate_surplus(&mut self, epoch: u64) -> Result<()> {
        let surplus = self.calculate_surplus(epoch)?;

        let allocation = SurplusAllocation {
            burn: 40,      // 40% burned (deflationary)
            grants: 50,    // 50% to ecosystem grants
            reserves: 10,  // 10% to reserves
        };

        // Burn portion
        self.burn(surplus * allocation.burn / 100)?;

        // Fund ecosystem
        self.distribute_grants(surplus * allocation.grants / 100)?;

        // Save for future
        self.reserves.deposit(surplus * allocation.reserves / 100)?;

        Ok(())
    }

    fn distribute_grants(&mut self, amount: Amount) -> Result<()> {
        // Fund public goods that create value for users
        self.fund_block_explorers(amount * 0.3)?;      // 30% - explorers
        self.fund_developer_grants(amount * 0.3)?;     // 30% - devs
        self.fund_research(amount * 0.2)?;             // 20% - research
        self.fund_community_tools(amount * 0.2)?;      // 20% - tools

        Ok(())
    }
}
```

**Example** (from $5,091 surplus):
- **Burn**: $2,036 (40%) → All token holders benefit
- **Grants**: $2,545 (50%) → Ecosystem development
  - Block explorers: $764
  - Developer grants: $764
  - Research: $509
  - Community tools: $509
- **Reserves**: $509 (10%) → Future needs

**Pros:**
- ✅ Returns value to users (burn benefits holders)
- ✅ Funds public goods (explorers, tools, dev)
- ✅ Creates long-term value
- ✅ Not extraction (value recycled into ecosystem)

**Cons:**
- ⚠️ Requires governance to allocate grants
- ⚠️ More complex than pure burn

## Why This Isn't Extraction

### The Key Difference: Who Benefits?

**Middleman extraction**:
```
User pays $10
Cost to provide: $0.50
Profit: $9.50

$9.50 → Middleman's pocket
User gets: Nothing
```

**Protocol recycling**:
```
User pays $0.01
Cost to provide: $0.000058 (operating costs)
Surplus: $0.009942

Surplus recycled:
  40% → Burned (user's tokens worth more)
  50% → Grants (better tools for user)
  10% → Reserves (future sustainability)

User gets: Deflationary token + better ecosystem
```

### Visualization

```
Traditional Middleman:
  Revenue
    ↓
  Operating Costs (minimize these!)
    ↓
  Profit → Shareholders → Extraction

Zeratul Treasury:
  Revenue
    ↓
  Operating Costs (pay fairly)
    ↓
  Surplus → Burn + Grants + Reserves → Back to users/ecosystem
```

**The surplus doesn't leave the system!**

## Concrete Example: Monthly Flows

**Assumptions:**
- 1,000 TPS
- $0.01/tx
- $5,184 treasury income/month
- $93 operating costs/month
- $5,091 surplus/month

### Traditional Bank Model (Extraction)
```
Income: $5,184
Costs: $93
Profit: $5,091

$5,091 → Bank shareholders
Users: Get nothing
```

### Zeratul Model (Recycling)
```
Income: $5,184
Costs: $93
Surplus: $5,091

Distribution:
  $2,036 → Burned (deflationary, benefits token holders)
  $764 → Block explorers (users benefit from better UX)
  $764 → Developer grants (users get new features)
  $509 → Research (users benefit from improvements)
  $509 → Community tools (users get better tooling)
  $509 → Reserves (insurance for users)

Users get:
  ✅ Deflationary token (40% burned)
  ✅ Better block explorers
  ✅ New features from devs
  ✅ Protocol improvements
  ✅ Better tools
  ✅ Long-term sustainability
```

**Value returns to users, not extracted!**

## The Governance Question

**Who decides how surplus is allocated?**

```rust
pub struct GrantProposal {
    recipient: PublicKey,
    amount: Amount,
    purpose: String,
    duration: Duration,
}

impl Validator {
    pub fn vote_on_grant(&self, proposal: GrantProposal) -> Result<()> {
        // Validators vote via FROST
        // Democratic allocation of surplus
        self.frost_sign_vote(proposal)
    }
}
```

**Example proposals:**
```
Proposal #1: Fund block explorer
  Amount: 10,000 tokens/month
  Duration: 12 months
  Purpose: Free, open-source explorer for all users
  Vote: 80% approve ✅

Proposal #2: Developer grant for new wallet
  Amount: 50,000 tokens (one-time)
  Purpose: Build mobile wallet with better UX
  Vote: 90% approve ✅

Proposal #3: Increase burn rate
  From: 40% → 60%
  Reason: Token supply too high
  Vote: 65% approve ✅
```

**Community governs surplus allocation!**

## Adjusting for Market Conditions

### If Treasury Surplus Shrinks

**Scenario**: Transaction volume drops, treasury income falls

```rust
// Before: $5,184/month income, $93 costs, $5,091 surplus
// After: $1,000/month income, $93 costs, $907 surplus

impl Treasury {
    pub fn adjust_allocation(&mut self) -> Result<()> {
        let surplus_ratio = self.calculate_surplus_ratio();

        if surplus_ratio < 10.0 {  // Less than 10x surplus
            // Reduce burn rate, increase reserves
            self.allocation = SurplusAllocation {
                burn: 20,      // Down from 40%
                grants: 50,    // Same
                reserves: 30,  // Up from 10%
            };
        }

        Ok(())
    }
}
```

**Governance can vote to adjust allocations based on needs!**

### If Treasury Surplus Grows

**Scenario**: Transaction volume 10x, treasury overflowing

```rust
// Before: $5,184/month income, $93 costs, $5,091 surplus
// After: $51,840/month income, $930 costs, $50,910 surplus

impl Treasury {
    pub fn adjust_allocation(&mut self) -> Result<()> {
        let surplus_ratio = self.calculate_surplus_ratio();

        if surplus_ratio > 50.0 {  // More than 50x surplus
            // Increase grants, maybe reduce fees!
            self.allocation = SurplusAllocation {
                burn: 30,      // Reduce burn
                grants: 70,    // Increase grants (more public goods!)
                reserves: 0,   // No need for reserves
            };

            // Or propose fee reduction!
            self.propose_fee_reduction()?;
        }

        Ok(())
    }
}
```

## Fee Reduction from Surplus

**The ultimate non-extraction**: When surplus is too high, reduce fees!

```rust
pub struct FeeAdjustmentProposal {
    current_fee: Amount,
    proposed_fee: Amount,
    reason: String,
}

// Example: Treasury has too much surplus
let proposal = FeeAdjustmentProposal {
    current_fee: 0.01,  // $0.01/tx
    proposed_fee: 0.005, // $0.005/tx (50% reduction!)
    reason: "Treasury surplus >100x operating costs, reduce user fees",
};

// Validators vote
// If approved, fees go down!
```

**This is the opposite of extraction:**
- Traditional middleman: Increase fees to maximize profit
- Zeratul: Decrease fees when surplus too high

## Summary: Extraction vs Recycling

### Middleman Extraction
```
✗ Surplus → Private shareholders
✗ Users pay, get nothing back
✗ Maximize profit motive
✗ Fees increase over time
✗ No transparency
```

### Zeratul Recycling
```
✓ Surplus → Burned + Grants + Reserves
✓ Users benefit from deflation + public goods
✓ Minimize necessary costs
✓ Fees decrease if surplus high
✓ Full transparency + governance
```

## The Philosophy

**A protocol treasury is NOT a middleman if:**

1. **Surplus is recycled** into the ecosystem (burn/grants)
2. **Governance is democratic** (validators/token holders vote)
3. **Fees adjust downward** when surplus is high
4. **Transparency is complete** (all flows on-chain)
5. **Public goods are funded** (value creation, not extraction)

**Zeratul commits to all five principles.**

---

**The difference between extraction and coordination:**

> Extraction: Value leaves the system
> Coordination: Value flows through the system and returns in different forms

**Zeratul's treasury is a coordination mechanism, not an extraction mechanism.**

The surplus doesn't disappear into private pockets - it cycles back through burns (benefiting token holders), grants (benefiting users), and reserves (benefiting long-term sustainability).

**This is what blockchain economics should be: sustainable, fair, and circular.** 🔄
