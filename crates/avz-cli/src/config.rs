//! `avz config` — the TOML template, and nothing else yet.
//!
//! The template itself is built in `avz-core`, from the resolved defaults
//! (`config::example`), because that is where the defaults live and where a
//! meta-test can hold it to them. This module only decides where the text goes:
//! stdout, so `avz config --example > avz.toml` works (`VISION.md` §5.5).

use avz_core::Error;

use crate::cli::ConfigArgs;

/// What `avz config` does when it is not asked to do anything.
///
/// Not `--help`: a subcommand that silently succeeds having printed nothing is
/// worse than one that says which flag it wanted, and this way the shell hears
/// about it as a usage error (exit 2).
fn nothing_to_do() -> Error {
    Error::Config(
        "`avz config` needs something to do; pass `--example` to print a documented \
         config file: `avz config --example > avz.toml`"
            .to_owned(),
    )
}

/// Print the documented example config, or say why nothing was printed.
pub fn run(args: &ConfigArgs) -> anyhow::Result<()> {
    if !args.example {
        return Err(nothing_to_do().into());
    }

    // `print!`, not `tracing`: this is the command's output, redirected into a
    // file by the very invocation `VISION.md` §5.5 documents. Logs go to stderr.
    print!("{}", avz_core::config::example());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flag the error names is the flag that works.
    #[test]
    fn a_config_command_with_no_flags_is_a_usage_error_naming_the_flag() {
        let err = run(&ConfigArgs { example: false }).expect_err("nothing was asked for");

        assert!(err.to_string().contains("--example"), "{err}");
        assert_eq!(
            crate::exit::code_for(&err),
            crate::exit::Exit::Usage,
            "a missing flag is the user's argument, not a render failure",
        );
    }
}
