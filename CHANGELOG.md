# Changelog

All notable changes to MistyBanqi. Strength figures are relative gains measured via
large-scale paired-deal bakeoffs (see the README).

## [0.3.0]

- Optional NNUE evaluation through UCI `EvalFile` (empty by default). No trained
  net ships with the engine. Separately supplied files use BQNN v2: a 24-byte
  little-endian header and float32 weights; see the README for the layout.
- An empty value or `<empty>` restores classical evaluation. Failed loads report
  `info string error EvalFile: ...; using classical` and clear any previous net;
  paths may contain spaces. Options persist across `ucinewgame`.
- Net mode uses neutral draws, four capture-quiescence plies, a depth ceiling of
  64 and exact chase/repetition state, with position-only TT and approximate cycle
  draws disabled. Classical search and the public node/time, root-filter and
  `NoProgressLimit` options retain their existing defaults and behavior.
- Fixed-node regression fixtures cover 78 classical positions from 0.2.7 and 78
  NNUE positions from the reference UCI engine, alongside inference and load tests.
- `Noise`, `NoiseDepth` and `Seed` work with both classical and NNUE evaluation.
  Trained-net checks read `MISTY_BANQI_EVALFILE` and skip when unset; synthetic-net
  tests and a repository scan run without it. All workspace crates are 0.3.0.
- UCI binary `banqi-engine` reports `id name MistyBanqi 0.3.0`.

## [0.2.8]

- **Weaker play**: UCI `Noise` (string, default 0) adds Gaussian noise to root
  move scores, including flips. `NoiseDepth` (spin, default 1, range 1-8) sets
  the scoring depth; `Seed` (string, default 0, u64) enables reproducible choices
  when nonzero. Settings persist across `ucinewgame`; Noise 0 keeps the normal search.
- **Bounded noisy search**: deepen to the requested depth under the node and time
  limits, choosing from the deepest completed pass. Depth 1 always completes.
  Guards preserve immediate wins and avoid immediate losses when a safe move exists,
  checking all flip reveals, and protect terminal scores found by completed passes.

## [0.2.7]

- **UCI option `NoProgressLimit`** (spin, default 40): the plies without a capture or flip
  after which the game is drawn. The search hard-coded 40, so under a host rule of 60 it
  scored plies 40-60 as already drawn and stopped trying to make progress there. Send
  `setoption name NoProgressLimit value <n>` after `uci`; without it the engine keeps 40.
- **wasm (banqi-wasm 0.2.5)**: `analyze_with_limit(fen, nodes, multipv, limit)` and
  `AnalysisSession.set_no_progress_limit(limit)` take the same limit for browser analysis.
  `analyze` and a session left unset keep 40.

## [0.2.6]

- **UCI `go searchmoves`**: the root search now picks only from the listed moves, which
  Mistboard uses to keep the bot out of a perpetual chase. A list with no legal move in it
  is ignored and the search runs unrestricted, so a stale list never yields `(none)`.

## [0.2.3]

- **Provable-elimination adjudication**: a side is now ruled lost the moment it has no
  piece on the board and no tile of its colour left in the bag to ever flip up, instead
  of only once every tile has been revealed. Such a side can never hold a piece again, so
  the game ends immediately rather than forcing the doomed side to flip the opponent's
  remaining tiles out first. Outcome-identical (same winner), just adjudicated sooner; the
  draw clocks still take precedence. Mirrors the reference rule implementations.

## [0.2.2]

- **Anti-draw-sac root guard** (`no_draw_sac`, Feat bit 512): at the root, a marginal move
  (eval < +0.3) that makes an obviously losing capture (crude SEE) is clamped below the draw
  value, so the engine never sheds material into a position it can't convert and then takes
  the draw anyway. Measured in ~385-game paired bakeoffs: no regression, losses 33→19.

## [0.2.1]

- **General-safety eval term** (`gen_danger`, Feat bit 256): a proximity- and escape-aware
  penalty for the general being threatened by an enemy soldier along an open line, or being
  cornered. Drives the "make-luft" defense (open an escape before the general is trapped) and
  measurably reduces own-general losses, with no regression in overall play.

## [0.2.0]

- **"Cheap-strength" eval:** covered-piece (full-alive) material, context-dependent general
  value, value-weighted mobility, an adaptive domination term, and a corrected value table.
  ~+16.6% win-rate in paired-deal bakeoffs (the corrected value table alone ~+10%).

## [0.1.0]

- Initial standalone engine: αβ + **Star1 chance-node** search, transposition table,
  repetition handling, and quiescence; UCI binary + PyO3 Python bindings.
