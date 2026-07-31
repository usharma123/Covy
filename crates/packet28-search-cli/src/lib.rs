//! Process boundary for the `p28` indexed-search command.

mod app;

use std::process::ExitCode;

/// Runs `p28` with the current process arguments.
///
/// Command failures are rendered to stderr and return exit code `2`, preserving
/// the command's established failure contract.
pub fn main_entry() -> ExitCode {
    match app::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(2)
        }
    }
}
