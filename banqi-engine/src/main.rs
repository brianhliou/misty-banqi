//! MistyBanqi — standalone Banqi (Chinese Dark Chess) UCI engine, v0.2.1.
//!
//! v0.2.1: general-safety eval term (`gen_danger`, Feat bit 256, weighted by w_king=28).
//! Proximity+escape-aware: penalizes the general being threatened by an enemy soldier along
//! an open line, and being cornered. Measured (~385-game paired bakeoffs): no
//! regression + own-general-loss 35.5%→26% (≈2.7σ); verified to execute the "make-luft"
//! save (flip a face-down neighbor to give a boxed general a 2×2 escape). See the README.
//!
//! v0.2.0: the "cheap-strength" eval — four flagged eval terms (covered-piece material +
//! context general value + value-aware mobility + adaptive domination) plus a corrected
//! value table; measured +16.6% / losses −67% over ~400-game paired bakeoffs.
//!
//! The αβ + Star1 + TT + repetition search core lives in `banqi_rust/src/engine.rs` (shared
//! with the PyO3 Python bindings, included here via `#[path]`); this file is a tiny UCI
//! front-end. Drive it as a UCI subprocess: feed a (redacted) Banqi FEN, read `bestmove`.
//!
//! Protocol (subset of UCI):
//!   uci               -> id name/author, uciok
//!   isready           -> readyok
//!   ucinewgame        -> clear position
//!   position fen <FEN> [moves ...] -> store the (redacted) Banqi FEN. If a trailing
//!                         "moves ..." list is present, the FEN is the position at the last
//!                         irreversible move (capture/flip) and the moves are the quiet plies
//!                         since; we replay them to seed the search's repetition history so the
//!                         engine avoids/seeks threefold (perpetual-chase) draws instead of
//!                         shuffling into them blind. FEN-only (no moves) = prior behavior.
//!   go [movetime <ms>] [nodes <n>]  -> search, emit "bestmove <uci>" (or "(none)")
//!   quit              -> exit
//!
//! The FEN/move contract is defined in `engine.rs` (see its FEN+UCI section). The engine
//! never sees a face-down piece's identity.

#[path = "../../banqi_rust/src/engine.rs"]
#[allow(dead_code)] // engine.rs also exposes the PyO3-facing entry points, unused here
mod engine;

use std::io::{self, BufRead, Write};

const ENGINE_NAME: &str = "MistyBanqi 0.2.1";
const DEFAULT_MOVETIME_MS: u64 = 1000;
// Search/eval features (banqi_rust Feat bitmask): TT(2) + repetition(8) + cover_mat(16) +
// king_ctx(32) + value-aware mobility(64) + adaptive domination value(128) + gen_danger(256)
// = 506. The +256 is the v0.2.1 general-safety term (proximity+escape-aware), weighted by
// w_king below. Ordering(1)/LMR(4) stay off (neutral in testing).
const FEATURES: u32 = 506;
// Corrected value table (gen,adv,ele,cha,hor,can,sol): the original had cannon UNDER-valued
// (12) and chariot OVER-valued (14) — a real eval bug. Cannon is banqi's most tactically
// dominant piece (screen capture); chariot is mid-ladder. This fix alone was ~+10%.
const DEFAULT_VALUES: [f64; 7] = [30.0, 14.0, 11.0, 9.0, 7.0, 16.0, 4.0];

fn search_best(
    p: &engine::Parsed,
    window_moves: &[(u8, u8)],
    movetime_ms: u64,
    node_budget: u64,
) -> String {
    // `p` is the position at the window start (last irreversible move); `window_moves`
    // are the quiet plies since. best_move_with_moves replays them to seed the search's
    // repetition history, so the engine sees threefold from real game history rather than
    // shuffling into it. No `moves` ⇒ empty window ⇒ identical to the prior FEN-only search.
    let (frm, to) = engine::best_move_with_moves(
        p.squares.clone(),
        p.bag.clone(),
        p.first_color,
        p.no_progress,
        window_moves.to_vec(),
        node_budget,
        0.1,  // contempt: seek decisive play
        true, // quiescence
        24,   // max depth
        0.8,  // w_mob
        28.0, // w_king — weight of the gen_danger general-safety term (v0.2.1; bit 256 in FEATURES)
        DEFAULT_VALUES.to_vec(),
        movetime_ms,
        FEATURES,
    );
    if frm == 255 {
        "(none)".to_string()
    } else {
        engine::move_to_uci((frm, to))
    }
}

fn main() {
    let stdin = io::stdin();
    let mut current: Option<engine::Parsed> = None;
    let mut current_moves: Vec<(u8, u8)> = Vec::new();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim();
        let cmd = line.split_whitespace().next().unwrap_or("");
        match cmd {
            "uci" => {
                println!("id name {ENGINE_NAME}");
                println!("id author Mistboard");
                println!("uciok");
            }
            "isready" => println!("readyok"),
            "ucinewgame" => {
                current = None;
                current_moves.clear();
            }
            "position" => {
                // "position fen <board> <turn> <pool> <clock> <movenum> [moves <m> ...]"
                // With a moves list, the FEN is the window-start (last irreversible move) and
                // the moves are the quiet plies since — replayed for repetition awareness.
                if let Some(rest) = line.strip_prefix("position") {
                    let rest = rest.trim();
                    if let Some(fenpart) = rest.strip_prefix("fen") {
                        let mut parts = fenpart.split(" moves ");
                        let fenstr = parts.next().unwrap_or(fenpart).trim();
                        current = engine::state_from_fen(fenstr);
                        current_moves = parts
                            .next()
                            .map(|s| s.split_whitespace().filter_map(engine::uci_to_move).collect())
                            .unwrap_or_default();
                    }
                }
            }
            "go" => {
                let mut movetime = DEFAULT_MOVETIME_MS;
                let mut nodes: u64 = 100_000_000;
                let mut t = line.split_whitespace().skip(1);
                while let Some(k) = t.next() {
                    match k {
                        "movetime" => {
                            if let Some(v) = t.next() {
                                movetime = v.parse().unwrap_or(DEFAULT_MOVETIME_MS);
                            }
                        }
                        "nodes" => {
                            if let Some(v) = t.next() {
                                nodes = v.parse().unwrap_or(nodes);
                            }
                        }
                        _ => {}
                    }
                }
                let mv = match &current {
                    Some(p) => search_best(p, &current_moves, movetime, nodes),
                    None => "(none)".to_string(),
                };
                println!("bestmove {mv}");
            }
            "quit" => break,
            _ => {}
        }
        io::stdout().flush().ok(); // UCI drivers read line-by-line; flush each response
    }
}

#[cfg(test)]
mod tests {
    use super::engine;

    // Piece code = color*7 + role (red=0, black=1; roles 0..6 = gen,adv,ele,cha,hor,can,sol).
    // Square index = file + rank*8 (file a..h = 0..7, rank 1..4 = 0..3). Empty = -1, face-down = -2.
    const V: [f64; 7] = [30.0, 14.0, 11.0, 9.0, 7.0, 16.0, 4.0]; // the shipped value table

    fn idx(file: usize, rank0: usize) -> usize {
        file + rank0 * 8
    }

    #[test]
    fn best_move_is_always_a_legal_move() {
        // Red general d2, black soldier e2 (adjacent). Red to move; a general can't capture a
        // soldier, so it must step to an empty square — whatever it picks must be a legal move.
        let mut sq = vec![-1i16; 32];
        sq[idx(3, 1)] = 0; // d2 red general
        sq[idx(4, 1)] = 1 * 7 + 6; // e2 black soldier
        let bag = vec![0u32; 14];
        let mv = engine::best_move(
            sq.clone(), bag.clone(), 0, 0, 0, 100_000, 0.1, true, 12, 0.8, 0.0, V.to_vec(), 0, 0,
            vec![],
        );
        let legal = engine::legal_moves(sq, bag, 0, 0, 0);
        assert!(legal.contains(&mv), "best_move {mv:?} not in legal set {legal:?}");
    }

    #[test]
    fn wiped_out_waiting_side_is_an_immediate_win() {
        // The early-adjudication rule: red general alone, black has no piece and no face-down
        // tile left, so black can never act again → red (to move) has already won. search_value
        // returns +1 from the mover's view, adjudicated now rather than one ply later.
        let mut sq = vec![-1i16; 32];
        sq[idx(3, 1)] = 0; // d2 red general
        let v =
            engine::search_value(sq, vec![0u32; 14], 0, 0, 0, 100_000, true, 8, 0.8, 0.0, V.to_vec());
        assert_eq!(v, 1.0, "opponent wiped out with no flips left → win adjudicated immediately");
    }

    #[test]
    fn opening_has_thirty_two_flips() {
        // All face-down, colors unbound: every tile is a legal flip (from == to), nothing else.
        let sq = vec![-2i16; 32];
        let mut bag = vec![0u32; 14];
        for c in 0..2 {
            let b = c * 7;
            bag[b] = 1; // general
            bag[b + 1] = 2; // advisors
            bag[b + 2] = 2; // elephants
            bag[b + 3] = 2; // chariots
            bag[b + 4] = 2; // horses
            bag[b + 5] = 2; // cannons
            bag[b + 6] = 5; // soldiers
        }
        let legal = engine::legal_moves(sq, bag, -1, 0, 0);
        assert_eq!(legal.len(), 32, "every face-down tile is a legal opening flip");
        assert!(legal.iter().all(|&(f, t)| f == t), "every opening move is a flip (from == to)");
    }

    #[test]
    fn uci_move_round_trips() {
        let m = (idx(3, 1) as u8, idx(4, 1) as u8);
        let uci = engine::move_to_uci(m);
        assert_eq!(engine::uci_to_move(&uci), Some(m), "uci codec must round-trip ({uci})");
    }
}
