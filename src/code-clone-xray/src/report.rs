use crate::cli::OutputFormat;
use crate::model::{CloneReport, FormatStats};

pub fn render_json(report: &CloneReport) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(report)
}

pub fn print_console(report: &CloneReport) {
    let tot = &report.statistics.total;
    println!("structural-clone  schema={}", report.schema_version);
    println!(
        "files={}  lines={}  cloneGroups={}  clonePairs={}  duplicatedLines={}  ({:.2}%)",
        tot.files,
        tot.lines,
        tot.clone_groups,
        tot.clone_pairs,
        tot.duplicated_lines,
        tot.percentage
    );

    let f = &report.statistics.formats;
    let rows: [(&str, &FormatStats, bool); 9] = [
        ("csharp", &f.csharp, true),
        ("xaml", &f.xaml, true),
        ("rust", &f.rust, true),
        ("typescript", &f.typescript, false),
        ("javascript", &f.javascript, false),
        ("python", &f.python, false),
        ("svelte", &f.svelte, false),
        ("html", &f.html, false),
        ("css", &f.css, false),
    ];
    for (label, st, always_show) in rows {
        if always_show || st.files > 0 {
            println!(
                "  {label:<10} files={} pairs={} dupLines={} ({:.2}%)",
                st.files, st.clone_pairs, st.duplicated_lines, st.percentage
            );
        }
    }

    if report.duplicates.is_empty() {
        println!("No structural clones above thresholds.");
        return;
    }

    println!();
    println!("Top duplicated blocks:");
    for (i, d) in report.duplicates.iter().enumerate() {
        println!(
            "  {}. {} lines [{}, sim={:.1}%]  {} L{}-{}  <->  {} L{}-{}",
            i + 1,
            d.lines,
            d.format,
            d.similarity,
            d.first_file.path,
            d.first_file.start_line,
            d.first_file.end_line,
            d.second_file.path,
            d.second_file.start_line,
            d.second_file.end_line
        );
    }
}

pub fn emit(
    report: &CloneReport,
    format: OutputFormat,
    output: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let wants_json = matches!(format, OutputFormat::Json | OutputFormat::Both);
    let wants_console = matches!(format, OutputFormat::Console | OutputFormat::Both);

    if wants_console {
        print_console(report);
    }

    if wants_json {
        let json = render_json(report)?;
        if let Some(path) = output {
            std::fs::write(path, &json)?;
            if wants_console {
                eprintln!("Wrote JSON report to {}", path.display());
            }
        } else {
            if wants_console {
                println!();
                println!("--- json ---");
            }
            println!("{json}");
        }
    }

    Ok(())
}
