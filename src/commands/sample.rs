use std::{
    collections::HashMap,
    fmt,
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

/// Wraps an RNG with its seed for reproducibility
struct SeededRng {
    seed: u64,
    rng: StdRng,
}

impl SeededRng {
    fn new(seed: u64) -> Self {
        SeededRng {
            seed,
            rng: StdRng::seed_from_u64(seed),
        }
    }

    fn seed(&self) -> u64 {
        self.seed
    }

    // Allow mutable access to the inner RNG for gen() operations
    fn gen<T>(&mut self) -> T
    where
        rand::distributions::Standard: rand::distributions::Distribution<T>,
    {
        self.rng.gen()
    }
}

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
struct StrategyDetails {
    name: String,
    parameters: HashMap<String, String>,
    seed: Option<u64>,
    expectations: HashMap<String, String>,
    stratum_stats: Option<HashMap<String, StratumStats>>,
}

impl fmt::Display for StrategyDetails {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.name)?;

        if !self.parameters.is_empty() {
            let params: Vec<String> = self
                .parameters
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect();
            writeln!(f, "  parameters: {}", params.join(", "))?;
        }

        if let Some(seed) = self.seed {
            writeln!(f, "  seed: {}", seed)?;
        }

        if !self.expectations.is_empty() {
            writeln!(f, "expectations:")?;
            for (key, value) in &self.expectations {
                writeln!(f, "  {}: {}", key, value)?;
            }
        }

        Ok(())
    }
}

/// Trait for streaming sampling decisions
trait StreamingStrategy {
    type State;

    fn init_state(&self, rng: StdRng) -> Self::State;
    fn should_sample(&self, state: &mut Self::State, line: &str) -> bool;
    fn format_details(&self) -> StrategyDetails;
}

struct ProcessResult {
    sampled: bool,
}

trait StreamingSampler {
    fn process_line(&mut self, line: &str) -> ProcessResult;
    fn format_details(&self) -> StrategyDetails;
}

/// Trait for batch sampling strategies (two-phase: collect then finalize)
trait BatchStrategy {
    type State;

    fn init_state(&self, rng: StdRng) -> Self::State;
    fn add_line(&self, state: &mut Self::State, line: String);
    fn finalize(&self, state: Self::State) -> Vec<(usize, String)>;
    fn format_details(&self) -> StrategyDetails;
}

trait BatchSampler {
    /// Collect a single line, returning stratum key if stratified
    fn collect_line(&mut self, line: String) -> Option<String>;

    /// Finalize processing and return selected lines with StrategyDetails
    /// Returns (selected lines, strategy details with stats)
    fn finalize(self: Box<Self>) -> (Vec<(Option<String>, String)>, StrategyDetails);
}

struct UniformStreamSampler<S: StreamingStrategy> {
    strategy: S,
    state: S::State,
    seed: u64,
}

impl<S: StreamingStrategy> UniformStreamSampler<S> {
    fn new(strategy: S, seeded_rng: SeededRng) -> Self {
        let seed = seeded_rng.seed();
        let state = strategy.init_state(seeded_rng.rng);
        UniformStreamSampler {
            strategy,
            state,
            seed,
        }
    }
}

impl<S: StreamingStrategy> StreamingSampler for UniformStreamSampler<S> {
    fn process_line(&mut self, line: &str) -> ProcessResult {
        let sampled = self.strategy.should_sample(&mut self.state, line);
        ProcessResult { sampled }
    }

    fn format_details(&self) -> StrategyDetails {
        let mut details = self.strategy.format_details();
        details.seed = Some(self.seed);
        details
    }
}

struct StratifiedStreamSampler<S: StreamingStrategy> {
    strategy: S,
    regex: Regex,
    stratum_states: HashMap<String, S::State>,
    stratum_stats: HashMap<String, StratumStats>,
    base_rng: SeededRng,
}

impl<S: StreamingStrategy> StratifiedStreamSampler<S> {
    fn new(strategy: S, regex: Regex, seeded_rng: SeededRng) -> Self {
        StratifiedStreamSampler {
            strategy,
            regex,
            stratum_states: HashMap::new(),
            stratum_stats: HashMap::new(),
            base_rng: seeded_rng,
        }
    }
}

impl<S: StreamingStrategy> StreamingSampler for StratifiedStreamSampler<S> {
    fn process_line(&mut self, line: &str) -> ProcessResult {
        let key = extract_stratum_key(line, &self.regex);

        self.stratum_stats
            .entry(key.clone())
            .or_insert(StratumStats {
                lines_read: 0,
                lines_output: 0,
            })
            .lines_read += 1;

        let state = self.stratum_states.entry(key.clone()).or_insert_with(|| {
            let stratum_seed = self.base_rng.gen::<u64>();
            let stratum_rng = StdRng::seed_from_u64(stratum_seed);
            self.strategy.init_state(stratum_rng)
        });
        let sampled = self.strategy.should_sample(state, line);

        if sampled {
            self.stratum_stats.get_mut(&key).unwrap().lines_output += 1;
        }

        ProcessResult { sampled }
    }

    fn format_details(&self) -> StrategyDetails {
        let mut details = self.strategy.format_details();
        details.name = format!("{} (stratified by {})", details.name, self.regex.as_str());
        details.stratum_stats = Some(self.stratum_stats.clone());
        details.seed = Some(self.base_rng.seed());
        details
    }
}

struct UniformBatchSampler<S: BatchStrategy> {
    strategy: S,
    state: S::State,
    seed: u64,
}

impl<S: BatchStrategy> UniformBatchSampler<S> {
    fn new(strategy: S, seeded_rng: SeededRng) -> Self {
        let seed = seeded_rng.seed();
        let state = strategy.init_state(seeded_rng.rng);
        UniformBatchSampler {
            strategy,
            state,
            seed,
        }
    }
}

impl<S: BatchStrategy> BatchSampler for UniformBatchSampler<S> {
    fn collect_line(&mut self, line: String) -> Option<String> {
        self.strategy.add_line(&mut self.state, line);
        None // Non-stratified
    }

    fn finalize(self: Box<Self>) -> (Vec<(Option<String>, String)>, StrategyDetails) {
        let selected = self.strategy.finalize(self.state);
        let result: Vec<(Option<String>, String)> = selected
            .into_iter()
            .map(|(_idx, line)| (None, line))
            .collect();
        let mut details = self.strategy.format_details();
        details.seed = Some(self.seed);
        (result, details)
    }
}

struct StratifiedBatchSampler<S: BatchStrategy> {
    strategy: S,
    regex: Regex,
    stratum_states: HashMap<String, S::State>,
    line_to_stratum: Vec<String>,
    stratum_stats: HashMap<String, StratumStats>,
    base_rng: SeededRng,
}

impl<S: BatchStrategy> StratifiedBatchSampler<S> {
    fn new(strategy: S, regex: Regex, seeded_rng: SeededRng) -> Self {
        StratifiedBatchSampler {
            strategy,
            regex,
            stratum_states: HashMap::new(),
            line_to_stratum: Vec::new(),
            stratum_stats: HashMap::new(),
            base_rng: seeded_rng,
        }
    }
}

impl<S: BatchStrategy> BatchSampler for StratifiedBatchSampler<S> {
    fn collect_line(&mut self, line: String) -> Option<String> {
        let key = extract_stratum_key(&line, &self.regex);
        self.line_to_stratum.push(key.clone());

        self.stratum_stats
            .entry(key.clone())
            .or_insert(StratumStats {
                lines_read: 0,
                lines_output: 0,
            })
            .lines_read += 1;

        let state = self.stratum_states.entry(key.clone()).or_insert_with(|| {
            let stratum_seed = self.base_rng.gen::<u64>();
            let stratum_rng = StdRng::seed_from_u64(stratum_seed);
            self.strategy.init_state(stratum_rng)
        });

        self.strategy.add_line(state, line);
        Some(key)
    }

    fn finalize(self: Box<Self>) -> (Vec<(Option<String>, String)>, StrategyDetails) {
        let mut stratum_to_global_indices: HashMap<String, Vec<usize>> = HashMap::new();
        for (global_idx, stratum_key) in self.line_to_stratum.iter().enumerate() {
            stratum_to_global_indices
                .entry(stratum_key.clone())
                .or_default()
                .push(global_idx);
        }

        let mut selected: Vec<(usize, String, String)> = Vec::new();
        let mut stratum_stats = self.stratum_stats;

        for (stratum_key, state) in self.stratum_states {
            let stratum_selected = self.strategy.finalize(state);
            let global_indices = &stratum_to_global_indices[&stratum_key];

            stratum_stats.get_mut(&stratum_key).unwrap().lines_output += stratum_selected.len();

            for (within_idx, line) in stratum_selected {
                let global_idx = global_indices[within_idx];
                selected.push((global_idx, line, stratum_key.clone()));
            }
        }

        selected.sort_by_key(|(idx, ..)| *idx);

        let mut details = self.strategy.format_details();
        details.name = format!("{} (stratified by {})", details.name, self.regex.as_str());
        details.stratum_stats = Some(stratum_stats);
        details.seed = Some(self.base_rng.seed());

        let result: Vec<(Option<String>, String)> = selected
            .into_iter()
            .map(|(_, line, stratum)| (Some(stratum), line))
            .collect();

        (result, details)
    }
}

struct EveryStrategy {
    n: usize,
}

impl StreamingStrategy for EveryStrategy {
    type State = usize;

    fn init_state(&self, _rng: StdRng) -> usize {
        0
    }

    fn should_sample(&self, counter: &mut usize, _line: &str) -> bool {
        let should_output = (*counter).is_multiple_of(self.n);
        *counter += 1;
        should_output
    }

    fn format_details(&self) -> StrategyDetails {
        let mut parameters = HashMap::new();
        parameters.insert("interval".to_string(), self.n.to_string());

        let mut expectations = HashMap::new();
        expectations.insert(
            "sampling_rate".to_string(),
            format!("{:.4}", 1.0 / self.n as f64),
        );

        StrategyDetails {
            name: "Every-Nth Sampling".to_string(),
            parameters,
            seed: None,
            expectations,
            stratum_stats: None,
        }
    }
}

struct RateStrategy {
    rate: f64,
}

impl StreamingStrategy for RateStrategy {
    type State = StdRng;

    fn init_state(&self, rng: StdRng) -> StdRng {
        rng
    }

    fn should_sample(&self, rng: &mut StdRng, _line: &str) -> bool {
        rng.gen::<f64>() < self.rate
    }

    fn format_details(&self) -> StrategyDetails {
        let mut parameters = HashMap::new();
        parameters.insert("rate".to_string(), format!("{:.4}", self.rate));

        let mut expectations = HashMap::new();
        expectations.insert("sampling_rate".to_string(), format!("{:.4}", self.rate));

        StrategyDetails {
            name: "Rate Sampling".to_string(),
            parameters,
            seed: None,
            expectations,
            stratum_stats: None,
        }
    }
}

struct ThrottleStrategy {
    duration: Duration,
}

impl StreamingStrategy for ThrottleStrategy {
    type State = Option<Instant>;

    fn init_state(&self, _rng: StdRng) -> Option<Instant> {
        None
    }

    fn should_sample(&self, last_output: &mut Option<Instant>, _line: &str) -> bool {
        let now = Instant::now();
        let should_output = match *last_output {
            None => true,
            Some(last) => now.duration_since(last) >= self.duration,
        };
        if should_output {
            *last_output = Some(now);
        }
        should_output
    }

    fn format_details(&self) -> StrategyDetails {
        let mut parameters = HashMap::new();
        parameters.insert("interval".to_string(), format!("{:?}", self.duration));

        let mut expectations = HashMap::new();
        expectations.insert(
            "max_rate".to_string(),
            format!("{:.2} lines/sec", 1.0 / self.duration.as_secs_f64()),
        );

        StrategyDetails {
            name: "Throttle Sampling".to_string(),
            parameters,
            seed: None,
            expectations,
            stratum_stats: None,
        }
    }
}

struct ReservoirState {
    reservoir: Vec<(usize, String)>,
    count: usize,
    lines_seen: usize,
    rng: StdRng,
}

struct ReservoirStrategy {
    count: usize,
}

impl BatchStrategy for ReservoirStrategy {
    type State = ReservoirState;

    fn init_state(&self, rng: StdRng) -> ReservoirState {
        ReservoirState {
            reservoir: Vec::with_capacity(self.count),
            count: self.count,
            lines_seen: 0,
            rng,
        }
    }

    fn add_line(&self, state: &mut ReservoirState, line: String) {
        let index = state.lines_seen;
        state.lines_seen += 1;

        if index < state.count {
            state.reservoir.push((index, line));
        } else {
            let j = state.rng.gen_range(0..=index);
            if j < state.count {
                state.reservoir[j] = (index, line);
            }
        }
    }

    fn finalize(&self, mut state: ReservoirState) -> Vec<(usize, String)> {
        state.reservoir.sort_by_key(|(idx, _)| *idx);
        state.reservoir
    }

    fn format_details(&self) -> StrategyDetails {
        let mut parameters = HashMap::new();
        parameters.insert("count".to_string(), self.count.to_string());

        let mut expectations = HashMap::new();
        expectations.insert("maintains_order".to_string(), "yes".to_string());

        StrategyDetails {
            name: "Reservoir Sampling".to_string(),
            parameters,
            seed: None,
            expectations,
            stratum_stats: None,
        }
    }
}

enum SamplingMode {
    Streaming(Box<dyn StreamingSampler>),
    Batch(Box<dyn BatchSampler>),
}

impl SamplingMode {
    fn sample(self, reader: BufReader<impl Read>, writer: &mut impl Write) -> Result<Statistics> {
        match self {
            SamplingMode::Streaming(mut machine) => {
                let mut lines_read = 0;
                let mut lines_output = 0;

                for line in reader.lines() {
                    let line = line?;
                    let result = machine.process_line(&line);

                    lines_read += 1;
                    if result.sampled {
                        writeln!(writer, "{}", line)?;
                        lines_output += 1;
                    }
                }

                let details = machine.format_details();
                Ok(Statistics::new(details, lines_read, lines_output))
            }
            SamplingMode::Batch(mut executor) => {
                let mut lines_read = 0;

                for line in reader.lines() {
                    let line = line?;
                    executor.collect_line(line);
                    lines_read += 1;
                }

                let (selected, details) = executor.finalize();
                let lines_output = selected.len();

                for (_stratum_key, line) in selected {
                    writeln!(writer, "{}", line)?;
                }

                Ok(Statistics::new(details, lines_read, lines_output))
            }
        }
    }
}

#[cfg(test)]
macro_rules! every {
    ($n:expr) => {
        EveryStrategy { n: $n }
    };
}

#[cfg(test)]
macro_rules! rate {
    ($rate:expr) => {
        RateStrategy { rate: $rate }
    };
}

#[cfg(test)]
macro_rules! throttle {
    ($duration:expr) => {
        ThrottleStrategy {
            duration: $duration,
        }
    };
}

#[cfg(test)]
macro_rules! reservoir {
    ($count:expr) => {
        ReservoirStrategy { count: $count }
    };
}

#[cfg(test)]
macro_rules! uniform_stream {
    ($strategy:expr, seed: $seed:expr) => {
        SamplingMode::Streaming(Box::new(UniformStreamSampler::new(
            $strategy,
            SeededRng::new($seed),
        )))
    };
}

#[cfg(test)]
macro_rules! stratified_stream {
    ($strategy:expr, pattern: $pattern:expr, seed: $seed:expr) => {
        SamplingMode::Streaming(Box::new(StratifiedStreamSampler::new(
            $strategy,
            Regex::new($pattern).unwrap(),
            SeededRng::new($seed),
        )))
    };
}

#[cfg(test)]
macro_rules! uniform_batch {
    ($strategy:expr, seed: $seed:expr) => {
        SamplingMode::Batch(Box::new(UniformBatchSampler::new(
            $strategy,
            SeededRng::new($seed),
        )))
    };
}

#[cfg(test)]
macro_rules! stratified_batch {
    ($strategy:expr, pattern: $pattern:expr, seed: $seed:expr) => {
        SamplingMode::Batch(Box::new(StratifiedBatchSampler::new(
            $strategy,
            Regex::new($pattern).unwrap(),
            SeededRng::new($seed),
        )))
    };
}

macro_rules! streaming {
    ($strategy:expr, $regex_opt:expr, $seed:expr) => {
        match $regex_opt {
            Some(regex) => SamplingMode::Streaming(Box::new(StratifiedStreamSampler::new(
                $strategy,
                regex,
                SeededRng::new($seed),
            ))),
            None => SamplingMode::Streaming(Box::new(UniformStreamSampler::new(
                $strategy,
                SeededRng::new($seed),
            ))),
        }
    };
}

macro_rules! batch {
    ($strategy:expr, $regex_opt:expr, $seed:expr) => {
        match $regex_opt {
            Some(regex) => SamplingMode::Batch(Box::new(StratifiedBatchSampler::new(
                $strategy,
                regex,
                SeededRng::new($seed),
            ))),
            None => SamplingMode::Batch(Box::new(UniformBatchSampler::new(
                $strategy,
                SeededRng::new($seed),
            ))),
        }
    };
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

        let regex_opt = sample
            .stratify
            .as_ref()
            .map(|pattern| Regex::new(pattern).expect("Invalid regex pattern"));

        match (
            sample.every,
            sample.rate,
            sample.count,
            sample.throttle.as_ref(),
        ) {
            (Some(n), None, None, None) => {
                streaming!(EveryStrategy { n: n as usize }, regex_opt, seed)
            }

            (None, Some(rate), None, None) => {
                streaming!(RateStrategy { rate }, regex_opt, seed)
            }

            (None, None, None, Some(duration)) => {
                streaming!(
                    ThrottleStrategy {
                        duration: *duration
                    },
                    regex_opt,
                    seed
                )
            }

            (None, None, Some(count), None) => {
                batch!(
                    ReservoirStrategy {
                        count: count as usize,
                    },
                    regex_opt,
                    seed
                )
            }

            (None, None, None, None) => {
                warn!("No sampling mode specified. Defaulting to --count 20");
                batch!(ReservoirStrategy { count: 20 }, regex_opt, seed)
            }

            _ => unreachable!("Clap should prevent invalid mode combinations"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StratumStats {
    pub lines_read: usize,
    pub lines_output: usize,
}

#[derive(Debug)]
struct Statistics {
    details: StrategyDetails,
    lines_read: usize,
    lines_output: usize,
}

impl Statistics {
    fn new(details: StrategyDetails, lines_read: usize, lines_output: usize) -> Self {
        Statistics {
            details,
            lines_read,
            lines_output,
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
        self.write_stats(&mut stderr)
    }

    fn write_stats(&self, writer: &mut impl std::io::Write) -> Result<()> {
        let percent = self.effective_rate() * 100.0;
        writeln!(
            writer,
            "{} / {} ({:.2}%)",
            self.lines_output, self.lines_read, percent
        )?;

        write!(writer, "{}", self.details)?;

        if let Some(ref strata) = self.details.stratum_stats {
            writeln!(writer, "stratification:")?;
            writeln!(writer, "  strata found: {}", strata.len())?;

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
                    writer,
                    "    {}: {} lines ({:.1}%) -> sampled {} ({:.1}%)",
                    key,
                    stats.lines_read,
                    proportion * 100.0,
                    stats.lines_output,
                    rate * 100.0
                )?;
            }
        }

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
        let mode = uniform_stream!(every!(2), seed: 0);
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
        let mode = uniform_stream!(every!(5), seed: 0);
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
        let mode = SamplingMode::Streaming(Box::new(UniformStreamSampler::new(
            EveryStrategy { n: 10 },
            SeededRng::new(0),
        )));
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
        let mode1 = uniform_stream!(rate!(0.5), seed: 42);
        let mode2 = uniform_stream!(rate!(0.5), seed: 42);
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
        let mode = SamplingMode::Streaming(Box::new(UniformStreamSampler::new(
            RateStrategy { rate: 0.0 },
            SeededRng::new(42),
        )));
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
        let mode = SamplingMode::Streaming(Box::new(UniformStreamSampler::new(
            RateStrategy { rate: 1.0 },
            SeededRng::new(42),
        )));
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
        let mode = uniform_batch!(reservoir!(10), seed: 42);
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
        let mode = SamplingMode::Batch(Box::new(UniformBatchSampler::new(
            ReservoirStrategy { count: 10 },
            SeededRng::new(42),
        )));
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
        let mode1 = uniform_batch!(reservoir!(10), seed: 42);
        let mode2 = uniform_batch!(reservoir!(10), seed: 42);
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
        let mode = SamplingMode::Batch(Box::new(UniformBatchSampler::new(
            ReservoirStrategy { count: 3 },
            SeededRng::new(42),
        )));
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
        let mode = SamplingMode::Streaming(Box::new(UniformStreamSampler::new(
            EveryStrategy { n: 5 },
            SeededRng::new(0),
        )));
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        assert_eq!(output.len(), 0);
        assert_eq!(stats.lines_read, 0);
        assert_eq!(stats.lines_output, 0);
    }

    #[test]
    fn test_statistics_effective_rate() {
        let details = StrategyDetails {
            name: "Test mode".to_string(),
            parameters: HashMap::new(),
            seed: None,
            expectations: HashMap::new(),
            stratum_stats: None,
        };
        let stats = Statistics::new(details, 100, 25);

        assert_eq!(stats.effective_rate(), 0.25);
    }

    #[test]
    fn test_statistics_effective_rate_zero_input() {
        let details = StrategyDetails {
            name: "Test mode".to_string(),
            parameters: HashMap::new(),
            seed: None,
            expectations: HashMap::new(),
            stratum_stats: None,
        };
        let stats = Statistics::new(details, 0, 0);
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
    fn test_every_stratified_basic() {
        let input = indoc! {"
            [INFO] 1
            [ERROR] 2
            [INFO] 3
            [ERROR] 4
            [INFO] 5
        "};
        let reader = BufReader::new(Cursor::new(input));
        let mode = stratified_stream!(every!(2), pattern: r"\[(INFO|ERROR)\]", seed: 0);
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
        let mode = SamplingMode::Streaming(Box::new(StratifiedStreamSampler::new(
            EveryStrategy { n: 2 },
            regex,
            SeededRng::new(0),
        )));
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
        let mode1 = stratified_stream!(rate!(0.5), pattern: r"\[INFO\]", seed: 42);
        let mode2 = stratified_stream!(rate!(0.5), pattern: r"\[INFO\]", seed: 42);
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
        let mode = SamplingMode::Streaming(Box::new(StratifiedStreamSampler::new(
            RateStrategy { rate: 0.5 },
            regex,
            SeededRng::new(42),
        )));
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        // With rate 0.5, we expect roughly half the lines from each stratum
        // The exact output depends on RNG but should be deterministic
        assert_eq!(stats.lines_read, 4);
        assert!(stats.lines_output >= 1 && stats.lines_output <= 3);

        // Verify strata stats exist
        assert!(stats.details.stratum_stats.is_some());
        let strata = stats.details.stratum_stats.as_ref().unwrap();
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
        let mode = SamplingMode::Streaming(Box::new(StratifiedStreamSampler::new(
            EveryStrategy { n: 1 },
            regex,
            SeededRng::new(0),
        ))); // Sample every line
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        assert_eq!(stats.lines_read, 4);
        assert_eq!(stats.lines_output, 4); // All lines should be sampled

        // Check strata breakdown
        let strata = stats.details.stratum_stats.as_ref().unwrap();
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
        let mode = uniform_stream!(throttle!(Duration::from_millis(10)), seed: 0);
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
        let mode = stratified_stream!(throttle!(Duration::from_millis(10)), pattern: r"\[(INFO|ERROR)\]", seed: 0);
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
        let strata = stats.details.stratum_stats.as_ref().unwrap();
        assert_eq!(strata["[INFO]"].lines_read, 3);
        assert_eq!(strata["[INFO]"].lines_output, 1);
        assert_eq!(strata["[ERROR]"].lines_read, 2);
        assert_eq!(strata["[ERROR]"].lines_output, 1);
    }

    #[test]
    fn test_throttle_single_line() {
        let input = "single line\n";
        let reader = BufReader::new(Cursor::new(input));
        let mode = SamplingMode::Streaming(Box::new(UniformStreamSampler::new(
            ThrottleStrategy {
                duration: Duration::from_secs(1),
            },
            SeededRng::new(0),
        )));
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        // Single line should always output
        let result = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines, vec!["single line"]);
        assert_eq!(stats.lines_read, 1);
        assert_eq!(stats.lines_output, 1);
    }

    #[test]
    fn test_stratified_reservoir_sampling() {
        let input = indoc! {"
            [INFO] 1
            [INFO] 2
            [ERROR] 3
            [INFO] 4
            [ERROR] 5
            [INFO] 6
            [ERROR] 7
            [INFO] 8
        "};
        let reader = BufReader::new(Cursor::new(input));
        let mode = stratified_batch!(reservoir!(2), pattern: r"\[(INFO|ERROR)\]", seed: 42);
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        // Should have selected 2 from each stratum = 4 total
        assert_eq!(stats.lines_output, 4);
        assert_eq!(stats.lines_read, 8);

        // Check strata breakdown
        let strata = stats.details.stratum_stats.as_ref().unwrap();
        assert_eq!(strata["[INFO]"].lines_read, 5);
        assert_eq!(strata["[INFO]"].lines_output, 2);
        assert_eq!(strata["[ERROR]"].lines_read, 3);
        assert_eq!(strata["[ERROR]"].lines_output, 2);

        // Verify output maintains input order
        let result = String::from_utf8(output).unwrap();
        let lines: Vec<&str> = result.lines().collect();

        // Lines should be interleaved (INFO and ERROR mixed in input order)
        // Extract line numbers and verify they're ascending
        let line_numbers: Vec<i32> = lines
            .iter()
            .map(|l| l.split_whitespace().last().unwrap().parse().unwrap())
            .collect();

        for i in 0..line_numbers.len() - 1 {
            assert!(
                line_numbers[i] < line_numbers[i + 1],
                "Lines should maintain input order across strata"
            );
        }
    }

    #[test]
    fn test_stratified_reservoir_deterministic() {
        let input = (0..100)
            .map(|i| {
                let tag = if i % 2 == 0 { "EVEN" } else { "ODD" };
                format!("[{}] line{}", tag, i)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let reader1 = BufReader::new(Cursor::new(&input));
        let reader2 = BufReader::new(Cursor::new(&input));
        let regex1 = Regex::new(r"\[(EVEN|ODD)\]").unwrap();
        let regex2 = Regex::new(r"\[(EVEN|ODD)\]").unwrap();
        let strategy1 = ReservoirStrategy { count: 5 };
        let strategy2 = ReservoirStrategy { count: 5 };
        let mode1 = SamplingMode::Batch(Box::new(StratifiedBatchSampler::new(
            strategy1,
            regex1,
            SeededRng::new(42),
        )));
        let mode2 = SamplingMode::Batch(Box::new(StratifiedBatchSampler::new(
            strategy2,
            regex2,
            SeededRng::new(42),
        )));
        let mut output1 = Vec::new();
        let mut output2 = Vec::new();

        mode1.sample(reader1, &mut output1).unwrap();
        mode2.sample(reader2, &mut output2).unwrap();

        // Same seed should produce identical output
        assert_eq!(output1, output2);
    }

    #[test]
    fn test_stratified_reservoir_insufficient_lines() {
        // When reservoir count > stratum size, should return all lines from that stratum
        let input = indoc! {"
            [INFO] 1
            [INFO] 2
            [ERROR] 3
        "};
        let reader = BufReader::new(Cursor::new(input));
        let regex = Regex::new(r"\[(INFO|ERROR)\]").unwrap();
        let strategy = ReservoirStrategy { count: 10 };
        let mode = SamplingMode::Batch(Box::new(StratifiedBatchSampler::new(
            strategy,
            regex,
            SeededRng::new(42),
        )));
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        // Should output all lines (count > available in each stratum)
        assert_eq!(stats.lines_output, 3);
        let strata = stats.details.stratum_stats.as_ref().unwrap();
        assert_eq!(strata["[INFO]"].lines_output, 2);
        assert_eq!(strata["[ERROR]"].lines_output, 1);
    }

    #[test]
    fn test_reservoir_state_index_tracking() {
        // Unit test to verify ReservoirState maintains indices correctly
        let strategy = ReservoirStrategy { count: 3 };
        let rng = StdRng::seed_from_u64(42);
        let mut state = strategy.init_state(rng);

        // Add 10 lines
        for i in 0..10 {
            strategy.add_line(&mut state, format!("line{}", i));
        }

        let selected = strategy.finalize(state);

        // Should have 3 lines
        assert_eq!(selected.len(), 3);

        // Indices should be in ascending order (input order preserved)
        for i in 0..selected.len() - 1 {
            assert!(
                selected[i].0 < selected[i + 1].0,
                "Indices should be in ascending order"
            );
        }

        // All indices should be < 10
        for (idx, _) in &selected {
            assert!(*idx < 10);
        }
    }

    #[test]
    fn test_stratified_reservoir_single_stratum() {
        // Edge case: stratified sampling with only one stratum should work like non-stratified
        let input = indoc! {"
            [INFO] 1
            [INFO] 2
            [INFO] 3
            [INFO] 4
            [INFO] 5
        "};
        let reader = BufReader::new(Cursor::new(input));
        let regex = Regex::new(r"\[INFO\]").unwrap();
        let strategy = ReservoirStrategy { count: 2 };
        let mode = SamplingMode::Batch(Box::new(StratifiedBatchSampler::new(
            strategy,
            regex,
            SeededRng::new(42),
        )));
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        assert_eq!(stats.lines_output, 2);
        let strata = stats.details.stratum_stats.as_ref().unwrap();
        assert_eq!(strata.len(), 1);
        assert_eq!(strata["[INFO]"].lines_output, 2);
    }

    #[test]
    fn test_statistics_output_format_stratified_rate() {
        // Test the formatted statistics output for stratified rate sampling
        let input = indoc! {"
            [ERROR] critical failure
            [INFO] starting process
            [INFO] processing item 1
            [ERROR] connection timeout
            [INFO] processing item 2
            [INFO] processing item 3
            [ERROR] validation failed
            [INFO] processing item 4
            [INFO] processing item 5
            [INFO] processing item 6
        "};
        let reader = BufReader::new(Cursor::new(input));
        let regex = Regex::new(r"\[(INFO|ERROR)\]").unwrap();
        let strategy = RateStrategy { rate: 0.5 };
        let mode = SamplingMode::Streaming(Box::new(StratifiedStreamSampler::new(
            strategy,
            regex,
            SeededRng::new(42),
        )));
        let mut output = Vec::new();

        let stats = mode.sample(reader, &mut output).unwrap();

        // Capture the formatted output
        let mut stats_output = Vec::new();
        stats.write_stats(&mut stats_output).unwrap();
        let stats_string = String::from_utf8(stats_output).unwrap();

        // Verify basic structure
        assert_eq!(stats.lines_read, 10);
        let strata = stats.details.stratum_stats.as_ref().unwrap();
        assert_eq!(strata["[ERROR]"].lines_read, 3);
        assert_eq!(strata["[INFO]"].lines_read, 7);

        // Expected output format (exact counts depend on seed=42 RNG)
        let expected = indoc! {"
            5 / 10 (50.00%)
            Rate Sampling (stratified by \\[(INFO|ERROR)\\])
              parameters: rate=0.5000
              seed: 42
            expectations:
              sampling_rate: 0.5000
            stratification:
              strata found: 2
                [ERROR]: 3 lines (30.0%) -> sampled 1 (33.3%)
                [INFO]: 7 lines (70.0%) -> sampled 4 (57.1%)
        "};

        assert_eq!(stats_string, expected);
    }
}
