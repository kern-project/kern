//! `kernup` installer entry point.
//!
//! The binary downloads or installs a Kern SDK archive, validates the resulting
//! toolchain, and optionally wires the installed tools into the user's PATH.

use shared_cli::{ColorChoice, ErrorReport, HelpDoc, HelpSection};
use shared_ops::{
    ArchiveExtractProgress, DownloadProgress, OpsError, OpsResult, SdkValidationProgress,
    archive_kind_from_path, configure_path, copy_sdk_contents, default_install_root,
    detect_host_target, download_file_with_progress, extract_archive_with_progress,
    fetch_latest_github_release, infer_release_version_from_archive_name, make_temp_dir,
    remove_path_if_exists, set_command_logging_enabled, set_status_logging_enabled,
    validate_sdk_root, validate_sdk_root_with_progress, verify_installed_tools,
};
use std::env;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug)]
enum Command {
    Install(InstallArgs),
    Doctor(DoctorArgs),
    Doc(DocArgs),
    Target,
    Version,
    Help,
}

#[derive(Debug, Default)]
struct InstallArgs {
    version: Option<String>,
    archive: Option<PathBuf>,
    dest: Option<PathBuf>,
    target: Option<String>,
    github_repo: String,
    no_path: bool,
    verbose: bool,
}

#[derive(Debug, Default)]
struct DoctorArgs {
    dest: Option<PathBuf>,
    verbose: bool,
}

#[derive(Debug, Default)]
struct DocArgs {
    dest: Option<PathBuf>,
    topic: Option<String>,
    path_only: bool,
}

struct Ui {
    verbose: bool,
    terminal: bool,
    color: ColorChoice,
}

struct StepProgress<'a> {
    ui: &'a Ui,
    message: String,
    start: Instant,
}

struct OpsLoggingGuard {
    previous_command: bool,
    previous_status: bool,
}

fn main() {
    if let Err(err) = run() {
        eprint!(
            "{}",
            ErrorReport::new("kernup error", err.to_string()).render(ColorChoice::Auto)
        );
        std::process::exit(1);
    }
}

fn run() -> OpsResult<()> {
    match parse_args(env::args().skip(1).collect())? {
        Command::Install(args) => install(args),
        Command::Doctor(args) => doctor(args),
        Command::Doc(args) => doc(args),
        Command::Target => {
            println!("{}", detect_host_target()?.archive_target);
            Ok(())
        }
        Command::Version => {
            println!("kernup v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Help => {
            print!("{}", help().render(ColorChoice::Auto));
            Ok(())
        }
    }
}

fn parse_args(args: Vec<String>) -> OpsResult<Command> {
    let (verbose, args) = strip_verbose_flags(args);
    let Some(command) = args.first().map(String::as_str) else {
        return Ok(Command::Help);
    };

    match command {
        "install" => parse_install_args(&args[1..], verbose).map(Command::Install),
        "doctor" => parse_doctor_args(&args[1..], verbose).map(Command::Doctor),
        "doc" => parse_doc_args(&args[1..]).map(Command::Doc),
        "target" => Ok(Command::Target),
        "help" | "--help" | "-h" => Ok(Command::Help),
        "--version" | "-V" => Ok(Command::Version),
        other => Err(OpsError::new(format!(
            "unknown command `{other}`; run `kernup help`"
        ))),
    }
}

fn strip_verbose_flags(args: Vec<String>) -> (bool, Vec<String>) {
    let mut verbose = false;
    let mut stripped = Vec::new();
    for arg in args {
        if arg == "-v" || arg == "--verbose" {
            verbose = true;
        } else {
            stripped.push(arg);
        }
    }
    (verbose, stripped)
}

fn parse_install_args(args: &[String], verbose: bool) -> OpsResult<InstallArgs> {
    let mut parsed = InstallArgs {
        github_repo: "kern-project/kern".into(),
        verbose,
        ..InstallArgs::default()
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--version" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(OpsError::new("`--version` requires a value"));
                };
                parsed.version = Some(value.clone());
            }
            "--archive" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(OpsError::new("`--archive` requires a value"));
                };
                parsed.archive = Some(PathBuf::from(value));
            }
            "--dest" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(OpsError::new("`--dest` requires a value"));
                };
                parsed.dest = Some(PathBuf::from(value));
            }
            "--target" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(OpsError::new("`--target` requires a value"));
                };
                parsed.target = Some(value.clone());
            }
            "--github-repo" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(OpsError::new("`--github-repo` requires a value"));
                };
                parsed.github_repo = value.clone();
            }
            "--no-path" => {
                parsed.no_path = true;
            }
            "-v" | "--verbose" => {
                parsed.verbose = true;
            }
            "--help" | "-h" => {
                print!("{}", install_help().render(ColorChoice::Auto));
                std::process::exit(0);
            }
            other => {
                return Err(OpsError::new(format!(
                    "unexpected install argument `{other}`"
                )));
            }
        }
        index += 1;
    }
    Ok(parsed)
}

fn parse_doctor_args(args: &[String], verbose: bool) -> OpsResult<DoctorArgs> {
    let mut parsed = DoctorArgs {
        verbose,
        ..DoctorArgs::default()
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--dest" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(OpsError::new("`--dest` requires a value"));
                };
                parsed.dest = Some(PathBuf::from(value));
            }
            "-v" | "--verbose" => {
                parsed.verbose = true;
            }
            "--help" | "-h" => {
                print!("{}", doctor_help().render(ColorChoice::Auto));
                std::process::exit(0);
            }
            other => {
                return Err(OpsError::new(format!(
                    "unexpected doctor argument `{other}`"
                )));
            }
        }
        index += 1;
    }
    Ok(parsed)
}

fn parse_doc_args(args: &[String]) -> OpsResult<DocArgs> {
    let mut parsed = DocArgs::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--dest" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err(OpsError::new("`--dest` requires a value"));
                };
                parsed.dest = Some(PathBuf::from(value));
            }
            "--path" => {
                parsed.path_only = true;
            }
            "--help" | "-h" => {
                print!("{}", doc_help().render(ColorChoice::Auto));
                std::process::exit(0);
            }
            value if value.starts_with('-') => {
                return Err(OpsError::new(format!("unexpected doc argument `{value}`")));
            }
            value => {
                if parsed.topic.is_some() {
                    return Err(OpsError::new(
                        "`kernup doc` accepts at most one topic".to_string(),
                    ));
                }
                parsed.topic = Some(value.to_string());
            }
        }
        index += 1;
    }
    Ok(parsed)
}

fn install(args: InstallArgs) -> OpsResult<()> {
    let ui = Ui::new(args.verbose);
    let _logging = OpsLoggingGuard::set(args.verbose, args.verbose);
    let host = detect_host_target()?;
    let target = args
        .target
        .clone()
        .unwrap_or_else(|| host.archive_target.clone());
    if target != host.archive_target {
        return Err(OpsError::new(format!(
            "target `{target}` does not match the current host `{}`",
            host.archive_target
        )));
    }

    let install_root = args.dest.clone().unwrap_or(default_install_root(&host)?);
    let temp_root = make_temp_dir("kernup-install-")?;
    let result = (|| -> OpsResult<()> {
        ui.header("install", &target);
        ui.meta("dest", install_root.display());
        let total_steps = install_step_count(&args);
        let mut step = 0usize;
        let (archive, version) = resolve_install_archive(
            &args,
            &target,
            &host,
            &temp_root,
            &ui,
            &mut step,
            total_steps,
        )?;
        let extract_root = temp_root.join("extract");
        step += 1;
        let sdk_root = ui.extract_step(&archive, &extract_root, step, total_steps)?;
        step += 1;
        ui.validate_step(&sdk_root, &target, step, total_steps)?;
        step += 1;
        ui.step(step, total_steps, "install", || {
            copy_sdk_contents(&sdk_root, &install_root)
        })?;
        step += 1;
        ui.step(step, total_steps, "verify", || {
            verify_installed_tools(&install_root, &host)
        })?;
        if args.no_path {
            ui.status(
                "path",
                format!(
                    "add `{}` to PATH when ready",
                    install_root.join("bin").display()
                ),
            );
        } else {
            step += 1;
            ui.step(step, total_steps, "path", || {
                configure_path(&install_root.join("bin"), &host)
            })?;
        }
        ui.ok(format!(
            "Kern {version} SDK installed into {}",
            install_root.display()
        ));
        Ok(())
    })();
    let _ = remove_path_if_exists(&temp_root);
    result
}

fn resolve_install_archive(
    args: &InstallArgs,
    target: &str,
    host: &shared_ops::HostTarget,
    temp_root: &std::path::Path,
    ui: &Ui,
    step: &mut usize,
    total_steps: usize,
) -> OpsResult<(PathBuf, String)> {
    if let Some(archive) = &args.archive {
        *step += 1;
        return ui.step(*step, total_steps, "resolve archive", || {
            if !archive.is_file() {
                return Err(OpsError::new(format!(
                    "archive `{}` does not exist",
                    archive.display()
                )));
            }
            let version = args
                .version
                .clone()
                .or_else(|| {
                    archive
                        .file_name()
                        .and_then(|name| name.to_str())
                        .and_then(|name| infer_release_version_from_archive_name(name, target))
                })
                .unwrap_or_else(|| "<local>".to_string());
            ui.meta("archive", archive.display());
            Ok((archive.clone(), version))
        });
    }

    let version = if let Some(version) = args.version.clone() {
        version
    } else {
        *step += 1;
        ui.step(*step, total_steps, "resolve", || {
            Ok(fetch_latest_github_release(&args.github_repo)
                .ok()
                .flatten()
                .unwrap_or_else(|| "v0.8.2".to_string()))
        })?
    };
    let archive_name = format!("kern-{version}-{target}.{}", host.archive_extension);
    let archive = temp_root.join(&archive_name);
    let url = format!(
        "https://github.com/{}/releases/download/{version}/{archive_name}",
        args.github_repo
    );
    *step += 1;
    ui.download_step(&url, &archive, *step, total_steps, &version)?;
    Ok((archive, version))
}

fn doctor(args: DoctorArgs) -> OpsResult<()> {
    let ui = Ui::new(args.verbose);
    let _logging = OpsLoggingGuard::set(args.verbose, args.verbose);
    let host = detect_host_target()?;
    let install_root = args.dest.unwrap_or(default_install_root(&host)?);
    ui.header("doctor", &host.archive_target);
    ui.meta("dest", install_root.display());
    ui.step(1, 2, "validate", || {
        validate_sdk_root(&install_root, &host.archive_target).map(|_| ())
    })?;
    ui.step(2, 2, "verify", || {
        verify_installed_tools(&install_root, &host)
    })?;
    ui.ok("SDK installation is healthy");
    Ok(())
}

fn doc(args: DocArgs) -> OpsResult<()> {
    let host = detect_host_target()?;
    let install_root = args.dest.unwrap_or(default_install_root(&host)?);
    let relative = doc_topic_path(args.topic.as_deref())?;
    let path = install_root.join(relative);
    if !path.is_file() {
        return Err(OpsError::new(format!(
            "documentation topic `{}` is missing at `{}`; reinstall the SDK or run `kernup doctor`",
            args.topic.as_deref().unwrap_or("index"),
            path.display()
        )));
    }
    if args.path_only {
        println!("{}", path.display());
    } else {
        println!("Kern documentation: {}", path.display());
    }
    Ok(())
}

fn doc_topic_path(topic: Option<&str>) -> OpsResult<&'static str> {
    match topic.unwrap_or("index") {
        "index" | "map" | "docs" => Ok(shared_ops::SDK_DOC_ENTRY),
        "install" => Ok("docs/install.md"),
        "kernc" => Ok("docs/kernc.md"),
        "craft" => Ok("docs/craft.md"),
        "design" | "language" => Ok("docs/design.md"),
        "runtime" => Ok("docs/runtime-architecture.md"),
        "style" => Ok("docs/style.md"),
        "nix" => Ok("docs/nix.md"),
        "tutorial" => Ok("docs/tutorial/README.md"),
        "tutorial-zh" | "zh" => Ok("docs/tutorial/zh/README.md"),
        other => Err(OpsError::new(format!(
            "unknown documentation topic `{other}`; try `index`, `tutorial`, `design`, `kernc`, `craft`, or `install`"
        ))),
    }
}

fn install_step_count(args: &InstallArgs) -> usize {
    let mut count = 4usize;
    if args.archive.is_some() {
        count += 1;
    } else {
        count += if args.version.is_some() { 1 } else { 2 };
    }
    if !args.no_path {
        count += 1;
    }
    count
}

impl Ui {
    fn new(verbose: bool) -> Self {
        Self {
            verbose,
            terminal: std::io::stderr().is_terminal(),
            color: ColorChoice::Auto,
        }
    }

    fn header(&self, command: &str, target: &str) {
        self.line(format!(
            "{} {} {target}",
            self.paint("1;36", "==>"),
            self.paint("1;36", command)
        ));
    }

    fn meta(&self, label: &str, value: impl std::fmt::Display) {
        if self.verbose {
            self.line(format!(
                "    {} {value}",
                self.paint("2", &format!("{label:<10}"))
            ));
        }
    }

    fn status(&self, label: &str, value: impl std::fmt::Display) {
        self.line(format!(
            "    {} {value}",
            self.paint("2", &format!("{label:<10}"))
        ));
    }

    fn ok(&self, message: impl std::fmt::Display) {
        self.line(format!("{} {message}", self.paint("1;32", "[ok]")));
    }

    fn step<T>(
        &self,
        index: usize,
        total: usize,
        message: impl Into<String>,
        action: impl FnOnce() -> OpsResult<T>,
    ) -> OpsResult<T> {
        let progress = self.start_step(index, total, message.into());
        let result = action();
        match &result {
            Ok(_) => progress.finish_ok(),
            Err(_) => progress.finish_err(),
        }
        result
    }

    fn download_step(
        &self,
        url: &str,
        archive: &std::path::Path,
        index: usize,
        total: usize,
        version: &str,
    ) -> OpsResult<()> {
        let progress = self.start_step(index, total, format!("download {version}"));
        let start = Instant::now();
        let mut last_render = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let result = download_file_with_progress(url, archive, |download| {
            if self.verbose || !self.terminal {
                return;
            }
            let now = Instant::now();
            let done = download
                .total
                .is_some_and(|total| total > 0 && download.downloaded >= total);
            if !done && now.duration_since(last_render) < Duration::from_millis(120) {
                return;
            }
            last_render = now;
            let _ = write!(
                std::io::stderr(),
                "\r\x1b[2K{} download {version}  {}",
                self.progress_line_fraction(
                    index.saturating_sub(1),
                    total,
                    download_fraction(download)
                ),
                render_download_progress(download, start.elapsed())
            );
            let _ = std::io::stderr().flush();
        });
        match &result {
            Ok(_) => progress.finish_ok(),
            Err(_) => progress.finish_err(),
        }
        result
    }

    fn extract_step(
        &self,
        archive: &std::path::Path,
        extract_root: &std::path::Path,
        index: usize,
        total: usize,
    ) -> OpsResult<PathBuf> {
        let progress = self.start_step(index, total, "extract".to_string());
        let start = Instant::now();
        let mut last_render = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let result = extract_archive_with_progress(
            archive,
            extract_root,
            archive_kind_from_path(archive)?,
            |extract| {
                if self.verbose || !self.terminal {
                    return;
                }
                let now = Instant::now();
                if now.duration_since(last_render) < Duration::from_millis(120) {
                    return;
                }
                last_render = now;
                let _ = write!(
                    std::io::stderr(),
                    "\r\x1b[2K{} extract  {}",
                    self.progress_line_fraction(
                        index.saturating_sub(1),
                        total,
                        extract_fraction(extract)
                    ),
                    render_extract_progress(extract, start.elapsed())
                );
                let _ = std::io::stderr().flush();
            },
        );
        match &result {
            Ok(_) => progress.finish_ok(),
            Err(_) => progress.finish_err(),
        }
        result
    }

    fn validate_step(
        &self,
        sdk_root: &std::path::Path,
        target: &str,
        index: usize,
        total: usize,
    ) -> OpsResult<()> {
        let progress = self.start_step(index, total, "validate".to_string());
        let start = Instant::now();
        let mut last_render = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);
        let result = validate_sdk_root_with_progress(sdk_root, target, |validation| {
            if self.verbose || !self.terminal {
                return;
            }
            let now = Instant::now();
            if now.duration_since(last_render) < Duration::from_millis(120) {
                return;
            }
            last_render = now;
            let _ = write!(
                std::io::stderr(),
                "\r\x1b[2K{} validate  {}",
                self.progress_line_fraction(
                    index.saturating_sub(1),
                    total,
                    validation_fraction(validation)
                ),
                render_validation_progress(validation, start.elapsed())
            );
            let _ = std::io::stderr().flush();
        })
        .map(|_| ());
        match &result {
            Ok(_) => progress.finish_ok(),
            Err(_) => progress.finish_err(),
        }
        result
    }

    fn start_step(&self, index: usize, total: usize, message: String) -> StepProgress<'_> {
        if self.verbose || !self.terminal {
            self.line(format!("  {} {message}", self.paint("1;34", "=>")));
        } else {
            let _ = write!(
                std::io::stderr(),
                "\r\x1b[2K{} {message}",
                self.progress_line(index.saturating_sub(1), total)
            );
            let _ = std::io::stderr().flush();
        }
        StepProgress {
            ui: self,
            message,
            start: Instant::now(),
        }
    }

    fn progress_line(&self, completed: usize, total: usize) -> String {
        self.progress_line_fraction(completed, total, None)
    }

    fn progress_line_fraction(
        &self,
        completed: usize,
        total: usize,
        current_fraction: Option<f64>,
    ) -> String {
        let total = total.max(1);
        let completed_units = completed as f64 + current_fraction.unwrap_or(0.0).clamp(0.0, 1.0);
        let percent = ((completed_units / total as f64) * 100.0).round() as usize;
        let filled = ((completed_units / total as f64) * 18.0).floor() as usize;
        format!(
            "kernup {} {percent:>3}%",
            self.paint(
                "1;34",
                &render_fractional_progress_bar(filled, completed >= total, 18)
            )
        )
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if !self.color_enabled() {
            return text.to_string();
        }
        format!("\x1b[{code}m{text}\x1b[0m")
    }

    fn color_enabled(&self) -> bool {
        match self.color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => self.terminal && env::var_os("NO_COLOR").is_none(),
        }
    }

    fn line(&self, line: impl std::fmt::Display) {
        let mut stderr = std::io::stderr();
        let _ = writeln!(stderr, "{line}");
        let _ = stderr.flush();
    }
}

impl StepProgress<'_> {
    fn finish_ok(self) {
        self.finish("1;32", "[ok]");
    }

    fn finish_err(self) {
        self.finish("1;31", "[err]");
    }

    fn finish(self, code: &str, marker: &str) {
        let elapsed = format_duration(self.start.elapsed());
        if self.ui.verbose || !self.ui.terminal {
            self.ui.line(format!(
                "  {} {} {elapsed}",
                self.ui.paint(code, marker),
                self.message
            ));
        } else {
            let _ = writeln!(
                std::io::stderr(),
                "\r\x1b[2K{} {} {elapsed}",
                self.ui.paint(code, marker),
                self.message
            );
        }
    }
}

impl OpsLoggingGuard {
    fn set(command: bool, status: bool) -> Self {
        let previous_command = set_command_logging_enabled(command);
        let previous_status = set_status_logging_enabled(status);
        Self {
            previous_command,
            previous_status,
        }
    }
}

impl Drop for OpsLoggingGuard {
    fn drop(&mut self) {
        set_command_logging_enabled(self.previous_command);
        set_status_logging_enabled(self.previous_status);
    }
}

#[cfg(test)]
fn render_progress_bar(completed: usize, total: usize, width: usize) -> String {
    if total == 0 {
        return format!("[>{}]", "-".repeat(width.saturating_sub(1)));
    }
    if completed >= total {
        return format!("[{}]", "=".repeat(width));
    }
    let filled = completed.saturating_mul(width) / total;
    let head = filled.min(width.saturating_sub(1));
    format!(
        "[{}>{}]",
        "=".repeat(head),
        "-".repeat(width.saturating_sub(head + 1))
    )
}

fn render_fractional_progress_bar(filled: usize, complete: bool, width: usize) -> String {
    if complete || filled >= width {
        return format!("[{}]", "=".repeat(width));
    }
    let head = filled.min(width.saturating_sub(1));
    format!(
        "[{}>{}]",
        "=".repeat(head),
        "-".repeat(width.saturating_sub(head + 1))
    )
}

fn download_fraction(progress: DownloadProgress) -> Option<f64> {
    progress
        .total
        .filter(|total| *total > 0)
        .map(|total| progress.downloaded as f64 / total as f64)
}

fn extract_fraction(progress: ArchiveExtractProgress) -> Option<f64> {
    if let Some(total_bytes) = progress.total_bytes.filter(|total| *total > 0) {
        return Some(progress.bytes as f64 / total_bytes as f64);
    }
    progress
        .total_entries
        .filter(|total| *total > 0)
        .map(|total| progress.entries as f64 / total as f64)
}

fn validation_fraction(progress: SdkValidationProgress) -> Option<f64> {
    progress
        .total
        .filter(|total| *total > 0)
        .map(|total| progress.completed as f64 / total as f64)
}

fn format_duration(duration: Duration) -> String {
    if duration.as_secs() >= 60 {
        let mins = duration.as_secs() / 60;
        let secs = duration.as_secs() % 60;
        format!("{mins}m{secs:02}s")
    } else if duration.as_secs() >= 1 {
        format!("{}s", duration.as_secs())
    } else {
        "<1s".to_string()
    }
}

fn render_download_progress(progress: DownloadProgress, elapsed: Duration) -> String {
    let downloaded = format_bytes(progress.downloaded);
    let rate = if elapsed.as_secs_f64() > 0.0 {
        Some((progress.downloaded as f64 / elapsed.as_secs_f64()) as u64)
    } else {
        None
    };
    let rate_text = rate
        .map(|bytes| format!("{}/s", format_bytes(bytes)))
        .unwrap_or_else(|| "--/s".to_string());
    if let Some(total) = progress.total {
        let percent = progress
            .downloaded
            .saturating_mul(100)
            .checked_div(total.max(1))
            .unwrap_or(0)
            .min(100);
        let eta = rate.and_then(|rate| {
            (rate > 0 && progress.downloaded < total)
                .then(|| Duration::from_secs((total - progress.downloaded) / rate))
        });
        let eta_text = eta
            .map(|duration| format!(" eta {}", format_duration(duration)))
            .unwrap_or_default();
        format!(
            "{downloaded}/{} {percent:>3}% {rate_text}{eta_text}",
            format_bytes(total)
        )
    } else {
        format!("{downloaded} {rate_text}")
    }
}

fn render_extract_progress(progress: ArchiveExtractProgress, elapsed: Duration) -> String {
    let rate = if elapsed.as_secs_f64() > 0.0 {
        Some((progress.bytes as f64 / elapsed.as_secs_f64()) as u64)
    } else {
        None
    };
    let rate_text = rate
        .map(|bytes| format!("{}/s", format_bytes(bytes)))
        .unwrap_or_else(|| "--/s".to_string());
    format!(
        "{} item(s), {} {rate_text}",
        progress.entries,
        format_bytes(progress.bytes)
    )
}

fn render_validation_progress(progress: SdkValidationProgress, elapsed: Duration) -> String {
    let elapsed = format_duration(elapsed);
    match progress.total {
        Some(total) if total > 0 => {
            let percent = progress
                .completed
                .saturating_mul(100)
                .checked_div(total)
                .unwrap_or(0)
                .min(100);
            format!(
                "{percent:>3}% {}/{} {} {elapsed}",
                progress.completed, total, progress.current
            )
        }
        _ => format!(
            "{} check(s), {} {elapsed}",
            progress.completed, progress.current
        ),
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let value = bytes as f64;
    if value >= GIB {
        format!("{:.1} GiB", value / GIB)
    } else if value >= MIB {
        format!("{:.1} MiB", value / MIB)
    } else if value >= KIB {
        format!("{:.1} KiB", value / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn help() -> HelpDoc {
    HelpDoc::new("kernup")
        .summary("Kern SDK installer.")
        .usage("kernup <command> [options]")
        .section(
            HelpSection::new("Commands")
                .entry("install", "Install a Kern SDK archive")
                .entry("doctor", "Validate the active SDK installation")
                .entry("doc", "Print the path to installed Kern documentation")
                .entry("target", "Print the current host archive target")
                .entry("help", "Show this help text"),
        )
        .example(
            "kernup install --archive ./kern-v0.8.2-x86_64-linux-gnu.tar.gz",
            "install a local SDK archive",
        )
        .example(
            "kernup install --version v0.8.2",
            "download and install a release SDK",
        )
        .example("kernup doctor", "verify the default installation")
        .example("kernup doc tutorial", "locate the installed tutorial")
        .note("kernup installs SDK archives only; it does not build Kern from source.")
        .note("For source builds, configure the host LLVM development environment and run Cargo directly.")
}

fn install_help() -> HelpDoc {
    HelpDoc::new("kernup install")
        .summary("Install a Kern SDK release.")
        .usage("kernup install [--version <tag>] [--archive <path>] [--dest <path>] [--target <target>] [--no-path]")
        .section(
            HelpSection::new("Options")
                .entry("--version <tag>", "release tag; defaults to the latest GitHub release")
                .entry("--archive <path>", "local SDK archive to install")
                .entry(
                    "--dest <path>",
                    "installation directory; defaults to ~/.kern",
                )
                .entry(
                    "--target <target>",
                    "host target label; defaults to the current host",
                )
                .entry("--github-repo <repo>", "GitHub repository for release downloads")
                .entry("--no-path", "skip PATH configuration")
                .entry("-v, --verbose", "print command-level installation details"),
        )
        .note("This command installs release SDK archives; it is not a source-build command.")
}

fn doctor_help() -> HelpDoc {
    HelpDoc::new("kernup doctor")
        .summary("Validate a Kern SDK installation.")
        .usage("kernup doctor [--dest <path>]")
        .section(
            HelpSection::new("Options")
                .entry(
                    "--dest <path>",
                    "installation directory; defaults to ~/.kern",
                )
                .entry("-v, --verbose", "print command-level validation details"),
        )
}

fn doc_help() -> HelpDoc {
    HelpDoc::new("kernup doc")
        .summary("Locate installed Kern documentation.")
        .usage("kernup doc [topic] [--path] [--dest <path>]")
        .section(
            HelpSection::new("Options")
                .entry("--path", "print only the resolved documentation path")
                .entry(
                    "--dest <path>",
                    "installation directory; defaults to ~/.kern",
                ),
        )
        .section(
            HelpSection::new("Topics")
                .entry("index", "documentation map")
                .entry("tutorial", "English tutorial")
                .entry("tutorial-zh", "Simplified Chinese tutorial")
                .entry("design", "language design reference")
                .entry("kernc", "compiler guide")
                .entry("craft", "package and build guide")
                .entry("install", "installation guide"),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_verbose_before_or_after_command() {
        match parse_args(vec!["-v".into(), "doctor".into()]).unwrap() {
            Command::Doctor(args) => assert!(args.verbose),
            _ => panic!("expected doctor command"),
        }
        match parse_args(vec!["install".into(), "--verbose".into()]).unwrap() {
            Command::Install(args) => assert!(args.verbose),
            _ => panic!("expected install command"),
        }
    }

    #[test]
    fn parses_doc_topic_and_path_options() {
        match parse_args(vec![
            "doc".into(),
            "tutorial".into(),
            "--path".into(),
            "--dest".into(),
            "/tmp/kern-sdk".into(),
        ])
        .unwrap()
        {
            Command::Doc(args) => {
                assert_eq!(args.topic.as_deref(), Some("tutorial"));
                assert!(args.path_only);
                assert_eq!(
                    args.dest.as_deref(),
                    Some(std::path::Path::new("/tmp/kern-sdk"))
                );
            }
            _ => panic!("expected doc command"),
        }
    }

    #[test]
    fn maps_doc_topics_to_installed_markdown() {
        assert_eq!(doc_topic_path(None).unwrap(), shared_ops::SDK_DOC_ENTRY);
        assert_eq!(doc_topic_path(Some("craft")).unwrap(), "docs/craft.md");
        assert_eq!(
            doc_topic_path(Some("tutorial-zh")).unwrap(),
            "docs/tutorial/zh/README.md"
        );
        assert!(doc_topic_path(Some("missing")).is_err());
    }

    #[test]
    fn install_step_count_tracks_download_and_path_steps() {
        let mut args = InstallArgs::default();
        assert_eq!(install_step_count(&args), 7);
        args.version = Some("v0.8.2".into());
        assert_eq!(install_step_count(&args), 6);
        args.archive = Some(PathBuf::from("kern.tar.gz"));
        assert_eq!(install_step_count(&args), 6);
        args.no_path = true;
        assert_eq!(install_step_count(&args), 5);
    }

    #[test]
    fn renders_progress_bar_like_craft() {
        assert_eq!(render_progress_bar(0, 4, 6), "[>-----]");
        assert_eq!(render_progress_bar(2, 4, 6), "[===>--]");
        assert_eq!(render_progress_bar(4, 4, 6), "[======]");
    }
}
