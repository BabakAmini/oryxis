//! Oryxis GIF export plugin (issue #71): a thin CLI wrapper over the
//! agg library. Reads an asciicast (.cast) recording, renders it to an
//! animated GIF, writes it to the output path. The app spawns this as
//! a subprocess after generating the .cast from a recorded session;
//! the terminal theme rides inside the cast header (`term.theme`), so
//! no color plumbing crosses the process boundary.
//!
//! Exit codes: 0 success, 1 render/IO failure (message on stderr),
//! 2 usage error. `--version` prints the plugin version for the
//! installed-version display.

use std::io::{BufReader, BufWriter};
use std::process::ExitCode;

/// Parsed CLI: input/output paths plus the few agg knobs the app (or a
/// curious user) may want to override. Everything else stays on agg's
/// defaults; notably the theme, which comes embedded in the cast
/// header, and `show_progress_bar`, which is forced off because the
/// consumer is a spawning app, not an interactive terminal.
struct Args {
    input: String,
    output: String,
    font_size: Option<usize>,
    speed: Option<f64>,
    fps_cap: Option<u8>,
    /// Extra font directories for the monospace face lookup (agg's
    /// escape hatch when the system fonts lack a usable face).
    font_dirs: Vec<String>,
}

const USAGE: &str = "usage: oryxis-gif <input.cast> <output.gif> \
[--font-size N] [--speed F] [--fps-cap N] [--font-dir PATH]...";

fn parse_args(argv: &[String]) -> Result<Args, String> {
    let mut positional: Vec<&String> = Vec::new();
    let mut font_size = None;
    let mut speed = None;
    let mut fps_cap = None;
    let mut font_dirs = Vec::new();
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        let mut take_value = |name: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match arg.as_str() {
            "--font-size" => {
                font_size = Some(
                    take_value("--font-size")?
                        .parse::<usize>()
                        .map_err(|e| format!("--font-size: {e}"))?,
                );
            }
            "--speed" => {
                speed = Some(
                    take_value("--speed")?
                        .parse::<f64>()
                        .map_err(|e| format!("--speed: {e}"))?,
                );
            }
            "--fps-cap" => {
                fps_cap = Some(
                    take_value("--fps-cap")?
                        .parse::<u8>()
                        .map_err(|e| format!("--fps-cap: {e}"))?,
                );
            }
            "--font-dir" => font_dirs.push(take_value("--font-dir")?),
            other if other.starts_with("--") => {
                return Err(format!("unknown option {other}"));
            }
            _ => positional.push(arg),
        }
    }
    let [input, output] = positional.as_slice() else {
        return Err(format!(
            "expected exactly 2 paths, got {}",
            positional.len()
        ));
    };
    if speed.is_some_and(|s| !(s.is_finite() && s > 0.0)) {
        return Err("--speed must be a positive number".into());
    }
    Ok(Args {
        input: (*input).clone(),
        output: (*output).clone(),
        font_size,
        speed,
        fps_cap,
        font_dirs,
    })
}

fn run(args: &Args) -> Result<(), String> {
    let input = std::fs::File::open(&args.input)
        .map_err(|e| format!("open {}: {e}", args.input))?;
    let output = std::fs::File::create(&args.output)
        .map_err(|e| format!("create {}: {e}", args.output))?;

    let mut config = agg::Config {
        // A spawning app consumes stderr, not a human at a TTY.
        show_progress_bar: false,
        ..Default::default()
    };
    if let Some(size) = args.font_size {
        config.font_size = size;
    }
    if let Some(speed) = args.speed {
        config.speed = speed;
    }
    if let Some(fps) = args.fps_cap {
        config.fps_cap = fps;
    }
    config.font_dirs = args.font_dirs.clone();

    let result = agg::run(BufReader::new(input), BufWriter::new(output), config)
        .map_err(|e| format!("render failed: {e:#}"));
    if result.is_err() {
        // Never leave a truncated GIF behind on failure; the app
        // reports the error and the user shouldn't find a broken file
        // where the export was supposed to land.
        let _ = std::fs::remove_file(&args.output);
    }
    result
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--version" || a == "-V") {
        println!("oryxis-gif {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if argv.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let args = match parse_args(&argv) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("oryxis-gif: {e}");
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("oryxis-gif: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_args;

    fn argv(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn two_positionals_parse_with_defaults() {
        let args = parse_args(&argv(&["in.cast", "out.gif"])).unwrap();
        assert_eq!(args.input, "in.cast");
        assert_eq!(args.output, "out.gif");
        assert!(args.font_size.is_none());
        assert!(args.speed.is_none());
        assert!(args.fps_cap.is_none());
        assert!(args.font_dirs.is_empty());
    }

    #[test]
    fn options_parse_and_repeat() {
        let args = parse_args(&argv(&[
            "in.cast",
            "out.gif",
            "--font-size",
            "20",
            "--speed",
            "1.5",
            "--fps-cap",
            "15",
            "--font-dir",
            "/a",
            "--font-dir",
            "/b",
        ]))
        .unwrap();
        assert_eq!(args.font_size, Some(20));
        assert_eq!(args.speed, Some(1.5));
        assert_eq!(args.fps_cap, Some(15));
        assert_eq!(args.font_dirs, vec!["/a", "/b"]);
    }

    #[test]
    fn bad_input_rejected() {
        assert!(parse_args(&argv(&["only-one.cast"])).is_err());
        assert!(parse_args(&argv(&["a", "b", "c"])).is_err());
        assert!(parse_args(&argv(&["a", "b", "--font-size"])).is_err());
        assert!(parse_args(&argv(&["a", "b", "--font-size", "x"])).is_err());
        assert!(parse_args(&argv(&["a", "b", "--speed", "0"])).is_err());
        assert!(parse_args(&argv(&["a", "b", "--speed", "-1"])).is_err());
        assert!(parse_args(&argv(&["a", "b", "--bogus"])).is_err());
    }
}
