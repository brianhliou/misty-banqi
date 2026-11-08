//! NNUE search configuration and incremental leaf evaluation.
use crate::{
    engine::{self, EvalPosition, Evaluator, Parsed, SearchStats},
    nnue::{Accumulator, Net},
    rules::SiteRules,
    State, Status,
};
use std::{cell::RefCell, sync::Arc};

struct Leaf<'a> {
    net: &'a Net,
    cache: RefCell<([Accumulator; 2], usize, bool)>,
}

impl Evaluator for Leaf<'_> {
    fn evaluate(&self, p: EvalPosition<'_>) -> f64 {
        let state = State {
            board: *p.squares,
            bag: *p.bag,
            first_color: (p.first_color >= 0).then_some(p.first_color as u8),
            ply: p.ply,
            no_progress: p.no_progress,
            repeat_count: p.repeat_count,
            history: Vec::new(),
            site: None,
        };
        let mut cache = self.cache.borrow_mut();
        let (pair, index, valid) = &mut *cache;
        let [a, b] = pair;
        let (parent, next) = if *index == 0 { (a, b) } else { (b, a) };
        if *valid {
            self.net.update(parent, &state, next);
        } else {
            self.net.refresh(&state, next);
            *valid = true;
        }
        *index ^= 1;
        self.net.utility_accumulator(next, state.turn())
    }
}

/// Replay the host's quiet window using the same FEN and move contract as the
/// classical search. No-progress limits always come from the UCI option.
pub fn search(
    net: &Net,
    p: &Parsed,
    window: &[(u8, u8)],
    nodes: u64,
    time_ms: u64,
    search_moves: &[(u8, u8)],
    no_progress_limit: u32,
    play: &engine::PlayOptions,
    seed: u64,
) -> ((u8, u8), f64, SearchStats) {
    let mut state = State {
        board: p.squares.as_slice().try_into().unwrap(),
        bag: p.bag.as_slice().try_into().unwrap(),
        first_color: (p.first_color >= 0).then_some(p.first_color as u8),
        ply: 0,
        no_progress: p.no_progress,
        repeat_count: 1,
        history: Vec::new(),
        site: Some(SiteRules {
            no_progress_limit,
            ..SiteRules::default()
        }),
    };
    for &mv in window {
        let Ok(next) = state.apply_legal(mv, None) else {
            return ((255, 255), 0., SearchStats::default());
        };
        state = next;
    }
    if let Status::Finished { winner, .. } = state.status() {
        let value = winner.map_or(0., |w| if w == state.turn() { 1. } else { -1. });
        return ((255, 255), value, SearchStats::default());
    }
    let leaf = Leaf {
        net,
        cache: RefCell::new(([Accumulator::default(), Accumulator::default()], 0, false)),
    };
    let (mv, score, stats) = engine::select_move_scored_stats_with_eval(
        state.board.to_vec(),
        state.bag.to_vec(),
        state.first_color.map_or(-1, |c| c as i16),
        state.ply,
        state.no_progress,
        nodes,
        0., // Neutral draws with NNUE.
        true,
        64,
        0.8,
        28.,
        vec![30., 14., 11., 9., 7., 16., 4.],
        time_ms,
        1018,
        state.history.clone(),
        search_moves.to_vec(),
        no_progress_limit,
        4, // Capture quiescence plies.
        Some(&leaf),
        Some(Arc::new(state.clone())),
        play,
        seed,
    );
    // Noisy guards can prove an immediate win without visiting search nodes.
    let score = if stats.depth == 0 && play.noise == 0.0 {
        net.utility(&state)
    } else {
        score
    };
    (mv, score, stats)
}
