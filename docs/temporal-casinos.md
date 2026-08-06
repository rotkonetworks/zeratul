# Temporal Casinos

### Or: how do you get the boys together for a game, when the boys are strangers on the internet?

---

There's a specific feeling you get playing cards at a kitchen table. It's late, someone's dog is under the table, the chips are mismatched because half are from a set someone lost the box to. Nobody's getting rich. The stakes are real enough to sting and small enough to laugh about. You can read the guy across from you because you've known him for ten years. When it's over, it's *over* — the cards go back in the drawer and the night becomes a story.

None of that is about poker. Poker is just the excuse. The thing that's actually happening is **trust, presence, and ephemerality**, braided together for a few hours and then gone.

Online poker took that feeling and threw almost all of it away.

This is a piece about why, and about what it would take to get it back — not by making a nicer casino, but by building something with the opposite values, using cryptography to do the one thing the kitchen table can't: let strangers trust each other the way old friends do.

---

## What digital poker optimized away

Mainstream online poker is a marvel of optimization, aimed squarely at the wrong target. It was built for **scale and extraction**, and every design decision followed from that:

- **A permanent house.** A company runs the tables, holds the money, and takes a rake off every pot, forever. You are not playing *with* people; you are playing *inside* an institution that profits from your play and could, in principle, deal itself the nuts.
- **Infinite, anonymous tables.** You sit down with usernames, not people. There are no faces, no voices, no history. The person across from you might be a friend, a shark, or a bot farm, and you will never know.
- **The grind.** The product wants you playing forever. There is no "and then the night was over." There is only the next table.

Each of these is the precise inverse of a home game. The home game is **small** (you know everyone), **house-less** (nobody skims, nobody can cheat you because there's no *nobody* — it's just the players), and **ephemeral** (it starts, it ends, it's a memory). Online poker is large, custodial, and endless.

So the problem was never "make online poker better." A better casino is still a casino. The problem is: **can you rebuild the home game for the digital world — small, trusted, house-less, ephemeral — and still let people who've never met sit down together?**

That last clause is the hard one, and it's the whole reason cryptography enters the story.

---

## Temporal casinos

Here is the shape of the answer we keep circling back to. Call it a **temporal casino**.

A temporal casino is a table that **pops into existence on demand, hosts a game, settles up, and dissolves.** There is no persistent operator, no standing pot of everyone's money, no company in the middle. Anyone can spin one up — for their friends, or open to the world — the way you'd text the group chat "cards at mine, 8pm." When the night's over, the casino evaporates. What's left is a settlement and a memory, which is exactly what's left after a real home game.

The word *casino* is almost a joke, because the defining feature is that **there is no house.** The building assembles itself out of the people who showed up, exists for the length of a session, and comes apart. It's a casino the way a pickup basketball game is a franchise: it isn't. It's just people, playing, tonight.

Everything technical below is in service of that one image. The question is only: **what has to be true, cryptographically, for a house-less pop-up game to be fair — even when the players are strangers?**

---

## The trust problem, stated honestly

At a kitchen table, fairness is solved socially. Nobody deals from the bottom because everybody's watching and everybody has to see these people again. Trust is *ambient* — it comes free with the fact that you're all in the same room and you all know each other.

Strip that away — put the players in different cities, behind screens, meeting for the first time — and you have to *manufacture* the fairness that the room used to provide for free. That's the job. And it turns out the answer is different for two players versus more than two, so we'll take them in turn.

### Two players: the pure case

Heads-up — one on one — has a beautiful property: **it can be made perfectly trustless.** Two players can shuffle and deal a deck between themselves such that neither can cheat and neither needs a third party. The technique is called *mental poker*, and at its heart is a lovely idea: both players encrypt the deck, one after the other, with keys only they hold, so the final order is a secret that requires *both* of them to unlock. Neither can peek. Neither can stack it. There is no dealer, because the two players *are* the dealer, jointly, and neither trusts the other an inch.

For heads-up, we keep this purity. No committee, no house, no chain — just two people and some math, exactly as trustless as it's possible to be. This is the crown jewel, and we don't compromise it.

### More than two players: the wall

Here's where honesty is required, because it's where a lot of ambitious projects quietly break.

**There is no practical, fully-trustless way for N strangers to deal cards among themselves.** Mental poker generalizes to more than two players *on paper*, but it dies in practice for one brutal reason: to keep the deck secret from any coalition, the unmasking has to require **everyone**. And the moment it requires everyone, a **single player closing their laptop mid-hand freezes the entire table.** You've traded a cheating problem for a liveness problem, and the liveness problem is worse, because it happens by accident, constantly, forever.

You can loosen the requirement — let, say, four of six players unmask the deck instead of all six — and now the table survives a disconnect. But now **any four players who quietly compare notes can see everyone's cards.** In a game whose entire point is hidden information, played for money, by adversaries: that's not a corner case, that's the end of the game.

This is not an engineering gap you can grind your way through. It's a theorem. **Among adversarial players alone, you get either fragility or cheating. Pick one.** No blockchain changes this, because — and this is the subtle part — **consensus solves ordering and liveness, not secrecy.** A shared, ordered log doesn't stop four players from combining their keys in a Signal group. The privacy of the cards is a property of *who holds the shuffle secret*, and it is completely independent of whatever chain you run underneath.

So if we want multiplayer, and we want it fair, we need to introduce *some* structure beyond the players. The whole art is in making that structure as small, blind, and powerless as possible — so it never becomes the house we're trying to abolish.

---

## Frostito: the smallest possible dealer

The structure we introduce is a **committee** — we call ours a *frostito* group, after the threshold-signature scheme (FROST) at its core. But before that word conjures images of a new middleman, look at how little it's allowed to do.

The naive version of a committee-dealer is dangerous: give a group of servers the key to the deck, and a majority of them can see everyone's cards. That's just a distributed house, and a distributed house that can peek is arguably worse than an honest single one.

We don't build that. We build something with two layers, and the two layers make all the difference:

- **Community cards** (the shared board) are decrypted by the committee for everyone to see. They're public anyway, so no secret is spent.
- **Hole cards** — your private two — are encrypted so that unmasking them requires the committee's help **and your own key.** The committee peels off the shuffle layer and is left holding a card that is *still locked to you.* 

The consequence is precise and worth saying slowly: **even a fully corrupt committee — every last member colluding — cannot see your hole cards.** They can learn the *order the deck was shuffled in* (that's the threshold-protected secret), but never the cards themselves, because those are sealed to their owners. Card privacy against the committee is not a matter of trust or threshold; it's *unconditional*.

What, then, does the committee actually control? Only **shuffle secrecy** — the ordering of the deck before it's dealt. And here's the trick that keeps them honest: set the threshold so that **the players, even all of them colluding, are one share short.** They need at least one committee member to reveal the shuffle. So the committee's entire power reduces to: *it can refuse to help deal* (a liveness fault, and you keep two or three of them so one going down doesn't matter), and *nothing else.* It cannot see your cards. It cannot touch your money — that lives somewhere else entirely (more on that in a moment). It cannot alter the deck, because the shuffle carries a mathematical proof that it's a genuine permutation with nothing added, removed, or peeked.

That's the whole "house": **two or three blind, broke, card-blind shares whose only job is to make it impossible for the table to collude against itself.** It can't rig the game, can't rob you, can't read your hand. It's less a dealer than a *witness* — a piece of math sitting at the table that can't play and can't steal, present only so that "everyone colludes" stops being an attack.

And for **free games — play money, friends only —** you delete even that. With no money on the line, collusion is a social problem, not a financial one, and the players can run the whole thing themselves, purely peer-to-peer, exactly the self-contained temporal casino of the original dream. The blind witnesses only show up when real value does.

---

## The mistake we almost made: coupling money to the cards

Somewhere in designing this, there's a trap that everyone falls into, and it's worth flagging because it *feels* like a dealbreaker until you see through it.

You start thinking: money moves every hand — someone wins, someone loses — so surely the cryptographic shares have to be re-split every hand to track who owns what? And you imagine constantly re-running key ceremonies keyed to the standings, and it collapses into nonsense.

It collapses because it's a category error. **The card key and the money are two different objects that must never touch.**

- The **card key** is about *secrecy of a deck.* It's generated once, reused across hands with a fresh shuffle each time, and it knows nothing about who's up or down. You reshare it only if the *committee* changes — which is rare and has nothing to do with the game.
- The **money** is just a **ledger of balances** — plain numbers. Winning a hand changes a number. It touches no cryptographic secret at all.

Once you see they're orthogonal, the design gets *simpler*, not harder. You don't move real money every hand — that would be death by transaction fees, and it would feel nothing like chips. You do what a real cardroom does: **buy in once**, into a single shared escrow; **play a hundred hands** as pure ledger deltas (numbers going up and down, no blockchain touched); **cash out once**, settling your net. A whole night is one deposit and one withdrawal, with the game itself living as weightless numbers in between. The chips are just chips. Nobody reshares anything when you win a pot. You were never supposed to.

---

## Reputation without a rap sheet

If anyone can host a temporal casino, and the frostito committees are pseudonymous, how do you know which ones to trust? At the kitchen table you trust *the guy who runs the Tuesday game* — not because you've seen his ID, but because he's run a hundred fair games and everyone knows it.

That's the model, digitized. A committee is a **stable pseudonym with a track record** — hands dealt, disputes at zero, uptime honored — bound to a key, not a person. Choosing a reputable committee is choosing the Tuesday-game guy. And crucially: **the protocol gates on the score, never on identity.** No KYC, no real names, no way to be excluded for who or where you are. Permissionless to host, permissionless to play.

There's an honest tension here worth naming rather than papering over: **strong, recognizable reputation and perfect anonymity pull against each other.** A track record *is* a thread of linkage — that's what makes it a track record. So the achievable, honest target isn't "reputable yet utterly unlinkable," which is close to a contradiction. It's **pseudonymous-persistent**: a poker screen-name with a verifiable history, unlinkable to your offline life but recognizable across games. Which is, again, exactly the home-game deal. You trust the Tuesday-game guy's *reputation*. You never needed his passport.

---

## The chain's actual job

So where does the "blockchain" everyone reaches for actually fit? Not, it turns out, as the dealer — we saw that a transparent ledger can't keep cards secret. Its job is humbler and real:

- a **registry** of committees and their reputations,
- **matchmaking** between players and hosts,
- **custody** of the buy-ins in escrow while a session runs,
- and **settlement** — turning the final ledger into real payouts, with a timeout that refunds everyone if a table simply dies.

The game itself — the ordering of bets, the flow of the hand — can run on a lightweight, **ephemeral consensus among the participants**, spun up for the session and dissolved after, precisely the *temporal* chain of the original vision. Just don't ask it to keep the cards secret; that's the committee's job, and the two are separate on purpose.

The picture that falls out is three clean layers:

| Layer | Who runs it | What it guarantees |
|---|---|---|
| **Ordering / game log** | the players' ephemeral chain | everyone agrees on what happened; survives a disconnect |
| **Card secrecy** | a blind 2-of-3 witness (or nobody, for free games) | no coalition peeks; your hole cards are yours, unconditionally |
| **Custody & settlement** | the durable anchor | the money is safe, and always refundable |

Money is never at the mercy of a browser tab. Cards are never at the mercy of a colluding table. And nobody, anywhere, is the house.

---

## The part the cryptography can't do

Here's the thing I most want to be honest about, because it's easy to fall in love with the protocol and forget it.

**The cryptography is the enabler. It is not the magic.**

The threshold dealing, the shuffle proofs, the ephemeral consensus — all of that exists to solve exactly one problem: letting strangers trust the game the way friends trust the room. It is necessary, it is beautiful, and it is *not the thing you feel.*

What you *feel* — the reason it lands as "the boys" and not "a slightly nicer casino" — lives almost entirely in a different layer:

- **Faces and voices.** You should see and hear the person across from you. Poker is a game of reading people; a game of usernames is a different, colder game. The banter, the tells, the groan when the river bricks — that's the night.
- **People you chose.** You're playing your friends, or friends of friends, pulled in by a shared link, not dropped among anonymous grinders. The social graph *is* the product.
- **It ends.** The table dissolves. There's no infinite grind pulling at you. It started, it happened, it's a story now. Ephemerality isn't a limitation to engineer around — it's the *point*.

Every one of these is more product than protocol. And every one of these is a dimension mainstream poker deliberately removed in the name of scale. So the deepest design instruction in this whole document is: **don't let the hard, gorgeous crypto eat all the oxygen.** The committee math is what makes it *possible*. The room is what makes it *matter*. Build both, but never confuse the one for the other.

---

## What's solved, and what's actually hard

To close the loop honestly, because a vision that hides its hard parts is just marketing:

**Solved, and we should stop worrying about it:** the dealing cryptography. Threshold encryption, verifiable shuffles, proofs of correct reveal — this is decades-old, well-analyzed mathematics. If we find ourselves inventing novel cryptography to deal a card, we've taken a wrong turn. Use the known primitives.

**The real work, and where the risk lives:** everything *around* the crypto.
- **The economics of the committee** — what bond makes colluding irrational, given that collusion is undetectable and so can't be punished after the fact. This is the actual security, and it's cryptoeconomic, not cryptographic.
- **Availability** — a committee of browser tabs going offline gracefully, backup members, clean re-keying.
- **The felt experience** — the presence, the friction of getting people to a table, the thousand small things that make a room feel like a room.
- **Metadata** — even with cards hidden, a transparent settlement layer leaks *who played whom, for how much.* If we want that private too — and for a privacy-first stack, we might — shielded settlement is a whole additional layer to decide on deliberately, now, not later.

And the honest sequencing: none of the grand committee machinery is where you start. You start with **free-play multiplayer** — players-only, no money, a simple fair deal — because it lets you build the thing that actually makes it feel human (the table, the faces, the friends, the fold-and-continue when someone drops) *without* the money-grade cryptography in the way. Get the room right on money-free ground. Then, and only then, slide the blind witnesses underneath and turn on real stakes.

---

## The aspiration

Most attempts to fix online poker try to make it *better* — smoother, faster, a marginally fairer casino. They end up with a casino.

This is trying to make it *warmer.* A home game that anyone can host and no one owns. A table that appears because you and your friends wanted to play tonight, deals itself fairly whether you've known each other for years or five minutes, takes nothing off the top, and disappears when the night is done — leaving behind exactly what a real game leaves behind: a settled score and a story.

It is genuinely hard to move that feeling into the digital world. Most of what makes it special was never in the cards. But you don't recover it by out-engineering the casinos at their own game. You recover it by refusing to build a casino at all — and building the kitchen table instead, with just enough mathematics underneath that strangers can sit down at it and trust each other like old friends.

Temporary. House-less. Yours for the night.

That's the game.
