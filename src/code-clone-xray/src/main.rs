use std::process::ExitCode;

use clap::{error::ErrorKind, Parser};
use code_clone_xray::cli::CliArgs;
use code_clone_xray::detect::{detect_clones, DetectConfig};
use code_clone_xray::normalize::parse_files_parallel;
use code_clone_xray::report;
use code_clone_xray::walk::collect_source_files;

fn main() -> ExitCode {
    let args = match CliArgs::try_parse() {
        Ok(a) => a,
        Err(e) => {
            let _ = e.print();
            return match e.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => ExitCode::SUCCESS,
                _ => ExitCode::from(3),
            };
        }
    };

    if let Err(msg) = args.validate() {
        eprintln!("error: {msg}");
        return ExitCode::from(3);
    }

    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(2)
        }
    }
}

fn run(args: CliArgs) -> anyhow::Result<()> {
    let files = collect_source_files(&args);
    let parsed = parse_files_parallel(&files);

    let config = DetectConfig {
        paths: args
            .path
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect(),
        min_lines: args.min_lines,
        min_similarity: args.min_similarity,
        window: args.window,
        top: args.top,
    };

    let report = detect_clones(&parsed, &config);
    report::emit(&report, args.format, args.output.as_deref())?;
    Ok(())
}
