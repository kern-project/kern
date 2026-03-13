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
        self.load_module(path, None, name)
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

        let dummy_def = ModuleDef {
            id: mod_id,
            name,
            parent,
            scope_id,
            dir_path: dir_path.clone(),
            file_id,
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

        let mut submodules = HashMap::new();

        // 4. 扫描文件内部的 `use .xxx`，按需去文件系统寻找子模块
        for decl in &ast.decls {
            if let ast::DeclKind::Use {
                kind,
                path: use_path,
                ..
            } = &decl.kind
            {
                if *kind == ast::UsePathKind::Relative {
                    if let Some(&first_seg) = use_path.first() {
                        if !submodules.contains_key(&first_seg) {
                            let mod_name_str = self.ctx.resolve(first_seg);

                            // Kern 路径嗅探规则：优先找 mod_name/init.kn，其次找 mod_name.kn
                            let dir_init = dir_path.join(mod_name_str).join("init.kn");
                            let file_kn = dir_path.join(format!("{}.kn", mod_name_str));

                            let sub_path = if dir_init.exists() {
                                Some(dir_init)
                            } else if file_kn.exists() {
                                Some(file_kn)
                            } else {
                                None
                            };

                            // 如果找到了物理文件，立刻递归加载它，并挂载到当前模块的 submodules 树上
                            if let Some(p) = sub_path {
                                if let Some(sub_id) = self.load_module(p, Some(mod_id), first_seg) {
                                    submodules.insert(first_seg, sub_id);
                                }
                            }
                        }
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
