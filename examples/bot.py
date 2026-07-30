#!/usr/bin/env python3
"""A minimal poker-arena wire bot in Python (protocol v1, no dependencies).

Reads arena messages from stdin, writes bot messages to stdout — run it with:

    poker-arena run --game holdem-nl \
        --bot cmd:"python3 examples/bot.py" --bot builtin:random

Protocol reference: WIRE_PROTOCOL.md. The strategy here is simple but
legal: check when free, call small bets, raise the minimum with strong-ish
preflop holdings. Replace `decide()` with your own brain.
"""

import json
import sys


def send(msg):
    sys.stdout.write(json.dumps(msg, separators=(",", ":")) + "\n")
    sys.stdout.flush()


def decide(act):
    """Return an action object conforming to act["legal"]."""
    legal = act["legal"]
    hole = act.get("hole", [])

    # A crude preflop heuristic: raise the minimum holding a pair or an ace.
    ranks = [c[0] for c in hole]
    strong = len(set(ranks)) < len(ranks) or "A" in ranks
    if strong and legal.get("raise") is not None:
        return {"kind": "raise", "to": legal["raise"]["min_to"]}
    if strong and legal.get("bet") is not None:
        return {"kind": "bet", "to": legal["bet"]["min_to"]}

    if legal.get("check"):
        return {"kind": "check"}

    # Call anything cheap relative to the pot; otherwise fold.
    call = legal.get("call")
    if call is not None and call * 3 <= act["pot_total"] + call:
        return {"kind": "call"}
    if legal.get("fold"):
        return {"kind": "fold"}
    return {"kind": "call"}  # facing an all-in smaller than a third of pot


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        msg = json.loads(line)
        t = msg.get("t")
        if t == "hello":
            send({"t": "join", "name": "python-example"})
        elif t == "act":
            send({"t": "action", "action": decide(msg)})
        elif t == "match-end":
            return
        # hand-start / event / hand-end / unknown types: nothing to do.


if __name__ == "__main__":
    main()
