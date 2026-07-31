#!/usr/bin/env python3
"""A minimal poker-arena wire bot in Python (protocol v1, no dependencies).

Reads arena messages from stdin, writes bot messages to stdout — run it with:

    poker-arena run --game holdem-nl \
        --bot cmd:"python3 examples/bot.py" --bot builtin:random

Protocol reference: WIRE_PROTOCOL.md. `hello` carries only game_id plus the
per-match parameters that can't be derived from it — bots are expected to
know the named game's rules. The event stream is the single source of truth
— `act` messages carry only the decision and deadline — so a bot tracks the
little state it cares about from events, as this one does (its own cards and
the pot size). The strategy is simple but legal across every registry game.
Replace `decide()` with your own brain.
"""

import json
import sys


def send(msg):
    sys.stdout.write(json.dumps(msg, separators=(",", ":")) + "\n")
    sys.stdout.flush()


class Table:
    """The slice of game state this bot bothers to track."""

    def __init__(self):
        self.seat = None
        self.hole = []
        self.pot = 0
        self.commits = {}

    def hand_start(self, msg):
        self.seat = msg["seat"]
        self.hole = []
        self.pot = 0
        self.commits = {}

    def observe(self, ev):
        kind = ev["event"]
        if kind == "post":
            self.pot += ev["amount"]
        elif kind == "street-start":
            self.commits = {}
        elif kind == "acted":
            prev = self.commits.get(ev["seat"], 0)
            self.commits[ev["seat"]] = ev["street_commit"]
            self.pot += ev["street_commit"] - prev
        elif kind == "deal-hole" and ev["seat"] == self.seat and ev["cards"]:
            self.hole.extend(ev["cards"])  # extend: stud deals down cards twice
        elif kind == "draw-result" and ev["seat"] == self.seat:
            # Own events are unredacted: discarded lists exactly what left.
            self.hole = [c for c in self.hole if c not in ev["discarded"]]
            self.hole.extend(ev["drawn"])


def decide(table, decision):
    """Return an action object conforming to `decision`."""
    kind = decision["kind"]

    # Draw streets: discard high cards, up to the offered max.
    if kind == "draw":
        high = [c for c in table.hole if c[0] in "9TJQKA"]
        return {"kind": "discard", "cards": high[: decision["max_discards"]]}

    # Stud: always post the forced bring-in rather than completing.
    if kind == "bring-in":
        return {"kind": "bring-in"}

    # kind == "wager": a crude opening heuristic — raise/bet the minimum
    # holding a pair or an ace, otherwise check, call cheap, or fold.
    ranks = [c[0] for c in table.hole]
    strong = len(set(ranks)) < len(ranks) or "A" in ranks
    if strong and decision.get("raise") is not None:
        return {"kind": "raise", "to": decision["raise"]["min_to"]}
    if strong and decision.get("bet") is not None:
        return {"kind": "bet", "to": decision["bet"]["min_to"]}

    if decision.get("check"):
        return {"kind": "check"}

    # Call anything cheap relative to the pot; otherwise fold.
    call = decision.get("call")
    if call is not None and call * 3 <= table.pot + call:
        return {"kind": "call"}
    if decision.get("fold"):
        return {"kind": "fold"}
    return {"kind": "call"}  # facing an all-in smaller than a third of pot


def main():
    table = Table()
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        msg = json.loads(line)
        t = msg.get("t")
        if t == "hello":
            send({"t": "join"})  # identity is operator-assigned
        elif t == "hand-start":
            table.hand_start(msg)
        elif t == "event":
            table.observe(msg["ev"])
        elif t == "act":
            send({"t": "action", "action": decide(table, msg["decision"])})
        elif t == "match-end":
            return
        # hand-end / unknown types: nothing to do.


if __name__ == "__main__":
    main()
