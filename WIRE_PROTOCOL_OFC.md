# poker-arena OFC wire protocol (v1)

This document specifies the wire protocol bots use to play Open Face
Chinese (OFC) in poker-arena (the `poker-arena` binary's OFC games). It is
a **separate protocol** from the betting protocol in `WIRE_PROTOCOL.md` —
an OFC hand has no chips, no betting, and no legal-actions surface, only
placement decisions — but it shares the same transports, the same JSON-lines
framing, and the same card vocabulary. The Rust reference implementation
lives in `crates/poker-wire/src/ofc/`; this document and that module must
never drift apart — if you change one, change the other.

`ofc::PROTO_VERSION = 1` (versioned independently of the betting
protocol). **Unknown JSON fields must be ignored by bots, and unknown `"t"`
(or event `"event"`) values must be skipped/ignored rather than treated as
errors** — the same forward-compatibility rule as the betting protocol.

## Transport and framing

Identical to the betting protocol; see `WIRE_PROTOCOL.md`:

- TCP (`--bot tcp:PORT`) or spawned subprocess over stdio
  (`--bot cmd:"..."`); stderr of a subprocess is free for logging.
- JSON Lines, compact, one object per `\n`-terminated line, 64 KiB max.
- Unknown fields and unknown message/event types are ignored, never errors.

## The games

`game_id` in `hello` names one of the four registry variants. A bot is
expected to know the rules from the id; the authoritative rules contract is
the module documentation of `crates/poker-core/src/ofc/state.rs`. In brief:
every seat fills a board of three rows — top (3 cards), middle (5), bottom
(5) — and hands score in **points**, pairwise between all seats: one point
per row won, three more for winning all three, plus royalties; a board
whose rows are out of order (top > middle or middle > bottom; for `ofc-27`,
top > bottom or a middle that is not a ten-low-or-better 2-7 hand) *fouls*
and pays six plus royalties to every non-fouled opponent.

| id | seats | structure | middle row | fantasyland |
|---|---|---|---|---|
| `ofc` | 2–4 | deal 5, then 8 × (deal 1, place 1) | high | QQ+ top → 13 cards |
| `ofc-pineapple` | 2–3 | deal 5, then 4 × (deal 3, place 2, discard 1) | high | QQ+ top → 14 |
| `ofc-progressive` | 2–3 | pineapple structure | high | QQ→14, KK→15, AA→16, trips→17 |
| `ofc-27` | 2–3 | pineapple structure | 2-7 low, ten-low qualifier | KK+ top or 7-5-4-3-2 middle → 14; both → 15 |

### Fantasyland

Fantasyland is a property of the *next* hand: the qualifying seat is dealt
all its cards at once, places 13 with its board hidden until showdown, and
discards the rest. It never changes how many hands a match plays. Two
preconditions govern everything below: a **fouled board never qualifies**,
and **entry and stay are exclusive** — a seat not in fantasyland can only
*enter*, a seat in fantasyland can only *stay* (making QQ+ on top while in
fantasyland does not re-enter).

**Entry** (top row unless noted):

| game | condition | cards dealt |
|---|---|---|
| `ofc` | pair QQ+ or any trips | 13 |
| `ofc-pineapple` | pair QQ+ or any trips | 14 |
| `ofc-progressive` | pair QQ / KK / AA / any trips | 14 / 15 / 16 / 17 |
| `ofc-27` | pair KK+ or trips, **or** exactly 7-5-4-3-2 middle | 14; both at once → 15 |

An `ofc-27` middle of 7-5-4-3-2 in one suit does not count: that hand is a
flush, which is no 2-7 low at all — it fails the ten-low qualifier and
fouls the board.

**Stay** (repeating from a fantasyland hand; always grants the variant's
base count, however strong the qualifying row):

| game | condition | cards dealt |
|---|---|---|
| `ofc` | top trips, or middle full house+, or bottom quads+ | 13 |
| `ofc-pineapple` | top trips or bottom quads+ | 14 |
| `ofc-progressive` | top trips or bottom quads+ (the entry ladder does not apply to stays) | 14 |
| `ofc-27` | top trips or bottom quads+ | 14 |

The earned count is announced in the seat's `showdown` event as
`next_fantasyland`, and again at the top of the next hand as its
`fantasyland` event.

## Handshake

Identical in shape to the betting protocol: the arena sends `hello`, the
bot answers `join {}` (bots carry no identity — names are
operator-assigned via `--bot NAME@spec`), and once every seat has
connected the arena sends each bot its `joined {name}` acknowledgment.
From then on the arena drives: `hand-start`, `event`s, `act` (answered
with `action`), `hand-end`, and finally `match-end`.

## Messages: arena → bot (`OfcArenaMsg`)

All arena→bot messages are JSON objects tagged on `"t"`.

### `hello`

```json
{"t":"hello","proto":1,"game_id":"ofc-pineapple","seat_count":3,"timeout_ms":5000}
```

- `proto` — OFC protocol version, currently `1`.
- `game_id` — registry id (see table above). Everything about the rules is
  derivable from it; `hello` carries only the per-match parameters that are
  not.
- `seat_count` — seats in the match (2..=4 for `ofc`, 2..=3 otherwise).
- `timeout_ms` — the per-decision deadline the arena enforces, or `null`
  for none.

### `joined`

```json
{"t":"joined","name":"greedy-2"}
```

The arena-assigned competition name (duplicates across the field get `-2`,
`-3`… suffixes). Purely informational; it is the name in all match records.

### `hand-start`

```json
{"t":"hand-start","hand_no":12,"seat":2}
```

`seat` is where this bot sits *this hand*; bots rotate seats between hands
for positional fairness — except into a hand where any seat is in
fantasyland: such a hand is an extension of the hand that earned the
fantasyland, so **everyone keeps their previous seat** until no seat is in
fantasyland, and the rotation then resumes (hand numbering continues
regardless). Seat 0 is always the button. Turn order for everything in a
hand is "table order": seat 1, 2, …, n−1, 0 — the button acts last.
Whether this bot (or anyone) is in fantasyland arrives in the event
stream, not here: one public `fantasyland {seat, cards}` event per
fantasyland seat opens the hand, before any deal.

### `event`

```json
{"t":"event","hand_no":12,"ev":{"event":"deal","seat":2,"cards":["As","Kd","2c"],"count":3}}
```

An observable event, **already redacted for this bot's seat** (see Events
below). The event stream is the single source of truth: deals, placements,
fantasyland status, showdown boards and values, and per-seat scores all
travel here and only here.

### `act`

```json
{"t":"act","hand_no":12,"seat":2,"decision":{"kind":"place","place":2,"discard":1},"deadline_ms":5000}
```

It is this bot's turn; reply with an `action`. The one decision kind today
is `place`: put exactly `place` of the just-dealt cards (the most recent
`deal` event for this seat) on your board and discard exactly `discard` of
them. `deadline_ms` is echoed so bots can self-limit; the arena enforces
the real deadline server-side regardless.

Deliberately carries no table state: your dealt cards, your board, and
every opponent's visible board are all reconstructible from the events
already delivered.

Every turn has this shape. The opening turn is `place: 5, discard: 0`; a
classic-OFC round turn is `place: 1, discard: 0`; a pineapple round turn is
`place: 2, discard: 1`; a fantasyland turn is `place: 13` with `discard`
making up the rest of the fantasyland deal.

### `hand-end`

```json
{"t":"hand-end","hand_no":12,"points":[4,-9,5]}
```

Net points by seat for the hand (this bot's own result is
`points[seat]` from its `hand-start`). Always sums to zero.

### `match-end`

```json
{"t":"match-end"}
```

No further messages follow; the bot should exit (a subprocess that
lingers is killed).

### Unknown

Any other `"t"` must be ignored, not treated as an error.

## Messages: bot → arena (`OfcBotMsg`)

### `join`

```json
{"t":"join"}
```

The ready signal answering `hello`. Carries nothing.

### `action`

```json
{"t":"action","action":{"placements":[{"card":"As","row":"bottom"},{"card":"Kd","row":"top"}],"discards":["2c"]}}
```

The answer to an `act`. `placements` assigns each placed card to a row
(`"top"` / `"middle"` / `"bottom"`); `discards` lists the rest of the
just-dealt cards. Legality (checked by the arena, never by the bot):

- exactly `place` placements and `discard` discards, all drawn from the
  just-dealt cards, each card used exactly once;
- every placed card's row must have free capacity (top 3, middle 5,
  bottom 5), counting the other placements in the same action.

The arithmetic of every variant guarantees a legal option always exists. A
non-conforming action is a **fault** (see Fault rules).

## Events (`OfcEvent`)

Events are JSON objects tagged on `"event"`. Redaction is per-seat: the
arena sends each bot the event as that bot's seat may see it.

- `{"event":"fantasyland","seat":1,"cards":14}` — seat 1 plays this hand
  in fantasyland and will be dealt 14 cards. Emitted at hand start, before
  any deal. Public.
- `{"event":"deal","seat":1,"cards":["As","Kd","2c"],"count":3}` — cards
  dealt to a seat. `cards` is private to that seat: other seats see
  `"cards":[]` with `count` intact.
- `{"event":"place","seat":1,"placements":[{"card":"As","row":"bottom"}],"discarded":["2c"],"count":1}`
  — a placement turn's result. `placements` is public (boards are open
  face) *except* when the placing seat is in fantasyland, in which case
  other seats see `"placements":[]` until showdown. `discarded` is always
  private to the placing seat (others see `[]`); `count` — the discard
  count — is always public.
- `{"event":"showdown","seat":1,"top":[...3 cards],"middle":[...5],"bottom":[...5],"top_value":1093632,"middle_value":5309834,"bottom_value":68976,"royalties":{"top":9,"middle":0,"bottom":4},"fouled":false,"next_fantasyland":14}`
  — the full board revealed (for a fantasyland seat, this is the reveal),
  with all three row values, raw per-row royalties (reported even for a
  fouled board; scoring voids them), the foul flag, and the fantasyland
  count this seat earned for the next hand (`null` if none). One per seat,
  in table order. Values are the engine's `HandValue` encodings — opaque
  u32s where greater is better within a row; `ofc-27` middle values use
  the 2-7 encoding.
- `{"event":"score","seat":1,"points":-7}` — the seat's net points for the
  hand. One per seat, in table order; they sum to zero.

## Deadline semantics

Same as the betting protocol: the arena starts the clock when it sends
`act`; an answer arriving after `timeout_ms` is a fault. A late answer to
a timed-out request is drained and discarded — it will not desync later
turns.

## Fault rules

A fault is: a malformed or illegal `action`, a timeout, a disconnect, or
protocol garbage. Faults are always counted and reported per bot. The
operator picks the policy (`--fault-policy`):

- `substitute` (default): the arena plays the deterministic filler
  placement on the bot's behalf — sort the just-dealt cards ascending by
  canonical index, take the first `place`, drop each into bottom if it has
  space, else middle, else top, discard the rest — and the match continues.
- `forfeit`: the match ends immediately; the offender forfeits.

## Match result & progress documents

Mirroring the betting protocol's report shapes (`WIRE_PROTOCOL.md`), with
points replacing chips; Rust types in `crates/poker-wire/src/ofc/report.rs`
(`schema_version` = 1, bumped on any breaking change).

### Match report (`--output json`, stdout, once)

```json
{"schema_version":1,"family":"ofc","game_id":"ofc","hands":1000,"seed":7,
 "seat_count":2,"timeout_ms":5000,"fault_policy":"substitute","forfeited_by":null,
 "bots":[{"name":"greedy","kind":"builtin:greedy","points":412,
   "points_per_hand_mean":0.412,"points_per_hand_ci95":0.53,"hands":1000,
   "fouls":31,"fantasylands":48,"scoops":112,"royalties":690,"faults":0,
   "decisions":{"count":4126,"mean_ms":0.6,"p50_ms":0.5,
     "p90_ms":1.1,"p99_ms":4.8,"max_ms":9.4}},
  ...]}
```

`family` is always `"ofc"` here — the one CLI emits the betting report
shape (`"family":"betting"`, specified in `WIRE_PROTOCOL.md`) for betting
games, and this field lets a consumer dispatch between the two schemas.
Points are the canonical unit (there is nothing to normalize by).
`points_per_hand_ci95` is the two-sided 95% Student-t half-width of the
mean, `null` under two observations. `fantasylands` counts hands *played
in* fantasyland; `scoops` counts opponents scooped (all three rows won
outright, or the opponent fouled while this bot did not), summed across
hands; `royalties` is total royalty points earned (fouled hands earn none).

`decisions`: wall-clock timing of this bot's decisions (`place` calls) —
the same object, under the same key, as the betting report's (see
`WIRE_PROTOCOL.md` for the full field semantics): `count` covers every
call whether it returned a placement or faulted, `mean_ms`/`max_ms` are
exact, `p50_ms`/`p90_ms`/`p99_ms` come from a fixed-size log-scaled
histogram (≈ ±4.5% relative error), all `*_ms` fields are `null` when the
bot never decided, and **the `*_ms` fields are the only ones in this
report that are not reproducible from `seed`**.

### Progress lines (`--progress-json`, stderr, repeating)

```json
{"schema_version":1,"hands_done":400,"hands_total":1000,"standings":[
  {"name":"greedy","points":180,"points_per_hand_mean":0.45,"points_per_hand_ci95":0.81},
  ...]}
```

## Example transcript

A complete heads-up `ofc-pineapple` session, **captured verbatim from a
real match** (one hand, seed 113, `builtin:greedy` at seat 0, a wire bot
playing the filler strategy at seat 1), from the wire bot's point of view.
`<` marks arena→bot lines, `>` bot→arena. The capture recipe lives in
`transcripts/README.md` ("`WIRE_PROTOCOL_OFC.md`'s example transcript,
captured").

```text
< {"t":"hello","proto":1,"game_id":"ofc-pineapple","seat_count":2,"timeout_ms":5000}
> {"t":"join"}
< {"t":"joined","name":"bot-2"}
< {"t":"hand-start","hand_no":0,"seat":1}
< {"t":"event","hand_no":0,"ev":{"event":"deal","seat":1,"cards":["2c","6s","6h","2h","6c"],"count":5}}
< {"t":"event","hand_no":0,"ev":{"event":"deal","seat":0,"cards":[],"count":5}}
< {"t":"act","hand_no":0,"seat":1,"decision":{"kind":"place","place":5,"discard":0},"deadline_ms":5000}
> {"t":"action","action":{"placements":[{"card":"2c","row":"bottom"},{"card":"2h","row":"bottom"},{"card":"6c","row":"bottom"},{"card":"6h","row":"bottom"},{"card":"6s","row":"bottom"}],"discards":[]}}
< {"t":"event","hand_no":0,"ev":{"event":"place","seat":1,"placements":[{"card":"2c","row":"bottom"},{"card":"2h","row":"bottom"},{"card":"6c","row":"bottom"},{"card":"6h","row":"bottom"},{"card":"6s","row":"bottom"}],"discarded":[],"count":0}}
< {"t":"event","hand_no":0,"ev":{"event":"place","seat":0,"placements":[{"card":"4s","row":"middle"},{"card":"5d","row":"top"},{"card":"Tc","row":"top"},{"card":"Kd","row":"middle"},{"card":"Ad","row":"bottom"}],"discarded":[],"count":0}}
< {"t":"event","hand_no":0,"ev":{"event":"deal","seat":1,"cards":["3h","9s","5c"],"count":3}}
< {"t":"act","hand_no":0,"seat":1,"decision":{"kind":"place","place":2,"discard":1},"deadline_ms":5000}
> {"t":"action","action":{"placements":[{"card":"3h","row":"middle"},{"card":"5c","row":"middle"}],"discards":["9s"]}}
< {"t":"event","hand_no":0,"ev":{"event":"place","seat":1,"placements":[{"card":"3h","row":"middle"},{"card":"5c","row":"middle"}],"discarded":["9s"],"count":1}}
< {"t":"event","hand_no":0,"ev":{"event":"deal","seat":0,"cards":[],"count":3}}
< {"t":"event","hand_no":0,"ev":{"event":"place","seat":0,"placements":[{"card":"Ts","row":"middle"},{"card":"Kc","row":"bottom"}],"discarded":[],"count":1}}
< {"t":"event","hand_no":0,"ev":{"event":"deal","seat":1,"cards":["As","Qc","7c"],"count":3}}
< {"t":"act","hand_no":0,"seat":1,"decision":{"kind":"place","place":2,"discard":1},"deadline_ms":5000}
> {"t":"action","action":{"placements":[{"card":"7c","row":"middle"},{"card":"Qc","row":"middle"}],"discards":["As"]}}
< {"t":"event","hand_no":0,"ev":{"event":"place","seat":1,"placements":[{"card":"7c","row":"middle"},{"card":"Qc","row":"middle"}],"discarded":["As"],"count":1}}
< {"t":"event","hand_no":0,"ev":{"event":"deal","seat":0,"cards":[],"count":3}}
< {"t":"event","hand_no":0,"ev":{"event":"place","seat":0,"placements":[{"card":"Js","row":"top"},{"card":"Ah","row":"bottom"}],"discarded":[],"count":1}}
< {"t":"event","hand_no":0,"ev":{"event":"deal","seat":1,"cards":["3s","8c","5s"],"count":3}}
< {"t":"act","hand_no":0,"seat":1,"decision":{"kind":"place","place":2,"discard":1},"deadline_ms":5000}
> {"t":"action","action":{"placements":[{"card":"3s","row":"middle"},{"card":"5s","row":"top"}],"discards":["8c"]}}
< {"t":"event","hand_no":0,"ev":{"event":"place","seat":1,"placements":[{"card":"3s","row":"middle"},{"card":"5s","row":"top"}],"discarded":["8c"],"count":1}}
< {"t":"event","hand_no":0,"ev":{"event":"deal","seat":0,"cards":[],"count":3}}
< {"t":"event","hand_no":0,"ev":{"event":"place","seat":0,"placements":[{"card":"8h","row":"bottom"},{"card":"Th","row":"middle"}],"discarded":[],"count":1}}
< {"t":"event","hand_no":0,"ev":{"event":"deal","seat":1,"cards":["Qd","Kh","Ks"],"count":3}}
< {"t":"act","hand_no":0,"seat":1,"decision":{"kind":"place","place":2,"discard":1},"deadline_ms":5000}
> {"t":"action","action":{"placements":[{"card":"Qd","row":"top"},{"card":"Kh","row":"top"}],"discards":["Ks"]}}
< {"t":"event","hand_no":0,"ev":{"event":"place","seat":1,"placements":[{"card":"Qd","row":"top"},{"card":"Kh","row":"top"}],"discarded":["Ks"],"count":1}}
< {"t":"event","hand_no":0,"ev":{"event":"deal","seat":0,"cards":[],"count":3}}
< {"t":"event","hand_no":0,"ev":{"event":"place","seat":0,"placements":[{"card":"4c","row":"bottom"},{"card":"Qs","row":"middle"}],"discarded":[],"count":1}}
< {"t":"event","hand_no":0,"ev":{"event":"showdown","seat":1,"top":["5s","Qd","Kh"],"middle":["3h","5c","7c","Qc","3s"],"bottom":["2c","2h","6c","6h","6s"],"top_value":762624,"middle_value":1156400,"bottom_value":6553600,"royalties":{"top":0,"middle":0,"bottom":6},"fouled":false,"next_fantasyland":null}}
< {"t":"event","hand_no":0,"ev":{"event":"showdown","seat":0,"top":["5d","Tc","Js"],"middle":["4s","Kd","Ts","Th","Qs"],"bottom":["Ad","Kc","Ah","8h","4c"],"top_value":623360,"middle_value":1620512,"bottom_value":1881632,"royalties":{"top":0,"middle":0,"bottom":0},"fouled":false,"next_fantasyland":null}}
< {"t":"event","hand_no":0,"ev":{"event":"score","seat":1,"points":7}}
< {"t":"event","hand_no":0,"ev":{"event":"score","seat":0,"points":-7}}
< {"t":"hand-end","hand_no":0,"points":[-7,7]}
< {"t":"match-end"}
```

Read it with the rules above: the two `deal` events before the first `act`
are the opening fives (the opponent's redacted to `"cards":[]`), each
round deals this seat three cards and asks for `place: 2, discard: 1`
(discards stay hidden — the opponent's `place` events show `"discarded":[]`
with `"count":1`), and the hand closes with both boards revealed in
`showdown` events, the zero-sum `score` pair, and `hand-end`. This hand
shows royalty scoring end to end: seat 1's bottom full house (sixes over
deuces, `"royalties":{"bottom":6}`) plus winning top and bottom while
losing the middle nets `1 − 1 + 1 + 6 = +7`.

## Reference clients

- `examples/ofc_bot.py` — dependency-free Python 3 client playing the
  filler strategy over stdio; the executable form of this document.
- `crates/poker-arena/src/bin/wire-placer.rs` — the Rust equivalent, used
  by the end-to-end tests. Zero faults over a full match is the bar for
  both.
