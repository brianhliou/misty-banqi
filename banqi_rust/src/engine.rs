//! Banqi (Chinese Dark Chess) board + search, in Rust, exposed to Python via PyO3.
//! Self-contained (only PyO3 as a dependency). The engine works on a masked-state
//! contract (the engine never sees the deal; a flip is resolved by an external
//! authority or a search sample over the bag), same αβ + Star1 + quiescence +
//! draw-contempt search. The point is speed: native search reaches far deeper than
//! the Python loop at equal wall-clock.
//!
//! Encoding across the Python boundary:
//!   square: -1 empty, -2 face-down, else piece code 0..13 = color*7 + role
//!   roles 0..6 = general, advisor, elephant, chariot, horse, cannon, soldier
//!   color: 0 red, 1 black; first_color: -1 unbound, 0 red, 1 black
//!   bag: [u32; 14] indexed by the piece code (count of unrevealed of that type)

use std::sync::OnceLock;
use std::time::Instant;

const W: i32 = 8;
const H: i32 = 4;
const NSQ: usize = 32;
const EMPTY: i16 = -1;
const DOWN: i16 = -2;

// roles: 0 general, 1 advisor, 2 elephant, 3 chariot, 4 horse, 5 cannon, 6 soldier
const VALUE: [f64; 7] = [30.0, 10.0, 10.0, 14.0, 8.0, 12.0, 4.0];
const RANK: [i32; 7] = [7, 6, 5, 4, 3, 2, 1];
const EVAL_SCALE: f64 = 60.0;
const W_MOB: f64 = 0.8;
const VMIN: f64 = -1.0;
const VMAX: f64 = 1.0;
const INF: f64 = f64::INFINITY;
const ORTHO: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

// result codes
const RES_RED: i16 = 0;
const RES_BLACK: i16 = 1;
const RES_DRAW: i16 = 2;
const RES_ONGOING: i16 = 3;

#[inline]
fn is_piece(c: i16) -> bool {
    c >= 0
}
#[inline]
fn code_color(c: i16) -> i16 {
    c / 7
}
#[inline]
fn code_role(c: i16) -> usize {
    (c % 7) as usize
}
#[inline]
fn coord(i: usize) -> (i32, i32) {
    ((i as i32) % W, (i as i32) / W + 1)
}
#[inline]
fn sqi(file: i32, rank: i32) -> usize {
    (file + (rank - 1) * W) as usize
}
#[inline]
fn in_bounds(file: i32, rank: i32) -> bool {
    file >= 0 && file < W && rank >= 1 && rank <= H
}

/// Convert a Python-supplied weights list into the fixed material array. An empty
/// or wrong-length list falls back to the built-in VALUE defaults, so callers that
/// don't tune material (and the parity path) get the canonical values.
fn to_values(v: &[f64]) -> [f64; 7] {
    if v.len() == 7 {
        let mut a = [0.0; 7];
        a.copy_from_slice(v);
        a
    } else {
        VALUE
    }
}

fn can_capture(a: usize, t: usize) -> bool {
    if a == 6 && t == 0 {
        return true; // soldier captures general
    }
    if a == 0 && t == 6 {
        return false; // general cannot capture soldier
    }
    RANK[a] >= RANK[t]
}

#[derive(Clone)]
struct State {
    sq: [i16; NSQ],
    bag: [u32; 14],
    first_color: i16,
    ply: u32,
    no_progress: u32,
}

impl State {
    fn mover_color(&self) -> i16 {
        if self.first_color < 0 {
            return -1;
        }
        if self.ply % 2 == 0 {
            self.first_color
        } else {
            1 - self.first_color
        }
    }

    fn piece_moves(&self, frm: usize, c: i16, mc: i16, out: &mut Vec<(u8, u8)>) {
        let role = code_role(c);
        let (file, rank) = coord(frm);
        if role == 5 {
            // cannon: one-step non-capturing move
            for (df, dr) in ORTHO {
                let (f, r) = (file + df, rank + dr);
                if in_bounds(f, r) && self.sq[sqi(f, r)] == EMPTY {
                    out.push((frm as u8, sqi(f, r) as u8));
                }
            }
            // cannon: capture by jumping exactly one screen
            for (df, dr) in ORTHO {
                let (mut f, mut r) = (file + df, rank + dr);
                while in_bounds(f, r) && self.sq[sqi(f, r)] == EMPTY {
                    f += df;
                    r += dr;
                }
                if !in_bounds(f, r) {
                    continue;
                }
                f += df;
                r += dr; // skip the screen
                while in_bounds(f, r) && self.sq[sqi(f, r)] == EMPTY {
                    f += df;
                    r += dr;
                }
                if !in_bounds(f, r) {
                    continue;
                }
                let t = self.sq[sqi(f, r)];
                if is_piece(t) && code_color(t) != mc {
                    out.push((frm as u8, sqi(f, r) as u8));
                }
            }
            return;
        }
        for (df, dr) in ORTHO {
            let (f, r) = (file + df, rank + dr);
            if !in_bounds(f, r) {
                continue;
            }
            let t = self.sq[sqi(f, r)];
            if t == EMPTY {
                out.push((frm as u8, sqi(f, r) as u8));
            } else if is_piece(t) && code_color(t) != mc && can_capture(role, code_role(t)) {
                out.push((frm as u8, sqi(f, r) as u8));
            }
        }
    }

    fn legal_moves(&self, out: &mut Vec<(u8, u8)>) {
        out.clear();
        for i in 0..NSQ {
            if self.sq[i] == DOWN {
                out.push((i as u8, i as u8));
            }
        }
        let mc = self.mover_color();
        if mc < 0 {
            return;
        }
        for i in 0..NSQ {
            let c = self.sq[i];
            if is_piece(c) && code_color(c) == mc {
                self.piece_moves(i, c, mc, out);
            }
        }
    }

    fn mobility(&self, color: i16) -> i32 {
        let mut tmp: Vec<(u8, u8)> = Vec::new();
        let mut n = 0;
        for i in 0..NSQ {
            let c = self.sq[i];
            if is_piece(c) && code_color(c) == color {
                tmp.clear();
                self.piece_moves(i, c, color, &mut tmp);
                n += tmp.len() as i32;
            }
        }
        n
    }

    /// Value-aware mobility (Feat::vmob): each legal move weighted by its mover's value,
    /// normalized by the average piece value so the result stays on the same scale as the
    /// plain move count (and w_mob keeps its tuned magnitude).
    fn mobility_valued(&self, color: i16, values: &[f64; 7]) -> f64 {
        const AVG_VAL: f64 = 12.0; // ~mean of the default value table
        let mut tmp: Vec<(u8, u8)> = Vec::new();
        let mut s = 0.0;
        for i in 0..NSQ {
            let c = self.sq[i];
            if is_piece(c) && code_color(c) == color {
                tmp.clear();
                self.piece_moves(i, c, color, &mut tmp);
                s += values[code_role(c)] * tmp.len() as f64;
            }
        }
        s / AVG_VAL
    }

    fn material(&self, persp: i16, values: &[f64; 7]) -> f64 {
        let mut m = 0.0;
        for i in 0..NSQ {
            let c = self.sq[i];
            if is_piece(c) {
                let v = values[code_role(c)];
                if code_color(c) == persp {
                    m += v;
                } else {
                    m -= v;
                }
            }
        }
        m
    }

    /// Covered-piece material differential (Feat::cover_mat): face-down pieces counted
    /// at their PUBLIC bag value. The bag is per-(color,role) and derivable by both seats
    /// (full set minus revealed), so this leaks no hidden information. Added to `material`
    /// it yields the full-alive count: a flip moves a piece from "covered" to "revealed"
    /// without changing the total, so only a CAPTURE changes material.
    fn covered_material(&self, persp: i16, values: &[f64; 7]) -> f64 {
        let opp = (1 - persp) as usize;
        let me = persp as usize;
        let mut m = 0.0;
        for r in 0..7 {
            m += self.bag[me * 7 + r] as f64 * values[r];
            m -= self.bag[opp * 7 + r] as f64 * values[r];
        }
        m
    }

    /// Context-king-value differential (Feat::king_ctx): each side's general gets a bonus
    /// that grows as the OPPONENT's soldier count (revealed + covered, all public) shrinks —
    /// because only a soldier (or a cannon screen) can take a general, so a general facing
    /// no enemy soldiers is nearly unkillable — a context-dependent general value.
    /// Returned as a my-minus-opp delta to add onto material.
    fn king_ctx_term(&self, persp: i16) -> f64 {
        // general bonus indexed by enemy-soldier-alive count (0..=5); ~general*{0.47,0.14,
        // 0.027,0.006,0,0}, scaled to our general value (30).
        const KING_CTX: [f64; 6] = [14.1, 4.1, 0.8, 0.2, 0.0, 0.0];
        let me = persp as usize;
        let opp = (1 - persp) as usize;
        let mut sol = [self.bag[me * 7 + 6], self.bag[opp * 7 + 6]]; // [me, opp] soldiers
        let mut gen_alive = [self.bag[me * 7] > 0, self.bag[opp * 7] > 0];
        for i in 0..NSQ {
            let c = self.sq[i];
            if !is_piece(c) {
                continue;
            }
            let side = if code_color(c) as usize == me { 0 } else { 1 };
            match code_role(c) {
                6 => sol[side] += 1,
                0 => gen_alive[side] = true,
                _ => {}
            }
        }
        let mut t = 0.0;
        if gen_alive[0] {
            t += KING_CTX[(sol[1] as usize).min(5)]; // my general vs opp soldiers
        }
        if gen_alive[1] {
            t -= KING_CTX[(sol[0] as usize).min(5)]; // opp general vs my soldiers
        }
        t
    }

    /// Adaptive domination value (Feat::dom_val): each REVEALED piece gets a bonus that
    /// grows as the alive (revealed + bag) enemy pieces able to CAPTURE it shrink — toward
    /// "immortal" when its dominators are gone. An enemy cannon screen-captures anything, so
    /// it always counts as a dominator. Bonus = value[r] * DOM_K / (1 + dominator_count),
    /// returned as a my-minus-opp delta. Generalizes king_ctx (the general's dominators are
    /// enemy general + cannon + soldier) to the whole capture lattice.
    fn domination_value(&self, persp: i16, values: &[f64; 7]) -> f64 {
        const DOM_K: f64 = 0.5;
        // alive[color][role] = revealed-on-board + still-in-bag (all public).
        let mut alive = [[0u32; 7]; 2];
        for color in 0..2 {
            for r in 0..7 {
                alive[color][r] = self.bag[color * 7 + r];
            }
        }
        for i in 0..NSQ {
            let c = self.sq[i];
            if is_piece(c) {
                alive[code_color(c) as usize][code_role(c)] += 1;
            }
        }
        let mut total = 0.0;
        for i in 0..NSQ {
            let c = self.sq[i];
            if !is_piece(c) {
                continue;
            }
            let r = code_role(c);
            let mine = code_color(c) == persp;
            let enemy = (if mine { 1 - persp } else { persp }) as usize;
            // count alive enemy pieces that can capture role r (cannon always: screen).
            let mut dom = 0u32;
            for d in 0..7 {
                if d == 5 || can_capture(d, r) {
                    dom += alive[enemy][d];
                }
            }
            let bonus = values[r] * DOM_K / (1.0 + dom as f64);
            total += if mine { bonus } else { -bonus };
        }
        total
    }

    /// General-danger for `color` (Feat::gen_danger) — the proximity-AND-escape-aware
    /// successor to `king_danger`. The pathology it fixes (see the README): a soldier
    /// marching down an OPEN line at the
    /// general is invisible to search (the approach is QUIET, so quiescence never extends
    /// it) until it is adjacent — by which point the general is cornered. king_danger only
    /// sees adjacency and rates an empty-neighbour corner as 0-danger (a trap). This term:
    ///   * threat = Σ over enemy soldiers of 1/2^(plies_to_capture−1), where
    ///     plies_to_capture = 1 (soldier already adjacent) or 1 + (BFS steps through EMPTY
    ///     squares to the nearest empty staging square = an empty orthogonal neighbour of
    ///     the general). A capped BFS (≤5) — beyond that the 1/2^d weight is negligible.
    ///     Only REVEALED enemy soldiers count: panicking over face-down neighbours walks
    ///     the general to corners (the falsified flip_risk disease) and ignores that the
    ///     real attacker may be blocked — react to concrete threats, not uncertainty.
    ///   * escape = empty orthogonal neighbours of the general NOT adjacent to an enemy
    ///     soldier (squares it can actually flee to). danger = threat / (1 + escape), so a
    ///     cornered general (escape 0) keeps full weight and an open one is damped.
    /// Returned for one side; the caller takes my-minus-opp. Cannon screens are a TODO.
    fn general_danger(&self, color: i16) -> f64 {
        let opp = 1 - color;
        let sol = (opp * 7 + 6) as i16; // enemy soldier code
        let gen = color * 7;
        let gpos = (0..NSQ).find(|&i| self.sq[i] == gen);
        let gpos = match gpos {
            Some(p) => p,
            None => return 0.0,
        };
        let (gf, gr) = coord(gpos);
        // Staging squares = orthogonal neighbours of the general. Classify each.
        let mut empty_staging: Vec<usize> = Vec::with_capacity(4);
        let mut threat = 0.0;
        let mut escape = 0u32;
        for (df, dr) in ORTHO {
            let (f, r) = (gf + df, gr + dr);
            if !in_bounds(f, r) {
                continue;
            }
            let s = sqi(f, r);
            let c = self.sq[s];
            if c == EMPTY {
                empty_staging.push(s);
                // a flee square only if not itself adjacent to an enemy soldier
                let mut safe = true;
                for (ef, er) in ORTHO {
                    let (nf, nr) = (f + ef, r + er);
                    if in_bounds(nf, nr) && self.sq[sqi(nf, nr)] == sol {
                        safe = false;
                        break;
                    }
                }
                if safe {
                    escape += 1;
                }
            } else if c == sol {
                threat += 1.0; // soldier already adjacent → captures next move (p=1)
            }
        }
        // Multi-source BFS from the empty staging squares through EMPTY squares; an enemy
        // soldier found at frontier-distance k has plies_to_capture = k + 1.
        if !empty_staging.is_empty() {
            let mut dist = [u8::MAX; NSQ];
            let mut queue: Vec<usize> = Vec::with_capacity(NSQ);
            for &s in &empty_staging {
                dist[s] = 0;
                queue.push(s);
            }
            let mut head = 0;
            let mut found = [false; NSQ]; // soldiers already counted
            while head < queue.len() {
                let cur = queue[head];
                head += 1;
                let d = dist[cur];
                if d >= 5 {
                    continue; // cap: 1/2^5 weight is negligible
                }
                let (cf, cr) = coord(cur);
                for (df, dr) in ORTHO {
                    let (nf, nr) = (cf + df, cr + dr);
                    if !in_bounds(nf, nr) {
                        continue;
                    }
                    let n = sqi(nf, nr);
                    let nc = self.sq[n];
                    if nc == sol && !found[n] {
                        found[n] = true;
                        // plies_to_capture = (steps to reach an empty staging) + 1 capture.
                        // The soldier sits one edge beyond the frontier square (dist d),
                        // so it needs d+1 steps to reach the staging, then 1 to capture.
                        let p = (d as u32) + 2; // (d+1) walk + 1 capture
                        threat += 0.5_f64.powi((p - 1) as i32);
                    } else if nc == EMPTY && dist[n] == u8::MAX {
                        dist[n] = d + 1;
                        queue.push(n);
                    }
                }
            }
        }
        // Confinement (the positional half — higher falsification risk, flip_risk-adjacent):
        // a general on an edge/corner is a LATENT trap even with no soldier on a path yet,
        // because once one arrives it has too few flee routes. Scaled by how many enemy
        // soldiers are still ALIVE (revealed + bag) to exploit it — with no enemy soldiers
        // a cornered general is fine (nothing can take it). corner=2, edge=1, centre=0.
        const CONF_K: f64 = 0.15;
        let onboard = ORTHO
            .iter()
            .filter(|(df, dr)| in_bounds(gf + df, gr + dr))
            .count();
        let confine = (4 - onboard) as f64;
        let mut enemy_sol = self.bag[(opp * 7 + 6) as usize];
        for i in 0..NSQ {
            if self.sq[i] == sol {
                enemy_sol += 1;
            }
        }
        let latent = (enemy_sol.min(4) as f64) / 4.0;
        threat / (1.0 + escape as f64) + CONF_K * confine * latent
    }

    /// King danger for `color`: face-down squares adjacent to its general (could
    /// be an enemy soldier — invisible to search) weigh most; an adjacent revealed
    /// enemy soldier (an immediate general-capture threat) also counts.
    fn king_danger(&self, color: i16) -> f64 {
        let gen = color * 7; // general = role 0
        let mut gpos: i32 = -1;
        for i in 0..NSQ {
            if self.sq[i] == gen {
                gpos = i as i32;
                break;
            }
        }
        if gpos < 0 {
            return 0.0;
        }
        let (gf, gr) = coord(gpos as usize);
        let mut d = 0.0;
        for (df, dr) in ORTHO {
            let (f, r) = (gf + df, gr + dr);
            if !in_bounds(f, r) {
                continue;
            }
            let c = self.sq[sqi(f, r)];
            if c == DOWN {
                d += 1.0; // unknown neighbor — could be a soldier
            } else if is_piece(c) && code_color(c) != color && code_role(c) == 6 {
                d += 2.0; // adjacent enemy soldier can capture the general
            }
        }
        d
    }

    fn eval(&self, persp: i16, w_mob: f64, w_king: f64, values: &[f64; 7], feat: &Feat) -> f64 {
        if persp < 0 {
            return 0.0;
        }
        let opp = 1 - persp;
        let mut mat = self.material(persp, values);
        if feat.cover_mat {
            mat += self.covered_material(persp, values);
        }
        if feat.king_ctx {
            mat += self.king_ctx_term(persp);
        }
        if feat.dom_val {
            mat += self.domination_value(persp, values);
        }
        let mob = if feat.vmob {
            self.mobility_valued(persp, values) - self.mobility_valued(opp, values)
        } else {
            (self.mobility(persp) - self.mobility(opp)) as f64
        };
        // King-safety term. With Feat::gen_danger the proximity+escape-aware
        // general_danger REPLACES the adjacency-only king_danger (both scaled by the
        // w_king weight slot). Short-circuit when off: at w_king=0 and no gen_danger
        // the eval value AND throughput stay byte-identical to the pre-term engine.
        let king = if feat.gen_danger && w_king != 0.0 {
            self.general_danger(persp) - self.general_danger(opp) // my danger minus theirs
        } else if !feat.gen_danger && w_king != 0.0 {
            self.king_danger(persp) - self.king_danger(opp) // my danger minus theirs
        } else {
            0.0
        };
        ((mat + w_mob * mob - w_king * king) / EVAL_SCALE).tanh()
    }

    fn push(&mut self, frm: u8, to: u8, revealed: i16) {
        if frm == to {
            self.sq[frm as usize] = revealed;
            self.bag[revealed as usize] -= 1;
            if self.first_color < 0 {
                self.first_color = code_color(revealed);
            }
            self.no_progress = 0;
        } else {
            let t = self.sq[to as usize];
            if is_piece(t) {
                self.no_progress = 0;
            } else {
                self.no_progress += 1;
            }
            self.sq[to as usize] = self.sq[frm as usize];
            self.sq[frm as usize] = EMPTY;
        }
        self.ply += 1;
    }

    /// Fills `mv` with legal moves when ongoing. Returns a RES_* code.
    fn result(&self, mv: &mut Vec<(u8, u8)>) -> i16 {
        // TCGA/Taiwanese 40-ply no-capture-no-flip clock. NOTE: threefold
        // position-repetition (also a draw, see board.py) is NOT modeled here —
        // the Rust search is fed per-position with no repetition history. Known
        // parity gap; the reference Python board adjudicates self-play with full history.
        if self.no_progress >= 40 {
            return RES_DRAW;
        }
        self.legal_moves(mv);
        if mv.is_empty() {
            let mc = self.mover_color();
            if mc < 0 {
                return RES_DRAW;
            }
            return if (1 - mc) == 0 { RES_RED } else { RES_BLACK };
        }
        // Early adjudication of a decided game (parity with the reference rules
        // implementations): if the side NOT
        // to move has been wiped out (no piece on the board AND no face-down tile
        // left to flip) it can never act again, so the mover wins now instead of one
        // ply later. The scan short-circuits on the first face-down tile or
        // waiting-colored piece, so it costs ~nothing outside the wiped-out endgame.
        let mc = self.mover_color();
        if mc >= 0 {
            let waiting = 1 - mc;
            let waiting_alive = self
                .sq
                .iter()
                .any(|&c| c == DOWN || (is_piece(c) && code_color(c) == waiting));
            if !waiting_alive {
                return if mc == 0 { RES_RED } else { RES_BLACK };
            }
        }
        RES_ONGOING
    }
}

// ===================== Transposition table (rung B3) =====================
//
// Zobrist hash over (square contents, bag counts, side-to-move). Bag is part of the
// key because two positions with identical boards but different unrevealed sets have
// different flip dynamics → different values. The 40-ply no-progress clock is EXCLUDED
// (standard chess convention for the move counter) — accept a tiny artifact near the
// clock for many more TT hits. TT is probed/stored at DECISION nodes only; chance
// (flip) nodes return Star1 bounds that propagate into the decision value + flag, so a
// decision-node entry with an exact/lower/upper flag is a sound αβ bound even with
// expectimax children. TT lives per best_move call (fresh each move — no cross-move
// staleness); the win is within-search transposition (same position via move-order
// permutations), which is where most of the duplication is.

struct Zobrist {
    sq: [[u64; 16]; NSQ], // content: 0..13 piece code, 14 face-down, 15 empty
    bag: [[u64; 33]; 14], // count 0..32 per piece code
    side: [u64; 3],       // 0 red, 1 black, 2 unbound
}

static ZOB: OnceLock<Zobrist> = OnceLock::new();

fn zob() -> &'static Zobrist {
    ZOB.get_or_init(|| {
        // splitmix64 from a fixed seed → deterministic tables (reproducible search).
        let mut s: u64 = 0x1234_5678_9ABC_DEF0;
        let mut next = || {
            s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        let mut zb = Zobrist { sq: [[0; 16]; NSQ], bag: [[0; 33]; 14], side: [0; 3] };
        for i in 0..NSQ {
            for c in 0..16 {
                zb.sq[i][c] = next();
            }
        }
        for code in 0..14 {
            for c in 0..33 {
                zb.bag[code][c] = next();
            }
        }
        for s2 in 0..3 {
            zb.side[s2] = next();
        }
        zb
    })
}

fn zkey(st: &State) -> u64 {
    let z = zob();
    let mut h = 0u64;
    for i in 0..NSQ {
        let c = st.sq[i];
        let ci = if c == EMPTY {
            15
        } else if c == DOWN {
            14
        } else {
            c as usize
        };
        h ^= z.sq[i][ci];
    }
    for code in 0..14 {
        let cnt = (st.bag[code] as usize).min(32);
        h ^= z.bag[code][cnt];
    }
    let mc = st.mover_color();
    h ^= z.side[if mc < 0 { 2 } else { mc as usize }];
    h
}

#[derive(Clone, Copy)]
struct TtEntry {
    key: u64,
    value: f64,
    depth: i16,
    flag: u8, // 0 empty, 1 exact, 2 lower bound (fail-high), 3 upper bound (fail-low)
    best: (u8, u8),
}

const TT_EMPTY: TtEntry = TtEntry { key: 0, value: 0.0, depth: 0, flag: 0, best: (255, 255) };

/// Search-feature flags, decoded from the `features` bitmask passed by Python. New
/// rungs of the path-B search push each claim one bit; `features=0` reproduces the
/// pre-push engine byte-for-byte (the repo hard rule: flag with prior-behavior default).
#[derive(Clone, Copy)]
struct Feat {
    ordering: bool, // bit 0: MVV-LVA + killer moves + history heuristic
    tt: bool,       // bit 1: transposition table (decision nodes)
    lmr: bool,      // bit 2: late move reductions
    rep: bool,      // bit 3: repetition-aware search
    // --- eval terms (cheap-strength probe, 2026-06-18) ---
    cover_mat: bool, // bit 4: full-alive material — count covered (face-down) pieces at
                     // their PUBLIC bag value, so a flip never changes material (only a
                     // capture does). Removes the phantom "+material for revealing my own
                     // good piece" the revealed-only count creates.
    king_ctx: bool,  // bit 5: context-king-value — the general is worth more when fewer
                     // ENEMY soldiers are alive (only a soldier — or a cannon screen — can
                     // capture a general; with no enemy soldiers it is near-invulnerable).
                     // A context-dependent general value.
    vmob: bool,      // bit 6: value-aware mobility — weight each legal move by the moving
                     // piece's value (a chariot's options matter more than a soldier's),
                     // normalized by the average piece value so w_mob keeps its scale.
                     // Value-weighted mobility.
    gen_danger: bool, // bit 8: proximity+escape-aware general-danger term (successor to the
                      // adjacency-only king_danger). When set, REPLACES king_danger and is
                      // scaled by the w_king weight slot (unused in prod). See
                      // State::general_danger + the general-safety pathology doc.
    dom_val: bool,   // bit 7: adaptive domination value — a revealed piece is worth more as
                     // the alive (revealed + bag) enemy pieces that can CAPTURE it dwindle.
                     // Generalizes king_ctx to every piece: as a piece's dominators die or
                     // stay in the bag, it nears "immortal" (free attacker). Objective banqi
                     // mechanics, so no opponent-specificity; targets the endgame lattice
                     // collapse where static value is most wrong. Subsumes king_ctx — test it
                     // WITHOUT king_ctx to avoid double-counting the general.
}

impl Feat {
    fn from_bits(b: u32) -> Feat {
        Feat {
            ordering: b & 1 != 0,
            tt: b & 2 != 0,
            lmr: b & 4 != 0,
            rep: b & 8 != 0,
            cover_mat: b & 16 != 0,
            king_ctx: b & 32 != 0,
            vmob: b & 64 != 0,
            dom_val: b & 128 != 0,
            gen_danger: b & 256 != 0,
        }
    }
    fn none() -> Feat {
        Feat {
            ordering: false,
            tt: false,
            lmr: false,
            rep: false,
            cover_mat: false,
            king_ctx: false,
            vmob: false,
            dom_val: false,
            gen_danger: false,
        }
    }
}

struct Cfg {
    contempt: f64,
    root: i16,
    quiesce: bool,
    quiesce_max: i32,
    w_mob: f64,
    w_king: f64,
    values: [f64; 7],
    feat: Feat,
}

struct Ctx {
    nodes: u64,
    budget: u64,
    start: Instant,
    time_limit_ms: u64, // 0 = no wall-clock limit (node budget only)
    // Move-ordering scratch (used only when Feat::ordering; allocated either way —
    // a few KB, negligible — but never read on the features=0 path).
    killers: Vec<[(u8, u8); 2]>, // two killer slots per remaining-depth
    history: Vec<i32>,           // [NSQ*NSQ] quiet-move cutoff history
    tt: Vec<TtEntry>,            // transposition table (empty unless Feat::tt)
    tt_mask: usize,              // index mask = tt.len()-1 (power of two)
    path: Vec<u64>,              // ancestor zkeys on the current search path (Feat::rep)
}

impl Ctx {
    fn new(budget: u64, time_limit_ms: u64, max_depth: i32, tt_bits: u32) -> Ctx {
        let kd = (max_depth.max(1) + 2) as usize;
        let tt_size = if tt_bits > 0 { 1usize << tt_bits } else { 0 };
        Ctx {
            nodes: 0,
            budget,
            start: Instant::now(),
            time_limit_ms,
            killers: vec![[(255, 255); 2]; kd],
            history: vec![0; NSQ * NSQ],
            tt: vec![TT_EMPTY; tt_size],
            tt_mask: tt_size.wrapping_sub(1),
            path: Vec::with_capacity(64),
        }
    }

    /// Record a quiet-move beta-cutoff: promote it into the killer slots for this
    /// depth and bump its history score (depth² — deeper cutoffs weigh more).
    #[inline]
    fn record_cutoff(&mut self, m: (u8, u8), depth: i32) {
        let d = depth as usize;
        if d < self.killers.len() && self.killers[d][0] != m {
            self.killers[d][1] = self.killers[d][0];
            self.killers[d][0] = m;
        }
        self.history[m.0 as usize * NSQ + m.1 as usize] += depth * depth;
    }

    #[inline]
    fn tick(&mut self) -> Result<(), ()> {
        self.nodes += 1;
        if self.nodes > self.budget {
            return Err(());
        }
        // Wall-clock check is amortized: only every 1024 nodes (Instant::elapsed is
        // ~tens of ns but adding it to the per-node hot path measurably slows search).
        if self.time_limit_ms > 0 && (self.nodes & 1023) == 0
            && self.start.elapsed().as_millis() as u64 >= self.time_limit_ms
        {
            return Err(());
        }
        Ok(())
    }
}

fn terminal_value(st: &State, res: i16, cfg: &Cfg) -> f64 {
    if res == RES_DRAW {
        if cfg.contempt != 0.0 && cfg.root >= 0 {
            return if st.mover_color() == cfg.root {
                -cfg.contempt
            } else {
                cfg.contempt
            };
        }
        return 0.0;
    }
    let winner = if res == RES_RED { 0 } else { 1 };
    if winner == st.mover_color() {
        1.0
    } else {
        -1.0
    }
}

#[inline]
fn order_key(st: &State, m: (u8, u8)) -> i32 {
    if m.0 == m.1 {
        return -1;
    }
    let t = st.sq[m.1 as usize];
    if is_piece(t) {
        100 + VALUE[code_role(t)] as i32
    } else {
        0
    }
}

fn order(st: &State, mv: &mut [(u8, u8)]) {
    mv.sort_by(|a, b| order_key(st, *b).cmp(&order_key(st, *a)));
}

/// Richer ordering key (Feat::ordering): captures by MVV-LVA (victim value high,
/// attacker value as the tiebreak so a cheap attacker is preferred), then killer
/// moves, then quiet moves by history score; flips last. Higher = searched first.
#[inline]
fn order_key_rich(st: &State, m: (u8, u8), depth: i32, ctx: &Ctx) -> i32 {
    if m.0 == m.1 {
        return -1_000_000; // flips last
    }
    let t = st.sq[m.1 as usize];
    if is_piece(t) {
        let victim = VALUE[code_role(t)] as i32;
        let attacker = VALUE[code_role(st.sq[m.0 as usize]) ] as i32;
        return 1_000_000 + victim * 8 - attacker; // MVV-LVA
    }
    // quiet move: killer bonus, else history score
    let d = depth as usize;
    if d < ctx.killers.len() {
        if ctx.killers[d][0] == m {
            return 900_000;
        }
        if ctx.killers[d][1] == m {
            return 800_000;
        }
    }
    ctx.history[m.0 as usize * NSQ + m.1 as usize]
}

/// Order moves in place. Falls back to the plain MVV order when ordering is off, so
/// the features=0 path is byte-identical to the pre-push engine.
fn order_moves(st: &State, mv: &mut [(u8, u8)], depth: i32, cfg: &Cfg, ctx: &Ctx) {
    if cfg.feat.ordering {
        mv.sort_by(|a, b| order_key_rich(st, *b, depth, ctx).cmp(&order_key_rich(st, *a, depth, ctx)));
    } else {
        order(st, mv);
    }
}

fn quiesce(st: &State, mut alpha: f64, beta: f64, cfg: &Cfg, ctx: &mut Ctx, qdepth: i32) -> Result<f64, ()> {
    ctx.tick()?;
    let mut mv: Vec<(u8, u8)> = Vec::new();
    let res = st.result(&mut mv);
    if res != RES_ONGOING {
        return Ok(terminal_value(st, res, cfg));
    }
    let stand = st.eval(st.mover_color(), cfg.w_mob, cfg.w_king, &cfg.values, &cfg.feat);
    if stand >= beta || qdepth <= 0 {
        return Ok(stand);
    }
    if stand > alpha {
        alpha = stand;
    }
    let mc = st.mover_color();
    let mut caps: Vec<(i32, (u8, u8))> = Vec::new();
    for &m in &mv {
        if m.0 != m.1 {
            let t = st.sq[m.1 as usize];
            if is_piece(t) {
                caps.push((VALUE[code_role(t)] as i32, m));
            }
        }
    }
    let _ = mc;
    caps.sort_by(|a, b| b.0.cmp(&a.0));
    let mut best = stand;
    for (_, m) in caps {
        let mut child = st.clone();
        child.push(m.0, m.1, 0);
        let v = -quiesce(&child, -beta, -alpha, cfg, ctx, qdepth - 1)?;
        if v > best {
            best = v;
        }
        if best > alpha {
            alpha = best;
        }
        if alpha >= beta {
            break;
        }
    }
    Ok(best)
}

fn flip_value(st: &State, m: (u8, u8), depth: i32, alpha: f64, beta: f64, cfg: &Cfg, ctx: &mut Ctx) -> Result<f64, ()> {
    let total: u32 = st.bag.iter().sum();
    let (l, u) = (VMIN, VMAX);
    let mut vsum = 0.0;
    let mut rem = 1.0;
    for code in 0..14usize {
        let cnt = st.bag[code];
        if cnt == 0 {
            continue;
        }
        let p = cnt as f64 / total as f64;
        rem -= p;
        if rem < 0.0 {
            rem = 0.0;
        }
        let ai = (alpha - vsum - rem * u) / p;
        let bi = (beta - vsum - rem * l) / p;
        if ai >= u {
            return Ok(alpha);
        }
        if bi <= l {
            return Ok(beta);
        }
        let cl = if ai > l { ai } else { l };
        let cu = if bi < u { bi } else { u };
        let mut child = st.clone();
        child.push(m.0, m.1, code as i16);
        let v = -negamax(&child, depth - 1, -cu, -cl, cfg, ctx)?;
        if v <= ai {
            return Ok(alpha);
        }
        if v >= bi {
            return Ok(beta);
        }
        vsum += p * v;
    }
    Ok(vsum)
}

fn move_value(st: &State, m: (u8, u8), depth: i32, alpha: f64, beta: f64, cfg: &Cfg, ctx: &mut Ctx) -> Result<f64, ()> {
    if m.0 == m.1 {
        return flip_value(st, m, depth, alpha, beta, cfg, ctx);
    }
    let mut child = st.clone();
    child.push(m.0, m.1, 0);
    Ok(-negamax(&child, depth - 1, -beta, -alpha, cfg, ctx)?)
}

fn negamax(st: &State, depth: i32, mut alpha: f64, beta: f64, cfg: &Cfg, ctx: &mut Ctx) -> Result<f64, ()> {
    ctx.tick()?;
    let mut mv: Vec<(u8, u8)> = Vec::new();
    let res = st.result(&mut mv);
    if res != RES_ONGOING {
        return Ok(terminal_value(st, res, cfg));
    }
    if depth <= 0 {
        if cfg.quiesce {
            return quiesce(st, alpha, beta, cfg, ctx, cfg.quiesce_max);
        }
        return Ok(st.eval(st.mover_color(), cfg.w_mob, cfg.w_king, &cfg.values, &cfg.feat));
    }
    let alpha_orig = alpha;
    let key = if cfg.feat.tt || cfg.feat.rep { zkey(st) } else { 0 };
    // --- Repetition detection ---
    // A repeated zkey can only arise from quiet-move cycles (captures/flips change
    // material/bag → a different key), so any ancestor match on the path is a true
    // repetition. Score it as a draw (with contempt): the engine then avoids cycling
    // when winning and seeks it when losing — fixing the "shuffle into a draw blind"
    // pathology. (Search treats the first repetition as a draw, the standard
    // efficiency approximation of the threefold rule.)
    if cfg.feat.rep && ctx.path.iter().any(|&k| k == key) {
        return Ok(terminal_value(st, RES_DRAW, cfg));
    }
    // --- TT probe (decision node) ---
    let mut tt_move: Option<(u8, u8)> = None;
    if cfg.feat.tt {
        let e = ctx.tt[(key as usize) & ctx.tt_mask];
        if e.flag != 0 && e.key == key {
            if e.depth as i32 >= depth {
                match e.flag {
                    1 => return Ok(e.value),                          // exact
                    2 => {
                        if e.value >= beta {
                            return Ok(e.value); // lower bound
                        }
                    }
                    3 => {
                        if e.value <= alpha {
                            return Ok(e.value); // upper bound
                        }
                    }
                    _ => {}
                }
            }
            tt_move = Some(e.best); // hash move improves ordering even if too shallow to cut
        }
    }
    order_moves(st, &mut mv, depth, cfg, ctx);
    if let Some(tm) = tt_move {
        if let Some(pos) = mv.iter().position(|&x| x == tm) {
            let m = mv.remove(pos);
            mv.insert(0, m);
        }
    }
    if cfg.feat.rep {
        ctx.path.push(key);
    }
    let mut best = -INF;
    let mut best_m = mv[0];
    for (i, &m) in mv.iter().enumerate() {
        // Late move reduction: depth is the lever in Banqi (TT proved it), and LMR is
        // a *direct* depth-for-breadth trade. Reduce late, quiet (non-capture/flip)
        // moves to a shallower null-window probe; re-search at full depth only if the
        // probe beats alpha. Captures, flips (Star1 chance nodes), and the first few
        // ordered moves are always searched full.
        let is_flip = m.0 == m.1;
        let quiet = !is_flip && !is_piece(st.sq[m.1 as usize]);
        let v = if cfg.feat.lmr && quiet && i >= 3 && depth >= 3 {
            let mut child = st.clone();
            child.push(m.0, m.1, 0);
            let probe = -negamax(&child, depth - 2, -alpha - 1e-6, -alpha, cfg, ctx)?;
            if probe > alpha {
                -negamax(&child, depth - 1, -beta, -alpha, cfg, ctx)? // re-search full depth
            } else {
                probe
            }
        } else {
            move_value(st, m, depth, alpha, beta, cfg, ctx)?
        };
        if v > best {
            best = v;
            best_m = m;
        }
        if best > alpha {
            alpha = best;
        }
        if alpha >= beta {
            // Quiet (non-capture, non-flip) cutoffs feed killer/history ordering.
            if cfg.feat.ordering && m.0 != m.1 && !is_piece(st.sq[m.1 as usize]) {
                ctx.record_cutoff(m, depth);
            }
            break;
        }
    }
    if cfg.feat.rep {
        ctx.path.pop();
    }
    // --- TT store (depth-preferred replacement) ---
    if cfg.feat.tt {
        let flag = if best <= alpha_orig {
            3 // fail-low → upper bound
        } else if best >= beta {
            2 // fail-high → lower bound
        } else {
            1 // exact
        };
        let idx = (key as usize) & ctx.tt_mask;
        let cur = ctx.tt[idx];
        if cur.flag == 0 || cur.key != key || (cur.depth as i32) <= depth {
            ctx.tt[idx] = TtEntry { key, value: best, depth: depth as i16, flag, best: best_m };
        }
    }
    Ok(best)
}

fn best_at_depth(st: &State, depth: i32, cfg: &Cfg, ctx: &mut Ctx, hint: Option<(u8, u8)>) -> Result<Option<(u8, u8)>, ()> {
    let mut mv: Vec<(u8, u8)> = Vec::new();
    st.legal_moves(&mut mv);
    if mv.is_empty() {
        return Ok(None);
    }
    order_moves(st, &mut mv, depth, cfg, ctx);
    if let Some(h) = hint {
        if let Some(pos) = mv.iter().position(|&x| x == h) {
            mv.remove(pos);
            mv.insert(0, h);
        }
    }
    let mut best_val = -INF;
    let mut best = None;
    let mut alpha = VMIN;
    for &m in &mv {
        let v = move_value(st, m, depth, alpha, VMAX, cfg, ctx)?;
        if v > best_val {
            best_val = v;
            best = Some(m);
            if v > alpha {
                alpha = v;
            }
        }
    }
    Ok(best)
}

/// Like `best_at_depth` but returns the chosen move AND its root negamax value
/// (side-to-move perspective). Used by `search_value` for value distillation —
/// the label is "what the engine truly thinks", not the static leaf eval.
fn root_value_at_depth(st: &State, depth: i32, cfg: &Cfg, ctx: &mut Ctx, hint: Option<(u8, u8)>) -> Result<Option<((u8, u8), f64)>, ()> {
    let mut mv: Vec<(u8, u8)> = Vec::new();
    st.legal_moves(&mut mv);
    if mv.is_empty() {
        return Ok(None);
    }
    order_moves(st, &mut mv, depth, cfg, ctx);
    if let Some(h) = hint {
        if let Some(pos) = mv.iter().position(|&x| x == h) {
            mv.remove(pos);
            mv.insert(0, h);
        }
    }
    let mut best_val = -INF;
    let mut best = None;
    let mut alpha = VMIN;
    for &m in &mv {
        let v = move_value(st, m, depth, alpha, VMAX, cfg, ctx)?;
        if v > best_val {
            best_val = v;
            best = Some(m);
            if v > alpha {
                alpha = v;
            }
        }
    }
    Ok(best.map(|m| (m, best_val)))
}

/// Zobrist key for a position (board + bag + side-to-move), exposed so callers can
/// compute the repetition window's keys to seed `best_move`'s `rep_history`. Uses the
/// same tables as the search, so a seeded key matches the in-search key for the same
/// (board, bag, side-to-move). `no_progress` is irrelevant to the key (set to 0).
pub fn zkey_for(squares: Vec<i16>, bag: Vec<u32>, first_color: i16, ply: u32) -> u64 {
    zkey(&make_state(squares, bag, first_color, ply, 0))
}

fn make_state(squares: Vec<i16>, bag: Vec<u32>, first_color: i16, ply: u32, no_progress: u32) -> State {
    let mut sq = [EMPTY; NSQ];
    for (i, &c) in squares.iter().enumerate().take(NSQ) {
        sq[i] = c;
    }
    let mut b = [0u32; 14];
    for (i, &c) in bag.iter().enumerate().take(14) {
        b[i] = c;
    }
    State { sq, bag: b, first_color, ply, no_progress }
}

/// Node-budgeted iterative deepening. Returns (from, to); a flip is from==to.
pub fn best_move(
    squares: Vec<i16>,
    bag: Vec<u32>,
    first_color: i16,
    ply: u32,
    no_progress: u32,
    node_budget: u64,
    contempt: f64,
    quiesce_on: bool,
    max_depth: i32,
    w_mob: f64,
    w_king: f64,
    values: Vec<f64>,
    time_ms: u64,
    features: u32,
    rep_history: Vec<u64>,
) -> (u8, u8) {
    let st = make_state(squares, bag, first_color, ply, no_progress);
    let mut mv: Vec<(u8, u8)> = Vec::new();
    st.legal_moves(&mut mv);
    if mv.is_empty() {
        return (255, 255);
    }
    order(&st, &mut mv);
    let vals = to_values(&values);
    let cfg = Cfg { contempt, root: st.mover_color(), quiesce: quiesce_on, quiesce_max: 8, w_mob, w_king, values: vals, feat: Feat::from_bits(features) };
    let mut ctx = Ctx::new(node_budget, time_ms, max_depth, if features & 2 != 0 { 18 } else { 0 });
    // Seed the path with the root key AND the GAME's repetition window (zkeys of every
    // position seen since the last irreversible move, supplied by the caller). The
    // in-search path alone only catches a cycle once it fully closes inside the tree —
    // for a 4-ply chase that needs depth ≥4, often unreachable under budget (esp. with
    // flip branching), so the engine shuffles into a threefold blind. Seeding the
    // window makes re-entering ANY already-seen position a detected repetition at depth
    // 1–2, so contempt steers the engine off the perpetual when ahead (and toward it
    // when losing). Empty rep_history = prior behavior (the repo flag rule). Persists
    // across ID depths (negamax push/pop balance above these seeded entries).
    if cfg.feat.rep {
        ctx.path.push(zkey(&st));
        for k in &rep_history {
            ctx.path.push(*k);
        }
    }
    let mut best = mv[0];
    let mut hint: Option<(u8, u8)> = None;
    for depth in 1..=max_depth {
        match best_at_depth(&st, depth, &cfg, &mut ctx, hint) {
            Ok(Some(m)) => {
                best = m;
                hint = Some(m);
            }
            _ => break,
        }
    }
    best
}

/// Like `best_move`, but the caller supplies the repetition WINDOW as a FEN at the last
/// irreversible move (capture/flip) plus the quiet `window_moves` played since. We replay
/// them to (a) reach the current root and (b) collect each intermediate position's zkey as
/// `rep_history`, which `best_move` seeds so the search detects threefold from GAME history
/// (not just within its own tree). This is how the UCI binary turns `position fen <window-
/// start> moves <...>` into repetition-aware play. The window is reversible by construction
/// (a capture/flip would have started a new window), so every move is quiet: replay with
/// `revealed = -1`. `first_color` is the side to move at the window start; ply replays from 0
/// so the final mover (and every zkey's side bit) is correct.
#[allow(clippy::too_many_arguments)]
pub fn best_move_with_moves(
    squares: Vec<i16>,
    bag: Vec<u32>,
    first_color: i16,
    no_progress: u32,
    window_moves: Vec<(u8, u8)>,
    node_budget: u64,
    contempt: f64,
    quiesce_on: bool,
    max_depth: i32,
    w_mob: f64,
    w_king: f64,
    values: Vec<f64>,
    time_ms: u64,
    features: u32,
) -> (u8, u8) {
    let mut st = make_state(squares, bag, first_color, 0, no_progress);
    let mut rep_history: Vec<u64> = Vec::with_capacity(window_moves.len());
    for (frm, to) in window_moves {
        rep_history.push(zkey(&st));
        st.push(frm, to, -1); // quiet move (no flip/capture inside a repetition window)
    }
    best_move(
        st.sq.to_vec(),
        st.bag.to_vec(),
        st.first_color,
        st.ply,
        st.no_progress,
        node_budget,
        contempt,
        quiesce_on,
        max_depth,
        w_mob,
        w_king,
        values,
        time_ms,
        features,
        rep_history,
    )
}

/// Node-budgeted iterative-deepening ROOT VALUE (side-to-move perspective) under the
/// full engine Cfg (quiescence, mobility, contempt=0). This is the value-distillation
/// label: "what the engine thinks at budget B", returned from the deepest completed
/// depth. Far cheaper than unbudgeted `negamax_value` (Star1 + ordering + ID) and a
/// stronger target than the static leaf eval. Returns a value in [-1, 1].
pub fn search_value(
    squares: Vec<i16>,
    bag: Vec<u32>,
    first_color: i16,
    ply: u32,
    no_progress: u32,
    node_budget: u64,
    quiesce_on: bool,
    max_depth: i32,
    w_mob: f64,
    w_king: f64,
    values: Vec<f64>,
) -> f64 {
    let st = make_state(squares, bag, first_color, ply, no_progress);
    let vals = to_values(&values);
    let cfg = Cfg { contempt: 0.0, root: st.mover_color(), quiesce: quiesce_on, quiesce_max: 8, w_mob, w_king, values: vals, feat: Feat::none() };
    // Terminal at the root: return the exact terminal value (no search needed).
    let mut mv: Vec<(u8, u8)> = Vec::new();
    let res = st.result(&mut mv);
    if res != RES_ONGOING {
        return terminal_value(&st, res, &cfg);
    }
    let mut ctx = Ctx::new(node_budget, 0, max_depth, 0);
    // Fallback if even depth-1 blows the budget: the static eval.
    let mut value = st.eval(st.mover_color(), w_mob, w_king, &vals, &cfg.feat);
    let mut hint: Option<(u8, u8)> = None;
    for depth in 1..=max_depth {
        match root_value_at_depth(&st, depth, &cfg, &mut ctx, hint) {
            Ok(Some((m, v))) => {
                value = v;
                hint = Some(m);
            }
            _ => break,
        }
    }
    value
}

/// DIAGNOSTIC (not on any shipped path): every root move's EXACT value under the full
/// shipped Cfg+features, at the deepest depth completed within budget. Unlike `best_move`
/// it does NOT narrow alpha across root siblings, so each move gets a true value rather
/// than an αβ upper bound — the introspection needed to tell the HORIZON effect (an
/// unavoidable loss pushed past the search depth, so a doomed-piece move scores a phantom
/// gain) apart from CORRECT abandonment (all defenses score equally lost). Flips get their
/// Star1 expectation via `move_value`. Returns (from, to, value, depth_reached) per move.
#[allow(clippy::too_many_arguments)]
pub fn root_move_values(
    squares: Vec<i16>,
    bag: Vec<u32>,
    first_color: i16,
    ply: u32,
    no_progress: u32,
    node_budget: u64,
    contempt: f64,
    quiesce_on: bool,
    max_depth: i32,
    w_mob: f64,
    w_king: f64,
    values: Vec<f64>,
    time_ms: u64,
    features: u32,
) -> Vec<(u8, u8, f64, i32)> {
    let st = make_state(squares, bag, first_color, ply, no_progress);
    let vals = to_values(&values);
    let cfg = Cfg { contempt, root: st.mover_color(), quiesce: quiesce_on, quiesce_max: 8, w_mob, w_king, values: vals, feat: Feat::from_bits(features) };
    let mut ctx = Ctx::new(node_budget, time_ms, max_depth, if features & 2 != 0 { 18 } else { 0 });
    let mut mv: Vec<(u8, u8)> = Vec::new();
    st.legal_moves(&mut mv);
    if mv.is_empty() {
        return Vec::new();
    }
    // Mirror best_move's repetition seeding: push the root key once; negamax balances
    // its own push/pop above this entry across all ID depths.
    if cfg.feat.rep {
        ctx.path.push(zkey(&st));
    }
    let mut completed: Vec<(u8, u8, f64)> = Vec::new();
    let mut depth_reached = 0;
    let mut hint: Option<(u8, u8)> = None;
    for depth in 1..=max_depth {
        order_moves(&st, &mut mv, depth, &cfg, &mut ctx);
        if let Some(h) = hint {
            if let Some(pos) = mv.iter().position(|&x| x == h) {
                mv.remove(pos);
                mv.insert(0, h);
            }
        }
        let mut this: Vec<(u8, u8, f64)> = Vec::with_capacity(mv.len());
        let mut aborted = false;
        for &m in &mv {
            // Full window per move (no alpha narrowing across siblings) → exact values.
            match move_value(&st, m, depth, VMIN, VMAX, &cfg, &mut ctx) {
                Ok(v) => this.push((m.0, m.1, v)),
                Err(_) => {
                    aborted = true;
                    break;
                }
            }
        }
        if aborted {
            break;
        }
        this.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        hint = Some((this[0].0, this[0].1));
        completed = this;
        depth_reached = depth;
    }
    completed.into_iter().map(|(f, t, v)| (f, t, v, depth_reached)).collect()
}

/// Full-window, no-quiescence, no-contempt negamax value — for the parity test
/// against Python's unpruned `_expectimax` oracle (a value that's order-independent).
pub fn negamax_value(squares: Vec<i16>, bag: Vec<u32>, first_color: i16, ply: u32, no_progress: u32, depth: i32) -> f64 {
    let st = make_state(squares, bag, first_color, ply, no_progress);
    let cfg = Cfg { contempt: 0.0, root: -1, quiesce: false, quiesce_max: 8, w_mob: W_MOB, w_king: 0.0, values: VALUE, feat: Feat::none() };
    let mut ctx = Ctx::new(u64::MAX, 0, depth, 0);
    negamax(&st, depth, -2.0, 2.0, &cfg, &mut ctx).unwrap_or(0.0)
}

/// Legal moves as (from, to) pairs — for movegen-set parity vs Python.
pub fn legal_moves(squares: Vec<i16>, bag: Vec<u32>, first_color: i16, ply: u32, no_progress: u32) -> Vec<(u8, u8)> {
    let st = make_state(squares, bag, first_color, ply, no_progress);
    let mut mv: Vec<(u8, u8)> = Vec::new();
    st.legal_moves(&mut mv);
    mv
}

// ===================== Nondeterministic MCTS (rung 3) =====================
//
// UCB1 tree search over the existing State. A decision node picks an action by
// UCB; a FLIP action is a chance edge whose outcome (the revealed piece) is
// SAMPLED from the bag — vs Star1's full-width expansion. That is the whole point
// versus the alpha-beta engine: spend visits selectively instead of widening over
// all ~14 outcomes at every ply. Leaf value = the static eval (truncated rollout,
// DarkKnight-style early-playout-termination). Backup is negamax-signed: each edge
// stores the mean value to the player to move at the node that owns it, so a
// child's value is negated one level up. The tree alternates correctly because
// every action (move or flip) advances the ply and flips the side to move.

/// splitmix64 — tiny deterministic PRNG so games are reproducible given a seed
/// (keeps the CRN harness usable; no external crate).
struct Rng(u64);
impl Rng {
    #[inline]
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    #[inline]
    fn below(&mut self, n: u32) -> u32 {
        (self.next_u64() % n as u64) as u32
    }
}

struct Edge {
    action: (u8, u8),
    n: u32,
    w: f64,                  // sum of value-to-owner over visits through this edge
    kids: Vec<(i16, usize)>, // revealed_code -> child idx (single (0,_) entry for a move)
}

struct MNode {
    state: State,
    edges: Vec<Edge>,
    untried: Vec<(u8, u8)>,
    n: u32,
    terminal: Option<f64>,   // terminal value from THIS node's mover perspective
}

struct Tree {
    nodes: Vec<MNode>,
    cfg: Cfg,
    c_uct: f64,
    leaf_depth: i32, // 0 = static eval leaf; >0 = short alpha-beta (with quiescence) leaf
}

impl Tree {
    #[inline]
    fn leaf_value(&self, st: &State) -> f64 {
        if self.leaf_depth > 0 {
            // Tactical leaf: a short alpha-beta search gives MCTS the capture-depth a
            // 1-ply static eval is blind to. Value is from st's mover perspective.
            let mut ctx = Ctx::new(u64::MAX, 0, self.leaf_depth, 0);
            negamax(st, self.leaf_depth, -2.0, 2.0, &self.cfg, &mut ctx).unwrap_or(0.0)
        } else {
            st.eval(st.mover_color(), self.cfg.w_mob, self.cfg.w_king, &self.cfg.values, &self.cfg.feat)
        }
    }

    fn new_node(&mut self, st: State) -> usize {
        let mut mv: Vec<(u8, u8)> = Vec::new();
        let res = st.result(&mut mv);
        let terminal = if res != RES_ONGOING {
            Some(terminal_value(&st, res, &self.cfg))
        } else {
            order(&st, &mut mv); // expand promising actions first
            None
        };
        let untried = if terminal.is_some() { Vec::new() } else { mv };
        self.nodes.push(MNode { state: st, edges: Vec::new(), untried, n: 0, terminal });
        self.nodes.len() - 1
    }

    /// Resolve an edge to a child, sampling the revealed code for a flip. Returns
    /// (child_idx, is_new) — is_new flags a never-before-seen chance outcome.
    fn resolve_child(&mut self, node_idx: usize, edge_idx: usize, rng: &mut Rng) -> (usize, bool) {
        let action = self.nodes[node_idx].edges[edge_idx].action;
        let code: i16 = if action.0 == action.1 {
            let st = &self.nodes[node_idx].state;
            let total: u32 = st.bag.iter().sum();
            let mut r = rng.below(total);
            let mut chosen = 0i16;
            for c in 0..14usize {
                let cnt = st.bag[c];
                if r < cnt {
                    chosen = c as i16;
                    break;
                }
                r -= cnt;
            }
            chosen
        } else {
            0
        };
        if let Some(&(_, idx)) = self.nodes[node_idx].edges[edge_idx].kids.iter().find(|&&(c, _)| c == code) {
            return (idx, false);
        }
        let mut child = self.nodes[node_idx].state.clone();
        child.push(action.0, action.1, code);
        let cidx = self.new_node(child);
        self.nodes[node_idx].edges[edge_idx].kids.push((code, cidx));
        (cidx, true)
    }

    fn select_edge(&self, node_idx: usize) -> usize {
        let node = &self.nodes[node_idx];
        let ln = (node.n.max(1) as f64).ln();
        let mut best = 0usize;
        let mut best_score = f64::NEG_INFINITY;
        for (i, e) in node.edges.iter().enumerate() {
            let q = e.w / e.n as f64;
            let u = self.c_uct * (ln / e.n as f64).sqrt();
            let s = q + u;
            if s > best_score {
                best_score = s;
                best = i;
            }
        }
        best
    }

    /// One simulation from node_idx. Returns the value from node_idx's mover
    /// perspective, and updates edge/node statistics along the path.
    fn simulate(&mut self, node_idx: usize, depth: i32, rng: &mut Rng) -> f64 {
        if let Some(v) = self.nodes[node_idx].terminal {
            return v;
        }
        if depth <= 0 {
            return self.leaf_value(&self.nodes[node_idx].state);
        }
        // Expand one untried action, evaluate the new child as a leaf.
        if let Some(action) = self.nodes[node_idx].untried.pop() {
            let edge_idx = self.nodes[node_idx].edges.len();
            self.nodes[node_idx].edges.push(Edge { action, n: 0, w: 0.0, kids: Vec::new() });
            let (cidx, _) = self.resolve_child(node_idx, edge_idx, rng);
            let cv = self.nodes[cidx].terminal.unwrap_or_else(|| self.leaf_value(&self.nodes[cidx].state));
            let val = -cv; // child mover is the opponent
            let e = &mut self.nodes[node_idx].edges[edge_idx];
            e.n += 1;
            e.w += val;
            self.nodes[node_idx].n += 1;
            return val;
        }
        // Fully expanded: UCB-select, resolve chance, descend (or treat a fresh
        // chance outcome as a leaf).
        let edge_idx = self.select_edge(node_idx);
        let (cidx, is_new) = self.resolve_child(node_idx, edge_idx, rng);
        let val = if is_new {
            -self.nodes[cidx].terminal.unwrap_or_else(|| self.leaf_value(&self.nodes[cidx].state))
        } else {
            -self.simulate(cidx, depth - 1, rng)
        };
        let e = &mut self.nodes[node_idx].edges[edge_idx];
        e.n += 1;
        e.w += val;
        self.nodes[node_idx].n += 1;
        val
    }
}

/// Nondeterministic MCTS move. `sims` simulations, UCB constant `c_uct`, tree-depth
/// cap `max_depth`, deterministic `seed`. Returns (from, to); a flip is from==to.
pub fn mcts_move(
    squares: Vec<i16>,
    bag: Vec<u32>,
    first_color: i16,
    ply: u32,
    no_progress: u32,
    sims: u32,
    c_uct: f64,
    max_depth: i32,
    leaf_depth: i32,
    seed: u64,
    w_mob: f64,
    w_king: f64,
    values: Vec<f64>,
) -> (u8, u8) {
    let st = make_state(squares, bag, first_color, ply, no_progress);
    let cfg = Cfg {
        contempt: 0.0,
        root: st.mover_color(),
        quiesce: leaf_depth > 0, // tactical leaf uses quiescence; static leaf doesn't
        quiesce_max: 8,
        w_mob,
        w_king,
        values: to_values(&values),
        feat: Feat::none(),
    };
    let mut tree = Tree { nodes: Vec::new(), cfg, c_uct, leaf_depth };
    let root = tree.new_node(st);
    if tree.nodes[root].terminal.is_some() || tree.nodes[root].untried.is_empty() {
        return (255, 255);
    }
    let mut rng = Rng(seed ^ 0x0123_4567_89AB_CDEF);
    for _ in 0..sims {
        tree.simulate(root, max_depth, &mut rng);
    }
    // Robust child: the most-visited root edge.
    let mut best = (255u8, 255u8);
    let mut best_n = 0u32;
    for e in &tree.nodes[root].edges {
        if e.n > best_n {
            best_n = e.n;
            best = e.action;
        }
    }
    best
}

// ===================== FEN + UCI (the redacted serving contract) =====================
//
// Any driver that builds the redacted Banqi FEN and this engine MUST agree
// on this redacted Banqi FEN. The engine NEVER learns a face-down piece's identity — all
// face-down squares are a single `X` (no colour), and the unrevealed `pool` carries only
// the PUBLIC per-(colour,role) counts (derivable by both seats), which the Star1 search
// needs to reason over flips.
//
//   FEN := "<board> <turn> <pool> <clock> <movenum>"
//     board: 4 ranks separated by '/', TOP-FIRST (rank 4, 3, 2, 1). Each rank is 8 files
//            a..h left-to-right: a revealed piece is its role letter (UPPER=red, lower=
//            black) from {G general, A advisor, E elephant, R chariot, H horse, C cannon,
//            S soldier}; a face-down piece is 'X'; empties are a digit run (FEN-style).
//     turn:  'r' red to move, 'b' black to move, '-' unbound (opening, no piece flipped yet).
//     pool:  unrevealed counts as <letter><count> pairs (red UPPER then black lower),
//            e.g. "G1A2E2R2H2C2S5g1a2e2r2h2c2s5"; '-' when empty. Σpool == #face-down.
//     clock: no-progress ply count (0..40). movenum: 1-based (cosmetic for search).

const ROLE_LETTERS: [u8; 7] = [b'G', b'A', b'E', b'R', b'H', b'C', b'S'];

fn letter_to_code(ch: u8) -> Option<i16> {
    let role = ROLE_LETTERS.iter().position(|&l| l == ch.to_ascii_uppercase())? as i16;
    let color = if ch.is_ascii_uppercase() { 0 } else { 1 };
    Some(color * 7 + role)
}

fn code_to_letter(code: i16) -> u8 {
    let l = ROLE_LETTERS[(code % 7) as usize];
    if code / 7 == 0 { l } else { l.to_ascii_lowercase() }
}

/// Parsed position for the UCI binary: (squares[32], bag[14], first_color, no_progress).
/// ply is always reconstructed as 0 (mover_color == first_color), which is search-correct
/// (the real ply/movenum only affect display; the no-progress clock is carried explicitly).
pub struct Parsed {
    pub squares: Vec<i16>,
    pub bag: Vec<u32>,
    pub first_color: i16,
    pub no_progress: u32,
}

/// Parse a redacted Banqi FEN. Returns None on any malformed field.
pub fn state_from_fen(fen: &str) -> Option<Parsed> {
    let parts: Vec<&str> = fen.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }
    // board
    let ranks: Vec<&str> = parts[0].split('/').collect();
    if ranks.len() != H as usize {
        return None;
    }
    let mut squares = vec![EMPTY; NSQ];
    for (i, rank_str) in ranks.iter().enumerate() {
        let rank = H as usize - i; // parts[0] is rank H (top)
        let mut file = 0usize;
        for ch in rank_str.bytes() {
            if file >= W as usize {
                return None;
            }
            let idx = file + (rank - 1) * W as usize;
            if ch.is_ascii_digit() {
                file += (ch - b'0') as usize;
                continue;
            } else if ch == b'X' {
                squares[idx] = DOWN;
            } else {
                squares[idx] = letter_to_code(ch)?;
            }
            file += 1;
        }
        if file != W as usize {
            return None;
        }
    }
    // turn
    let first_color = match parts[1] {
        "r" => 0,
        "b" => 1,
        "-" => -1,
        _ => return None,
    };
    // pool
    let mut bag = vec![0u32; 14];
    if parts[2] != "-" {
        let bytes = parts[2].as_bytes();
        let mut j = 0;
        while j < bytes.len() {
            let code = letter_to_code(bytes[j])? as usize;
            j += 1;
            let mut n = 0u32;
            let mut saw_digit = false;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                n = n * 10 + (bytes[j] - b'0') as u32;
                j += 1;
                saw_digit = true;
            }
            if !saw_digit {
                return None;
            }
            bag[code] = n;
        }
    }
    let no_progress: u32 = parts[3].parse().ok()?;
    Some(Parsed { squares, bag, first_color, no_progress })
}

/// Encode a position back to a redacted FEN (for tests / debugging; the platform owns the
/// canonical encoder). `ply` is only used to derive movenum cosmetically.
pub fn fen_from_state(squares: &[i16], bag: &[u32], first_color: i16, no_progress: u32, movenum: u32) -> String {
    let mut board = String::new();
    for i in 0..H as usize {
        let rank = H as usize - i;
        let mut empties = 0;
        for file in 0..W as usize {
            let c = squares[file + (rank - 1) * W as usize];
            if c == EMPTY {
                empties += 1;
                continue;
            }
            if empties > 0 {
                board.push_str(&empties.to_string());
                empties = 0;
            }
            if c == DOWN {
                board.push('X');
            } else {
                board.push(code_to_letter(c) as char);
            }
        }
        if empties > 0 {
            board.push_str(&empties.to_string());
        }
        if i + 1 < H as usize {
            board.push('/');
        }
    }
    let turn = match first_color {
        0 => "r",
        1 => "b",
        _ => "-",
    };
    let mut pool = String::new();
    for code in 0..14usize {
        if bag[code] > 0 {
            pool.push(code_to_letter(code as i16) as char);
            pool.push_str(&bag[code].to_string());
        }
    }
    if pool.is_empty() {
        pool.push('-');
    }
    format!("{board} {turn} {pool} {no_progress} {movenum}")
}

/// Square index (0..31) -> UCI token: file a..h + rank digit 0..3 (rank-1, 0-indexed).
pub fn square_to_uci(i: usize) -> String {
    let file = (b'a' + (i % W as usize) as u8) as char;
    let rank = (b'0' + (i / W as usize) as u8) as char;
    format!("{file}{rank}")
}

fn uci_to_square(s: &[u8]) -> Option<u8> {
    if s.len() != 2 {
        return None;
    }
    let file = s[0].checked_sub(b'a')?;
    let rank = s[1].checked_sub(b'0')?;
    if file >= W as u8 || rank >= H as u8 {
        return None;
    }
    Some(file + rank * W as u8)
}

/// (from, to) -> UCI; a flip is from==to (e.g. "a0a0").
pub fn move_to_uci(m: (u8, u8)) -> String {
    format!("{}{}", square_to_uci(m.0 as usize), square_to_uci(m.1 as usize))
}

/// UCI "a0b0" -> (from, to). None if malformed.
pub fn uci_to_move(s: &str) -> Option<(u8, u8)> {
    let b = s.trim().as_bytes();
    if b.len() != 4 {
        return None;
    }
    Some((uci_to_square(&b[0..2])?, uci_to_square(&b[2..4])?))
}

#[cfg(test)]
mod fen_tests {
    use super::*;

    #[test]
    fn opening_all_face_down_round_trips() {
        // All 32 face-down, unbound, full pool.
        let pool = "G1A2E2R2H2C2S5g1a2e2r2h2c2s5";
        let fen = format!("XXXXXXXX/XXXXXXXX/XXXXXXXX/XXXXXXXX - {pool} 0 1");
        let p = state_from_fen(&fen).expect("parse");
        assert_eq!(p.first_color, -1);
        assert_eq!(p.no_progress, 0);
        assert!(p.squares.iter().all(|&c| c == DOWN));
        assert_eq!(p.bag.iter().sum::<u32>(), 32);
        // re-encode equals input board+pool
        let re = fen_from_state(&p.squares, &p.bag, p.first_color, p.no_progress, 1);
        assert_eq!(re, fen);
    }

    #[test]
    fn mixed_board_round_trips() {
        // rank4: red general a4, then 7 empty; rank3: black soldier h3; rank2: face-down a2;
        // rank1: empty.
        let fen = "G7/7s/X7/8 r G1s4 3 5";
        let p = state_from_fen(fen).expect("parse");
        assert_eq!(p.first_color, 0);
        assert_eq!(p.no_progress, 3);
        // a4 = idx 24 = red general (code 0)
        assert_eq!(p.squares[24], 0);
        // h3 = idx 23 = black soldier (code 13)
        assert_eq!(p.squares[23], 13);
        // a2 = idx 8 = face-down
        assert_eq!(p.squares[8], DOWN);
        assert_eq!(p.bag[0], 1); // red general in pool
        assert_eq!(p.bag[13], 4); // 4 black soldiers in pool
        let re = fen_from_state(&p.squares, &p.bag, p.first_color, p.no_progress, 5);
        assert_eq!(re, fen);
    }

    #[test]
    fn uci_codec() {
        assert_eq!(square_to_uci(0), "a0");
        assert_eq!(square_to_uci(7), "h0");
        assert_eq!(square_to_uci(31), "h3");
        assert_eq!(move_to_uci((0, 1)), "a0b0");
        assert_eq!(uci_to_move("a0b0"), Some((0, 1)));
        assert_eq!(uci_to_move("a0a0"), Some((0, 0))); // flip
        assert_eq!(uci_to_move("z9z9"), None);
    }
}

#[cfg(test)]
mod result_tests {
    use super::*;

    // Piece codes: color*7 + role. red=0, black=1; roles 0 general … 6 soldier.
    const BLACK_GENERAL: i16 = 7; // 1*7 + 0
    const BLACK_SOLDIER: i16 = 13; // 1*7 + 6
    const RED_GENERAL: i16 = 0; // 0*7 + 0

    fn at(squares: &mut Vec<i16>, idx: usize, code: i16) {
        squares[idx] = code;
    }

    #[test]
    fn wiped_out_waiting_side_ends_at_once() {
        // BLACK to move (ply odd, red bound first) on a fully-revealed, all-black
        // board: RED (the waiting side) has no piece and no face-down tile to flip,
        // so it can never act again. BLACK wins now, before BLACK has to move.
        let mut squares = vec![EMPTY; NSQ];
        at(&mut squares, 0, BLACK_GENERAL); // a1
        at(&mut squares, 31, BLACK_SOLDIER); // h4
        let st = make_state(squares, vec![], 0, 1, 0);
        let mut probe = Vec::new();
        st.legal_moves(&mut probe);
        assert!(!probe.is_empty()); // BLACK still has moves...
        let mut mv = Vec::new();
        assert_eq!(st.result(&mut mv), RES_BLACK); // ...but the game is decided

        // A lone face-down tile keeps RED potentially alive (it could flip) → ongoing.
        let mut sq2 = vec![EMPTY; NSQ];
        at(&mut sq2, 0, BLACK_GENERAL);
        at(&mut sq2, 31, DOWN);
        let st2 = make_state(sq2, vec![], 0, 1, 0);
        let mut mv2 = Vec::new();
        assert_eq!(st2.result(&mut mv2), RES_ONGOING);

        // RED still has a piece on a fully-revealed board → ongoing.
        let mut sq3 = vec![EMPTY; NSQ];
        at(&mut sq3, 0, BLACK_GENERAL);
        at(&mut sq3, 16, RED_GENERAL); // a3
        let st3 = make_state(sq3, vec![], 0, 1, 0);
        let mut mv3 = Vec::new();
        assert_eq!(st3.result(&mut mv3), RES_ONGOING);
    }
}


