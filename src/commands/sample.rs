use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    hash::{Hash, Hasher},
    io::{BufRead, BufReader, Read, Write},
    time::{Duration, Instant},
};

use anyhow::Result;
use clap::Parser;
use clap_stdin::FileOrStdin;
use humantime::parse_duration as humantime_parse;
use log::warn;
use rand::{rngs::StdRng, Rng, SeedableRng};
use regex::Regex;

use crate::command::CommandRunner;

fn validate_rate(s: &str) -> Result<f64, String> {
    let val: f64 = s
        .parse()
        .map_err(|_| format!("'{}' is not a valid number", s))?;
    if !(0.0..=1.0).contains(&val) {
        return Err(format!("rate must be between 0.0 and 1.0, got {}", val));
    }
    Ok(val)
}

fn parse_duration(s: &str) -> Result<Duration, String> {
    let duration = humantime_parse(s).map_err(|e| format!("Invalid duration '{}': {}", s, e))?;

    if duration.is_zero() {
        return Err("Duration must be > 0".to_string());
    }

    Ok(duration)
}

/// Extract stratum key from line using regex. Returns "unmatched" if no match.
fn extract_stratum_key(line: &str, regex: &Regex) -> String {
    regex
        .find(line)
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "unmatched".to_string())
}

/// Derive a deterministic seed for a stratum from base seed + stratum key
fn derive_seed(base_seed: u64, stratum_key: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    base_seed.hash(&mut hasher);
    stratum_key.hash(&mut hasher);
    hasher.finish()
}

#[derive(Parser)]
#[command(
    name = "sample",
    about = "Sample lines from input using various strategies"
)]
pub struct Sample {
    /// Output every Nth line (deterministic sampling)
    #[arg(long, conflicts_with_all = ["rate", "count", "throttle"], value_parser = clap::value_parser!(u64).range(1..))]
    every: Option<u64>,

    /// Random sampling probability (0.0-1.0)
    #[arg(long, conflicts_with_all = ["every", "count", "throttle"], value_parser = validate_rate)]
    rate: Option<f64>,

    /// Reservoir sampling - select exactly N lines uniformly
    #[arg(long, conflicts_with_all = ["every", "rate", "throttle"], value_parser = clap::value_parser!(u64).range(1..))]
    count: Option<u64>,

    /// Time-based throttling - minimum time between outputs (e.g., "1s", "500ms")
    #[arg(long, conflicts_with_all = ["every", "rate", "count"], value_parser = parse_duration)]
    throttle: Option<Duration>,

    /// Random seed for deterministic random sampling (works with --rate and --count)
    #[arg(long, conflicts_with_all = ["every", "throttle"])]
    seed: Option<u64>,

    /// Print verbose statistics to stderr
    #[arg(long)]
    stats: bool,

    /// Stratify sampling by regex pattern (entire match becomes stratum key)
    /// Works with --every, --rate, and --throttle modes (not --count)
    #[arg(long, value_name = "REGEX", conflicts_with = "count")]
    stratify: Option<String>,

    /// File from which to read, defaulting to stdin
    #[clap(default_value = "-")]
    input: FileOrStdin,
}

#[derive(Debug, Clone)]
enum SamplingMode {
    Every(usize),
    Rate { rate: f64, seed: u64 },
    Reservoir { count: usize, seed: u64 },
    EveryStratified { n: usize, regex: Regex },
    RateStratified { rate: f64, seed: u64, regex: Regex },
    Throttle { duration: Duration },
    ThrottleStratified { duration: Duration, regex: Regex },
}

impl SamplingMode {
    fn sample(self, reader: BufReader<impl Read>, writer: &mut impl Write) -> Result<Statistics> {
        // Clone mode before creating stats to avoid borrow checker issues
        let mode_clone = self.clone();
        let mut stats = Statistics::new(self);

        match mode_clone {
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
                let mut rng = StdRng::seed_from_u64(seed);

                for line in reader.lines() {
                    let line = line?;
                    stats.lines_read += 1;

                    // Random sampling with given probability
                    if rng.gen::<f64>() < rate {
                        writeln!(writer, "{}", line)?;
                        stats.lines_output += 1;
                    }
                }
            }
            SamplingMode::Reservoir { count, seed } => {
                let mut rng = StdRng::seed_from_u64(seed);
                let mut reservoir: Vec<String> = Vec::with_capacity(count);

                for (index, line) in reader.lines().enumerate() {
                    let line = line?;
                    stats.lines_read += 1;

                    if index < count {
                        // Fill reservoir with first k lines
                        reservoir.push(line);
                    } else {
                        // Randomly replace elements with decreasing probability
                        let j = rng.gen_range(0..=index);
                        if j < count {
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
            SamplingMode::EveryStratified { n, regex } => {
                let mut stratum_counters: HashMap<String, usize> = HashMap::new();

                for line in reader.lines() {
                    let line = line?;
                    let key = extract_stratum_key(&line, &regex);

                    let counter = stratum_counters.entry(key.clone()).or_insert(0);
                    stats.increment_read(&key);

                    if *counter % n == 0 {
                        writeln!(writer, "{}", line)?;
                        stats.increment_output(&key);
                    }

                    *counter += 1;
                }
            }
            SamplingMode::RateStratified { rate, seed, regex } => {
                let mut stratum_rngs: HashMap<String, StdRng> = HashMap::new();

                for line in reader.lines() {
                    let line = line?;
                    let key = extract_stratum_key(&line, &regex);

                    let rng = stratum_rngs
                        .entry(key.clone())
                        .or_insert_with(|| StdRng::seed_from_u64(derive_seed(seed, &key)));

                    stats.increment_read(&key);

                    if rng.gen::<f64>() < rate {
                        writeln!(writer, "{}", line)?;
                        stats.increment_output(&key);
                    }
                }
            }
            SamplingMode::Throttle { duration } => {
                let mut last_output: Option<Instant> = None;

                for line in reader.lines() {
                    let line = line?;
                    stats.lines_read += 1;

                    let now = Instant::now();
                    let should_output = match last_output {
                        None => true, // First line always outputs
                        Some(last) => now.duration_since(last) >= duration,
                    };

                    if should_output {
                        writeln!(writer, "{}", line)?;
                        stats.lines_output += 1;
                        last_output = Some(now);
                    }
                }
            }
            SamplingMode::ThrottleStratified { duration, regex } => {
                let mut stratum_last_output: HashMap<String, Instant> = HashMap::new();

                for line in reader.lines() {
                    let line = line?;
                    let key = extract_stratum_key(&line, &regex);
                    stats.increment_read(&key);

                    let now = Instant::now();
                    let should_output = match stratum_last_output.get(&key) {
                        None => true, // First line in this stratum always outputs
                        Some(last) => now.duration_since(*last) >= duration,
                    };

                    if should_output {
                        writeln!(writer, "{}", line)?;
                        stats.increment_output(&key);
                        stratum_last_output.insert(key, now);
                    }
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

        // Compile regex if stratify is present
        let regex_opt = sample
            .stratify
            .as_ref()
            .map(|pattern| Regex::new(pattern).expect("Invalid regex pattern"));

        match (
            sample.every,
            sample.rate,
            sample.count,
            sample.throttle.as_ref(),
            regex_opt,
        ) {
            // Stratified throttle modes
            (None, None, None, Some(duration), Some(regex)) => SamplingMode::ThrottleStratified {
                duration: *duration,
                regex,
            },

            // Non-stratified throttle mode
            (None, None, None, Some(duration), None) => SamplingMode::Throttle {
                duration: *duration,
            },

            // Stratified modes
            (Some(n), None, None, None, Some(regex)) => SamplingMode::EveryStratified {
                n: n as usize,
                regex,
            },
            (None, Some(rate), None, None, Some(regex)) => {
                SamplingMode::RateStratified { rate, seed, regex }
            }

            // Non-stratified modes
            (Some(n), None, None, None, None) => SamplingMode::Every(n as usize),
            (None, Some(rate), None, None, None) => SamplingMode::Rate { rate, seed },
            (None, None, Some(count), None, None) => SamplingMode::Reservoir {
                count: count as usize,
                seed,
            },

            // Default (no mode specified)
            (None, None, None, None, regex_opt) => {
                warn!("No sampling mode specified. Defaulting to --count 20");
                if regex_opt.is_some() {
                    warn!("--stratify ignored without --every, --rate, or --throttle");
                }
                SamplingMode::Reservoir { count: 20, seed }
            }

            // Invalid combinations (should be caught by clap)
            _ => unreachable!("Clap should prevent invalid mode combinations"),
        }
    }
}

#[derive(Debug, Clone)]
struct StratumStats {
    lines_read: usize,
    lines_output: usize,
}

#[derive(Debug)]
struct Statistics {
    mode: SamplingMode,
    lines_read: usize,
    lines_output: usize,
    strata_stats: Option<HashMap<String, StratumStats>>,
}

impl Statistics {
    fn new(mode: SamplingMode) -> Self {
        let stratified = matches!(
            mode,
            SamplingMode::EveryStratified { .. }
                | SamplingMode::RateStratified { .. }
                | SamplingMode::ThrottleStratified { .. }
        );

        Statistics {
            mode,
            lines_read: 0,
            lines_output: 0,
            strata_stats: if stratified {
                Some(HashMap::new())
            } else {
                None
            },
        }
    }

    fn increment_read(&mut self, stratum: &str) {
        self.lines_read += 1;
        if let Some(ref mut strata) = self.strata_stats {
            strata
                .entry(stratum.to_string())
                .or_insert(StratumStats {
                    lines_read: 0,
                    lines_output: 0,
                })
                .lines_read += 1;
        }
    }

    fn increment_output(&mut self, stratum: &str) {
        self.lines_output += 1;
        if let Some(ref mut strata) = self.strata_stats {
            strata
                .entry(stratum.to_string())
                .or_insert(StratumStats {
                    lines_read: 0,
                    lines_output: 0,
                })
                .lines_output += 1;
        }
    }

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
            SamplingMode::EveryStratified { n, regex } => {
                writeln!(stderr, "Mode: Stratified Every-Nth")?;
                writeln!(stderr, "  N (interval): {}", n)?;
                writeln!(stderr, "  Stratification pattern: {}", regex.as_str())?;
                writeln!(stderr, "  Per-stratum sampling: every {}th line", n)?;
            }
            SamplingMode::RateStratified { rate, seed, regex } => {
                writeln!(stderr, "Mode: Stratified Rate-Based")?;
                writeln!(stderr, "  Target rate: {:.4}", rate)?;
                writeln!(stderr, "  Random seed: {}", seed)?;
                writeln!(stderr, "  Stratification pattern: {}", regex.as_str())?;
                writeln!(stderr, "  Per-stratum sampling: independent RNGs")?;
            }
            SamplingMode::Throttle { duration } => {
                writeln!(stderr, "Mode: Time-Based Throttling")?;
                writeln!(stderr, "  Minimum interval: {:?}", duration)?;
                writeln!(
                    stderr,
                    "  Max output rate: {:.2} lines/sec",
                    1.0 / duration.as_secs_f64()
                )?;
            }
            SamplingMode::ThrottleStratified { duration, regex } => {
                writeln!(stderr, "Mode: Stratified Time-Based Throttling")?;
                writeln!(stderr, "  Minimum interval: {:?}", duration)?;
                writeln!(
                    stderr,
                    "  Max output rate: {:.2} lines/sec",
                    1.0 / duration.as_secs_f64()
                )?;
                writeln!(stderr, "  Stratification pattern: {}", regex.as_str())?;
                writeln!(stderr, "  Per-stratum throttling: independent timers")?;
            }
        }

        // Show stratum breakdown if stratified
        if let Some(ref strata) = self.strata_stats {
            writeln!(stderr)?;
            writeln!(stderr, "Stratification:")?;
            writeln!(stderr, "  Strata found: {}", strata.len())?;

            let mut sorted_strata: Vec<_> = strata.iter().collect();
            sorted_strata.sort_by_key(|(k, _)| k.as_str());

            for (key, stats) in sorted_strata {
                let proportion = stats.lines_read as f64 / self.lines_read as f64;
                let rate = if stats.lines_read > 0 {
                    stats.lines_output as f64 / stats.lines_read as f64
                } else {
                    0.0
                };
                writeln!(
                    stderr,
                    "    {}: {} lines ({:.1}%) -> sampled {} ({:.1}%)",
                    key,
                    stats.lines_read,
                    proportion * 100.0,
                    stats.lines_output,
                    rate * 100.0
                )?;
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
    use std::io::Cursor;

    use indoc::indoc;

    use super::*;

    #[test]
    fn test_sample_every_basic() {
        let input = indoc! {"
            line1
            line2
            line3
            line4
            line5
        "};
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
        let input = indoc! {"
            line1
            line2
            line3
            line4
            line5
        "};
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
        let input = indoc! {"
            line1
            line2
            line3
            line4
            line5
            line6
            line7
            line8
            line9
            line10
        "};
        let reader1 = BufReader::new(Cursor::new(input));
        let reader2 = BufReader::new(Cursor::new(input));
        let mode1 = SamplingMode::Rate {
            rate: 0.5,
            seed: 42,
        };
        let mode2 = SamplingMode::Rate {
            rate: 0.5,
            seed: 42,
        };
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
        let input = indoc! {"
            line1
            line2
            line3
        "};
        let reader = BufReader::new(Cursor::new(input));
        let mode = SamplingMode::Rate {
            rate: 0.0,
            seed: 42,
        };
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        assert_eq!(output.len(), 0);
        assert_eq!(stats.lines_read, 3);
        assert_eq!(stats.lines_output, 0);
    }

    #[test]
    fn test_sample_rate_one() {
        let input = indoc! {"
            line1
            line2
            line3
        "};
        let reader = BufReader::new(Cursor::new(input));
        let mode = SamplingMode::Rate {
            rate: 1.0,
            seed: 42,
        };
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
        let mode = SamplingMode::Reservoir {
            count: 10,
            seed: 42,
        };
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
        let input = indoc! {"
            line1
            line2
            line3
        "};
        let reader = BufReader::new(Cursor::new(input));
        let mode = SamplingMode::Reservoir {
            count: 10,
            seed: 42,
        };
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
        let mode1 = SamplingMode::Reservoir {
            count: 10,
            seed: 42,
        };
        let mode2 = SamplingMode::Reservoir {
            count: 10,
            seed: 42,
        };
        let mut output1 = Vec::new();
        let mut output2 = Vec::new();

        mode1.sample(reader1, &mut output1).unwrap();
        mode2.sample(reader2, &mut output2).unwrap();

        // Same seed should produce identical results
        assert_eq!(output1, output2);
    }

    #[test]
    fn test_sample_reservoir_maintains_order() {
        let input = indoc! {"
            line1
            line2
            line3
            line4
            line5
        "};
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
            strata_stats: None,
        };

        assert_eq!(stats.effective_rate(), 0.25);
    }

    #[test]
    fn test_statistics_effective_rate_zero_input() {
        let stats = Statistics {
            mode: SamplingMode::Every(1),
            lines_read: 0,
            lines_output: 0,
            strata_stats: None,
        };
        assert_eq!(stats.effective_rate(), 0.0);
    }

    #[test]
    fn test_extract_stratum_key_match() {
        let regex = Regex::new(r"\[(INFO|WARN|ERROR)\]").unwrap();
        assert_eq!(extract_stratum_key("[INFO] message", &regex), "[INFO]");
        assert_eq!(extract_stratum_key("[ERROR] bad", &regex), "[ERROR]");
    }

    #[test]
    fn test_extract_stratum_key_unmatched() {
        let regex = Regex::new(r"\[(INFO|WARN|ERROR)\]").unwrap();
        assert_eq!(extract_stratum_key("no match here", &regex), "unmatched");
    }

    #[test]
    fn test_derive_seed_deterministic() {
        let seed1 = derive_seed(42, "INFO");
        let seed2 = derive_seed(42, "INFO");
        assert_eq!(seed1, seed2);
    }

    #[test]
    fn test_derive_seed_different_strata() {
        let seed1 = derive_seed(42, "INFO");
        let seed2 = derive_seed(42, "ERROR");
        assert_ne!(seed1, seed2);
    }

    #[test]
    fn test_every_stratified_basic() {
        let input = indoc! {"
            [INFO] 1
            [ERROR] 2
            [INFO] 3
            [ERROR] 4
            [INFO] 5
        "};
        let reader = BufReader::new(Cursor::new(input));
        let regex = Regex::new(r"\[(INFO|ERROR)\]").unwrap();
        let mode = SamplingMode::EveryStratified { n: 2, regex };
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        let result = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = result.lines().collect();

        // Every 2nd line within each stratum:
        // INFO stratum: [INFO] 1 (index 0), [INFO] 3 (skipped), [INFO] 5 (index 2)
        // ERROR stratum: [ERROR] 2 (index 0), [ERROR] 4 (index 1, skipped)
        assert_eq!(lines.len(), 3); // [INFO] 1, [ERROR] 2, [INFO] 5
        assert_eq!(stats.lines_read, 5);
        assert_eq!(stats.lines_output, 3);
    }

    #[test]
    fn test_every_stratified_per_stratum_counting() {
        let input = indoc! {"
            A
            A
            A
            B
            B
            A
        "};
        let reader = BufReader::new(Cursor::new(input));
        let regex = Regex::new(r"[AB]").unwrap();
        let mode = SamplingMode::EveryStratified { n: 2, regex };
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        let result = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = result.lines().collect();

        // A stratum: A (idx 0), A (idx 2)  -> indices 0,2 sampled
        // B stratum: B (idx 0) -> index 0 sampled
        assert_eq!(lines.len(), 3); // First A, third A, first B
        assert_eq!(stats.lines_read, 6);
        assert_eq!(stats.lines_output, 3);
    }

    #[test]
    fn test_rate_stratified_deterministic() {
        let input = (0..100)
            .map(|i| format!("[INFO] line{}", i))
            .collect::<Vec<_>>()
            .join("\n");

        let reader1 = BufReader::new(Cursor::new(&input));
        let reader2 = BufReader::new(Cursor::new(&input));
        let regex1 = Regex::new(r"\[INFO\]").unwrap();
        let regex2 = Regex::new(r"\[INFO\]").unwrap();
        let mode1 = SamplingMode::RateStratified {
            rate: 0.5,
            seed: 42,
            regex: regex1,
        };
        let mode2 = SamplingMode::RateStratified {
            rate: 0.5,
            seed: 42,
            regex: regex2,
        };
        let mut output1 = Vec::new();
        let mut output2 = Vec::new();

        let stats1 = mode1.sample(reader1, &mut output1).unwrap();
        let stats2 = mode2.sample(reader2, &mut output2).unwrap();

        // Same seed should produce identical output
        assert_eq!(output1, output2);
        assert_eq!(stats1.lines_output, stats2.lines_output);
    }

    #[test]
    fn test_rate_stratified_per_stratum_rng() {
        // Test that different strata get different RNGs
        let input = indoc! {"
            [INFO] 1
            [ERROR] 2
            [INFO] 3
            [ERROR] 4
        "};
        let reader = BufReader::new(Cursor::new(input));
        let regex = Regex::new(r"\[(INFO|ERROR)\]").unwrap();
        let mode = SamplingMode::RateStratified {
            rate: 0.5,
            seed: 42,
            regex,
        };
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        // With rate 0.5, we expect roughly half the lines from each stratum
        // The exact output depends on RNG but should be deterministic
        assert_eq!(stats.lines_read, 4);
        assert!(stats.lines_output >= 1 && stats.lines_output <= 3);

        // Verify strata stats exist
        assert!(stats.strata_stats.is_some());
        let strata = stats.strata_stats.as_ref().unwrap();
        assert!(strata.contains_key("[INFO]"));
        assert!(strata.contains_key("[ERROR]"));
    }

    #[test]
    fn test_stratified_unmatched_stratum() {
        let input = indoc! {"
            [INFO] 1
            no match
            [ERROR] 2
            another no match
        "};
        let reader = BufReader::new(Cursor::new(input));
        let regex = Regex::new(r"\[(INFO|ERROR)\]").unwrap();
        let mode = SamplingMode::EveryStratified { n: 1, regex }; // Sample every line
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        assert_eq!(stats.lines_read, 4);
        assert_eq!(stats.lines_output, 4); // All lines should be sampled

        // Check strata breakdown
        let strata = stats.strata_stats.as_ref().unwrap();
        assert!(strata.contains_key("[INFO]"));
        assert!(strata.contains_key("[ERROR]"));
        assert!(strata.contains_key("unmatched"));
        assert_eq!(strata["unmatched"].lines_read, 2);
    }

    #[test]
    fn test_parse_duration_valid() {
        // Seconds
        assert_eq!(parse_duration("1s").unwrap(), Duration::from_secs(1));
        assert_eq!(parse_duration("1sec").unwrap(), Duration::from_secs(1));
        assert_eq!(parse_duration("2seconds").unwrap(), Duration::from_secs(2));

        // Milliseconds
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("100ms").unwrap(), Duration::from_millis(100));

        // Minutes (bonus from humantime)
        assert_eq!(parse_duration("1m").unwrap(), Duration::from_secs(60));
        assert_eq!(parse_duration("1min").unwrap(), Duration::from_secs(60));
    }

    #[test]
    fn test_parse_duration_invalid() {
        // Invalid format
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("").is_err());
        assert!(parse_duration("notaduration").is_err());

        // Zero duration should be rejected
        assert!(parse_duration("0s").is_err());
        assert!(parse_duration("0ms").is_err());
    }

    #[test]
    fn test_throttle_basic() {
        let input = indoc! {"
            line1
            line2
            line3
            line4
        "};
        let reader = BufReader::new(Cursor::new(input));
        let mode = SamplingMode::Throttle {
            duration: Duration::from_millis(10),
        };
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        // In tests, lines arrive instantly (no real time delay)
        // So only the first line should output
        let result = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 1);
        assert!(result.contains("line1"));
        assert_eq!(stats.lines_read, 4);
        assert_eq!(stats.lines_output, 1);
    }

    #[test]
    fn test_throttle_stratified() {
        let input = indoc! {"
            [INFO] 1
            [INFO] 2
            [ERROR] 3
            [ERROR] 4
            [INFO] 5
        "};
        let reader = BufReader::new(Cursor::new(input));
        let regex = Regex::new(r"\[(INFO|ERROR)\]").unwrap();
        let mode = SamplingMode::ThrottleStratified {
            duration: Duration::from_millis(10),
            regex,
        };
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        // In tests, lines arrive instantly
        // First line of each stratum should output
        let result = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2); // [INFO] 1, [ERROR] 3
        assert!(result.contains("[INFO] 1"));
        assert!(result.contains("[ERROR] 3"));
        assert_eq!(stats.lines_read, 5);
        assert_eq!(stats.lines_output, 2);

        // Check strata breakdown
        let strata = stats.strata_stats.as_ref().unwrap();
        assert_eq!(strata["[INFO]"].lines_read, 3);
        assert_eq!(strata["[INFO]"].lines_output, 1);
        assert_eq!(strata["[ERROR]"].lines_read, 2);
        assert_eq!(strata["[ERROR]"].lines_output, 1);
    }

    #[test]
    fn test_throttle_single_line() {
        let input = "single line\n";
        let reader = BufReader::new(Cursor::new(input));
        let mode = SamplingMode::Throttle {
            duration: Duration::from_secs(1),
        };
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        // Single line should always output
        let result = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines, vec!["single line"]);
        assert_eq!(stats.lines_read, 1);
        assert_eq!(stats.lines_output, 1);
    }
}
