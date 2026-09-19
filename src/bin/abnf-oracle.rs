//! Command-line interface (feature `cli`).
//!
//! Opt-in: the library target has no required dependencies, and `clap` and `anyhow` are pulled
//! in only by this binary (SCOPE.md 10, D13).
//!
//! Exit codes follow SCOPE.md 11. `0` means the question was answered, `1` that the answer was
//! "no", and `2` that it could not be answered at all — a grammar that will not parse or check,
//! an unknown rule, a compatibility limit, a resource limit. A limit is never reported as a
//! rejection: "does not match" and "could not decide" are different answers, and conflating
//! them is what would make this useless as an oracle.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use abnf_oracle::{CheckedGrammar, Grammar, MinLen};
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

/// The question was answered.
const OK: u8 = 0;
/// The question could not be answered.
const ERROR: u8 = 2;

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
        grammar: PathBuf,
        /// Entry points; naming them enables unreachable-rule warnings.
        #[arg(long = "start", value_name = "RULE")]
        starts: Vec<String>,
    },
    /// Decide whether input matches a rule.
    Match {
        /// The grammar file.
        grammar: PathBuf,
        /// The start rule.
        #[arg(long)]
        rule: String,
    },
    /// Generate strings that match a rule.
    Gen {
        /// The grammar file.
        grammar: PathBuf,
        /// The start rule.
        #[arg(long)]
        rule: String,
    },
    /// List the rules of a grammar with their analyses.
    Rules {
        /// The grammar file.
        grammar: PathBuf,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(ERROR)
        }
    }
}

fn run() -> Result<u8> {
    match Cli::parse().command {
        Command::Check { grammar, starts } => check(&grammar, &starts),
        Command::Rules { grammar } => rules(&grammar),
        Command::Match { .. } => bail!("`match` lands in M2.7"),
        Command::Gen { .. } => bail!("`gen` lands in M3.4"),
    }
}

/// Parses and checks, reporting diagnostics against the source text.
///
/// `None` means the grammar cannot be used at all, and the caller should exit with [`ERROR`].
fn load(path: &Path) -> Result<Option<(String, CheckedGrammar)>> {
    let src = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;

    let grammar = match Grammar::parse(&src) {
        Ok(grammar) => grammar,
        Err(error) => {
            eprintln!("{}", error.render(&src));
            return Ok(None);
        }
    };

    match grammar.check() {
        Ok(checked) => Ok(Some((src, checked))),
        Err(errors) => {
            // Every error, not the first: a grammar with four undefined references should take
            // one round of fixing, not four.
            for error in &errors {
                eprintln!("{}", error.render(&src));
            }
            eprintln!(
                "{} structural {}",
                errors.len(),
                plural(errors.len(), "error", "errors")
            );
            Ok(None)
        }
    }
}

fn check(path: &Path, starts: &[String]) -> Result<u8> {
    let Some((_, grammar)) = load(path)? else {
        return Ok(ERROR);
    };

    let warnings = if starts.is_empty() {
        grammar.lint()
    } else {
        let starts: Vec<&str> = starts.iter().map(String::as_str).collect();
        grammar.lint_from(&starts)
    };
    for warning in &warnings {
        println!("warning: {warning}");
    }

    // Warnings are advisory and never change the exit code (D5). The entry rule of any grammar
    // is reported unreferenced, so failing on warnings would fail on every grammar.
    println!(
        "{} {}, {} {}",
        grammar.rules().len(),
        plural(grammar.rules().len(), "rule", "rules"),
        warnings.len(),
        plural(warnings.len(), "warning", "warnings")
    );
    if starts.is_empty() && !warnings.is_empty() {
        println!("note: pass --start to name entry points and enable unreachable-rule warnings");
    }
    Ok(OK)
}

fn rules(path: &Path) -> Result<u8> {
    let Some((_, grammar)) = load(path)? else {
        return Ok(ERROR);
    };

    let width = grammar
        .rules()
        .iter()
        .map(|rule| rule.name.as_str().len())
        .max()
        .unwrap_or(4)
        .max(4);

    println!(
        "{:<width$}  {:<8}  {:>7}  notes",
        "rule", "nullable", "min-len"
    );
    for rule in grammar.rules() {
        let name = rule.name.as_str();
        let nullable = if grammar.is_nullable(rule.body) {
            "yes"
        } else {
            "no"
        };
        let min_len = match grammar.min_len(rule.body) {
            MinLen::Finite(length) => length.to_string(),
            // Not a number: the rule matches nothing at all.
            MinLen::Infinite => "-".to_owned(),
        };

        let mut notes = Vec::new();
        if grammar
            .core_rules()
            .iter()
            .any(|core| core.name == rule.name)
        {
            notes.push("shadows a core rule".to_owned());
        }
        if !grammar.min_len(rule.body).is_finite() {
            notes.push("matches nothing".to_owned());
        }
        if let Err(why) = grammar.can_recognize(name) {
            notes.push(format!("not a usable start rule: {why}"));
        }

        let row = format!(
            "{name:<width$}  {nullable:<8}  {min_len:>7}  {}",
            notes.join("; ")
        );
        println!("{}", row.trim_end());
    }
    Ok(OK)
}

fn plural(count: usize, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}
