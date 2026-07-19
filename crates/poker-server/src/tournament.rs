//! Single-elimination tournament bracket — the NON-CUSTODIAL matchmaker core.
//!
//! The server only pairs players and advances winners; it NEVER holds funds. Free tournaments
//! are chip-only. For PAID tournaments each match is its own 2-player P2P FROST escrow and the
//! winner rolls both stakes into the next round, so the org is a scoreboard, not a custodian —
//! the property that keeps this out of money-transmitter / custody territory.
//!
//! This module is pure logic (no I/O, no money) so it's exhaustively testable in isolation.

use std::collections::HashMap;

pub type PlayerId = String;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TournState {
    /// accepting registrations
    Registering,
    /// bracket built, matches in progress
    Running,
    /// champion decided
    Finished,
    /// scheduled start time passed with fewer than 2 players (or a bracket that couldn't be
    /// built) — auto-cancelled. No custody, so nothing to refund.
    Cancelled,
}

/// A tournament sponsor — branding shown to every participant (logo + click-through URL). The
/// sponsor NEVER holds player funds; sponsorship is advertising, not custody, so it adds no
/// regulatory surface. A sponsor MAY pledge an `added_prize` they pay the champion DIRECTLY
/// (peer-to-peer), announced up front — the org still touches nothing.
/// Sponsor tier. GOLD = the tournament creator's own unescrowed pledge (a trusted promise; the org
/// touches no funds). PLATINUM = a permissionless third-party sponsor whose prize is only real once
/// a 2-of-3 FROST {sponsor, creator, bot} escrow has landed the funds (Phase B). Until then a
/// platinum entry is `funded: false` and is shown as "pending," never counted toward the prize.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SponsorTier {
    Gold,
    Platinum,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Sponsor {
    pub name: String,
    pub logo_url: String,
    pub url: String,
    /// pledged (gold) or escrowed (platinum) prize (zatoshi), paid to the champion. 0 = branding only.
    pub added_prize_zat: u64,
    /// handle that added this entry — owner-gates edit/remove (the sponsor themself, or the organizer).
    pub by: PlayerId,
    /// GOLD (organizer, unescrowed pledge) vs PLATINUM (third-party, escrow-backed).
    pub tier: SponsorTier,
    /// PLATINUM only: has the 2-of-3 escrow actually landed the funds? Gold is always effectively
    /// funded (a trusted promise). The escrow watcher flips this true; the add-path never can.
    #[serde(default)]
    pub funded: bool,
    /// PLATINUM escrow/vault room id, set by the escrow flow. None until a vault is opened.
    #[serde(default)]
    pub escrow_room: Option<String>,
}

impl Sponsor {
    /// Does this entry's prize count toward the champion prize? Gold (organizer pledge) always
    /// counts; platinum counts ONLY once its escrow is funded.
    pub fn counts(&self) -> bool {
        matches!(self.tier, SponsorTier::Gold) || self.funded
    }
}

/// One bracket slot. `a`/`b` are `None` until the feeding match resolves (or `None` = a bye).
#[derive(Clone, Debug, serde::Serialize)]
pub struct Match {
    pub id: u32,
    /// 1-based round number (round 1 = first games; `rounds` = the final).
    pub round: u32,
    pub a: Option<PlayerId>,
    pub b: Option<PlayerId>,
    pub winner: Option<PlayerId>,
    /// Stake EACH player must deposit into this match's own P2P escrow (zatoshi). Doubles per round
    /// (winner rolls both stakes forward), so round r stake = buyin × 2^(r-1). 0 for free play.
    /// The escrow is per-match and settles to the winner's own wallet — nothing is held between
    /// rounds, so there is no custody: the winner re-deposits the (larger) stake for the next match.
    #[serde(default)]
    pub stake_zat: u64,
}

impl Match {
    /// A match is playable when both seats are filled and it isn't already decided.
    pub fn is_playable(&self) -> bool {
        self.a.is_some() && self.b.is_some() && self.winner.is_none()
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Tournament {
    pub id: String,
    pub name: String,
    /// whoever created it — tournaments are permissionless, anyone can organize one. The organizer
    /// may `start` it and attach a sponsor; they hold no funds and no special custody power.
    pub organizer: PlayerId,
    /// true = paid (per-match P2P FROST escrow, roll-over); false = free chip-only.
    pub paid: bool,
    /// buy-in in zatoshi for paid tournaments (the round-1 stake; unused when `!paid`).
    pub buyin_zat: u64,
    /// sponsors shown to all participants (add-order). Gold = organizer pledge; platinum =
    /// permissionless, escrow-gated. See `add_sponsor` / `remove_sponsor` / `total_prize`.
    pub sponsors: Vec<Sponsor>,
    pub state: TournState,
    pub players: Vec<PlayerId>,
    pub matches: Vec<Match>,
    /// total rounds = ceil(log2(N)). 0 until started.
    pub rounds: u32,
    /// unix seconds when the tournament auto-starts (None = organizer starts it manually). At this
    /// time the relay ticker starts the bracket if ≥2 players are registered, else cancels it.
    #[serde(default)]
    pub scheduled_start: Option<u64>,
    /// how much of the pot the winner re-risks into the next round, in basis points. 10000 = 100%
    /// (stake doubles each round → winner-take-all). 7500 = ×1.5 per round; 5000 = flat stake, so
    /// the winner banks half the pot each round ("everybody a bit a winner"). Paid tournaments only.
    pub roll_bps: u16,
}

impl Tournament {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        organizer: impl Into<String>,
        paid: bool,
        buyin_zat: u64,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            organizer: organizer.into(),
            paid,
            buyin_zat,
            sponsors: Vec::new(),
            state: TournState::Registering,
            players: Vec::new(),
            matches: Vec::new(),
            rounds: 0,
            scheduled_start: None,
            roll_bps: 10000, // 100% roll-forward (classic winner-take-all doubling) by default
        }
    }

    /// Attach (or replace) sponsor branding. Allowed any time — a sponsor can back an already-open
    /// tournament. Non-custodial: sponsorship is advertising; any pledged prize is paid by the
    /// sponsor directly to the champion, never held by the org.
    /// Permissionless append. The organizer's own entry is GOLD (trusted pledge, counts
    /// immediately); anyone else is PLATINUM and starts `funded=false` — shown as "pending" and
    /// NOT counted until the escrow watcher flips `funded`. `who` is the authenticated caller;
    /// `by`/`tier`/`funded` are stamped HERE, ignoring whatever the caller put in `s`.
    pub fn add_sponsor(&mut self, who: PlayerId, mut s: Sponsor) {
        if who == self.organizer {
            s.tier = SponsorTier::Gold;
            s.funded = true; // an unescrowed trusted promise → always counts
        } else {
            s.tier = SponsorTier::Platinum;
            s.funded = false; // not counted / not "verified" until the 2-of-3 escrow lands
        }
        s.escrow_room = None;
        s.by = who;
        self.sponsors.push(s);
    }

    /// Remove the sponsor entry added by `target_by`. Allowed if `who` is that sponsor
    /// (self-remove) or the organizer (moderation). Err if no such entry or not permitted.
    pub fn remove_sponsor(&mut self, who: &str, target_by: &str) -> Result<(), String> {
        let idx = self.sponsors.iter().position(|s| s.by == target_by)
            .ok_or_else(|| "no such sponsor".to_string())?;
        if who != target_by && who != self.organizer {
            return Err("only the sponsor or the organizer can remove this sponsor".into());
        }
        self.sponsors.remove(idx);
        Ok(())
    }

    /// Escrow-watcher hook (Phase B): mark a platinum sponsor funded once its 2-of-3 vault
    /// confirms, recording the vault room id. No player gating — called from the escrow poller.
    pub fn mark_sponsor_funded(&mut self, target_by: &str, escrow_room: String) -> Result<(), String> {
        let s = self.sponsors.iter_mut().find(|s| s.by == target_by)
            .ok_or_else(|| "no such sponsor".to_string())?;
        if !matches!(s.tier, SponsorTier::Platinum) {
            return Err("only platinum sponsors are escrow-funded".into());
        }
        s.funded = true;
        s.escrow_room = Some(escrow_room);
        Ok(())
    }

    /// Sum of prizes that COUNT: gold always, platinum only when funded. Unfunded platinum is
    /// excluded (renders as "pending"). Saturating to avoid overflow on absurd fields.
    pub fn total_prize(&self) -> u64 {
        self.sponsors.iter().filter(|s| s.counts())
            .fold(0u64, |acc, s| acc.saturating_add(s.added_prize_zat))
    }

    /// Cancel the tournament (organizer action). Allowed while registering or running; a finished
    /// one can't be undone. Non-custodial, so nothing to refund for free play; platinum sponsor
    /// vaults (Phase B) route to their own refund on cancel.
    pub fn cancel(&mut self) -> Result<(), String> {
        match self.state {
            TournState::Finished => Err("tournament already finished".into()),
            TournState::Cancelled => Ok(()), // idempotent
            _ => { self.state = TournState::Cancelled; Ok(()) }
        }
    }

    /// Stake each player deposits in a given round (zatoshi). Doubles per round because the winner
    /// carries both stakes forward: round 1 = buyin, round 2 = 2×buyin, … Saturates rather than
    /// overflowing on absurd fields. Always 0 for free tournaments.
    pub fn stake_for_round(&self, round: u32) -> u64 {
        if !self.paid || round == 0 {
            return 0;
        }
        // round-1 stake = buyin; each later round the winner re-risks `roll_bps` of the pot
        // (pot = 2× the current stake) and banks the rest. roll_bps=10000 → ×2 (classic doubling),
        // 7500 → ×1.5, 5000 → flat stake. u128 math, saturating to u64.
        let roll = self.roll_bps.clamp(1, 10000) as u128;
        let mut stake = self.buyin_zat as u128;
        for _ in 1..round {
            stake = stake.saturating_mul(2).saturating_mul(roll) / 10000;
        }
        stake.min(u64::MAX as u128) as u64
    }

    /// One-line disclosure shown to players BEFORE they join a PAID tournament: for the P2P /
    /// no-custody design, each round is its own on-chain settlement, so a small network fee is
    /// deducted per round — players accept this up front (the org takes no rake / holds nothing).
    pub fn fee_disclosure(&self, per_round_fee_zat: u64) -> Option<String> {
        if !self.paid {
            return None;
        }
        // rounds is 0 until start(); estimate from current registrant count so the lobby can show it.
        let est_rounds = ceil_log2(self.players.len().max(2)) as u64;
        Some(format!(
            "Peer-to-peer & non-custodial: no house holds the pot. Each round is settled on-chain, \
             so a ~{} zat network fee applies per round (~{} rounds for the current field). The \
             winner takes the pot minus these network fees — no rake.",
            per_round_fee_zat, est_rounds
        ))
    }

    /// Register a player. Only while `Registering`; no duplicates.
    pub fn join(&mut self, p: PlayerId) -> Result<(), String> {
        if self.state != TournState::Registering {
            return Err("registration is closed".into());
        }
        if self.players.iter().any(|x| x == &p) {
            return Err("already registered".into());
        }
        self.players.push(p);
        Ok(())
    }

    /// Drop a registrant before the bracket is built.
    pub fn leave(&mut self, p: &str) -> Result<(), String> {
        if self.state != TournState::Registering {
            return Err("cannot leave after start".into());
        }
        let before = self.players.len();
        self.players.retain(|x| x != p);
        if self.players.len() == before { Err("not registered".into()) } else { Ok(()) }
    }

    /// Build the bracket and begin. Requires >= 2 players. Pads to the next power of two with
    /// byes; a player drawn against a bye auto-advances. `seed` maps registration order to slots
    /// (identity by default; callers may shuffle `players` first for random seeding).
    pub fn start(&mut self) -> Result<(), String> {
        if self.state != TournState::Registering {
            return Err("already started".into());
        }
        let n = self.players.len();
        if n < 2 {
            return Err("need at least 2 players".into());
        }
        // PAID tournaments must have a power-of-two field (2/4/8/16/…). Byes would let a player
        // advance a round without staking, leaving unequal stacks that break the doubling roll-over
        // (and hand a free pass in a money game). Free tournaments allow any N — byes are harmless
        // when no money rides on them.
        if self.paid {
            if self.buyin_zat == 0 {
                return Err("paid tournament needs a non-zero buy-in".into());
            }
            if !n.is_power_of_two() {
                return Err(format!(
                    "paid tournaments need a power-of-two field (2, 4, 8, 16…); have {} players",
                    n
                ));
            }
        }
        // rounds = ceil(log2(n)); size = 2^rounds
        let rounds = ceil_log2(n);
        let size = 1usize << rounds;

        // Byes go to the FRONT, one per pair, so no round-1 pairing is ever bye-vs-bye (which
        // would be a phantom match that never resolves and stalls the bracket). Since a next-pow2
        // bracket has byes < size/2, every bye fits in its own pair. The remaining players
        // (count = n - byes, always even) play real round-1 matches.
        let byes = size - n;
        let mut matches: Vec<Match> = Vec::new();
        let mut next_id = 0u32;
        let mut pi = 0usize;
        let r1_stake = self.stake_for_round(1);
        // round-1 byes: a real player advances automatically
        for _ in 0..byes {
            let a = self.players[pi].clone();
            pi += 1;
            matches.push(Match { id: next_id, round: 1, a: Some(a.clone()), b: None, winner: Some(a), stake_zat: r1_stake });
            next_id += 1;
        }
        // round-1 real matches
        while pi < n {
            let a = self.players[pi].clone();
            let b = self.players[pi + 1].clone();
            pi += 2;
            matches.push(Match { id: next_id, round: 1, a: Some(a), b: Some(b), winner: None, stake_zat: r1_stake });
            next_id += 1;
        }

        // Empty placeholder matches for rounds 2..=rounds; filled by propagation.
        let mut prev = matches.len();
        for r in 2..=rounds {
            let count = prev / 2;
            let stake = self.stake_for_round(r as u32);
            for _ in 0..count {
                matches.push(Match { id: next_id, round: r as u32, a: None, b: None, winner: None, stake_zat: stake });
                next_id += 1;
            }
            prev = count;
        }

        self.matches = matches;
        self.rounds = rounds as u32;
        self.state = TournState::Running;
        self.propagate(); // push any round-1 byes forward
        Ok(())
    }

    /// Report the winner of a match and advance the bracket.
    pub fn report_winner(&mut self, match_id: u32, winner: &str) -> Result<(), String> {
        if self.state != TournState::Running {
            return Err("tournament is not running".into());
        }
        let m = self
            .matches
            .iter_mut()
            .find(|m| m.id == match_id)
            .ok_or_else(|| "no such match".to_string())?;
        if m.winner.is_some() {
            return Err("match already decided".into());
        }
        // winner must actually be one of the two seated players
        let ok = m.a.as_deref() == Some(winner) || m.b.as_deref() == Some(winner);
        if !ok {
            return Err("winner is not a player in this match".into());
        }
        m.winner = Some(winner.to_string());
        self.propagate();
        Ok(())
    }

    /// Matches that are ready to play right now (both seats filled, undecided).
    pub fn pending_matches(&self) -> Vec<&Match> {
        self.matches.iter().filter(|m| m.is_playable()).collect()
    }

    /// The champion, once the final is decided.
    pub fn champion(&self) -> Option<&PlayerId> {
        if self.state != TournState::Finished {
            return None;
        }
        self.matches
            .iter()
            .find(|m| m.round == self.rounds)
            .and_then(|m| m.winner.as_ref())
    }

    /// Feed each decided match's winner into its parent slot; flip to `Finished` when the final
    /// resolves. Idempotent — safe to call after every result.
    fn propagate(&mut self) {
        // matches are laid out round-by-round; within a round, match index i feeds parent i/2,
        // taking seat a (i even) or b (i odd). Walk rounds in order so a chain of byes cascades.
        for r in 1..self.rounds {
            let round_matches: Vec<(usize, Option<PlayerId>)> = self
                .matches
                .iter()
                .enumerate()
                .filter(|(_, m)| m.round == r)
                .map(|(idx, m)| (idx, m.winner.clone()))
                .collect();
            // index within this round → parent slot in round r+1
            let parent_start = self
                .matches
                .iter()
                .position(|m| m.round == r + 1);
            let Some(parent_start) = parent_start else { continue };
            for (pos_in_round, (_, winner)) in round_matches.iter().enumerate() {
                let Some(w) = winner else { continue };
                let parent_idx = parent_start + pos_in_round / 2;
                if parent_idx >= self.matches.len() {
                    continue;
                }
                let parent = &mut self.matches[parent_idx];
                if pos_in_round % 2 == 0 {
                    if parent.a.is_none() {
                        parent.a = Some(w.clone());
                    }
                } else if parent.b.is_none() {
                    parent.b = Some(w.clone());
                }
                // a bye that meets an empty parent seat still needs a real opponent; only auto-win
                // if BOTH parent seats resolve to the same single feed (can't happen here) — so we
                // leave parent undecided until its real opponent arrives.
            }
        }
        // final decided → finished
        if let Some(f) = self.matches.iter().find(|m| m.round == self.rounds) {
            if f.winner.is_some() {
                self.state = TournState::Finished;
            }
        }
    }
}

/// In-memory registry of tournaments. **Permissionless**: anyone can `create`. **Non-custodial**:
/// it holds tournament STATE (brackets, results, sponsor branding), never funds.
#[derive(Default)]
pub struct Registry {
    tournaments: HashMap<String, Tournament>,
    seq: u64,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a tournament — open to any player. The registry assigns a monotonic id and records
    /// the caller as organizer. Returns the new id.
    pub fn create(
        &mut self,
        name: impl Into<String>,
        organizer: impl Into<String>,
        paid: bool,
        buyin_zat: u64,
        scheduled_start: Option<u64>,
        roll_bps: u16,
    ) -> String {
        self.seq += 1;
        let id = format!("t{}", self.seq);
        let mut t = Tournament::new(id.clone(), name, organizer, paid, buyin_zat);
        t.scheduled_start = scheduled_start;
        t.roll_bps = roll_bps.clamp(1, 10000); // guard 0/out-of-range → keep it a real fraction
        self.tournaments.insert(id.clone(), t);
        id
    }

    /// Relay ticker hook — auto-start / auto-cancel any tournament whose scheduled start time has
    /// passed. `now` is unix seconds. The schedule IS the authorization, so this bypasses the
    /// organizer gate: ≥2 registered players → start the bracket; otherwise (or if the bracket
    /// can't be built, e.g. a paid non-power-of-two field) → cancel. Returns (id, outcome) to log.
    pub fn tick(&mut self, now: u64) -> Vec<(String, &'static str)> {
        let mut out = Vec::new();
        for (id, t) in self.tournaments.iter_mut() {
            if t.state != TournState::Registering { continue; }
            match t.scheduled_start {
                Some(ts) if now >= ts => {}
                _ => continue,
            }
            if t.players.len() < 2 {
                t.state = TournState::Cancelled;
                out.push((id.clone(), "cancelled (too few players)"));
            } else if t.start().is_ok() {
                out.push((id.clone(), "started"));
            } else {
                t.state = TournState::Cancelled;
                out.push((id.clone(), "cancelled (bracket unbuildable)"));
            }
        }
        out
    }

    pub fn get(&self, id: &str) -> Option<&Tournament> {
        self.tournaments.get(id)
    }

    /// All tournaments (lobby list). Caller sorts/filters (e.g. Registering first).
    pub fn list(&self) -> Vec<&Tournament> {
        self.tournaments.values().collect()
    }

    /// Register `player` into tournament `id` (anyone may join while it's Registering).
    pub fn join(&mut self, id: &str, player: PlayerId) -> Result<(), String> {
        self.with(id, |t| t.join(player))
    }

    pub fn leave(&mut self, id: &str, player: &str) -> Result<(), String> {
        self.with(id, |t| t.leave(player))
    }

    /// Start — only the organizer may start their own tournament.
    pub fn start(&mut self, id: &str, who: &str) -> Result<(), String> {
        self.with(id, |t| {
            if t.organizer != who {
                return Err("only the organizer can start this tournament".into());
            }
            t.start()
        })
    }

    /// Add a sponsor — PERMISSIONLESS. Organizer's entry becomes gold; anyone else platinum
    /// (unfunded until the escrow lands). `who` is the authenticated caller.
    pub fn add_sponsor(&mut self, id: &str, who: &str, s: Sponsor) -> Result<(), String> {
        self.with(id, |t| { t.add_sponsor(who.to_string(), s); Ok(()) })
    }

    /// Remove a sponsor entry (by its `by` handle). Sponsor-self or organizer only.
    pub fn remove_sponsor(&mut self, id: &str, who: &str, target_by: &str) -> Result<(), String> {
        self.with(id, |t| t.remove_sponsor(who, target_by))
    }

    /// Escrow-watcher hook (Phase B): flip a platinum sponsor to funded + record its vault room.
    pub fn mark_sponsor_funded(&mut self, id: &str, target_by: &str, escrow_room: String) -> Result<(), String> {
        self.with(id, |t| t.mark_sponsor_funded(target_by, escrow_room))
    }

    /// Cancel a tournament — organizer only.
    pub fn cancel(&mut self, id: &str, who: &str) -> Result<(), String> {
        self.with(id, |t| {
            if t.organizer != who {
                return Err("only the organizer can cancel this tournament".into());
            }
            t.cancel()
        })
    }

    /// Report a match winner and advance the bracket.
    pub fn report_winner(&mut self, id: &str, match_id: u32, winner: &str) -> Result<(), String> {
        self.with(id, |t| t.report_winner(match_id, winner))
    }

    fn with<T>(&mut self, id: &str, f: impl FnOnce(&mut Tournament) -> Result<T, String>) -> Result<T, String> {
        let t = self
            .tournaments
            .get_mut(id)
            .ok_or_else(|| "no such tournament".to_string())?;
        f(t)
    }
}

/// ceil(log2(n)) for n >= 1. ceil_log2(1)=0, (2)=1, (3)=2, (4)=2, (5)=3, (8)=3, (9)=4.
fn ceil_log2(n: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    (usize::BITS - (n - 1).leading_zeros()) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("p{}", i)).collect()
    }

    #[test]
    fn ceil_log2_correct() {
        assert_eq!(ceil_log2(1), 0);
        assert_eq!(ceil_log2(2), 1);
        assert_eq!(ceil_log2(3), 2);
        assert_eq!(ceil_log2(4), 2);
        assert_eq!(ceil_log2(5), 3);
        assert_eq!(ceil_log2(8), 3);
        assert_eq!(ceil_log2(9), 4);
        assert_eq!(ceil_log2(16), 4);
    }

    #[test]
    fn join_dedup_and_state_gate() {
        let mut t = Tournament::new("T1", "Test", "org", false, 0);
        assert!(t.join("a".into()).is_ok());
        assert!(t.join("a".into()).is_err(), "dup rejected");
        assert!(t.join("b".into()).is_ok());
        t.start().unwrap();
        assert!(t.join("c".into()).is_err(), "no join after start");
    }

    #[test]
    fn start_needs_two() {
        let mut t = Tournament::new("T", "T", "org", false, 0);
        assert!(t.start().is_err());
        t.join("a".into()).unwrap();
        assert!(t.start().is_err());
        t.join("b".into()).unwrap();
        assert!(t.start().is_ok());
    }

    /// A clean power-of-two bracket plays to exactly one champion.
    #[test]
    fn power_of_two_runs_to_champion() {
        let mut t = Tournament::new("T", "T", "org", false, 0);
        for p in ids(8) {
            t.join(p).unwrap();
        }
        t.start().unwrap();
        assert_eq!(t.rounds, 3);
        // always beat the lower-numbered ("a") seat so the outcome is deterministic
        let mut guard = 0;
        while t.state == TournState::Running {
            let ready: Vec<(u32, String)> = t
                .pending_matches()
                .iter()
                .map(|m| (m.id, m.a.clone().unwrap()))
                .collect();
            assert!(!ready.is_empty(), "running tournament must always have a playable match");
            for (mid, a) in ready {
                t.report_winner(mid, &a).unwrap();
            }
            guard += 1;
            assert!(guard < 10, "must converge");
        }
        assert_eq!(t.state, TournState::Finished);
        assert!(t.champion().is_some());
    }

    /// Non-power-of-two: byes auto-advance and the bracket still crowns one champion, and every
    /// registered player is either eliminated or the champion (no one stranded).
    #[test]
    fn odd_counts_with_byes_converge() {
        for n in [3usize, 5, 6, 7, 9, 11, 13] {
            let mut t = Tournament::new("T", "T", "org", false, 0);
            for p in ids(n) {
                t.join(p).unwrap();
            }
            t.start().unwrap();
            let mut guard = 0;
            while t.state == TournState::Running {
                let ready: Vec<(u32, String)> = t
                    .pending_matches()
                    .iter()
                    .map(|m| (m.id, m.a.clone().unwrap()))
                    .collect();
                assert!(!ready.is_empty(), "n={}: stuck with no playable match", n);
                for (mid, a) in ready {
                    t.report_winner(mid, &a).unwrap();
                }
                guard += 1;
                assert!(guard < 20, "n={}: must converge", n);
            }
            assert_eq!(t.state, TournState::Finished, "n={}", n);
            assert!(t.champion().is_some(), "n={} must have a champion", n);
        }
    }

    #[test]
    fn report_winner_validates() {
        let mut t = Tournament::new("T", "T", "org", false, 0);
        t.join("a".into()).unwrap();
        t.join("b".into()).unwrap();
        t.start().unwrap();
        let m = t.pending_matches()[0].id;
        assert!(t.report_winner(m, "zzz").is_err(), "non-player winner rejected");
        assert!(t.report_winner(m, "a").is_ok());
        assert!(t.report_winner(m, "b").is_err(), "already decided");
        assert_eq!(t.champion().unwrap(), "a");
    }

    #[test]
    fn paid_requires_power_of_two_and_buyin() {
        // non-power-of-two field is rejected for paid
        let mut t = Tournament::new("T", "T", "org", true, 100_000);
        for p in ids(3) {
            t.join(p).unwrap();
        }
        assert!(t.start().is_err(), "3 players rejected for paid");

        // zero buy-in rejected
        let mut z = Tournament::new("T", "T", "org", true, 0);
        z.join("a".into()).unwrap();
        z.join("b".into()).unwrap();
        assert!(z.start().is_err(), "zero buy-in rejected");

        // clean power-of-two paid field starts and stamps doubling stakes per round
        let mut ok = Tournament::new("T", "T", "org", true, 100_000);
        for p in ids(4) {
            ok.join(p).unwrap();
        }
        ok.start().unwrap();
        assert_eq!(ok.rounds, 2);
        // round 1 = buyin, round 2 (final) = 2× buyin
        for m in &ok.matches {
            let want = if m.round == 1 { 100_000 } else { 200_000 };
            assert_eq!(m.stake_zat, want, "round {} stake", m.round);
        }
        assert_eq!(ok.stake_for_round(1), 100_000);
        assert_eq!(ok.stake_for_round(2), 200_000);
        assert_eq!(ok.stake_for_round(3), 400_000);
    }

    #[test]
    fn free_tournament_has_zero_stakes() {
        let mut t = Tournament::new("T", "T", "org", false, 0);
        for p in ids(4) {
            t.join(p).unwrap();
        }
        t.start().unwrap();
        assert!(t.matches.iter().all(|m| m.stake_zat == 0));
        assert_eq!(t.stake_for_round(1), 0);
    }

    #[test]
    fn registry_permissionless_create_organizer_gated_start() {
        let mut reg = Registry::new();
        // anyone can create a tournament
        let id = reg.create("Friday Night", "alice", false, 0, None, 10000);
        assert_eq!(reg.get(&id).unwrap().organizer, "alice");
        reg.join(&id, "alice".into()).unwrap();
        reg.join(&id, "bob".into()).unwrap();
        // a non-organizer cannot start it…
        assert!(reg.start(&id, "bob").is_err());
        // …the organizer can
        assert!(reg.start(&id, "alice").is_ok());
        assert_eq!(reg.get(&id).unwrap().state, TournState::Running);
    }

    #[test]
    fn sponsors_permissionless_tiers_and_prize() {
        let mut reg = Registry::new();
        let id = reg.create("Cup", "alice", false, 0, None, 10000);
        let mk = |prize: u64| Sponsor {
            name: "Zcash".into(), logo_url: "https://z.cash/logo.png".into(), url: "https://z.cash".into(),
            added_prize_zat: prize, by: String::new(), tier: SponsorTier::Platinum, funded: false, escrow_room: None,
        };
        // permissionless: a non-organizer CAN add — lands as platinum, unfunded, not counted.
        assert!(reg.add_sponsor(&id, "bob", mk(500)).is_ok());
        {
            let t = reg.get(&id).unwrap();
            assert_eq!(t.sponsors.len(), 1);
            assert_eq!(t.sponsors[0].tier, SponsorTier::Platinum);
            assert!(!t.sponsors[0].funded);
            assert_eq!(t.total_prize(), 0); // unfunded platinum doesn't count
        }
        // organizer's entry is gold and counts immediately.
        reg.add_sponsor(&id, "alice", mk(1000)).unwrap();
        assert_eq!(reg.get(&id).unwrap().total_prize(), 1000);
        // escrow watcher funds bob's platinum → now it counts.
        reg.mark_sponsor_funded(&id, "bob", "vault-1".into()).unwrap();
        assert_eq!(reg.get(&id).unwrap().total_prize(), 1500);
        // remove gating: a stranger can't; the sponsor or organizer can.
        assert!(reg.remove_sponsor(&id, "carol", "bob").is_err());
        assert!(reg.remove_sponsor(&id, "bob", "bob").is_ok());
        assert!(reg.remove_sponsor(&id, "alice", "alice").is_ok()); // organizer moderates own gold
        assert_eq!(reg.get(&id).unwrap().sponsors.len(), 0);
    }
}
