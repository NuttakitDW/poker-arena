# Curated hand transcripts

This directory holds hand-picked, verified-interesting hands for each of the
twenty registered betting-game variants — real output from `poker-arena run
--log`, not synthetic examples — plus the four registered Open Face Chinese
(OFC) variants, real output from the same binary's `run --log` on an OFC game.
OFC is a different binary, a different registry, and a different wire
protocol (no chips, no betting, no pot — see "OFC transcripts" below and
`WIRE_PROTOCOL_OFC.md`), but the same curation discipline applies: every
`transcripts/<game-id>.jsonl` file, OFC included, contains 3–5 complete
hands, extracted **verbatim** (same bytes, same line order, no renumbering
or reformatting) from a full match log produced with a fixed seed, so
anyone can reproduce the source match and find the same hands at the same
hand numbers.

The first thirteen games (holdem through 27sd-nl) are the "classic"
families: community-card, stud, and draw games where a hand is the
traditional best-5-card poker hand (or a hi/lo split of two such hands).
The last seven (badacey-fl, badeucy-fl, archie-fl, bigo-pl, and the
drawmaha family) are split-pot games built by evaluating the *same* hole
cards two different, independent ways — see "Split-half games" below
before reading those sections, since their `hi`/`lo` fields don't mean
what they mean in the classic games.

Beyond those twenty, four more files (`ofc`, `ofc-pineapple`,
`ofc-progressive`, `ofc-27`) cover Open Face Chinese: no betting, no pot,
boards built one placement at a time instead of hole cards played against a
board, and points instead of chips. See "OFC transcripts" below, after the
twenty betting-game sections, for their own line shape and reading notes
before diving into those four.

## Line shape

Every line is one JSON object:

```json
{"hand": 175, "ev": {"event": "...", ...}}
```

`hand` is the 0-based hand counter for the match (matches `Event::HandStart`
/ `Event::HandEnd`'s position in the stream — the wire protocol's per-bot
`hand_no` is 1-based, but this is the arena-side log, which counts from 0).
`ev` is one `poker_wire::event::Event`, serialized exactly as
[`WIRE_PROTOCOL.md`](../WIRE_PROTOCOL.md) documents under
[Events (`Event`)](../WIRE_PROTOCOL.md#events-event) — the log and
the wire protocol share the same event enum byte-for-byte, so that section
is the authoritative field reference for every event type below
(`hand-start`, `post`, `deal-hole`, `street-start`, `deal-community` /
`deal-up`, `acted`, `draw-result`, `showdown-show`, `pot-awarded`,
`hand-end`).

A hand in one of these files is always the complete, contiguous run from
its `hand-start` line to its matching `hand-end` line — nothing is
truncated mid-hand.

**Two more line kinds, `27sd-nl` only.** The engine that produced
`27sd-nl.jsonl` emits two line kinds with no `"ev"` key at all: a per-hand
header immediately before that hand's `hand-start`,
`{"hand": N, "deck": D, "seats": [...]}` (which bot sat which seat that
hand), and a single trailing `{"log_summary": {"hands_seen": N,
"hands_kept": N}}` at the very end of the file, match-level rather than
per-hand. `27sd-nl.jsonl` keeps each curated hand's header line verbatim
immediately before its events (so `grep`/`jq` still land on a complete,
self-describing block); it does not include the trailing `log_summary`
line, since that's a whole-match summary, not part of any one hand. The
other nineteen files predate this log format change and don't have either
line kind — if you regenerate one of *those* nineteen with the current
binary, expect the same two additions to show up in your fresh log even
though the checked-in file doesn't have them.

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
   stud/stud8, 5cd, bigo, and the omaha half of drawmaha. Badugi's value
   instead encodes how many of the (up to five) cards form a badugi in that
   same nibble, `4`=four-card badugi down to `1`; razz/2-7/A-5's low-hand
   values, and every value in the seven split-half games below, aren't
   nibble-classed the same way, so those games' descriptions spell out the
   actual hand (low string, badugi string, or class name) by hand — see
   "Split-half games" below for how those two values were independently
   re-derived and checked against every one of the 2,800 showdowns in
   their source matches (0 mismatches).
6. `pot-awarded` shows exactly who won what, `side` distinguishing a
   `whole`-pot scoop from a `hi`/`lo` split; `pot` is a **pot index** (0 =
   main pot, 1+ = a side pot), not a chip amount. Side pots never occur in
   these matches: `poker-arena run` resets every seat to the same stack
   each hand, and with equal stacks any all-in call converges on exactly
   the same total commitment as the shove itself, so there's no way for a
   shorter stack to arise mid-hand — see the note under "How to
   regenerate" below. In the seven split-half games, `side: "whole"` has a
   second meaning beyond "no low qualified" — see "Split-half games" below.
7. `hand-end.nets` is winnings-minus-contributions per seat for that hand
   and always sums to zero.

## Split-half games

`badacey-fl`, `badeucy-fl`, `archie-fl`, `bigo-pl`, `drawmaha-fl`,
`drawmaha-27-fl`, and `drawmaha-dugi-fl` all split the pot between two
*independent* evaluations of the same hole cards, rather than a
traditional "best high hand vs. best qualifying low hand" split. Per the
engine's `ShowdownSpec { hi, lo }`, each side names its own evaluator
(`EvalKind`) and its own rule for combining hole cards with the board
(`HoleUsage`) — see `crates/poker-core/src/game/spec.rs` and
`crates/poker-core/src/eval/mod.rs` for the authoritative definitions. In
this directory's event JSON, the field is still always called `hi`/`lo`
(that's the wire format), but what it *means* varies by game:

| game | `hi` side (odd chip) | `lo` side |
|---|---|---|
| badacey-fl | best **A-5 low** (5 cards, aces low) | best **badugi** (4 distinct-rank, distinct-suit cards, aces low) |
| badeucy-fl | best **2-7 low** (aces high, straights/flushes count against you) | best **badugi, aces high** (nut badugi is 5-4-3-2 rainbow) |
| archie-fl | best High hand **with a sixes-or-better qualifier** (no-pair never qualifies) | best A-5 low **with an eight-or-better qualifier** |
| bigo-pl | ordinary Omaha High (exactly 2 of 5 hole + 3 of 5 board) | ordinary eight-or-better Omaha low, same 2-and-3 rule |
| drawmaha-fl | plain High poker on **all five of the player's own cards**, board unused | Omaha-style High (exactly 2 hole + 3 board) |
| drawmaha-27-fl | **2-7 low** on all five own cards, board unused | Omaha-style High (exactly 2 hole + 3 board) |
| drawmaha-dugi-fl | **badugi** on all five own cards, board unused | Omaha-style High (exactly 2 hole + 3 board) |

**A note on which slot is "hi."** For badacey/badeucy/drawmaha, "hi" and
"lo" are just the wire protocol's two slot names, not a claim about which
hand is bigger — per an owner ruling, the *five-card-hand* half (A-5 low,
2-7 low, or the drawmaha in-hand read) is the `hi` slot and gets the odd
chip on an indivisible pot; badugi and the drawmaha Omaha-style half are
`lo`. Archie's and bigo's slot assignment is the intuitive one (badugi
never appears in either of those two games) and is unaffected.

Both sides in badacey/badeucy/drawmaha are "total" evaluators (they always
produce a value), so those five games' pots **always** split — a `"whole"`
award there only ever happens when everyone but one player folds before
showdown, never as a showdown resolution. Archie's and bigo's `lo` (and
archie's `hi`) are *qualifiers* that can fail to produce a hand at all
(`null` in the JSON); per `pot.rs`'s `award_pots`: if exactly one side has
a qualifying hand among the showdown players, that side scoops the *whole*
pot (`side: "whole"`, one winner or a tied group); if **neither** side
qualifies anywhere — only possible in archie, since it's the only one of
these games where both sides are qualifiers — the whole pot splits evenly
among *every* player who showed down, regardless of hand strength. That's
the "neither-qualifies" oddity called out in archie-fl's hand 60 below,
and it's exactly what to grep for: a `pot-awarded` line with
`"side":"whole"` and two or more `winners`, where every remaining player
got a piece.

Every hi/lo value quoted in the seven sections below was independently
re-derived from the raw cards (not read off the encoded `u32`) using a
from-scratch comparator for each evaluator, matching the encoding rules
documented in `eval/mod.rs`'s module doc comment. That comparator's
predicted winner was checked against the engine's actual `pot-awarded`
winner for **all 2,800 showdown hands** across the seven source matches
(400 hands × 7 games) with **zero mismatches**, so the class names, low
strings, and badugi strings quoted below are trustworthy, not guessed.
(Re-verified again, still zero mismatches, after the hi/lo slot ruling
above flipped which side is which for five of these seven games.)

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
- `badacey-fl` / `badeucy-fl` / `archie-fl` deal identically for a given
  hand number (all three are fixed-limit 5-card triple draw); only the
  showdown evaluators differ (see "Split-half games" above).
- `drawmaha-fl` / `drawmaha-27-fl` / `drawmaha-dugi-fl` deal identically
  for a given hand number (identical streets: 5 hole cards, flop, one draw,
  turn, river); only the in-hand `hi` evaluator differs.

Nineteen of the twenty source matches here used `--hands 400 --seed 7
--dealing seeded`, mixing `builtin:random` (creates raises and folds),
`builtin:shover` (creates all-ins), and `builtin:caller` (creates
showdowns) so each match samples a wide range of textures; the seven
split-half games use a 4-seat table (2..=6 max for the draw-based ones)
except `bigo-pl`, which uses 6 seats (its Omaha-style community streets
support up to 9). `27sd-nl` is the exception — see its own section below
for why its bot mix drops `builtin:caller` in favor of `builtin:folder`.
Every claim below (winner, hand class, pot size, discard counts, capped
streets, low strings, badugi strings) was checked against the actual
event lines and, for every low-hand or split-half game, independently
hand-computed
and cross-checked against the engine's own winner — not guessed from the
bot mix or reused from a previous run.

**A note on engine changes across three regeneration passes.** This
directory has been rebuilt against three successive engine revisions: (1)
a change to seat-assignment randomization, (2) a change adding cumulative
bet-reopening after short all-ins, and (3) a change that reshuffles folded
hands back into the muck. Passes (2) and (3) turned out not to change any
of the twelve classic games' output at all: `poker-arena run` resets every
seat to an identical stack each hand, so the reopening paths never trigger
(see the side-pot note below for the identical underlying reason), and
folded-card reshuffling doesn't perturb a *seeded* per-hand deck the way
it might in a continuously-dealt cash-game shoe. Each pass was verified by
literally re-running every command below and diffing the regenerated log
byte-for-byte against the previously captured one before touching
anything; only pass (1) actually changed any hands, and only the twelve
games that existed before this pass's engine changes were subject to (2)
and (3) — the seven split-half games below were generated fresh against
the current binary and have only ever seen this one engine revision.

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
happen here. We looked for one across all twenty 400-hand source matches
(8,000 hands) and confirmed zero `pot-awarded` events with a nonzero pot
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

## badacey-fl — Badacey

5-card fixed-limit triple draw, split every hand between the best A-5 low
(the `hi` slot, gets the odd chip) and the best badugi, aces low (the `lo`
slot) on the same five cards — see "Split-half games" above for why the
low is the "hi" slot here. Because both sides are total evaluators,
badacey **always** splits at showdown (a `"whole"` award only happens if
everyone else folds pre-showdown); a "scoop" below means the same seat
wins both halves, not that the pot goes undivided.

```sh
./target/release/poker-arena run --game badacey-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log badacey-fl.log
```

- **hand 4** — seat 2 discards its entire hand on the first draw (a total
  reshuffle) then folds; seat 0 and seat 1 go to showdown. Seat 1's `3h Ac
  Qc 7c 4h` is a no-pair Q-7-4-3-A that beats seat 0's paired K-K-T-8-4 low
  and takes the A-5/hi half (1500); seat 0's `8h Kd 4s Kc Td` makes a
  four-card K-T-8-4 badugi (the two kings conflict), better than seat 1's
  two-card 3-A (three clubs and two hearts collide), and takes the
  badugi/lo half (1500) — a clean split, each seat winning the half the
  other is worse at.
- **hand 6** — seat 0 folds after a raise war, seat 1 discards three then
  folds too, leaving seat 2 vs. seat 3. Seat 3's own five cards read a
  no-pair K-T-9-8-6 that beats seat 2's paired T-T-8-7-A and takes the A-5
  half (1750); seat 2's `Ac 8s Th 7d` (dropping the second ten) is a
  four-card T-8-7-A badugi — the ace counts *low* here and helps — bigger
  than seat 3's two-card 9-6, and takes the badugi half (1750) — another
  split.
- **hand 9** — seat 0 folds preflop; seat 2 discards two, then all five,
  chasing improvement before folding on the last round. On the A-5 side
  seat 1's pair of treys (9-7-3-3-A, using all five cards) beats seat 3's
  trip sixes (Q-6-6-6-3) — fewer duplicates wins; seat 1 and seat 3 also
  scrape together lousy two-card badugis (3-A vs. 6-3), and seat 1's is
  lower — seat 1 scoops both halves, 4200.
- **hand 24** — a multi-way preflop pot narrows to seat 0 vs. seat 2 after
  draw1 betting caps at four raises. Seat 2's `4d 9c Ts Th 2c` carries a
  pair of tens (T-T-9-4-2) that beats seat 0's pair of kings (K-K-Q-T-7)
  on the A-5 half, and also makes a three-card T-4-2 badugi, bigger than
  seat 0's two-card T-7, on the badugi half — seat 2 scoops both halves,
  3500.
- **hand 34** — the richest badacey hand in the set: draw1 and draw2 both
  go to a three-way raise war after seat 0 folds; seat 1 discards three,
  stands pat a round, then discards four more on the final draw. The
  resulting `9c Jh Qc 8d 2h` makes a no-pair Q-J-9-8-2 low that beats seat
  2's paired Q-9-9-5-4 and seat 3's higher-topped K-J-6-4-A on the A-5
  half, and a three-card 9-8-2 badugi — the best of the three players'
  badugis (seat 2 makes Q-9-4, seat 3 makes A-J-4, both also three-card
  but higher) — on the badugi half; seat 1 scoops both halves, 5400.

## badeucy-fl — Badeucy

Badacey's evil twin: the same 5-card fixed-limit triple draw, but the low
half (`hi` slot, odd chip) is 2-7 (aces high, straights/flushes count
against you) and the badugi half (`lo` slot) counts aces *high* too (the
nut badugi is 5-4-3-2 rainbow) — so an ace is bad news on *both* halves at
once, unlike badacey where aces help both. Like badacey, both sides are
total evaluators, so the pot always splits at showdown.

```sh
./target/release/poker-arena run --game badeucy-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log badeucy-fl.log
```

- **hand 4** — the badeucy sibling of badacey's hand 4 (identical deal):
  seat 1's ace gets excluded from its own best badugi (a small two-card
  7-3 ace-high, three clubs and two hearts leaving no room for it), but
  DeuceToSevenLow must use all five cards including that ace, and seat 1's
  no-pair `A-Q-7-4-3` still beats seat 0's paired `K-K-T-8-4` (no-pair
  beats any pair, ace-high top card and all) to take the 2-7/hi half
  (1500); seat 0's `8h Kd 4s Kc Td` makes a four-card K-T-8-4 badugi and
  takes the badugi/lo half (1500) — split.
- **hand 27** — seat 0 folds preflop; seat 2 discards its whole hand on
  draw1, then four more on draw2 (two near-total resets), before folding
  to a raise on draw2; seat 1 and seat 3 — who both stand pat the entire
  hand — reach showdown untouched. Seat 1's own `8-5-4-3-2` beats seat 3's
  `J-8-6-3-2` on the 2-7 half (2200); seat 3's `Jd 2c 8h 6s 3s` makes a
  four-card J-8-3-2 badugi (bigger than seat 1's three-card 4-3-2) and
  takes the badugi half (2200) — split, no ace involved on either side
  this time.
- **hand 34** — the badeucy sibling of badacey's hand 34 (identical deal):
  seat 1 (after discarding three, then four more) makes a no-pair
  Q-J-9-8-2 low and an ace-free three-card 9-8-2 badugi, both good enough
  to scoop against seat 2's paired Q-9-9-5-4/three-card Q-9-4 and seat 3's
  A-K-J-6-4/three-card A-J-4. Seat 3's ace tops their 2-7 low as the worst
  possible card, and is also *forced* into their badugi — excluding it
  would leave only two cards, worse than three; seat 1 wins both halves,
  5400.
- **hand 40** — seat 3 folds preflop; seat 1 churns hard (discards four,
  then four more) and still can't keep up. Seat 2 is dealt pocket aces
  (`Ah`, `As`) — about the worst possible badeucy holding: DeuceToSevenLow
  must count both, reading a paired `A-A-T-7-2` (doubly bad: a pair, and
  the pair is aces), and the same pair limits their badugi to a three-card
  `A-T-2` (forced to keep one ace since a pair of clubs also collides).
  Seat 0's ace-free `Jc 5s 9d Td 2h` — a no-pair J-T-9-5-2 low and a
  four-card J-9-5-2 badugi — scoops both halves, 4900 total.
- **hand 91** — the clearest double-ace-penalty in the set. DeuceToSevenLow
  uses all five own cards with no choice, so seat 2 reads `A-T-8-7-6` and
  seat 3 reads `A-K-K-8-5` (double-cursed: an ace on top of a pair of
  kings) — both damaged hands. The same aces show up on the badugi side:
  seat 2's `Ts 7h Ac 6d` makes a four-card badugi (`A-T-7-6`) only because
  including the ace still beats a smaller three-card hand — the ace is
  dragged along as the worst member — while seat 3's `Kc Ks 5c As 8d`
  shows the opposite: the best three-card badugi available (`K-8-5`)
  explicitly *prefers* the king over the ace it's also holding, discarding
  `As` entirely rather than let it in. Seat 1's ace-free `Kd 9s 8c Jc 3h`
  (a K-J-9-8-3 low and a four-card K-9-8-3 badugi) has no such problem and
  scoops both halves, 4500 total.

## archie-fl — Archie

The third 5-card fixed-limit triple draw variant, and the only one of the
three where either half can fail to qualify: the hi side needs at least a
pair of sixes (SixesOrBetterHigh — a no-pair hand *never* qualifies, no
matter how high), and the lo side needs an 8-or-better A-5 low, same as
omaha8/stud8. If exactly one side qualifies anywhere, that side scoops the
*whole* pot (logged as `side: "whole"`, not a hi/lo split); if **neither**
side qualifies for anyone, the whole pot splits evenly among every player
who reached showdown, regardless of hand strength — the one true oddity in
this set, see hand 60.

```sh
./target/release/poker-arena run --game archie-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log archie-fl.log
```

- **hand 4** — the archie sibling of badacey's/badeucy's hand 4 (identical
  deal): seat 0's `Kd Kc` pair of kings is the only qualifying hand at the
  table (seat 1's ace-high never reaches a pair) — since the lo side never
  qualifies for anyone either, seat 0 scoops the *entire* pot as a single
  `"whole"` award (not a hi/lo split), 3000. One side scooping because the
  other never had a candidate.
- **hand 59** — a clean qualifying split: seat 0's dealt pair of queens
  (`Qc Qd`) is sixes-or-better and takes the hi half (1800); seat 3
  discards all five chasing a low and folds, leaving seat 1's untouched
  `3h 8h Ad 5c 6d` to make an 8-6-5-3-A low and take the lo half (1800).
- **hand 60** — the flagship hand: **neither side qualifies for anyone**.
  Seat 3 folds preflop; draw1 and draw2 both cap at four raises three-way;
  seat 0 discards two, then four more, but folds on draw3 facing a bet.
  Seat 1 reaches showdown with a jack-high no-pair (`3h 6h 8d Jc 9s` — a
  no-pair hand never qualifies for the hi side, no matter how high) and
  seat 2 shows a pair of deuces (`4d 7s Ks 2d 2c` — a real pair, but well
  below the sixes-or-better bar); neither hand has a qualifying low either.
  Per the rule above, the entire pot is awarded as a single `"whole"` line
  with **two winners**, split evenly regardless of either hand's actual
  strength: seats 1 and 2 each get 2600.
- **hand 163** — a genuine double-qualifying scoop, not a "nobody else
  qualified" scoop: seat 3's `8h 5h 7d 4d 6s` is simultaneously a made
  4-to-8 straight (qualifies as sixes-or-better *and* better than seat 1's
  two pair) and an 8-7-6-5-4 low — the same five cards legitimately win
  both sides outright; wins 2800 total.
- **hand 191** — a three-way showdown where every side goes to a different
  seat and a third player qualifies for nothing at all: seat 1's dealt
  pair of queens takes the hi half (1650); seat 3's `Ad 6s 5h 2h 4s` (a
  6-5-4-2-A low) takes the lo half (1650); seat 0, who churns through the
  hand discarding five, then three, then three more, qualifies for
  neither and wins nothing.

## bigo-pl — Big O (Five-Card Omaha Hi-Lo)

Omaha hi-lo with five hole cards instead of four (up to nine seats fit —
5 × 9 + 5 = 50 ≤ 52 cards). Both sides use the ordinary Omaha
exactly-2-hole/3-board rule; the hi side is plain High poker, the lo side
is the standard eight-or-better A-5 low, exactly like `omaha8-pl`, just
with one more hole card to choose from.

```sh
./target/release/poker-arena run --game bigo-pl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --bot builtin:random:3 \
  --bot builtin:random:5 --log bigo-pl.log
```

- **hand 0** — six-way preflop pot (preflop caps at five raises); board
  `7d 2h 2s / 6h / 3c` pairs deuces. Seat 4's `7h 7c` (two of the five
  hole cards) makes a full house (sevens full of twos) with the board's
  `7d 2h 2s`, and a different pair from that same five-card hole, `Ac 4s`,
  combines with `2h 6h 3c` for a 6-4-3-2-A low — two different
  two-card slices of the same hand, each winning its own half — scoops
  both halves, the entire 50000 pot.
- **hand 10** — seat 2's `5h 5d` trips up with the board's `5c` for trip
  fives and takes the hi half alone (15325); seat 0 and seat 4 tie for the
  best qualifying low (`6-5-3-2-A` each) and split that half, 7662/7663
  (odd chip breaks the tie) — hi-alone plus a quartered low.
- **hand 43** — seat 5's `7s 6h` completes a 4-to-8 straight off `7d 5d 8s
  / Jd / 4d` and takes the entire hi half alone (30000); on the low side,
  the same `Ah 2d` plus board cards *also* make a qualifying `7-5-4-2-A`
  for seat 5, tying with seat 1's identical `7-5-4-2-A` and splitting that
  half 15000/15000 — the hi winner doubles up on half the low too.
- **hand 327** — seat 1 and seat 3 tie on *both* sides: board `Ks 4d 5s /
  Ac / 2s` lets both make a 2-to-6 straight (`3h 6h`/`4d 5s 2s` for seat 1,
  `6d 3d`/the same three board cards for seat 3) that also qualifies as a
  `5-4-3-2-A` low — both halves chop evenly between the same two seats,
  7638/7637 each.
- **hand 363** — the classic double-chop: seat 0 and seat 5 both hold the
  wheel itself (`Ah 4d`/`5s 3c 2d` and `As 4c`/the same three board cards),
  which is simultaneously the nut low (`5-4-3-2-A`) and a made straight —
  both hi and lo split evenly between the same two seats, 12500 each,
  25000 total per seat.

## drawmaha-fl — Drawmaha

Five hole cards over a hold'em-style board (preflop → flop → **one draw
round, no betting on it** → turn → river). The pot always splits between
an "in-hand" half (`hi` slot, odd chip) — plain High poker on the whole
five-card hand the player ends the draw with, board not used at all — and
an Omaha-style half (`lo` slot, exactly 2 hole + 3 board, also ordinary
High poker). Both are total evaluators, so — like badacey/badeucy — the
pot always splits at showdown; a "scoop" means the same seat wins both
halves.

```sh
./target/release/poker-arena run --game drawmaha-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log drawmaha-fl.log
```

- **hand 0** — seat 3 discards its entire hand on the single draw round;
  seat 2's own dealt pair of nines beats seat 1's no-pair in-hand read and
  takes the in-hand/hi half, and seat 2's `Ks 8h` also makes two pair
  (kings and eights) with the board's `Kc 8c Qs` on the omaha/lo half —
  seat 2 scoops both halves, 2100.
- **hand 15** — seat 1, dealt pocket trip aces (`As Ad Ac`, stood pat), has
  the best in-hand read at the table and takes the in-hand/hi half (1850);
  seat 0's `4s 3s` completes a flush with the board's `Js 6s 7s` and takes
  the omaha/lo half alone (1850) — despite only a merely-ordinary two-pair
  omaha hand of its own, seat 1's in-hand and omaha winners are two
  different seats, and the same player's own two evaluations disagree
  about who's best.
- **hand 34** — seat 2's dealt pair of nines beats seat 3's no-pair
  in-hand read and takes the in-hand/hi half (1500); seat 3's `6h Kh`
  makes a flush with the board's `Th 5h 8h` and takes the omaha/lo half
  (1500) — in-hand and omaha go to different seats again.
- **hand 65** — seat 2's own five cards read a pair of kings, better than
  seat 3's in-hand pair of nines, and take the in-hand/hi half; seat 2's
  `Ks Qd` also completes a broadway straight with the board's `Td Jd As`
  and takes the omaha/lo half — seat 2 scoops both halves, 2000.
- **hand 224** — seat 2 discards its entire hand on the single draw round;
  the fresh five cards read a pair of sevens, beating seat 1's and seat
  0's in-hand no-pairs, and also river trip sevens on the omaha half (`7d
  7h` plus the board's `9s 7c As`) — seat 2 scoops both halves, 1800.

## drawmaha-27-fl — Drawmaha 2-7

The drawmaha structure (5 hole cards, flop, one no-betting draw, turn,
river), but the in-hand half (`hi` slot, odd chip) is scored as 2-7
lowball (aces high, straights/flushes count against you) instead of High
poker; the omaha half (`lo` slot) is still ordinary High poker. Both sides
are total evaluators, so the pot always splits.

```sh
./target/release/poker-arena run --game drawmaha-27-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log drawmaha-27-fl.log
```

- **hand 6** — seat 3's own `6d 8d 9h Kd Td` reads a no-pair `K-T-9-8-6` on
  the 2-7/in-hand side, beating seat 2's paired `A-T-T-8-7` (a pair of
  tens) and seat 0's own no-pair `A-Q-J-8-6` (worse than seat 3's simply
  because ace is the highest, worst 2-7 card) — seat 3 takes the
  in-hand/hi half (3175) even though its omaha hand was worse than both
  rivals'; seat 2's `Th Ts` makes two pair (tens and deuces) with the
  board's `2d 8c 2s` and takes the omaha/lo half (3175).
- **hand 15** — seat 0, on the in-hand side, reads a no-pair `A-8-5-4-3`
  that beats seat 1's dealt `3h As 2c Ad Ac` — trip aces, about as bad as
  2-7 gets, since trips are bad and aces count as the worst rank — and
  seat 3's paired `Q-T-8-4-4`, taking the in-hand/hi half; seat 0's `4s
  3s` also flushes with the board's `Js 6s 7s` for the omaha/lo half —
  seat 0 scoops both halves, 1850.
- **hand 34** — the same five cards' no-pair `A-K-J-6-4` beats seat 2's
  paired `Q-9-9-5-4` on the 2-7/in-hand side, taking that half for seat 3;
  seat 3's flush (`6h Kh` plus the board's `Th 5h 8h`) also takes the
  omaha/lo half — seat 3 scoops both halves, 1500.
- **hand 65** — seat 2's own hand reads a paired `K-K-Q-8-7` on the 2-7
  side — worse than seat 3's paired `T-9-9-8-2` (the lower pair wins for
  2-7) — so seat 3 takes the in-hand/hi half (2000); seat 2's broadway
  straight (`Ks Qd` plus the board's `Td Jd As`) takes the omaha/lo half
  (2000): in-hand and omaha split between two seats again.
- **hand 356** — the richest hand in the set: seat 1 discards its entire
  hand on the single draw round; the same fresh five cards read a no-pair
  `K-Q-6-4-2` on the 2-7 side, better than all three rivals' one-pair
  in-hand reads (seat0's paired treys, seat2's paired nines, seat3's
  paired fives), taking the in-hand/hi half; seat 1 also rivers a full
  house (fours full of sixes: `6h 4c` plus the board's `4s 6s 4h`) on the
  omaha half — seat 1 scoops both halves, 4200.

## drawmaha-dugi-fl — Drawmaha Dugi

The drawmaha structure again, but the in-hand half (`hi` slot, odd chip)
is badugi (aces low, best of up to four of the player's own five cards —
same evaluator as `badugi-fl`, just fed the drawmaha hand instead of a
dedicated deal); the omaha half (`lo` slot) is ordinary High poker. Both
sides are total evaluators, so the pot always splits.

```sh
./target/release/poker-arena run --game drawmaha-dugi-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log drawmaha-dugi-fl.log
```

- **hand 34** — the drawmaha-dugi sibling of the drawmaha-fl/-27-fl hand
  34 (identical deal, same `9c Th 5h` flop): seat 3's five cards make a
  three-card `J-4-A` badugi (the ace counts *low* here, unlike badeucy, so
  it's a genuine asset) that beats seat 2's three-card `Q-9-4`, taking the
  badugi/hi half; seat 3's `6h Kh` also flushes with the board's `Th 5h
  8h` for the omaha/lo half — seat 3 scoops both halves, 1500.
- **hand 55** — seat 2 discards its entire hand on the single draw round
  and still can't beat a four-card badugi: seat 1's `3h 7c 3c 8d As` is a
  four-card `8-7-3-A` badugi (the ace slots in as the low anchor), taking
  the badugi/hi half, as well as two pair (aces and sevens: hole
  `7c`/`As` plus the board's `Ah`/`7h`, kicker `Qc`) on the omaha half —
  seat 1 scoops both halves, 1950.
- **hand 65** — the flagship: seat 3's `9s 8h 9d 2s Tc` is a genuine
  **four-card** `T-9-8-2` badugi and takes the badugi/hi half (2000)
  despite only a one-pair omaha hand of its own; seat 2's own five cards
  reduce to just a two-card `8-7` badugi, but seat 2's `Ks Qd` completes a
  broadway straight with the board's `Td Jd As` and takes the omaha/lo
  half (2000) — badugi and omaha go to two different seats, and the
  badugi winner has the weaker omaha hand of the two.
- **hand 79** — seat 3 folds preflop; seat 0 and seat 2 cap the turn at
  four raises (200→400→600→800) three-way before seat 2 folds the river.
  Seat 1's `3h 4c 2d 7d 7s` makes a clean four-card `7-4-3-2` badugi with
  no ace needed at all, beating seat 0's two-card `T-2`, and takes the
  badugi/hi half; seat 1 also makes trip sevens on the omaha half (`7d
  7s` plus the board's `Kd Td 7h`) — scoops both halves, 2000.
- **hand 142** — seat 0, who stood pat the entire hand, has only a
  one-pair omaha read but a three-card `6-4-A` badugi (the ace anchoring
  it low) that beats seat 3's two-card `Q-2` and seat 2's three-card
  `J-5-2` — seat 0 takes the badugi/hi half (3200); seat 2's `Jd Jh` trips
  up with the board's `Td Jc Kc` for trip jacks and takes the omaha/lo
  half (3200): badugi and omaha go to different seats again.

## 27sd-nl — No-Limit 2-7 Single Draw

The newest registered game: the same skeleton as `5cd-nl` (predraw no-limit
bet, one draw, postdraw no-limit bet, single showdown side, `lo` always
`null`) but scored by `DeuceToSevenLow` instead of standard high — a made
pair is *worse* than any unpaired hand, the ace always counts high (no
wheel), and the best possible hand is `7-5-4-3-2`.

```sh
./target/release/poker-arena run --game 27sd-nl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:random:3 \
  --bot builtin:folder --bot builtin:random:9 --log 27sd-nl.log
```

This bot mix is deliberately different from the other nineteen games'
`random`/`shover`/`caller`/`random:9` (see "How to regenerate" above).
`builtin:caller` never folds, which structurally guarantees a showdown
whenever it's dealt in, so a match built from the usual mix produced zero
fold-outs and zero snows in 400 hands — nothing for a single-draw lowball
game to show off. Swapping `caller` for `builtin:folder` alone still
wasn't enough: with `builtin:shover` still at the table, its immediate
predraw all-ins short-circuited most hands before they ever reached the
draw, so the resulting fold-outs were all pre-draw and postdraw play never
happened. Dropping `builtin:shover` too — leaving `random` / `random:3` /
`folder` / `random:9`, no unconditional all-in bot and no unconditional
caller — let hands actually reach the draw via checks and calls, which is
what produces genuine post-draw raise wars, fold-outs, and snows. (As a
side note, this game deals identically to `5cd-nl` at matching hand
numbers whenever both are run with `--seed 7 --dealing seeded` despite the
different bot lineups — dealing is independent of which bots are seated —
though the two files' curated hand numbers don't happen to overlap.)

Every claim below (low string, pair/no-pair class, fold timing, pot size)
was independently hand-computed from the raw `showdown-show`/`draw-result`
events with a from-scratch `DeuceToSevenLow` comparator and cross-checked
against the engine's own `pot-awarded` winner, not guessed.

- **hand 201** — three-way to the draw after a preflop raise to 4771; seat
  1 stands pat on `3h Qd 2h 9s Td` (`Q-T-9-3-2`, no pair), seat 2 discards
  two, seat 0 discards one. Postdraw, seat 0 bets 1970, seat 1 raises to
  5100, seat 2 calls, seat 0 folds. At showdown seat 1's pat `Q-T-9-3-2`
  beats seat 2's drawn `3s 4d 6d 6s Jd` (`J-6-6-4-3`, paired sixes); wins
  26483.
- **hand 207** — a three-way preflop raise war (2828 → 5848 → 9186, all
  three call the last raise) before the draw; seat 2 discards three, seats
  3 and 0 both stand pat, and everyone checks the entire postdraw street to
  showdown. Seat 0's pat `4h 2s 6h 7c 8d` (`8-7-6-4-2`) beats seat 3's pat
  `Tc 6s 3c 9d 7d` (`T-9-7-6-3`) and seat 2's drawn `9h 4d 6c Ad 3d`
  (`A-9-6-4-3`) — the best of three pat-or-near-pat hands takes it; wins
  the full 27608 pot.
- **hand 233** — three-way to the draw (one seat folds preflop); seat 1
  discards all five cards, seat 2 stands pat, seat 0 discards two.
  Postdraw, seat 1 leads for 9296, seat 2 raises all-in to 9900, seat 0
  calls all-in, and seat 1 folds its own bet rather than continue three-way
  — a multiway all-in missing its own aggressor at showdown. Seat 2's pat
  `7s 5c Th 8h Jd` (`J-T-8-7-5`) beats seat 0's drawn `2d 6s 4d As Qs`
  (`A-Q-6-4-2`, ace-high); wins 29396.
- **hand 274** — three-way to the draw (one seat folds preflop); seat 1
  discards all five, seat 0 discards all five, and seat 2 discards three,
  drawing back both `Ah` *and* `Ad` — meaning seat 2's final hand contains
  a pair of aces no matter which two of its original five cards it kept,
  about as bad a made hand as 2-7 has. Postdraw, seat 2 bets 3183, seat 0
  raises to 9364, seat 1 calls, seat 2 shoves all-in to 9900, and both
  opponents fold to a hand that — per the guaranteed ace pair — was almost
  certainly no good; seat 2 takes the full 28392 pot without ever showing,
  the flagship post-draw bluff fold-out.
- **hand 295** — the flagship snow: seat 2 raises preflop to 7507 and seat
  1 calls; both stand pat. Postdraw, seat 2 bets 942 and seat 1 raises to
  2432; seat 2 folds. Seat 1's actual hand, `3h 2d 4s Th 3s` (`T-4-3-3-2`,
  a pair of treys), is the *weaker* of the two by 2-7 rules — a made pair
  is categorically worse than any unpaired hand — since seat 2's own pat
  `Jh Ad Kc 6s Qd` (`A-K-Q-J-6`, no pair) is unpaired and would have won
  any showdown; seat 1 stood pat with the worse hand and bet it as the
  better one, winning 16898 purely on fold equity.

---

## OFC transcripts

Four more files, one per registered OFC variant
(`poker-arena games` lists them under the `ofc` family). OFC has no chips, no betting, no pot,
and no legal-actions surface — only placement decisions — so its wire
protocol (`WIRE_PROTOCOL_OFC.md`) and its log format are both entirely
different from the twenty games above. The authoritative rules contract is
the module doc at the top of `crates/poker-core/src/ofc/state.rs`; what
follows is enough to read these four files without it.

**Line shape.** Every OFC hand opens with a header line carrying no `"ev"`
key, `{"hand":N,"seats":[...]}` — like `27sd-nl`'s header (see above) except
there is no `deck` field: an OFC match has no deck-grouping concept, one hand
is one deck, so there is nothing to group (see the module doc at the top of
`crates/poker-arena/src/ofc/log.rs`). Every event line is
`{"hand":N,"ev":<OfcEvent>}`, serialized exactly as
[`WIRE_PROTOCOL_OFC.md`](../WIRE_PROTOCOL_OFC.md) documents under
[Events (`OfcEvent`)](../WIRE_PROTOCOL_OFC.md#events-ofcevent) — `fantasyland`,
`deal`, `place`, `showdown`, `score`. As with `27sd-nl`, each file below keeps
its curated hands' header lines but drops the trailing
`{"log_summary":{...}}` line, since that's match-level, not per-hand.

**How to read a hand.** `fantasyland` (if present) announces a seat's card
count for the hand before any deal. `deal`/`place` pairs repeat once per
placement turn, in table order (seat 1, 2, …, n−1, 0 — the button, seat 0,
goes last), until every seat's board is full: `deal` shows the cards a seat
was just dealt (private — other seats' copies of the same event show
`"cards":[]` with `count` intact, exactly like `deal-hole` in the twenty
games above), and `place` shows where they went (`placements`; `discarded`
lists the rest). Once every board is full, one `showdown` event per seat (top
three cards, middle five, bottom five, each row's `HandValue`, its royalties,
the foul flag, and next hand's fantasyland count, `null` if none) reveals
everything, in table order; one `score` event per seat, also table order,
gives that seat's net points for the hand, always summing to zero.

**Discard and fantasyland privacy, visible here.** On the real wire, a `place`
event's `discarded` field is always redacted to `[]` for every seat but the
one placing (`count` stays accurate), and a fantasyland seat's `placements`
read as `[]` to everyone else until its `showdown` reveal — see
[Events (`OfcEvent`)](../WIRE_PROTOCOL_OFC.md#events-ofcevent). This
directory's logs are the arena's own internal record, not any one bot's wire
feed, so — exactly like the "a spectator/log reader sees everything" note for
the twenty games above — neither redaction applies here: every seat's
`discarded` cards and every fantasyland seat's `placements` are printed in
full as they happen, not just at showdown. Hand 249 in `ofc.jsonl` and hand
97 in `ofc-pineapple.jsonl` are fantasyland hands below; their `place` events
show the placing seat's full board being built turn by turn, something no
live opponent bot would see until that hand's `showdown` line.

**Fantasyland freezes the seats.** The OFC runner rotates bots through
seats between hands for positional fairness — except into a hand where any
seat is in fantasyland: that hand is an extension of the hand that earned
the fantasyland, so everyone keeps their previous seat until no seat is in
fantasyland, and the rotation then resumes (see the module doc at the top
of `crates/poker-arena/src/ofc/runner.rs`). A `showdown` event's
`next_fantasyland` count travels with the *bot* that earned it, and because
of the freeze, the bot plays its fantasyland hand from the very seat where
it earned it. Below, every fantasyland-entry hand is immediately followed
by that same bot's fantasyland hand, same seat, same header order.

**Scoring, briefly.** Rows are valued bottom/middle with `high` (or, for
`ofc-27`'s middle, `deuce_to_seven_low`) and top with `three_card_high`;
greater is always better. A board fouls if its rows are out of order (top >
middle or middle > bottom; `ofc-27` instead fouls on top > bottom, or a
middle that isn't a qualifying ten-low-or-better 2-7 hand — `ofc-27`'s middle
has no ordering relationship with its neighbors at all, only its own
qualifier). Scoring is pairwise over every pair of seats: with neither
fouled, +1 per row won outright (ties pay nothing) plus 3 more for winning
all three, plus the royalty difference (royalties always count, win or
lose); with one fouled, the fouler pays 6 plus the opponent's royalties and
its own are voided, rows uncompared; with both fouled, nothing changes
hands. A seat's net is the sum over all its pairs, and every hand's nets sum
to zero. Royalty tables (top pairs/trips are rank-scaled; middle and bottom
are flat per hand class) are in the module doc.

**Build.** The same one binary as the twenty games above:
`cargo build --release -p poker-arena-cli` — OFC variants run through
`poker-arena run --game ofc…` like any other game. Each section below gives
the exact command used for that variant's source match, plus which hand
numbers were kept from it.

**`WIRE_PROTOCOL_OFC.md`'s example transcript, captured.** That document's
"Example transcript" section (a wire-bot's-eye view of one hand, seed 113)
says to regenerate it with the capture recipe here: it was captured with
`poker-arena run --game ofc-pineapple --bot builtin:greedy --bot
cmd:"python3 <capture wrapper>" --hands 1 --seed 113`, where `<capture
wrapper>` is `examples/ofc_bot.py` modified to tee each line it receives to a
file prefixed `"< "` and each line it sends prefixed `"> "`, in the order
they cross the wire — the same `<`/`>` convention `WIRE_PROTOCOL.md`'s own
example transcript uses. That file's own transcript is captured the same
way it's always been: no recipe is spelled out in its own doc beyond calling
itself "captured verbatim from a real match" — this note exists because
`WIRE_PROTOCOL_OFC.md` explicitly points here for its recipe; `WIRE_PROTOCOL.md`
makes no such promise and needs no such note.

## ofc — Open Face Chinese

```sh
./target/release/poker-arena run --game ofc --hands 400 --seed 7 \
  --bot builtin:greedy --bot builtin:random --bot builtin:filler \
  --bot builtin:random:9 --log ofc.log
```

Four seats (`greedy`, `random`, `filler`, `random-2`) so at least one hand
below shows genuine multiway pairwise scoring, not just a single 1-vs-1
comparison.

- **hand 3** — a clean four-way hand: nobody fouls and nobody rivers a
  royalty-grade row; three ordinary rows per seat are compared pairwise with
  nothing else going on. Final nets: seat 1 −13, seat 2 +11, seat 3
  (`greedy`) +13, seat 0 −11.
- **hand 127** — seat 0 fouls: its middle (a bare pair of sixes, `6c 6d 8c
  Jd 4d`) is worth more than its own bottom (`Jc 5d Ks 4c 7d`, king-high, no
  pair at all), breaking the required top ≤ middle ≤ bottom order, so it
  pays 6 plus royalties to each of the other three seats and its own
  royalties are voided. Seat 3 (`greedy`) happens to river bottom quad
  nines (`9c 9h 9s 3d 9d`, royalty 10) — comfortably the best hand at the
  table — and nets +34; seats 1 and 2 net −2 and −4 from their own
  row-by-row play against each other and seat 3; seat 0 nets −28, the 6+10,
  6+0, 6+0 foul tax paid three times over.
- **hand 248** — three of the four seats foul at once (seats 1, 2, and 3),
  leaving seat 0 (`greedy`) the only clean board; a foul-vs-foul pairing
  changes nothing per the rule above, so the whole hand reduces to three
  identical 6-plus-royalties payments into seat 0. Seat 0's `9d Qs Qd` pairs
  queens on top (royalty 7) for 6+7=13 from each opponent — net +39, a clean
  13×3 sweep. QQ+ on top also earns fantasyland: `next_fantasyland:13`. See
  hand 249.
- **hand 249** — the fantasyland hand itself, one hand later. Under the
  freeze rule, `greedy` — the seat-0 bot from hand 248 — is *still* seat 0
  here, not rotated away (contrast the old behavior described in "Fantasyland
  freezes the seats" above); its `fantasyland` event announces 13 cards up
  front, and its one placement turn puts down a full board at once with no
  single row earning a royalty: two pair (deuces and treys) in the middle,
  two pair (fours and sixes, ace kicker) on the bottom, no pair on top. Even
  royalty-less, it's enough: seat 1 (`random`) fouls a pair of aces on top
  over a pair of eights in the middle, and seat 2 (`filler`) fouls an
  ace-high top over a king-high middle — each pays greedy the flat 6 (no
  royalties on either side to add); seat 3 (`random-2`), the only clean
  board, still loses every row outright (seven-high top, no-pair middle,
  one-pair bottom, all weaker than greedy's) for a further 6 — the +1-per-row
  plus the +3 sweep bonus. Greedy nets +18. A two-pair bottom isn't quads, so
  fantasyland doesn't carry over: `next_fantasyland:null`.

## ofc-pineapple — Pineapple OFC

```sh
./target/release/poker-arena run --game ofc-pineapple --hands 200 \
  --seed 7 --bot builtin:greedy --bot builtin:random --bot builtin:filler \
  --log ofc-pineapple.log
```

Pineapple rounds deal 3 and place 2, discarding the third; per the privacy
note above, every discard below is visible in this log. In `hand 45`, for
example, every `place` event after the opening 5-card turn carries its
seat's real discarded card in the clear (e.g. seat 1's `"discarded":["Qd"]`)
— a live opponent bot watching that same seat's `place` event over the wire
would see only `"discarded":[]` with `"count":1` instead: unlike a
fantasyland board, a discard is never revealed to opponents, not even at
showdown.

- **hand 45** — a royalty-heavy scoop: seat 0 (`greedy`) makes a full house
  on both back rows at once — middle `Jc 9h Jh Js 9d` (jacks full of nines,
  royalty 12) and bottom `8c 8d Qc Qs Qh` (queens full of eights, royalty
  6), 18 royalty points on one board — while both opponents foul (each
  one's own top row outranks its own weaker middle). Seat 0 collects
  6+18=24 from each and nets +48.
- **hand 96** — seat 0 (`greedy`)'s top pair of queens (`6d Qc Qh`, royalty
  7) is QQ+, earning fantasyland: `next_fantasyland:14` (pineapple's entry
  count — one more than classic `ofc`'s 13, since a pineapple board is built
  from 5 + 4×3 = 17 dealt cards rather than 5 + 8×1 = 13). Seat 2 fouls;
  seat 0 nets +26. See hand 97.
- **hand 97** — `greedy`'s fantasyland hand, one hand later, still at seat 0
  (frozen — not rotated to seat 1 the way it would have been before the
  freeze rule). Its single 14-card turn places a no-pair top, a bare pair of
  deuces in the middle (no royalty), and a king-high diamond flush on the
  bottom (`4d 6d 8d Qd Kd`, royalty 4) — modest for fantasyland, but enough:
  seat 1 (`random`) fouls a king-high top over a jack-high middle, and seat 2
  (`filler`) fouls a top pair of jacks over an eight-high middle; each pays
  greedy 6 plus its 4 bottom royalty points (10 apiece). Greedy nets +20. A
  flush bottom isn't quads, so no stay: `next_fantasyland:null`.

## ofc-progressive — Progressive Pineapple OFC

```sh
./target/release/poker-arena run --game ofc-progressive --hands 1000 \
  --seed 2 --bot builtin:greedy --bot builtin:random --bot builtin:filler \
  --log ofc-progressive.log
```

Progressive's fantasyland entry count scales with the top row (`QQ→14,
KK→15, AA→16, any top trips→17` — see the table in
[`WIRE_PROTOCOL_OFC.md`](../WIRE_PROTOCOL_OFC.md#the-games)); hand 793 below
lands the maximum, 17, the biggest entry found scanning several seeds up to
a few thousand hands each.

- **hand 26** — a clean scoop, no foul on either side: seat 2 (`greedy`)'s
  middle trip nines (`As 9c 9s 9d Ks`, royalty 2) and bottom full house
  (`4d 4s 6d 6s 4h`, fours full of sixes, royalty 6) beat both rivals'
  boards outright; nets +28.
- **hand 793** — the richest hand in the set: seat 1 (`greedy`) makes top
  trip tens (`Ts Td Th`, royalty 18 — the top end of the rank-scaled trips
  table, `222`=10 up to `AAA`=22), middle trip queens (`Qs Ad Qh Qd 7d`,
  royalty 2), and bottom quad deuces (`2c 2h 2s 2d 6h`, royalty 10) — 30
  royalty points on one board. Both opponents foul, so seat 1 collects
  6+30=36 from each and nets +72. Top trips is progressive's maximum entry
  trigger: `next_fantasyland:17`. See hand 794.
- **hand 794** — the 17-card fantasyland hand, one hand later; `greedy` is
  frozen at seat 1, the same seat it entered from (not rotated to seat 2 the
  way the pre-freeze rule would have moved it). This is the richer half of
  the pair: greedy makes full houses on both back rows at once — eights full
  of sevens in the middle (royalty 12) and jacks full of nines on the bottom
  (royalty 6), 18 royalty points behind an ace-high top. Seat 2 (`random`)
  fouls (a middle pair of aces outranks its own bottom pair of sixes) and
  pays 6 plus greedy's 18 royalties (24); seat 0 (`filler`), clean but
  overmatched (a bare pair of deuces on both middle and bottom), loses all
  three rows outright to greedy's full houses for a further 24 (the sweep
  bonus plus the royalty difference) while still collecting 6 from random's
  foul tax. Greedy nets +48; filler nets −18; random nets −30. No stay:
  `next_fantasyland:null`.

## ofc-27 — 2-7 Pineapple OFC

```sh
./target/release/poker-arena run --game ofc-27 --hands 2100 --seed 7 \
  --bot builtin:greedy --bot builtin:random --bot builtin:filler \
  --log ofc-27.log
```

The middle row is scored `DeuceToSevenLow` instead of `High`, with a
ten-low-or-better qualifier (worst qualifier `T-9-8-7-5`); fouling is top >
bottom *or* a non-qualifying middle, checked independently — the middle has
no ordering relationship with its neighbors at all. Fantasyland entry is
`KK+` on top *or* the exact `7-5-4-3-2` wheel on the middle → 14, both at
once → 15 (a suited 7-5-4-3-2 is a flush, not a qualifying low, and doesn't
count).

- **hand 476** — seat 2 (`greedy`)'s middle is the exact wheel, `4h 7c 3d
  2c 5s` — the best possible 2-7 middle, royalty 8 — carried by an
  unremarkable top (no pair) and a two-pair bottom (jacks and eights, no
  royalty). Both opponents foul: seat 1 (`filler`) fails purely on the
  *other* route, a bare pair of sixes in the middle, independent of how its
  own top (queen-high) and bottom (queen-high) compare; seat 0 (`random`)
  fails the qualifier too (an ace makes its middle's high card too high) and
  also has top (a pair of aces) beating its own bottom outright, a second,
  independent foul reason. Either way both pay greedy 6 plus its 8 middle
  royalty (14 apiece); seat 2 nets +28. The wheel middle alone (no `KK+` top
  needed) earns fantasyland: `next_fantasyland:14`. See hand 477.
- **hand 477** — the fantasyland hand, one hand later; the freeze keeps
  `greedy` at seat 2, the same seat it entered from. Its middle qualifies
  for a modest 8-low (`2d 3c 4d 6c 8h`, royalty 2) while its bottom rivers a
  full house (`Tc Td Th Kc Kh`, tens full of kings, royalty 6). Both
  opponents foul again, each purely on the non-qualifying-middle route (seat
  0's `Kd`-high middle and seat 1's paired-sixes middle are both worse than
  their own bottoms, so top-versus-bottom never enters into it this time);
  each pays greedy 6 plus its 8 combined royalty points (14 apiece). Greedy
  nets +28. No stay (a full house bottom isn't quads, no top trips):
  `next_fantasyland:null`.
- **hand 892** — a clean 2-7 middle royalty with no foul on the winning
  side: seat 1 (`greedy`)'s middle (`3h 6h 7s 2c 5c`, high card seven) is a
  7-low, royalty 4, alongside a top pair of kings (royalty 8 — also a fresh
  `KK+` fantasyland entry, `next_fantasyland:14`, not followed further in
  this file). Both opponents foul: seat 0's top pair of sixes outranks its
  own no-pair bottom, while seat 2 fails purely on the
  non-qualifying-middle route (a bare pair of eights) — its own jack-high
  top is actually the *weaker* side of its top/bottom comparison, so that
  route never comes into play for seat 2 at all. Each pays the 6-plus-12
  tax; seat 1 nets +36.
- **hand 1451** — a *different* fantasyland hand for `greedy` (seat 0 here,
  dealt 14 cards from an entry at hand 1450, not shown in this file — the
  same seats-equal property holds between that hand and this one, just not
  printed), chosen to show a middle fouled purely on the non-qualifying-middle
  rule, this time via a straight rather than a pair: seat 2's middle (`4h 6c
  3c 5h 2c`, a made 2-to-6 straight) fails the qualifier outright — a
  straight never qualifies, regardless of how its own top (jack-high) and
  bottom (ace-high) compare, which here don't even foul on their own (top <
  bottom). For contrast, the two non-fouled boards both carry ordinary 2-7
  middle royalties: seat 0's own middle is another wheel (`2d 3d 4s 5s 7h`,
  royalty 8, alongside a broadway straight bottom worth 2), and seat 1's
  middle is a plain 9-low (`3h 8h 6s 7d 9d`, royalty 1). Seat 0 collects the
  foul tax from seat 2 and, with its wheel-plus-straight board, also sweeps
  every row against seat 1's no-pair top, 9-low middle, and one-pair bottom;
  nets +31.

