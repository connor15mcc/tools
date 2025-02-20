use anyhow::Result;
use chrono::{DateTime, Utc};
use std::io::{self, BufRead};

pub fn score<D: Decay>(d: D) -> Result<f64> {
    let stdin = io::stdin();
    let lines = stdin.lock().lines();

    let date_times = lines
        .map(|line| dateparser::parse(&line?))
        .collect::<Result<Vec<DateTime<_>>>>()?;
    Ok(d.decay(date_times))
}

pub trait Decay {
    // TODO: this should take (id, DateTime) tuples instead? would probably be more powerful
    fn decay(&self, elts: Vec<DateTime<Utc>>) -> f64;
}

pub struct HackerNews;

impl Decay for HackerNews {
    /// A modified implementation of hackernews' original scoring formula (described below).
    /// Given no points system, the numerator should := 1; measure time in days (vs hours);
    /// and use an ~arbitrary scaling constant for niceness.
    ///
    /// Original formula:
    /// Score = (P-1) / (T+2)^G
    /// where:
    ///   P = points of an item (and -1 is to negate submitters vote)
    ///   T = time since submission (in hours)
    ///   G = Gravity, defaults to 1.8 in news.arc
    /// https://medium.com/hacking-and-gonzo/how-hacker-news-ranking-algorithm-works-1d9b0cf2c08d#.ox5alx7gb
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

#[cfg(test)]
mod hackernews_tests {
    use super::*;

    #[test]
    fn all_now() {
        let now = chrono::offset::Utc::now();
        let dates = [now; 5].to_vec();
        let score = HackerNews.decay(dates);
        assert!(score <= 200.0);
        assert!(100.0 <= score);
    }

    #[test]
    fn all_last_week() {
        let last_week = chrono::offset::Utc::now() - chrono::Days::new(7);
        let dates = [last_week; 5].to_vec();
        let score = HackerNews.decay(dates);
        assert!(score <= 10.0);
        assert!(1.0 <= score);
    }

    #[test]
    fn all_last_year() {
        let last_year = chrono::offset::Utc::now() - chrono::Months::new(12);
        let dates = [last_year; 5].to_vec();
        let score = HackerNews.decay(dates);
        assert!(score <= 0.1);
        assert!(0.01 <= score);
    }
}

/// Based on this "ship of theseus" analysis of popular OSS projects, which estimates the half-life
/// of a project to be ~3.3 years.
///
/// This is probably skewed (they studied popular, successful projects like git, rails, redis,
/// django, node, etc), but it feels like a good place to start
///
/// https://raw.githubusercontent.com/erikbern/git-of-theseus/master/pics/git-projects-survival-exp-fit.png
const ANNUAL_RATE: f64 = 0.23;

pub struct InterestRate {
    rate: f64,
}

impl InterestRate {
    /// rate is assumed to be the annual rate
    pub fn new(rate: Option<f64>) -> Self {
        // we adjust rate for our implicit (internal) period of compounding (weeks)
        let rate = rate.unwrap_or(ANNUAL_RATE) / 52.0;
        InterestRate { rate }
    }
}

impl Decay for InterestRate {
    /// InterestRate decay simulates the deflation in value of any past event by a constant rate.
    /// This definition has a number of advantages, namely:
    ///
    /// For one such event:
    /// PV = HV / (1 + rate) ^ P
    /// where:
    ///   PV = present value
    ///   HV = historical value (assumed to be one for the singular event)
    ///   P  = number of periods (measured in days here)
    fn decay(&self, elts: Vec<DateTime<chrono::Utc>>) -> f64 {
        // TODO: this should take a time period (DI) and be agnostic to that length of time
        // (or at least just be agnostic, such that rates are given in years but computed
        // more continuously
        let now = chrono::offset::Utc::now();

        let mut score = 0.0;
        for elt in elts {
            let p = (now - elt).num_weeks().try_into().unwrap();
            score += 1.0 / (1.0 + self.rate).powi(p)
        }

        score
    }
}
#[cfg(test)]
mod interestrate_tests {
    use super::*;

    #[test]
    fn old_values_more_decayed() {
        let today = InterestRate::new(None).decay(vec![chrono::offset::Utc::now()]);
        let last_month = InterestRate::new(None)
            .decay(vec![chrono::offset::Utc::now() - chrono::Months::new(1)]);
        assert!(last_month < today);
    }

    #[test]
    fn convergence() {
        let old = InterestRate::new(None)
            .decay(vec![chrono::offset::Utc::now() - chrono::Months::new(66)]);
        let even_older = InterestRate::new(None)
            .decay(vec![chrono::offset::Utc::now() - chrono::Months::new(67)]);

        assert!(old - even_older <= 0.005);
    }

    #[test]
    fn half_life() {
        let today = InterestRate::new(None).decay(vec![chrono::offset::Utc::now()]);
        let half_life = InterestRate::new(None)
            .decay(vec![chrono::offset::Utc::now() - chrono::Months::new(36)]);
        let ratio = half_life / today;

        assert!(ratio <= 0.51);
        assert!(0.49 <= ratio);
    }
}
