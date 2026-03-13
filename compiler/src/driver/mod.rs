pub mod config;
pub mod context;
pub mod diagnostic;

pub use context::Context;

use crate::codegen::llvm::CodeGenerator;
use crate::mast::lower::Lowerer;
use crate::sema::builtin::BuiltinInjector;
use crate::sema::collect::Collector;
use crate::sema::resolve_imports::ImportResolver;
use crate::sema::resolve_types::TypeResolver;
use crate::sema::typeck::TypeckDriver;
use config::CompileOptions;
use inkwell::context::Context as LlvmContext;

use std::fs;

pub struct CompilerDriver {
    pub options: CompileOptions,
}

/// 临时文件守卫 (RAII)
/// 当变量离开作用域时，自动删除产生的临时文件
struct TempFileGuard {
    path: String,
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl CompilerDriver {
    pub fn new(options: CompileOptions) -> Self {
        Self { options }
    }

    pub fn compile(&self) -> bool {
        let mut ctx = Context::new();
        ctx.apply_options(&self.options);

        let source_code = match fs::read_to_string(&self.options.input_file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Error: Cannot read input file '{}': {}", self.options.input_file, e);
                return false;
            }
        };

        let _ = ctx.source_manager.add_file(self.options.input_file.clone(), source_code);

        let mut builtin = BuiltinInjector::new(&mut ctx);
        builtin.inject();

        let mut asts = {
            let mut loader = crate::sema::module_loader::ModuleLoader::new(&mut ctx);
            loader.load_root(&self.options.input_file);
            std::mem::take(&mut loader.asts)
        };

        if ctx.has_errors() { ctx.print_diagnostics(); return false; }

        let mut pruner = crate::sema::prune::Pruner::new(&mut ctx);
        pruner.prune_all(&mut asts);
        if ctx.has_errors() { ctx.print_diagnostics(); return false; }

        let mut collector = Collector::new(&mut ctx);
        for (mod_id, ast) in asts {
            collector.collect_ast(mod_id, &ast);
        }
        if ctx.has_errors() { ctx.print_diagnostics(); return false; }

        let mut import_resolver = ImportResolver::new(&mut ctx);
        import_resolver.resolve_all();
        if ctx.has_errors() { ctx.print_diagnostics(); return false; }

        let mut type_resolver = TypeResolver::new(&mut ctx);
        type_resolver.resolve_all();
        if ctx.has_errors() { ctx.print_diagnostics(); return false; }

        let mut typeck = TypeckDriver::new(&mut ctx);
        typeck.check_all();
        if ctx.has_errors() { ctx.print_diagnostics(); return false; }

        let mut lowerer = Lowerer::new(&mut ctx);
        let mast_module = lowerer.lower_all();

        let llvm_ctx = LlvmContext::create();
        let mod_name = std::path::Path::new(&self.options.input_file)
            .file_stem()
            .unwrap_or_default()
            .to_str()
            .unwrap_or("kern_module");

        let resolve_fn = |sym| ctx.resolve(sym);
        let mut codegen = CodeGenerator::new(
            &llvm_ctx,
            mod_name,
            &ctx.type_registry,
            &ctx.defs,
            &resolve_fn,
        );

        codegen.asm_dialect = match self.options.asm_dialect {
            config::AsmDialect::Intel => inkwell::InlineAsmDialect::Intel,
            config::AsmDialect::Att => inkwell::InlineAsmDialect::ATT,
        };

        codegen.compile(&mast_module);

        if self.options.emit_llvm_ir {
            codegen.print_ir();
            return true;
        }

        let obj_path_str = format!("{}.tmp.o", self.options.output_file);
        
        // 使用 RAII 守卫保护临时文件，无论是 Err 提前返回还是 CC 失败，都会自动清理
        let _guard = TempFileGuard { path: obj_path_str.clone() };

        if let Err(e) = codegen.emit_to_file(
            &self.options.target.triple.to_string(), 
            &obj_path_str,
            self.options.opt_level
        ) {
            eprintln!("Error: LLVM failed to generate object file: {}", e);
            return false;
        }

        println!("Linking...");
        
        // 支持通过环境变量指定自定义交叉编译器 (如 CC=x86_64-elf-gcc)
        let cc_compiler = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
        let mut cmd = std::process::Command::new(&cc_compiler);
        
        cmd.arg(&obj_path_str)
            .arg("-no-pie")
            .arg("-o")
            .arg(&self.options.output_file);

        if self.options.freestanding {
            cmd.arg("-nostdlib");
        }

        match cmd.status() {
            Ok(s) if s.success() => {
                println!("Successfully compiled to `{}`", self.options.output_file);
                true // _guard 离开作用域，tmp.o 被静默删除
            }
            Ok(s) => {
                eprintln!("Error: Linker failed with exit code {}", s);
                false
            }
            Err(e) => {
                eprintln!(
                    "Error: Failed to invoke linker (`{}`). Make sure a C compiler is installed. ({})",
                    cc_compiler, e
                );
                false
            }
        }
    }
}