use std::fmt::{Display, Write as _};
use std::time::Duration;

use colorgrad::{Gradient as _, LinearGradient};
use owo_colors::OwoColorize;

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
        let rtt_grad = colorgrad::GradientBuilder::new()
            .colors(&[
                colorgrad::Color::from_hsva(120.0, 1.0, 1.0, 1.0),
                colorgrad::Color::from_hsva(60.0, 1.0, 1.0, 1.0),
                colorgrad::Color::from_hsva(0.0, 1.0, 1.0, 1.0),
            ])
            .domain(&[10.0, 25.0, 50.0])
            .build::<colorgrad::LinearGradient>()
            .unwrap();

        let std_dev_grad = colorgrad::GradientBuilder::new()
            .colors(&[
                colorgrad::Color::from_hsva(120.0, 1.0, 1.0, 1.0),
                colorgrad::Color::from_hsva(60.0, 1.0, 1.0, 1.0),
                colorgrad::Color::from_hsva(0.0, 1.0, 1.0, 1.0),
            ])
            .domain(&[1.0, 3.0, 6.0])
            .build::<colorgrad::LinearGradient>()
            .unwrap();

        let mut buffer = String::with_capacity(128);
        let mut fmt_col = |rtt: f64, grad: &LinearGradient| {
            let [r, g, b, _] = grad.at((rtt as f32) * 1e3).to_rgba8();
            buffer.clear();
            write!(buffer, "{:>5.1}ms", rtt * 1e3).unwrap();
            buffer.color(owo_colors::Rgb(r, g, b)).to_string()
        };

        write!(
            f,
            "{} ±{} (min: {}, max: {})",
            fmt_col(self.stats.average, &rtt_grad),
            fmt_col(self.stats.standard_dev, &std_dev_grad),
            fmt_col(self.stats.minimum, &rtt_grad),
            fmt_col(self.stats.maximum, &rtt_grad),
        )
    }
}
