//! PyO3 bindings for the Banqi engine.
//!
//! All search/board logic lives in `engine` (a pure, pyo3-free module shared with the
//! standalone `banqi-engine` UCI binary via a `#[path]` include — so the engine that
//! ships through the platform and the engine the Python bakeoffs drive are the SAME
//! code). This file is only the Python surface: thin `#[pyfunction]` forwarders.
//!
//! Encoding across the Python boundary:
//!   square: -1 empty, -2 face-down, else piece code 0..13 = color*7 + role
//!   roles 0..6 = general, advisor, elephant, chariot, horse, cannon, soldier
//!   color: 0 red, 1 black; first_color: -1 unbound, 0 red, 1 black
//!   bag: [u32; 14] indexed by the piece code (count of unrevealed of that type)

use pyo3::prelude::*;

mod engine;

#[pyfunction]
#[allow(clippy::too_many_arguments)]
#[pyo3(signature = (squares, bag, first_color, ply, no_progress, node_budget, contempt,
    quiesce_on, max_depth, w_mob, w_king, values, time_ms, features, rep_history=Vec::new()))]
fn best_move(
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
    engine::best_move(
        squares, bag, first_color, ply, no_progress, node_budget, contempt, quiesce_on,
        max_depth, w_mob, w_king, values, time_ms, features, rep_history,
    )
}

/// Zobrist key for a position (board + bag + side-to-move) — used to compute the
/// repetition window passed to `best_move(..., rep_history=...)`.
#[pyfunction]
fn zkey(squares: Vec<i16>, bag: Vec<u32>, first_color: i16, ply: u32) -> u64 {
    engine::zkey_for(squares, bag, first_color, ply)
}

#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn search_value(
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
    engine::search_value(
        squares, bag, first_color, ply, no_progress, node_budget, quiesce_on, max_depth, w_mob,
        w_king, values,
    )
}

#[pyfunction]
fn negamax_value(squares: Vec<i16>, bag: Vec<u32>, first_color: i16, ply: u32, no_progress: u32, depth: i32) -> f64 {
    engine::negamax_value(squares, bag, first_color, ply, no_progress, depth)
}

/// DIAGNOSTIC: per-root-move exact values under the full shipped Cfg+features. Returns a
/// list of (from, to, value, depth_reached). Not on any shipped path — introspection only.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn root_move_values(
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
    engine::root_move_values(
        squares, bag, first_color, ply, no_progress, node_budget, contempt, quiesce_on,
        max_depth, w_mob, w_king, values, time_ms, features,
    )
}

#[pyfunction]
fn legal_moves(squares: Vec<i16>, bag: Vec<u32>, first_color: i16, ply: u32, no_progress: u32) -> Vec<(u8, u8)> {
    engine::legal_moves(squares, bag, first_color, ply, no_progress)
}

#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn mcts_move(
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
    engine::mcts_move(
        squares, bag, first_color, ply, no_progress, sims, c_uct, max_depth, leaf_depth, seed,
        w_mob, w_king, values,
    )
}

#[pymodule]
fn banqi_rust(_py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(best_move, m)?)?;
    m.add_function(wrap_pyfunction!(zkey, m)?)?;
    m.add_function(wrap_pyfunction!(search_value, m)?)?;
    m.add_function(wrap_pyfunction!(negamax_value, m)?)?;
    m.add_function(wrap_pyfunction!(root_move_values, m)?)?;
    m.add_function(wrap_pyfunction!(legal_moves, m)?)?;
    m.add_function(wrap_pyfunction!(mcts_move, m)?)?;
    Ok(())
}
