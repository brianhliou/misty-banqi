mod common;
use banqi_engine::{
    engine::{self, EvalPosition, Evaluator},
    nnue::Net,
    rules::ChaseKind,
    State, Status,
};
use std::sync::Arc;

fn chase() -> State {
    let mut s = State::initial_site();
    s.board = [-1; 32];
    s.bag = [0; 14];
    s.first_color = Some(0);
    s.board[0] = 1;
    s.board[1] = 7;
    s.board[9] = 13;
    s.site.as_mut().unwrap().last_move = Some((17, 9));
    let next = s.apply((0, 8), None).unwrap().site_key();
    s.site.as_mut().unwrap().history = vec![next.clone(), next];
    s
}

#[test]
fn a_repeated_chase_is_filtered_but_a_forced_chase_remains_legal() {
    let forced = chase();
    assert_eq!(forced.pseudo_legal_moves(), [(0, 8)]);
    assert_eq!(forced.legal_moves(), [(0, 8)]);
    assert!(forced.forbidden_chase_moves().is_empty());
    let mut s = forced.clone();
    s.board[31] = 6; // An alternative move makes the repeated chase forbidden.
    for key in &mut s.site.as_mut().unwrap().history {
        key.board[31] = 6;
    }
    assert_eq!(s.chase_kind((0, 8)), Some(ChaseKind::Chase));
    assert_eq!(s.forbidden_chase_moves(), [(0, 8)]);
    assert!(!s.legal_moves().contains(&(0, 8)));
}

#[test]
fn threefold_draws_and_irreversible_moves_reset_the_window() {
    let mut s = State::initial_site();
    s.board = [-1; 32];
    s.bag = [0; 14];
    s.first_color = Some(0);
    s.board[0] = 0;
    s.board[31] = 7;
    for _ in 0..2 {
        for mv in [(0, 1), (31, 30), (1, 0), (30, 31)] {
            s = s.apply(mv, None).unwrap();
        }
    }
    assert_eq!(s.repeat_count, 3);
    assert!(matches!(
        s.status(),
        Status::Finished {
            winner: None,
            reason: "repetition"
        }
    ));
    let mut s = chase();
    s.no_progress = 20;
    s.board[8] = 13;
    let captured = s.apply((0, 8), None).unwrap();
    assert_eq!((captured.no_progress, captured.repeat_count), (0, 1));
    assert!(captured.history.is_empty());
    assert!(captured.site.unwrap().history.is_empty());
    s.board[8] = -2;
    s.bag[6] = 1;
    let flipped = s.apply((8, 8), Some(6)).unwrap();
    assert_eq!((flipped.no_progress, flipped.repeat_count), (0, 1));
    assert!(flipped.site.unwrap().history.is_empty());
}

struct Leaf(Net);
impl Evaluator for Leaf {
    fn evaluate(&self, p: EvalPosition<'_>) -> f64 {
        self.0.utility(&State {
            board: *p.squares,
            bag: *p.bag,
            first_color: (p.first_color >= 0).then_some(p.first_color as u8),
            ply: p.ply,
            no_progress: p.no_progress,
            repeat_count: p.repeat_count,
            history: Vec::new(),
            site: None,
        })
    }
}
fn exact(s: &State, depth: i32, net: &Net) -> f64 {
    match s.status() {
        Status::Finished { winner, .. } => {
            winner.map_or(0., |w| if w == s.turn() { 1. } else { -1. })
        }
        _ if depth == 0 => net.utility(s),
        _ => s
            .legal_moves()
            .iter()
            .map(|&m| -exact(&s.apply(m, None).unwrap(), depth - 1, net))
            .fold(f64::NEG_INFINITY, f64::max),
    }
}

#[test]
fn search_carries_exact_rules_and_features_below_the_root() {
    let mut before = chase();
    before.board[9] = -1;
    before.board[17] = 13;
    before.board[31] = 6;
    for key in &mut before.site.as_mut().unwrap().history {
        key.board[31] = 6;
    }
    before.ply = 1;
    before.site.as_mut().unwrap().last_move = Some((8, 0));
    before.site.as_mut().unwrap().chased_square = None;
    let leaf = Leaf(Net::from_bytes(&common::net_bytes(7)).unwrap());
    for depth in 2..=4 {
        let (mv, value, stats) = engine::best_move_scored_stats_with_eval(
            before.board.to_vec(),
            before.bag.to_vec(),
            0,
            before.ply,
            before.no_progress,
            100_000,
            0.,
            false,
            depth,
            0.8,
            28.,
            vec![30., 14., 11., 9., 7., 16., 4.],
            0,
            1018 & !512,
            vec![],
            vec![],
            40,
            0,
            Some(&leaf),
            Some(Arc::new(before.clone())),
        );
        assert_eq!(stats.depth, depth);
        assert!(before.legal_moves().contains(&mv));
        assert!((value - exact(&before, depth, &leaf.0)).abs() < 1e-12);
    }
}
