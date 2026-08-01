#!/usr/bin/env python3
"""A minimal poker-arena OFC wire bot in Python (protocol v1, no dependencies).

Reads arena messages from stdin, writes bot messages to stdout — run it with:

    poker-arena run --game ofc-pineapple \
        --bot cmd:"python3 examples/ofc_bot.py" --bot builtin:greedy

Protocol reference: WIRE_PROTOCOL_OFC.md. `hello` carries only game_id plus
the per-match parameters that can't be derived from it — bots are expected
to know the named variant's rules. The event stream is the single source of
truth — `act` carries only the decision (how many to place, how many to
discard) — so a bot tracks the little state it cares about from events, as
this one does (its own seat, its own dealt-but-unplaced cards, and its own
board, to know each row's free capacity). The strategy is the contract's
filler rule: lowest cards first, into bottom, then middle, then top, as each
fills up. Replace `decide()` with your own brain.
"""

import json
import sys

ROW_CAPACITY = {"top": 3, "middle": 5, "bottom": 5}
ROW_ORDER = ("bottom", "middle", "top")  # fill order: bottom first

RANKS = "23456789TJQKA"


def card_index(card):
    """Ascending sort key matching Card::index: rank first, then suit."""
    rank = RANKS.index(card[0])
    suit = "cdhs".index(card[1])
    return rank * 4 + suit


def send(msg):
    sys.stdout.write(json.dumps(msg, separators=(",", ":")) + "\n")
    sys.stdout.flush()


class Table:
    """The slice of OFC state this bot bothers to track."""

    def __init__(self):
        self.seat = None
        self.dealt = []
        self.board = {"top": [], "middle": [], "bottom": []}

    def hand_start(self, msg):
        self.seat = msg["seat"]
        self.dealt = []
        self.board = {"top": [], "middle": [], "bottom": []}

    def observe(self, ev):
        kind = ev["event"]
        if kind == "deal" and ev["seat"] == self.seat:
            self.dealt.extend(ev["cards"])
        elif kind == "place" and ev["seat"] == self.seat:
            for placement in ev["placements"]:
                self.board[placement["row"]].append(placement["card"])
            self.dealt = []

    def free(self, row):
        return ROW_CAPACITY[row] - len(self.board[row])


def decide(table, decision):
    """The filler rule: sort dealt cards ascending, place into the first row
    (bottom, then middle, then top) that still has room, discard the rest.

    Capacities are tracked in a scratch copy, not `table.board` itself: the
    real board only ever updates from the `place` event echoed back on the
    wire (see `Table.observe`), so mutating it here too would double-count
    every card this turn placed.
    """
    place = decision["place"]
    dealt = sorted(table.dealt, key=card_index)
    placed, discarded = dealt[:place], dealt[place:]

    free = {row: table.free(row) for row in ROW_ORDER}
    placements = []
    for card in placed:
        row = next(r for r in ROW_ORDER if free[r] > 0)
        placements.append({"card": card, "row": row})
        free[row] -= 1

    return {"placements": placements, "discards": discarded}


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
        # joined ack / hand-end / unknown types: nothing to do.


if __name__ == "__main__":
    main()
