# Changelog

All notable changes to MistyBanqi. Strength figures are relative gains measured via
large-scale paired-deal bakeoffs (see the README).

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
