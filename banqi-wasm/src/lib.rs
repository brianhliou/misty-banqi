//! MistyBanqi WebAssembly shim — the in-browser client engine for Mistboard's Banqi
//! review/analysis panel. Unlike the UCI binary (`banqi-engine`), which emits a single
//! `bestmove` for PvE play, this build exposes the search core's **per-root-move exact
//! values** (`root_move_values`) so the browser panel can render live MultiPV: the top-K
//! legal moves, each with its own eval, single-shot.
//!
//! Redaction contract is identical to the UCI binary: the caller feeds a REDACTED Banqi
//! FEN (face-down tiles as `X`, pool as public per-(ink,role) counts) — the engine never
//! learns a hidden tile's identity. See `banqi_rust/src/engine.rs` FEN section.
//!
//! Time source: `wasm32-unknown-unknown` has no monotonic clock, so search is driven by
//! the NODE BUDGET only (`time_ms` is passed as 0; the engine's `Instant` is a no-op on
//! wasm — see engine.rs). Node budget makes the search deterministic per position anyway.

#[path = "../../banqi_rust/src/engine.rs"]
#[allow(dead_code)] // engine.rs also exposes PyO3/UCI-facing entry points, unused here
mod engine;

use wasm_bindgen::prelude::*;

// Mirror the UCI binary's shipped search config (banqi-engine/src/main.rs) so the client
// engine and the server engine evaluate positions identically.
// TT(2)+rep(8)+cover_mat(16)+king_ctx(32)+value-mobility(64)+adaptive-domination(128)
// +gen_danger(256)+no_draw_sac(512) = 1018.
const FEATURES: u32 = 1018;
// Corrected value table (gen,adv,ele,cha,hor,can,sol) — matches the UCI binary.
const DEFAULT_VALUES: [f64; 7] = [30.0, 14.0, 11.0, 9.0, 7.0, 16.0, 4.0];
const CONTEMPT: f64 = 0.1;
const MAX_DEPTH: i32 = 24;
const W_MOB: f64 = 0.8;
const W_KING: f64 = 28.0;

/// Evaluate a redacted Banqi FEN and return the top-`multipv` legal moves as JSON,
/// ranked best-first, each with an exact side-to-move centipawn score.
///
/// Returns `{"lines":[{"uci":"c1c1","cp":123,"depth":6},...]}` (a flip is `from==to`, e.g.
/// `"c1c1"`), or `{"error":"bad_fen"}` on a malformed FEN, or `{"lines":[]}` when there is
/// no legal move (terminal). `cp` is side-to-move POV (the browser normalizes to Red).
#[wasm_bindgen]
pub fn analyze(fen: &str, nodes: u32, multipv: u32) -> String {
    let parsed = match engine::state_from_fen(fen) {
        Some(p) => p,
        None => return "{\"error\":\"bad_fen\"}".to_string(),
    };
    // ply = 0: Parsed reconstructs mover_color == first_color (the FEN's turn token), which
    // is search-correct — only display/movenum depend on the real ply.
    let ranked = engine::root_move_values(
        parsed.squares,
        parsed.bag,
        parsed.first_color,
        0,
        parsed.no_progress,
        nodes as u64,
        CONTEMPT,
        true, // quiescence
        MAX_DEPTH,
        W_MOB,
        W_KING,
        DEFAULT_VALUES.to_vec(),
        0, // time_ms = 0: node-budget only (no wall clock on wasm)
        FEATURES,
    );
    // ranked: Vec<(from, to, value, depth_reached)>, already sorted descending by value.
    let take = (multipv.max(1) as usize).min(ranked.len());
    let mut out = String::from("{\"lines\":[");
    for (i, &(from, to, value, depth)) in ranked.iter().take(take).enumerate() {
        if i > 0 {
            out.push(',');
        }
        let uci = engine::move_to_uci((from, to));
        // Root value is side-to-move win-ness in ~[-1, 1]; ×1000 maps onto the platform's
        // centipawn win% curve (±1 ≈ decisive ≈ ±1000 cp), same as the UCI binary.
        let cp = (value.clamp(-1.0, 1.0) * 1000.0).round() as i64;
        out.push_str(&format!("{{\"uci\":\"{uci}\",\"cp\":{cp},\"depth\":{depth}}}"));
    }
    out.push_str("]}");
    out
}
