use std::fmt::Display;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RunningStats {
    sum: f64,
    sum_of_squares: f64,
    minimum: f64,
    maximum: f64,
    samples: usize,
}

impl RunningStats {
    pub fn new() -> RunningStats {
        RunningStats {
            sum: 0.0,
            sum_of_squares: 0.0,
            minimum: f64::MAX,
            maximum: f64::MIN,
            samples: 0,
        }
    }

    pub fn update(&mut self, next: f64) {
        self.sum += next;
        self.sum_of_squares += next * next;
        self.minimum = self.minimum.min(next);
        self.maximum = self.maximum.max(next);
        self.samples += 1;
    }

    pub fn finish(self) -> Option<Stats> {
        if self.samples < 2 {
            return None;
        }

        let sq_sum_avg = (self.sum * self.sum) / self.samples as f64;
        Some(Stats {
            average: self.sum / self.samples as f64,
            standard_dev: ((self.sum_of_squares - sq_sum_avg) / (self.samples - 1) as f64).sqrt(),
            minimum: self.minimum,
            maximum: self.maximum,
        })
    }
}

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub average: f64,
    pub standard_dev: f64,
    pub minimum: f64,
    pub maximum: f64,
}

impl Stats {
    #[allow(dead_code)]
    pub fn format_seconds(&self) -> PrintStatsSeconds {
        PrintStatsSeconds { stats: self }
    }
    pub fn format_millis(&self) -> PrintStatsMillis {
        PrintStatsMillis { stats: self }
    }
    pub fn from_durations<I>(durations: I) -> Option<Stats>
    where
        I: Iterator<Item = Duration>,
    {
        let mut stats = RunningStats::new();
        durations.for_each(|duration| stats.update(duration.as_secs_f64()));
        stats.finish()
    }
}

pub struct PrintStatsSeconds<'a> {
    stats: &'a Stats,
}
impl<'a> Display for PrintStatsSeconds<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:>5.3}s ±{:>5.3}s (min: {:>5.3}s, max: {:>5.3}s)",
            self.stats.average, self.stats.standard_dev, self.stats.minimum, self.stats.maximum
        )
    }
}

pub struct PrintStatsMillis<'a> {
    stats: &'a Stats,
}
impl<'a> Display for PrintStatsMillis<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:>4.0}ms ±{:>4.0}ms (min: {:>4.0}ms, max: {:>4.0}ms)",
            self.stats.average * 1e3,
            self.stats.standard_dev * 1e3,
            self.stats.minimum * 1e3,
            self.stats.maximum * 1e3
        )
    }
}
