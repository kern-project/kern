use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn run_pass_tests() {
    run_tests("pass", true);
}

#[test]
fn run_fail_tests() {
    run_tests("fail", false);
}

fn run_tests(dir_name: &str, expect_success: bool) {
    let compiler_path = env!("CARGO_BIN_EXE_kernc");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let test_dir = Path::new(manifest_dir).join("../tests").join(dir_name);

    // 如果目录还不存在，直接跳过，防止报错
    if !test_dir.exists() {
        return; 
    }

    let entries = fs::read_dir(test_dir).expect("Failed to read test directory");

    for entry in entries {
        let entry = entry.expect("Failed to read directory entry");
        let path = entry.path();
        
        if path.extension().and_then(|s| s.to_str()) != Some("kn") {
            continue;
        }

        let content = fs::read_to_string(&path).expect("Failed to read test file");
        let mut compile_flags = Vec::new();
        let mut expected_stdout = String::new();
        let mut expected_error = String::new();
        let mut build_only = false;

        for line in content.lines() {
            let line = line.trim();
            if let Some(flags) = line.strip_prefix("// compile-flags: ") {
                compile_flags.extend(flags.split_whitespace().map(|s| s.to_string()));
            } else if let Some(out) = line.strip_prefix("// expected-stdout: ") {
                expected_stdout.push_str(out);
                expected_stdout.push('\n');
            } else if let Some(err) = line.strip_prefix("// expected-error: ") {
                expected_error.push_str(err);
            } else if line == "// build-only" {
                build_only = true;
            } else if !line.starts_with("//") {
                break;
            }
        }

        let out_bin = std::env::temp_dir().join(path.file_stem().unwrap()).with_extension("out");
        
        let mut cmd = Command::new(compiler_path);
        cmd.arg(&path).arg("-o").arg(&out_bin);
        for flag in compile_flags {
            cmd.arg(flag);
        }

        // 读取 stderr
        let compile_output = cmd.output().expect("Failed to execute compiler");
        
        if expect_success {
            assert!(
                compile_output.status.success(),
                "Compilation failed for pass test: {:?}\nStderr: {}",
                path,
                String::from_utf8_lossy(&compile_output.stderr)
            );

            if !build_only {
                let run_output = Command::new(&out_bin)
                    .output()
                    .expect("Failed to run the compiled binary");

                assert!(
                    run_output.status.success(),
                    "Execution crashed or returned non-zero for pass test: {:?}",
                    path
                );

                if !expected_stdout.is_empty() {
                    let actual_stdout = String::from_utf8_lossy(&run_output.stdout);
                    assert_eq!(
                        actual_stdout.trim(),
                        expected_stdout.trim(),
                        "Stdout mismatch in pass test: {:?}",
                        path
                    );
                }
            }
        } else {
            // Fail 测试逻辑：断言编译状态为失败
            assert!(
                !compile_output.status.success(),
                "Compilation unexpectedly succeeded for fail test: {:?}",
                path
            );

            // 断言错误信息匹配
            if !expected_error.is_empty() {
                let actual_stderr = String::from_utf8_lossy(&compile_output.stderr);
                assert!(
                    actual_stderr.contains(expected_error.trim()),
                    "Expected error message not found in fail test: {:?}\nExpected: {}\nActual: {}",
                    path,
                    expected_error,
                    actual_stderr
                );
            }
        }

        let _ = fs::remove_file(out_bin);
    }
}