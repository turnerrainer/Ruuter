//! ruuter-doctor — pre-boot config sanity checker (h2ck.me v1 T-11).
//!
//! Loads a ruuter.yaml (or falls back to the same conventional
//! search paths the main binary uses), runs the exact same
//! `warn_on_stale_config_fields` registry the boot path uses, and
//! reports:
//!
//!   exit 0 → config parsed and NO warnings would fire at boot.
//!   exit 1 → config parsed but at least one WARN would fire.
//!   exit 2 → config file unreadable / unparseable.
//!   exit 3 → invalid CLI arguments.
//!
//! Every WARN is captured via a scoped tracing subscriber and
//! echoed to stdout so operators can pipe / grep the output in CI.
//!
//! Usage:
//!   ruuter-doctor                            # ./ruuter.yaml (+ conventional paths)
//!   ruuter-doctor --config /etc/ruuter.yaml  # explicit path
//!   ruuter-doctor --help                     # this help

use ruuter_on_rust::config::{
    load_or_default_via_env_or_path, warn_on_stale_config_fields, AppConfig,
};
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let mut config_arg: Option<PathBuf> = None;
    let mut iter = args.iter().skip(1);
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--help" | "-h" => {
                print_help();
                return ExitCode::from(0);
            }
            "--config" | "-c" => {
                if let Some(v) = iter.next() {
                    config_arg = Some(PathBuf::from(v));
                } else {
                    eprintln!("ruuter-doctor: --config requires a path argument");
                    return ExitCode::from(3);
                }
            }
            other if other.starts_with("--config=") => {
                config_arg = Some(PathBuf::from(other.trim_start_matches("--config=")));
            }
            other => {
                eprintln!("ruuter-doctor: unknown argument: {}", other);
                print_help();
                return ExitCode::from(3);
            }
        }
    }

    let (config, source) = match load_or_default_via_env_or_path(config_arg.as_deref()) {
        Ok(tuple) => tuple,
        Err(e) => {
            eprintln!("ruuter-doctor: config load failed: {e}");
            return ExitCode::from(2);
        }
    };

    // Capture WARNs into a shared buffer so we can echo them and
    // count them to pick the exit code.
    let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let writer = SharedBufWriter(buf.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_max_level(tracing::Level::WARN)
        .with_env_filter(tracing_subscriber::EnvFilter::new("warn"))
        .with_ansi(false)
        .without_time()
        .finish();

    let warn_count = tracing::subscriber::with_default(subscriber, || {
        run_all_boot_warn_checks(&config);
        // Count WARN lines by scanning captured bytes.
        let bytes = buf.lock().unwrap().clone();
        String::from_utf8_lossy(&bytes)
            .lines()
            .filter(|l| l.contains(" WARN "))
            .count()
    });

    println!("ruuter-doctor: v{} check", env!("CARGO_PKG_VERSION"));
    match source.as_ref() {
        Some(p) => println!("ruuter-doctor: config source: {}", p.display()),
        None => println!("ruuter-doctor: config source: built-in defaults"),
    }
    let captured = String::from_utf8_lossy(&buf.lock().unwrap()).into_owned();
    if warn_count == 0 {
        println!("ruuter-doctor: 0 warnings — config is clean.");
        ExitCode::from(0)
    } else {
        println!("ruuter-doctor: {warn_count} warning(s):");
        for line in captured.lines() {
            println!("  {line}");
        }
        println!(
            "ruuter-doctor: at least one warning would fire at boot — exit 1 (h2ck.me v1 T-11)."
        );
        ExitCode::from(1)
    }
}

fn print_help() {
    println!(
        "ruuter-doctor — pre-boot config sanity checker\n\n\
         Usage:\n\
         \x20 ruuter-doctor [--config PATH]\n\n\
         Exit codes:\n\
         \x20 0  config loaded cleanly, no warnings would fire at boot\n\
         \x20 1  config loaded, at least one warning would fire\n\
         \x20 2  config file unreadable or unparseable\n\
         \x20 3  invalid CLI arguments\n\n\
         Options:\n\
         \x20 --config, -c PATH  Explicit config path (else RUUTER_CONFIG env / ./ruuter.yaml)\n\
         \x20 --help, -h         This help\n\n\
         h2ck.me v1 T-11 — see CHANGELOG.md for the check catalogue."
    );
}

fn run_all_boot_warn_checks(config: &AppConfig) {
    // Same registry the main binary calls right after config load.
    warn_on_stale_config_fields(config);
}

#[derive(Clone)]
struct SharedBufWriter(Arc<Mutex<Vec<u8>>>);

impl io::Write for SharedBufWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedBufWriter {
    type Writer = SharedBufWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
