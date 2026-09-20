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

use abnf_oracle::{
    CheckedGrammar, DEFAULT_MAX_DEPTH, GenOptions, Generator, Grammar, MatchError, MatchOptions,
    MinLen, Recognizer,
};
use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

/// The question was answered, and the answer was yes.
const OK: u8 = 0;
/// The question was answered, and the answer was no.
const NO: u8 = 1;
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
        /// The input, given directly.
        #[arg(long, group = "source")]
        input: Option<String>,
        /// A file holding the input.
        #[arg(long, group = "source")]
        file: Option<PathBuf>,
        /// A directory of inputs, one per file.
        #[arg(long, group = "source")]
        dir: Option<PathBuf>,
        /// Give up after this many steps rather than running unboundedly.
        #[arg(long, value_name = "N")]
        max_steps: Option<u64>,
        /// Allow recursion this deep. Nesting in the input costs depth; length does not.
        #[arg(long, value_name = "N")]
        max_depth: Option<usize>,
    },
    /// Generate strings that match a rule.
    Gen {
        /// The grammar file.
        grammar: PathBuf,
        /// The start rule.
        #[arg(long)]
        rule: String,
        /// Seed for the generator. The same seed gives the same strings.
        #[arg(long, default_value_t = 0)]
        seed: u64,
        /// How many strings to generate.
        #[arg(long, default_value_t = 1, value_name = "N")]
        count: usize,
        /// Steer towards branches not yet taken, rather than choosing randomly.
        #[arg(long)]
        coverage: bool,
        /// How deep to recurse before falling back to the shortest known derivation.
        #[arg(long, value_name = "N")]
        depth: Option<usize>,
        /// How many repetitions beyond the minimum to consider.
        #[arg(long, value_name = "N")]
        spread: Option<usize>,
        /// Emit case-insensitive strings as written, rather than varying their case.
        #[arg(long)]
        preserve_case: bool,
        /// Give up once a string reaches this many characters.
        #[arg(long, value_name = "N")]
        max_output_len: Option<usize>,
        /// Give up after visiting this many nodes.
        #[arg(long, value_name = "N")]
        max_steps: Option<u64>,
        /// Write one file per string into this directory, rather than one per line.
        ///
        /// Generated strings may contain line endings — a grammar of grammars produces them by
        /// the handful — so a line-oriented listing cannot represent them unambiguously. This
        /// pairs with `match --dir`.
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
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
        Command::Match {
            grammar,
            rule,
            input,
            file,
            dir,
            max_steps,
            max_depth,
        } => {
            let options = MatchOptions {
                max_steps,
                // Absent means the library default, not "unlimited": a crash is not an answer
                // (SCOPE.md 6.2, D42).
                max_depth: max_depth.or(Some(DEFAULT_MAX_DEPTH)),
            };
            let source = match (input, file, dir) {
                (Some(text), None, None) => Source::Literal(text),
                (None, Some(path), None) => Source::File(path),
                (None, None, Some(path)) => Source::Dir(path),
                // clap's group enforces at most one; this is the none-at-all case.
                _ => bail!("one of --input, --file or --dir is required"),
            };
            matching(&grammar, &rule, source, &options)
        }
        Command::Gen {
            grammar,
            rule,
            seed,
            count,
            coverage,
            depth,
            spread,
            preserve_case,
            max_output_len,
            max_steps,
            out,
        } => {
            let defaults = GenOptions::default();
            let options = GenOptions {
                coverage,
                preserve_case,
                max_depth: depth.unwrap_or(defaults.max_depth),
                spread: spread.unwrap_or(defaults.spread),
                // An absent flag means the library default, which is finite. Passing 0 is how a
                // caller asks for no limit at all.
                max_output_len: max_output_len
                    .map_or(defaults.max_output_len, |n| (n > 0).then_some(n)),
                max_steps: max_steps.map_or(defaults.max_steps, |n| (n > 0).then_some(n)),
            };
            generating(&grammar, &rule, seed, count, &options, out.as_deref())
        }
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
            eprintln!("{}", errors.render(&src));
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

/// Where the input to match comes from.
enum Source {
    /// `--input`: the text itself.
    Literal(String),
    /// `--file`: one input.
    File(PathBuf),
    /// `--dir`: one input per file.
    Dir(PathBuf),
}

/// What happened to one input.
enum Verdict {
    Accept,
    Reject,
    /// Something stopped the question being answered — a decoding failure, or a limit.
    Error(String),
}

impl Verdict {
    fn label(&self) -> String {
        match self {
            Self::Accept => "ACCEPT".to_owned(),
            Self::Reject => "REJECT".to_owned(),
            Self::Error(why) => format!("ERROR {why}"),
        }
    }
}

/// Decides one input, or every input in a directory.
///
/// The exit code is the whole contract here (SCOPE.md 11): `0` accepted, `1` rejected, `2`
/// could not be decided. Across a directory an error dominates a rejection, because a run that
/// failed to answer part of the question has not answered it.
fn matching(path: &Path, rule: &str, source: Source, options: &MatchOptions) -> Result<u8> {
    let Some((_, grammar)) = load(path)? else {
        return Ok(ERROR);
    };

    // Before reading any input: a rule that reaches prose or an unrepresentable terminal is not
    // a usable start rule at all, and reporting that as "does not match" would be a lie
    // (SCOPE.md 6.5, 6.6).
    if let Err(why) = grammar.can_recognize(rule) {
        eprintln!("error: {why}");
        return Ok(ERROR);
    }

    match source {
        Source::Literal(text) => Ok(report_one(&grammar, rule, &text, options)),
        Source::File(file) => {
            let bytes = fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
            match decode(&bytes) {
                Ok(text) => Ok(report_one(&grammar, rule, &text, options)),
                Err(why) => {
                    eprintln!("error: {}: {why}", file.display());
                    Ok(ERROR)
                }
            }
        }
        Source::Dir(dir) => matching_dir(&grammar, rule, &dir, options),
    }
}

/// Decides one input and turns the verdict into an exit code.
fn report_one(grammar: &CheckedGrammar, rule: &str, text: &str, options: &MatchOptions) -> u8 {
    match decide(grammar, rule, text, options) {
        Verdict::Accept => OK,
        Verdict::Reject => NO,
        Verdict::Error(why) => {
            eprintln!("error: {why}");
            ERROR
        }
    }
}

fn decide(grammar: &CheckedGrammar, rule: &str, text: &str, options: &MatchOptions) -> Verdict {
    let mut recognizer = Recognizer::new(grammar, text).with_options(options.clone());
    match recognizer.accepts(rule) {
        Ok(true) => Verdict::Accept,
        Ok(false) => Verdict::Reject,
        // A limit is not a verdict. `DepthLimit` in particular says the input nests more deeply
        // than the recognizer was allowed to follow, which is not the same as not matching.
        Err(error @ (MatchError::StepLimit | MatchError::DepthLimit)) => {
            Verdict::Error(error.to_string())
        }
        Err(error) => Verdict::Error(error.to_string()),
    }
}

/// Decodes one input file. Invalid UTF-8 is an error, never a rejection (D14).
fn decode(bytes: &[u8]) -> Result<String, &'static str> {
    core::str::from_utf8(bytes)
        .map(ToOwned::to_owned)
        .map_err(|_| "not valid UTF-8")
}

fn matching_dir(
    grammar: &CheckedGrammar,
    rule: &str,
    dir: &Path,
    options: &MatchOptions,
) -> Result<u8> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .filter(|path| path.is_file())
        .collect();
    // Sorted, so a run over a directory is reproducible and two runs can be diffed.
    entries.sort();

    if entries.is_empty() {
        bail!("{} holds no files", dir.display());
    }

    let width = entries
        .iter()
        .map(|path| file_name(path).len())
        .max()
        .unwrap_or(0);

    let mut rejected = false;
    let mut errored = false;
    for path in &entries {
        let verdict = match fs::read(path) {
            Ok(bytes) => match decode(&bytes) {
                Ok(text) => decide(grammar, rule, &text, options),
                Err(why) => Verdict::Error(why.to_owned()),
            },
            Err(error) => Verdict::Error(error.to_string()),
        };
        match verdict {
            Verdict::Accept => {}
            Verdict::Reject => rejected = true,
            Verdict::Error(_) => errored = true,
        }
        println!("{:<width$}  {}", file_name(path), verdict.label());
    }

    // Error dominates rejection: a run that could not answer for one file has not answered.
    Ok(if errored {
        ERROR
    } else if rejected {
        NO
    } else {
        OK
    })
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("?")
        .to_owned()
}

/// Generates `count` strings from `rule`.
///
/// Exit codes follow the same rule as everything else here: `0` if the question was answered,
/// `2` if it could not be. A resource limit is the second kind — a string that could not be
/// produced is not the same as a rule that produces nothing (D29).
fn generating(
    path: &Path,
    rule: &str,
    seed: u64,
    count: usize,
    options: &GenOptions,
    out: Option<&Path>,
) -> Result<u8> {
    let Some((_, grammar)) = load(path)? else {
        return Ok(ERROR);
    };

    // Before generating anything: a rule that reaches prose or an unrepresentable terminal has
    // no expansion at all, and neither does an unproductive one (SCOPE.md 6.5, D9).
    if let Err(why) = grammar.can_generate(rule) {
        eprintln!("error: {why}");
        return Ok(ERROR);
    }

    if let Some(dir) = out {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }

    let mut generator = Generator::new(&grammar, seed).with_options(options.clone());
    for index in 0..count {
        let produced = match generator.generate(rule) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("error: string {index}: {error}");
                return Ok(ERROR);
            }
        };

        match out {
            // Named by index alone: a generated string has no description to give it, and the
            // corpus convention of SCOPE.md 9 expects the caller to rename what it keeps.
            Some(dir) => {
                let file = dir.join(format!("{index:04}.txt"));
                fs::write(&file, produced.as_bytes())
                    .with_context(|| format!("writing {}", file.display()))?;
            }
            None => println!("{produced}"),
        }
    }

    if options.coverage {
        // Worth saying, because it is the number that bounds how many more calls full coverage
        // would take.
        let left = generator.uncovered(rule).unwrap_or(0);
        eprintln!(
            "{count} {}, {left} coverage {} remaining",
            plural(count, "string", "strings"),
            plural(left, "unit", "units")
        );
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
