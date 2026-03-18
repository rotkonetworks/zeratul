//! staking tiers for poker arbitration
//!
//! each tier sets the max pot size, required jury stake, and jury deposit.
//! the deposit discourages frivolous disputes — both players lock it at table
//! creation and the loser forfeits it on dispute.

/// stake tier for a poker table
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tier {
    /// max 0.1 ZEC pot
    Micro,
    /// max 1 ZEC pot
    Regular,
    /// max 10 ZEC pot
    High,
    /// max 100 ZEC pot
    Whale,
}

/// amounts in zatoshi (1 ZEC = 100_000_000 zat)
const ZEC: u64 = 100_000_000;

impl Tier {
    /// maximum total pot size for this tier
    pub fn max_pot(&self) -> u64 {
        match self {
            Tier::Micro   => ZEC / 10,    // 0.1 ZEC
            Tier::Regular => ZEC,         // 1 ZEC
            Tier::High    => 10 * ZEC,    // 10 ZEC
            Tier::Whale   => 100 * ZEC,   // 100 ZEC
        }
    }

    /// minimum stake each jury node must hold
    pub fn min_jury_stake(&self) -> u64 {
        match self {
            Tier::Micro   => ZEC / 10,    // 0.1 ZEC
            Tier::Regular => ZEC,         // 1 ZEC
            Tier::High    => 10 * ZEC,    // 10 ZEC
            Tier::Whale   => 100 * ZEC,   // 100 ZEC
        }
    }

    /// deposit each player locks at table creation (refunded on happy path)
    pub fn jury_deposit(&self) -> u64 {
        match self {
            Tier::Micro   => ZEC / 1000,  // 0.001 ZEC
            Tier::Regular => ZEC / 200,   // 0.005 ZEC
            Tier::High    => ZEC / 50,    // 0.02 ZEC
            Tier::Whale   => ZEC / 10,    // 0.1 ZEC
        }
    }

    /// determine tier from a buy-in amount
    pub fn from_buy_in(amount: u64) -> Self {
        if amount <= ZEC / 20 {          // ≤ 0.05 ZEC
            Tier::Micro
        } else if amount <= ZEC / 2 {    // ≤ 0.5 ZEC
            Tier::Regular
        } else if amount <= 5 * ZEC {    // ≤ 5 ZEC
            Tier::High
        } else {
            Tier::Whale
        }
    }
}

/// rake parameters
pub struct Rake;

impl Rake {
    /// total rake as basis points (0.4% = 40 bps)
    pub const TOTAL_BPS: u64 = 40;

    /// protocol treasury share of rake (10%)
    pub const TREASURY_BPS: u64 = 4;

    /// jury pool share of rake (90%)
    pub const JURY_POOL_BPS: u64 = 36;

    /// compute rake from a pot amount
    pub fn compute(pot: u64) -> RakeBreakdown {
        let total = pot * Self::TOTAL_BPS / 10_000;
        let treasury = pot * Self::TREASURY_BPS / 10_000;
        let jury_pool = total - treasury;
        RakeBreakdown { total, treasury, jury_pool }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RakeBreakdown {
    pub total: u64,
    pub treasury: u64,
    pub jury_pool: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_max_pot() {
        assert_eq!(Tier::Micro.max_pot(), 10_000_000);
        assert_eq!(Tier::Regular.max_pot(), 100_000_000);
        assert_eq!(Tier::High.max_pot(), 1_000_000_000);
        assert_eq!(Tier::Whale.max_pot(), 10_000_000_000);
    }

    #[test]
    fn test_rake_breakdown() {
        let pot = 1_000_000; // 0.01 ZEC
        let rake = Rake::compute(pot);
        assert_eq!(rake.total, 4_000);    // 0.4%
        assert_eq!(rake.treasury, 400);    // 0.04%
        assert_eq!(rake.jury_pool, 3_600); // 0.36%
    }

    #[test]
    fn test_tier_from_buy_in() {
        assert_eq!(Tier::from_buy_in(1_000_000), Tier::Micro);
        assert_eq!(Tier::from_buy_in(50_000_000), Tier::Regular);
        assert_eq!(Tier::from_buy_in(200_000_000), Tier::High);
        assert_eq!(Tier::from_buy_in(600_000_000), Tier::Whale);
    }

    #[test]
    fn test_deposit_less_than_stake() {
        for tier in [Tier::Micro, Tier::Regular, Tier::High, Tier::Whale] {
            assert!(tier.jury_deposit() < tier.min_jury_stake(),
                "jury deposit must be less than stake for {:?}", tier);
        }
    }
}
