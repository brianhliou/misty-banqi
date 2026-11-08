//! History-dependent repetition and chase rules for NNUE search.
//! History contains prior positions in the current irreversible window; the
//! current position contributes the implicit first occurrence.
use crate::{engine as core, State};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionKey {
    pub board: [i16; 32],
    pub parity: u8,
    pub first_color: Option<u8>,
}
#[derive(Clone, Debug)]
pub struct SiteRules {
    pub no_progress_limit: u32,
    pub last_move: Option<(u8, u8)>,
    pub chased_square: Option<u8>,
    pub repetition_can_draw: bool,
    pub history: Vec<PositionKey>,
}
impl Default for SiteRules {
    fn default() -> Self {
        Self {
            no_progress_limit: core::DEFAULT_NO_PROGRESS_LIMIT,
            last_move: None,
            chased_square: None,
            repetition_can_draw: true,
            history: Vec::new(),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChaseKind {
    Chase,
    Flee,
}

fn threatens(board: &[i16; 32], from: u8, target: u8) -> bool {
    let (a, t) = (board[from as usize], board[target as usize]);
    if a < 0 || t < 0 || a / 7 == t / 7 {
        return false;
    }
    let (x, y) = (from as i32 % 8, from as i32 / 8);
    let (tx, ty) = (target as i32 % 8, target as i32 / 8);
    if a % 7 == 5 {
        if x != tx && y != ty {
            return false;
        }
        let step = if x == tx {
            8 * (ty - y).signum()
        } else {
            (tx - x).signum()
        };
        let mut sq = from as i32 + step;
        let mut screens = 0;
        while sq != target as i32 {
            screens += u8::from(board[sq as usize] != -1);
            sq += step;
        }
        return screens == 1;
    }
    (x - tx).abs() + (y - ty).abs() == 1
        && if a % 7 == 6 && t % 7 == 0 {
            true
        } else if a % 7 == 0 && t % 7 == 6 {
            false
        } else {
            a % 7 <= t % 7
        }
}
impl State {
    pub fn site_key(&self) -> PositionKey {
        PositionKey {
            board: self.board,
            parity: (self.ply % 2) as u8,
            first_color: self.first_color,
        }
    }
    pub fn chase_target(&self, mv: (u8, u8)) -> Option<u8> {
        if mv.0 == mv.1 || self.board[mv.1 as usize] != -1 {
            return None;
        }
        let last = self.site.as_ref()?.last_move.filter(|m| m.0 != m.1)?;
        let mut board = self.board;
        board[mv.1 as usize] = board[mv.0 as usize];
        board[mv.0 as usize] = -1;
        threatens(&board, mv.1, last.1).then_some(last.1)
    }
    pub fn chase_kind(&self, mv: (u8, u8)) -> Option<ChaseKind> {
        let r = self.site.as_ref()?;
        match (
            self.chase_target(mv).is_some(),
            r.chased_square == Some(mv.0),
        ) {
            (true, false) => Some(ChaseKind::Chase),
            (false, true) => Some(ChaseKind::Flee),
            _ => None, // neither or mutual chase
        }
    }
    fn chase_forbidden(&self, mv: (u8, u8)) -> bool {
        if self.chase_kind(mv) != Some(ChaseKind::Chase) {
            return false;
        }
        let mut key = self.site_key();
        key.board[mv.1 as usize] = key.board[mv.0 as usize];
        key.board[mv.0 as usize] = -1;
        key.parity ^= 1;
        // The next side-to-move differs, so the current position cannot match.
        self.site
            .as_ref()
            .unwrap()
            .history
            .iter()
            .filter(|k| **k == key)
            .count()
            >= 2
    }
    pub fn forbidden_chase_moves(&self) -> Vec<(u8, u8)> {
        if self.site.is_none() || matches!(self.status(), crate::Status::Finished { .. }) {
            return vec![];
        }
        let moves = self.pseudo_legal_moves();
        let forbidden: Vec<_> = moves
            .iter()
            .copied()
            .filter(|&m| self.chase_forbidden(m))
            .collect();
        if forbidden.len() < moves.len() {
            forbidden
        } else {
            vec![]
        }
    }
    pub(crate) fn filter_chase_moves(&self, moves: &mut Vec<(u8, u8)>) {
        if self.site.is_none() {
            return;
        }
        // All forbidden => all stay legal, including the only-move exception.
        if moves.iter().any(|&m| !self.chase_forbidden(m)) {
            moves.retain(|&m| !self.chase_forbidden(m));
        }
    }
    pub(crate) fn repetition_draw(&self) -> bool {
        self.repeat_count >= 3 && self.site.as_ref().is_none_or(|r| r.repetition_can_draw)
    }
}
impl core::SearchRules for State {
    fn advance(&self, mv: (u8, u8), revealed: i16) -> Arc<dyn core::SearchRules> {
        Arc::new(
            self.apply_legal(mv, (mv.0 == mv.1).then_some(revealed))
                .expect("search legal move"),
        )
    }
    fn filter_moves(&self, moves: &mut Vec<(u8, u8)>) {
        self.filter_chase_moves(moves);
    }
    fn repetition_draw(&self) -> bool {
        self.repetition_draw()
    }
    fn repeat_count(&self) -> u32 {
        self.repeat_count
    }
}
