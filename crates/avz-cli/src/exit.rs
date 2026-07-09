//! The one place process exit codes are decided (`VISION.md` §8).
//!
//! `avz-core` reports *what* went wrong as a typed error; this module decides
//! *how the shell hears about it*. Keeping the mapping in one function means a
//! new core error variant has exactly one place to be classified.

use avz_core::Error;

/// Process exit codes. Values are contractual — scripts depend on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// Success.
    Ok = 0,
    /// Bad arguments or configuration.
    Usage = 2,
    /// The input file is missing, unreadable, or the wrong format.
    Input = 3,
    /// Rendering or encoding failed.
    Failure = 4,
}

impl From<Exit> for std::process::ExitCode {
    fn from(exit: Exit) -> Self {
        std::process::ExitCode::from(exit as u8)
    }
}

/// Classify a CLI-level error into an exit code.
///
/// Walks the `anyhow` context chain so an error keeps its classification even
/// after the CLI layer wraps it with human-facing context. Errors that did not
/// originate in `avz-core` are treated as pipeline failures.
pub fn code_for(err: &anyhow::Error) -> Exit {
    let Some(core) = err.chain().find_map(|cause| cause.downcast_ref::<Error>()) else {
        return Exit::Failure;
    };

    match core {
        Error::Config(_) => Exit::Usage,
        Error::Input(_) => Exit::Input,
        Error::NotImplemented { .. } | Error::Analysis(_) | Error::Render(_) | Error::Encode(_) => {
            Exit::Failure
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_errors_are_usage_errors() {
        let err = anyhow::Error::new(Error::Config("unknown key `fpss`".into()));
        assert_eq!(code_for(&err), Exit::Usage);
    }

    #[test]
    fn input_errors_get_their_own_code() {
        let err = anyhow::Error::new(Error::Input("song.mp3: no such file".into()));
        assert_eq!(code_for(&err), Exit::Input);
    }

    #[test]
    fn not_implemented_is_a_failure() {
        let err = anyhow::Error::new(Error::NotImplemented { command: "render" });
        assert_eq!(code_for(&err), Exit::Failure);
    }

    #[test]
    fn classification_survives_added_context() {
        let err = anyhow::Error::new(Error::Input("song.mp3: no such file".into()))
            .context("while probing song.mp3");
        assert_eq!(code_for(&err), Exit::Input);
    }

    #[test]
    fn errors_from_outside_core_are_failures() {
        let err = anyhow::anyhow!("something unexpected");
        assert_eq!(code_for(&err), Exit::Failure);
    }

    #[test]
    fn exit_codes_match_the_documented_contract() {
        assert_eq!(Exit::Ok as u8, 0);
        assert_eq!(Exit::Usage as u8, 2);
        assert_eq!(Exit::Input as u8, 3);
        assert_eq!(Exit::Failure as u8, 4);
    }
}
