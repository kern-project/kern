use crate::driver::context::Context;
use crate::parser::Parser;
use crate::parser::ast;
use crate::sema::def::{Def, ModuleDef};
use crate::sema::ty::DefId;
use crate::utils::SymbolId;
use std::collections::HashMap;
use std::path::PathBuf;

pub struct ModuleLoader<'a> {
    pub ctx: &'a mut Context,
    // 避免循环依赖：物理绝对路径 -> 模块 ID
    pub loaded_files: HashMap<PathBuf, DefId>,
    // 暂存所有解析好的 AST，等待下一阶段 Collector 提取符号
    pub asts: Vec<(DefId, ast::Module)>,
}

impl<'a> ModuleLoader<'a> {
    pub fn new(ctx: &'a mut Context) -> Self {
        Self {
            ctx,
            loaded_files: HashMap::new(),
            asts: Vec::new(),
        }
    }

    /// 入口：加载主文件
    pub fn load_root(&mut self, root_file: &str) -> Option<DefId> {
        let path = PathBuf::from(root_file);
        let name = self.ctx.intern("root");
        let root_id = self.load_module(path, None, name);

        // 预加载所有通过 -M 注入的外部别名包
        let aliases = self.ctx.module_aliases.clone();
        for (alias_name, alias_path) in aliases {
            let sym = self.ctx.intern(&alias_name);
            let base_path = PathBuf::from(&alias_path);

            let dir_init = base_path.join("init.kn");
            let file_kn = PathBuf::from(format!("{}.kn", base_path.display()));

            let sub_path = if dir_init.exists() {
                Some(dir_init)
            } else if file_kn.exists() {
                Some(file_kn)
            } else if base_path.exists() && base_path.is_file() {
                Some(base_path)
            } else {
                eprintln!(
                    "Error: Cannot find module path for alias `{}` at `{}`",
                    alias_name, alias_path
                );
                None
            };

            if let Some(p) = sub_path {
                // 作为顶级独立模块加载 (parent 传入 None)
                if let Some(mod_id) = self.load_module(p, None, sym) {
                    // 将其 DefId 存入全局外部包注册表
                    self.ctx.alias_roots.insert(sym, mod_id);
                }
            }
        }

        root_id
    }

    /// 核心算法：递归按需加载
    fn load_module(
        &mut self,
        path: PathBuf,
        parent: Option<DefId>,
        name: SymbolId,
    ) -> Option<DefId> {
        let abs_path = std::fs::canonicalize(&path).unwrap_or(path.clone());

        // 1. 查重，防止循环引用 (A 引用 B，B 引用 A)
        if let Some(&mod_id) = self.loaded_files.get(&abs_path) {
            return Some(mod_id);
        }

        // 2. 分配模块 ID 并预注册，保证后续循环引用的安全
        let mod_id = DefId(self.ctx.defs.len() as u32);
        self.loaded_files.insert(abs_path.clone(), mod_id);

        let src = match std::fs::read_to_string(&abs_path) {
            Ok(s) => s,
            Err(e) => {
                // 不仅打印，还要通知 Context 出现了致命错误
                self.ctx.error_count += 1;
                eprintln!(
                    "Error: Cannot read module file '{}': {}",
                    abs_path.display(),
                    e
                );
                return None;
            }
        };

        // 获取文件所在目录作为模块的基准查找路径
        let dir_path = abs_path.parent().unwrap().to_path_buf();

        let file_id = self
            .ctx
            .source_manager
            .add_file(abs_path.to_string_lossy().to_string(), src.clone());

        // 预创建一个空的 ModuleDef，提前分配 Scope
        let scope_id = self.ctx.scopes.enter_scope();
        self.ctx.scopes.exit_scope();

        let is_init = abs_path.file_name().and_then(|n| n.to_str()) == Some("init.kn");

        let dummy_def = ModuleDef {
            id: mod_id,
            name,
            parent,
            scope_id,
            dir_path: dir_path.clone(),
            file_id,
            is_init,
            submodules: HashMap::new(),
            items: Vec::new(),
            imports: Vec::new(),
        };
        self.ctx.add_def(Def::Module(dummy_def));

        // 3. 执行语法解析
        let mut parser = Parser::new(&src, file_id, self.ctx);
        let mut ast = match parser.parse_module() {
            Ok(ast) => ast,
            Err(_) => return None,
        };
        ast.path = abs_path.to_string_lossy().to_string();

        // 在扫描子模块之前，对当前 AST 进行就地剪枝
        // 这样带有 false 条件的 #[if(...)] mod xxx; 就会被直接删除，不会触发后续的物理加载
        let mut pruner = crate::sema::prune::Pruner::new(self.ctx);
        pruner.prune_module(&mut ast);

        let mut submodules = HashMap::new();

        // 4. 扫描显式的 `mod xxx;` 声明来挂载子模块
        for decl in &ast.decls {
            if let ast::DeclKind::ModDecl { .. } = &decl.kind {
                let mod_name_str = self.ctx.resolve(decl.name);

                // 内部子模块直接去物理目录下找
                let dir_init = dir_path.join(mod_name_str).join("init.kn");
                let file_kn = dir_path.join(format!("{}.kn", mod_name_str));

                let sub_path = if dir_init.exists() {
                    Some(dir_init)
                } else if file_kn.exists() {
                    Some(file_kn)
                } else {
                    self.ctx
                        .struct_error(
                            decl.span,
                            format!("Cannot find module file for `{}`", mod_name_str),
                        )
                        .with_hint(format!(
                            "expected to find `{}` or `{}`",
                            file_kn.display(),
                            dir_init.display()
                        ))
                        .emit();
                    None
                };

                if let Some(p) = sub_path {
                    if let Some(sub_id) = self.load_module(p, Some(mod_id), decl.name) {
                        submodules.insert(decl.name, sub_id);
                    }
                }
            }
        }

        // 5. 更新预注册的模块信息
        if let Def::Module(m) = &mut self.ctx.defs[mod_id.0 as usize] {
            m.submodules = submodules;
        }

        // 6. 将 AST 存档，供后续阶段提取符号
        self.asts.push((mod_id, ast));

        Some(mod_id)
    }
}
