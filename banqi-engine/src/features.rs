//! Sparse public features for BQNN v2, relative to each absolute seat.
//!
//! 0..448: own then opposing piece code (seven roles each) * 32 + square;
//! 448..480: covered squares; 480..512: own then opposing bag thresholds
//! (1,2,2,2,2,2,5 per colour); 512..571: clock >= 1..59; 571: at least
//! two occurrences; 572: seat to move; 573: colours unbound; 574..576 reserved.
//! Squares use a0=0, h3=31, matching UCI notation. With colours unbound, red ink is the own colour
//! for both seats until the first flip binds it. No hidden identity is input.
use crate::{Seat, State, DOWN, NSQ, START_COUNTS};
pub const COVERED: usize = 448;
pub const BAG: usize = 480;
pub const CLOCK: usize = 512;
pub const REPEAT: usize = 571;
pub const TURN: usize = 572;
pub const UNBOUND: usize = 573;
pub const N_FEATURES: usize = 576; // 574..575 reserved
/// Conservative even for individually valid but unreachable snapshots:
/// 32 occupied points + 32 bag thresholds + 59 clock + repeat + turn + unbound.
pub const MAX_ACTIVE: usize = 126;
const WORDS: usize = N_FEATURES / 64;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeatureSet(pub(crate) [u64; WORDS]);
impl FeatureSet {
    fn insert(&mut self, i: usize) {
        self.0[i / 64] |= 1 << (i % 64);
    }
    pub fn for_seat(s: &State, seat: Seat) -> Self {
        Self::from_public(
            &s.board,
            &s.bag,
            s.first_color,
            s.turn(),
            s.no_progress,
            s.repeat_count,
            seat,
        )
    }
    pub fn from_public(
        board: &[i16; 32],
        bag: &[u32; 14],
        first: Option<u8>,
        mover: Seat,
        clock: u32,
        repeat: u32,
        seat: Seat,
    ) -> Self {
        let own = first.map_or(0, |c| c ^ seat as u8) as usize;
        let mut out = Self::default();
        for (sq, &c) in board.iter().enumerate() {
            if c >= 0 {
                let c = c as usize;
                out.insert((c % 7 + 7 * usize::from(c / 7 != own)) * NSQ + sq);
            } else if c == DOWN {
                out.insert(COVERED + sq);
            }
        }
        let mut offset = BAG;
        for c in 0..14 {
            let absolute = (own ^ (c / 7)) * 7 + c % 7;
            for t in 1..=START_COUNTS[c % 7] {
                if bag[absolute] >= t {
                    out.insert(offset);
                }
                offset += 1;
            }
        }
        for t in 1..=59 {
            if clock >= t {
                out.insert(CLOCK + t as usize - 1);
            }
        }
        if repeat >= 2 {
            out.insert(REPEAT);
        }
        if mover == seat {
            out.insert(TURN);
        }
        if first.is_none() {
            out.insert(UNBOUND);
        }
        out
    }
    pub fn both(s: &State) -> [Self; 2] {
        [Seat::Red, Seat::Black].map(|seat| Self::for_seat(s, seat))
    }
    pub fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(word, &bits)| {
            let mut b = bits;
            std::iter::from_fn(move || {
                if b == 0 {
                    return None;
                }
                let bit = b.trailing_zeros();
                b &= b - 1;
                Some(word * 64 + bit as usize)
            })
        })
    }
}
pub fn active(s: &State, seat: Seat) -> Vec<u16> {
    FeatureSet::for_seat(s, seat)
        .indices()
        .map(|x| x as u16)
        .collect()
}
