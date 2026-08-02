# poker-arena wire protocol (v1)

This document specifies the wire protocol bots use to play the *betting*
games (hold'em through drawmaha) in poker-arena. It is transport- and
language-agnostic: anything that can read/write lines of JSON over a stream
can be a bot. The Rust reference implementation lives in
`crates/poker-wire` (`message.rs` and `framing.rs` for the envelope,
`card.rs` / `action.rs` / `event.rs` / `value.rs` / `game.rs` for the
payload types); this document and that crate must never drift apart — if you
change one, change the other.

The Open Face Chinese games speak a **separate protocol** — same
transports, same framing, same card vocabulary, different messages (no
chips, no betting, placement decisions instead) — specified in
[WIRE_PROTOCOL_OFC.md](WIRE_PROTOCOL_OFC.md). A bot speaks exactly one of
the two; read the spec for the game family you are entering.

`PROTO_VERSION = 1`. **Unknown JSON fields must be ignored by bots, and
unknown `"t"` (or event `"event"`) values must be skipped/ignored rather
than treated as errors.** This is how the protocol stays forward-compatible:
a newer arena can add fields or message types without breaking older bots.

## Transport options

A bot connects to the arena one of two ways; the message protocol is
identical either way.

1. **TCP.** The arena listens on a configured host:port; the bot connects
   and the same JSON-lines protocol runs over the socket.
2. **Spawned subprocess (stdio).** The arena spawns the bot as a child
   process; the bot reads arena→bot messages from its stdin and writes
   bot→arena messages to its stdout. The bot's stderr is free for logging
   and is not part of the protocol.

## Framing

- The stream is **JSON Lines**: exactly one JSON object per line, each line
  terminated by `\n`.
- Each line is **compact** (no pretty-printing) — don't rely on any specific
  whitespace, but don't add embedded newlines inside a message either.
- **Maximum line length is 65536 bytes (64 KiB).** A line longer than that
  is a protocol violation; a well-behaved peer disconnects rather than try
  to recover mid-stream.
- Blank lines are ignored by readers and should not be emitted by writers.
- Unknown fields inside an otherwise-recognized message are ignored.
- A message with an unrecognized `"t"` (or, inside an event payload, an
  unrecognized `"event"`) must not cause an error — treat it as a no-op and
  keep reading.

## Handshake

1. On connect, the arena sends `hello` (protocol version, the game id, the
   per-match parameters that can't be derived from that id, seat count,
   starting stack, and the per-action timeout it will enforce).
2. The bot replies with `join` — a bare ready signal carrying nothing.
   Bots have no naming concept: identity is operator-assigned (the CLI's
   `--bot NAME@spec`); a legacy `name` field in `join` is ignored like any
   unknown field.
3. Once **every** seat has connected, the arena sends each bot a `joined`
   message announcing its operator-assigned competition name (duplicates
   across the field get `-2`, `-3`… suffixes). Purely informational — use
   it to label your own logs; it is the name appearing in all match
   records. Because names are assigned field-wide, there may be a delay
   between your `join` and the `joined` while other bots connect.
4. From then on, the arena drives the conversation: `hand-start` at the top
   of each hand, `event` for everything observable, `act` when it's this
   bot's turn (which the bot answers with `action`), `hand-end` at the
   bottom of each hand, and finally `match-end` when the match is over and
   no further messages will be sent.

## Messages: arena → bot (`ArenaMsg`)

Every message is a JSON object tagged by its `"t"` field, kebab-case.

### `hello`

Sent once, immediately after connection.

> **Design decision: "just game id is fine — the bot is expected to know
> the game."** `hello` does not describe the game's rules (streets, deck,
> hand ranking, deal shape) — a bot is expected to already know how its
> named game is played, the same way a human sitting down at a table named
> "Seven-Card Stud" is expected to know stud rules going in. `hello` carries
> only the per-match *parameters* that cannot be derived from the id alone:
> the actual stakes, the betting structure, table size, stack depth, and
> the timing the arena will enforce.

| field           | type              | meaning                                   |
|-----------------|-------------------|--------------------------------------------|
| `proto`         | u32               | Protocol version (currently `1`).          |
| `game_id`       | string            | Registry id, e.g. `"holdem-nl"`, `"drawmaha-27-fl"`. Bots are expected to know the named game's rules from its id. |
| `stakes`        | `Stakes`          | The actual per-match stakes (see below).   |
| `betting`       | `BettingKind`     | The betting structure (see below).         |
| `seat_count`    | usize             | Number of seats at the table.              |
| `starting_stack`| u64               | Chips every seat starts each hand with.    |
| `timeout_ms`    | u64 or `null`     | Per-action deadline the arena enforces, or `null` for none. |

`stakes` is tagged on `kind` and has two shapes, depending on the game
family:

- Blind games (hold'em, Omaha, draw): `{ kind: "blinds", small_blind: u64,
  big_blind: u64, ante: u64 }` — `ante` is the per-player ante posted
  before the deal, `0` when the game is played without one. Antes join the
  pot but never count toward street commitments.
- Stud games: `{ kind: "stud", ante: u64, bring_in: u64, small_bet: u64,
  big_bet: u64 }`.

`betting` is tagged on `kind` and has three shapes:

- `{ kind: "no-limit" }`.
- `{ kind: "pot-limit" }`.
- `{ kind: "fixed-limit", raise_cap: u8 or null }` — `raise_cap` is the
  maximum number of wagers (the opening bet/blind counts as the first) a
  fixed-limit betting round allows; `null` means uncapped. Fixed-limit bots
  need this up front to plan a street: without the cap, a bot can't tell
  how many more raises are legal before `raise` in `act` stops being
  offered.

```json
{"t":"hello","proto":1,"game_id":"holdem-nl","stakes":{"kind":"blinds","small_blind":50,"big_blind":100,"ante":0},"betting":{"kind":"no-limit"},"seat_count":2,"starting_stack":10000,"timeout_ms":5000}
```

```json
{"t":"hello","proto":1,"game_id":"stud-fl","stakes":{"kind":"stud","ante":20,"bring_in":50,"small_bet":100,"big_bet":200},"betting":{"kind":"fixed-limit","raise_cap":4},"seat_count":2,"starting_stack":10000,"timeout_ms":5000}
```

### `joined`

Handshake acknowledgment; sent once, after every seat has connected.

| field  | type   | meaning                                  |
|--------|--------|-------------------------------------------|
| `name` | string | This bot's final, arena-assigned name.    |

```json
{"t":"joined","name":"caller-2"}
```

### `hand-start`

Sent at the beginning of every hand: "a new hand is starting, and you sit
at `seat`". Everything else about the hand — stacks, deals, the button —
arrives in the event stream that follows.

**The button is always seat 0.** The arena rotates *bots* through seats
between hands (that is the variance-reduction mechanism), never the button,
so a seat number is also a position: seat 0 is the button, seat 1 the small
blind, seat 2 the big blind, and so on — except heads-up, where standard
rules apply: seat 0 (the button) posts the small blind and seat 1 the big
blind. Expect your `seat` to change from hand to hand.

| field     | type  | meaning                             |
|-----------|-------|--------------------------------------|
| `hand_no` | u64   | 1-based hand counter for the match.  |
| `seat`    | usize | This bot's seat *for this hand*.     |

```json
{"t":"hand-start","hand_no":1,"seat":0}
```

### `event`

Wraps one observable [`Event`](#events-event) (see below) with the
hand it belongs to. Events are the single source of truth for everything
that happens in a hand; there is one `event` message per occurrence, in
order.

| field     | type        | meaning                        |
|-----------|-------------|----------------------------------|
| `hand_no` | u64         | Which hand this event belongs to. |
| `ev`      | `Event`     | The event payload (see below).  |

```json
{"t":"event","hand_no":1,"ev":{"event":"post","seat":0,"kind":"small-blind","amount":50,"all_in":false}}
```

### `act`

Sent when it is this bot's turn. Reply with a `BotMsg::action` message
conforming to `decision` (see [Action semantics](#action-semantics)).

`act` deliberately carries **no table state**. The event stream is the
single source of truth: your hole cards arrive in `deal-hole` /
`draw-result` events, the board in `deal-community`, upcards in `deal-up`,
and every wager in `post` / `acted` events — reconstruct whatever your
strategy needs from those. `decision` is included (and only `decision`)
because legality must remain arena-authoritative; bots must never derive it
themselves.

| field         | type           | meaning                                          |
|---------------|----------------|--------------------------------------------------|
| `hand_no`     | u64            | Current hand.                                    |
| `seat`        | usize          | This bot's seat (redundant but explicit).        |
| `decision`    | `WireDecision` | What kind of decision this is, and what's legal (see below). |
| `deadline_ms` | u64 or `null`  | Echo of the enforced deadline for this decision. |

`Card` (used throughout the events) is a 2-character string, rank then
suit: `"As"`, `"Td"`, `"2c"`.

`decision` is tagged on `kind`, kebab-case, and is exactly one of three
self-describing shapes — a bot switches on `kind` instead of probing a bag
of possibly-absent fields:

- **`wager`** — an ordinary betting decision:

  ```json
  {"t":"act","hand_no":1,"seat":0,"decision":{"kind":"wager","fold":true,"check":false,"call":100,"raise":{"min_to":300,"max_to":10000}},"deadline_ms":5000}
  ```

- **`draw`** — a draw-street decision (see [Draw decisions](#draw-decisions)):

  ```json
  {"t":"act","hand_no":4,"seat":1,"decision":{"kind":"draw","max_discards":3},"deadline_ms":5000}
  ```

- **`bring-in`** — the stud bring-in decision (see
  [Stud bring-in decisions](#stud-bring-in-decisions)):

  ```json
  {"t":"act","hand_no":9,"seat":2,"decision":{"kind":"bring-in","bring_in":10,"complete":{"min_to":20,"max_to":20}},"deadline_ms":5000}
  ```

### `hand-end`

Terminal event for a hand.

| field     | type     | meaning                                                    |
|-----------|----------|---------------------------------------------------------------|
| `hand_no` | u64      | Which hand ended.                                           |
| `nets`    | `[i64]`  | Net result by seat (winnings − contributions); sums to zero. This bot's own result is `nets[seat]` from that hand's `hand-start`. |

```json
{"t":"hand-end","hand_no":1,"nets":[600,-600]}
```

### `match-end`

Sent once, after the final hand's `hand-end`. No further messages follow;
the bot should exit or close the connection.

```json
{"t":"match-end"}
```

### Unknown

Any `"t"` this build doesn't recognize deserializes to an `unknown`
message — ignore it and keep reading. (Reference implementations represent
this as an internal `Unknown` variant; on the wire it is simply whatever
`"t"` you don't understand.)

## Messages: bot → arena (`BotMsg`)

### `join`

The only message a bot sends unprompted, immediately after `hello`. A bare
ready signal — no fields. Identity is operator-assigned and announced back
in `joined`; a legacy `name` field here is ignored like any unknown field.

```json
{"t":"join"}
```

### `action`

The only other message a bot sends, always in reply to an `act`.

| field    | type     | meaning                        |
|----------|----------|-----------------------------------|
| `action` | `Action` | The chosen action (see below).   |

`Action` is itself tagged, on `"kind"`, kebab-case:

| kind      | fields         | meaning                                          |
|-----------|----------------|----------------------------------------------------|
| `fold`    | —              | Fold. Only ever legal when `decision.fold` is true. |
| `check`   | —              | Check.                                             |
| `call`    | —              | Call the current wager (amount is implied).        |
| `bet`     | `to: u64`      | Open the betting this street to a total of `to`.   |
| `raise`   | `to: u64`      | Raise to a total street commitment of `to`.        |
| `bring-in`| —              | Stud bring-in.                                |
| `discard` | `cards: [Card]`| Draw-street discard; empty = stand pat.       |

```json
{"t":"action","action":{"kind":"raise","to":300}}
```

```json
{"t":"action","action":{"kind":"call"}}
```

A non-conforming action (illegal per `decision`, or malformed JSON) is a
**fault** — see [Fault rules](#fault-rules).

## Events (`Event`)

Every `event` message's `ev` field is one of the following, tagged on
`"event"`, kebab-case. This is not a separate protocol: `Event` is a single
type in `poker-wire`, and it is literally what the engine emits and what the
hand logs record, so the wire form and the log form cannot drift apart.

| event            | fields                                                          |
|------------------|-------------------------------------------------------------------|
| `hand-start`     | `hand_no, button, stacks: [u64]`                                   |
| `post`           | `seat, kind: "ante"\|"small-blind"\|"big-blind"\|"bring-in", amount, all_in` |
| `deal-hole`      | `seat, cards: [Card], count`. Private to `seat` — observers (and this bot, for other seats) see `cards: []` with `count` still populated. |
| `street-start`   | `street, label` (e.g. `"flop"`)                                    |
| `deal-community` | `street, cards: [Card]`                                            |
| `deal-up`        | `seat, cards: [Card]` (stud upcards; public)                   |
| `acted`          | `seat, action: Action, street_commit, all_in`                      |
| `draw-result`    | `seat, discarded: [Card], drawn: [Card], count` (`discarded`/`drawn` private to the seat; observers keep `count`) |
| `showdown-show`  | `seat, cards: [Card], hi: HandValue\|null, lo: HandValue\|null`    |
| `pot-awarded`    | `pot, side: "whole"\|"hi"\|"lo", winners: [[seat, amount], ...]`    |
| `hand-end`       | `nets: [i64]`                                                       |

All non-folded hands are shown at showdown — there is no mucking in the
arena, so `showdown-show` is never redacted.

## Action semantics

- **`to` is a total, not an increment.** `bet.to` and `raise.to` are the
  actor's *total* commitment on the current street after the action, not
  the additional chips being added. This removes ambiguity around blinds
  and short/partial calls: e.g. facing a big blind of 100, `raise` to `300`
  means "my total street commitment becomes 300", regardless of what was
  already in front of you.
- **`decision` fully describes what's allowed** at this decision point,
  and is exactly one of three tagged shapes:

  - **`{"kind":"wager", ...}`** — an ordinary betting decision:
    - `fold: bool` — folding is only ever offered when there's something to
      call (open-folding for free is never legal — if you see `fold: false`
      and there's no bet to face, `check` is your only non-committing move).
    - `check: bool`.
    - `call: u64 | absent` — additional chips required to call, when facing
      a wager. Present only when a call is available. May be less than the
      nominal price if it puts you all-in.
    - `bet: { min_to, max_to } | absent` — present only when nothing has
      been wagered yet this street.
    - `raise: { min_to, max_to } | absent` — present only when facing a
      wager.
    - Exactly one of `check`/`call` applies at a given `wager` decision, and
      `bet`/`raise` are mutually exclusive with each other.
    - **`min_to == max_to` means there is exactly one legal total** for that
      action — typically a short all-in below the normal minimum raise, or
      a fixed-limit bet size. Send that exact value; there is no range to
      pick from.
  - **`{"kind":"draw", max_discards}`** — a draw-street decision (see
    [Draw decisions](#draw-decisions)).
  - **`{"kind":"bring-in", bring_in, complete}`** — the stud bring-in
    decision (see [Stud bring-in decisions](#stud-bring-in-decisions)).

  `call`, `bet`, and `raise` are omitted from the JSON entirely (not sent
  as `null`) when not applicable — check for the field's presence, not for
  a null value.

### Draw decisions

- On a draw street, every non-folded seat (all-in seats included) is asked
  exactly once, in seat order starting left of the button, *before* that
  street's betting round. `decision` is `{"kind":"draw","max_discards":u8}`
  — no fold/check/call/bet/raise.
- Reply with `discard`: `cards` must be distinct cards you actually hold,
  and at most `max_discards`. An empty list is standing pat.
- Replacements are dealt immediately and observed via the `draw-result`
  event (`discarded` and `drawn` card lists private to the drawing seat,
  the `count` public — redacted like `deal-hole` for other seats). The
  next street's betting round then opens as usual.

```json
{"t":"act","hand_no":4,"seat":1,"decision":{"kind":"draw","max_discards":3},"deadline_ms":5000}
{"t":"action","action":{"kind":"discard","cards":["2c","7h"]}}
```

### Stud bring-in decisions

- The first betting street of a stud game (`ForcedBets::BringIn` variants —
  `stud-fl`, `stud8-fl`, `razz-fl`) opens with the worst door card owing a
  forced bring-in instead of an ordinary first action. `decision` is
  `{"kind":"bring-in","bring_in":u64,"complete":{"min_to","max_to"}}` —
  `bring_in` is the forced amount (capped at your stack), `complete` is a
  bet range with `min_to == max_to` (completing straight to the small bet)
  — no fold/check/call/raise.
- Reply with either `{"kind":"bring-in"}` (post the forced amount) or
  `{"kind":"bet","to":<complete.min_to (== complete.max_to)>}` (complete
  directly to the small bet). Both are posted as a normal `acted` event;
  later seats can still raise the completed bet through the usual `raise`
  family.

```json
{"t":"act","hand_no":9,"seat":2,"decision":{"kind":"bring-in","bring_in":10,"complete":{"min_to":20,"max_to":20}},"deadline_ms":5000}
{"t":"action","action":{"kind":"bring-in"}}
```

## Deadline semantics

- `deadline_ms` on `act` is the number of milliseconds the arena will wait
  for this decision before treating it as a timeout fault.
- **The arena enforces this server-side** regardless of what the bot does;
  it is not advisory only. A bot should self-limit its thinking time to
  comfortably less than `deadline_ms` (accounting for its own I/O latency)
  so it isn't faulted by transport jitter.
- `timeout_ms` in `hello` is the match-wide default; `deadline_ms` on each
  `act` is the concrete value in effect for that decision (normally the
  same number every time, but treat the per-`act` value as authoritative).

## Fault rules

A **fault** is either an illegal/malformed action or a failure to answer at
all (timeout or disconnect). What happens next is controlled by the
arena's configured fault policy, not by the bot:

- **substitute** (default): the arena substitutes the decision's minimal
  legal action on the bot's behalf — a check (if free) or a fold, a stand
  pat at a draw, the bring-in at a bring-in decision — logs the fault, and
  the match continues.
- **forfeit**: the match ends immediately and the offending bot forfeits.

Either way, faults are visible in the arena's reporting — a bot that relies
on getting away with illegal actions will show up in the fault count even
under substitution. Bots should treat every fault as a bug to fix, not a
recoverable strategy.

## Match result & progress documents

Not bot messages — these are the JSON documents the **CLI** emits for
programmatic consumers (a website ranking bots, a sweep script). Their Rust
definitions live in `poker-wire`'s `report` module (`Serialize` +
`Deserialize`, so a Rust consumer parses them typed with the wire crate
alone); `schema_version` bumps on any breaking shape change.

### Match report (`--output json`, stdout, once)

```json
{"schema_version":1,"family":"betting","game_id":"27td-fl","seed":9,"dealing":"duplicate","decks":50,"hands":100,"seat_count":2,"starting_stack":10000,"stakes":{"kind":"blinds","small_blind":50,"big_blind":100,"ante":0},"betting":{"kind":"fixed-limit","raise_cap":4},"fault_policy":"substitute","timeout_ms":1000,"forfeited_by":null,"bots":[{"name":"random","hands":100,"total_chips":650,"chips_per100_mean":650.0,"chips_per100_ci95":8071.0,"observations":50,"faults":0,"decisions":{"count":812,"mean_ms":0.4,"p50_ms":0.3,"p90_ms":0.9,"p99_ms":5.2,"max_ms":12.1},"behavior":{"vpip":0.63,"pfr":0.36,"af":1.41,"wtsd":0.11,"wsd":0.47,"fold_rate":0.69}}]}
```

- `family` is always `"betting"` here: the CLI emits a different report
  shape for Open Face Chinese games (`"family":"ofc"`, specified in
  `WIRE_PROTOCOL_OFC.md`), and this field lets a consumer dispatch between
  the two without a registry lookup.

- `chips_per100_mean` / `chips_per100_ci95`: winnings per 100 hands in
  **chips** — the canonical unit; normalize for display using `stakes` and
  `betting` (fixed limit: divide by the big bet; pot/no-limit: by the big
  blind). The CI is a two-sided 95% Student-t half-width, `null` under two
  observations.
- `observations`: the sample size behind the interval — hands in seeded
  mode, duplicate rotation-sets in duplicate mode.
- `behavior.af` is `null` when infinite (no calls but some aggression).
- `seed` reproduces the match exactly; `forfeited_by` names the offender
  when a forfeit ended it early (exit code 2).
- `decisions`: wall-clock timing of this bot's decisions (`act` calls),
  measured around the arena's call into the bot — for a wire bot this
  includes the transport round-trip, not pure think time. `count` covers
  every call whether it returned an action or faulted (a timeout's elapsed
  time is real and counted); `mean_ms` and `max_ms` are exact, while
  `p50_ms`/`p90_ms`/`p99_ms` are approximated from a fixed-size log-scaled
  histogram (not the exact values), with relative error bounded by about
  ±4.5%. All five `*_ms` fields are `null` when the bot never decided.
  **The `*_ms` fields are the only ones in this report that are not
  reproducible from `seed`** — the same seed reproduces everything else in
  this document byte-for-byte (`count` included, given deterministic
  bots), but timing varies run to run. The same object, under the same
  key, appears in the OFC report (`WIRE_PROTOCOL_OFC.md`).

### Progress lines (`--progress-json`, stderr, repeating)

Interim standings at the configured cadence (`--progress-every N` decks
and/or `--progress-secs S`), one JSON object per line — a live leaderboard
whose intervals tighten as evidence accumulates:

```json
{"decks_done":100,"hands_done":200,"bots":[{"name":"random","total_chips":-4550,"chips_per100_mean":-2275.0,"chips_per100_ci95":5312.0,"observations":100,"faults":0}]}
```

Field meanings match the final report. Per-hand detail is a separate
stream: `--log FILE` writes the unredacted event log as JSON lines (the
`{"hand":N,"ev":{...}}` shape used throughout `transcripts/`), bracketed by
a `{"hand":N,"deck":D,"seats":["caller","random-2"]}` header line opening
each hand (`deck` groups duplicate rotations of the same deck; `seats[s]`
is the bot at seat `s`) and, once from the CLI, a trailing
`{"log_summary":{"hands_seen":H,"hands_kept":H}}` line.

`--log-sample N` / `--log-top K` / `--log-faults K` switch `--log`
into selective mode: only the first N hands (extended to whole decks so a
duplicate rotation set is never split), the K biggest-pot hands, and the
first K fault hands (forfeited hands always kept) are written, as a batch
when the match ends rather than incrementally. Selective headers add a
`"kept"` array naming the reasons, e.g.
`{"hand":7,"deck":3,"seats":[...],"kept":["sample","top-pot"]}`, and the
summary line adds `sample_first_hands`, `top_pots`, and
`fault_hands_kept`.

## Example transcript

A complete heads-up no-limit hold'em session, **captured verbatim from a
real match** (one hand, both bots checking/calling to showdown), from the
bot at seat 0's point of view. `<` marks arena→bot lines, `>` bot→arena.
Note the heads-up blind convention: seat 0 is the button *and posts the
small blind*, acts first preflop, and acts last on every later street.

```text
< {"t":"hello","proto":1,"game_id":"holdem-nl","stakes":{"kind":"blinds","small_blind":50,"big_blind":100,"ante":0},"betting":{"kind":"no-limit"},"seat_count":2,"starting_stack":10000,"timeout_ms":1000}
> {"t":"join"}
< {"t":"joined","name":"check-call-bot"}
< {"t":"hand-start","hand_no":0,"seat":0}
< {"t":"event","hand_no":0,"ev":{"event":"hand-start","hand_no":0,"button":0,"stacks":[10000,10000]}}
< {"t":"event","hand_no":0,"ev":{"event":"post","seat":0,"kind":"small-blind","amount":50,"all_in":false}}
< {"t":"event","hand_no":0,"ev":{"event":"post","seat":1,"kind":"big-blind","amount":100,"all_in":false}}
< {"t":"event","hand_no":0,"ev":{"event":"street-start","street":0,"label":"preflop"}}
< {"t":"event","hand_no":0,"ev":{"event":"deal-hole","seat":1,"cards":[],"count":2}}
< {"t":"event","hand_no":0,"ev":{"event":"deal-hole","seat":0,"cards":["8h","6c"],"count":2}}
< {"t":"act","hand_no":0,"seat":0,"decision":{"kind":"wager","fold":true,"check":false,"call":50,"raise":{"min_to":200,"max_to":10000}},"deadline_ms":1000}
> {"t":"action","action":{"kind":"call"}}
< {"t":"event","hand_no":0,"ev":{"event":"acted","seat":0,"action":{"kind":"call"},"street_commit":100,"all_in":false}}
< {"t":"event","hand_no":0,"ev":{"event":"acted","seat":1,"action":{"kind":"check"},"street_commit":100,"all_in":false}}
< {"t":"event","hand_no":0,"ev":{"event":"street-start","street":1,"label":"flop"}}
< {"t":"event","hand_no":0,"ev":{"event":"deal-community","street":1,"cards":["Qh","6h","7h"]}}
< {"t":"event","hand_no":0,"ev":{"event":"acted","seat":1,"action":{"kind":"check"},"street_commit":0,"all_in":false}}
< {"t":"act","hand_no":0,"seat":0,"decision":{"kind":"wager","fold":false,"check":true,"bet":{"min_to":100,"max_to":9900}},"deadline_ms":1000}
> {"t":"action","action":{"kind":"check"}}
< {"t":"event","hand_no":0,"ev":{"event":"acted","seat":0,"action":{"kind":"check"},"street_commit":0,"all_in":false}}
< {"t":"event","hand_no":0,"ev":{"event":"street-start","street":2,"label":"turn"}}
< {"t":"event","hand_no":0,"ev":{"event":"deal-community","street":2,"cards":["3s"]}}
< {"t":"event","hand_no":0,"ev":{"event":"acted","seat":1,"action":{"kind":"check"},"street_commit":0,"all_in":false}}
< {"t":"act","hand_no":0,"seat":0,"decision":{"kind":"wager","fold":false,"check":true,"bet":{"min_to":100,"max_to":9900}},"deadline_ms":1000}
> {"t":"action","action":{"kind":"check"}}
< {"t":"event","hand_no":0,"ev":{"event":"acted","seat":0,"action":{"kind":"check"},"street_commit":0,"all_in":false}}
< {"t":"event","hand_no":0,"ev":{"event":"street-start","street":3,"label":"river"}}
< {"t":"event","hand_no":0,"ev":{"event":"deal-community","street":3,"cards":["Js"]}}
< {"t":"event","hand_no":0,"ev":{"event":"acted","seat":1,"action":{"kind":"check"},"street_commit":0,"all_in":false}}
< {"t":"act","hand_no":0,"seat":0,"decision":{"kind":"wager","fold":false,"check":true,"bet":{"min_to":100,"max_to":9900}},"deadline_ms":1000}
> {"t":"action","action":{"kind":"check"}}
< {"t":"event","hand_no":0,"ev":{"event":"acted","seat":0,"action":{"kind":"check"},"street_commit":0,"all_in":false}}
< {"t":"event","hand_no":0,"ev":{"event":"showdown-show","seat":1,"cards":["Jd","4s"],"hi":1680704,"lo":null}}
< {"t":"event","hand_no":0,"ev":{"event":"showdown-show","seat":0,"cards":["8h","6c"],"hi":1354080,"lo":null}}
< {"t":"event","hand_no":0,"ev":{"event":"pot-awarded","pot":0,"side":"whole","winners":[[1,200]]}}
< {"t":"event","hand_no":0,"ev":{"event":"hand-end","nets":[-100,100]}}
< {"t":"hand-end","hand_no":0,"nets":[-100,100]}
< {"t":"match-end"}
```

## Appendix: a minimal Python bot

A complete, dependency-free check/call bot. Reads `ArenaMsg` lines from
stdin, writes `BotMsg` lines to stdout — works unmodified as a spawned
subprocess bot.

```python
#!/usr/bin/env python3
"""Minimal check/call bot for the poker-arena wire protocol (v1)."""
import json
import sys


def send(msg):
    sys.stdout.write(json.dumps(msg, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def choose_action(decision):
    kind = decision["kind"]
    if kind == "draw":
        return {"kind": "discard", "cards": []}  # stand pat
    if kind == "bring-in":
        return {"kind": "bring-in"}
    # kind == "wager"
    if decision.get("check"):
        return {"kind": "check"}
    if decision.get("call") is not None:
        return {"kind": "call"}
    return {"kind": "fold"}  # only legal when facing a wager


for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    t = msg.get("t")
    if t == "hello":
        send({"t": "join"})
    elif t == "act":
        send({"t": "action", "action": choose_action(msg["decision"])})
    elif t == "match-end":
        break
    # hand-start / event / hand-end: nothing for this bot to do.
```
