//! Fuzzer engine: spawns kernc, detects ICEs, manages crash corpus.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::generate::Generator;

pub struct FuzzEngine {
    r#gen: Generator,
    kernc_bin: String,
    timeout: Duration,
    seed: u64,

    stats: Stats,
    crash_dir: PathBuf,
}

#[derive(Default)]
struct Stats {
    total: u64,
    ice: u64,
    success: u64,
    user_error: u64,
    timed_out: u64,
    spawn_err: u64,
}

impl FuzzEngine {
    pub fn new(r#gen: Generator, kernc_bin: &str, timeout_ms: u64, seed: u64) -> Self {
        let crash_dir = PathBuf::from("crashes");
        let _ = fs::create_dir_all(&crash_dir);

        Self {
            r#gen: r#gen,
            kernc_bin: kernc_bin.to_string(),
            timeout: Duration::from_millis(timeout_ms),
            seed,
            stats: Stats::default(),
            crash_dir,
        }
    }

    pub fn run(&mut self, limit: Option<u64>) {
        let start = Instant::now();

        loop {
            if let Some(limit) = limit {
                if self.stats.total >= limit {
                    break;
                }
            }

            self.stats.total += 1;
            let source = self.r#gen.generate();

            match self.compile_and_check(&source) {
                Outcome::Ok => self.stats.success += 1,
                Outcome::UserError => self.stats.user_error += 1,
                Outcome::Ice(msg) => {
                    self.stats.ice += 1;
                    self.save_crash(&source, &msg);
                    eprintln!(
                        "!!! ICE #{} at iteration {} !!!",
                        self.stats.ice, self.stats.total
                    );
                }
                Outcome::Timeout => self.stats.timed_out += 1,
                Outcome::SpawnErr(e) => {
                    self.stats.spawn_err += 1;
                    eprintln!("spawn error at iteration {}: {e}", self.stats.total);
                }
            }

            if self.stats.total % 100 == 0 {
                let elapsed = start.elapsed().as_secs_f64();
                let rate = self.stats.total as f64 / elapsed;
                self.print_stats(elapsed, rate);
            }
        }

        let elapsed = start.elapsed().as_secs_f64();
        let rate = self.stats.total as f64 / elapsed.max(0.001);
        eprintln!("\nfinal:\n");
        self.print_stats(elapsed, rate);
    }

    fn compile_and_check(&mut self, source: &str) -> Outcome {
        let tmp_path = std::env::temp_dir().join(format!("kernfuzz_{}.kn", self.stats.total));
        if let Err(e) = fs::write(&tmp_path, source) {
            return Outcome::SpawnErr(format!("write temp file: {e}"));
        }

        let mut child = match Command::new(&self.kernc_bin)
            .arg("-c")
            .arg(&tmp_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = fs::remove_file(&tmp_path);
                return Outcome::SpawnErr(format!("spawn kernc: {e}"));
            }
        };

        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = fs::remove_file(&tmp_path);
                        return Outcome::Timeout;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => {
                    let _ = fs::remove_file(&tmp_path);
                    return Outcome::SpawnErr(format!("wait kernc: {e}"));
                }
            }
        };

        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr {
            use std::io::Read;
            let _ = pipe.read_to_string(&mut stderr);
        }

        let _ = fs::remove_file(&tmp_path);

        if is_ice(&stderr, status.code()) {
            Outcome::Ice(stderr)
        } else if status.success() {
            Outcome::Ok
        } else {
            Outcome::UserError
        }
    }

    fn save_crash(&self, source: &str, diag: &str) {
        let crash_path = self
            .crash_dir
            .join(format!("crash_{}_{:06}.kn", self.seed, self.stats.ice));

        let mut f = match fs::File::create(&crash_path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("failed to save crash: {e}");
                return;
            }
        };

        let _ = writeln!(
            f,
            "// kernfuzz crash #{ice} (seed={seed}, iter={total})",
            ice = self.stats.ice,
            seed = self.seed,
            total = self.stats.total
        );
        let _ = writeln!(f, "//");
        for line in diag.lines() {
            let _ = writeln!(f, "// {line}");
        }
        let _ = writeln!(f);
        let _ = f.write_all(source.as_bytes());

        eprintln!("    saved: {}", crash_path.display());
    }

    fn print_stats(&self, elapsed_secs: f64, rate: f64) {
        eprintln!(
            "iter={total:>8} | ice={ice:>4} | ok={ok:>5} | err={err:>6} | \
             timeout={to:>4} | spawn_err={se:>3} | {rate:.1}/s | elapsed={elapsed:.0}s",
            total = self.stats.total,
            ice = self.stats.ice,
            ok = self.stats.success,
            err = self.stats.user_error,
            to = self.stats.timed_out,
            se = self.stats.spawn_err,
            rate = rate,
            elapsed = elapsed_secs,
        );
    }
}

enum Outcome {
    Ok,
    UserError,
    Ice(String),
    Timeout,
    SpawnErr(String),
}

fn is_ice(stderr: &str, exit_code: Option<i32>) -> bool {
    if exit_code == Some(101) {
        return true;
    }
    if stderr.contains("Kern Compiler Internal Error") {
        return true;
    }
    if stderr.contains("LLVM IR Verification Failed") {
        return true;
    }
    if stderr.contains("panicked at") {
        return true;
    }
    false
}
