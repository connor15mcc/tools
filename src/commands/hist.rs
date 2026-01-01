use std::{
    fmt,
    io::{BufRead, BufReader, Read},
};

use anyhow::{bail, Result};
use clap::Parser;
use clap_stdin::FileOrStdin;
use num_format::{Locale, ToFormattedString};
use statistical::{mean, median, standard_deviation};

use crate::command::CommandRunner;

#[derive(Parser)]
#[command(
    name = "hist",
    about = "Generate a text-based histogram from numerical data"
)]
pub struct Hist {
    /// File from which to read, defaulting to stdin
    #[clap(default_value = "-")]
    input: FileOrStdin,

    #[arg(short, long, default_value = "10")]
    buckets: usize,

    #[arg(long)]
    log: bool,

    #[arg(short, long, default_value = "50")]
    width: usize,

    #[arg(short = 'S', long)]
    no_stats: bool,
}

impl CommandRunner for Hist {
    fn run(self) -> anyhow::Result<()> {
        let data = NumberParser::read_from_input(self.input.into_reader()?)?;

        if data.is_empty() {
            bail!("No valid numbers found in input");
        }

        let histogram = {
            let strategy = match self.log {
                true => BucketStrategy::Logarithmic,
                false => BucketStrategy::Linear,
            };
            let config = HistogramConfig::new(self.buckets, strategy, self.width);
            Histogram::new(data, config)?
        };

        if !self.no_stats {
            println!("{}", histogram.statistics());
            println!();
        }

        println!("{}", histogram);

        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
enum BucketStrategy {
    Linear,
    Logarithmic,
}

#[derive(Debug, Clone)]
struct Bucket {
    range: (f64, f64),
    count: usize,
}

impl Bucket {
    fn new(start: f64, end: f64) -> Self {
        Self {
            range: (start, end),
            count: 0,
        }
    }

    fn contains(&self, value: f64, is_last: bool) -> bool {
        if is_last {
            value >= self.range.0 && value <= self.range.1
        } else {
            value >= self.range.0 && value < self.range.1
        }
    }

    fn increment(&mut self) {
        self.count += 1;
    }
}

#[derive(Debug, Clone)]
struct Statistics {
    count: usize,
    mean: f64,
    median: f64,
    std_dev: f64,
    min: f64,
    max: f64,
}

impl Statistics {
    fn from_data(data: &[f64]) -> Self {
        let count = data.len();
        let mean_val = mean(data);
        let median_val = median(data);
        let std_dev = standard_deviation(data, Some(mean_val));
        let min = data.iter().copied().fold(f64::INFINITY, f64::min);
        let max = data.iter().copied().fold(f64::NEG_INFINITY, f64::max);

        Self {
            count,
            mean: mean_val,
            median: median_val,
            std_dev,
            min,
            max,
        }
    }
}

impl fmt::Display for Statistics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Statistics:")?;
        writeln!(
            f,
            "  Count:      {}",
            self.count.to_formatted_string(&Locale::en)
        )?;
        writeln!(f, "  Mean:       {}", format_number(self.mean))?;
        writeln!(f, "  Median:     {}", format_number(self.median))?;
        writeln!(f, "  Std Dev:    {}", format_number(self.std_dev))?;
        writeln!(f, "  Min:        {}", format_number(self.min))?;
        write!(f, "  Max:        {}", format_number(self.max))
    }
}

#[derive(Debug)]
struct HistogramConfig {
    bucket_count: usize,
    strategy: BucketStrategy,
    bar_width: usize,
}

impl HistogramConfig {
    fn new(bucket_count: usize, strategy: BucketStrategy, bar_width: usize) -> Self {
        Self {
            bucket_count,
            strategy,
            bar_width,
        }
    }
}

#[derive(Debug)]
struct Histogram {
    config: HistogramConfig,
    buckets: Vec<Bucket>,
    stats: Statistics,
}

impl Histogram {
    fn new(data: Vec<f64>, config: HistogramConfig) -> Result<Self> {
        if data.is_empty() {
            bail!("Cannot create histogram from empty data");
        }

        let stats = Statistics::from_data(&data);
        let mut buckets = Self::create_buckets(&stats, &config)?;
        Self::fill_buckets(&data, &mut buckets);

        Ok(Self {
            config,
            buckets,
            stats,
        })
    }

    fn create_buckets(stats: &Statistics, config: &HistogramConfig) -> Result<Vec<Bucket>> {
        if stats.min == stats.max {
            bail!("All values are identical: {}", format_number(stats.min));
        }

        match config.strategy {
            BucketStrategy::Linear => Ok(Self::create_linear_buckets(
                stats.min,
                stats.max,
                config.bucket_count,
            )),
            BucketStrategy::Logarithmic => {
                Self::create_log_buckets(stats.min, stats.max, config.bucket_count)
            }
        }
    }

    fn create_linear_buckets(min: f64, max: f64, count: usize) -> Vec<Bucket> {
        let step = (max - min) / count as f64;
        (0..count)
            .map(|i| {
                let start = min + i as f64 * step;
                let end = if i == count - 1 { max } else { start + step };
                Bucket::new(start, end)
            })
            .collect()
    }

    fn create_log_buckets(min: f64, max: f64, count: usize) -> Result<Vec<Bucket>> {
        if min <= 0.0 {
            eprintln!(
                "Warning: Logarithmic bucketing requires positive values. Using linear bucketing."
            );
            return Ok(Self::create_linear_buckets(min, max, count));
        }

        let log_min = min.ln();
        let log_max = max.ln();
        let step = (log_max - log_min) / count as f64;

        let buckets = (0..count)
            .map(|i| {
                let start = (log_min + i as f64 * step).exp();
                let end = if i == count - 1 {
                    max
                } else {
                    (log_min + (i + 1) as f64 * step).exp()
                };
                Bucket::new(start, end)
            })
            .collect();

        Ok(buckets)
    }

    fn fill_buckets(data: &[f64], buckets: &mut [Bucket]) {
        let bucket_count = buckets.len();
        for &value in data {
            for (i, bucket) in buckets.iter_mut().enumerate() {
                if bucket.contains(value, i == bucket_count - 1) {
                    bucket.increment();
                    break;
                }
            }
        }
    }

    fn max_bucket_count(&self) -> usize {
        self.buckets.iter().map(|b| b.count).max().unwrap_or(0)
    }

    fn statistics(&self) -> &Statistics {
        &self.stats
    }
}

impl fmt::Display for Histogram {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let max_label_width = self
            .buckets
            .iter()
            .map(|b| {
                format!(
                    "[{}, {})",
                    format_number(b.range.0),
                    format_number(b.range.1)
                )
                .len()
            })
            .max()
            .unwrap_or(0);

        let max_count_width = self
            .buckets
            .iter()
            .map(|b| b.count.to_formatted_string(&Locale::en).len())
            .max()
            .unwrap_or(0);

        let max_count = self.max_bucket_count();

        for bucket in &self.buckets {
            let label = format!(
                "[{}, {})",
                format_number(bucket.range.0),
                format_number(bucket.range.1)
            );
            let bar_length = if max_count > 0 {
                (bucket.count as f64 / max_count as f64 * self.config.bar_width as f64) as usize
            } else {
                0
            };
            let bar = "#".repeat(bar_length);
            let count_str = bucket.count.to_formatted_string(&Locale::en);

            writeln!(
                f,
                "{:label_width$} | {:>count_width$} {}",
                label,
                count_str,
                bar,
                label_width = max_label_width,
                count_width = max_count_width
            )?;
        }

        Ok(())
    }
}

struct NumberParser;

impl NumberParser {
    fn read_from_input<R: Read>(reader: R) -> Result<Vec<f64>> {
        let reader = BufReader::new(reader);
        let mut numbers = Vec::new();

        for line in reader.lines() {
            let line = line?;
            for token in line.split_whitespace() {
                if let Ok(num) = token.parse::<f64>() {
                    if num.is_finite() {
                        numbers.push(num);
                    }
                }
            }
        }

        Ok(numbers)
    }
}

fn format_number(n: f64) -> String {
    if n.abs() >= 1_000_000.0 || (n.abs() < 0.001 && n != 0.0) {
        format!("{:.2e}", n)
    } else if n.fract() == 0.0 && n.abs() < 1_000_000.0 {
        let int_val = n as i64;
        int_val.to_formatted_string(&Locale::en)
    } else {
        format!("{:.2}", n)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn linear_buckets_cover_range() {
        let buckets = Histogram::create_linear_buckets(0.0, 100.0, 10);
        assert_eq!(buckets.len(), 10);
        assert_eq!(buckets[0].range.0, 0.0);
        assert_eq!(buckets[9].range.1, 100.0);
    }

    #[test]
    fn linear_buckets_no_gaps() {
        let buckets = Histogram::create_linear_buckets(0.0, 100.0, 10);
        for i in 0..buckets.len() - 1 {
            assert_eq!(buckets[i].range.1, buckets[i + 1].range.0);
        }
    }

    #[test]
    fn log_buckets_falls_back_on_negative() {
        let buckets = Histogram::create_log_buckets(-10.0, 10.0, 5).unwrap();
        assert_eq!(buckets.len(), 5);
    }

    #[test]
    fn parse_whitespace_separated() {
        let input = "1 2 3 4 5";
        let numbers = NumberParser::read_from_input(Cursor::new(input)).unwrap();
        assert_eq!(numbers, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn parse_ignores_invalid() {
        let input = "1 foo 2 bar 3";
        let numbers = NumberParser::read_from_input(Cursor::new(input)).unwrap();
        assert_eq!(numbers, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn bucket_contains_boundary_handling() {
        let bucket = Bucket::new(0.0, 10.0);
        assert!(bucket.contains(0.0, false));
        assert!(!bucket.contains(10.0, false));
        assert!(bucket.contains(10.0, true));
    }
}
