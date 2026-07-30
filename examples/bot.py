#!/usr/bin/env python3
"""A minimal poker-arena wire bot in Python (protocol v1, no dependencies).

Reads arena messages from stdin, writes bot messages to stdout — run it with:

    poker-arena run --game holdem-nl \
        --bot cmd:"python3 examples/bot.py" --bot builtin:random

Protocol reference: WIRE_PROTOCOL.md. The event stream is the single source
of truth — `act` messages carry only the legal actions and deadline — so a
bot tracks the little state it cares about from events, as this one does
(its own cards and the pot size). The strategy is simple but legal across
every registry game. Replace `decide()` with your own brain.
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
        self.chosen_discards = []

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
            self.hole = [c for c in self.hole if c not in self.chosen_discards]
            self.hole.extend(ev["drawn"])


def decide(table, legal):
    """Return an action object conforming to `legal`."""
    # Draw streets: discard high cards, up to the offered max.
    draw = legal.get("draw")
    if draw is not None:
        high = [c for c in table.hole if c[0] in "9TJQKA"]
        table.chosen_discards = high[: draw["max_discards"]]
        return {"kind": "discard", "cards": table.chosen_discards}

    # Stud: always post the forced bring-in rather than completing.
    if legal.get("bring_in") is not None:
        return {"kind": "bring-in"}

    # A crude opening heuristic: raise the minimum holding a pair or an ace.
    ranks = [c[0] for c in table.hole]
    strong = len(set(ranks)) < len(ranks) or "A" in ranks
    if strong and legal.get("raise") is not None:
        return {"kind": "raise", "to": legal["raise"]["min_to"]}
    if strong and legal.get("bet") is not None:
        return {"kind": "bet", "to": legal["bet"]["min_to"]}

    if legal.get("check"):
        return {"kind": "check"}

    # Call anything cheap relative to the pot; otherwise fold.
    call = legal.get("call")
    if call is not None and call * 3 <= table.pot + call:
        return {"kind": "call"}
    if legal.get("fold"):
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
            send({"t": "join", "name": "python-example"})
        elif t == "hand-start":
            table.hand_start(msg)
        elif t == "event":
            table.observe(msg["ev"])
        elif t == "act":
            send({"t": "action", "action": decide(table, msg["legal"])})
        elif t == "match-end":
            return
        # hand-end / unknown types: nothing to do.


if __name__ == "__main__":
    main()
