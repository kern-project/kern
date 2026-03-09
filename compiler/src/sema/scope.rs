use super::ty::{DefId, TypeId};
use crate::parser::ast::NodeId;
use crate::utils::{Span, SymbolId};
use std::collections::HashMap;

/// 全局唯一的作用域 ID
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScopeId(pub usize);

/// 符号种类
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Var,       // 变量 (let, param)
    Const,     // 常量
    Static,    // 静态变量
    Function,  // 函数
    Struct,    // 结构体定义
    Enum,      // 枚举定义
    Union,     // 联合体定义
    Adt,       // 代数数据类型定义
    Trait,     // 特征定义
    Module,    // 模块
    TypeAlias, // 类型别名
    TypeParam,
}

/// 符号信息
#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub kind: SymbolKind,
    pub node_id: NodeId,       // 对应的 AST 节点
    pub type_id: TypeId,       // 语义类型
    pub def_id: Option<DefId>, // 指向具体的定义表
    pub span: Span,
    pub is_pub: bool,
}

/// 单层作用域 (持久化结构)
#[derive(Debug)]
pub struct Scope {
    pub id: ScopeId,
    /// 指向父作用域。对于模块的顶层作用域，可能是 None 或者指向全局 Builtin 作用域
    pub parent: Option<ScopeId>,
    pub symbols: HashMap<SymbolId, SymbolInfo>,
}

impl Scope {
    pub fn new(id: ScopeId, parent: Option<ScopeId>) -> Self {
        Self {
            id,
            parent,
            symbols: HashMap::new(),
        }
    }
}

/// 符号表 (Arena & 执行上下文)
pub struct SymbolTable {
    /// 所有的作用域都永久存储在这里 (Arena)
    scopes: Vec<Scope>,
    /// 当前正在遍历的作用域节点
    current_scope: Option<ScopeId>,
}

impl SymbolTable {
    pub fn new() -> Self {
        let mut table = Self {
            scopes: Vec::new(),
            current_scope: None,
        };
        // 默认初始化一个全局根作用域 (可以放内置类型、函数)
        let root_id = table.create_scope(None);
        table.current_scope = Some(root_id);
        table
    }

    /// 在底层的 Arena 中创建一个新的作用域，但不改变当前上下文
    fn create_scope(&mut self, parent: Option<ScopeId>) -> ScopeId {
        let id = ScopeId(self.scopes.len());
        self.scopes.push(Scope::new(id, parent));
        id
    }

    /// 进入一个新的块级作用域（将其父节点设为当前作用域）
    /// 返回创建的 ScopeId，方便在 Collect 阶段将其绑定到模块上
    pub fn enter_scope(&mut self) -> ScopeId {
        let new_id = self.create_scope(self.current_scope);
        self.current_scope = Some(new_id);
        new_id
    }

    /// 离开当前作用域，回退到父作用域
    pub fn exit_scope(&mut self) {
        if let Some(current) = self.current_scope {
            // 获取当前作用域的父节点，并将其设为 current
            self.current_scope = self.scopes[current.0].parent;
        } else {
            panic!("Cannot exit scope: current scope is already None!");
        }
    }

    /// 强制跳转到指定的作用域 (在 Typecheck 阶段查阅模块时非常有用)
    pub fn set_current_scope(&mut self, scope_id: ScopeId) {
        self.current_scope = Some(scope_id);
    }

    /// 获取当前作用域的 ID
    pub fn current_scope_id(&self) -> Option<ScopeId> {
        self.current_scope
    }

    /// 在当前作用域定义符号。
    /// 如果成功，返回 Ok(())。
    /// 如果失败（发生同名冲突），返回 Err(旧的 SymbolInfo)，以便调用者可以报出极具建设性的错误
    pub fn define(&mut self, name: SymbolId, info: SymbolInfo) -> Result<(), SymbolInfo> {
        let current_id = self
            .current_scope
            .expect("No active scope to define symbol");
        let current_scope = &mut self.scopes[current_id.0];

        if let Some(existing) = current_scope.symbols.get(&name) {
            // 返回旧变量的信息，方便报错时指出 "previous definition was here"
            return Err(existing.clone());
        }
        current_scope.symbols.insert(name, info);
        Ok(())
    }

    /// 查找符号 (沿 Scope Tree 向上追溯)
    pub fn resolve(&self, name: SymbolId) -> Option<&SymbolInfo> {
        let mut curr = self.current_scope;

        while let Some(id) = curr {
            let scope = &self.scopes[id.0];
            if let Some(info) = scope.symbols.get(&name) {
                return Some(info);
            }
            curr = scope.parent; // 向上找
        }
        None
    }

    /// 仅在当前作用域查找 (用于检查重定义)
    pub fn resolve_local(&self, name: SymbolId) -> Option<&SymbolInfo> {
        let current_id = self.current_scope?;
        self.scopes[current_id.0].symbols.get(&name)
    }

    /// 跨模块查询：直接在指定的 Scope 中查找符号 (不向上追溯)
    /// 用于解析 `use std.math.add` 时，在 `math` 模块的 Scope 中精准查找 `add`
    pub fn resolve_in(&self, scope_id: ScopeId, name: SymbolId) -> Option<&SymbolInfo> {
        self.scopes[scope_id.0].symbols.get(&name)
    }

    /// 更新已存在符号的类型 (用于 let 绑定的类型推导回填)
    pub fn update_type(&mut self, name: SymbolId, ty: TypeId) {
        let mut curr = self.current_scope;

        // 沿作用域链向上找，找到在哪定义的，就更新哪里的 info
        while let Some(id) = curr {
            let scope = &mut self.scopes[id.0];
            if let Some(info) = scope.symbols.get_mut(&name) {
                info.type_id = ty;
                return;
            }
            curr = scope.parent;
        }
    }
}
