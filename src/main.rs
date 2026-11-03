//! `ht`: the HerdR Terminal command line. With no subcommand it opens the
//! workspace; `screen` is the M2 single-pane screen and `probe` the raw pipe.

use herdr_terminal::core::{Error, Result};
use herdr_terminal::{probe, screen, workspace};
use std::ffi::OsString;
use std::iter::once;
use std::path::PathBuf;

const HELP: &str = "\
ht — the terminal client for Herdr Core

usage:
  ht [options] [--] [PROGRAM [ARGS...]]
  ht screen [options] [--] [PROGRAM [ARGS...]]
  ht probe [options] [--] [PROGRAM [ARGS...]]
  ht --version
  ht --help

`ht` opens a workspace: a tab bar, a status bar and panes of terminals. It
restores the spaces, tabs, panes and programs saved in core's `herdr-terminal`
document; a new workspace starts a shell in the first pane. A pane the event
snapshot has not reported yet is checked with `terminal.get`, so a live pane is
not mistaken for a missing one. If another client deleted the document while
programs were running, their labels rebuild it and no new shell is started. An
explicit PROGRAM runs in a new tab, so restored panes are never replaced.
Prefix Ctrl-b, then:
  c new tab   C new space   n/p tab   N/P space   1..9 select tab
  v split left/right   s split top/bottom   h/j/k/l focus   H/J/K/L move
  z zoom   x close   e keep and close   X kill and close
  , rename tab   . rename space   ? help
  a agents sidebar   i peek agent   g/G next/prev agent
  : command palette   f search scrollback   [ select mode   y copy
  Shift-PgUp/PgDn scroll history   Shift-Home/End top/bottom
The agents sidebar lists every agent in the workspace (a terminal counts only
when core says it reported through OSC 7501). Pinned agents sort first, then
blocked, done, working and idle. In the sidebar, j/k move, g/G jump to the
first/last row, Enter jumps to the agent, i peeks it, p pins or unpins it, u
marks it unread; Esc or a closes. Prefix i peeks the focused agent and any key
closes the peek. Seen and pinned state are core labels, not part of the saved
document.
Mouse: a click focuses a pane or selects a tab; a click without a drag only
focuses and never enters selection mode. Dragging on a pane grid selects
locally, Shift-drag selects even over a program that tracks the mouse, and
dragging a split gap resizes. The wheel scrolls local history unless the
program tracks the mouse. Chrome never reaches a program. Scrollback and
search are local: they never read core or change the document. A selection is
copied with y (or Enter) and cancelled with Esc; any other key leaves selection
and goes to the focused program, and a paste is ignored while selecting. Copy
uses OSC 52 by default, optionally a native helper.
Settings are read once at startup from
`$XDG_CONFIG_HOME/herdr-terminal/config.json` (falling back to
`$HOME/.config/herdr-terminal/config.json`): `theme` (default, dark, light),
`scrollback` lines, `mouse` on/off, `clipboard` (osc52, native, both, off) and
`keys` overrides for prefix bindings. A missing file is normal; a bad file
warns and falls back. Ctrl-] (or prefix q) detaches: every program keeps
running and the document stays. Focus, zoom and the active tab are local per
screen, named by HERDR_TERMINAL_SCREEN (default `default`). It needs a
terminal on stdin and stdout.

`ht screen` is the single-pane screen. It draws one terminal's cells, colors,
cursor and wide characters; keys and the mouse go to it, including Ctrl-C.
Without PROGRAM it reattaches to the terminal a previous client left running,
or starts a shell. Ctrl-] detaches.

`ht probe` copies the program's raw bytes to stdout; Ctrl-C quits.

options (all commands):
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
        return workspace::run(workspace::Options::default());
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
        "screen" => screen_command(command_options("screen", args)?),
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
        // Options without a subcommand open the workspace: `ht --cols 100`.
        _ if command.starts_with('-') => {
            let options = command_options("ht", once(OsString::from(command)).chain(args))?;
            workspace_command(options)
        }
        other => Err(Error::Usage(format!("unknown command {other:?}\n\n{HELP}"))),
    }
}

fn workspace_command(options: Option<CommandOptions>) -> Result<i32> {
    match options {
        None => {
            print!("{HELP}");
            Ok(0)
        }
        Some(options) => workspace::run(workspace::Options {
            cols: options.cols,
            rows: options.rows,
            hc: options.hc,
            argv: options.argv,
            verbose: options.verbose,
        }),
    }
}

fn screen_command(options: Option<CommandOptions>) -> Result<i32> {
    match options {
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
    }
}

/// Options shared by the workspace, `screen` and `probe`.
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
