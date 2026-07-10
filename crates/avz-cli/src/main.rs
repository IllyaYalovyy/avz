//! `avz` — abstract music video generator.
//!
//! This binary is the only place in the project that talks to a terminal. It
//! parses arguments, sets up tracing, calls into `avz-core`, and turns typed
//! core errors into the exit codes documented in `VISION.md` §8.

#![forbid(unsafe_code)]

mod cli;
mod exit;
mod presets;
mod probe;
mod progress;
mod render;

use std::process::ExitCode;

use clap::Parser as _;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};
use crate::exit::Exit;
use crate::progress::{LogWriter, Ui};

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            // `--help` and `--version` are requests, not failures: clap prints
            // them to stdout and we exit 0. Everything else is a usage error.
            let code = if err.use_stderr() {
                Exit::Usage
            } else {
                Exit::Ok
            };
            let _ = err.print();
            return code.into();
        }
    };

    // Built before tracing, because tracing writes *through* it: a log line that
    // did not clear the bars first would be overwritten by the next redraw.
    let ui = Ui::new(cli.quiet);
    init_tracing(cli.verbose, cli.quiet, ui.log_writer());

    match run(&cli, &ui) {
        Ok(()) => Exit::Ok.into(),
        Err(err) => {
            eprintln!("error: {err:#}");
            exit::code_for(&err).into()
        }
    }
}

/// Send `tracing` output to stderr so it never contaminates piped stdout.
///
/// `AVZ_LOG` overrides the verbosity flags for debugging.
fn init_tracing(verbose: bool, quiet: bool, writer: LogWriter) {
    let default_level = match (verbose, quiet) {
        (true, _) => "debug",
        (_, true) => "error",
        _ => "info",
    };

    let filter =
        EnvFilter::try_from_env("AVZ_LOG").unwrap_or_else(|_| EnvFilter::new(default_level));

    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

fn run(cli: &Cli, ui: &Ui) -> anyhow::Result<()> {
    match &cli.command {
        Command::Render(args) => {
            tracing::debug!(
                input = ?args.input,
                out = ?args.out,
                sample = ?args.sample,
                adapter = %args.adapter,
                "render requested"
            );
            render::run(args, ui)
        }
        Command::Probe(args) => {
            tracing::debug!(input = ?args.input, "probe requested");
            probe::run(args)
        }
        Command::Presets(args) => {
            tracing::debug!(name = ?args.name, "presets requested");
            presets::run(args)
        }
        Command::Config(args) => {
            tracing::debug!(example = args.example, "config requested");
            not_implemented(cli)
        }
    }
}

/// A command that parses and validates, then politely refuses (`VISION.md` §9).
fn not_implemented(cli: &Cli) -> anyhow::Result<()> {
    Err(avz_core::Error::NotImplemented {
        command: cli.command.name(),
    }
    .into())
}
