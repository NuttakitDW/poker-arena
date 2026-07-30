# Curated hand transcripts

This directory holds hand-picked, verified-interesting hands for each of the
twelve registered game variants — real output from `poker-arena run --log`,
not synthetic examples. Each `transcripts/<game-id>.jsonl` contains 3–5
complete hands, extracted **verbatim** (same bytes, same line order, no
renumbering or reformatting) from a full match log produced with a fixed
seed, so anyone can reproduce the source match and find the same hands at
the same hand numbers.

## Line shape

Every line is one JSON object:

```json
{"hand": 175, "ev": {"event": "...", ...}}
```

`hand` is the 0-based hand counter for the match (matches `Event::HandStart`
/ `Event::HandEnd`'s position in the stream — the wire protocol's per-bot
`hand_no` is 1-based, but this is the arena-side log, which counts from 0).
`ev` is one `poker_core::game::Event`, serialized exactly as
[`WIRE_PROTOCOL.md`](../WIRE_PROTOCOL.md) documents under
[Events (`WireEvent`)](../WIRE_PROTOCOL.md#events-wireevent) — the log and
the wire protocol share the same event enum byte-for-byte, so that section
is the authoritative field reference for every event type below
(`hand-start`, `post`, `deal-hole`, `street-start`, `deal-community` /
`deal-up`, `acted`, `draw-result`, `showdown-show`, `pot-awarded`,
`hand-end`).

A hand in one of these files is always the complete, contiguous run from
its `hand-start` line to its matching `hand-end` line — nothing is
truncated mid-hand.

## How to read a hand

1. Group lines by `hand` (they're already grouped and in ascending order
   within each file).
2. `hand-start` gives the button seat and starting stacks; `post` events
   give blinds/antes/bring-in.
3. `deal-hole` / `deal-up` / `deal-community` / `draw-result` reveal cards
   as the real hand would see them (a card dealt to seat *N* is only
   visible on other seats' lines when it's face-up; a spectator/log reader
   sees everything since `poker-arena` logs are unredacted).
4. `acted` is one action per decision point (`action.kind` is
   `fold`/`check`/`call`/`bet`/`raise`/`bring-in`/`discard`; `bet`/`raise`
   carry a **total** street commitment in `to`, not an increment — see
   [Action semantics](../WIRE_PROTOCOL.md#action-semantics)).
5. `showdown-show` reveals each remaining seat's hand and its evaluator
   value(s) (`hi`/`lo`, `HandValue(u32)`; the top nibble, `(v >> 20) & 0xF`,
   is the hand class for the `High` evaluator — `0`=high card ... `8` =
   straight flush — used below to call out "premium" showdowns for the
   games whose hi side actually uses the High evaluator: holdem, omaha,
   stud/stud8, 5cd. Badugi's value instead encodes how many of the four
   cards form a badugi in that same nibble, `4`=four-card badugi down to
   `1`; razz/2-7/A-5's low-hand values aren't nibble-classed at all, so
   those games' descriptions below spell out the actual low by hand).
6. `pot-awarded` shows exactly who won what, `side` distinguishing a
   `whole`-pot scoop from a `hi`/`lo` split; `pot` is a **pot index** (0 =
   main pot, 1+ = a side pot), not a chip amount. Side pots never occur in
   these matches: `poker-arena run` resets every seat to the same stack
   each hand, and with equal stacks any all-in call converges on exactly
   the same total commitment as the shove itself, so there's no way for a
   shorter stack to arise mid-hand — see the note under "How to
   regenerate" below.
7. `hand-end.nets` is winnings-minus-contributions per seat for that hand
   and always sums to zero.

## How to regenerate

Build the CLI once:

```sh
cargo build --release -p poker-arena-cli
```

Each game section below gives the exact command used to produce that
game's source match (same seed, same bots, same seat count — reproducible
forever per the fairness model in the top-level README, given the same
binary). Add `--log somewhere.log` to capture the full match, then find a
specific curated hand with e.g. `grep '"hand":175,' somewhere.log` (or
`jq 'select(.hand==175)'` if lines aren't pre-filtered).

The bots (`builtin:random[:seed]`, `builtin:shover`, `builtin:caller`) are
deterministic and hand-strength-oblivious — they don't evaluate their cards
at all, so a given seed produces the exact same sequence of actions
regardless of which registered game variant you point it at, *as long as
the two games deal the same number of hole cards and share the same
betting structure* (so their `legal` action shapes line up decision for
decision). Only the showdown evaluator (and therefore who wins) can differ
between such sibling variants dealt from the same seed. Several hand
descriptions below lean on this to compare the *same* deal across sibling
games:

- `holdem-nl` / `holdem-fl` deal the same two hole cards per seat for a
  given hand number (see hands 0 and 28 in both files).
- `omaha-pl` / `omaha8-pl` / `omaha8-fl` deal the same four hole cards per
  seat for a given hand number (see hand 0, present in all three files).
- `stud-fl` / `stud8-fl` share the identical fixed-limit betting structure,
  so their action sequences match move-for-move, not just the deal (see
  hand 22, present in both files).
- `27td-fl` / `a5td-fl` deal and play out identically for a given hand
  number (both are fixed-limit triple draw with a 5-card hand); only the
  low evaluator (2-7 vs. A-5) differs, which occasionally flips who wins —
  see hand 78 below, the flagship example.

All twelve source matches here used `--hands 400 --seed 7 --dealing
seeded`, mixing `builtin:random` (creates raises and folds), `builtin:
shover` (creates all-ins), and `builtin:caller` (creates showdowns) so each
match samples a wide range of textures. Every claim below (winner, hand
class, pot size, discard counts, capped streets, low strings) was checked
against the actual event lines and, for the low-hand games, hand-computed
from the raw cards — not guessed from the bot mix or reused from a
previous run.

**A note on regeneration and reproducibility.** The commands below are
exact, but a match built from a *different* binary build can produce
different hands from the same `--seed`: this project's dealing/seating
internals are not part of its public API contract, only the CLI's
documented flags are. The transcripts in this directory were captured
against one specific release build; if you rebuild and rerun a command and
get different cards, that's expected — rerun the analysis pipeline
(described below) against your own log rather than assuming hand numbers
carry over.

**A note on side pots.** With `poker-arena run`, every seat starts every
hand at the same stack (`--stack-bb`, reset per hand), and forced bets
(blinds/antes/bring-in) are the same small fraction of a stack for
everyone. Under those conditions no side pot can ever form: whenever an
active player is fully matching bets street after street, their remaining
stack tracks every other fully-matched player's remaining stack exactly,
so the moment anyone goes all-in, every other player who can still call
does so for the *same* total commitment (their whole equal stack) rather
than a lesser amount. Side pots need players to already be at different
effective stack depths when the all-in happens, which structurally can't
happen here. We looked for one across all twelve 400-hand source matches
(4,800 hands) and confirmed zero `pot-awarded` events with a nonzero pot
index anywhere — so no curated hand below claims a side pot.

---

## holdem-nl — No-Limit Texas Hold'em

```sh
./target/release/poker-arena run --game holdem-nl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --bot builtin:random:3 \
  --log holdem-nl.log
```

- **hand 0** — five-way all-in preflop (seat 0 raises to 9563, seat 1
  shoves to 10000, everyone calls); board runs `As 5d 3s / 5c / Jc`; seat
  2's `Ad 6d` makes two pair (aces and fives) — the best of all five hands
  shown — and scoops the 50000 pot.
- **hand 3** — seat 4 shoves preflop and gets called down three-way; board
  `7s 2d 9h / 2c / 9s` pairs twice; seat 2's `2h Ks` rivers a full house
  (deuces full of nines) to beat seat 3's and seat 4's matching board two
  pair; wins 30050.
- **hand 28** — three-way all-in preflop (seat 0 shoves, seat 1 and seat 4
  call); board `Jd 3s Qc / 2d / Qd` pairs queens; seat 4's `5d Ad` makes an
  ace-high diamond flush (using both hole diamonds plus three on the
  board) to beat seat 1's and seat 0's matching board-based two pair;
  scoops 30100. **Compare with holdem-fl's hand 28 below** — same deal,
  much smaller pot, because fixed-limit lets a player escape.
- **hand 141** — a four-bet preflop war (2340 → 4723 → 7505 → 10000) puts
  four of the five seats all-in; the board pairs twice more (`8c`/`8d` and
  `9s`) on a `Kc 8c 2s / 9s / 6s` runout, so it comes down to who also
  paired a hole card: seat 4's `4s 3s` rivers a flush (five spades: `9s 6s
  4s 3s 2s`) to beat three rivals' pairs and a bare high card; scoops
  42340.

## holdem-fl — Fixed-Limit Texas Hold'em

```sh
./target/release/poker-arena run --game holdem-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --bot builtin:random:3 \
  --log holdem-fl.log
```

- **hand 0** — the fixed-limit sibling of holdem-nl's hand 0: identical
  hole cards and board (`As 5d 3s / 5c / Jc`); turn caps at four raises
  four-way, but seat 1 folds the turn to survive; seat 2's `Ad 6d` two pair
  (aces and fives) still wins, but for a modest 5700 instead of an
  all-in-preflop 50000 — fixed-limit lets seat 1 escape a hand that no-limit
  couldn't.
- **hand 4** — a clean, uncapped river cooler: after two folds preflop,
  seat 0's `4c Tc` completes the wheel (A-2-3-4-5, using the flop's `As 5h`
  and the turn's `2c`) to beat seat 1's rivered two pair (aces and treys);
  wins 2600.
- **hand 28** — the fixed-limit sibling of holdem-nl's hand 28: same board
  and hole cards, but this time the turn caps at four raises three-way and
  seat 1 folds the river before seeing the last card; seat 4's `5d Ad`
  ace-high diamond flush still beats seat 0's two pair, but the pot is only
  5900 — a fifth of holdem-nl's 30100 all-in for the identical cooler.
- **hand 31** — a slow-played monster: seat 2's pocket eights (`8c 8d`)
  flop trip eights (`Kc 8h Th`) for a flopped set that becomes quad eights
  when the board pairs itself again with nothing more than routine calls
  and one river raise; seat 2's quads beat seat 1's and seat 4's two pair
  and seat 0's one pair; wins 3600.

## omaha-pl — Pot-Limit Omaha

```sh
./target/release/poker-arena run --game omaha-pl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log omaha-pl.log
```

- **hand 0** — three-way preflop all-in resolving into a triple cooler:
  board `7h Ac 4s / 7c / Kc` pairs sevens then rivers a king; seat 2's
  `Kh...Ks` makes kings full of sevens, beating seat 1's sevens full of
  aces *and* seat 0's ace-high club flush (`Jc 5c` plus three board clubs)
  — all three are legitimate premium hands, and the biggest boat wins;
  scoops 33447.
- **hand 2** — heads-up turn all-in; board `3d 5h Qd / 4c / Ad` lets both
  players make a straight: seat 0's `6c 7h` completes 3-to-7 (using the
  flop's `3d 5h` and turn's `4c`), while seat 2's `2c 3s` makes the wheel
  (A-2-3-4-5, using the same flop cards plus the river's `Ad`) — the 7-high
  straight beats the 5-high wheel; wins 24987.
- **hand 3** — four-way all-in preflop; board `Td 5d 6c / Jc / 6h` pairs
  sixes, and *all four* players make two pair off that shared pair: seat
  2's jacks-and-fives (`Ac 5c`) edges seat 1's tens-and-sixes, seat 0's
  nines-and-sixes, and seat 3's sevens-and-sixes; scoops the entire 40000
  pot.
- **hand 4** — three-way turn all-in; board `Kd 4s Kc / Td / 8d` pairs
  kings, and it comes down to who else paired: seat 0's `5h...8h` makes
  kings-and-eights to beat seat 2's kings-and-fours and seat 1's bare pair
  of kings (no second pair at all); wins 30000.
- **hand 324** — heads-up, no all-in, just pot-limit pressure (600 → 1800
  → 5400) called down each street; board `Jc 2c 3s / Td / Th` gives three
  tens between the board and seat 1's own hole `Tc`; seat 1's `3h 9h`
  rivers tens full of treys to beat seat 2's two pair (kings and twos);
  wins 16200.

## omaha8-pl — Pot-Limit Omaha Hi-Lo (8 or Better)

```sh
./target/release/poker-arena run --game omaha8-pl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log omaha8-pl.log
```

- **hand 0** — the hi-lo sibling of omaha-pl's hand 0 (same deal, same
  triple cooler): seat 2's kings-full boat takes the hi half (16724) while
  seat 0's flush hand *also* holds the best qualifying low (A-3-4-5-7,
  using `3s 5c` plus the board's `Ac 4s 7c`) and takes the lo half (16723);
  seat 1's bigger boat (sevens full of aces) wins nothing.
- **hand 1** — a four-bet preflop war ends in a 2-way all-in; board `Kh Jd
  2h / 2d / Qh` has only one card 8-or-under (a paired deuce), so no low
  ever qualifies; seat 3's `Qs Js Jh Jc` rivers a full house (jacks full of
  twos, using two hole jacks plus the board's third jack and paired twos)
  to scoop the entire 25914 pot, no lo split despite the hi-lo format.
- **hand 15** — a preflop all-in between two players who both hold `A-2-3`
  plus a kicker; the board (`4s 3s 5c` / `8c` / `Js`) lets both make the
  *exact same* wheel (A-2-3-4-5) — simultaneously a straight and the nut
  low. Hi splits evenly (6852/6852) and lo splits evenly too (6852/6852):
  a wheel scooping its own quarter on both sides.
- **hand 78** — three-way all-in preflop (one player folds to the shove);
  board `6d Ts 2d / 8h / Qs`; seat 0's `8c 6c 9d As` turns two pair (eights
  and sixes) to take the entire hi half (16506) alone, while the other two
  live players — both holding `A-...-3` low draws — tie for the low and
  split that half (8253 each): the hi winner and the lo winners are three
  completely different hands.
- **hand 201** — flop all-in three-way (one player folds); seat 3's `6d 5h
  7c 6c` rivers trip sixes off `Qs 7s 8d / Jh / 6s` to take the entire hi
  half (15640) alone, while the other two live players tie for the low and
  split it (7820 each) — again nobody double-dips between hi and lo; total
  pot 31280.

## omaha8-fl — Fixed-Limit Omaha Hi-Lo (8 or Better)

```sh
./target/release/poker-arena run --game omaha8-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log omaha8-fl.log
```

- **hand 0** — the fixed-limit sibling of omaha8-pl's hand 0 and
  omaha-pl's hand 0 (same deck): turn caps at four raises; seat 2's
  kings-full boat takes the hi side (2700), seat 0's A-3-4-5-7 low takes
  the lo side (2700).
- **hand 8** — flop caps at four raises three-way; seat 0's `Kd 8d` rivers
  kings-and-eights (using a paired `Kc`-`Th` flop) to take the hi side
  (1750) while seat 2's `Ah...6s` takes the lo side (1750) with an 8-low —
  the simplest hi-lo split in the set, no premium hand, no scoop, just a
  clean division.
- **hand 15** — the fixed-limit sibling of omaha8-pl's hand 15: the same
  double-wheel chop (both players make A-2-3-4-5, a straight that's also
  the nut low), flop capped at four raises; hi splits 850/850 and lo
  splits 850/850.
- **hand 96** — board pairs fours (`4h 4d 6c`); seat 0's `Kh Qc` rivers
  trip fours to scoop the entire hi half (1600) while the other two live
  players tie and split the low (800 each) — a three-winner hand with no
  overlap between the hi winner and the lo winners.
- **hand 163** — flop war ends with seat 0 folding; seat 1's `Kh Kd`
  rivers a full house (kings full of sixes, off a `Kc 6c Ad / 2c / 6d`
  board) to scoop the entire hi half (2000) alone while seat 2 and seat 3
  tie and split the low (1000 each).

## stud-fl — Seven-Card Stud

```sh
./target/release/poker-arena run --game stud-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log stud-fl.log
```

- **hand 1** — third street caps at four raises (seat 0's jack door
  completes straight to a bet rather than posting the forced bring-in);
  seat 1's rivered pair of queens beats seat 3's pair of jacks; a simple
  capped-street pot, no premium hand needed — wins 2880.
- **hand 8** — seat 2's deuce door owes the bring-in and completes
  straight to a bet; fourth *and* fifth street both cap at four raises;
  seat 1's pocket treys catch a third on fourth street and pair up with
  the board's fours for a full house (treys full of fours), beating seat
  2's and seat 0's one-pair hands; wins 6680.
- **hand 22** — fifth street caps at four raises three-way; seat 3's `6d
  4h` completes a 3-to-7 straight (using upcards `3s`, `7d`, `5s`) to beat
  seat 2's trip jacks and seat 0's two pair (aces and queens); wins 6480.
  **Compare with stud8-fl's hand 22 below** — identical deal and action.
- **hand 24** — a fairly quiet hand (third street's only real raise
  exchange doesn't reach the cap) that runs all the way to seventh street;
  seat 0 rivers a queen-high club flush (five clubs picked up across the
  streets, starting from the `9c` hole card) to beat seat 2's two pair
  (kings and sevens); wins 2480.
- **hand 36** — seat 0's trey door posts a literal forced bring-in; third
  street caps at four raises; seat 2's `Jd Kd` rivers a king-high diamond
  flush to beat seat 1's ace-high (no pair); wins 3680.

## stud8-fl — Seven-Card Stud Hi-Lo (8 or Better)

```sh
./target/release/poker-arena run --game stud8-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log stud8-fl.log
```

- **hand 10** — third, fourth, *and* seventh street all cap at four raises
  — a marathon three-street war; seat 2's pair of eights narrowly beats
  seat 3's pair of sevens for the hi half (3790), while seat 3's 7-6-5-3-A
  takes the low half (3790) over seat 0's weaker qualifying low: hi and lo
  go to different winners despite the war being three-handed the whole
  way.
- **hand 22** — identical deal and action to stud-fl's hand 22 (stud-fl
  and stud8-fl share the same fixed-limit betting structure, so these
  strength-oblivious bots play it out move-for-move): fifth street caps at
  four raises three-way; seat 3's `6d 4h` completes a 3-to-7 straight that
  *also* qualifies as the best low (every card is 8-or-under), scooping
  both the hi (3240) and lo (3240) halves instead of winning hi-only as in
  stud-fl; beats seat 2's trip jacks and seat 0's two pair; wins 6480
  total.
- **hand 29** — heads-up from fourth street on; seat 2's `As Ts` pairs up
  twice more (trip tens, then a second pair) for tens full of sevens,
  beating seat 3's two pair (queens and fives); no low qualifies; wins
  3580.
- **hand 60** — heads-up from fifth street, fourth street capped at four
  raises; seat 2's flush (`Jc` plus four more clubs picked up along the
  way) is simultaneously an 8-7-5-4-2 low, scooping both the hi (1490) and
  lo (1490) halves against seat 1's ace-high with no qualifying low; 2980
  total.
- **hand 136** — seat 1's deuce door posts a literal forced bring-in;
  third and fourth street both cap at four raises; seat 2's king-high
  diamond flush (`Jd` plus four more diamonds) doubles as an 8-6-5-4-3
  low, scooping both hi (2090) and lo (2090) to beat seat 1's two pair
  (aces and treys, no qualifying low); 4180 total.

## razz-fl — Razz

```sh
./target/release/poker-arena run --game razz-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log razz-fl.log
```

Razz's showdown evaluator is A-to-5 low (aces count low), so the
high-class-nibble "premium" tagging used above doesn't apply — every
showdown below is described by its actual low, hand-computed from the
seven cards shown. Razz also forces in the *highest* door card (an
exposed ace counts high for this purpose only, the opposite of how it
counts at showdown).

- **hand 0** — seat 3's exposed ace door forces the bring-in (then folds
  to a raise); third street caps at four raises; seat 2 rivers a
  7-6-5-4-A (seven-low) to beat seat 1's 9-8-7-3-A (nine-low); wins 2730.
- **hand 1** — seat 1's queen door is the highest showing and brings it
  in; fifth street caps at four raises; seat 3's J-T-7-4-3 (jack-low)
  edges seat 1's J-T-8-2-A — both jack-and-ten-low, seat 3's third card
  (7) beats seat 1's (8); wins 5080.
- **hand 4** — seat 0's exposed ace door forces the bring-in, which seat 0
  completes straight to a bet; a raise exchange on third street doesn't
  quite cap; seat 1's 8-5-4-3-A (eight-low) beats seat 0's own
  9-8-6-3-A (nine-low); wins 2380.
- **hand 8** — seat 3's queen door forces the bring-in and completes
  straight to a bet; third *and* fourth street both cap at four raises;
  seat 2's J-9-8-4-2 (jack-low) beats seat 0's Q-T-8-6-5 (queen-low); wins
  4280.
- **hand 10** — seat 0's eight door forces the bring-in and completes
  straight to a bet; fourth street caps at four raises; seat 0's own
  6-5-3-2-A (six-low) crushes seat 3's 9-7-6-5-3 (nine-low) — the
  bring-in completer wins their own pot; wins 2880.

## 27td-fl — 2-7 Triple Draw

```sh
./target/release/poker-arena run --game 27td-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log 27td-fl.log
```

2-7 lowball: aces always count high (worst), straights/flushes count
against you, and any unpaired hand beats any paired hand regardless of
rank; between two paired hands the *lower* pair wins.

- **hand 1** — seat 2 draws one, then discards its entire hand (5 cards)
  on the second draw, then folds anyway; seat 1 and seat 3 both stand pat
  the whole hand. Seat 1's pair of treys (`3h 3c`) beats seat 3's pair of
  jacks (`Jh Jc`) — between two paired hands, the lower pair wins; wins
  3500.
- **hand 4** — seat 2 discards its entire hand on the first draw then
  folds to a raise, seat 3 folds too; seat 1 and seat 0 stand pat the rest
  of the way. Seat 1's rough ace-high (A-Q-7-4-3, the worst possible
  no-pair start) still beats seat 0's pair of kings — any no-pair beats
  any pair; wins 3000.
- **hand 7** — seat 3 draws two, then stands pat, then discards its entire
  hand (5 cards) on the final draw with no more chances left — and it
  works: seat 3's fresh K-J-9-4-2 no-pair beats seat 1's pair of treys and
  seat 0's pair of tens, both of whom stood pat the entire hand; wins
  2800.
- **hand 34** — draw1 and draw2 both cap at four raises across three
  players; seat 1 discards three, stands pat, then discards four more on
  the final draw; the resulting Q-J-9-8-2 no-pair beats seat 3's rough
  ace-high no-pair (Q beats A as the worst card) and seat 2's pair of
  nines; wins 5400.
- **hand 78** — seat 1 draws two, ends up discarding all five, then folds;
  heads-up from there, both remaining players stand pat. Seat 3's `4d Qd
  8c 6c 9d` (Q-9-8-6-4) beats seat 0's `As 6d Ts 2d 8h` (A-T-8-6-2) — ace
  counts as the *worst* card in 2-7, so seat 0's ace-high loses to seat
  3's queen-high; wins 4900. **See a5td-fl's hand 78 below: the identical
  deal and action, but the winner flips**, because A-5 lowball treats that
  same ace as the *best* card instead.

## a5td-fl — A-5 Triple Draw

```sh
./target/release/poker-arena run --game a5td-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log a5td-fl.log
```

Same seed, same bots, same seat count as 27td-fl — and since these builtin
bots never look at their own cards, the action sequence for a given hand
number is identical between the two games. The evaluator isn't: A-5
lowball treats aces as low (the opposite of 2-7). Hands 1, 4, and 34 below
reach the same winner as their 27td-fl counterparts (the ace never ends up
being the deciding card); hand 78 is the one where the ace-low/ace-high
difference flips the result.

- **hand 1** — mirrors 27td-fl's hand 1 move for move: seat 1's pat pair
  of treys again beats seat 3's pair of jacks; a pair still loses to a
  lower pair under A-5 rules the same way it does under 2-7; wins 3500.
- **hand 4** — mirrors 27td-fl's hand 4: seat 1's no-pair ace-high still
  beats seat 0's pair of kings — no-pair beats pair regardless of which
  lowball variant is being played, since seat 0's pair is the deciding
  factor either way; wins 3000.
- **hand 34** — mirrors 27td-fl's hand 34: seat 1's Q-J-9-8-2 no-pair
  (after the same three-then-stand-then-four discard pattern) again beats
  seat 3's no-pair (here reading K-J-6-4-A once the ace counts low, still
  worse than seat 1's queen-high) and seat 2's pair of nines; wins 5400.
- **hand 78** — the flagship hand for this file: identical deal and action
  to 27td-fl's hand 78, but the winner *flips*. Seat 3 stands pat with `4d
  Qd 8c 6c 9d` and seat 0 stands pat with `As 6d Ts 2d 8h`. In 27td-fl the
  ace in seat 0's hand counts high, so seat 0 reads A-T-8-6-2 and loses to
  seat 3's Q-9-8-6-4. In a5td-fl the same ace counts *low*, so seat 0's
  hand reads T-8-6-2-A — and seat 0's ten beats seat 3's queen as the
  worst (highest) card — flipping the winner to seat 0; wins 4900.
- **hand 91** — not in the 27td-fl set: seat 2 draws one, then stands pat,
  then discards three on the final draw and rivers a fresh low; draw2 caps
  at four raises three-way. Seat 2's resulting T-8-7-6-A (ten-low, ace
  counted low) beats seat 1's stand-pat K-J-9-8-3 (king-low) and seat 3's
  stand-pat pair of kings; wins 4500.

## badugi-fl — Badugi

```sh
./target/release/poker-arena run --game badugi-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log badugi-fl.log
```

Badugi hands are ranked by how many of the four cards form a "badugi"
(distinct ranks *and* distinct suits) — 4 beats 3 beats 2 beats 1 card —
then by the low value of the cards actually used; aces count low, same as
A-5 lowball.

- **hand 4** — seat 2 discards its entire hand on the first draw then
  folds to a raise; seat 1 and seat 0 both stand pat the whole hand.
  Seat 1's `3h Ac` (the other two cards, `Qc 7c`, are extra clubs) reduces
  to a two-card 3-A badugi that beats seat 0's two-card 5-2 (`5h 2c`, its
  own extra heart and club discarded); wins 2800.
- **hand 6** — seat 0 discards its entire four-card hand on draw1 (a
  total reset) then stands pat twice; seat 2 stands pat the entire hand.
  Seat 0's reset lands `8h 6c Ad Jd` — an 8-6-A three-card badugi (the two
  diamonds conflict) — that edges seat 2's own three-card T-8-A (also
  ace-anchored, but capped by a ten instead of an eight); both beat seat
  3's two-card T-6; wins 3650.
- **hand 7** — seat 3 folds preflop; seat 2 draws three cards on *every
  one* of the three draw rounds (nine replacement cards total) chasing a
  badugi and still folds on the last draw; seat 1 and seat 0 both stand
  pat the entire hand. Seat 1's `3h 9s 2d` (the second trey is dead
  weight) makes a 9-3-2 three-card badugi that beats seat 0's J-6-4 (two
  clubs conflict, keeping the jack); wins 4300.
- **hand 8** — seat 3 discards its entire hand on draw1, keeps churning
  with three more on draw2, then discards all four again on draw3 before
  folding — three near-total resets in one hand, none of which pay off;
  seat 2 stands pat the whole time with a Q-6-A three-card badugi (two
  spades conflict) that beats seat 0's stand-pat two-card Q-3 (three
  diamonds collide); wins 5400.
- **hand 141** — seat 0 folds draw1 after a bet; seat 2 discards its
  entire hand *twice* (draw1 and draw3, a total reset each time) while
  seat 1 and seat 3 both stand pat the whole hand. Seat 2's second reset
  lands a 7-6-2 three-card badugi that edges seat 1's stand-pat 9-3-A
  (ace-anchored but capped by a nine) and beats seat 3's two-card K-4;
  wins 2900.

## 5cd-nl — No-Limit Five-Card Draw

```sh
./target/release/poker-arena run --game 5cd-nl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log 5cd-nl.log
```

- **hand 0** — four-way all-in preflop; seat 3 discards three cards on the
  single draw street (still only high card afterward) while the other
  three stand pat; seat 2's dealt pair of nines (`9d 9h`, untouched) beats
  seat 0's dealt pair of sevens and two high-card hands; wins the full
  40000 pot.
- **hand 3** — four-way all-in preflop; seat 2 discards its entire hand
  (all five cards) on the draw and comes back with a pair of queens, but
  seat 3 was already dealt two pair (deuces and nines, `7s 2d 9h 2c 9s`)
  and simply stands pat — the best hand at the table needed no help at
  all; beats seat 2's post-discard pair and two high-card stand-pat hands;
  wins the full 40000 pot.
- **hand 31** — four-way all-in preflop; seat 2 discards four cards and
  rivers two pair (kings and tens) to beat seat 1's and seat 3's stand-pat
  pairs of eights and seat 0's discard-three pair of sevens; wins the full
  40000 pot.
- **hand 146** — three-way all-in preflop (one seat folds); seat 2 is
  dealt a straight outright (`9s 7d 8c Tc 6s`, six-to-ten) and stands pat;
  seat 0 discards its entire hand on the draw and still only manages a
  pair of nines, while seat 3 stands pat with a pair of queens; seat 2's
  dealt straight needs no help and wins 30050.
- **hand 163** — four-way all-in preflop; seat 3 is dealt a straight
  outright (`8h 5h 7d 4d 6s`, four-to-eight) and stands pat, beating seat
  1's dealt two pair (kings and queens, also stood pat) and two high-card
  hands; wins the full 40000 pot.
