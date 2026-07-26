//! nowayprompt public executable multiplexer.

use std::process::ExitCode;

fn main() -> ExitCode {
    match nowayprompt::command::run(std::env::args_os().collect()) {
        Ok(status) => ExitCode::from(status),
        Err(error) => {
            eprintln!("nowayprompt: {error}");
            ExitCode::from(1)
        }
    }
}
