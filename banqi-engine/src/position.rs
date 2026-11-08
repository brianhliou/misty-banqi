//! Public position used by NNUE and history-dependent search rules.
use crate::{engine as core, rules};
pub const NSQ: usize = 32;
pub const DOWN: i16 = -2;
pub const START_COUNTS: [u32; 7] = [1, 2, 2, 2, 2, 2, 5];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seat {
    Red,
    Black,
}
impl Seat {
    pub fn other(self) -> Self {
        match self {
            Self::Red => Self::Black,
            Self::Black => Self::Red,
        }
    }
}
#[derive(Clone, Debug)]
pub struct State {
    pub board: [i16; NSQ],
    pub bag: [u32; 14],
    pub first_color: Option<u8>,
    pub ply: u32,
    pub no_progress: u32,
    pub repeat_count: u32,
    pub history: Vec<u64>,
    /// Optional history-dependent repetition and chase rules.
    pub site: Option<rules::SiteRules>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Playing(Seat),
    Finished {
        winner: Option<Seat>,
        reason: &'static str,
    },
}
impl State {
    pub fn initial() -> Self {
        Self {
            board: [DOWN; NSQ],
            bag: std::array::from_fn(|i| START_COUNTS[i % 7]),
            first_color: None,
            ply: 0,
            no_progress: 0,
            repeat_count: 1,
            history: Vec::new(),
            site: None,
        }
    }
    pub fn initial_site() -> Self {
        Self {
            site: Some(rules::SiteRules::default()),
            ..Self::initial()
        }
    }
    pub fn turn(&self) -> Seat {
        if self.ply % 2 == 0 {
            Seat::Red
        } else {
            Seat::Black
        }
    }
    pub fn ink_for_seat(&self, seat: Seat) -> Option<u8> {
        self.first_color.map(|c| c ^ seat as u8)
    }
    pub fn position_key(&self) -> u64 {
        core::zkey_for(
            self.board.to_vec(),
            self.bag.to_vec(),
            self.first_color.map_or(-1, |x| x as i16),
            self.ply,
        )
    }
    pub fn legal_moves(&self) -> Vec<(u8, u8)> {
        if self.site.is_some() && matches!(self.status(), Status::Finished { .. }) {
            return Vec::new();
        }
        let mut moves = self.pseudo_legal_moves();
        self.filter_chase_moves(&mut moves);
        moves
    }
    pub fn pseudo_legal_moves(&self) -> Vec<(u8, u8)> {
        core::legal_moves(
            self.board.to_vec(),
            self.bag.to_vec(),
            self.first_color.map_or(-1, |x| x as i16),
            self.ply,
            self.no_progress,
        )
    }
    pub fn status(&self) -> Status {
        if self.no_progress >= self.site.as_ref().map_or(40, |r| r.no_progress_limit) {
            return Status::Finished {
                winner: None,
                reason: "clock",
            };
        }
        if self.repetition_draw() {
            return Status::Finished {
                winner: None,
                reason: "repetition",
            };
        }
        let moves = self.pseudo_legal_moves();
        if moves.is_empty() {
            return Status::Finished {
                winner: self.first_color.map(|_| self.turn().other()),
                reason: "no_moves",
            };
        }
        if self.first_color.is_some() {
            let has = |color: u8| {
                self.bag[color as usize * 7..color as usize * 7 + 7]
                    .iter()
                    .any(|&n| n > 0)
                    || self.board.iter().any(|&c| c >= 0 && c / 7 == color as i16)
            };
            for seat in [self.turn(), self.turn().other()] {
                if !has(self.ink_for_seat(seat).unwrap()) {
                    return Status::Finished {
                        winner: Some(seat.other()),
                        reason: "elimination",
                    };
                }
            }
        }
        Status::Playing(self.turn())
    }
    pub fn apply(&self, mv: (u8, u8), reveal: Option<i16>) -> Result<Self, &'static str> {
        if !matches!(self.status(), Status::Playing(_)) || !self.legal_moves().contains(&mv) {
            return Err("illegal move");
        }
        self.apply_legal(mv, reveal)
    }
    /// Search-only transition: move must come from this state's legal set.
    pub fn apply_legal(&self, mv: (u8, u8), reveal: Option<i16>) -> Result<Self, &'static str> {
        let mut s = self.clone();
        let irreversible = mv.0 == mv.1 || s.board[mv.1 as usize] >= 0;
        if let Some(site) = &mut s.site {
            site.repetition_can_draw = irreversible || self.chase_kind(mv).is_none();
            site.chased_square = if irreversible {
                None
            } else {
                self.chase_target(mv)
            };
            site.last_move = Some(mv);
            if irreversible {
                site.history.clear();
            } else {
                site.history.push(self.site_key());
            }
        }
        if mv.0 == mv.1 {
            let c = reveal.ok_or("missing reveal")?;
            if !(0..14).contains(&c) || s.bag[c as usize] == 0 {
                return Err("invalid reveal");
            }
            s.board[mv.0 as usize] = c;
            s.bag[c as usize] -= 1;
            if s.first_color.is_none() {
                s.first_color = Some((c / 7) as u8);
            }
        } else {
            if reveal.is_some() {
                return Err("unexpected reveal");
            }
            s.board[mv.1 as usize] = s.board[mv.0 as usize];
            s.board[mv.0 as usize] = -1;
        }
        if irreversible {
            s.no_progress = 0;
            s.history.clear();
        } else {
            s.no_progress += 1;
            s.history.push(self.position_key());
        }
        s.ply += 1;
        let key = s.position_key();
        s.repeat_count = s.site.as_ref().map_or_else(
            || 1 + s.history.iter().filter(|&&k| k == key).count() as u32,
            |r| 1 + r.history.iter().filter(|k| **k == s.site_key()).count() as u32,
        );
        Ok(s)
    }
}
