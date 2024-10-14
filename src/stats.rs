use std::fmt::Display;
use std::time::Duration;

use owo_colors::{AnsiColors, DynColors, OwoColorize};

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub average: f64,
    pub standard_dev: f64,
    pub minimum: f64,
    pub maximum: f64,
}

impl Stats {
    pub fn format_millis(&self) -> PrintStatsMillis {
        PrintStatsMillis { stats: self }
    }

    pub fn from_secs_f64(secs: &[f64]) -> Option<Stats> {
        if secs.is_empty() {
            return None;
        }

        let (min, max, sum) = secs
            .iter()
            .fold((f64::MAX, f64::MIN, 0.0), |(min, max, sum), &x| {
                (min.min(x), max.max(x), sum + x)
            });
        let average = sum / secs.len() as f64;

        let std_dev = {
            let sum: f64 = secs.iter().map(|&x| (x - average).powi(2)).sum();
            (sum / secs.len() as f64).sqrt()
        };

        Some(Stats {
            average,
            standard_dev: std_dev,
            minimum: min,
            maximum: max,
        })
    }

    pub fn from_durations<I>(durations: I) -> Option<Stats>
    where
        I: Iterator<Item = Duration>,
    {
        let secs = durations.map(|d| d.as_secs_f64()).collect::<Vec<_>>();
        Stats::from_secs_f64(&secs)
    }
}

pub struct PrintStatsMillis<'a> {
    stats: &'a Stats,
}
impl Display for PrintStatsMillis<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn rtt_color(secs: f64) -> DynColors {
            match secs {
                secs if secs <= 0.025 => DynColors::Ansi(AnsiColors::Green),
                secs if secs <= 0.100 => DynColors::Ansi(AnsiColors::Yellow),
                _ => DynColors::Ansi(AnsiColors::Red),
            }
        }
        fn std_dev_color(secs: f64) -> DynColors {
            match secs {
                secs if secs <= 0.003 => DynColors::Ansi(AnsiColors::Green),
                secs if secs <= 0.010 => DynColors::Ansi(AnsiColors::Yellow),
                _ => DynColors::Ansi(AnsiColors::Red),
            }
        }

        write!(
            f,
            "{}ms ",
            format!("{:>5.1}", self.stats.average * 1e3).color(rtt_color(self.stats.average)),
        )?;
        write!(
            f,
            "±{}ms (min: ",
            format!("{:>5.1}", self.stats.standard_dev * 1e3)
                .color(std_dev_color(self.stats.standard_dev)),
        )?;
        write!(
            f,
            "{}ms, max: ",
            format!("{:>4.0}", self.stats.minimum * 1e3).color(rtt_color(self.stats.minimum))
        )?;
        write!(
            f,
            "{}ms)",
            format!("{:>4.0}", self.stats.maximum * 1e3).color(rtt_color(self.stats.maximum))
        )
    }
}
