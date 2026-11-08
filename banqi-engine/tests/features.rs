mod common;
use banqi_engine::{features::*, Seat, State, DOWN, START_COUNTS};

#[test]
fn opening_features_are_public_sorted_and_bounded() {
    let state = State::initial();
    for seat in [Seat::Red, Seat::Black] {
        let active = active(&state, seat);
        assert!(active.windows(2).all(|p| p[0] < p[1]));
        assert!(active.iter().all(|&i| (i as usize) < N_FEATURES));
        assert_eq!(
            active
                .iter()
                .filter(|&&i| (COVERED..BAG).contains(&(i as usize)))
                .count(),
            32
        );
        assert_eq!(
            active
                .iter()
                .filter(|&&i| (BAG..CLOCK).contains(&(i as usize)))
                .count(),
            32
        );
        assert!(active.contains(&(UNBOUND as u16)));
        assert_eq!(active.contains(&(TURN as u16)), seat == Seat::Red);
        assert!(!active.contains(&(REPEAT as u16)));
    }
}

#[test]
fn relative_colours_bag_thresholds_and_clock_saturation() {
    let mut s = State::initial();
    s.board = [-1; 32];
    s.board[0] = 0;
    s.board[31] = 13;
    s.bag = [0; 14];
    s.bag[13] = 3;
    s.first_color = Some(0);
    s.no_progress = 100;
    s.repeat_count = 3;
    let red = active(&s, Seat::Red);
    let black = active(&s, Seat::Black);
    assert!(red.contains(&0));
    assert!(red.contains(&(13 * 32 + 31)));
    assert!(black.contains(&(7 * 32)));
    assert!(black.contains(&(6 * 32 + 31)));
    assert_eq!(
        red.iter()
            .filter(|&&i| (CLOCK..REPEAT).contains(&(i as usize)))
            .count(),
        59
    );
    assert_eq!(
        red.iter()
            .filter(|&&i| (BAG..CLOCK).contains(&(i as usize)))
            .count(),
        3
    );
    assert!(red.contains(&(REPEAT as u16)));
    let old = red;
    s.repeat_count = 2;
    s.no_progress = 59;
    assert_eq!(active(&s, Seat::Red), old);
    // Swapping ink labels and the first-seat binding preserves relative features.
    for c in &mut s.board {
        if *c >= 0 {
            *c = (*c + 7) % 14;
        }
    }
    s.bag.rotate_left(7);
    s.first_color = Some(1);
    assert_eq!(active(&s, Seat::Red), old);
}

#[test]
fn legal_playouts_fit_the_accumulator_feature_bound() {
    let mut saw_flip = false;
    let mut saw_capture = false;
    for s in common::positions(1000) {
        saw_flip |= s.bag.iter().sum::<u32>() < 32 && s.board.contains(&DOWN);
        let alive = s.bag.iter().sum::<u32>() + s.board.iter().filter(|&&c| c >= 0).count() as u32;
        saw_capture |= alive < 32;
        for seat in [Seat::Red, Seat::Black] {
            let set = FeatureSet::for_seat(&s, seat);
            let active = active(&s, seat);
            assert_eq!(set.indices().map(|i| i as u16).collect::<Vec<_>>(), active);
            assert!(active.len() <= MAX_ACTIVE);
            assert!(active.iter().all(|&i| (i as usize) < N_FEATURES));
        }
    }
    assert!(saw_flip && saw_capture);
    // Maximum individually valid snapshot: 32 occupied points, full bag,
    // saturated clock, repetition, turn and unbound flags.
    let mut edge = State::initial();
    edge.bag = std::array::from_fn(|i| START_COUNTS[i % 7]);
    edge.no_progress = u32::MAX;
    edge.repeat_count = u32::MAX;
    assert_eq!(active(&edge, Seat::Red).len(), MAX_ACTIVE);
}
