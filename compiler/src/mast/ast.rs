use crate::parser::ast;
use crate::sema::ty::TypeId;
use crate::utils::{Span, SymbolId};

/// 单态化 ID (Monomorphized ID)
/// 与前端的 DefId 不同，前端一个泛型 `List[T]` 只有一个 DefId，
/// 但在这里，`List[i32]` 和 `List[u8]` 会拥有两个完全不同的 MonoId。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MonoId(pub u32);

/// MAST 模块 (编译单元的最终扁平化表示)
/// 一切都被平铺，没有嵌套模块，没有 Impl 块，没有泛型。
#[derive(Debug, Clone)]
pub struct MastModule {
    pub name: String,
    pub structs: Vec<MastStruct>,
    pub globals: Vec<MastGlobal>, // 所有 static (含全局和局部) 都被提升到这里
    pub functions: Vec<MastFunction>,
    // Trait, Enum(被降级为整数和常量), TypeAlias 在这里彻底消失
}

#[derive(Debug, Clone)]
pub struct MastStruct {
    pub id: MonoId,
    pub name: String, // 扁平化后的全限定名，例如 "std_collections_ArrayList_i32"
    pub fields: Vec<MastField>,
    pub is_extern: bool, // 用于对接 C 的 struct
    pub is_union: bool,
    pub largest_field_idx: usize,
}

#[derive(Debug, Clone)]
pub struct MastField {
    pub name: SymbolId,
    pub ty: TypeId, // 保证是绝对具体的类型，绝不含 Param
}

#[derive(Debug, Clone)]
pub struct MastGlobal {
    pub id: MonoId,
    pub name: String, // 扁平化的全局符号名
    pub ty: TypeId,
    pub is_mut: bool,           // 对应 static mut
    pub init: Option<MastExpr>, // extern 的时候为 None。初始化必须是常量表达式。
    pub is_extern: bool,
}

#[derive(Debug, Clone)]
pub struct MastFunction {
    pub id: MonoId,
    pub name: String, // 例如 "Point_i32_move_by" (方法被扁平化为普通函数)
    pub params: Vec<MastParam>,
    pub ret_ty: TypeId,
    pub body: Option<MastBlock>, // extern 时为 None
    pub is_extern: bool,
    pub is_variadic: bool,
}

#[derive(Debug, Clone)]
pub struct MastParam {
    pub name: SymbolId,
    pub ty: TypeId,
}

#[derive(Debug, Clone)]
pub struct MastAsmBlock {
    /// 经过合并的汇编模板字符串，例如 "out dx, al \n in al, dx"
    pub asm_template: String,

    /// LLVM 标准约束字符串，例如 "={al},{dx},{al},~{memory}"
    pub constraints: String,

    /// 传给内联汇编的实参 (仅包含 inputs)
    pub input_args: Vec<MastExpr>,

    /// 接收返回值的指针 (对应 outputs)
    /// Codegen 阶段会自动将汇编返回的结果 Store 到这些指针里
    pub output_ptrs: Vec<MastExpr>,

    /// 输出变量的基础类型 (用于 Codegen 生成正确的接收和提取指令)
    pub output_tys: Vec<TypeId>,

    pub is_volatile: bool,
}

// ==========================================
//          Statements & Blocks
// ==========================================

#[derive(Debug, Clone)]
pub struct MastBlock {
    pub stmts: Vec<MastStmt>,
    pub result: Option<Box<MastExpr>>, // 块的返回值
    pub defers: Vec<MastExpr>,
}

#[derive(Debug, Clone)]
pub enum MastStmt {
    /// 局部变量绑定 (注意：局部 static 不在这里，已被提升为 MastGlobal)
    Let {
        name: SymbolId,
        ty: TypeId,
        init: MastExpr,
    },
    /// 表达式语句
    Expr(MastExpr),
    // 在 Lowering 阶段，所有的 defer 都已经被
    // 倒序强行插入到了此 Block 的每一个返回/退出路径上。
}
/// 每一个 MAST 表达式都必须显式携带它的具体类型。
#[derive(Debug, Clone)]
pub struct MastExpr {
    pub ty: TypeId,
    pub span: Span, // 仅用于报错或生成 Debug Info (DWARF)
    pub kind: MastExprKind,
}

impl MastExpr {
    pub fn new(ty: TypeId, kind: MastExprKind, span: Span) -> Self {
        Self { ty, kind, span }
    }
}

#[derive(Debug, Clone)]
pub enum MastExprKind {
    // --- 1. 基本字面量 ---
    Undef,
    Integer(u128),
    Float(f64),
    Bool(bool),
    /// 字符串在 LLVM 中通常生成一个全局常量数组。
    /// 保留 StringLiteral 方便 Codegen 时自动生成 Global Variable 并返回指针。
    StringLiteral(String),

    // --- 2. 引用 ---
    Var(SymbolId),     // 局部变量/函数参数引用
    GlobalRef(MonoId), // 引用 static 全局变量 (返回的是指针)
    FuncRef(MonoId),   // 引用具体的函数 (返回函数指针)

    // --- 3. 内存操作 ---
    AddressOf(Box<MastExpr>),
    Deref(Box<MastExpr>),

    // --- 4. 聚合数据访问与构造 ---
    StructInit {
        struct_id: MonoId,
        /// 已经按照结构体内存布局排序好的字段初始化值
        fields: Vec<MastExpr>,
    },
    UnionInit {
        union_id: MonoId,
        field_idx: usize,
        value: Box<MastExpr>,
    },
    ArrayInit(Vec<MastExpr>),

    /// 结构体字段访问
    FieldAccess {
        lhs: Box<MastExpr>,
        struct_id: MonoId, // 显式记录所属结构体的具体 MonoId
        field_idx: usize,
    },

    /// 数组或切片索引
    IndexAccess {
        lhs: Box<MastExpr>,
        index: Box<MastExpr>,
    },

    // --- 5. 执行与控制流 ---
    /// 统一的调用接口 (方法调用、泛型调用均已被 Lowerer 转换为普通的 FuncRef 或 Var 调用)
    Call {
        callee: Box<MastExpr>,
        args: Vec<MastExpr>,
    },

    If {
        cond: Box<MastExpr>,
        then_branch: MastBlock,
        else_branch: Option<MastBlock>,
    },

    /// 包含循环体和一个专门的 Latch (锁存) 块，用于执行 `i += 1` 等 post 语句。
    /// 遇到 continue 时，会直接跳转到 latch 块执行，然后再判断是否进入下一轮。
    Loop {
        body: MastBlock,
        latch: Option<MastBlock>, // 对应 for 循环的 post 语句
    },

    /// Switch 被保留，因为 LLVM 有原生的 `switch` 指令，比 if-else 链快得多。
    Switch {
        target: Box<MastExpr>,
        cases: Vec<MastSwitchCase>,
        default_case: Option<MastBlock>,
    },

    Break,
    Continue,
    Return(Option<Box<MastExpr>>), // 包含的表达式已经过 Coercion 类型转换

    // --- 6. 运算 ---
    Binary {
        op: ast::BinaryOperator,
        lhs: Box<MastExpr>,
        rhs: Box<MastExpr>,
    },
    Unary {
        op: ast::UnaryOperator,
        operand: Box<MastExpr>,
    },
    Assign {
        op: ast::AssignmentOperator,
        lhs: Box<MastExpr>,
        rhs: Box<MastExpr>,
    },

    // --- 7. 类型转换 (细化，讨好 LLVM) ---
    /// 在前端，一切转换都是 `as`。但在 MAST，必须拆分成 LLVM 级别的具体操作。
    Cast {
        kind: MastCastKind,
        operand: Box<MastExpr>,
    },

    // --- 8. 胖指针 / Trait Object 构建 ---
    /// `let r = p as mut Reader;` 降级为手动拼装一个包含两个指针的 Struct
    ConstructFatPointer {
        data_ptr: Box<MastExpr>,
        /// 如果是 Trait Object，这是 vtable_ptr；
        /// 如果是 Slice/String，这是一个常量 Integer 表示长度！
        meta: Box<MastExpr>,
    },

    /// 提取胖指针的数据指针 (相当于 llvm extractvalue 0)
    ExtractFatPtrData(Box<MastExpr>),
    /// 提取胖指针的元数据 (vtable_ptr 或 slice_len，相当于 extractvalue 1)
    ExtractFatPtrMeta(Box<MastExpr>),

    // --- 9. 执行块 ---
    /// 作为一个整体表达式执行的代码块 (用于嵌套作用域和 Defer 展开)
    Block(MastBlock),

    // --- 10. ADT 原语 (实际上背后是 Struct+Union 布局) ---
    /// 构建一个 ADT 实例。
    /// 在物理上，LLVM 把它当作一个 `{ TagType, UnionType }` 的结构体。
    AdtInit {
        adt_struct_id: MonoId, // 降级后的包装结构体 ID
        tag_value: u128,       // 具体的枚举鉴别器
        /// 变体的具体负载，如果没有负载就是 Undef
        payload: Box<MastExpr>,
    },

    // --- 11. LLVM Inline Assembly ---
    /// 经过 Lowering 降级后，完美契合 LLVM `call asm` 指令的数据结构
    Asm(MastAsmBlock),
}

#[derive(Debug, Clone)]
pub struct MastSwitchCase {
    // 经过 Const Eval 后，所有的 case pattern 都变成了确定的整数值
    pub values: Vec<u128>,
    pub body: MastBlock,
}

/// 详尽的类型转换分类，与 LLVM IR 指令一一对应
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MastCastKind {
    Bitcast,      // 相同大小的位模式转换 (如 *i32 到 *u8)
    PtrToInt,     // 指针转整数 (如 *u8 到 usize)
    IntToPtr,     // 整数转指针 (如 usize 到 *u8)
    SignExt,      // 有符号整数扩展 (如 i8 到 i32)
    ZeroExt,      // 无符号整数扩展 (如 u8 到 u32)
    Trunc,        // 整数截断 (如 i32 到 i8)
    IntToFloat,   // 整数转浮点数 (如 i32 到 f32)
    FloatToInt,   // 浮点数转整数
    FloatCast,    // 浮点数精度转换 (f32 <=> f64)
    ArrayToSlice, // 隐式降级：构造切片胖指针
}
