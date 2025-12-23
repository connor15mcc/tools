use crate::command::CommandRunner;
use clap::Parser;
use clap_stdin::FileOrStdin;
use crossterm::{
    cursor,
    terminal::{self, ClearType},
    ExecutableCommand,
};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, IsTerminal, Read, Write};

#[derive(Parser)]
#[command(
    name = "ilimit",
    about = "Interactively tail a limited number of lines"
)]
pub struct Ilimit {
    /// Number of lines to display from the end of input
    #[arg(long, default_value_t = 10)]
    limit: usize,

    /// File from which to read, defaulting to stdin
    #[clap(default_value = "-")]
    input: FileOrStdin,
}

impl CommandRunner for Ilimit {
    fn run(self) -> anyhow::Result<()> {
        let reader = self
            .input
            .clone()
            .into_reader()
            .expect("failed to convert to reader");
        interactive_tail(reader, self.limit)?;
        Ok(())
    }
}

fn interactive_tail<R: Read>(reader: R, limit: usize) -> std::io::Result<()> {
    let mut buffer = VecDeque::with_capacity(limit);
    let buf_reader = BufReader::new(reader);
    let mut stdout = std::io::stdout();

    // Check if stdout is a terminal to determine if we can use cursor operations
    let is_terminal = stdout.is_terminal();

    // Setup Ctrl+C handler that exits immediately
    // Note: We don't do cursor cleanup in the handler as it's not signal-safe
    ctrlc::set_handler(|| {
        std::process::exit(130); // Exit code 128 + SIGINT(2) = 130
    })
    .map_err(|e| {
        std::io::Error::other(
            format!("Failed to set Ctrl-C handler: {}", e),
        )
    })?;

    if !is_terminal {
        // Non-terminal mode: just print continuously without cursor manipulation
        for line in buf_reader.lines() {
            let line = line?;
            if buffer.len() >= limit {
                buffer.pop_front();
            }
            buffer.push_back(line);
        }

        // Print final buffer
        for line in buffer {
            println!("{}", line);
        }

        return Ok(());
    }

    // Terminal mode: use cursor operations for interactive display
    let mut lines_displayed = 0;

    for line in buf_reader.lines() {
        let line = line?;

        // Update buffer
        if buffer.len() >= limit {
            buffer.pop_front();
        }
        buffer.push_back(line);

        // Move cursor up to start of our display area (if we've already drawn)
        if lines_displayed > 0 {
            stdout.execute(cursor::MoveUp(lines_displayed as u16))?;
        }

        // Clear and redraw each line in buffer
        for buffered_line in &buffer {
            stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
            println!("{}", buffered_line);
        }
        stdout.flush()?;

        lines_displayed = buffer.len();
    }

    Ok(())
}
