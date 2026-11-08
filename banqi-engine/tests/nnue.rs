mod common;
use banqi_engine::{
    features,
    nnue::{Accumulator, Net},
    Seat, State,
};

#[test]
fn malformed_headers_lengths_and_weights_are_rejected() {
    let valid = common::net_bytes(1);
    for (offset, word) in [
        (0, 0),
        (4, 1u32),
        (8, 575),
        (12, 0),
        (12, 4097),
        (16, 15),
        (20, 31),
    ] {
        let mut bytes = valid.clone();
        bytes[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
        assert!(Net::from_bytes(&bytes).is_err(), "offset {offset}");
    }
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1_000_001.] {
        let mut bytes = valid.clone();
        bytes[24..28].copy_from_slice(&bad.to_le_bytes());
        assert!(Net::from_bytes(&bytes).is_err());
    }
    for length in [0, 4, 23, 24, valid.len() - 1] {
        assert!(Net::from_bytes(&valid[..length]).is_err());
    }
    let mut extra = valid.clone();
    extra.push(0);
    assert!(Net::from_bytes(&extra).is_err());
}

#[test]
fn incremental_updates_equal_refresh_across_playouts_and_edge_features() {
    let mut states = common::positions(500);
    for first in [None, Some(0), Some(1)] {
        for clock in [0, 39, 40, 59, 60, 100] {
            for repeat in [1, 2, 3] {
                let mut state = State::initial();
                state.first_color = first;
                state.no_progress = clock;
                state.repeat_count = repeat;
                state.ply = repeat % 2;
                states.push(state);
            }
        }
    }
    for width in [1, 7, 31, 255, 257] {
        let net = Net::from_bytes(&common::net_bytes(width)).unwrap();
        assert!(net.can_quantize());
        let mut parent = Accumulator::default();
        let mut delta = Accumulator::default();
        let mut fresh = Accumulator::default();
        net.refresh(&states[0], &mut parent);
        for (i, s) in states.iter().enumerate() {
            net.update(&parent, s, &mut delta);
            net.refresh(s, &mut fresh);
            assert_eq!(delta, fresh, "width {width}, state {i}");
            let error =
                (net.utility_accumulator(&delta, s.turn()) - net.utility_reference(s)).abs();
            assert!(error < 0.02, "width {width}, state {i}, error {error}");
            std::mem::swap(&mut parent, &mut delta);
        }
    }
}

#[test]
fn accumulator_fit_is_checked_and_float_fallback_stays_finite() {
    let mut bytes = common::net_bytes(1);
    for i in 0..features::N_FEATURES {
        bytes[24 + 4 * i..28 + 4 * i].copy_from_slice(&2f32.to_le_bytes());
    }
    let net = Net::from_bytes(&bytes).unwrap();
    assert!(!net.can_quantize());
    assert!(net.utility(&State::initial()).is_finite());
    let mut bias = common::net_bytes(1);
    let offset = 24 + features::N_FEATURES * 4;
    bias[offset..offset + 4].copy_from_slice(&200f32.to_le_bytes());
    assert!(!Net::from_bytes(&bias).unwrap().can_quantize());
}

#[test]
fn external_net_loads_and_updates_exactly() {
    let Some(path) = common::external_net() else {
        return;
    };
    let bytes = std::fs::read(path).unwrap();
    let net = Net::from_bytes(&bytes).unwrap();
    assert!(net.can_quantize());
    let mut acc = Accumulator::default();
    let mut next = Accumulator::default();
    let mut fresh = Accumulator::default();
    let states = common::positions(200);
    net.refresh(&states[0], &mut acc);
    for s in states {
        net.update(&acc, &s, &mut next);
        net.refresh(&s, &mut fresh);
        assert_eq!(next, fresh);
        assert!((-0.95..=0.95).contains(&net.utility_accumulator(&next, s.turn())));
        std::mem::swap(&mut acc, &mut next);
    }
    assert_ne!(net.utility(&State::initial()), 0.);
    assert_eq!(
        features::FeatureSet::both(&State::initial())[Seat::Red as usize],
        features::FeatureSet::for_seat(&State::initial(), Seat::Red)
    );
}
