use super::token::TokenType;
use crate::utils::{Span, SymbolId};

// ==========================================
//               Node ID
// ==========================================

/// 节点 ID，用于在 AST 列表中索引
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    pub fn to_usize(self) -> usize {
        self.0 as usize
    }
}

// ==========================================
//               Operators
// ==========================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOperator {
    Add,            // +
    Subtract,       // -
    Multiply,       // *
    Divide,         // /
    Modulo,         // %
    Equal,          // ==
    NotEqual,       // !=
    LessThan,       // <
    GreaterThan,    // >
    LessOrEqual,    // <=
    GreaterOrEqual, // >=
    LogicalAnd,     // and
    LogicalOr,      // or
    BitwiseAnd,     // &
    BitwiseOr,      // |
    BitwiseXor,     // ^
    ShiftLeft,      // <<
    ShiftRight,     // >>
}

impl BinaryOperator {
    pub fn from_token(token: TokenType) -> Self {
        match token {
            TokenType::Plus => Self::Add,
            TokenType::Minus => Self::Subtract,
            TokenType::Star => Self::Multiply,
            TokenType::Slash => Self::Divide,
            TokenType::Percent => Self::Modulo,
            TokenType::EqualEqual => Self::Equal,
            TokenType::NotEqual => Self::NotEqual,
            TokenType::LessThan => Self::LessThan,
            TokenType::GreaterThan => Self::GreaterThan,
            TokenType::LessEqual => Self::LessOrEqual,
            TokenType::GreaterEqual => Self::GreaterOrEqual,
            TokenType::And => Self::LogicalAnd,
            TokenType::Or => Self::LogicalOr,
            TokenType::Ampersand => Self::BitwiseAnd,
            TokenType::Pipe => Self::BitwiseOr,
            TokenType::Caret => Self::BitwiseXor,
            TokenType::LShift => Self::ShiftLeft,
            TokenType::RShift => Self::ShiftRight,
            _ => unreachable!("Token {:?} is not a binary operator", token),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOperator {
    Negate,       // -
    LogicalNot,   // !
    BitwiseNot,   // ~
    AddressOf,    // .&
    LengthOf,     // #
    PointerDeRef, // .*
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignmentOperator {
    Assign,           // =
    AddAssign,        // +=
    SubtractAssign,   // -=
    MultiplyAssign,   // *=
    DivideAssign,     // /=
    ModuloAssign,     // %=
    BitwiseAndAssign, // &=
    BitwiseOrAssign,  // |=
    BitwiseXorAssign, // ^=
    ShiftLeftAssign,  // <<=
    ShiftRightAssign, // >>=
}

impl AssignmentOperator {
    pub fn from_token(token: TokenType) -> Self {
        match token {
            TokenType::Assign => Self::Assign,
            TokenType::PlusAssign => Self::AddAssign,
            TokenType::MinusAssign => Self::SubtractAssign,
            TokenType::StarAssign => Self::MultiplyAssign,
            TokenType::SlashAssign => Self::DivideAssign,
            TokenType::PercentAssign => Self::ModuloAssign,
            TokenType::AmpersandAssign => Self::BitwiseAndAssign,
            TokenType::PipeAssign => Self::BitwiseOrAssign,
            TokenType::CaretAssign => Self::BitwiseXorAssign,
            TokenType::LShiftAssign => Self::ShiftLeftAssign,
            TokenType::RShiftAssign => Self::ShiftRightAssign,
            _ => unreachable!("Token {:?} is not an assignment operator", token),
        }
    }
}

// ==========================================
//             Type System
// ==========================================

#[derive(Debug, Clone, PartialEq)]
pub struct TypeNode {
    pub id: NodeId,
    pub span: Span,
    pub kind: TypeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeKind {
    /// 路径类型引用: `i32`, `std.io.File`, `Map[K, V]`
    Path {
        segments: Vec<SymbolId>,
        generics: Vec<TypeNode>, // 这里不需要 Box，因为 TypeNode 已经是固定大小
    },

    /// 指针: `*T`, `*mut T`
    Pointer { elem: Box<TypeNode> },

    /// 易失指针: `^T`, `^mut T`
    VolatilePtr { elem: Box<TypeNode> },

    /// 数组: `[N]T`
    Array {
        elem: Box<TypeNode>,
        len: Box<Expr>, // 必须是常量表达式
    },

    /// 切片: `[]T`
    Slice { elem: Box<TypeNode> },

    /// 可变类型修饰符 `mut T`
    Mut(Box<TypeNode>),

    /// 函数指针类型: `fn(i32) bool`
    Function {
        params: Vec<TypeNode>,
        ret: Option<Box<TypeNode>>,
        is_variadic: bool,
    },

    // === 匿名/内联类型定义 (Structural Types) ===
    /// 结构体定义
    Struct { fields: Vec<StructFieldDef> },

    /// 联合体定义
    Union { fields: Vec<StructFieldDef> },

    /// 枚举定义
    Enum {
        backing_type: Option<Box<TypeNode>>, // enum : u8 { ... }
        variants: Vec<EnumVariant>,
    },

    /// 特征定义 (Trait)
    Trait { fields: Vec<StructFieldDef> },

    /// 代数数据类型定义 (Algebraic Data Type)
    /// 例如: adt { Some: T, None }
    Adt { variants: Vec<AdtVariant> },

    /// 推导占位符 `_`
    Infer,

    /// 代表当前 impl 块的目标类型
    SelfType,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructFieldDef {
    pub name: SymbolId,
    pub type_node: TypeNode, // TypeNode 结构体大小固定，可以直接嵌入
    pub default_value: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    pub name: SymbolId,
    pub value: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdtVariant {
    pub name: SymbolId,
    /// 负载类型。如果像 `None` 一样没有数据负载，则为 None
    pub payload_type: Option<Box<TypeNode>>,
    pub span: Span,
}

// ==========================================
//               Expressions
// ==========================================

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub id: NodeId,
    pub span: Span,
    pub kind: ExprKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// `let x = v` (可变性包含在 v 的类型中，例如 let x = mut i32.{10})
    Let {
        name: SymbolId,
        init: Box<Expr>,
    },

    /// `static x = v`
    Static {
        name: SymbolId,
        init: Box<Expr>,
    },

    // --- Literals ---
    Integer(u128),
    Float(f64),
    Bool(bool),
    Char(char),
    String(String),
    Identifier(SymbolId),

    // --- Ops ---
    Binary {
        lhs: Box<Expr>,
        op: BinaryOperator,
        rhs: Box<Expr>,
    },
    Unary {
        op: UnaryOperator,
        operand: Box<Expr>,
    },

    // --- Access ---
    FieldAccess {
        lhs: Box<Expr>,
        field: SymbolId,
    },
    IndexAccess {
        lhs: Box<Expr>,
        index: Box<Expr>,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },

    // --- Constructors ---
    /// 统一的数据字面量初始化，支持 Type.{ ... } 和 .{ ... }
    DataInit {
        /// 如果是 `.{ ... }`，此字段为 None；
        /// 如果是 `Point.{ ... }`，此字段就是 `Point` 对应的 TypeNode。
        type_node: Option<Box<TypeNode>>,
        /// 具体的数据负载
        literal: DataLiteralKind,
    },

    /// 枚举/上下文简写: .Red, .Ok
    EnumLiteral(SymbolId),

    // --- Control Flow ---
    If {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Option<Box<Expr>>,
    },
    Match {
        target: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    Switch {
        target: Box<Expr>,
        cases: Vec<SwitchCase>,
        default_case: Option<Box<Expr>>,
    },

    /// 块表达式: `{ stmt; stmt; expr }`
    Block {
        stmts: Vec<Stmt>,
        result: Option<Box<Expr>>,
    },

    /// `for (init; cond; post) body`
    For {
        init: Option<Box<Expr>>,
        cond: Option<Box<Expr>>,
        post: Option<Box<Expr>>,
        body: Box<Expr>,
    },

    /// 切片构造: arr.[start..end]
    SliceOp {
        lhs: Box<Expr>,
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        is_inclusive: bool,
    },

    /// 延迟执行: defer expr
    Defer {
        expr: Box<Expr>,
    },

    Break,
    Continue,
    Return(Option<Box<Expr>>),

    // 赋值表达式
    Assign {
        lhs: Box<Expr>,
        op: AssignmentOperator,
        rhs: Box<Expr>,
    },

    // --- Conversion ---
    As {
        lhs: Box<Expr>,
        target: Box<TypeNode>,
    },

    Undef,

    /// 泛型实例化: target[T, U]
    GenericInstantiation {
        target: Box<Expr>,
        types: Vec<TypeNode>,
    },

    /// 代表 `self`
    SelfValue,

    /// 无状态匿名函数 (Lambda)
    /// 语法: `fn(a: i32, b: i32) bool { return a < b; }`
    Lambda {
        params: Vec<FuncParam>,
        ret_type: Box<TypeNode>,
        body: Box<Expr>, // 必定是一个 Block 表达式
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DataLiteralKind {
    /// 结构体模式: `.{ x: 1, y: 2 }`
    Struct(Vec<StructFieldInit>),

    /// 数组列表模式: `.{ 1, 2, 3 }`
    Array(Vec<Expr>),

    /// 数组重复模式: `.{ 0; 1024 }`
    Repeat { value: Box<Expr>, count: Box<Expr> },

    /// 标量/单值构造模式: `.{ 10 }`
    Scalar(Box<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructFieldInit {
    pub name: SymbolId,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwitchCase {
    pub patterns: Vec<SwitchPattern>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SwitchPattern {
    Value(Expr),
    Range {
        start: Expr,
        end: Expr,
        inclusive: bool,
    },
}

impl SwitchPattern {
    /// 动态获取该模式的 Span
    pub fn span(&self) -> Span {
        match self {
            SwitchPattern::Value(expr) => expr.span,
            SwitchPattern::Range { start, end, .. } => start.span.to(end.span),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: MatchPattern,
    pub body: Expr, // 匹配成功后执行的表达式/块
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchPattern {
    /// ADT 变体匹配
    /// 语法: `[TypeNode.]Variant [: binding]`
    /// 例子: `.Ok: val`, `Result[i32, i32].Err: code`, `.None`
    Variant {
        /// 可选的完整类型路径 (例如显式写出的 `Result[i32, i32]`)
        target_type: Option<Box<TypeNode>>,
        /// 变体名称 (例如 `Ok`, `None`)
        variant_name: SymbolId,
        /// 提取的数据绑定 (例如 `: val`)。如果变体无负载则为 None
        binding: Option<SymbolId>,
        /// 整个 pattern 的 span
        span: Span,
    },

    /// 捕获所有分支: `else =>`
    CatchAll(Span),
}

impl MatchPattern {
    /// 动态获取该模式的 Span
    pub fn span(&self) -> Span {
        match self {
            MatchPattern::Variant { span, .. } => *span,
            MatchPattern::CatchAll(span) => *span,
        }
    }
}

// ==========================================
//               Statements
// ==========================================

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub id: NodeId,
    pub span: Span,
    pub kind: StmtKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    /// 表达式语句: `expr;`
    ExprStmt(Expr),

    /// 块末尾的表达式: `expr`
    ExprValue(Expr),
}

// ==========================================
//          Top-Level Declarations
// ==========================================

#[derive(Debug, Clone, PartialEq)]
pub struct GenericParam {
    pub name: SymbolId,
    pub constraints: Vec<TypeNode>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decl {
    pub id: NodeId,
    pub span: Span,
    pub name: SymbolId,
    pub is_pub: bool,
    pub kind: DeclKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeclKind {
    /// `fn name[T](...) Ret { ... }`
    Function {
        generics: Vec<GenericParam>,
        params: Vec<FuncParam>,
        ret_type: TypeNode,
        body: Option<Box<Expr>>, // Block
        is_extern: bool,
        is_variadic: bool,
    },

    /// `const x = ...` 或 `static x = ...`
    Var {
        value: Expr,
        is_static: bool,
        is_extern: bool,
    },

    /// `type Name[T] = Target;`
    TypeAlias {
        generics: Vec<GenericParam>,
        bounds: Vec<TypeNode>,
        target: TypeNode,
        is_extern: bool,
    },

    /// 模块引入
    Use {
        kind: UsePathKind,
        path: Vec<SymbolId>,
        target: UseTarget,
        is_reexport: bool,
    },

    /// Extern 块
    ExternBlock {
        abi: Option<String>,
        decls: Vec<Decl>,
    },

    /// Impl 块
    Impl {
        generics: Vec<GenericParam>,
        target_type: TypeNode,
        trait_type: Option<TypeNode>,
        decls: Vec<Decl>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsePathKind {
    Absolute, // use std.io
    Relative, // use .utils
    Super,    // use ..common
}

#[derive(Debug, Clone, PartialEq)]
pub enum UseTarget {
    Module(Option<SymbolId>),
    Members(Vec<UseMember>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct UseMember {
    pub name: SymbolId,
    pub alias: Option<SymbolId>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FuncParam {
    pub name: SymbolId,
    pub type_node: TypeNode,
    pub span: Span,
}

// ==========================================
//               Module
// ==========================================

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub path: String,
    pub decls: Vec<Decl>,
}
