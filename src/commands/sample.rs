use crate::command::CommandRunner;
use anyhow::Result;
use clap::Parser;
use clap_stdin::FileOrStdin;
use log::warn;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::io::{BufRead, BufReader, Read, Write};

fn validate_rate(s: &str) -> Result<f64, String> {
    let val: f64 = s.parse().map_err(|_| format!("'{}' is not a valid number", s))?;
    if !(0.0..=1.0).contains(&val) {
        return Err(format!("rate must be between 0.0 and 1.0, got {}", val));
    }
    Ok(val)
}

#[derive(Parser)]
#[command(
    name = "sample",
    about = "Sample lines from input using various strategies"
)]
pub struct Sample {
    /// Output every Nth line (deterministic sampling)
    #[arg(long, conflicts_with_all = ["rate", "count"], value_parser = clap::value_parser!(u64).range(1..))]
    every: Option<u64>,

    /// Random sampling probability (0.0-1.0)
    #[arg(long, conflicts_with_all = ["every", "count"], value_parser = validate_rate)]
    rate: Option<f64>,

    /// Reservoir sampling - select exactly N lines uniformly
    #[arg(long, conflicts_with_all = ["every", "rate"], value_parser = clap::value_parser!(u64).range(1..))]
    count: Option<u64>,

    /// Random seed for deterministic random sampling (works with --rate and --count)
    #[arg(long, conflicts_with = "every")]
    seed: Option<u64>,

    /// Print verbose statistics to stderr
    #[arg(long)]
    stats: bool,

    /// File from which to read, defaulting to stdin
    #[clap(default_value = "-")]
    input: FileOrStdin,
}

#[derive(Debug)]
enum SamplingMode {
    Every(usize),
    Rate { rate: f64, seed: u64 },
    Reservoir { count: usize, seed: u64 },
}

impl SamplingMode {
    fn sample(self, reader: BufReader<impl Read>, writer: &mut impl Write) -> Result<Statistics> {
        let mut stats = Statistics {
            mode: self,
            lines_read: 0,
            lines_output: 0,
        };

        match &stats.mode {
            SamplingMode::Every(n) => {
                for (index, line) in reader.lines().enumerate() {
                    let line = line?;
                    stats.lines_read += 1;

                    // Output every Nth line (0-indexed: 0, N, 2N, ...)
                    if index % n == 0 {
                        writeln!(writer, "{}", line)?;
                        stats.lines_output += 1;
                    }
                }
            }
            SamplingMode::Rate { rate, seed } => {
                let mut rng = StdRng::seed_from_u64(*seed);

                for line in reader.lines() {
                    let line = line?;
                    stats.lines_read += 1;

                    // Random sampling with given probability
                    if rng.gen::<f64>() < *rate {
                        writeln!(writer, "{}", line)?;
                        stats.lines_output += 1;
                    }
                }
            }
            SamplingMode::Reservoir { count, seed } => {
                let mut rng = StdRng::seed_from_u64(*seed);
                let mut reservoir: Vec<String> = Vec::with_capacity(*count);

                for (index, line) in reader.lines().enumerate() {
                    let line = line?;
                    stats.lines_read += 1;

                    if index < *count {
                        // Fill reservoir with first k lines
                        reservoir.push(line);
                    } else {
                        // Randomly replace elements with decreasing probability
                        let j = rng.gen_range(0..=index);
                        if j < *count {
                            reservoir[j] = line;
                        }
                    }
                }

                // Write buffered output
                stats.lines_output = reservoir.len();
                for line in reservoir {
                    writeln!(writer, "{}", line)?;
                }
            }
        }

        Ok(stats)
    }
}

impl From<&Sample> for SamplingMode {
    fn from(sample: &Sample) -> Self {
        let seed = sample.seed.unwrap_or_else(|| {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
        });

        match (sample.every, sample.rate, sample.count) {
            (Some(n), None, None) => SamplingMode::Every(n as usize),
            (None, Some(rate), None) => {
                SamplingMode::Rate { rate, seed }
            }
            (None, None, Some(count)) => {
                SamplingMode::Reservoir { count: count as usize, seed }
            }
            (None, None, None) => {
                // Default: sample 20 rows
                warn!("No sampling mode specified. Defaulting to --count 20");
                SamplingMode::Reservoir { count: 20, seed }
            }
            _ => unreachable!("Clap should prevent multiple modes from being set"),
        }
    }
}

#[derive(Debug)]
struct Statistics {
    mode: SamplingMode,
    lines_read: usize,
    lines_output: usize,
}

impl Statistics {
    fn effective_rate(&self) -> f64 {
        if self.lines_read == 0 {
            0.0
        } else {
            self.lines_output as f64 / self.lines_read as f64
        }
    }

    fn print_to_stderr(&self) -> Result<()> {
        let stderr = std::io::stderr();
        let mut stderr = stderr.lock();

        // Mode-specific details
        match &self.mode {
            SamplingMode::Every(n) => {
                writeln!(stderr, "Mode: Deterministic Every-Nth")?;
                writeln!(stderr, "  N (interval): {}", n)?;
                writeln!(stderr, "  Expected rate: {:.4}", 1.0 / *n as f64)?;
            }
            SamplingMode::Rate { rate, seed } => {
                writeln!(stderr, "Mode: Random Rate-Based")?;
                writeln!(stderr, "  Target rate: {:.4}", rate)?;
                writeln!(stderr, "  Random seed: {}", seed)?;
                writeln!(
                    stderr,
                    "  Expected output: {:.1} lines",
                    self.lines_read as f64 * rate
                )?;
            }
            SamplingMode::Reservoir { count, seed } => {
                writeln!(stderr, "Mode: Reservoir Sampling")?;
                writeln!(stderr, "  Target count: {}", count)?;
                writeln!(stderr, "  Random seed: {}", seed)?;
                writeln!(stderr, "  Maintains input order: yes")?;
            }
        }

        writeln!(stderr)?;
        writeln!(stderr, "Results:")?;
        writeln!(stderr, "  Lines read: {}", self.lines_read)?;
        writeln!(stderr, "  Lines output: {}", self.lines_output)?;
        writeln!(
            stderr,
            "  Effective sampling rate: {:.4} ({:.2}%)",
            self.effective_rate(),
            self.effective_rate() * 100.0
        )?;

        Ok(())
    }
}

impl CommandRunner for Sample {
    fn run(self) -> Result<()> {
        let mode = SamplingMode::from(&self);

        let reader = self.input.into_reader()?;
        let buf_reader = BufReader::new(reader);

        let stdout = std::io::stdout();
        let mut stdout_lock = stdout.lock();

        let stats = mode.sample(buf_reader, &mut stdout_lock)?;

        if self.stats {
            stats.print_to_stderr()?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_sample_every_basic() {
        let input = "line1\nline2\nline3\nline4\nline5\n";
        let reader = BufReader::new(Cursor::new(input));
        let mode = SamplingMode::Every(2);
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        let result = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines, vec!["line1", "line3", "line5"]);
        assert_eq!(stats.lines_read, 5);
        assert_eq!(stats.lines_output, 3);
    }

    #[test]
    fn test_sample_every_first_line_included() {
        let input = "line1\nline2\nline3\nline4\nline5\n";
        let reader = BufReader::new(Cursor::new(input));
        let mode = SamplingMode::Every(5);
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        let result = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        // First line (index 0) should always be included
        assert_eq!(lines, vec!["line1"]);
        assert_eq!(stats.lines_output, 1);
    }

    #[test]
    fn test_sample_every_single_line() {
        let input = "line1\n";
        let reader = BufReader::new(Cursor::new(input));
        let mode = SamplingMode::Every(10);
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        let result = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines, vec!["line1"]);
        assert_eq!(stats.lines_read, 1);
        assert_eq!(stats.lines_output, 1);
    }

    #[test]
    fn test_sample_rate_deterministic_with_seed() {
        let input = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\n";
        let reader1 = BufReader::new(Cursor::new(input));
        let reader2 = BufReader::new(Cursor::new(input));
        let mode1 = SamplingMode::Rate { rate: 0.5, seed: 42 };
        let mode2 = SamplingMode::Rate { rate: 0.5, seed: 42 };
        let mut output1 = Vec::new();
        let mut output2 = Vec::new();

        let stats1 = mode1.sample(reader1, &mut output1).unwrap();
        let stats2 = mode2.sample(reader2, &mut output2).unwrap();

        // Same seed should produce identical results
        assert_eq!(output1, output2);
        assert_eq!(stats1.lines_output, stats2.lines_output);
    }

    #[test]
    fn test_sample_rate_zero() {
        let input = "line1\nline2\nline3\n";
        let reader = BufReader::new(Cursor::new(input));
        let mode = SamplingMode::Rate { rate: 0.0, seed: 42 };
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        assert_eq!(output.len(), 0);
        assert_eq!(stats.lines_read, 3);
        assert_eq!(stats.lines_output, 0);
    }

    #[test]
    fn test_sample_rate_one() {
        let input = "line1\nline2\nline3\n";
        let reader = BufReader::new(Cursor::new(input));
        let mode = SamplingMode::Rate { rate: 1.0, seed: 42 };
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        let result = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(stats.lines_output, 3);
        assert_eq!(lines, vec!["line1", "line2", "line3"]);
    }

    #[test]
    fn test_sample_reservoir_exact_count() {
        let input = (0..100)
            .map(|i| format!("line{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let reader = BufReader::new(Cursor::new(input));
        let mode = SamplingMode::Reservoir { count: 10, seed: 42 };
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        let result = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 10);
        assert_eq!(stats.lines_read, 100);
        assert_eq!(stats.lines_output, 10);
    }

    #[test]
    fn test_sample_reservoir_insufficient_input() {
        let input = "line1\nline2\nline3\n";
        let reader = BufReader::new(Cursor::new(input));
        let mode = SamplingMode::Reservoir { count: 10, seed: 42 };
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        let result = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        // Should return all available lines when input < count
        assert_eq!(lines.len(), 3);
        assert_eq!(stats.lines_output, 3);
        assert_eq!(lines, vec!["line1", "line2", "line3"]);
    }

    #[test]
    fn test_sample_reservoir_deterministic_with_seed() {
        let input = (0..100)
            .map(|i| format!("line{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let reader1 = BufReader::new(Cursor::new(&input));
        let reader2 = BufReader::new(Cursor::new(&input));
        let mode1 = SamplingMode::Reservoir { count: 10, seed: 42 };
        let mode2 = SamplingMode::Reservoir { count: 10, seed: 42 };
        let mut output1 = Vec::new();
        let mut output2 = Vec::new();

        mode1.sample(reader1, &mut output1).unwrap();
        mode2.sample(reader2, &mut output2).unwrap();

        // Same seed should produce identical results
        assert_eq!(output1, output2);
    }

    #[test]
    fn test_sample_reservoir_maintains_order() {
        let input = "line1\nline2\nline3\nline4\nline5\n";
        let reader = BufReader::new(Cursor::new(input));
        let mode = SamplingMode::Reservoir { count: 3, seed: 42 };
        let mut output = Vec::new();

        mode.sample(reader, &mut output).unwrap();

        let result = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        // Check that selected lines maintain relative order
        for i in 0..lines.len() - 1 {
            // Extract line numbers and verify they're in ascending order
            let num1: usize = lines[i].trim_start_matches("line").parse().unwrap();
            let num2: usize = lines[i + 1].trim_start_matches("line").parse().unwrap();
            assert!(num1 < num2, "Lines should maintain original order");
        }
    }

    #[test]
    fn test_empty_input() {
        let input = "";
        let reader = BufReader::new(Cursor::new(input));
        let mode = SamplingMode::Every(5);
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        assert_eq!(output.len(), 0);
        assert_eq!(stats.lines_read, 0);
        assert_eq!(stats.lines_output, 0);
    }

    #[test]
    fn test_statistics_effective_rate() {
        let stats = Statistics {
            mode: SamplingMode::Every(1),
            lines_read: 100,
            lines_output: 25,
        };

        assert_eq!(stats.effective_rate(), 0.25);
    }

    #[test]
    fn test_statistics_effective_rate_zero_input() {
        let stats = Statistics {
            mode: SamplingMode::Every(1),
            lines_read: 0,
            lines_output: 0,
        };
        assert_eq!(stats.effective_rate(), 0.0);
    }
}
