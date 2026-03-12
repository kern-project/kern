use kernc::driver::CompilerDriver;
use kernc::driver::config::{CompileOptions, OptLevel, TargetMachine, AsmDialect};
use std::env;
use std::process;

fn print_usage(program_name: &str) {
    println!("Usage: {} [options] <input.kn>", program_name);
    println!("Options:");
    println!("  -o <file>      Write output to <file>");
    println!("  -O0, -O1, -O2, -O3  Set optimization level");
    println!("  --emit-llvm    Print LLVM IR to stdout");
    println!("  --target <T>   Set target triple (e.g. x86_64-unknown-linux-gnu)");
    println!("  --asm-dialect <intel|att> Set inline assembly dialect (default: intel)"); 
    println!("  -v, --version  Display version information and exit");
    println!("  -h, --help     Display this help and exit");
}

fn parse_args() -> CompileOptions {
    let mut args = env::args();
    let program_name = args.next().unwrap_or_else(|| "kernc".to_string());

    let mut options = CompileOptions::default();
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
            "-o" => {
                options.output_file = args.next().unwrap_or_else(|| {
                    eprintln!("Error: Expected file name after `-o`");
                    process::exit(1);
                });
            }
            "-O0" => options.opt_level = OptLevel::O0,
            "-O1" => options.opt_level = OptLevel::O1,
            "-O2" => options.opt_level = OptLevel::O2,
            "-O3" => options.opt_level = OptLevel::O3,
            "--emit-llvm" => options.emit_llvm_ir = true,
            "--target" => {
                let triple_str = args.next().unwrap_or_else(|| {
                    eprintln!("Error: Expected target triple after `--target`");
                    process::exit(1);
                });
                options.target = TargetMachine::new(&triple_str).unwrap_or_else(|e| {
                    eprintln!("Error: Invalid target triple: {}", e);
                    process::exit(1);
                });
            }
            "--asm-dialect" => {
                let dialect_str = args.next().unwrap_or_else(|| {
                    eprintln!("Error: Expected dialect after `--asm-dialect`");
                    process::exit(1);
                });
                options.asm_dialect = match dialect_str.as_str() {
                    "intel" => AsmDialect::Intel,
                    "att" => AsmDialect::Att,
                    _ => {
                        eprintln!("Error: Invalid asm dialect `{}`. Expected `intel` or `att`.", dialect_str);
                        process::exit(1);
                    }
                };
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

    options
}

fn main() {
    let options = parse_args();
    let driver = CompilerDriver::new(options);

    if !driver.compile() {
        process::exit(1);
    }
}
