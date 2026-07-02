//! `snowdrop` — command-line tool for generating, encoding, and decoding
//! Snowdrop IDs. Built with `--features cli`:
//!
//! ```text
//! cargo install snowdrop-id --features cli
//! ```

use std::process::ExitCode;

use snowdrop_id::{Epoch, Generator, MachineId, SnowdropId};

const USAGE: &str = "\
snowdrop — generate, encode, and decode Snowdrop IDs

USAGE:
    snowdrop generate [-m <machine-id>] [-n <count>] [--epoch-ms <ms>]
    snowdrop encode <integer-id>
    snowdrop decode <base62-string> [--epoch-ms <ms>]

COMMANDS:
    generate    Generate new IDs, one per line as `<integer>\\t<base62>`
    encode      Encode an integer ID (decimal or 0x-hex) to base62
    decode      Decode a base62 string and print its fields

OPTIONS:
    -m, --machine-id <0-1023>    Machine ID to stamp into IDs [default: 0]
    -n, --count <N>              Number of IDs to generate [default: 1]
        --epoch-ms <MS>          Epoch in Unix ms [default: 1735689600000,
                                 i.e. 2025-01-01T00:00:00Z]
    -h, --help                   Print this help
    -V, --version                Print version
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Error::Usage(message)) => {
            eprintln!("error: {message}\n\n{USAGE}");
            ExitCode::from(2)
        }
        Err(Error::Runtime(message)) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

enum Error {
    Usage(String),
    Runtime(String),
}

fn usage(message: impl Into<String>) -> Error {
    Error::Usage(message.into())
}

fn run(args: &[String]) -> Result<(), Error> {
    match args.first().map(String::as_str) {
        Some("-h") | Some("--help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some("-V") | Some("--version") => {
            println!("snowdrop {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("generate") => generate(&args[1..]),
        Some("encode") => encode(&args[1..]),
        Some("decode") => decode(&args[1..]),
        Some(other) => Err(usage(format!("unknown command `{other}`"))),
        None => Err(usage("missing command")),
    }
}

/// Parses `--flag value` / `-f value` style options plus positionals.
struct Options {
    machine_id: MachineId,
    count: u64,
    epoch: Epoch,
    positionals: Vec<String>,
}

fn parse_options(args: &[String]) -> Result<Options, Error> {
    let mut options = Options {
        machine_id: MachineId::new(0).unwrap(),
        count: 1,
        epoch: Epoch::DEFAULT,
        positionals: Vec::new(),
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value_for = |name: &str| {
            iter.next()
                .cloned()
                .ok_or_else(|| usage(format!("{name} requires a value")))
        };
        match arg.as_str() {
            "-m" | "--machine-id" => {
                let raw = value_for(arg)?;
                let parsed: u16 = raw
                    .parse()
                    .map_err(|_| usage(format!("invalid machine ID `{raw}`")))?;
                options.machine_id = MachineId::new(parsed)
                    .ok_or_else(|| usage(format!("machine ID `{raw}` is not in 0..=1023")))?;
            }
            "-n" | "--count" => {
                let raw = value_for(arg)?;
                options.count = raw
                    .parse()
                    .map_err(|_| usage(format!("invalid count `{raw}`")))?;
            }
            "--epoch-ms" => {
                let raw = value_for(arg)?;
                let ms: u64 = raw
                    .parse()
                    .map_err(|_| usage(format!("invalid epoch `{raw}`")))?;
                options.epoch = Epoch::from_unix_ms(ms);
            }
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(usage(format!("unknown option `{other}`")));
            }
            _ => options.positionals.push(arg.clone()),
        }
    }
    Ok(options)
}

fn generate(args: &[String]) -> Result<(), Error> {
    let options = parse_options(args)?;
    if let Some(extra) = options.positionals.first() {
        return Err(usage(format!("unexpected argument `{extra}`")));
    }
    let generator = Generator::builder(options.machine_id)
        .epoch(options.epoch)
        .build();
    for _ in 0..options.count {
        let id = generator
            .generate()
            .map_err(|e| Error::Runtime(e.to_string()))?;
        println!("{}\t{}", id.as_u64(), id.encode());
    }
    Ok(())
}

fn encode(args: &[String]) -> Result<(), Error> {
    let options = parse_options(args)?;
    let [raw] = options.positionals.as_slice() else {
        return Err(usage("encode takes exactly one integer ID"));
    };
    let value = parse_u64(raw).ok_or_else(|| usage(format!("invalid integer ID `{raw}`")))?;
    let id = SnowdropId::from_u64(value).map_err(|e| Error::Runtime(e.to_string()))?;
    println!("{}", id.encode());
    Ok(())
}

fn decode(args: &[String]) -> Result<(), Error> {
    let options = parse_options(args)?;
    let [raw] = options.positionals.as_slice() else {
        return Err(usage("decode takes exactly one base62 string"));
    };
    let id = SnowdropId::decode(raw).map_err(|e| Error::Runtime(format!("`{raw}`: {e}")))?;
    let window_start = id.window_start_ms(options.epoch);
    println!("id:           {}", id.as_u64());
    println!("hex:          {:#018x}", id.as_u64());
    println!("base62:       {}", id.encode());
    println!("timestamp:    {}", id.timestamp());
    println!("machine-id:   {}", id.machine_id());
    println!("sequence:     {}", id.sequence());
    println!(
        "window-start: {} ({} ms, epoch {} ms)",
        format_iso8601_ms(window_start),
        window_start,
        options.epoch.unix_ms(),
    );
    Ok(())
}

fn parse_u64(raw: &str) -> Option<u64> {
    if let Some(hex) = raw.strip_prefix("0x").or_else(|| raw.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok()
    } else {
        raw.parse().ok()
    }
}

/// Formats Unix milliseconds as ISO 8601 UTC, e.g. `2026-07-01T12:34:55.616Z`.
/// Date conversion via the days-to-civil algorithm (Howard Hinnant).
fn format_iso8601_ms(unix_ms: u64) -> String {
    let secs = unix_ms / 1000;
    let ms = unix_ms % 1000;
    let days = secs / 86_400;
    let second_of_day = secs % 86_400;

    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year_of_era = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = year_of_era + u64::from(month <= 2);

    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        second_of_day / 3_600,
        second_of_day % 3_600 / 60,
        second_of_day % 60,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_formatting() {
        assert_eq!(format_iso8601_ms(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(
            format_iso8601_ms(1_735_689_600_000),
            "2025-01-01T00:00:00.000Z"
        );
        // 2026-07-01T12:34:56Z rounded down to its 1024 ms window start.
        let window_start = 1_735_689_600_000 + ((47_219_696_000u64 >> 10) << 10);
        assert_eq!(format_iso8601_ms(window_start), "2026-07-01T12:34:55.616Z");
        assert_eq!(
            format_iso8601_ms(951_827_696_789),
            "2000-02-29T12:34:56.789Z"
        );
    }

    #[test]
    fn integer_parsing() {
        assert_eq!(parse_u64("42"), Some(42));
        assert_eq!(parse_u64("0x2A"), Some(42));
        assert_eq!(parse_u64("0X2a"), Some(42));
        assert_eq!(parse_u64("nope"), None);
    }
}
