# Kern Language Design (v0.3.8)

## Table of Contents

1. [Core Philosophy](#1-core-philosophy-and-manifesto)
2. [Type System](#2-type-system)
3. [Declarations and Storage](#3-declarations-and-storage)
4. [Data Structures](#4-data-structures)
5. [Functions and Traits](#5-functions-and-traits)
6. [Control Flow](#6-control-flow)
7. [Modules](#7-modules)
8. [Interoperability](#8-interoperability)
9. [Algebraic Data Types (ADT) and Pattern Matching](#9-algebraic-data-types-adt-and-pattern-matching)
10. [Stateless Anonymous Functions (Lambdas)](#10-stateless-anonymous-functions-lambdas)
11. [Inline Assembly (`@asm`)](#11-inline-assembly-asm)
12. [AST Attributes and Metadata (`#[...]`)](#12-ast-attributes-and-metadata--and-)
13. [Compiler Intrinsics (`@...`)](#13-compiler-intrinsics-)

---

## 1. Core Philosophy and Manifesto

**Kern** is a systems‑level language for operating system kernels, embedded firmware, and high‑performance infrastructure.

Kern’s design is based on the observation that languages trade off **abstraction capability** against **policy constraints**. Kern aims to occupy the fourth quadrant: **high abstraction, low policy**.

### 1.1 Core Values

#### 1. Clarity over novelty

* Syntax must be simpler and more consistent than C.
* Remove features that make generated assembly unpredictable.
* Fix C legacy warts (spiral declarations, implicit array decay).
* Goal: what you write is what the machine executes.

#### 2. Explicit over implicit

* No implicit heap allocation.
* No exceptions, no background GC, no implicit destructor chains.
* Unless explicitly introduced, Kern binaries have no runtime dependencies.

#### 3. Mechanism Trinity

To achieve “high abstraction, low policy”, Kern provides three core mechanisms:

1. **Module system** – modern namespaces and visibility control.
2. **Generics** – strongly‑typed code reuse via monomorphisation (zero runtime cost).
3. **Algebraic Data Types** – precise state management without implicit control flow.

### 1.2 Non‑Goals

* **Compile‑time enforced memory safety** – no borrow checker.
* **Standard library design** – Kern is freestanding.
* **Optimisation that exploits undefined behaviour** – ambiguous behaviour (integer overflow, uninitialised reads) is either defined or a compile‑time error.

## 2. Type System

### 2.1 Primitive Types

* **Integers**: `i8`, `i16`, `i32`, `i64`, `i128` (signed); `u8`, `u16`, `u32`, `u64`, `u128` (unsigned); `usize`, `isize` (pointer‑sized).
* **Floats**: `f32`, `f64`.
* **Boolean**: `bool` (1 byte, no arithmetic).
* **Never**: `!` (represents computations that never resolve, e.g., infinite loops or fatal halts).

### 2.2 Mutability and Scalar Initialization

Kern **does not have a concept of "default types"** derived from compiler assumptions. Mutability and typing are absolutely controlled by the programmer via **Scalar Initialization Syntax**: `Type.{value}`. The type `T` and its mutable variant `mut T` are distinct types.

* **Explicit Initialization**: To declare a variable, you explicitly define its type and mutability.
`let a = i32.{10};` (immutable `i32`)
`let b = mut i32.{20};` (mutable `i32`)
* **Address‑of (`.&`)**: Strictly preserves mutability based on the source scalar.
`let ptr = mut i32.{10}.&;` (constructs a scalar `mut i32` with value `10`, then takes its address, yielding `*mut i32`).
* **The `mut usize` / `mut f32` Sugar**: As a pragmatic exception for ergonomics (primarily for `for` loops and basic math), Kern allows raw literals `let a = 10;` and `let f = 3.14;`. These do not imply a "default type flaw" but are strict syntactic sugar expanding directly to `let a = mut usize.{10};` and `let f = mut f32.{3.14};`.

### 2.3 Pointers and Volatility

* **Normal pointers**: `*T`, `*mut T` – ordinary memory, compiler may optimise.
* **Volatile pointers**: `^T`, `^mut T` – MMIO/hardware registers, no optimisations.
* **Dereference**: `ptr.*` (postfix, allows chained access like `ptr.*.field`).
* **Null pointer**: literal `0` must be explicitly cast to a pointer type (e.g., `0 as *i32`).
* **Pointer Arithmetic**: Implicit pointer arithmetic (e.g., `ptr + 1`) is **strictly forbidden**. To compute addresses, you must either:

1. Cast to `usize`, perform the math, and cast back (e.g., `(ptr as usize + 4) as *u32`).
2. Use standard library pointer methods.

* **Casts**: explicit conversion required using `as`, preserving bit-patterns only.

### 2.4 Arrays, Slices, and Strings

* **Arrays**: `[N]T`, `[N]mut T` – value type, copy on assignment/parameter passing.
* **Array initialisers**: `.{1, 2, 3}`, `.{0; 1024}`.
* **Slices**: `[]T`, `[]mut T` – fat pointer (pointer + length).
* **Slice creation**: `arr.[start..end]`, `arr.[..]`, `ptr.[0..10]`.
* **Indexing**: `arr.[i]` (dot notation).
* **Length operator**: `#arr` (prefix `#`).
* **Strings**: String literals (e.g., `"Hello"`) inherently evaluate to `[]u8`. Kern strictly avoids C-style implicit `\0` termination. If passing strings to C-ABI functions, the null terminator must be manually included (e.g., `"Hello\0"`).

## 3. Declarations and Storage

* **Stack (local)**: `let name = Type.{value};`.
* **Static (global)**: `static name = Type.{value};`
* **Compile‑time constant**: `const NAME = Type.{value};` (inlined, no memory location).
* **Uninitialized Storage**: Use the `undef` keyword within the literal to leave memory uninitialized intentionally: `let name = mut T.{undef};`.
* **External Storage**: For variables defined in external object files or C code, use an `extern` block and the `undef` keyword within the literal: `extern { static name = T.{undef}; }` (resolved at link time, see Section 8).

## 4. Data Structures

### 4.1 Structs

```kern
type Point = struct {
    x: i32,
    y: i32,
};

```

* **Generics**: `type Point[T] = struct { x: T, y: T };`
* **Default fields**: `type Config = struct { port: u16 = 8080, host: u32 = 0 };`
* **Layout**: default reorder/padding for size; `extern type …` guarantees C‑compatible memory layout and alignment.
* **Initialization and `undef**`: When initializing a struct using `Type.{ ... }`, any field without a default value **must** be explicitly provided; omitting it is a strict compile-time error. If you intentionally want to leave a field uninitialized, you must explicitly use `undef` (e.g., `priority = u8.{undef};`).

```kern
// Immutable 
let p1 = Point.{x: 10, y: 20};       

// Mutable binding with explicit type and shorthand literal
let p2 = mut Point.{x: 10, y: 20}; 

```

### 4.2 Unions

```kern
type Payload = union {
    as_int: i32,
    as_float: f32,
    raw: [4]mut u8,
};

```

No active‑field tracking; no default values.

### 4.3 Enums

C‑style integer constant sets, but with strict value guarantees. Backing type defaults to `u32`.

```kern
type Color: u8 = enum {
    Red = 0,
    Green, // 1
    Blue,  // 2
};

```

* **Data Sanitization**: Kern **forbids** using the `as` operator or intrinsics to implicitly cast untrusted dynamic integers (e.g., hardware port reads) into Enums. Valid variants are constructed directly (`Color.Red`). Sanitizing external data must be explicitly handled by the programmer via an exhaustive `switch` block:

```kern
let raw_data = inb(0x60); // Read u8 from port
let color = switch (raw_data) {
    0 => Color.Red,
    1 => Color.Green,
    2 => Color.Blue,
    else => Color.Red, // Mandatory fallback for unexpected values
};

```

### 4.4 Conversions

* **`as` operator**: Reinterpretation that preserves the physical bit pattern (e.g., pointer casts). **Cannot** be used for numeric conversions that change the representation, nor for Trait Object construction.
* **Numeric conversions**: Must use intrinsics: `@intToFloat`, `@floatToInt`, `@intCast` (for truncation and zext/sext).

## 5. Functions and Traits

### 5.1 Free Functions

Defined at module level.

```kern
pub fn max(a: i32, b: i32) i32 {
    if (a > b) a else b
}

```

### 5.2 Implementation Blocks (`impl`)

* `impl` blocks attach methods to a type.
* **Absolute Contextual Binding**: Because the `impl` block defines an unambiguous target type, Kern enforces extreme syntactical minimalism: the `self` parameter **must be omitted** from the method signature. The Semantic Analyzer (Sema) implicitly and strictly injects `self` based on the target type.

```kern
type Point = struct { x: i32, y: i32 };

impl *mut Point {
    // Signature omits 'self'. 'self' is inherently available as *mut Point.
    pub fn move_by(dx: i32, dy: i32) void {
        self.x += dx;
        self.y += dy;
    }
}

```

### 5.3 Generics and Elision

Kern uses zero-cost monomorphisation for generics.
To maintain high abstraction without losing predictability, Kern enforces **Strict Parameter-Driven Deduction** for function calls:

1. **Explicit Instantiation**: You can always provide the explicit generic type: `max[u32](a, b)`.
2. **Safe Elision**: If the generic parameter `T` appears in the function's parameter list, it can be safely elided. The compiler will implicitly deduce `T` from the arguments: `max(u32.{10}, u32.{20})`.
3. **No Implicit Casting**: If deduction finds conflicting types (e.g., `u32` vs `u8`), the compiler will strictly throw an error. It will not attempt to implicitly cast them.
4. **Mandatory Instantiation**: If `T` only appears in the return type (e.g., `@sizeOf[T]() -> usize`), it **must** be explicitly provided.

### 5.4 Traits

Traits define a set of pure function signatures representing a VTable. Similar to `impl` blocks, the first parameter (`self`) is intrinsically understood and **must be omitted** from the signature.

```kern
type Reader = trait {
    read: fn([]u8) usize,
};

// Pure semantic composition
type ReadWriter: Reader + Writer = trait {
    flush: fn() void,
};

```

### 5.5 Trait Objects

A trait object is a built-in primitive representing a fat pointer (data pointer + vtable pointer). It is constructed using the **uniform initialization syntax**, eliminating the need for `as` casting.

```kern
type File = struct { ... };
impl *mut File : Reader { ... }

let file = mut File.{ ... };
// Step 1: Obtain the concrete pointer
let p = file.&; 

// Step 2: Construct the trait object via explicit initialization
let r = mut Reader.{p}; 

// Step 3: Call methods directly
let bytes_read = r.read(buf);

```

* **Pointer Matching Rule**: Constructing a trait object is **strictly forbidden** unless the implementation target is explicitly a pointer type (e.g., `impl *mut T : Trait`). This guarantees the compiler always knows the exact stack size during dynamic dispatch.

### 5.6 Error Handling

No built‑in policy. No exceptions, no panic. Use `adt`, `union` + `enum`, or integer error codes.

## 6. Control Flow

### 6.1 Conditional Expressions

`if` is an expression.

```kern
let a = if (b < 10) i32.{10} else i32.{20};

```

### 6.2 Switch Expressions

Enhanced C‑style `switch`. No fallthrough.

* **Ranges**: `..` defines a left-closed, right-open range. `..=` defines a fully inclusive range.

```kern
let result = switch (val) {
    1..10 => 10,       // 1 to 9
    11, 12, 13 => 20,
    14..=15 => 30,     // 14 and 15
    else => 0,
};

```

* **Exhaustiveness**: Switch expressions must be exhaustive. When matching on an `enum`, `else =>` is not required if all variants are explicitly matched.

### 6.3 For Loops

Only `for` (no `while`, `do‑while`).

```kern
for (let i = 0; i < 10; i += 1) { … }
for (; cond ;) { … }          // while
for (;;) { … }                // infinite loop

```

### 6.4 Defer

Executes an expression or block when the **current lexical scope (block `{\}`)** exits (LIFO). `defer` is strictly block‑scoped, not function‑scoped.

```kern
let ptr = malloc(1024);
defer free(ptr);

```

### 6.5 Blocks, Expressions, and Discards

Blocks evaluate to their last expression.
Kern strictly mandates that returned values cannot be implicitly ignored to prevent logical errors in systems programming.

* **Explicit Discard**: If a function or expression returns a value that is intentionally unused, it **must** be bound to the discard identifier `_`.
`let _ = file.write(buf); // Explicitly discard the returned usize`
* `expr;` evaluates to `void`. Dropping a non-void return value by simply appending a semicolon is a compiler error.

**Evaluation Order with Defer:**
When a block `{ … }` evaluates as an expression and contains `defer` statements, the exact exit sequence is:

1. **Evaluate**: Compute the value of the final expression.
2. **Execute**: Run all `defer` statements registered in the current block in LIFO order.
3. **Yield**: Pass the computed value to the outer context.

> **Warning**: Returning a pointer to a resource that is freed by a `defer` within the exact same block will result in a dangling pointer. Kern prioritizes explicit execution order over implicit memory protection.

## 7. Modules

### 7.1 Module Resolution

Absolute paths in Kern are resolved through two precise roots:

1. **Compiler Root Directory**: The root module entry point provided to `kernc` (e.g., treating the project root similar to `crate::`).
2. **CLI Alias Mappings**: External package paths explicitly mapped via compiler options (e.g., `-M std=./libs/std` allows `use std.io;`). This forms the foundation of the Kern package manager and standard library injection.

* **Relative import**: `use .utils;`, `use ..common.types;`

### 7.2 Directory Modules (`init.kn`)

A directory becomes a module if it contains `init.kn`.

* **Multi-pass Type Resolution**: Kern uses multi-pass parsing. Circular type dependencies across different module files are fully supported without forward declarations.

### 7.3 Idiom: Static Methods via Modules

File name matches type name; module functions act as “static methods”.

```kern
// std/collections/ArrayList.kn
pub type ArrayList[T] = struct { … };
pub fn new[T]() ArrayList[T] { … }

// main.kn
use std.collections.ArrayList;
let list = ArrayList.new[i32]();

```

## 8. Interoperability

Kern uses the C Application Binary Interface (ABI) as the universal language for all external communication.

### 8.1 Exporting Functions to C/Assembly

Use the `extern` modifier on the function definition. This instructs the compiler to use the standard C calling convention and disables name mangling. (Note: `pub` is a frontend semantic modifier for Kern modules; `extern` alone handles external linkage).

```kern
extern fn _start() void { ... }

```

### 8.2 Importing External Functions and Statics

External C functions can use the `...` syntax to support C-style variadic arguments. External statics must be declared using `T.{undef}`. Items inside an `extern` block can be marked `pub` to expose them through the Kern module system.

```kern
extern {
    pub fn malloc(size: usize) *mut u8;
    pub fn printf(format: *u8, ...) i32;
    pub static MULTIBOOT_MAGIC = u32.{undef};
}

```

## 9. Algebraic Data Types (ADT) and Pattern Matching

An `adt` is implemented at the physical memory level as a Tagged Union (a hidden scalar discriminant tag followed by a union aligned to its largest variant).
Like enums, the backing integer type of an ADT tag can be strictly defined (e.g., `type Status: u8 = adt { ... }`).

### 9.1 Defining ADTs

```kern
pub type Result[T, E] = adt {
    Ok: T,
    Err: E,
};

```

### 9.2 Elided Initialization Syntax

Where the target type context is strictly explicit (e.g., function returns, arguments, explicit variable declarations), **any type** (including ADTs, Arrays, and Structs) can be initialized using the elided literal syntax `.{ ... }`.

```kern
fn safe_divide(a: i32, b: i32) Result[i32, i32] {
    if (b == 0) return .{ Err: -1 }; 
    return .{ Ok: a / b };
}

```

### 9.3 Pattern Matching (`match`)

Destructuring requires the `match` expression. `match` bindings perfectly mirror the `adt` definition syntax using a colon (`:`).

* **Syntax and Elision**: Data extraction is performed by mapping the variant to a local binding name: `Variant: binding_name`.
* **Exhaustiveness**: `match` blocks must be strictly exhaustive. Provide all variants or a catch-all `else =>`.

```kern
match (res) {
    .Ok: val => printf("Result: %d\n\0", val),
    .Err: code => printf("Error code: %d\n\0", code),
}

```

* **No Direct Access**: Attempting to access an ADT's internal payload without a `match` statement is a strict compile-time error.

## 10. Stateless Anonymous Functions (Lambdas)

To support inline callbacks without violating Kern's strict memory rules, the language supports stateless anonymous functions.

### 10.1 Strict Statelessness

Anonymous functions use the `fn(...) ReturnType { ... }` syntax.
Crucially, Kern **strictly forbids environmental capturing (closures)**. An anonymous function cannot access local variables from its enclosing scope. This physical limitation guarantees that anonymous functions compile down to pure, static function pointers (`fn`), entirely preventing use-after-free bugs caused by stack-allocated environments escaping their scope.

```kern
let arr = [3]mut i32.{ 3, 1, 2 };

// Safe, zero-allocation callback
arr.sort(fn(a: i32, b: i32) bool {
    return a < b;
});

```

## 11. Inline Assembly (`@asm`)

To maintain Kern's philosophy of "explicit over implicit", inline assembly does not use format strings with hidden index bindings. Instead, it leverages Kern's elided struct literal syntax (`.{ ... }`) to create a strict, named mapping between CPU registers and Kern variables.

### 11.1 Syntax, Register Binding, and MAST Evaluation

The parameters passed to `@asm` (such as the `asm` string array, `clobbers`, and `volatile` flag) are **not runtime structures**. They are resolved and evaluated entirely at compile-time during the MAST (Monomorphized Abstract Syntax Tree) phase.

```kern
pub fn outb_and_read(port: u16, data: u8) u8 {
    let status = mut u8.{undef};

    @asm(.{
        asm: .{
            "out dx, al",
            "in al, dx"
        },
        outputs: .{ al: status.& },   // Binds register to mutable pointer
        inputs: .{ dx: port, al: data },
        clobbers: .{ "memory" },      // Compile-time known
        volatile: true                // Compile-time known
    });

    return status;
}

```

## 12. AST Attributes and Metadata (`#[...]` and `#![...]`)

Kern completely rejects traditional C-style preprocessor macros, substituting them with an **Attribute Mini-Language**. Attributes are strictly parsed by the frontend and natively understood by the compiler backend to control memory layout, linkage, and optimization.

### 12.1 Scope: Outer vs. Inner Attributes

* **Outer Attributes (`#[...]`)**: Attached to the immediately following AST node (e.g., a function, struct, or variable declaration).
* **Inner Attributes (`#![...]`)**: Applies to the entire enclosing lexical scope (usually the file). If placed at the top of an `init.kn` file, the attribute applies to the entire module.

### 12.2 Mutually Exclusive Content

Kern strictly enforces single-responsibility for attribute brackets. The content inside the brackets `[...]` must be **either** a condition evaluator **or** a list of metadata tags.

#### 1. Conditional Compilation (`if(...)`)

Uses a strict boolean evaluator at compile-time. If the condition evaluates to `false`, the target node (or file) is entirely pruned before semantic analysis. It supports logical operators (`and`, `or`, `not`) and checking custom compiler flags (`-D key=value`).

```kern
#![if(os == "bare_metal")]
#[if(not debug_mode)]

```

#### 2. Metadata Tags

A comma-separated list of tags attached to the AST for compiler side-effects. Metadata tags are grouped by their specific impact on the generated binary:

**A. Linkage & FFI Control**

* `export_name("...")`: Overrides the mangled name with a specific string for the linker.
* `link_section("...")`: Forces a global variable or function into a specific ELF/Mach-O/COFF section (crucial for OS bootloaders, e.g., `#[link_section(".multiboot")]`).

**B. Memory Layout**

* `packed`: Removes all padding between struct/union fields. The size becomes exactly the sum of its fields, at the cost of potential unaligned memory access penalties.
* `align(N)`: Forces the alignment of a struct or static variable to `N` bytes (e.g., `#[align(4096)]` for page tables).

**C. Optimization & Control Flow**

* `cold`: Marks a function as rarely executed, moving it out of the hot instruction cache and optimizing branching.
* `naked`: Instructs the compiler to omit the standard function prologue and epilogue. Strictly used for hardware interrupt handlers and contextual context-switching alongside `@asm`.
* `inline(always)` / `inline(never)`: *(Planned)* Overrides the LLVM inliner's heuristic.

---

## 13. Compiler Intrinsics (`@...`)

Intrinsics are special functions implemented directly within the Kern compiler backend (e.g., LLVM). They are prefixed with `@` to strictly separate them from user-defined functions. They are used for operations that alter data representation, query compile-time information, or emit specialized CPU instructions.

### 13.1 Compile-Time Type Information

These intrinsics are evaluated completely at compile-time and result in a constant `usize`.

* `@sizeOf[T]() -> usize`: Returns the memory footprint (size in bytes) of type `T`.
* `@alignOf[T]() -> usize`: Returns the ABI-required alignment (in bytes) of type `T`.

### 13.2 Numeric Conversions

Kern explicitly rejects using the `as` operator for data conversions that mutate the underlying bit representation (e.g., truncation or floating-point conversions).

* `@intCast[T: Integer, U: Integer](val: T) -> U`: Bit-width truncation (e.g., `i32` to `u8`) or zero/sign-extension.
* `@intToFloat[T: Integer, U: Float](val: T) -> U`
* `@floatCast[T: Float, U: Float](val: T) -> U`
* `@floatToInt[T: Float, U: Integer](val: T) -> U`

*(Note: Pointer-to-integer (`*T` to `usize`) and pointer-to-pointer (`*T` to `*U`) casts preserve the physical bit-pattern and are still performed using the `as` operator).*

### 13.3 Hardware & Execution Control

* `@unreachable() -> !`: Emits an unreachable instruction. Informs the optimizer that a control flow path is physically impossible, allowing it to eliminate dead branches (often used in exhaustiveness fallback).
* `@trap() -> !`: Emits an illegal instruction (`llvm.trap`) to deliberately crash/halt the program securely.
* `@fence()`: Emits a strictly sequentially-consistent memory fence (`mfence`) to prevent instruction reordering around sensitive MMIO operations.
* `@breakpoint()`: Triggers a hardware breakpoint (`llvm.debugtrap`) for system debuggers.

*(Note: Kern does not provide `@volatileLoad` or `@volatileStore` intrinsics. Instead, Kern treats volatility as a first-class type qualifier (`^T` and `^mut T`). Hardware register accesses are performed via standard dereferencing `ptr.*` on a volatile pointer, yielding perfectly predictable code without intrinsic clutter.)*

### 13.4 Bitwise Math & Memory Operations

Mapped directly to single-cycle CPU instructions and highly optimized backend primitives where available:

* `@popCount[T: Integer](val: T) -> T`: Returns the number of set bits (1s).
* `@clz[T: Integer](val: T) -> T`: Count leading zeros.
* `@ctz[T: Integer](val: T) -> T`: Count trailing zeros.
* `@bswap[T: Integer](val: T) -> T`: Reverses the byte order of an integer value (useful for endianness conversions).
* `@memcpy(dest: *mut u8, src: *u8, len: usize) void`: Performs a highly-optimized bulk memory copy.
* `@memset(dest: *mut u8, val: u8, len: usize) void`: Performs a highly-optimized bulk memory fill.