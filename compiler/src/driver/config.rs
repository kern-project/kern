// src/config.rs
use std::collections::HashMap;
use std::str::FromStr;
use target_lexicon::{PointerWidth, Triple};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptLevel {
    O0,
    O1,
    O2,
    O3,
}

#[derive(Debug, Clone)]
pub struct TargetMachine {
    pub triple: Triple,
    pub pointer_size: u64, // 提前提取出来，方便 Sema 计算大小
}

impl TargetMachine {
    /// 从字符串解析目标架构 (例如 "x86_64-unknown-linux-gnu")
    pub fn new(triple_str: &str) -> Result<Self, String> {
        let triple = Triple::from_str(triple_str).map_err(|e| e.to_string())?;

        let pointer_size = match triple.pointer_width() {
            Ok(PointerWidth::U16) => 2,
            Ok(PointerWidth::U32) => 4,
            Ok(PointerWidth::U64) => 8,
            Err(_) => 8, // 默认 fallback 到 64-bit
        };

        Ok(Self {
            triple,
            pointer_size,
        })
    }
}

impl Default for TargetMachine {
    fn default() -> Self {
        // 默认使用当前宿主机的架构
        let triple = Triple::host();
        let pointer_size = match triple.pointer_width() {
            Ok(PointerWidth::U16) => 2,
            Ok(PointerWidth::U32) => 4,
            Ok(PointerWidth::U64) => 8,
            Err(_) => 8,
        };
        Self {
            triple,
            pointer_size,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsmDialect {
    Intel,
    Att,
}

impl Default for AsmDialect {
    fn default() -> Self {
        AsmDialect::Intel // 默认使用 Intel
    }
}

#[derive(Debug, Clone)]
pub struct CompileOptions {
    pub input_file: String,
    pub output_file: String,
    pub target: TargetMachine,
    pub opt_level: OptLevel,
    pub emit_llvm_ir: bool,
    /// 允许通过 CLI 传入自定义环境变量，例如 kernc -D my_feature=true
    pub custom_defines: HashMap<String, String>,
    // 模块别名映射表
    pub module_aliases: HashMap<String, String>,
    pub asm_dialect: AsmDialect,
    pub link_libc: bool,
    pub linker_cmd: String,
}

impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            input_file: String::new(),
            output_file: "a.out".to_string(),
            target: TargetMachine::default(),
            opt_level: OptLevel::O0,
            emit_llvm_ir: false,
            custom_defines: HashMap::new(),
            module_aliases: HashMap::new(),
            asm_dialect: AsmDialect::default(),
            link_libc: false,
            linker_cmd: "cc".to_string(),
        }
    }
}
