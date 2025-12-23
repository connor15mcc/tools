use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::Datelike;
use clap::Parser;
use ignore::Walk;
use petname::Generator;
use regex::Regex;

use crate::command::CommandRunner;

#[derive(Parser)]
#[command(name = "notes", about = "note-taking utility")]
pub struct Notes {
    #[command(subcommand)]
    command: Option<NoteCommand>,
}

#[derive(Parser)]
enum NoteCommand {
    /// Edit today's note or a specific date
    Edit {
        /// Date for the note (supports 'yesterday', 'last monday', '2025-01-15', etc.)
        #[arg(short, long)]
        date: Option<String>,
    },

    /// List recent notes
    List {
        /// Number of notes to show
        #[arg(default_value = "10")]
        count: usize,
    },

    /// Search notes for a query string
    Search {
        /// Search query
        query: String,

        /// Search notes before this date (supports 'yesterday', '2025-01-15', etc.)
        #[arg(short = 'B', long)]
        before: Option<String>,

        /// Search notes after this date (supports 'yesterday', '2025-01-15', etc.)
        #[arg(short = 'A', long)]
        after: Option<String>,
    },

    /// Create a temporary note with a random petname
    #[command(alias = "t")]
    Tmp,

    /// Show configuration (editor, notes directory, etc.)
    Config,
}

impl CommandRunner for Notes {
    fn run(self) -> anyhow::Result<()> {
        match self.command {
            None => {
                let notes_dir = get_notes_dir()?;
                let date = chrono::Local::now().date_naive();
                let path = build_note_path(&notes_dir, &date);
                edit_note(&notes_dir, &path)?;
            }

            Some(NoteCommand::Edit { date }) => {
                let parsed_date = if let Some(date_str) = date {
                    parse_date(&date_str)?
                } else {
                    chrono::Local::now().date_naive()
                };

                let notes_dir = get_notes_dir()?;
                let path = build_note_path(&notes_dir, &parsed_date);
                edit_note(&notes_dir, &path)?;
            }

            Some(NoteCommand::List { count }) => {
                list_notes(count)?;
            }

            Some(NoteCommand::Search {
                query,
                before,
                after,
            }) => {
                let before_date = if let Some(date_str) = before {
                    Some(parse_date(&date_str)?)
                } else {
                    None
                };
                let after_date = if let Some(date_str) = after {
                    Some(parse_date(&date_str)?)
                } else {
                    None
                };
                search_notes(&query, before_date, after_date)?;
            }

            Some(NoteCommand::Tmp) => {
                create_temp_note()?;
            }

            Some(NoteCommand::Config) => {
                show_config()?;
            }
        }

        Ok(())
    }
}

fn get_notes_dir() -> anyhow::Result<PathBuf> {
    let path = if let Ok(dir) = std::env::var("NOTES_DIR") {
        if let Some(rest) = dir.strip_prefix("~/") {
            let home = std::env::var("HOME").context("HOME environment variable not set")?;
            PathBuf::from(home).join(rest)
        } else if dir == "~" {
            let home = std::env::var("HOME").context("HOME environment variable not set")?;
            PathBuf::from(home)
        } else {
            PathBuf::from(dir)
        }
    } else {
        let xdg_dirs = xdg::BaseDirectories::new();
        xdg_dirs
            .get_data_home()
            .context("Failed to determine XDG data home directory")?
            .join("notes")
    };

    if !path.exists() {
        std::fs::create_dir_all(&path)
            .with_context(|| format!("Failed to create notes directory: {}", path.display()))?;
    }

    Ok(path)
}

fn get_editor() -> anyhow::Result<String> {
    std::env::var("EDITOR")
        .context("EDITOR environment variable not set. Please set it to your preferred editor (e.g., export EDITOR=vim)")
}

fn parse_date(date_str: &str) -> anyhow::Result<chrono::NaiveDate> {
    if let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        return Ok(date);
    }

    let parsed = dateparser::parse(date_str)
        .with_context(|| format!("Failed to parse date: '{}'. Try formats like 'yesterday', 'last monday', '2025-01-15', or 'Jan 15'", date_str))?;

    Ok(parsed.with_timezone(&chrono::Local).date_naive())
}

fn build_note_path(base_dir: &Path, date: &chrono::NaiveDate) -> PathBuf {
    base_dir
        .join(format!("{:04}", date.year()))
        .join(format!("{:02}", date.month()))
        .join(format!("{:02}.md", date.day()))
}

fn ensure_parent_dirs(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create directory structure: {}", parent.display())
        })?;
    }
    Ok(())
}

fn edit_note(notes_dir: &PathBuf, path: &PathBuf) -> anyhow::Result<()> {
    let editor = get_editor()?;

    ensure_parent_dirs(path)?;

    if !path.exists() {
        let mut notes = collect_notes(notes_dir).context("Failed to find existing notes")?;
        notes.sort_by(|a, b| b.cmp(a));

        let latest_note = notes
            .first()
            .context("No existing notes found to copy from")?;

        std::fs::copy(&latest_note.path, path).with_context(|| {
            format!(
                "Failed to copy latest note from {}",
                latest_note.path.display()
            )
        })?;
    }

    let status = std::process::Command::new(&editor)
        .arg(path)
        .status()
        .with_context(|| format!("Failed to launch editor: {}", editor))?;

    if !status.success() {
        anyhow::bail!("Editor exited with non-zero status: {}", status);
    }

    Ok(())
}

fn list_notes(count: usize) -> anyhow::Result<()> {
    let notes_dir = get_notes_dir()?;
    let mut notes = collect_notes(&notes_dir)?;

    if notes.is_empty() {
        return Ok(());
    }

    notes.sort_by(|a, b| b.cmp(a));

    for note in notes.iter().take(count) {
        println!(
            "{:04}-{:02}-{:02}  {}",
            note.year,
            note.month,
            note.day,
            note.path.display()
        );
    }

    Ok(())
}

fn search_notes(
    query: &str,
    before: Option<chrono::NaiveDate>,
    after: Option<chrono::NaiveDate>,
) -> anyhow::Result<()> {
    let notes_dir = get_notes_dir()?;
    let notes = collect_notes(&notes_dir)?;

    let query_lower = query.to_lowercase();

    for note in notes {
        let note_date =
            chrono::NaiveDate::from_ymd_opt(note.year as i32, note.month as u32, note.day as u32)
                .expect("invalid date in note");

        if let Some(before_date) = before {
            if note_date > before_date {
                continue;
            }
        }

        if let Some(after_date) = after {
            if note_date < after_date {
                continue;
            }
        }

        let content = std::fs::read_to_string(&note.path)
            .with_context(|| format!("Failed to read note: {}", note.path.display()))?;

        if content.to_lowercase().contains(&query_lower) {
            println!(
                "{:04}-{:02}-{:02}  {}",
                note.year,
                note.month,
                note.day,
                note.path.display()
            );

            for line in content.lines() {
                if line.to_lowercase().contains(&query_lower) {
                    println!("  > {}", line.trim());
                }
            }
            println!();
        }
    }

    Ok(())
}

fn create_temp_note() -> anyhow::Result<()> {
    let notes_dir = get_notes_dir()?;

    let name = petname::Petnames::default()
        .generate_one(2, "-")
        .expect("couldn't generate petname");

    let path = notes_dir.join(format!("{}.md", name));

    println!("Creating temporary note: {}", path.display());

    let editor = get_editor()?;
    let status = std::process::Command::new(&editor)
        .arg(&path)
        .status()
        .with_context(|| format!("Failed to launch editor: {}", editor))?;

    if !status.success() {
        anyhow::bail!("Editor exited with non-zero status: {}", status);
    }

    Ok(())
}

fn show_config() -> anyhow::Result<()> {
    let notes_dir = get_notes_dir()?;
    let notes_dir_source = if std::env::var("NOTES_DIR").is_ok() {
        "NOTES_DIR"
    } else {
        "XDG (default)"
    };

    println!("Configuration:");
    println!(
        "  Notes directory: {} (from {})",
        notes_dir.display(),
        notes_dir_source
    );
    println!("    To override: export NOTES_DIR=/path/to/notes");
    println!();

    match get_editor() {
        Ok(editor) => println!("  Editor: {}", editor),
        Err(_) => println!("  Editor: not set"),
    }
    println!("    To override: export EDITOR=vim");
    println!();

    let note_count = collect_notes(&notes_dir)
        .map(|notes| notes.len())
        .unwrap_or(0);
    println!("  Total notes: {}", note_count);

    Ok(())
}

fn collect_notes(base_dir: &PathBuf) -> anyhow::Result<Vec<Note>> {
    let nested_re = Regex::new(r"(?<year>[0-9]{4})/(?<month>[0-9]{2})/(?<day>[0-9]{2})\.md$")
        .expect("Invalid regex");

    let mut notes = Vec::new();

    for entry in Walk::new(base_dir) {
        let entry = entry?;
        let path = entry.path();

        if !path.is_file() {
            continue;
        }

        if let Some(path_str) = path.to_str() {
            if let Some(caps) = nested_re.captures(path_str) {
                let year = caps.name("year").unwrap().as_str().parse().unwrap();
                let month = caps.name("month").unwrap().as_str().parse().unwrap();
                let day = caps.name("day").unwrap().as_str().parse().unwrap();

                notes.push(Note {
                    year,
                    month,
                    day,
                    path: path.to_owned(),
                });
            }
        }
    }

    Ok(notes)
}

#[derive(Eq, PartialOrd, Ord, Clone, Debug)]
struct Note {
    year: u16,
    month: u8,
    day: u8,
    path: PathBuf,
}

impl PartialEq for Note {
    fn eq(&self, other: &Self) -> bool {
        self.year == other.year && self.month == other.month && self.day == other.day
    }
}

impl std::hash::Hash for Note {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.year.hash(state);
        self.month.hash(state);
        self.day.hash(state);
    }
}
