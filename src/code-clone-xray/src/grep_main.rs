use std::process::ExitCode;

use clap::Parser;
use code_clone_xray::grep::{run_grep_command, GrepArgs};

#[derive(Parser)]
#[command(
    name = "xray-grep",
    version,
    about = "Review-First multi-language grep with tree-sitter enclosing symbol & category tagging"
)]
struct Cli {
    #[command(flatten)]
    args: GrepArgs,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run_grep_command(&cli.args) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(1)
        }
    }
}
