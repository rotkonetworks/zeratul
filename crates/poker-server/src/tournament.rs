//! Single-elimination tournament bracket — the NON-CUSTODIAL matchmaker core.
//!
//! The server only pairs players and advances winners; it NEVER holds funds. Free tournaments
//! are chip-only. For PAID tournaments each match is its own 2-player P2P FROST escrow and the
//! winner rolls both stakes into the next round, so the org is a scoreboard, not a custodian —
//! the property that keeps this out of money-transmitter / custody territory.
//!
//! This module is pure logic (no I/O, no money) so it's exhaustively testable in isolation.

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
}

/// A tournament sponsor — branding shown to every participant (logo + click-through URL). The
/// sponsor NEVER holds player funds; sponsorship is advertising, not custody, so it adds no
/// regulatory surface. A sponsor MAY pledge an `added_prize` they pay the champion DIRECTLY
/// (peer-to-peer), announced up front — the org still touches nothing.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Sponsor {
    pub name: String,
    pub logo_url: String,
    pub url: String,
    /// optional sponsor-pledged prize (zatoshi), paid sponsor→champion directly. 0 = branding only.
    pub added_prize_zat: u64,
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
    /// true = paid (per-match P2P FROST escrow, roll-over); false = free chip-only.
    pub paid: bool,
    /// buy-in in zatoshi for paid tournaments (the round-1 stake; unused when `!paid`).
    pub buyin_zat: u64,
    /// optional sponsor branding (logo + URL shown to all participants).
    pub sponsor: Option<Sponsor>,
    pub state: TournState,
    pub players: Vec<PlayerId>,
    pub matches: Vec<Match>,
    /// total rounds = ceil(log2(N)). 0 until started.
    pub rounds: u32,
}

impl Tournament {
    pub fn new(id: impl Into<String>, name: impl Into<String>, paid: bool, buyin_zat: u64) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            paid,
            buyin_zat,
            sponsor: None,
            state: TournState::Registering,
            players: Vec::new(),
            matches: Vec::new(),
            rounds: 0,
        }
    }

    /// Attach (or replace) sponsor branding. Allowed any time — a sponsor can back an already-open
    /// tournament. Non-custodial: sponsorship is advertising; any pledged prize is paid by the
    /// sponsor directly to the champion, never held by the org.
    pub fn set_sponsor(&mut self, s: Sponsor) {
        self.sponsor = Some(s);
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
        // round-1 byes: a real player advances automatically
        for _ in 0..byes {
            let a = self.players[pi].clone();
            pi += 1;
            matches.push(Match { id: next_id, round: 1, a: Some(a.clone()), b: None, winner: Some(a) });
            next_id += 1;
        }
        // round-1 real matches
        while pi < n {
            let a = self.players[pi].clone();
            let b = self.players[pi + 1].clone();
            pi += 2;
            matches.push(Match { id: next_id, round: 1, a: Some(a), b: Some(b), winner: None });
            next_id += 1;
        }

        // Empty placeholder matches for rounds 2..=rounds; filled by propagation.
        let mut prev = matches.len();
        for r in 2..=rounds {
            let count = prev / 2;
            for _ in 0..count {
                matches.push(Match { id: next_id, round: r as u32, a: None, b: None, winner: None });
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
        let mut t = Tournament::new("T1", "Test", false, 0);
        assert!(t.join("a".into()).is_ok());
        assert!(t.join("a".into()).is_err(), "dup rejected");
        assert!(t.join("b".into()).is_ok());
        t.start().unwrap();
        assert!(t.join("c".into()).is_err(), "no join after start");
    }

    #[test]
    fn start_needs_two() {
        let mut t = Tournament::new("T", "T", false, 0);
        assert!(t.start().is_err());
        t.join("a".into()).unwrap();
        assert!(t.start().is_err());
        t.join("b".into()).unwrap();
        assert!(t.start().is_ok());
    }

    /// A clean power-of-two bracket plays to exactly one champion.
    #[test]
    fn power_of_two_runs_to_champion() {
        let mut t = Tournament::new("T", "T", false, 0);
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
            let mut t = Tournament::new("T", "T", false, 0);
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
        let mut t = Tournament::new("T", "T", false, 0);
        t.join("a".into()).unwrap();
        t.join("b".into()).unwrap();
        t.start().unwrap();
        let m = t.pending_matches()[0].id;
        assert!(t.report_winner(m, "zzz").is_err(), "non-player winner rejected");
        assert!(t.report_winner(m, "a").is_ok());
        assert!(t.report_winner(m, "b").is_err(), "already decided");
        assert_eq!(t.champion().unwrap(), "a");
    }
}
