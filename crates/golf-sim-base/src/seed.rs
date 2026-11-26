use std::f64::consts::TAU;

use crate::{Real, Seed, fm};

const FNV_OFFSET: u64 = 0xCBF2_9CE4_8422_2325;
const FNV_PRIME: u64 = 1_099_511_628_211;
const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;
const UNIT: Real = 1.0 / (1_u64 << 53) as Real;

impl Seed {
    pub fn derive(self, label: &str) -> Self {
        let mut hash = FNV_OFFSET ^ self.0 as u64;
        for byte in label.bytes() {
            hash = (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME);
        }
        Self(mix(hash) as i64)
    }
}

#[derive(Default)]
pub struct SeedStream {
    state: u64,
    spare: Option<Real>,
}

impl SeedStream {
    pub const fn new(seed: Seed) -> Self {
        Self {
            state: seed.0 as u64,
            spare: None,
        }
    }

    pub fn derived(parent: Seed, label: &str) -> Self {
        Self::new(parent.derive(label))
    }

    pub fn next_int(&mut self) -> i64 {
        self.state = self.state.wrapping_add(GAMMA);
        mix(self.state) as i64
    }

    pub fn next_float(&mut self) -> Real {
        ((self.next_int() as u64) >> 11) as Real * UNIT
    }

    pub fn next_range(&mut self, low: Real, high: Real) -> Real {
        low + (high - low) * self.next_float()
    }

    pub fn next_gaussian(&mut self) -> Real {
        if let Some(spare) = self.spare.take() {
            return spare;
        }

        let radius = fm::sqrt(-2.0 * fm::log(1.0 - self.next_float()));
        let angle = TAU * self.next_float();
        self.spare = Some(radius * fm::sin(angle));
        radius * fm::cos(angle)
    }

    pub fn next_sign(&mut self) -> Real {
        if self.next_float() < 0.5 { 1.0 } else { -1.0 }
    }
}

fn mix(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}
