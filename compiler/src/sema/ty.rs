#![allow(unused)]
use crate::parser::ast::NodeId;
use crate::utils::SymbolId;
use std::collections::HashMap;

/// 类型的唯一 ID (轻量级 Handle)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeId(pub u32);

impl TypeId {
    // 预留前 20 个 ID 给基础类型，方便快速访问
    pub const VOID: Self = Self(0);
    pub const BOOL: Self = Self(1);
    pub const I8: Self = Self(2);
    pub const I16: Self = Self(3);
    pub const I32: Self = Self(4);
    pub const I64: Self = Self(5);
    pub const I128: Self = Self(6);
    pub const U8: Self = Self(7);
    pub const U16: Self = Self(8);
    pub const U32: Self = Self(9);
    pub const U64: Self = Self(10);
    pub const U128: Self = Self(11);
    pub const F32: Self = Self(12);
    pub const F64: Self = Self(13);
    pub const ISIZE: Self = Self(14);
    pub const USIZE: Self = Self(15);
    // 字符串字面量类型 (只读切片)
    pub const STR: Self = Self(16);
    // 错误占位符 (防止级联报错)
    pub const ERROR: Self = Self(17);
}

/// 类型的具体结构
/// 注意：这里不包含 field/variant 的具体信息，只包含“形状”。
/// 具体定义存储在 Context 的 Decl 表中。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeKind {
    /// 基础类型 (i32, bool, void...)
    Primitive(PrimitiveType),

    /// 普通指针: *T (如果是 *mut T，则 base 指向一个 Mut 类型)
    Pointer(TypeId),

    /// 易失指针: ^T
    VolatilePtr(TypeId),

    /// 数组: [N]T
    Array {
        elem: TypeId,
        len: u64, // 数组长度必须是常量
    },

    /// 切片: []T (胖指针)
    Slice(TypeId),

    /// 可变类型修饰符 mut T
    Mut(TypeId),

    /// 引用具体的定义 (Struct/Enum/Union)
    /// 我们只存 ID。具体字段信息去 Context 里查。
    /// 这样设计是为了处理递归类型 (e.g., struct Node { next: *Node })
    Def(DefId, Vec<TypeId>),

    /// 代数数据类型 (ADT)
    /// 存储其 DefId 以及可能绑定的泛型参数
    Adt(DefId, Vec<TypeId>),

    /// 专门用于表示 ADT 在底层的物理 Union 负载 (Tag 之后的部分)
    /// 依然绑定原 ADT 的 DefId 和泛型
    AdtPayload(DefId, Vec<TypeId>),

    /// 特征对象 (Trait Object)
    /// 内存布局：胖指针 { data_ptr: *mut void, vtable: *mut VTable }
    TraitObject(DefId, Vec<TypeId>),

    /// 类型别名: type A = B;
    /// 记录了 "A" 这个名字，以及它指向的 "B"
    Alias(SymbolId, TypeId),

    /// 泛型参数占位符 (impl[T] 中的 T)
    /// 在单态化之前，它只是一个名字。
    Param(SymbolId),

    Function {
        params: Vec<TypeId>,
        ret: TypeId,
        is_variadic: bool,
    },

    /// 具体的函数定义项 (Function Item)
    /// 带有其 DefId 和已绑定的泛型实参。例如 `ArrayList.new[i32]`
    FnDef(DefId, Vec<TypeId>),

    /// 未知/错误类型
    Error,

    /// 专门用于表示这是一个模块（Namespace），防止与普通值混淆
    Module(DefId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrimitiveType {
    Void,
    Bool,
    I8,
    I16,
    I32,
    I64,
    I128,
    ISize,
    U8,
    U16,
    U32,
    U64,
    U128,
    USize,
    F32,
    F64,
    Str, // 内部使用的字符串字面量类型
}

/// 定义 ID (指向 struct/enum/union/trait 的声明)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DefId(pub u32);

/// 类型仓库
pub struct TypeRegistry {
    /// 存储具体的类型结构
    types: Vec<TypeKind>,

    /// 去重表：保证相同的类型 (如 *i32) 永远拥有相同的 TypeId
    interner: HashMap<TypeKind, TypeId>,
}

impl TypeRegistry {
    pub fn new() -> Self {
        let mut reg = Self {
            types: Vec::new(),
            interner: HashMap::new(),
        };
        reg.init_primitives();
        reg
    }

    fn init_primitives(&mut self) {
        // 顺序必须与 TypeId 中的常量对应
        self.add_primitive(PrimitiveType::Void); // 0
        self.add_primitive(PrimitiveType::Bool); // 1
        self.add_primitive(PrimitiveType::I8); // 2
        self.add_primitive(PrimitiveType::I16); // 3
        self.add_primitive(PrimitiveType::I32); // 4
        self.add_primitive(PrimitiveType::I64); // 5
        self.add_primitive(PrimitiveType::I128); // 6
        self.add_primitive(PrimitiveType::U8); // 7
        self.add_primitive(PrimitiveType::U16); // 8
        self.add_primitive(PrimitiveType::U32); // 9
        self.add_primitive(PrimitiveType::U64); // 10
        self.add_primitive(PrimitiveType::U128); // 11
        self.add_primitive(PrimitiveType::F32); // 12
        self.add_primitive(PrimitiveType::F64); // 13
        self.add_primitive(PrimitiveType::ISize); // 14
        self.add_primitive(PrimitiveType::USize); // 15
        self.add_primitive(PrimitiveType::Str); // 16

        // 17: Error
        self.types.push(TypeKind::Error);
    }

    fn add_primitive(&mut self, p: PrimitiveType) {
        let kind = TypeKind::Primitive(p);
        let id = TypeId(self.types.len() as u32);
        self.types.push(kind.clone());
        self.interner.insert(kind, id);
    }

    /// 获取或创建类型的 ID (Interning)
    /// 如果请求 *i32 且之前创建过，直接返回旧 ID
    pub fn intern(&mut self, kind: TypeKind) -> TypeId {
        if let Some(&id) = self.interner.get(&kind) {
            return id;
        }

        let id = TypeId(self.types.len() as u32);
        self.types.push(kind.clone());
        self.interner.insert(kind, id);
        id
    }

    /// 通过 ID 获取类型结构
    pub fn get(&self, id: TypeId) -> &TypeKind {
        &self.types[id.0 as usize]
    }

    /// 规范化类型 (穿透 Alias)
    /// 用于类型检查： check(A) == check(B) 应使用 normalize(A) == normalize(B)
    pub fn normalize(&self, mut id: TypeId) -> TypeId {
        loop {
            match self.get(id) {
                TypeKind::Alias(_, target) => {
                    id = *target;
                }
                _ => return id,
            }
        }
    }

    /// 判断是否是整数 (辅助函数)
    pub fn is_integer(&self, id: TypeId) -> bool {
        match self.get(self.normalize(id)) {
            TypeKind::Primitive(p) => matches!(
                p,
                PrimitiveType::I8
                    | PrimitiveType::I16
                    | PrimitiveType::I32
                    | PrimitiveType::I64
                    | PrimitiveType::I128
                    | PrimitiveType::ISize
                    | PrimitiveType::U8
                    | PrimitiveType::U16
                    | PrimitiveType::U32
                    | PrimitiveType::U64
                    | PrimitiveType::U128
                    | PrimitiveType::USize
            ),
            _ => false,
        }
    }

    pub fn is_float(&self, id: TypeId) -> bool {
        match self.get(self.normalize(id)) {
            TypeKind::Primitive(p) => matches!(p, PrimitiveType::F32 | PrimitiveType::F64),
            _ => false,
        }
    }

    /// 剥离外层的 Mut 包裹，返回底层类型。
    /// 如果原来不是 Mut，原样返回。
    /// 极其重要：后端 Lowering 和类型兼容性检查必备！
    pub fn strip_mut(&self, id: TypeId) -> TypeId {
        match self.get(self.normalize(id)) {
            TypeKind::Mut(inner) => *inner,
            _ => id,
        }
    }

    /// 检查一个类型是否显式声明了可变性
    pub fn is_mut(&self, id: TypeId) -> bool {
        matches!(self.get(self.normalize(id)), TypeKind::Mut(_))
    }

    /// （可选但很有用）获取指针或切片的底层元素类型
    pub fn get_elem_type(&self, id: TypeId) -> Option<TypeId> {
        match self.get(self.normalize(id)) {
            TypeKind::Pointer(elem) | TypeKind::VolatilePtr(elem) | TypeKind::Slice(elem) => {
                Some(*elem)
            }
            TypeKind::Array { elem, .. } => Some(*elem),
            TypeKind::Mut(inner) => self.get_elem_type(*inner), // 穿透 Mut 找元素
            _ => None,
        }
    }
}
