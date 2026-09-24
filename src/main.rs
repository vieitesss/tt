//! `tt` — CLI and TUI entry point.
//!
//! All logic lives in the `tt` library; this binary parses arguments and
//! dispatches to the CLI module (and, from M1 node 7, the TUI).

mod cli;
mod editor;
mod tui;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

/// ToTask — a keyboard-driven terminal todo graph.
#[derive(Debug, Parser)]
#[command(name = "tt", version, about)]
struct Cli {
    /// Vault directory to operate on (legacy escape hatch; skips the registry).
    #[arg(long, global = true, value_name = "PATH", hide = true)]
    vault: Option<PathBuf>,

    /// Directory to resolve the project from (default: $TT_PATH, then cwd).
    #[arg(long, global = true, value_name = "PATH")]
    path: Option<PathBuf>,

    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,

    /// Open the project picker without registering the current directory.
    #[arg(short = 'p', long)]
    projects: bool,

    #[command(subcommand)]
    command: Option<cli::Command>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    cli::run(&cli)
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn projects_flag_parses_without_a_subcommand() {
        let cli = Cli::try_parse_from(["tt", "-p"]).expect("parse bare project picker");
        assert!(cli.projects);
        assert!(cli.command.is_none());
    }

    #[test]
    fn projects_flag_does_not_parse_after_a_subcommand() {
        assert!(Cli::try_parse_from(["tt", "list", "-p"]).is_err());
    }
}
