use kernc::driver::CompilerDriver;
use kernc::driver::config::{AsmDialect, CompileOptions, OptLevel, TargetMachine};
use std::env;
use std::path::PathBuf;
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

    // 自动推导标准库路径
    if !options.module_aliases.contains_key("std") {
        let std_path = if let Ok(custom_std) = env::var("KERN_STD_PATH") {
            // 1. 最高优先级：用户指定的环境变量 (极其适合本地开发测试)
            PathBuf::from(custom_std)
        } else if let Ok(mut exe_path) = env::current_exe() {
            // 2. 标准工具链相对路径解析
            // 假设 exe 位于 /usr/local/kern/bin/kernc
            exe_path.pop(); // 弹出 kernc -> 得到 bin/

            // 检查是不是在 cargo run 的 target/debug/ 目录下
            if exe_path.ends_with("debug") || exe_path.ends_with("release") {
                // 如果是开发环境，退回到项目根目录去找 library/std
                exe_path.pop(); // 弹出 debug
                exe_path.pop(); // 弹出 target
                exe_path.join("library/std")
            } else {
                // 生产环境工具链：弹出 bin/，进入 lib/kern/std/
                exe_path.pop();
                exe_path.join("lib/kern/std")
            }
        } else {
            // 3. 兜底方案
            PathBuf::from("library/std")
        };

        // 验证标准库路径是否真的存在（提供友好的错误提示）
        if !std_path.exists() {
            eprintln!(
                "Warning: Standard library not found at `{}`. \n\
                 Please set the `KERN_STD_PATH` environment variable or ensure the compiler is installed in a valid toolchain directory.",
                std_path.display()
            );
        }

        options
            .module_aliases
            .insert("std".to_string(), std_path.to_string_lossy().to_string());
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
