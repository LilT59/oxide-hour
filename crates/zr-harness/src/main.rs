//! `zr-harness`: headless JSON-lines control interface over the simulation (ROADMAP step 3.2).
//!
//! Reads one JSON request object per line from stdin, writes one JSON reply per line to stdout.
//! Protocol and field meanings: `docs/HARNESS.md` "Control interface (`zr-harness`)"; `--help` prints
//! a short summary. Nothing but replies goes to stdout; diagnostics go to stderr.

use std::io::{self, BufRead, Write};

use zr_harness::protocol;
use zr_harness::Session;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{}", protocol::HELP);
        return;
    }
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    let mut session = Session::new();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(e) => {
                eprintln!("zr-harness: reading stdin: {e}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let (reply, quit) = protocol::handle_line(&mut session, &line);
        if writeln!(out, "{reply}").is_err() || out.flush().is_err() {
            break;
        }
        if quit {
            break;
        }
    }
}
