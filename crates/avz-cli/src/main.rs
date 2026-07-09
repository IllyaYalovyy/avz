//! `avz` — abstract music video generator.
//!
//! This binary is the only place in the project that talks to a terminal. It
//! parses arguments, sets up tracing, calls into `avz-core`, and turns typed
//! core errors into the exit codes documented in `VISION.md` §8.

#![forbid(unsafe_code)]

mod cli;
mod exit;

use std::process::ExitCode;

use clap::Parser as _;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};
use crate::exit::Exit;

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

    init_tracing(cli.verbose, cli.quiet);

    match run(&cli) {
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
fn init_tracing(verbose: bool, quiet: bool) {
    let default_level = match (verbose, quiet) {
        (true, _) => "debug",
        (_, true) => "error",
        _ => "info",
    };

    let filter =
        EnvFilter::try_from_env("AVZ_LOG").unwrap_or_else(|_| EnvFilter::new(default_level));

    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

fn run(cli: &Cli) -> anyhow::Result<()> {
    match &cli.command {
        Command::Render(args) => {
            tracing::debug!(input = ?args.input, out = ?args.out, "render requested");

            // Before analysis, before the GPU, before a single frame: a render
            // that cannot be encoded should fail in the first second, not the
            // last one (VISION.md §5.4). Step 8 hands the verified binary to the
            // encoder; for now the check itself is the value.
            avz_core::encode::preflight(avz_core::encode::DEFAULT_PROGRAM)?;
        }
        Command::Probe(args) => tracing::debug!(input = ?args.input, "probe requested"),
        Command::Presets(args) => tracing::debug!(name = ?args.name, "presets requested"),
        Command::Config(args) => tracing::debug!(example = args.example, "config requested"),
    }

    Err(avz_core::Error::NotImplemented {
        command: cli.command.name(),
    }
    .into())
}
