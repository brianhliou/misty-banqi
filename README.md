# MistyBanqi

[![ci](https://github.com/brianhliou/misty-banqi/actions/workflows/ci.yml/badge.svg)](https://github.com/brianhliou/misty-banqi/actions/workflows/ci.yml)
[![release](https://img.shields.io/github/v/release/brianhliou/misty-banqi)](https://github.com/brianhliou/misty-banqi/releases/latest)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A [Banqi](https://en.wikipedia.org/wiki/Banqi) (Chinese Dark Chess) engine in Rust —
αβ search with **Star1 chance-node expectiminimax** for the game's hidden-tile flips,
a transposition table, repetition handling, quiescence, and a handcrafted evaluation.
Ships as a tiny UCI binary; the same search core is exposed to Python via PyO3.

Banqi is a hidden-information game: pieces start face-down and are *flipped* (revealed)
during play, so a move can be a deterministic move/capture **or** a chance event whose
outcome is drawn from the bag of unrevealed pieces. That mix of decision nodes and chance
nodes is what makes the search interesting — and it's the heart of this engine.

## Strength (honest)

A competent αβ CDC engine, tuned by **large-scale paired-deal bakeoffs** (every relative gain
below is measured that way — paired deals on identical hardware). In context:

- It is **not** SOTA — the strong CDC programs (CLAP_CDC, DarkKnight) are closed and stronger.
- The interesting part isn't an absolute rating; it's *how* the strength was built — see the
  two engineering stories below.

## How it works

**Search — αβ + Star1 over a mixed decision/chance tree.**
A flip is a chance node: the engine doesn't know which piece a face-down tile holds, only the
public *bag* of remaining pieces. Star1 computes the expectiminimax value of a flip as the
probability-weighted average over bag outcomes, with αβ-style bounds (`flip_value` in
[`engine.rs`](banqi_rust/src/engine.rs)) so it prunes chance branches instead of always
expanding all ~14 of them. Decision nodes are ordinary negamax with αβ.

**The usual machinery, tuned for Banqi:** a Zobrist transposition table keyed on
(board, bag, side-to-move); repetition detection (so it avoids shuffling into draws when
ahead and seeks them when losing, via contempt) plus a root anti-draw-sac guard (it won't
shed material into a losing capture just to score a hair above an available draw);
quiescence over captures; and iterative
deepening under a node budget (so strength is CPU-independent — `go nodes N`).

**Evaluation — handcrafted, measured.** Material on a corrected value table, covered-piece
("full-alive") material so a flip never creates phantom value, value-aware mobility,
context-dependent general value, an adaptive *domination* term (a piece is worth more as the
enemy pieces that could capture it dwindle), and a general-safety term (below).

## Two engineering stories

The code is a competent CDC engine; the part worth reading is *how it was tuned*.

**1. The cheap-strength climb (+16.6% win-rate in paired bakeoffs).**
Stacking handcrafted eval terms, each gated behind a feature bit and validated by
**cloud-scale paired-deal bakeoffs** (local 40-game matches are too noisy at Banqi's
~56% draw rate to see a few-percent edge). The single biggest win was finding a **real
bug in the value table** — the cannon (Banqi's most tactically dominant piece, via screen
capture) was *under*-valued and the chariot *over*-valued; correcting the ordering alone
was ~+10%.

**2. The general-safety term — and a lesson in measuring the right thing.**
The engine would let its general get cornered and captured. The fix turned out to be
"make luft": flip a face-down neighbor to give a boxed general a 2×2 escape *before* the
hunting soldier arrives. Two things made this a good case study:

- **Don't adjudicate a defensive idea with the engine's own evaluation** — if the eval is
  blind to the danger, asking it "is this move good?" is circular. The save was verified by
  exact analysis and a played-out line, not by the engine's own score.
- **A defensive term is invisible to an opponent that can't exploit the weakness.** Against
  a baseline that doesn't hunt generals, win-rate barely moved — so a *direct* metric was added
  ("did we lose our own general this game?"), which showed a real, significant drop
  (35.5% → 26%) that win-rate alone hid. Picking the instrument that can actually *see* the
  effect was the whole game.

## Roadmap — the strength ceiling

Handcrafted αβ has a ceiling: the cheap-eval climb plateaued and the latest
term (general safety) bought robustness, not raw strength. The path to the level of the
strong *closed* CDC engines is almost certainly a **learned value network** (AlphaZero-style
— CLAP_CDC proves CDC is learnable to a high level). That's the north star.

It's parked, gated on compute **and** engineering: a local self-play de-risk hasn't yet
climbed past the αβ clone, so the cloud-scale spend isn't justified on local evidence alone
(open question — value-variance / capacity / implementation vs fundamental). αβ today is a
deliberate, measured choice; a value net is the likely next leap once the de-risk clears and
the compute is on the table.

## Build & run

The UCI binary (no Python needed):

```sh
cargo build --release -p banqi-engine
echo "uci" | ./target/release/banqi-engine        # → id name MistyBanqi 0.2.2 ...
```

Drive it over UCI with a Banqi FEN (face-down tile = `X`; turn `r`/`b`/`-`; then the bag
and clock):

```
uci
position fen XXXXXXXX/XXXXXXXX/XXXXXXXX/XXXXXXXX - <bag> 0 1
go nodes 1500000
```

The Python bindings (the same search core via PyO3):

```sh
pip install maturin
maturin develop --release -m banqi_rust/Cargo.toml
python -c "import banqi_rust; print([f for f in dir(banqi_rust) if not f.startswith('_')])"
```

## Layout

```
banqi_rust/      engine core (engine.rs) + PyO3 Python bindings (lib.rs)
banqi-engine/    standalone UCI binary (main.rs; #[path]-includes the core)
.github/workflows/  ci (build + uci smoke) and release (tag → published binary)
```

## Acknowledgements

The handcrafted evaluation started from — and drew ideas out of —
[george0828Zhang/chinese-dark-chess-hw](https://github.com/george0828Zhang/chinese-dark-chess-hw),
which was also the fixed reference opponent the bakeoffs were tuned against during development.
The context-dependent general value and the value-weighted mobility, in particular, are adapted
from it.

## License

MIT — see [LICENSE](LICENSE).
