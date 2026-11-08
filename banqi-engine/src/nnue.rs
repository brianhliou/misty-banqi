//! BQNN v2 evaluation file (no padding or trailing bytes).
//!
//! Header: ASCII `BQNN`, then five little-endian u32 values: version 2,
//! feature count 576, hidden width H (1..=4096), dense widths 16 and 32.
//! Payload: little-endian finite f32 values, each with absolute value <= 1e6,
//! in this order: FT[576,H], FT bias[H], W1[16,2H], b1[16], W2[32,16],
//! b2[32], W3[1,32], b3[1]. FT rows are feature-major; dense rows are
//! output-major. Total length is 24 + 4*(609*H + 593) bytes.
//!
//! Features use two absolute-seat accumulators. Dense input is side-to-move
//! then other-seat, with clipped [0,1] activations. FT weights/biases are
//! rounded at scale 254 into i16; clipped FT output is rounded to [0,127].
//! Dense weights use per-output-row scale 127/max_abs into i8, exact i32
//! dot products, float biases, and clipped activations rounded to [0,127].
//! Utility is 2*sigmoid(logit)-1, clamped to [-0.95,0.95]. UCI accepts only
//! nets whose conservative feature bound fits the integer accumulator.
use crate::{
    features::{self, FeatureSet},
    Seat, State,
};
use std::{io, path::Path};
#[derive(Debug)]
pub struct Net {
    pub hidden: usize,
    ft: Vec<f32>,
    bias: Vec<f32>,
    w1: Vec<f32>,
    b1: Vec<f32>,
    w2: Vec<f32>,
    b2: Vec<f32>,
    w3: Vec<f32>,
    b3: f32,
    quantized: Quantized,
}
impl Net {
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::from_bytes(&std::fs::read(path)?)
    }
    pub fn from_bytes(data: &[u8]) -> io::Result<Self> {
        let bad = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid BQNN header, length or weights",
            )
        };
        if data.len() < 24 || &data[..4] != b"BQNN" {
            return Err(bad());
        }
        let word = |i| u32::from_le_bytes(data[i..i + 4].try_into().unwrap()) as usize;
        let h = word(12);
        if word(4) != 2
            || word(8) != features::N_FEATURES
            || h == 0
            || h > 4096
            || word(16) != 16
            || word(20) != 32
        {
            return Err(bad());
        }
        let count = features::N_FEATURES * h + h + 16 * 2 * h + 16 + 32 * 16 + 32 + 32 + 1;
        if data.len() != 24 + 4 * count {
            return Err(bad());
        }
        let vals: Vec<f32> = data[24..]
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        if vals.iter().any(|x| !x.is_finite() || x.abs() > 1e6) {
            return Err(bad());
        }
        let mut offset = 0;
        let mut take = |n| {
            let a = vals[offset..offset + n].to_vec();
            offset += n;
            a
        };
        let mut net = Self {
            hidden: h,
            ft: take(features::N_FEATURES * h),
            bias: take(h),
            w1: take(32 * h),
            b1: take(16),
            w2: take(512),
            b2: take(32),
            w3: take(32),
            b3: take(1)[0],
            quantized: Quantized::default(),
        };
        net.quantized = Quantized::new(&net);
        Ok(net)
    }
    /// Float logit for comparison with the integer evaluation.
    pub fn logit(&self, state: &State) -> f32 {
        self.logit_reference(state)
    }
    /// Float full-refresh evaluation.
    pub fn logit_reference(&self, state: &State) -> f32 {
        let mut input = Vec::with_capacity(2 * self.hidden);
        for seat in [state.turn(), state.turn().other()] {
            let mut a = self.bias.clone();
            for index in features::active(state, seat) {
                for (x, w) in a
                    .iter_mut()
                    .zip(&self.ft[index as usize * self.hidden..][..self.hidden])
                {
                    *x += w;
                }
            }
            input.extend(a.into_iter().map(|x| x.clamp(0., 1.)));
        }
        let dense = |x: &[f32], w: &[f32], b: &[f32]| -> Vec<f32> {
            w.chunks_exact(x.len())
                .zip(b)
                .map(|(row, bias)| {
                    row.iter()
                        .zip(x)
                        .fold(*bias, |s, (w, x)| s + w * x)
                        .clamp(0., 1.)
                })
                .collect()
        };
        let a = dense(&input, &self.w1, &self.b1);
        let b = dense(&a, &self.w2, &self.b2);
        self.w3.iter().zip(b).fold(self.b3, |s, (w, x)| s + w * x)
    }
    pub fn utility_reference(&self, state: &State) -> f64 {
        utility(self.logit_reference(state))
    }
    pub fn utility(&self, state: &State) -> f64 {
        if !self.can_quantize() {
            return self.utility_reference(state);
        }
        let mut acc = Accumulator::default();
        self.refresh(state, &mut acc);
        self.utility_accumulator(&acc, state.turn())
    }
    pub fn can_quantize(&self) -> bool {
        self.quantized.safe
    }
    pub fn refresh(&self, state: &State, acc: &mut Accumulator) {
        self.refresh_features(FeatureSet::both(state), acc);
    }
    fn refresh_features(&self, features: [FeatureSet; 2], acc: &mut Accumulator) {
        acc.features = features;
        for (seat, feature_set) in features.iter().enumerate() {
            let values = &mut acc.values[seat][..self.hidden];
            values.copy_from_slice(&self.quantized.bias);
            for index in feature_set.indices() {
                self.apply_row(values, index, true);
            }
        }
    }
    fn apply_row(&self, values: &mut [i16], index: usize, add: bool) {
        let weights = &self.quantized.ft[index * self.hidden..][..self.hidden];
        if add {
            for (v, w) in values.iter_mut().zip(weights) {
                *v = v.wrapping_add(*w);
            }
        } else {
            for (v, w) in values.iter_mut().zip(weights) {
                *v = v.wrapping_sub(*w);
            }
        }
    }
    pub fn update(&self, parent: &Accumulator, state: &State, next: &mut Accumulator) {
        self.update_features(parent, FeatureSet::both(state), next);
    }
    fn update_features(
        &self,
        parent: &Accumulator,
        features: [FeatureSet; 2],
        next: &mut Accumulator,
    ) {
        let changed: u32 = features
            .iter()
            .zip(parent.features)
            .map(|(a, b)| {
                a.0.iter()
                    .zip(b.0)
                    .map(|(x, y)| (x ^ y).count_ones())
                    .sum::<u32>()
            })
            .sum();
        // Colour binding and large clock resets are cheaper as a full refresh.
        let active: u32 = features
            .iter()
            .map(|f| f.0.iter().map(|x| x.count_ones()).sum::<u32>())
            .sum();
        if changed > active {
            self.refresh_features(features, next);
            return;
        }
        next.features = features;
        for (seat, feature_set) in features.iter().enumerate() {
            let values = &mut next.values[seat][..self.hidden];
            values.copy_from_slice(&parent.values[seat][..self.hidden]);
            for word in 0..features::N_FEATURES / 64 {
                let before = parent.features[seat].0[word];
                let after = feature_set.0[word];
                let mut bits = before ^ after;
                while bits != 0 {
                    let bit = bits.trailing_zeros();
                    bits &= bits - 1;
                    self.apply_row(values, word * 64 + bit as usize, after & (1 << bit) != 0);
                }
            }
        }
    }
    pub fn logit_accumulator(&self, acc: &Accumulator, mover: Seat) -> f32 {
        let mut input = [0i8; 2 * MAX_HIDDEN];
        for (dst, seat) in input[..2 * self.hidden]
            .chunks_exact_mut(self.hidden)
            .zip([mover as usize, mover.other() as usize])
        {
            for (x, &v) in dst.iter_mut().zip(&acc.values[seat][..self.hidden]) {
                *x = ((v.clamp(0, 254) + 1) / 2) as i8;
            }
        }
        let a: [i8; 16] = self
            .quantized
            .l1
            .forward(&input[..2 * self.hidden], &self.b1);
        let b: [i8; 32] = self.quantized.l2.forward(&a, &self.b2);
        dot(&b, &self.quantized.out.weights) as f32 * self.quantized.out.inverse[0] + self.b3
    }
    pub fn utility_accumulator(&self, acc: &Accumulator, mover: Seat) -> f64 {
        utility(self.logit_accumulator(acc, mover))
    }
}

fn utility(logit: f32) -> f64 {
    (2.0 / (1.0 + (-(logit as f64)).exp()) - 1.0).clamp(-0.95, 0.95)
}

pub const MAX_HIDDEN: usize = 4096;
/// Indexed by absolute seat, never swapped when the side to move changes.
/// Reused between leaf evaluations; only the loaded width is copied/updated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accumulator {
    pub values: [[i16; MAX_HIDDEN]; 2],
    pub features: [FeatureSet; 2],
}
impl Default for Accumulator {
    fn default() -> Self {
        Self {
            values: [[0; MAX_HIDDEN]; 2],
            features: [FeatureSet::default(); 2],
        }
    }
}

#[derive(Debug, Default)]
struct Quantized {
    ft: Vec<i16>,
    bias: Vec<i16>,
    l1: Layer,
    l2: Layer,
    out: Layer,
    safe: bool,
}
impl Quantized {
    fn new(net: &Net) -> Self {
        let quantize = |x: &f32| (x * 254.).round().clamp(-32767., 32767.) as i16;
        let ft: Vec<_> = net.ft.iter().map(quantize).collect();
        let bias: Vec<_> = net.bias.iter().map(quantize).collect();
        // MAX_ACTIVE public features per perspective. Reject integer overflow risk conservatively,
        // retaining the float reference for unusually large valid weights.
        let safe = (0..net.hidden).all(|i| {
            let max = net
                .ft
                .chunks_exact(net.hidden)
                .map(|row| row[i].abs())
                .fold(0f32, f32::max);
            (max * 254.).ceil() * features::MAX_ACTIVE as f32 + (net.bias[i].abs() * 254.).ceil()
                < 32767.
        });
        Self {
            ft,
            bias,
            l1: Layer::new(&net.w1, 2 * net.hidden),
            l2: Layer::new(&net.w2, 16),
            out: Layer::new(&net.w3, 32),
            safe,
        }
    }
}
#[derive(Debug, Default)]
struct Layer {
    weights: Vec<i8>,
    inverse: Vec<f32>,
}
impl Layer {
    fn new(weights: &[f32], width: usize) -> Self {
        let mut out = Self::default();
        for row in weights.chunks_exact(width) {
            let max = row.iter().map(|w| w.abs()).fold(0f32, f32::max);
            let scale = if max > 0. { 127. / max } else { 1. };
            out.weights.extend(
                row.iter()
                    .map(|w| (w * scale).round().clamp(-127., 127.) as i8),
            );
            out.inverse.push(1. / (127. * scale));
        }
        out
    }
    fn forward<const N: usize>(&self, input: &[i8], bias: &[f32]) -> [i8; N] {
        let mut output = [0; N];
        for (i, row) in self.weights.chunks_exact(input.len()).enumerate() {
            let value = dot(input, row) as f32 * self.inverse[i] + bias[i];
            output[i] = (value.clamp(0., 1.) * 127.).round() as i8;
        }
        output
    }
}
/// Widen before multiplying: exact int8 x int8 -> int32, with no saturating
/// intermediate sums. LLVM vectorizes this on both aarch64 and x86_64.
fn dot(input: &[i8], weights: &[i8]) -> i32 {
    input
        .iter()
        .zip(weights)
        .map(|(&a, &b)| a as i32 * b as i32)
        .sum()
}
