use std::cmp::Ordering;
use std::fmt;
use std::fmt::Display;
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct Stats {
    /// Average
    pub avg: f32,
    /// Stdandard deviation
    pub std_dev: f32,
    /// Minimum
    pub min: f32,
    /// Maximum
    pub max: f32,
}

impl PartialEq for Stats {
    /// Compare by `avg`-field assuming it always contains a valid float
    fn eq(&self, other: &Self) -> bool {
        return self.avg.eq(&other.avg);
    }
}
impl Eq for Stats {}

impl PartialOrd for Stats {
    /// Compare by `avg`-field assuming it always contains a valid float
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        return self.avg.partial_cmp(&other.avg);
    }
}
impl Ord for Stats {
    /// Compare by `avg`-field assuming it always contains a valid float
    fn cmp(&self, other: &Self) -> Ordering {
        return self.avg.partial_cmp(&other.avg).unwrap();
    }
}

impl Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let std_dev = format!("± {:.0}ms", self.std_dev);
        write!(
            f,
            "{:>3.0}ms {:<6} (min: {:>3.0}ms, max: {:>3.0}ms)",
            self.avg, std_dev, self.min, self.max
        )
    }
}

impl Stats {
    pub fn new<I: ExactSizeIterator<Item = f32>>(mut ms: I) -> Stats {
        let count = ms.len();

        if count == 0 {
            return Stats {
                avg: 0_f32,
                std_dev: 0_f32,
                min: 0_f32,
                max: 0_f32,
            };
        } else if count == 1 {
            let val = ms.next().unwrap();
            return Stats {
                avg: val,
                std_dev: 0_f32,
                min: val,
                max: val,
            };
        }

        let mut sum = 0_f32;
        let mut sum_of_sq = 0_f32;
        let mut max = -f32::MAX;
        let mut min = f32::MAX;

        for ms in ms {
            sum += ms;
            sum_of_sq += ms * ms;
            max = f32::max(max, ms);
            min = f32::min(min, ms);
        }

        let sum_sq_avg = (sum * sum) / count as f32;
        let std_dev = ((sum_of_sq - sum_sq_avg) / (count - 1) as f32).sqrt();
        let avg = sum / count as f32;

        Stats {
            avg,
            std_dev,
            min,
            max,
        }
    }

    pub fn from_durations<'a, I>(durs: I) -> Stats
    where
        I: IntoIterator<Item = &'a Duration>,
        <I as IntoIterator>::IntoIter: ExactSizeIterator,
    {
        let ms = durs.into_iter().map(|d| d.as_secs_f32() * 1000_f32);
        return Self::new(ms);
    }

    /// By default, `Some(_)` is always greater than `None`,
    /// This function reverses this behavious and sets `None` always greater than `Some(_)`.
    pub fn cmp(lhs: &Option<Stats>, rhs: &Option<Stats>) -> Ordering {
        match (lhs, rhs) {
            (None, None) => Ordering::Equal,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(lhs), Some(rhs)) => lhs.cmp(rhs),
        }
    }
}
