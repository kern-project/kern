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

impl CompilerDriver {
    pub fn new(options: CompileOptions) -> Self {
        Self { options }
    }

    /// 执行完整编译流程，返回 true 表示成功，false 表示失败
    pub fn compile(&self) -> bool {
        // 1. 初始化上下文并注入配置
        let mut ctx = Context::new();
        ctx.target = self.options.target.clone();

        // 2. 读取源代码
        let source_code = match fs::read_to_string(&self.options.input_file) {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "Error: Cannot read input file '{}': {}",
                    self.options.input_file, e
                );
                return false;
            }
        };

        let _ = ctx
            .source_manager
            .add_file(self.options.input_file.clone(), source_code.clone());

        // 3. 注入内置宏与特性
        let mut builtin = BuiltinInjector::new(&mut ctx);
        builtin.inject();

        // 4. 智能按需模块加载
        let mut asts = {
            let mut loader = crate::sema::module_loader::ModuleLoader::new(&mut ctx);
            loader.load_root(&self.options.input_file);
            std::mem::take(&mut loader.asts)
        };

        if ctx.has_errors() {
            ctx.print_diagnostics();
            return false;
        }

        // 4.5. AST 条件剪枝 (Prune Pass)
        // 在任何语义分析开始之前，剔除所有平台不匹配的代码
        let mut pruner = crate::sema::prune::Pruner::new(&mut ctx);
        pruner.prune_all(&mut asts);

        if ctx.has_errors() {
            ctx.print_diagnostics();
            return false;
        }

        // 5. 符号收集：遍历 Loader 解析出的所有 AST
        let mut collector = Collector::new(&mut ctx);
        for (mod_id, ast) in asts {
            collector.collect_ast(mod_id, &ast);
        }

        if ctx.has_errors() {
            ctx.print_diagnostics();
            return false;
        }

        // 6. 语义分析 Pass 2: 模块导入解析
        let mut import_resolver = ImportResolver::new(&mut ctx);
        import_resolver.resolve_all();
        if ctx.has_errors() {
            ctx.print_diagnostics();
            return false;
        }

        // 7. 语义分析 Pass 3: 类型解析
        let mut type_resolver = TypeResolver::new(&mut ctx);
        type_resolver.resolve_all();
        if ctx.has_errors() {
            ctx.print_diagnostics();
            return false;
        }

        // 8. 语义分析 Pass 4: 类型检查与推导
        let mut typeck = TypeckDriver::new(&mut ctx);
        typeck.check_all();
        if ctx.has_errors() {
            ctx.print_diagnostics();
            return false;
        }

        // 9. MAST 降级与单态化
        let mut lowerer = Lowerer::new(&mut ctx);
        let mast_module = lowerer.lower_all();

        // 10. LLVM 代码生成
        let llvm_ctx = LlvmContext::create();
        // 取文件名作为 module 名字
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

        // 🌟 将配置层的枚举映射到 LLVM 的枚举
        codegen.asm_dialect = match self.options.asm_dialect {
            config::AsmDialect::Intel => inkwell::InlineAsmDialect::Intel,
            config::AsmDialect::Att => inkwell::InlineAsmDialect::ATT,
        };

        codegen.compile(&mast_module);

        if self.options.emit_llvm_ir {
            codegen.print_ir();
            return true; // 如果只打印 IR，就不需要走后续的二进制生成了
        }

        // 决定临时 .o 文件的路径 (不要用 with_extension，直接加后缀)
        let obj_path_str = format!("{}.tmp.o", self.options.output_file);

        // 1. 调用 emit_to_file 生成临时的 .o 文件
        if let Err(e) = codegen.emit_to_file(&self.options.target.triple.to_string(), &obj_path_str)
        {
            eprintln!("Error: LLVM failed to generate object file: {}", e);
            return false;
        }

        // 2. 调用 cc 进行链接
        println!("Linking...");
        let mut cmd = std::process::Command::new("cc");
        cmd.arg(&obj_path_str)
            .arg("-no-pie")
            .arg("-o")
            .arg(&self.options.output_file);

        // 如果是 freestanding 模式，才不链接标准库
        if self.options.freestanding {
            cmd.arg("-nostdlib");
        }

        let status = cmd.status();

        match status {
            Ok(s) if s.success() => {
                // 链接成功后，把临时文件删掉
                let _ = std::fs::remove_file(&obj_path_str);
                println!("Successfully compiled to `{}`", self.options.output_file);
                true
            }
            Ok(s) => {
                eprintln!("Error: Linker failed with exit code {}", s);
                false
            }
            Err(e) => {
                eprintln!(
                    "Error: Failed to invoke linker (`cc`). Make sure a C compiler is installed. ({})",
                    e
                );
                false
            }
        }
    }
}
