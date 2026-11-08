use super::*;
use std::collections::HashSet;

const VALUES: [f64; 7] = [30.0, 14.0, 11.0, 9.0, 7.0, 16.0, 4.0];
const FEATURES: u32 = 1018;

fn position(pieces: &[(usize, i16)]) -> State {
    let mut st = make_state(vec![], vec![], 0, 0, 0);
    for &(sq, code) in pieces {
        st.sq[sq] = code;
    }
    st
}

fn opening() -> State {
    let mut st = make_state(vec![DOWN; NSQ], vec![], -1, 0, 0);
    st.bag = [1, 2, 2, 2, 2, 2, 5, 1, 2, 2, 2, 2, 2, 5];
    st
}

fn masked() -> State {
    let mut st = position(&[(0, 1), (9, DOWN), (18, DOWN), (31, DOWN), (30, 13)]);
    st.bag[2] = 2;
    st.bag[7] = 1;
    st
}

fn cfg(st: &State) -> Cfg {
    Cfg {
        contempt: 0.1,
        root: st.mover_color(),
        quiesce: true,
        quiesce_max: 8,
        w_mob: 0.8,
        w_king: 28.0,
        values: VALUES,
        feat: Feat::from_bits(FEATURES),
    }
}

fn play(depth: i32) -> PlayOptions {
    PlayOptions {
        noise: 0.3,
        noise_depth: depth,
    }
}

fn run(
    st: &State,
    play: PlayOptions,
    seed: u64,
    nodes: u64,
    time_ms: u64,
) -> ((u8, u8), f64, SearchStats) {
    select_move_with_moves_scored_stats_restricted(
        st.sq.to_vec(),
        st.bag.to_vec(),
        st.first_color,
        st.no_progress,
        vec![],
        nodes,
        0.1,
        true,
        4,
        0.8,
        28.0,
        VALUES.to_vec(),
        time_ms,
        FEATURES,
        vec![],
        st.no_progress_limit,
        &play,
        seed,
    )
}

fn guarded(st: &State) -> Vec<(u8, u8)> {
    let mut legal = Vec::new();
    st.legal_moves(&mut legal);
    guarded_root_moves(st, &legal)
}

fn bits(result: ((u8, u8), f64, SearchStats)) -> ((u8, u8), u64, i32, u64) {
    (result.0, result.1.to_bits(), result.2.depth, result.2.nodes)
}

#[test]
fn noise_zero_preserves_search_bits_on_revealed_and_chance_positions() {
    for st in [
        opening(),
        masked(),
        position(&[(0, 0), (2, 13)]),
        position(&[(0, 1), (1, 13)]),
    ] {
        for seed in [0, 1, 123] {
            let old = best_move_with_moves_scored_stats_restricted(
                st.sq.to_vec(),
                st.bag.to_vec(),
                st.first_color,
                st.no_progress,
                vec![],
                400,
                0.1,
                true,
                4,
                0.8,
                28.0,
                VALUES.to_vec(),
                0,
                FEATURES,
                vec![],
                st.no_progress_limit,
            );
            let new = run(
                &st,
                PlayOptions {
                    noise_depth: 8,
                    ..Default::default()
                },
                seed,
                400,
                0,
            );
            assert_eq!(bits(new), bits(old));
        }
    }
}

#[test]
fn noise_zero_preserves_repetition_replay_root_filter_and_draw_limit() {
    let st = masked();
    for limit in [40, 60] {
        let old = best_move_with_moves_scored_stats_restricted(
            st.sq.to_vec(),
            st.bag.to_vec(),
            0,
            37,
            vec![(0, 1), (30, 29)],
            4000,
            0.1,
            true,
            4,
            0.8,
            28.0,
            VALUES.to_vec(),
            0,
            FEATURES,
            vec![(1, 2)],
            limit,
        );
        let new = select_move_with_moves_scored_stats_restricted(
            st.sq.to_vec(),
            st.bag.to_vec(),
            0,
            37,
            vec![(0, 1), (30, 29)],
            4000,
            0.1,
            true,
            4,
            0.8,
            28.0,
            VALUES.to_vec(),
            0,
            FEATURES,
            vec![(1, 2)],
            limit,
            &PlayOptions {
                noise_depth: 8,
                ..Default::default()
            },
            123,
        );
        assert_eq!(bits(new), bits(old));
    }
}

#[test]
fn immediate_capture_and_flip_wins_override_any_noise() {
    let capture = position(&[(0, 1), (1, 13)]);
    // Revealing the last black soldier in this corner leaves it without a move.
    let mut flip = position(&[(0, DOWN), (1, 1), (8, 1)]);
    flip.bag[13] = 1;
    for (st, expected) in [(capture, (0, 1)), (flip, (0, 0))] {
        for seed in 0..64 {
            let result = run(
                &st,
                PlayOptions {
                    noise: f64::MAX,
                    noise_depth: 8,
                },
                seed,
                0,
                1,
            );
            assert_eq!(result.0, expected);
            assert_eq!(result.1, VMAX);
            assert_eq!(result.2.nodes, 0);
        }
    }
}

#[test]
fn avoids_immediate_loss_and_checks_every_flip_reveal() {
    let st = position(&[(0, 0), (2, 13)]);
    assert_eq!(guarded(&st), vec![(0, 8)]);
    let mut flip = position(&[(0, 1), (1, DOWN), (31, DOWN)]);
    flip.bag[7] = 1; // general can capture the last advisor
    flip.bag[13] = 1; // soldier cannot
    assert!(!guarded(&flip).contains(&(1, 1)));
    assert!(guarded(&flip).contains(&(31, 31)));
    for seed in 0..64 {
        let options = PlayOptions {
            noise: 100.0,
            noise_depth: 1,
        };
        assert_eq!(run(&st, options, seed, 400, 0).0, (0, 8));
        assert_ne!(run(&flip, options, seed, 400, 0).0, (1, 1));
    }
    let mut general = flip.clone();
    general.push(1, 1, 7);
    general.push(1, 0, EMPTY);
    assert_eq!(general.result(&mut Vec::new()), RES_BLACK);
    let mut soldier = flip.clone();
    soldier.push(1, 1, 13);
    assert_eq!(soldier.result(&mut Vec::new()), RES_ONGOING);
}

#[test]
fn all_unsafe_retains_legal_moves_and_draws_are_not_losses() {
    let st = position(&[(0, 0), (9, 13)]);
    let mut legal = Vec::new();
    st.legal_moves(&mut legal);
    assert_eq!(guarded(&st), legal);
    let seen: HashSet<_> = (0..32)
        .map(|seed| run(&st, play(1), seed, 400, 0).0)
        .collect();
    assert_eq!(seen.len(), legal.len());
    let mut draw = position(&[(0, 0), (2, 13)]);
    draw.no_progress = 39;
    draw.legal_moves(&mut legal);
    assert_eq!(guarded(&draw), legal);
    for seed in 0..32 {
        let r = run(&draw, play(1), seed, 400, 0);
        assert!(legal.contains(&r.0));
        assert_eq!(r.1, -0.1);
        assert!(r.2.nodes > 0);
    }
}

#[test]
fn gaussian_is_replayable_centered_and_unit_variance() {
    let (mut a, mut b) = (PlayRng::new(123), PlayRng::new(123));
    let (mut sum, mut squares) = (0.0, 0.0);
    for _ in 0..10_000 {
        let z = a.gaussian();
        assert_eq!(z, b.gaussian());
        sum += z;
        squares += z * z;
    }
    assert!((sum / 10_000.0).abs() < 0.04);
    assert!((squares / 10_000.0 - 1.0).abs() < 0.06);
}

#[test]
fn noisy_values_match_full_window_scores_including_weighted_flips() {
    let st = masked();
    for depth in [1, 3] {
        let roots = root_move_values(
            st.sq.to_vec(),
            st.bag.to_vec(),
            0,
            0,
            0,
            u64::MAX,
            0.1,
            true,
            depth,
            0.8,
            28.0,
            VALUES.to_vec(),
            0,
            FEATURES,
        );
        assert_eq!(roots.iter().filter(|&&(f, t, _, _)| f == t).count(), 3);
        let mut seen = HashSet::new();
        let mut flipped = false;
        for seed in 0..32 {
            let a = run(&st, play(depth), seed, u64::MAX, 0);
            let b = run(&st, play(depth), seed, 10_000_000, 10_000);
            assert_eq!(bits(a), bits(b));
            assert_eq!(a.2.depth, depth);
            let v = roots.iter().find(|&&(f, t, _, _)| (f, t) == a.0).unwrap().2;
            assert!((a.1 - v).abs() < 1e-12, "{:?}: {} vs {v}", a.0, a.1);
            seen.insert(a.0);
            flipped |= a.0 .0 == a.0 .1;
        }
        assert!(seen.len() > 1);
        assert!(flipped);
    }
    for seed in 0..16 {
        assert_eq!(
            bits(run(&st, play(0), seed, 400, 0)),
            bits(run(&st, play(1), seed, 400, 0))
        );
    }
}

// Independent complete passes expose exactly where each budget interrupts the
// next depth. Noise must be drawn only once, including when a pass aborts.
fn full_pass(st: &State, depth: i32) -> (Vec<(u8, u8)>, Vec<f64>, u64) {
    let cfg = cfg(st);
    let mut ctx = Ctx::new(u64::MAX, 0, depth, 18);
    ctx.path.push(zkey(st));
    let mut moves = Vec::new();
    st.legal_moves(&mut moves);
    order_moves(st, &mut moves, 1, &cfg, &ctx);
    let scores = moves
        .iter()
        .map(|&m| move_value(st, m, depth, VMIN, VMAX, &cfg, &mut ctx).unwrap())
        .collect();
    (moves, scores, ctx.nodes)
}

#[test]
fn aborted_pass_keeps_last_complete_depth_and_counts_all_nodes() {
    let st = masked();
    let (one, two, three) = (full_pass(&st, 1), full_pass(&st, 2), full_pass(&st, 3));
    for (budget, expected_depth) in [(one.2 + two.2 / 2, 1), (one.2 + two.2 + three.2 / 2, 2)] {
        let mut rng = PlayRng::new(123);
        let actual = noisy_root_choice(
            &st,
            &cfg(&st),
            &one.0,
            &guarded(&st),
            &play(4),
            budget,
            0,
            Instant::now(),
            &[],
            &mut rng,
        );
        let mut expected_rng = PlayRng::new(123);
        let expected = noisy_root_choice(
            &st,
            &cfg(&st),
            &one.0,
            &guarded(&st),
            &play(expected_depth),
            u64::MAX,
            0,
            Instant::now(),
            &[],
            &mut expected_rng,
        );
        assert_eq!(
            (actual.0, actual.1.to_bits(), actual.2.depth),
            (expected.0, expected.1.to_bits(), expected_depth)
        );
        assert_eq!(actual.2.nodes, budget + 1);
        assert_eq!(format!("{rng:?}"), format!("{expected_rng:?}"));
    }
    for (budget, time_ms, start) in [
        (0, 0, Instant::now()),
        (
            u64::MAX,
            1,
            Instant::now() - std::time::Duration::from_secs(1),
        ),
    ] {
        let actual = noisy_root_choice(
            &st,
            &cfg(&st),
            &one.0,
            &guarded(&st),
            &play(4),
            budget,
            time_ms,
            start,
            &[],
            &mut PlayRng::new(123),
        );
        let expected = noisy_root_choice(
            &st,
            &cfg(&st),
            &one.0,
            &guarded(&st),
            &play(1),
            u64::MAX,
            0,
            Instant::now(),
            &[],
            &mut PlayRng::new(123),
        );
        assert_eq!(
            (actual.0, actual.1.to_bits(), actual.2.depth),
            (expected.0, expected.1.to_bits(), 1)
        );
        assert_eq!(actual.2.nodes, one.2 + 1);
    }
}

#[test]
fn completed_pass_terminal_scores_override_noise_without_immediate_guards() {
    // Exercise the score guard directly, allowing every root move. Otherwise
    // the immediate-result guard would hide whether terminal scores survive noise.
    for (st, expected) in [
        (position(&[(0, 1), (1, 13)]), (0, 1)),
        (position(&[(0, 0), (2, 13)]), (0, 8)),
    ] {
        let (moves, scores, _) = full_pass(&st, 1);
        assert!(scores.iter().any(|&v| v.abs() == 1.0));
        for seed in 0..64 {
            let result = noisy_root_choice(
                &st,
                &cfg(&st),
                &moves,
                &moves,
                &PlayOptions {
                    noise: f64::MAX,
                    noise_depth: 1,
                },
                u64::MAX,
                0,
                Instant::now(),
                &[],
                &mut PlayRng::new(seed),
            );
            assert_eq!(result.0, expected);
        }
    }
}

#[test]
fn noisy_search_replays_history_and_obeys_root_filter_and_draw_limit() {
    let mut st = masked();
    st.no_progress = 37;
    let history = vec![(0, 1), (30, 29)];
    for limit in [40, 60] {
        let run = |filter: Vec<(u8, u8)>| {
            select_move_with_moves_scored_stats_restricted(
                st.sq.to_vec(),
                st.bag.to_vec(),
                0,
                37,
                history.clone(),
                10_000,
                0.1,
                true,
                4,
                0.8,
                28.0,
                VALUES.to_vec(),
                0,
                FEATURES,
                filter,
                limit,
                &play(2),
                123,
            )
        };
        let restricted = run(vec![(1, 2)]);
        assert_eq!(restricted.0, (1, 2));
        if limit == 40 {
            assert_eq!(restricted.1, -0.1);
        }
        assert_eq!(bits(run(vec![(31, 0)])), bits(run(vec![])));
        // Replaying the window must give the same root, repetition history,
        // position-mixed random stream and full-window scores as direct search.
        let mut root = st.clone();
        root.no_progress_limit = limit;
        let mut keys = Vec::new();
        for &(f, t) in &history {
            keys.push(zkey(&root));
            root.push(f, t, EMPTY);
        }
        let mut moves = Vec::new();
        root.legal_moves(&mut moves);
        order_moves(&root, &mut moves, 1, &cfg(&root), &Ctx::new(0, 0, 2, 0));
        let direct = noisy_root_choice(
            &root,
            &cfg(&root),
            &moves,
            &guarded(&root),
            &play(2),
            10_000,
            0,
            Instant::now(),
            &keys,
            &mut PlayRng::new(123 ^ zkey(&root)),
        );
        assert_eq!(bits(run(vec![])), bits(direct));
    }
}

#[test]
fn noisy_opening_respects_nodes_and_movetime() {
    // A timeout keeps a limit regression from hanging the test process.
    for (nodes, time_ms) in [(10_000, 0), (u64::MAX, 1000)] {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run(&opening(), play(4), 123, nodes, time_ms));
        });
        let (m, value, stats) = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("noisy search exceeded 2 seconds");
        assert_eq!(m.0, m.1);
        assert!(m.0 < NSQ as u8);
        assert!(value.is_finite());
        assert!((1..4).contains(&stats.depth));
        assert!(stats.nodes > 0);
        if time_ms == 0 {
            assert_eq!(stats.nodes, nodes + 1);
        }
    }
}

#[test]
fn options_validate_finite_nonnegative_sigma_and_depth_range() {
    for noise in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(PlayOptions {
            noise,
            ..Default::default()
        }
        .validate()
        .is_err());
    }
    for noise in [0.0, 0.01, 100.0, f64::MAX] {
        assert!(PlayOptions {
            noise,
            ..Default::default()
        }
        .validate()
        .is_ok());
    }
    for depth in [-1, 9] {
        assert!(play(depth).validate().is_err());
    }
    for depth in 0..=8 {
        assert!(play(depth).validate().is_ok());
    }
}
