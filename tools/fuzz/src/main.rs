//! `kernfuzz` generates random syntactically plausible Kern source files, feeds them to
//! `kernc`, and watches for internal compiler errors (panics, ICE diagnostics, LLVM IR
//! verification failures).
//!
//! Crash-triggering inputs will be saved to `crashes/`.

mod engine;
mod generate;

use std::env;
use std::process;

use engine::FuzzEngine;
use generate::Generator;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

fn main() {
    let mut args = env::args().skip(1).collect::<Vec<_>>();

    let seed: u64 = if let Some(pos) = args.iter().position(|a| a == "--seed") {
        let val = args
            .get(pos + 1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| {
                eprintln!("kernfuzz: --seed requires a u64 argument");
                process::exit(1);
            });
        args.remove(pos);
        args.remove(pos);
        val
    } else {
        rand::random()
    };

    let timeout_ms: u64 = if let Some(pos) = args.iter().position(|a| a == "--timeout-ms") {
        let val = args
            .get(pos + 1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(30_000);
        args.remove(pos);
        args.remove(pos);
        val
    } else {
        60_000
    };

    let limit: Option<u64> = if let Some(pos) = args.iter().position(|a| a == "--limit") {
        let val = args.get(pos + 1).and_then(|s| s.parse().ok());
        args.remove(pos);
        args.remove(pos);
        val
    } else {
        None
    };

    let kernc_bin: String = if let Some(pos) = args.iter().position(|a| a == "--kernc") {
        let val = args
            .get(pos + 1)
            .cloned()
            .unwrap_or_else(|| "kernc".to_string());
        args.remove(pos);
        args.remove(pos);
        val
    } else {
        "kernc".to_string()
    };

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return;
    }

    let rng = ChaCha8Rng::seed_from_u64(seed);
    let generator = Generator::new(rng);

    let mut engine = FuzzEngine::new(generator, &kernc_bin, timeout_ms, seed);

    eprintln!("kernfuzz: seed={seed} timeout_ms={timeout_ms} kernc={kernc_bin}");

    engine.run(limit);
}

fn print_help() {
    eprintln!(
        "kernfuzz — random-program fuzzer for kernc\n\n\
         USAGE: kernfuzz [OPTIONS]\n\n\
         OPTIONS:\n\
           --seed <u64>        RNG seed (default: random)\n\
           --timeout-ms <ms>   Per-invocation timeout in ms (default: 60000)\n\
           --limit <n>         Stop after N iterations (default: unlimited)\n\
           --kernc <path>      Path to kernc binary (default: kernc on PATH)\n\
           --help, -h          Show this help"
    );
}
