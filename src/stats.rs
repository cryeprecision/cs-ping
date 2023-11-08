use std::cmp::Ordering;
use std::fmt;
use std::fmt::Display;

#[derive(Debug, Default, Clone)]
pub struct RunningStats {
    sum: f64,
    sum_of_squares: f64,
    minimum: f64,
    maximum: f64,
    samples: usize,
}

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub average: f64,
    pub standard_dev: f64,
    pub minimum: f64,
    pub maximum: f64,
    pub samples: usize,
}

impl RunningStats {
    pub fn update(&mut self, next: f64) {
        self.sum += next;
        self.sum_of_squares += next * next;
        self.minimum = self.minimum.min(next);
        self.maximum = self.maximum.max(next);
        self.samples += 1;
    }
    pub fn update_iter(&mut self, iter: impl Iterator<Item = f64>) {
        iter.for_each(|num| self.update(num));
    }
    pub fn finish(self) -> Stats {
        let sq_sum_avg = (self.sum * self.sum) / self.samples as f64;
        Stats {
            average: self.sum / self.samples as f64,
            standard_dev: ((self.sum_of_squares - sq_sum_avg) / (self.samples - 1) as f64).sqrt(),
            minimum: self.minimum,
            maximum: self.maximum,
            samples: self.samples,
        }
    }
}

impl Stats {
    /// By default, `Some(_)` is always greater than `None`,
    /// This function reverses this behaviour and sets `None` always greater than `Some(_)`.
    pub fn cmp(lhs: &Option<Stats>, rhs: &Option<Stats>) -> Ordering {
        match (lhs, rhs) {
            (None, None) => Ordering::Equal,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(lhs), Some(rhs)) => lhs.cmp(rhs),
        }
    }
}

impl PartialEq for Stats {
    /// Compare by `average`-field assuming it always contains a valid float
    fn eq(&self, other: &Self) -> bool {
        return self.average.eq(&other.average);
    }
}
impl Eq for Stats {}

impl PartialOrd for Stats {
    /// Compare by `average`-field assuming it always contains a valid float
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        return self.average.partial_cmp(&other.average);
    }
}

impl Ord for Stats {
    /// Compare by `average`-field assuming it always contains a valid float
    fn cmp(&self, other: &Self) -> Ordering {
        return self.average.partial_cmp(&other.average).unwrap();
    }
}

impl Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let standard_dev = format!("± {:.0}ms", self.standard_dev);
        write!(
            f,
            "{:>3.0}ms {:<6} (min: {:>3.0}ms, max: {:>3.0}ms)",
            self.average, standard_dev, self.minimum, self.maximum
        )
    }
}
