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
{"hand": 175, "ev": {"event": "multiway all-in etc...", ...}}
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
   straight flush — used below to call out "premium" showdowns).
6. `pot-awarded` shows exactly who won what, `side` distinguishing a
   `whole`-pot scoop from a `hi`/`lo` split; `pot` is a **pot index** (0 =
   main pot, 1+ = a side pot), not a chip amount.
7. `hand-end.nets` is winnings-minus-contributions per seat for that hand
   and always sums to zero.

## How to regenerate

Build the CLI once:

```sh
cargo build --release -p poker-arena-cli
```

Each game section below gives the exact command used to produce that
game's source match (same seed, same bots, same seat count — reproducible
forever per the fairness model in the top-level README). Add `--log
somewhere.log` to capture the full match, then find a specific curated hand
with e.g. `grep '"hand":175,' somewhere.log` (or `jq 'select(.hand==175)'`
if lines aren't pre-filtered). The bots (`builtin:random[:seed]`,
`builtin:shover`, `builtin:caller`) are deterministic and hand-strength-
oblivious — they don't evaluate their cards at all, so a given seed
produces the exact same sequence of actions regardless of which registered
game variant you point it at; only the showdown evaluator (and therefore
who wins) can differ between variants dealt from the same seed. A few hand
descriptions below rely on this to compare the *same* deal across sibling
games (e.g. `holdem-nl`/`holdem-fl`, or `27td-fl`/`a5td-fl`).

All twelve source matches here used `--hands 400 --seed 7 --dealing
seeded`, mixing `builtin:random` (creates raises and folds),
`builtin:shover` (creates all-ins), and `builtin:caller` (creates
showdowns) so each match samples a wide range of textures. Every claim
below (winner, hand class, pot size, discard counts, capped streets) was
checked against the actual event lines, not guessed from the bot mix.

---

## holdem-nl — No-Limit Texas Hold'em

```sh
./target/release/poker-arena run --game holdem-nl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --bot builtin:random:3 \
  --log holdem-nl.log
```

- **hand 0** — five-way all-in preflop (seat 4 re-raises to 6430, seat 0
  shoves to 10000, everyone calls); board runs out `As 5d 3s / 5c / Jc`;
  seat 2's `Ad 6d` makes two pair (aces and fives) — the best of all five
  hands shown — and scoops the 50000 pot.
- **hand 4** — a preflop raising war narrows to seat 0 vs seat 1 all-in;
  seat 0's `4c Tc` completes the wheel (A-2-3-4-5) off an `As 5h / 2c / 3c`
  board to beat seat 1's two pair (aces and treys); wins 21859.
- **hand 9** — four-way all-in preflop; the board pairs to `6c 3d 6d` on
  the flop and pairs *again* with `6s` on the turn, giving the board trip
  sixes outright; seat 0's `6h 8d` turns that into quad sixes, beating seat
  1's full house (sixes full of treys, made the same way) and two rivals'
  bare trip sixes; scoops 40100.
- **hand 175** — preflop goes four raises deep (245 → 1723 → 7688 → 10000)
  before all five seats are all-in; the board pairs twice (`Th Ts` on the
  flop, `6s 6c` on flop/turn) so everyone effectively plays the board's two
  pair — it comes down to kickers, and seat 4's ace kicker (`4s As`) beats
  four rivals holding the same two pair with a king, jack, or 9 kicker;
  scoops 50000.

## holdem-fl — Fixed-Limit Texas Hold'em

```sh
./target/release/poker-arena run --game holdem-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --bot builtin:random:3 \
  --log holdem-fl.log
```

- **hand 3** — the flop gets raised to the fixed-limit cap (four
  bets/raises: bet, raise, raise, raise) three-way; seat 2 rivers a full
  house (deuces full of nines, the board pairs both 2s and 9s) to beat
  seat 0's and seat 3's matching board-based two pair; wins 7100.
- **hand 5** — no capped street, just a clean river cooler: seat 1's
  `3h 6c` completes a 3-to-7 straight on `Kc 9d 5d 4s 7h`; seat 0 folds the
  river after a raising war, and seat 1's straight beats seat 2's pair of
  fours at showdown; wins 3400.
- **hand 9** — the *same deck* as holdem-nl's hand 9 (same seed reproduces
  identical hole cards and board across game variants): board again runs
  `6c 3d 6d / 6s / Qd` for the same quad-sixes-vs-boat cooler, but
  fixed-limit's small bet sizes and three folds keep it heads-up for a
  modest 2000 pot instead of an all-in.
- **hand 119** — the richest capped hand in the set: flop, turn, *and*
  river are each raised to the four-bet cap among the same three players;
  seat 1's `3h Js` rivers two pair (jacks and treys) off `Qd 3s 8s 5h Jh`
  to beat seat 0's and seat 4's matching pair of eights; wins 7800.

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
- **hand 3** — a plain two-way all-in (no capped street, no premium
  showdown): after a big pot-limit turn/river escalation, seat 1's rivered
  two pair (tens and sixes) holds up against seat 0's two pair (nines and
  sixes); wins 20550.
- **hand 5** — three-way all-in on the flop (`7d Jd 3d`) after a raising
  war; seat 3's `Qd...9d` makes a queen-high diamond flush (two hole
  diamonds plus three on the board) to beat seat 1's two pair and seat 2's
  one pair; scoops 30100.
- **hand 9** — the flop board `Ah Tc Jh`, completed by `Kc`/`Qc`, shows a
  four-card broadway run (T-J-Q-K) that *nobody* can actually use, because
  Omaha requires exactly two hole cards and no hole cards bridge the gap;
  three-way flop all-in resolves down to seat 1's bare pair of aces beating
  seat 3's lower pair and seat 2's high card; scoops 30000.
- **hand 52** — no all-in, just a river-heavy pot-limit escalation
  (600/1800/5400) that gets called down; seat 2's `Ad 7s 9s 4c` makes a
  ten-high spade flush to beat seat 1's rivered two pair; wins 16200.

## omaha8-pl — Pot-Limit Omaha Hi-Lo (8 or Better)

```sh
./target/release/poker-arena run --game omaha8-pl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log omaha8-pl.log
```

- **hand 0** — same three-way preflop all-in and board (`7h Ac 4s / 7c /
  Kc`) as omaha-pl's hand 0, but here the low counts: seat 2's kings-full
  boat takes the hi half (16724) while seat 0's flush hand *also* holds the
  best qualifying low (A-3-4-5-7, using 3s/5c plus the board's A-4-7) and
  takes the lo half (16723); seat 1's bigger boat (sevens full of aces)
  wins nothing.
- **hand 15** — a preflop all-in between seat 0 and seat 1 where both hold
  `A-2-3` plus a kicker; the board (`4s 3s 5c` / `8c` / `Js`) lets both
  make the *exact same* wheel (A-2-3-4-5) — simultaneously a straight and
  the nut low. Hi splits evenly (6896/6896) and lo splits evenly too
  (6896/6896): a wheel scooping its own quarter on both sides.
- **hand 19** — heads-up flop all-in; seat 1's `3h Qs 2d 9d` makes two pair
  *and* the best qualifying low, scooping both the hi (14039) and lo
  (14039) halves outright since seat 0's hand has no qualifying low at
  all; 28078 total.
- **hand 96** — four-way preflop all-in (raises to 500/1762/7548/10000);
  the board pairs fours (`4h 4d 6c`); seat 0's `Kh Qc` rivers trip fours to
  scoop the entire hi half (20000) while seat 2 and seat 3 tie and split
  the low (10000 each) — a genuine three-winner hand.
- **hand 264** — three-way flop all-in (raise war to 9600); seat 0's
  `Ad Kd` makes an ace-high diamond flush to take the hi half (15000) alone
  while the low ties three ways between seat 0, seat 1, and seat 2 (5000
  each) — one player collects both a hi share and a lo share in the same
  hand.

## omaha8-fl — Fixed-Limit Omaha Hi-Lo (8 or Better)

```sh
./target/release/poker-arena run --game omaha8-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log omaha8-fl.log
```

- **hand 0** — the fixed-limit sibling of omaha8-pl's hand 0 and
  omaha-pl's hand 0 (same deck): turn gets capped at four raises; seat 2's
  kings-full boat takes the hi side (2700), seat 0's A-3-4-5-7 low takes
  the lo side (2700).
- **hand 15** — the fixed-limit sibling of omaha8-pl's hand 15: the same
  double-wheel chop (both seat 0 and seat 1 make A-2-3-4-5, a straight
  that's also the nut low), river capped at four raises; hi splits
  1425/1425 and lo splits 1425/1425.
- **hand 67** — flop capped at four raises (100/200/300/400) heads-up into
  `Jc 7c 4d` / `Ts` / `9h`; both players make straights, but *neither*
  qualifies for an 8-or-better low, so the whole pot goes as a single
  `side: "whole"` award (not split) to seat 0's higher straight — an
  omaha8 hand that plays exactly like plain-hi Omaha when no low
  qualifies; wins 3200.
- **hand 72** — three-way, no capped street: seat 1's `Jh Jc` makes a full
  house (treys full of fours) to take the hi side alone (1400) while seat 2
  and seat 3 tie and split the low (700 each) — hi and lo go to entirely
  different sets of players.
- **hand 193** — the simplest hi-lo split in the set: heads-up, no raising
  war, no capped street; seat 3's two pair takes the hi side (1075) and
  seat 2/seat 3 tie for the low, splitting 538/537 (odd chip to seat 2).

## stud-fl — Seven-Card Stud

```sh
./target/release/poker-arena run --game stud-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log stud-fl.log
```

- **hand 1** — third street goes four bets deep (100/200/300/400) with
  three jack door-cards clashing; fifth and sixth street are *also*
  capped at four raises each; seat 2 rivers a queen-to-ace broadway
  straight (`Tc Js Qc Kh Ah`) to beat seat 3's pair of jacks and seat 1's
  ace-high; wins 7080.
- **hand 5** — seat 0's 9-door owes the bring-in and completes straight to
  a bet instead of posting the forced amount; third street caps at four
  raises; seat 3's rivered pair of kings beats seat 2's pair of fours; a
  simple one-pair pot (7080's neighbor in variance, not every capped
  street ends in a monster) — wins 3380.
- **hand 8** — seat 2's 2-door actually posts the forced bring-in (rather
  than completing); third street caps at four raises; seat 1's `3h 3s`
  rivers a full house (treys full of fours) to crush seat 3's trip tens;
  wins 6680.
- **hand 13** — the richest raising war in the stud-fl set: fourth, fifth,
  *and* sixth street are each capped at four bets/raises between the same
  three players; seat 3's `4s 5s` completes a 3-to-7 straight to beat seat
  2's two pair (aces and kings) and seat 0's ace-high; wins 7680.
- **hand 69** — a raise war (3 bets/raises third street, not quite capped)
  that seat 0 folds out of; seat 2's `Jh Qh` rivers a queen-high heart
  flush (two hole hearts plus three on later streets) to beat seat 3's two
  pair (kings and jacks); wins 3480.

## stud8-fl — Seven-Card Stud Hi-Lo (8 or Better)

```sh
./target/release/poker-arena run --game stud8-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log stud8-fl.log
```

- **hand 1** — identical door cards and action to stud-fl's hand 1 (same
  seed): third, fifth, and sixth street all cap at four raises; seat 2
  again rivers the `Tc Js Qc Kh Ah` broadway straight — but here *nobody*
  makes a qualifying low, so the pot is awarded as a single `whole` scoop
  (7080) rather than split.
- **hand 13** — the stud8 sibling of stud-fl's hand 13 (same three-street
  capped raising war, same cards): this time seat 3's 3-to-7 straight
  *also* qualifies as the best low, so seat 3 scoops both sides explicitly
  (hi 3840 + lo 3840) instead of just winning a hi-only pot.
- **hand 33** — seat 3's 2-door posts an actual forced bring-in (not a
  completion); third street caps at four raises; seat 3's `2s 4c` rivers a
  4-to-8 straight that is simultaneously the best low, scooping both sides
  (hi 2240 + lo 2240) to beat seat 2's high card.
- **hand 61** — fifth street caps at four raises three-way; seat 3's
  `6h Js` rivers an 8-to-Q straight to take the hi side (2690) while seat
  2's rough high card holds the best qualifying low and takes the lo side
  (2690) — hi and lo split cleanly between two different players.
- **hand 223** — a quiet hi-lo split with no capped street or raising war:
  heads-up, seat 1's rivered flush (`Kd Qd Jd 6d` plus the 9d hole card)
  takes the hi side (1440), seat 0 holds the only qualifying low and takes
  the lo side (1440).

## razz-fl — Razz

```sh
./target/release/poker-arena run --game razz-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log razz-fl.log
```

Razz's showdown evaluator is A-to-5 low (not High), so class-nibble
"premium" tagging doesn't apply here — every showdown below is described by
its actual low.

- **hand 0** — seat 3's exposed ace brings it in (razz forces in the
  *highest* door card, and an exposed ace counts high for that purpose
  only); third street caps at four raises; seat 2 rivers a 7-6-5-4-A
  (seven-low) to beat seat 1's 9-8-7-3-A (nine-low); wins 2730.
- **hand 1** — same three-jack-door clash as stud-fl/stud8-fl's hand 1,
  but razz: seat 1's queen door brings it in; fifth street caps at four
  raises; seat 3's J-T-7-4-3 (jack-low) narrowly beats seat 2's J-T-8-2-A —
  both jack-low, seat 3's third card (7) beats seat 2's (8); wins 3780.
- **hand 2** — the biggest razz pot in the set: seat 1's king door
  completes the bring-in straight to a bet; fourth *and* seventh street
  both cap at four raises across a four-way-then-two-way battle; seat 3
  rivers a T-8-6-5-2 (ten-low) to beat seat 0's J-7-4-3-A (jack-low);
  wins 9480.
- **hand 7** — seat 0's exposed door ace forces the bring-in, which seat 0
  completes straight to a bet; fifth street caps at four raises three-way;
  seat 0 rivers a T-8-7-2-A (ten-low) to beat seat 1's rough K-J-8-5-3
  (king-low, about as bad as a made low gets); wins 4380.
- **hand 11** — seat 0's king door brings it in and completes straight to
  a bet; a heads-up raising war on third street (100/200/300); seat 0
  rivers a J-T-9-4-2 (jack-low) to beat seat 1's K-J-5-3-A (king-low);
  wins 2280.

## 27td-fl — 2-7 Triple Draw

```sh
./target/release/poker-arena run --game 27td-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log 27td-fl.log
```

2-7 lowball: aces always count high (worst), straights/flushes count
against you, and any unpaired hand beats any paired hand regardless of
rank.

- **hand 1** — seat 1 draws one card, then discards its *entire* hand (5
  cards) on the second draw, then folds anyway to a raise; seat 2 and seat
  3 both stand pat through every draw; seat 2's rough `7-4-T-Q-J` beats
  seat 3's pair of jacks — a pair loses to any no-pair hand, however ugly;
  wins 3200.
- **hand 3** — seat 2 discards all five cards on the first draw (a total
  reshuffle) then folds; seat 3 keeps drawing across all three rounds (2,
  then 1, then 4 cards) and still folds on the last round; seat 0 and seat
  1 both stand pat the whole hand, and seat 0's king-high no-pair beats
  seat 1's ace-high no-pair (in 2-7, ace counts as the *worst* high card,
  so king-high edges ace-high); wins 7600.
- **hand 12** — seat 0 draws three, then one, then three more across all
  three rounds and still folds on the last one; seat 1 and seat 2 both
  stand pat the entire hand; seat 1's deuces-paired hand beats seat 2's
  sixes-paired hand (the lower pair wins); wins 4500.
- **hand 29** — seat 0 draws one, then three, then discards its *entire*
  final hand (5 cards) on the last draw with no more chances left to
  improve — and it pays off: seat 0's fresh pair of treys beats seat 2's
  pair of queens and seat 3's pair of kings; wins 4700.
- **hand 392** — seat 3 draws three on the first round then stands pat
  twice; seat 1 and seat 2 stand pat the entire hand. Seat 3's rough
  king-high no-pair beats seat 1's pair of treys *and* seat 2's two pair
  (fours and sixes) — any unpaired hand beats any paired hand in 2-7
  lowball, no matter how high; wins 4800.

## a5td-fl — A-5 Triple Draw

```sh
./target/release/poker-arena run --game a5td-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log a5td-fl.log
```

Same seed, same bots, same seat count as 27td-fl — and since these builtin
bots never look at their own cards, the action sequence for a given hand
number is often *identical* between the two games. The evaluator isn't:
A-5 lowball treats aces as low (the opposite of 2-7). Hands 1, 12, and 29
below reach the same winner as their 27td-fl counterparts; hand 3 is the
one where the ace-low/ace-high difference actually flips the result.

- **hand 1** — mirrors 27td-fl's hand 1 move for move: seat 2's pat
  `7-4-T-Q-J` again beats seat 3's pair of jacks; a pair still loses to any
  no-pair hand under A-5 rules; wins 3200.
- **hand 3** — the flagship hand for this file: identical deal and action
  to 27td-fl's hand 3, but the winner *flips*. Seat 1 stands pat with
  `3h Th 2h Ks Ac` and seat 0 stands pat with `Kd Td 5d 6c Jc`. In 27td-fl
  the ace in seat 1's hand counts high, so seat 1's hand reads
  A-K-T-3-2 and loses to seat 0's K-J-T-6-5. In a5td-fl the same ace counts
  *low*, so seat 1's hand reads K-T-3-2-A — and a king-ten beats seat 0's
  king-jack on the second card — flipping the winner to seat 1; wins 7600.
- **hand 12** — mirrors 27td-fl's hand 12: seat 1's pair of deuces again
  beats seat 2's pair of sixes (lower pair still wins); wins 4500.
- **hand 29** — mirrors 27td-fl's hand 29: seat 0's total final-draw
  reshuffle again lands a pair of treys that beats seat 2's queens and
  seat 3's kings; wins 4700.
- **hand 154** — not in the 27td-fl set: seat 1 draws three fresh cards on
  *every one* of the three draw rounds (nine replacement cards total off a
  five-card hand) chasing a low, and rivers a pair of aces; draw1 and
  draw2 both cap at four raises three-way. Because A-5 lowball treats aces
  as the lowest rank, a pair of aces is the best (least-bad) pair
  possible: it beats seat 0's pair of tens and seat 3's pair of kings;
  wins 7000.

## badugi-fl — Badugi

```sh
./target/release/poker-arena run --game badugi-fl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log badugi-fl.log
```

Badugi hands are ranked by how many of the four cards form a "badugi"
(distinct ranks *and* distinct suits) — 4 beats 3 beats 2 beats 1 card —
then by the low value of the cards actually used.

- **hand 1** — seat 1 draws two, then three more, then folds to a raise;
  seat 2 stands pat the whole hand with a three-card badugi (`3c 4s Td` —
  the second club, `7c`, is dead weight) and beats seat 3's two-card
  badugi (`Qs` plus one of three same-rank jacks); wins 3300.
- **hand 2** — seat 2 discards its *entire* four-card hand on draw1, then
  discards all four again on draw2 (two consecutive total resets) before
  folding on draw3; seat 3 and seat 0 both stand pat with king-high
  three-card badugis (`Kc 5s 4d` vs `Kd Ah 6c`), and seat 3's lower second
  card (5 vs 6) edges it out; wins 5800.
- **hand 3** — seat 3 discards all four cards on draw1 (total reset) then
  stands pat twice; seat 1 and seat 0 both stand pat the entire hand.
  Seat 0's original deal (`9h 2c 9s Kd`, the two nines conflicting) reduces
  to a three-card `K-9-2` badugi that beats seat 3's reset two-card `6-5`
  and seat 1's two-card badugi (three hearts in its original four cut it
  down to two cards); wins 3100.
- **hand 29** — the biggest pot in the set: four-way action through all
  three draw rounds with heavy churn (seat 0 discards 2, then 4 — a
  near-total reset, then 1 more; seat 1 discards 2, then 3). At showdown
  seat 0's `9-3-2` is the only three-card badugi at the table, beating
  three separate two-card badugis (seat 1's, seat 2's — reduced from three
  same-rank queens — and seat 3's, reduced from two same-rank kings plus
  two same-suit hearts); wins 7600.
- **hand 129** — seat 1 draws just one card then folds to a raise; seat 0
  discards its *entire* four-card hand on draw2 (total reset), then draws
  one more on draw3; the resulting `Q-J-6` three-card badugi beats seat 2's
  two-card `3-A` and seat 3's two-card `4-2` (three clubs collide in seat
  3's hand); wins 3500.

## 5cd-nl — No-Limit Five-Card Draw

```sh
./target/release/poker-arena run --game 5cd-nl --hands 400 --seed 7 \
  --dealing seeded --bot builtin:random --bot builtin:shover \
  --bot builtin:caller --bot builtin:random:9 --log 5cd-nl.log
```

- **hand 2** — four-way all-in preflop (everyone calls seat 3's shove);
  seat 1 discards all five cards on the single draw street and rivers a
  pair of tens to beat three high-card hands, including seat 3's and seat
  0's stand-pat holdings (ace-high and queen-high, respectively); wins the
  full 40000 pot.
- **hand 10** — the mirror image of hand 2: seat 3 shoves preflop, seat 1
  folds, seat 0 and seat 2 call all-in; seat 3 stands pat with a pair of
  eights already in the original five cards, while seat 2 discards all
  five and whiffs to high card, and seat 0 (also standing pat) only has
  high card; seat 3's dealt pair holds up; wins 30050.
- **hand 145** — the standout hand of the file: four-way all-in preflop;
  on the draw, seat 0 discards its *entire* hand (all five cards) and
  comes back with `3d 5d Ad 9d 2d` — a flush drawn completely from
  scratch — to beat seat 2's stand-pat pair of queens and two high-card
  hands; scoops the full 40000 pot.
- **hand 198** — four-way all-in preflop; seat 0 is dealt a full house
  straight off the deal (`Jc Qc Js Qh Qs`, queens full of jacks) and
  understandably stands pat, while seat 2 discards all five cards on the
  draw and still only manages high card; seat 0's pat boat scoops the
  entire 40000 pot untouched.
- **hand 201** — three-way all-in preflop (seat 1 folds); nobody draws on
  the single draw street at all — every remaining player stands pat; seat
  2's dealt straight (`2d 3s 4d 6d 5h`) needs no improvement and beats two
  high-card hands; wins 30100.
