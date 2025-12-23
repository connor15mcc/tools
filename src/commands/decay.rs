use crate::command::CommandRunner;
use anyhow::Result;
use chrono::{DateTime, Utc};
use clap::Parser;
use std::io::{self, BufRead};

#[derive(Parser)]
#[command(name = "decay", about = "Calculate decay score from timestamps")]
pub struct DecayCommand {
    /// Rate with which to decay / depreciate older values (annual)
    #[arg(short, long)]
    rate: Option<f64>,
}

impl CommandRunner for DecayCommand {
    fn run(self) -> Result<()> {
        let score = score(InterestRate::new(self.rate))?;
        println!("Decay score: {score:.2}");
        Ok(())
    }
}

fn score<D: Decay>(d: D) -> Result<f64> {
    let stdin = io::stdin();
    let lines = stdin.lock().lines();

    let date_times = lines
        .map(|line| dateparser::parse(&line?))
        .collect::<Result<Vec<DateTime<_>>>>()?;
    Ok(d.decay(date_times))
}

trait Decay {
    fn decay(&self, elts: Vec<DateTime<Utc>>) -> f64;
}

struct _HackerNews;

impl Decay for _HackerNews {
    fn decay(&self, elts: Vec<DateTime<chrono::Utc>>) -> f64 {
        let now = chrono::offset::Utc::now();

        let mut score = 0.0;
        for elt in elts {
            let t = now - elt;
            let denominator = (t.num_days() as f64 + 2.0).powf(1.8);
            score += 1.0 / denominator
        }

        score * 100.0
    }
}

const ANNUAL_RATE: f64 = 0.23;

struct InterestRate {
    rate: f64,
}

impl InterestRate {
    fn new(rate: Option<f64>) -> Self {
        let rate = rate.unwrap_or(ANNUAL_RATE) / 52.0;
        InterestRate { rate }
    }
}

impl Decay for InterestRate {
    fn decay(&self, elts: Vec<DateTime<chrono::Utc>>) -> f64 {
        let now = chrono::offset::Utc::now();

        let mut score = 0.0;
        for elt in elts {
            let p = (now - elt).num_weeks().try_into().unwrap();
            score += 1.0 / (1.0 + self.rate).powi(p)
        }

        score
    }
}
