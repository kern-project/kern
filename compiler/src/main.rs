use kernc::driver::CompilerDriver;
use kernc::driver::config::{AsmDialect, CompileOptions, OptLevel, TargetMachine};
use std::env;
use std::process;

fn print_usage(program_name: &str) {
    let version = env!("CARGO_PKG_VERSION");

    println!("Kern Compiler v{}", version);
    println!("Usage: {} [OPTIONS] <input.kn>\n", program_name);

    println!("Build Options:");
    println!("  -o <file>             Write output to <file>");
    println!("  -D <key=val>          Define a variable for conditional compilation");
    println!(
        "  -M <name=path>        Map a module name to a physical directory (e.g., -M std=./library/std)"
    );
    println!("  -O<0-3>               Set optimization level (default: O0)");

    println!("\nTargeting & Codegen:");
    println!("  --target <T>          Set target triple (e.g. x86_64-unknown-linux-gnu)");
    println!("  --asm-dialect <D>     Set assembly dialect: intel (default) or att");
    println!("  --cc <cmd>            Set the C compiler/linker to use (default: $CC or cc)");
    println!("  --link-libc           Link the C standard library (disabled by default)");
    println!("  --emit-llvm           Print LLVM IR to stdout");

    println!("\nInformation:");
    println!("  -v, --version         Display version information and exit");
    println!("  -h, --help            Display this help and exit");
}

fn parse_args() -> CompileOptions {
    let mut args = env::args();
    let program_name = args.next().unwrap_or_else(|| "kernc".to_string());

    let mut options = CompileOptions::default();

    // 在解析 CLI 参数之前读取环境变量。
    // 这样它的优先级介于 "默认值" 和 "CLI 参数" 之间。
    if let Ok(cc_env) = env::var("CC") {
        options.linker_cmd = cc_env;
    }

    let mut input_file_set = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_usage(&program_name);
                process::exit(0);
            }
            "-v" | "--version" => {
                println!("kernc version {}", env!("CARGO_PKG_VERSION"));
                process::exit(0);
            }
            "-o" => options.output_file = args.next().expect("Expected file name after `-o`"),
            "-O0" => options.opt_level = OptLevel::O0,
            "-O1" => options.opt_level = OptLevel::O1,
            "-O2" => options.opt_level = OptLevel::O2,
            "-O3" => options.opt_level = OptLevel::O3,
            "--emit-llvm" => options.emit_llvm_ir = true,
            "--link-libc" => options.link_libc = true,
            "--target" => {
                let triple_str = args
                    .next()
                    .expect("Expected target triple after `--target`");
                options.target = TargetMachine::new(&triple_str).unwrap_or_else(|e| {
                    eprintln!("Error: Invalid target triple: {}", e);
                    process::exit(1);
                });
            }
            "--asm-dialect" => {
                let dialect_str = args.next().expect("Expected dialect after `--asm-dialect`");
                options.asm_dialect = match dialect_str.as_str() {
                    "intel" => AsmDialect::Intel,
                    "att" => AsmDialect::Att,
                    _ => {
                        eprintln!("Error: Invalid asm dialect");
                        process::exit(1);
                    }
                };
            }
            "--cc" => {
                options.linker_cmd = args.next().expect("Expected command after `--cc`");
            }
            "-D" => {
                let define_str = args.next().expect("Expected `key=value` after `-D`");
                let parts: Vec<&str> = define_str.splitn(2, '=').collect();
                if parts.len() != 2 {
                    eprintln!("Error: Invalid define format. Expected `key=value`.");
                    process::exit(1);
                }
                options
                    .custom_defines
                    .insert(parts[0].to_string(), parts[1].to_string());
            }
            "-M" => {
                let map_str = args.next().expect("Expected `name=path` after `-M`");
                let parts: Vec<&str> = map_str.splitn(2, '=').collect();
                if parts.len() != 2 {
                    eprintln!("Error: Invalid module map format. Expected `name=path`.");
                    process::exit(1);
                }
                options
                    .module_aliases
                    .insert(parts[0].to_string(), parts[1].to_string());
            }
            _ => {
                if arg.starts_with('-') {
                    eprintln!("Error: Unrecognized option `{}`", arg);
                    process::exit(1);
                }
                if input_file_set {
                    eprintln!("Error: Multiple input files are not supported yet.");
                    process::exit(1);
                }
                options.input_file = arg;
                input_file_set = true;
            }
        }
    }

    if !input_file_set {
        eprintln!("Error: No input file specified.");
        print_usage(&program_name);
        process::exit(1);
    }

    // 如果没有手动指定 -M std=...，补齐标准库路径
    if !options.module_aliases.contains_key("std") {
        // 假设编译器运行在项目根目录，标准库在 ./library/std
        options
            .module_aliases
            .insert("std".to_string(), "library/std".to_string());
    }

    options
}

fn main() {
    let options = parse_args();
    let driver = CompilerDriver::new(options);

    if !driver.compile() {
        process::exit(1);
    }
}
