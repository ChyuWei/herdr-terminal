//! `ht`: the HerdR Terminal command line. M0 has `probe`; M1 adds `screen`.

use herdr_terminal::core::{Error, Result};
use herdr_terminal::{probe, screen};
use std::ffi::OsString;
use std::path::PathBuf;

const HELP: &str = "\
ht — the terminal client for Herdr Core

usage:
  ht screen [options] [--] [PROGRAM [ARGS...]]
  ht probe [options] [--] [PROGRAM [ARGS...]]
  ht --version
  ht --help

`ht screen` is one full-screen terminal rendered with ratatui. It runs a shell
(or PROGRAM), attaches to it, and draws its cells, colors, cursor and wide
characters. Keys and the mouse go to the program, including Ctrl-C. Ctrl-]
leaves the screen: it detaches, ends the program, and removes its record.
It needs a terminal on stdin and stdout.

`ht probe` copies the program's raw bytes to stdout; Ctrl-C quits.

options (both commands):
  --hc PATH    herdr-core binary to use (default: HERDR_TERMINAL_HC, PATH, ...)
  --cols N     attach width in cells (default: the terminal's, else 80)
  --rows N     attach height in cells (default: the terminal's, else 24)
  -v, --verbose  log connection details to stderr
";

fn main() {
    match run(std::env::args_os().skip(1).collect()) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("ht: {error}");
            let code = if matches!(error, Error::Usage(_)) {
                2
            } else {
                1
            };
            std::process::exit(code);
        }
    }
}

fn run(args: Vec<OsString>) -> Result<i32> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        print!("{HELP}");
        return Ok(0);
    };
    let command = text(command)?;
    match command.as_str() {
        "-V" | "--version" => {
            println!("ht {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        "-h" | "--help" | "help" => {
            print!("{HELP}");
            Ok(0)
        }
        "screen" => match command_options("screen", args)? {
            None => {
                print!("{HELP}");
                Ok(0)
            }
            Some(options) => screen::run(screen::Options {
                cols: options.cols,
                rows: options.rows,
                hc: options.hc,
                argv: options.argv,
                verbose: options.verbose,
            }),
        },
        "probe" => match command_options("probe", args)? {
            None => {
                print!("{HELP}");
                Ok(0)
            }
            Some(options) => probe::run(probe::Options {
                cols: options.cols,
                rows: options.rows,
                hc: options.hc,
                argv: options.argv,
                verbose: options.verbose,
            }),
        },
        other => Err(Error::Usage(format!("unknown command {other:?}\n\n{HELP}"))),
    }
}

/// Options shared by `screen` and `probe`.
struct CommandOptions {
    cols: Option<u16>,
    rows: Option<u16>,
    hc: Option<PathBuf>,
    argv: Vec<String>,
    verbose: bool,
}

/// Parses the shared options. `None` means `--help` was asked for.
fn command_options(
    command: &str,
    mut args: impl Iterator<Item = OsString>,
) -> Result<Option<CommandOptions>> {
    let mut options = CommandOptions {
        cols: None,
        rows: None,
        hc: None,
        argv: Vec::new(),
        verbose: false,
    };
    while let Some(argument) = args.next() {
        if !options.argv.is_empty() {
            options.argv.push(text(argument)?);
            continue;
        }
        let argument = text(argument)?;
        match argument.as_str() {
            "--" => {
                for argument in args {
                    options.argv.push(text(argument)?);
                }
                break;
            }
            "-h" | "--help" => return Ok(None),
            "-v" | "--verbose" => options.verbose = true,
            "--hc" => {
                options.hc = Some(PathBuf::from(value(command, &mut args, "--hc")?));
            }
            "--cols" => {
                options.cols = Some(dimension(command, &mut args, "--cols")?);
            }
            "--rows" => {
                options.rows = Some(dimension(command, &mut args, "--rows")?);
            }
            _ if argument.starts_with('-') && argument != "-" => {
                return Err(Error::Usage(format!(
                    "{command}: unknown option {argument:?} (try ht --help)"
                )));
            }
            _ => options.argv.push(argument),
        }
    }
    Ok(Some(options))
}

fn value(command: &str, args: &mut impl Iterator<Item = OsString>, option: &str) -> Result<String> {
    let value = args
        .next()
        .ok_or_else(|| Error::Usage(format!("{command}: {option} needs a value")))?;
    text(value)
}

fn dimension(
    command: &str,
    args: &mut impl Iterator<Item = OsString>,
    option: &str,
) -> Result<u16> {
    let value = value(command, args, option)?;
    match value.parse::<u16>() {
        Ok(size) if size > 0 => Ok(size),
        _ => Err(Error::Usage(format!(
            "{command}: {option} needs a number from 1 to 65535, not {value:?}"
        ))),
    }
}

fn text(argument: OsString) -> Result<String> {
    argument
        .into_string()
        .map_err(|_| Error::Usage("arguments must be UTF-8".into()))
}
