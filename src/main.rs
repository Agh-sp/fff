#![allow(dead_code)]
#![allow(unused_imports)]

#[cfg(not(target_pointer_width = "64"))]
compile_error!("cs2-dumper requires a 64-bit host");

use std::fs::File;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Instant;

use anyhow::{Context, Result, bail};

use clap::{ArgAction, Parser};

use log::{LevelFilter, info};

use memflow::prelude::v1::*;

use simplelog::*;

use output::Output;

mod analysis;
mod memory;
mod output;
mod source2;

#[derive(Debug, Parser)]
#[command(author, version)]
struct Args {
    /// The name of the memflow connector to use.
    #[arg(short, long)]
    connector: Option<String>,

    /// Additional arguments to pass to the memflow connector.
    #[arg(short = 'a', long, requires = "connector")]
    connector_args: Option<String>,

    /// The types of files to generate.
    #[arg(
        short,
        long,
        value_delimiter = ',',
        value_parser = ["cs", "hpp", "json", "rs", "zig"],
        default_values = ["cs", "hpp", "json", "rs", "zig"]
    )]
    file_types: Vec<String>,

    /// The number of spaces to use per indentation level.
    #[arg(short, long, default_value_t = 4, value_parser = parse_indent_size)]
    indent_size: usize,

    /// The output directory to write the generated files to.
    #[arg(short, long, default_value = "output")]
    output: PathBuf,

    /// The name of the game process.
    #[arg(short, long, default_value = "cs2.exe")]
    process_name: String,

    /// Increase logging verbosity. Can be specified multiple times.
    #[arg(short, long, action = ArgAction::Count)]
    verbose: u8,

    /// Prevent creation of the cs2-dumper.log file.
    #[arg(short, long)]
    no_log_file: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let level_filter = match args.verbose {
        0 => LevelFilter::Error,
        1 => LevelFilter::Warn,
        2 => LevelFilter::Info,
        3 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    };

    let mut loggers: Vec<Box<dyn SharedLogger>> = vec![TermLogger::new(
        level_filter,
        Config::default(),
        TerminalMode::Mixed,
        ColorChoice::Auto,
    )];

    // Create the log file by default.
    if !args.no_log_file {
        loggers.push(WriteLogger::new(
            LevelFilter::Info,
            Config::default(),
            File::create("cs2-dumper.log")?,
        ));
    }

    CombinedLogger::init(loggers)?;

    let conn_args = args
        .connector_args
        .as_deref()
        .map(ConnectorArgs::from_str)
        .transpose()
        .context("unable to parse connector arguments")?
        .unwrap_or_default();

    let mut os = match args.connector {
        Some(conn) => {
            let mut inventory = Inventory::scan();

            inventory
                .builder()
                .connector(&conn)
                .args(conn_args)
                .os("win32")
                .build()?
        }
        None => {
            #[cfg(windows)]
            {
                memflow_native::create_os(&OsArgs::default(), LibArc::default())?
            }
            #[cfg(not(windows))]
            {
                bail!(
                    "on non-Windows hosts specify --connector for a Windows target; native Linux CS2 is not supported by this branch"
                )
            }
        }
    };

    let mut process = os.process_by_name(&args.process_name)?;

    let now = Instant::now();

    let result = analysis::analyze_all(&mut process)?;
    let output = Output::new(&args.file_types, args.indent_size, &args.output, &result)?;

    output.dump_all(&mut process)?;

    info!("analysis completed in {:.2?}", now.elapsed());

    Ok(())
}

fn parse_indent_size(value: &str) -> std::result::Result<usize, String> {
    let size: usize = value
        .parse()
        .map_err(|_| "indent size must be an integer from 0 to 16".to_string())?;
    if size > 16 {
        return Err("indent size must be between 0 and 16".to_string());
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_cli_options() {
        assert!(Args::try_parse_from(["cs2-dumper", "-f", "txt"]).is_err());
        assert!(Args::try_parse_from(["cs2-dumper", "-i", "999999999"]).is_err());
        assert!(Args::try_parse_from(["cs2-dumper", "-a", "invalid"]).is_err());
    }

    #[test]
    fn accepts_supported_formats_and_indentation() {
        let args = Args::try_parse_from(["cs2-dumper", "-f", "json,rs", "-i", "0"]).unwrap();
        assert_eq!(args.file_types, vec!["json", "rs"]);
        assert_eq!(args.indent_size, 0);
        assert_eq!(
            Args::try_parse_from(["cs2-dumper"])
                .unwrap()
                .file_types
                .len(),
            5
        );
    }
}
