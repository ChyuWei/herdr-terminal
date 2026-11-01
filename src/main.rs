//! `ht`: the HerdR Terminal command line. M0 has only `probe`.

use herdr_terminal::core::{Error, Result};
use herdr_terminal::probe;
use std::ffi::OsString;
use std::path::PathBuf;

const HELP: &str = "\
ht — the terminal client for Herdr Core

usage:
  ht probe [options] [--] [PROGRAM [ARGS...]]
  ht --version
  ht --help

M0 has only `probe`: it connects to core, runs a shell (or PROGRAM), attaches,
and copies raw bytes to stdout. Ctrl-C exits. A program that exits before the
attach lands still has its final screen and exit status printed.

probe options:
  --hc PATH    herdr-core binary to use (default: HERDR_TERMINAL_HC, PATH, ...)
  --cols N     attach width in cells (default: the terminal's, else 80)
  --rows N     attach height in cells (default: the terminal's, else 24)
  -v, --verbose  log events and connection details to stderr
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
        "probe" => match probe_options(args)? {
            None => {
                print!("{HELP}");
                Ok(0)
            }
            Some(options) => probe::run(options),
        },
        other => Err(Error::Usage(format!("unknown command {other:?}\n\n{HELP}"))),
    }
}

/// `None` means `--help` was asked for.
fn probe_options(mut args: impl Iterator<Item = OsString>) -> Result<Option<probe::Options>> {
    let mut options = probe::Options::default();
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
            "--hc" => options.hc = Some(PathBuf::from(value(&mut args, "--hc")?)),
            "--cols" => options.cols = Some(dimension(&mut args, "--cols")?),
            "--rows" => options.rows = Some(dimension(&mut args, "--rows")?),
            _ if argument.starts_with('-') && argument != "-" => {
                return Err(Error::Usage(format!(
                    "probe: unknown option {argument:?} (try ht --help)"
                )));
            }
            _ => options.argv.push(argument),
        }
    }
    Ok(Some(options))
}

fn value(args: &mut impl Iterator<Item = OsString>, option: &str) -> Result<String> {
    let value = args
        .next()
        .ok_or_else(|| Error::Usage(format!("probe: {option} needs a value")))?;
    text(value)
}

fn dimension(args: &mut impl Iterator<Item = OsString>, option: &str) -> Result<u16> {
    let value = value(args, option)?;
    match value.parse::<u16>() {
        Ok(size) if size > 0 => Ok(size),
        _ => Err(Error::Usage(format!(
            "probe: {option} needs a number from 1 to 65535, not {value:?}"
        ))),
    }
}

fn text(argument: OsString) -> Result<String> {
    argument
        .into_string()
        .map_err(|_| Error::Usage("arguments must be UTF-8".into()))
}
