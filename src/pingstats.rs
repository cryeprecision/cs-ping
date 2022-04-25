use std::cmp::Ordering;
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

impl Stats {
    pub fn new<I: ExactSizeIterator<Item = f32>>(ms: I) -> Stats {
        let count = ms.len();
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

        Stats {
            avg: sum / (count as f32),
            std_dev: ((sum_of_sq - sum_sq_avg) / (count - 1) as f32).sqrt(),
            min,
            max,
        }
    }
    pub fn new_from_durations<I: ExactSizeIterator<Item = Duration>>(durs: I) -> Stats {
        println!("{}", durs.len());
        let ms = durs.map(|d| d.as_secs_f32() * 1000_f32);
        return Self::new(ms);
    }
}
