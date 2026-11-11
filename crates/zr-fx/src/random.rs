//! `GameClientRandomVariable` (ClientRandomValue.h / RandomValue.cpp): a
//! `low high [distribution]` range. Only `CONSTANT` and `UNIFORM` are
//! implemented by the engine; the others are accepted but return 0.

/// Distribution of a [`RandomValue`] (`GameClientRandomVariable::DistributionType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Distribution {
    Constant,
    #[default]
    Uniform,
    Gaussian,
    Triangular,
    LowBias,
    HighBias,
}

impl Distribution {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_uppercase().as_str() {
            "CONSTANT" => Distribution::Constant,
            "UNIFORM" => Distribution::Uniform,
            "GAUSSIAN" => Distribution::Gaussian,
            "TRIANGULAR" => Distribution::Triangular,
            "LOW_BIAS" => Distribution::LowBias,
            "HIGH_BIAS" => Distribution::HighBias,
            _ => return None,
        })
    }
}

/// A `GameClientRandomVariable`: a constant when `low == high`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RandomValue {
    pub low: f32,
    pub high: f32,
    pub distribution: Distribution,
}

impl Default for RandomValue {
    fn default() -> Self {
        Self::constant(0.0)
    }
}

impl RandomValue {
    pub const fn constant(v: f32) -> Self {
        Self { low: v, high: v, distribution: Distribution::Constant }
    }

    /// Parses the INI form `low high [distribution]` (distribution defaults to
    /// `UNIFORM`). A single token is treated as a constant.
    pub fn parse(s: &str) -> Option<Self> {
        let mut it = s.split_whitespace();
        let low: f32 = it.next()?.parse().ok()?;
        let high: f32 = it.next().and_then(|t| t.parse().ok()).unwrap_or(low);
        let distribution = it.next().and_then(Distribution::parse).unwrap_or(if low == high {
            Distribution::Constant
        } else {
            Distribution::Uniform
        });
        Some(Self { low, high, distribution })
    }

    pub fn min(&self) -> f32 {
        self.low.min(self.high)
    }

    pub fn max(&self) -> f32 {
        self.low.max(self.high)
    }

    /// A value from the distribution. `Constant` returns `low`; `Uniform` is a
    /// client random in `[low, high]`; unsupported distributions return 0
    /// (`GameClientRandomVariable::getValue`).
    pub fn get(&self, rng: &mut ClientRng) -> f32 {
        match self.distribution {
            Distribution::Constant => self.low,
            Distribution::Uniform => rng.real(self.low, self.high),
            _ => 0.0,
        }
    }
}

/// A tiny xorshift client RNG. Client-side effects must never touch the
/// simulation RNG, so this is seeded independently.
pub struct ClientRng {
    state: u64,
}

impl ClientRng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed | 1 }
    }

    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        (x >> 32) as u32
    }

    /// Uniform in `[lo, hi)`.
    pub fn real(&mut self, lo: f32, hi: f32) -> f32 {
        let u = self.next_u32() as f32 / u32::MAX as f32;
        lo + (hi - lo) * u
    }

    /// Uniform integer in `[lo, hi]`.
    pub fn int(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u32() % ((hi - lo + 1) as u32)) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_forms() {
        let v = RandomValue::parse("1.0 2.0").unwrap();
        assert_eq!(v.distribution, Distribution::Uniform);
        let v = RandomValue::parse("3.5 3.5").unwrap();
        assert_eq!(v.distribution, Distribution::Constant);
        assert_eq!(v.get(&mut ClientRng::new(1)), 3.5);
        let v = RandomValue::parse("-1 1 CONSTANT").unwrap();
        assert_eq!(v.distribution, Distribution::Constant);
        assert!(RandomValue::parse("").is_none());
    }

    #[test]
    fn uniform_in_range() {
        let v = RandomValue::parse("0 10").unwrap();
        let mut rng = ClientRng::new(12345);
        for _ in 0..1000 {
            let x = v.get(&mut rng);
            assert!((0.0..=10.0).contains(&x), "{x}");
        }
    }
}
