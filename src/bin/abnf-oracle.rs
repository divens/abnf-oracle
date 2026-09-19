//! Command-line interface (feature `cli`).
//!
//! Opt-in: the library target has no required dependencies, and `clap` and `anyhow` are pulled
//! in only by this binary (SCOPE.md 10, D13).

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

/// Parse, check, match against and generate from ABNF grammars.
#[derive(Parser)]
#[command(name = "abnf-oracle", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse, check and lint a grammar.
    Check {
        /// The grammar file.
        grammar: std::path::PathBuf,
        /// Entry points; naming them enables unreachable-rule warnings.
        #[arg(long = "start")]
        starts: Vec<String>,
    },
    /// Decide whether input matches a rule.
    Match {
        /// The grammar file.
        grammar: std::path::PathBuf,
        /// The start rule.
        #[arg(long)]
        rule: String,
    },
    /// Generate strings that match a rule.
    Gen {
        /// The grammar file.
        grammar: std::path::PathBuf,
        /// The start rule.
        #[arg(long)]
        rule: String,
    },
    /// List the rules of a grammar with their analyses.
    Rules {
        /// The grammar file.
        grammar: std::path::PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Check { .. } => bail!("`check` lands in M1.9"),
        Command::Match { .. } => bail!("`match` lands in M2.7"),
        Command::Gen { .. } => bail!("`gen` lands in M3.4"),
        Command::Rules { .. } => bail!("`rules` lands in M1.9"),
    }
}
