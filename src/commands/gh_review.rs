use std::{
    collections::HashMap,
    fs::File,
    io::{self, IsTerminal, Read, Write},
    path::PathBuf,
    process::Command,
};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local, NaiveDate, Utc};
use clap::{Parser, ValueEnum};
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{self, ClearType},
};
use log::{debug, info, warn};
use num_format::{Locale, ToFormattedString};
use serde::{Deserialize, Serialize};
use xshell::{cmd, Shell};

use crate::command::CommandRunner;

// =============================================================================
// Config file structures
// =============================================================================

#[derive(Debug, Deserialize)]
struct Config {
    default_team: Option<String>,
    default_since: Option<String>,
    #[serde(default)]
    teams: HashMap<String, Team>,
    #[serde(default)]
    preferences: Preferences,
}

#[derive(Debug, Deserialize)]
struct Team {
    members: Vec<String>,
    default_since: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Preferences {
    #[serde(default = "default_show_line_stats")]
    show_line_stats: bool,
}

fn default_show_line_stats() -> bool {
    true
}

// =============================================================================
// CLI structures
// =============================================================================

#[derive(Parser)]
#[command(
    name = "gh-review",
    about = "Review GitHub PRs from a search query or team config"
)]
pub struct Review {
    /// GitHub search query string (e.g., "is:pr is:open author:username")
    /// If provided, bypasses team mode entirely.
    query: Vec<String>,

    /// Team name from config file (overrides default_team)
    #[arg(short, long)]
    team: Option<String>,

    /// Time window for PR search: 7d, 2w, 1m, etc.
    #[arg(short, long)]
    since: Option<String>,

    /// Filter PRs by date type
    #[arg(short = 'f', long, value_enum, default_value = "updated")]
    date_filter: DateFilter,

    /// Limit the number of PRs per member (or total in query mode)
    #[arg(short, long, default_value = "30")]
    limit: u32,

    /// Skip fetching line statistics (faster, less API calls)
    #[arg(long)]
    no_stats: bool,

    /// Path to config file (default: ~/.config/gh-review/config.toml)
    #[arg(long)]
    config: Option<PathBuf>,

    /// Enable interactive TUI mode (team mode only)
    #[arg(short, long)]
    interactive: bool,
}

#[derive(Clone, Debug, ValueEnum)]
enum DateFilter {
    Created,
    Updated,
    Merged,
}

impl DateFilter {
    fn as_query_field(&self) -> &'static str {
        match self {
            DateFilter::Created => "created",
            DateFilter::Updated => "updated",
            DateFilter::Merged => "merged",
        }
    }
}

// =============================================================================
// PR data structures
// =============================================================================

#[derive(Debug, Clone, Deserialize, Serialize)]
struct PrInfo {
    number: u32,
    repository: Repository,
    title: String,
    url: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct Repository {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Debug, Deserialize)]
struct PrStats {
    additions: u32,
    deletions: u32,
}

#[derive(Debug, Clone)]
struct MemberPrData {
    member: String,
    prs: Vec<PrInfo>,
    total_additions: u32,
    total_deletions: u32,
}

// =============================================================================
// Session data structures
// =============================================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum MemberReviewStatus {
    Pending,
    Reviewed,
    Skipped,
}

#[derive(Debug, Serialize, Deserialize)]
struct ReviewSession {
    team: String,
    start_date: NaiveDate,
    end_date: NaiveDate,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    members: HashMap<String, MemberSessionData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemberSessionData {
    status: MemberReviewStatus,
    reviewed_at: Option<DateTime<Utc>>,
    pr_count: u32,
    additions: u32,
    deletions: u32,
    #[serde(default)]
    prs: Vec<PrInfo>,
}

// Runtime state for the TUI
struct InteractiveState {
    session: ReviewSession,
    member_order: Vec<String>, // Maintains consistent order
    cursor: usize,
    show_stats: bool,
}

// =============================================================================
// Config loading
// =============================================================================

fn default_config_path() -> PathBuf {
    let xdg = xdg::BaseDirectories::new();
    xdg.get_config_home()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("gh-review")
        .join("config.toml")
}

fn load_config(path: Option<&PathBuf>) -> Result<Option<Config>> {
    let config_path = path.cloned().unwrap_or_else(default_config_path);

    if !config_path.exists() {
        return Ok(None);
    }

    let mut file = File::open(&config_path)
        .with_context(|| format!("Failed to open config file: {}", config_path.display()))?;

    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;

    let config: Config = toml::from_str(&contents)
        .with_context(|| format!("Failed to parse config file: {}", config_path.display()))?;

    Ok(Some(config))
}

fn print_config_help() {
    let config_path = default_config_path();
    eprintln!(
        r#"Error: No query provided and no config file found.

To use team mode, create a config file at:
  {}

Example config:

  default_team = "my-team"
  default_since = "7d"

  [teams.my-team]
  members = ["alice", "bob", "charlie"]

  [preferences]
  show_line_stats = true

Or provide a direct query:
  tools gh-review "is:pr author:alice"
"#,
        config_path.display()
    );
}

// =============================================================================
// Session file management
// =============================================================================

fn sessions_dir() -> PathBuf {
    let xdg = xdg::BaseDirectories::new();
    xdg.get_data_home()
        .unwrap_or_else(|| PathBuf::from("~/.local/share"))
        .join("gh-review")
        .join("sessions")
}

fn session_file_path(team: &str, start_date: NaiveDate, end_date: NaiveDate) -> PathBuf {
    sessions_dir().join(format!(
        "{}-{}-{}.json",
        team,
        start_date.format("%Y-%m-%d"),
        end_date.format("%Y-%m-%d")
    ))
}

fn load_session(
    team: &str,
    start_date: NaiveDate,
    end_date: NaiveDate,
) -> Result<Option<ReviewSession>> {
    let path = session_file_path(team, start_date, end_date);

    if !path.exists() {
        return Ok(None);
    }

    let mut file = File::open(&path)
        .with_context(|| format!("Failed to open session file: {}", path.display()))?;

    let mut contents = String::new();
    file.read_to_string(&mut contents)
        .with_context(|| format!("Failed to read session file: {}", path.display()))?;

    match serde_json::from_str(&contents) {
        Ok(session) => Ok(Some(session)),
        Err(e) => {
            warn!("Session file corrupted, starting fresh: {}", e);
            Ok(None)
        }
    }
}

fn save_session(session: &ReviewSession) -> Result<()> {
    let dir = sessions_dir();
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create sessions directory: {}", dir.display()))?;

    let path = session_file_path(&session.team, session.start_date, session.end_date);

    let contents = serde_json::to_string_pretty(session).context("Failed to serialize session")?;

    std::fs::write(&path, contents)
        .with_context(|| format!("Failed to write session file: {}", path.display()))?;

    debug!("Saved session to {}", path.display());
    Ok(())
}

fn create_new_session(
    team_name: &str,
    start_date: NaiveDate,
    end_date: NaiveDate,
    member_data: &[MemberPrData],
) -> ReviewSession {
    let mut members = HashMap::new();

    for data in member_data {
        members.insert(
            data.member.clone(),
            MemberSessionData {
                status: MemberReviewStatus::Pending,
                reviewed_at: None,
                pr_count: data.prs.len() as u32,
                additions: data.total_additions,
                deletions: data.total_deletions,
                prs: data.prs.clone(),
            },
        );
    }

    ReviewSession {
        team: team_name.to_string(),
        start_date,
        end_date,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        members,
    }
}

// =============================================================================
// Date/time parsing
// =============================================================================

fn parse_since(since: &str) -> Result<NaiveDate> {
    let since_trimmed = since.trim();

    // Handle shorthand formats: 7d, 2w, 1m
    if let Some(duration) = parse_shorthand_duration(since_trimmed) {
        let now = Local::now().naive_local();
        let target = now - duration;
        return Ok(target.date());
    }

    // Try parsing with humantime for formats like "7 days", "2 weeks"
    if let Ok(duration) = humantime::parse_duration(since_trimmed) {
        let now = Local::now().naive_local();
        let chrono_duration = chrono::Duration::from_std(duration).context("Duration too large")?;
        let target = now - chrono_duration;
        return Ok(target.date());
    }

    // Try parsing as an absolute date
    if let Ok(date) = NaiveDate::parse_from_str(since_trimmed, "%Y-%m-%d") {
        return Ok(date);
    }

    bail!(
        "Could not parse '{}' as a duration or date. \
         Try formats like '7d', '2w', '1m', '7 days', or '2024-12-16'",
        since
    )
}

fn parse_shorthand_duration(s: &str) -> Option<chrono::Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let (num_str, unit) = s.split_at(s.len() - 1);
    let num: i64 = num_str.parse().ok()?;

    match unit {
        "d" => Some(chrono::Duration::days(num)),
        "w" => Some(chrono::Duration::weeks(num)),
        "m" => Some(chrono::Duration::days(num * 30)),
        _ => None,
    }
}

// =============================================================================
// GitHub API functions
// =============================================================================

fn fetch_prs_for_member(
    sh: &Shell,
    member: &str,
    date_filter: &DateFilter,
    since_date: NaiveDate,
    limit: u32,
) -> Result<Vec<PrInfo>> {
    let date_query = format!(
        "{}:>={}",
        date_filter.as_query_field(),
        since_date.format("%Y-%m-%d")
    );

    debug!("Searching PRs for {} with filter: {}", member, date_query);

    let search_result = cmd!(sh, "gh search prs")
        .arg(format!("author:{}", member))
        .arg(&date_query)
        .arg("--limit")
        .arg(limit.to_string())
        .arg("--json")
        .arg("number,repository,title,url")
        .read()
        .with_context(|| format!("Failed to search PRs for {}", member))?;

    let prs: Vec<PrInfo> = serde_json::from_str(&search_result)
        .with_context(|| format!("Failed to parse PR search results for {}", member))?;

    debug!("Found {} PRs for {}", prs.len(), member);
    Ok(prs)
}

fn fetch_pr_stats(sh: &Shell, pr: &PrInfo) -> Result<PrStats> {
    let result = cmd!(sh, "gh pr view")
        .arg(&pr.url)
        .arg("--json")
        .arg("additions,deletions")
        .read()
        .with_context(|| format!("Failed to fetch stats for PR #{}", pr.number))?;

    let stats: PrStats = serde_json::from_str(&result)
        .with_context(|| format!("Failed to parse stats for PR #{}", pr.number))?;

    Ok(stats)
}

fn fetch_all_member_data(
    sh: &Shell,
    team: &Team,
    date_filter: &DateFilter,
    since_date: NaiveDate,
    limit: u32,
    show_stats: bool,
) -> Result<Vec<MemberPrData>> {
    let mut all_member_data: Vec<MemberPrData> = Vec::new();

    for member in &team.members {
        info!("Fetching PRs for {}...", member);

        let prs = fetch_prs_for_member(sh, member, date_filter, since_date, limit)?;

        let (total_additions, total_deletions) = if show_stats && !prs.is_empty() {
            let mut additions = 0u32;
            let mut deletions = 0u32;

            for pr in &prs {
                match fetch_pr_stats(sh, pr) {
                    Ok(stats) => {
                        additions += stats.additions;
                        deletions += stats.deletions;
                    }
                    Err(e) => {
                        warn!("Could not fetch stats for PR #{}: {}", pr.number, e);
                    }
                }
            }

            (additions, deletions)
        } else {
            (0, 0)
        };

        all_member_data.push(MemberPrData {
            member: member.clone(),
            prs,
            total_additions,
            total_deletions,
        });
    }

    Ok(all_member_data)
}

// =============================================================================
// Display functions (non-interactive)
// =============================================================================

fn print_team_summary(
    team_name: &str,
    since_date: NaiveDate,
    end_date: NaiveDate,
    members: &[MemberPrData],
    show_stats: bool,
) {
    let since_str = since_date.format("%b %d").to_string();
    let end_str = end_date.format("%b %d").to_string();

    println!();
    println!(
        "Weekly PR Review - {} team ({}-{})",
        team_name, since_str, end_str
    );
    println!("{}", "─".repeat(50));

    let mut total_prs = 0u32;
    let mut total_additions = 0u32;
    let mut total_deletions = 0u32;

    let max_name_len = members.iter().map(|m| m.member.len()).max().unwrap_or(0);

    for member_data in members {
        total_prs += member_data.prs.len() as u32;
        total_additions += member_data.total_additions;
        total_deletions += member_data.total_deletions;

        let pr_count = member_data.prs.len();
        let pr_word = if pr_count == 1 { "PR" } else { "PRs" };

        if show_stats {
            println!(
                "{:<width$}  {:>3} {:3}  {:>+7}/{:<-7} lines",
                member_data.member,
                pr_count,
                pr_word,
                format!(
                    "+{}",
                    member_data.total_additions.to_formatted_string(&Locale::en)
                ),
                format!(
                    "-{}",
                    member_data.total_deletions.to_formatted_string(&Locale::en)
                ),
                width = max_name_len
            );
        } else {
            println!(
                "{:<width$}  {:>3} {}",
                member_data.member,
                pr_count,
                pr_word,
                width = max_name_len
            );
        }
    }

    println!("{}", "─".repeat(50));

    let pr_word = if total_prs == 1 { "PR" } else { "PRs" };
    if show_stats {
        println!(
            "{:<width$}  {:>3} {:3}  {:>+7}/{:<-7} lines",
            "Total",
            total_prs,
            pr_word,
            format!("+{}", total_additions.to_formatted_string(&Locale::en)),
            format!("-{}", total_deletions.to_formatted_string(&Locale::en)),
            width = max_name_len
        );
    } else {
        println!(
            "{:<width$}  {:>3} {}",
            "Total",
            total_prs,
            pr_word,
            width = max_name_len
        );
    }
    println!();
}

fn wait_for_enter() -> Result<bool> {
    print!("Press Enter to review diffs, Ctrl+C to cancel...");
    io::stdout().flush()?;

    let mut input = String::new();
    match io::stdin().read_line(&mut input) {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

// =============================================================================
// Interactive TUI functions
// =============================================================================

/// Print a line in raw mode (requires \r\n for proper line breaks)
macro_rules! tui_println {
    ($stdout:expr) => {
        write!($stdout, "\r\n")?
    };
    ($stdout:expr, $($arg:tt)*) => {
        write!($stdout, "{}\r\n", format!($($arg)*))?
    };
}

/// Format a key hint with color highlighting
fn key(k: &str) -> String {
    format!(
        "{}{}[{}]{}{}",
        SetForegroundColor(Color::Cyan),
        SetAttribute(Attribute::Bold),
        k,
        SetAttribute(Attribute::Reset),
        ResetColor
    )
}

/// Format a key hint with description
fn key_desc(k: &str, desc: &str) -> String {
    format!("{} {}", key(k), desc)
}

fn render_interactive_screen(state: &InteractiveState) -> Result<()> {
    let mut stdout = io::stdout();

    // Clear screen and move to top
    execute!(
        stdout,
        terminal::Clear(ClearType::All),
        cursor::MoveTo(0, 0)
    )?;

    let session = &state.session;
    let since_str = session.start_date.format("%b %d").to_string();
    let end_str = session.end_date.format("%b %d").to_string();

    // Header
    tui_println!(
        stdout,
        "Weekly PR Review - {} team ({}-{})",
        session.team,
        since_str,
        end_str
    );
    tui_println!(stdout, "{}", "─".repeat(70));
    tui_println!(stdout);

    // Calculate totals and progress
    let mut total_prs = 0u32;
    let mut total_additions = 0u32;
    let mut total_deletions = 0u32;
    let mut reviewed_count = 0usize;
    let mut reviewed_prs = 0u32;

    for member in &state.member_order {
        if let Some(data) = session.members.get(member) {
            total_prs += data.pr_count;
            total_additions += data.additions;
            total_deletions += data.deletions;
            if data.status == MemberReviewStatus::Reviewed {
                reviewed_count += 1;
                reviewed_prs += data.pr_count;
            }
        }
    }

    // Member list
    let max_name_len = state
        .member_order
        .iter()
        .map(|m| m.len())
        .max()
        .unwrap_or(0)
        .max(5);

    for (idx, member) in state.member_order.iter().enumerate() {
        let data = match session.members.get(member) {
            Some(d) => d,
            None => continue,
        };

        let status_icon = match data.status {
            MemberReviewStatus::Reviewed => "[✓]",
            MemberReviewStatus::Skipped => "[–]",
            MemberReviewStatus::Pending => "[ ]",
        };

        let status_text = match data.status {
            MemberReviewStatus::Reviewed => "reviewed",
            MemberReviewStatus::Skipped => "skipped",
            MemberReviewStatus::Pending => "pending",
        };

        let cursor_indicator = if idx == state.cursor { "  <--" } else { "" };

        let pr_word = if data.pr_count == 1 { "PR" } else { "PRs" };

        if state.show_stats {
            tui_println!(
                stdout,
                "  {} {:<width$}  {:>3} {:3}  {:>+7}/{:<-7} lines   {:8}{}",
                status_icon,
                member,
                data.pr_count,
                pr_word,
                format!("+{}", data.additions.to_formatted_string(&Locale::en)),
                format!("-{}", data.deletions.to_formatted_string(&Locale::en)),
                status_text,
                cursor_indicator,
                width = max_name_len
            );
        } else {
            tui_println!(
                stdout,
                "  {} {:<width$}  {:>3} {:3}   {:8}{}",
                status_icon,
                member,
                data.pr_count,
                pr_word,
                status_text,
                cursor_indicator,
                width = max_name_len
            );
        }
    }

    tui_println!(stdout);
    tui_println!(stdout, "{}", "─".repeat(70));

    // Progress summary
    let pr_word = if total_prs == 1 { "PR" } else { "PRs" };
    if state.show_stats {
        tui_println!(
            stdout,
            "Total: {} {}  +{}/{} lines   Progress: {}/{} members ({}/{} PRs)",
            total_prs,
            pr_word,
            format!("+{}", total_additions.to_formatted_string(&Locale::en)),
            format!("-{}", total_deletions.to_formatted_string(&Locale::en)),
            reviewed_count,
            state.member_order.len(),
            reviewed_prs,
            total_prs
        );
    } else {
        tui_println!(
            stdout,
            "Total: {} {}   Progress: {}/{} members ({}/{} PRs)",
            total_prs,
            pr_word,
            reviewed_count,
            state.member_order.len(),
            reviewed_prs,
            total_prs
        );
    }

    tui_println!(stdout, "{}", "─".repeat(70));

    // Commands
    let current_member = &state.member_order[state.cursor];
    tui_println!(stdout);
    tui_println!(stdout, "Commands:");
    tui_println!(
        stdout,
        "  {} Review {}'s PRs    {}    {}    {}",
        key("Enter"),
        current_member,
        key_desc("s", "skip"),
        key_desc("m", "mark reviewed"),
        key_desc("r", "re-review")
    );
    tui_println!(
        stdout,
        "  {} Down    {} Up    {}    {}",
        key("j/↓"),
        key("k/↑"),
        key_desc("q", "Quit & save"),
        key_desc("Q", "Quit without saving")
    );

    stdout.flush()?;
    Ok(())
}

fn review_member_prs(sh: &Shell, state: &mut InteractiveState) -> Result<()> {
    let member = &state.member_order[state.cursor];
    let member_data = match state.session.members.get(member) {
        Some(d) => d.clone(),
        None => return Ok(()),
    };

    if member_data.prs.is_empty() {
        return Ok(());
    }

    // Exit raw mode temporarily
    terminal::disable_raw_mode()?;
    execute!(
        io::stdout(),
        terminal::Clear(ClearType::All),
        cursor::MoveTo(0, 0)
    )?;

    // Create temp file for this member's diffs
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join(format!("pr-review-{}.diff", member));

    let mut file = File::create(&temp_file)
        .with_context(|| format!("Failed to create temp file: {}", temp_file.display()))?;

    // Write member header
    let member_header = format!(
        "\n{}\n{} ({} PRs)\n{}\n\n",
        "=".repeat(80),
        member.to_uppercase(),
        member_data.prs.len(),
        "=".repeat(80)
    );
    file.write_all(member_header.as_bytes())?;

    // Fetch and write each PR's diff
    for pr in &member_data.prs {
        info!("Fetching diff for PR #{}: {}", pr.number, pr.title);

        let diff = cmd!(sh, "gh pr diff")
            .arg(pr.number.to_string())
            .arg("--repo")
            .arg(&pr.repository.name_with_owner)
            .arg("--color")
            .arg("always")
            .read()
            .with_context(|| format!("Failed to fetch diff for PR #{}", pr.number))?;

        let pr_header = format!(
            "\n{}\n\
             Repository: {}\n\
             PR #{}: {}\n\
             URL: {}\n\
             {}\n\n",
            "-".repeat(80),
            pr.repository.name_with_owner,
            pr.number,
            pr.title,
            pr.url,
            "-".repeat(80)
        );

        file.write_all(pr_header.as_bytes())?;
        file.write_all(diff.as_bytes())?;
        file.write_all(b"\n")?;
    }

    drop(file);

    // Open in less
    Command::new("less")
        .arg("-R")
        .arg(format!("--prompt=Reviewing {}'s PRs (q to finish)", member))
        .arg(&temp_file)
        .status()
        .context("Failed to run 'less'")?;

    // Clean up
    let _ = std::fs::remove_file(temp_file);

    // Mark as reviewed
    if let Some(data) = state.session.members.get_mut(member) {
        data.status = MemberReviewStatus::Reviewed;
        data.reviewed_at = Some(Utc::now());
    }
    state.session.updated_at = Utc::now();

    // Move to next pending member
    move_to_next_pending(state);

    // Re-enter raw mode
    terminal::enable_raw_mode()?;

    Ok(())
}

fn move_to_next_pending(state: &mut InteractiveState) {
    let start = state.cursor;
    let len = state.member_order.len();

    for i in 1..=len {
        let idx = (start + i) % len;
        let member = &state.member_order[idx];
        if let Some(data) = state.session.members.get(member) {
            if data.status == MemberReviewStatus::Pending {
                state.cursor = idx;
                return;
            }
        }
    }
    // No pending members found, stay at current position
}

fn skip_member(state: &mut InteractiveState) {
    let member = &state.member_order[state.cursor];
    if let Some(data) = state.session.members.get_mut(member) {
        data.status = MemberReviewStatus::Skipped;
    }
    state.session.updated_at = Utc::now();
    move_to_next_pending(state);
}

fn mark_reviewed(state: &mut InteractiveState) {
    let member = &state.member_order[state.cursor];
    if let Some(data) = state.session.members.get_mut(member) {
        data.status = MemberReviewStatus::Reviewed;
        data.reviewed_at = Some(Utc::now());
    }
    state.session.updated_at = Utc::now();
    move_to_next_pending(state);
}

fn reset_to_pending(state: &mut InteractiveState) {
    let member = &state.member_order[state.cursor];
    if let Some(data) = state.session.members.get_mut(member) {
        data.status = MemberReviewStatus::Pending;
        data.reviewed_at = None;
    }
    state.session.updated_at = Utc::now();
}

enum ResumeChoice {
    ResumeCached,
    ResumeRefetch,
    NewSession,
    Quit,
}

fn prompt_resume_session(session: &ReviewSession) -> Result<ResumeChoice> {
    let updated = session.updated_at.with_timezone(&Local);

    // Count statuses
    let mut reviewed = 0;
    let mut pending = 0;
    let mut skipped = 0;

    for data in session.members.values() {
        match data.status {
            MemberReviewStatus::Reviewed => reviewed += 1,
            MemberReviewStatus::Pending => pending += 1,
            MemberReviewStatus::Skipped => skipped += 1,
        }
    }

    println!();
    println!(
        "Found existing session for {} team ({} - {})",
        session.team,
        session.start_date.format("%b %d"),
        session.end_date.format("%b %d")
    );
    println!("Last updated: {}", updated.format("%Y-%m-%d %H:%M"));
    println!(
        "Progress: {}/{} members reviewed ({} pending, {} skipped)",
        reviewed,
        session.members.len(),
        pending,
        skipped
    );
    println!();
    println!("{} Resume with cached data (default)", key("r"));
    println!("{} Resume and re-fetch PR data from GitHub", key("R"));
    println!("{} Start new session", key("n"));
    println!("{} Quit", key("q"));
    print!("\nChoice: ");
    io::stdout().flush()?;

    // Use raw mode for single-character input (no Enter required)
    terminal::enable_raw_mode()?;

    let choice = loop {
        if let Event::Key(key) = event::read()? {
            let result = match key.code {
                KeyCode::Char('r') => Some(ResumeChoice::ResumeCached),
                KeyCode::Char('R') => Some(ResumeChoice::ResumeRefetch),
                KeyCode::Char('n') => Some(ResumeChoice::NewSession),
                KeyCode::Char('q') | KeyCode::Char('Q') => Some(ResumeChoice::Quit),
                KeyCode::Enter => Some(ResumeChoice::ResumeCached), // Default
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    Some(ResumeChoice::Quit)
                }
                _ => None,
            };
            if let Some(choice) = result {
                break choice;
            }
        }
    };

    terminal::disable_raw_mode()?;
    println!(); // Move to next line after selection

    Ok(choice)
}

fn run_interactive_mode(
    sh: &Shell,
    team_name: &str,
    team: &Team,
    since_date: NaiveDate,
    date_filter: &DateFilter,
    limit: u32,
    show_stats: bool,
) -> Result<()> {
    // Check if we're in a terminal
    if !io::stdout().is_terminal() {
        bail!("Interactive mode requires a terminal. Use without --interactive for non-TTY environments.");
    }

    let end_date = Local::now().naive_local().date();

    // Check for existing session
    let existing_session = load_session(team_name, since_date, end_date)?;

    let (session, member_order) = if let Some(existing) = existing_session {
        match prompt_resume_session(&existing)? {
            ResumeChoice::ResumeCached => {
                // Use cached session, preserve order from team config
                let order: Vec<String> = team
                    .members
                    .iter()
                    .filter(|m| existing.members.contains_key(*m))
                    .cloned()
                    .collect();
                (existing, order)
            }
            ResumeChoice::ResumeRefetch => {
                // Re-fetch data but preserve review status
                println!("\nRe-fetching PR data from GitHub...");
                let member_data =
                    fetch_all_member_data(sh, team, date_filter, since_date, limit, show_stats)?;

                let mut new_session =
                    create_new_session(team_name, since_date, end_date, &member_data);

                // Restore review status from existing session
                for (member, old_data) in &existing.members {
                    if let Some(new_data) = new_session.members.get_mut(member) {
                        new_data.status = old_data.status.clone();
                        new_data.reviewed_at = old_data.reviewed_at;
                    }
                }

                let order: Vec<String> = team.members.clone();
                (new_session, order)
            }
            ResumeChoice::NewSession => {
                // Start fresh
                println!("\nFetching PR data from GitHub...");
                let member_data =
                    fetch_all_member_data(sh, team, date_filter, since_date, limit, show_stats)?;
                let session = create_new_session(team_name, since_date, end_date, &member_data);
                let order: Vec<String> = team.members.clone();
                (session, order)
            }
            ResumeChoice::Quit => {
                return Ok(());
            }
        }
    } else {
        // No existing session, fetch fresh data
        println!("Fetching PR data from GitHub...");
        let member_data =
            fetch_all_member_data(sh, team, date_filter, since_date, limit, show_stats)?;
        let session = create_new_session(team_name, since_date, end_date, &member_data);
        let order: Vec<String> = team.members.clone();
        (session, order)
    };

    // Check if there are any PRs to review
    let total_prs: u32 = session.members.values().map(|d| d.pr_count).sum();
    if total_prs == 0 {
        println!("\nNo PRs found for the specified time period.");
        return Ok(());
    }

    // Initialize interactive state
    let mut state = InteractiveState {
        session,
        member_order,
        cursor: 0,
        show_stats,
    };

    // Move cursor to first pending member
    move_to_next_pending(&mut state);

    // Enter raw mode for keyboard input
    terminal::enable_raw_mode()?;

    // Main TUI loop
    let result = run_tui_loop(sh, &mut state);

    // Always restore terminal state
    terminal::disable_raw_mode()?;
    execute!(
        io::stdout(),
        terminal::Clear(ClearType::All),
        cursor::MoveTo(0, 0),
        cursor::Show
    )?;

    result
}

fn run_tui_loop(sh: &Shell, state: &mut InteractiveState) -> Result<()> {
    loop {
        render_interactive_screen(state)?;

        // Wait for keyboard input
        if let Event::Key(key) = event::read()? {
            match key {
                // Quit and save
                KeyEvent {
                    code: KeyCode::Char('q'),
                    modifiers: KeyModifiers::NONE,
                    ..
                } => {
                    save_session(&state.session)?;
                    println!("\nSession saved.");
                    break;
                }
                // Quit without saving
                KeyEvent {
                    code: KeyCode::Char('Q'),
                    modifiers: KeyModifiers::SHIFT,
                    ..
                } => {
                    println!("\nExiting without saving.");
                    break;
                }
                // Ctrl+C
                KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                    ..
                } => {
                    save_session(&state.session)?;
                    println!("\nSession saved.");
                    break;
                }
                // Navigate down
                KeyEvent {
                    code: KeyCode::Char('j'),
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Down,
                    ..
                } => {
                    if state.cursor < state.member_order.len() - 1 {
                        state.cursor += 1;
                    }
                }
                // Navigate up
                KeyEvent {
                    code: KeyCode::Char('k'),
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Up, ..
                } => {
                    if state.cursor > 0 {
                        state.cursor -= 1;
                    }
                }
                // Review current member
                KeyEvent {
                    code: KeyCode::Enter,
                    ..
                }
                | KeyEvent {
                    code: KeyCode::Char('n'),
                    ..
                } => {
                    let member = &state.member_order[state.cursor];
                    let pr_count = state
                        .session
                        .members
                        .get(member)
                        .map(|d| d.pr_count)
                        .unwrap_or(0);
                    if pr_count > 0 {
                        review_member_prs(sh, state)?;
                    }
                }
                // Skip member
                KeyEvent {
                    code: KeyCode::Char('s'),
                    ..
                } => {
                    skip_member(state);
                }
                // Mark as reviewed without viewing
                KeyEvent {
                    code: KeyCode::Char('m'),
                    ..
                } => {
                    mark_reviewed(state);
                }
                // Re-review (reset to pending)
                KeyEvent {
                    code: KeyCode::Char('r'),
                    ..
                } => {
                    reset_to_pending(state);
                }
                _ => {}
            }
        }
    }

    Ok(())
}

// =============================================================================
// Non-interactive team mode
// =============================================================================

fn run_team_mode(
    sh: &Shell,
    team_name: &str,
    team: &Team,
    since_date: NaiveDate,
    date_filter: &DateFilter,
    limit: u32,
    show_stats: bool,
) -> Result<()> {
    let all_member_data =
        fetch_all_member_data(sh, team, date_filter, since_date, limit, show_stats)?;

    // Print summary
    let end_date = Local::now().naive_local().date();
    print_team_summary(
        team_name,
        since_date,
        end_date,
        &all_member_data,
        show_stats,
    );

    // Check if there are any PRs to review
    let total_prs: usize = all_member_data.iter().map(|m| m.prs.len()).sum();
    if total_prs == 0 {
        println!("No PRs found for the specified time period.");
        return Ok(());
    }

    // Wait for user confirmation
    if !wait_for_enter()? {
        return Ok(());
    }

    // Create temp files for diffs, grouped by member
    let temp_dir = std::env::temp_dir();
    let mut temp_files = Vec::new();

    for member_data in &all_member_data {
        if member_data.prs.is_empty() {
            continue;
        }

        let temp_file = temp_dir.join(format!("pr-review-{}.diff", member_data.member));
        let mut file = File::create(&temp_file)
            .with_context(|| format!("Failed to create temp file: {}", temp_file.display()))?;

        // Write member header
        let member_header = format!(
            "\n{}\n{} ({} PRs)\n{}\n\n",
            "=".repeat(80),
            member_data.member.to_uppercase(),
            member_data.prs.len(),
            "=".repeat(80)
        );
        file.write_all(member_header.as_bytes())?;

        // Fetch and write each PR's diff
        for pr in &member_data.prs {
            info!("Fetching diff for PR #{}: {}", pr.number, pr.title);

            let diff = cmd!(sh, "gh pr diff")
                .arg(pr.number.to_string())
                .arg("--repo")
                .arg(&pr.repository.name_with_owner)
                .arg("--color")
                .arg("always")
                .read()
                .with_context(|| format!("Failed to fetch diff for PR #{}", pr.number))?;

            let pr_header = format!(
                "\n{}\n\
                 Repository: {}\n\
                 PR #{}: {}\n\
                 URL: {}\n\
                 {}\n\n",
                "-".repeat(80),
                pr.repository.name_with_owner,
                pr.number,
                pr.title,
                pr.url,
                "-".repeat(80)
            );

            file.write_all(pr_header.as_bytes())?;
            file.write_all(diff.as_bytes())?;
            file.write_all(b"\n")?;
        }

        temp_files.push(temp_file);
    }

    // Open all temp files in less
    if !temp_files.is_empty() {
        Command::new("less")
            .arg("-R")
            .arg("--prompt=PR Review (%i/%m) [n=next file | p=prev | q=quit]")
            .args(&temp_files)
            .status()
            .context("Failed to run 'less'")?;
    }

    // Clean up temp files
    for temp_file in temp_files {
        let _ = std::fs::remove_file(temp_file);
    }

    Ok(())
}

fn run_query_mode(sh: &Shell, query: &[String], limit: u32) -> Result<()> {
    let query_str = query.join(" ");
    debug!("Searching for PRs with query: '{}'", query_str);

    // Search for PRs using gh search prs
    let mut search_cmd = cmd!(sh, "gh search prs");
    for term in query {
        search_cmd = search_cmd.arg(term);
    }
    let search_result = search_cmd
        .arg("--limit")
        .arg(limit.to_string())
        .arg("--json")
        .arg("number,repository,title,url")
        .read()
        .context("Failed to search PRs. Make sure 'gh' is installed and authenticated")?;

    // Parse the JSON result
    let prs: Vec<PrInfo> =
        serde_json::from_str(&search_result).context("Failed to parse PR search results")?;

    if prs.is_empty() {
        println!("No PRs found for query: '{}'", query_str);
        return Ok(());
    }

    debug!("Found {} PRs to review", prs.len());

    // Create temp files for each PR diff
    let temp_dir = std::env::temp_dir();
    let mut temp_files = Vec::new();

    for pr in &prs {
        let temp_file = temp_dir.join(format!(
            "pr-{}-{}.diff",
            pr.repository.name_with_owner.replace('/', "-"),
            pr.number
        ));

        info!("Fetching diff for PR #{}: {}", pr.number, pr.title);

        // Fetch the diff
        let diff = cmd!(sh, "gh pr diff")
            .arg(pr.number.to_string())
            .arg("--repo")
            .arg(&pr.repository.name_with_owner)
            .arg("--color")
            .arg("always")
            .read()
            .with_context(|| format!("Failed to fetch diff for PR #{}", pr.number))?;

        // Write header and diff to temp file
        let mut file = File::create(&temp_file)
            .with_context(|| format!("Failed to create temp file: {}", temp_file.display()))?;

        let header = format!(
            "================================================================================\n\
             Repository: {}\n\
             PR #{}: {}\n\
             URL: {}\n\
             ================================================================================\n\n",
            pr.repository.name_with_owner, pr.number, pr.title, pr.url
        );

        file.write_all(header.as_bytes())?;
        file.write_all(diff.as_bytes())?;

        temp_files.push(temp_file);
    }

    // Open all temp files in less
    Command::new("less")
        .arg("-R")
        .arg("--prompt=PR Review (%i/%m) [:n=next | :p=prev | q=quit]")
        .args(&temp_files)
        .status()
        .context("Failed to run 'less'")?;

    // Clean up temp files
    for temp_file in temp_files {
        let _ = std::fs::remove_file(temp_file);
    }

    Ok(())
}

// =============================================================================
// Main entry point
// =============================================================================

impl CommandRunner for Review {
    fn run(self) -> Result<()> {
        let sh = Shell::new()?;

        // Mode 1: Direct query provided - use existing behavior
        if !self.query.is_empty() {
            if self.interactive {
                warn!("--interactive flag is ignored when using direct query mode");
            }
            return run_query_mode(&sh, &self.query, self.limit);
        }

        // Mode 2 & 3: Team mode (explicit or default)
        let config = load_config(self.config.as_ref())?;

        let config = match config {
            Some(c) => c,
            None => {
                print_config_help();
                bail!("No config file found");
            }
        };

        // Determine which team to use
        let team_name = self
            .team
            .or(config.default_team.clone())
            .context("No team specified and no default_team in config")?;

        let team = config.teams.get(&team_name).with_context(|| {
            let available: Vec<_> = config.teams.keys().collect();
            format!(
                "Team '{}' not found in config.\n\nAvailable teams:\n{}",
                team_name,
                available
                    .iter()
                    .map(|t| format!("  - {} ({} members)", t, config.teams[*t].members.len()))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        })?;

        // Determine the since date
        let since_str = self
            .since
            .or_else(|| team.default_since.clone())
            .or(config.default_since.clone())
            .unwrap_or_else(|| "7d".to_string());

        let since_date = parse_since(&since_str)?;

        // Determine whether to show stats
        let show_stats = !self.no_stats && config.preferences.show_line_stats;

        info!(
            "Reviewing PRs for team '{}' since {} ({})",
            team_name,
            since_date.format("%Y-%m-%d"),
            self.date_filter.as_query_field()
        );

        // Choose interactive or non-interactive mode
        if self.interactive {
            run_interactive_mode(
                &sh,
                &team_name,
                team,
                since_date,
                &self.date_filter,
                self.limit,
                show_stats,
            )
        } else {
            run_team_mode(
                &sh,
                &team_name,
                team,
                since_date,
                &self.date_filter,
                self.limit,
                show_stats,
            )
        }
    }
}
