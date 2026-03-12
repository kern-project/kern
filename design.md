# Kern Language Design 

## Table of Contents
   1. [Core Philosophy](#1-core-philosophy)
   2. [Type System](#2-type-system)
   3. [Declarations and Storage](#3-declarations-and-storage)
   4. [Data Structures](#4-data-structures)
   5. [Functions and Traits](#5-functions-and-traits)
   6. [Control Flow](#6-control-flow)
   7. [Modules](#7-modules)
   8. [Interoperability](#8-interoperability)
   9. [Algebraic Data Types (ADT) and Pattern Matching](#9-algebraic-data-types-adt-and-pattern-matching)
   10. [Stateless Anonymous Functions (Lambdas)](#10-stateless-anonymous-functions-lambdas)

**[Experimental Features (Unstable)](#experimental-features-unstable)** 
    
   11. [Inline Assembly (`@asm`)](#11-inline-assembly-asm)
   12. [AST Attributes and Metadata (#[...])](#12-ast-attributes-and-metadata-)
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

### 1.2 Non‑Goals

* **Compile‑time enforced memory safety** – no borrow checker.
* **Standard library design** – Kern is freestanding.
* **Optimisation that exploits undefined behaviour** – ambiguous behaviour (integer overflow, uninitialised reads) is either defined or a compile‑time error.

## 2. Type System

### 2.1 Primitive Types

* **Integers**: `i8`, `i16`, `i32`, `i64`, `i128` (signed); `u8`, `u16`, `u32`, `u64`, `u128` (unsigned); `usize`, `isize` (pointer‑sized).
* **Floats**: `f32`, `f64`.
* **Boolean**: `bool` (1 byte, no arithmetic).
* **Default inference**: integer literals → `usize`; float literals → `f32`.

### 2.2 Mutability Types

Variables and bindings are **immutable by default**. Every type `T` has a corresponding *mutable variant* `mut T`.
* **Default immutability**: In `let` bindings, the inferred type is strictly immutable (e.g., `let a = 10` gives `a` the type `usize`). To create a mutable variable, you must explicitly use a mutable scalar literal: `let b = mut usize.{20};`.
* **Address‑of and Pointer Inference**: The postfix operator `.&` yields a pointer whose mutability strictly matches the variable's declaration.
* `a.&` (where `a` is `T`) yields `*T` (read-only pointer).
* `b.&` (where `b` is `mut T`) yields `*mut T` (read-write pointer).

* **Safe Downgrade**: You can safely assign a mutable reference to an immutable pointer type (e.g., `let p = *usize.{b.&};` or simply rely on inference `let p = b.&;`), but you cannot obtain a mutable pointer from an immutable variable.
* **Array and slice mutability**: Arrays and slices follow the same rules. The mutable variants `[N]mut T` allow element modification.

### 2.3 Pointers and Volatility

* **Normal pointers**: `*T`, `*mut T` – ordinary memory, compiler may optimise.
* **Volatile pointers**: `^T`, `^mut T` – MMIO/hardware registers, no optimisations.
* **Dereference**: `ptr.*` (postfix, allows `ptr.*.field`)
* **Null pointer**: literal `0` must be explicitly cast to a pointer type (e.g., `0 as *i32`). Pointer arithmetic requires converting the pointer to `usize` or `isize` first via `as`.
* **Pointer Arithmetic**: Implicit pointer arithmetic (e.g., `ptr + 1`) is **strictly forbidden**. To compute addresses, you must either:
  1. Cast to `usize`, perform the math, and cast back (e.g., `(ptr as usize + 4) as *u32`).
  2. Use standard library pointer methods (e.g., `ptr.offset(1)`).
* **Casts**: explicit conversion required, e.g. `x as *i32`.

### 2.4 Arrays and Slices

* **Arrays**: `[N]T`, `[N]mut T` – value type, copy on assignment/parameter passing.
* **Array initialisers**: `.{1, 2, 3}`, `.{0; 1024}`, `.{p; 10}` (copy semantics).
* **Slices**: `[]T`, `[]mut T` – fat pointer (pointer + length).
* **Slice creation**: `arr.[start..end]`, `arr.[..]`, `ptr.[0..10]`.
* **Indexing**: `arr.[i]` (dot notation).
* **Length operator**: `#arr` (prefix `#`).

## 3. Declarations and Storage

* **Stack (local)**: `let name = value;` (immutable by default). To declare a specific type or a mutable variable, use type literals: `let name = mut T.{value};`.
* **Static (global)**: `static name = T.{value};`
* **Compile‑time constant**: `const NAME = T.{value};` (inlined, no memory location).
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

* **Initialization and `undef`**: When initializing a struct using `Type.{ ... }`, any field without a default value **must** be explicitly provided; omitting it is a strict compile-time error. If you intentionally want to leave a field uninitialized (retaining garbage data for performance), you must explicitly use the `undef` keyword (e.g., `priority = u8.{undef};`). There is no implicit zero-initialization.

```kern
// Immutable 
let p1 = Point.{x: 10, y: 20};       

// Mutable binding with explicit type and shorthand literal
let p2 = mut Point.{x: 10, y: 20}; 

// Arrays follow the exact same rule
let arr = [3]mut u8.{1, 2, 3};
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

C‑style integer constant sets, but with strict value guarantees.

```kern
type Color: u8 = enum {
    Red = 0,
    Green, // 1
    Blue,  // 2
};
```

Backing type defaults to `u32`.

* **Strict Casts**: The `as` operator guarantees that a value belongs to the enum's defined set. Casting an invalid integer literal (e.g., `99 as Status`) is a compile-time error. Converting dynamic runtime integers to enums must be handled explicitly by the programmer (e.g., via a `switch` statement) to handle invalid hardware/network states.

### 4.4 Conversions

* **`as` operator** – reinterpretation that preserves the bit pattern (pointer casts, trait‑object construction). Cannot be used for numeric conversions that change the representation (e.g., signed/unsigned, integer/float).
* **Numeric conversions** – use intrinsics: `@intToFloat`, `@floatToInt`, `@truncate`, `@zext`, `@sext`.

### 4.5 Manual Vtables

```kern
type FileOps = struct {
    read: fn(*mut File, []u8) usize,
    write: fn(*mut File, []u8) usize,
};
```

## 5. Functions and Traits

### 5.1 Free Functions

Defined at module level.

```kern
pub fn max(a: i32, b: i32) i32 {
    if (a > b) a else b
}
```

> Functions can specify external linkage (e.g., `pub extern fn _start() void`). See Section 8 for FFI details.

### 5.2 Implementation Blocks and Methods

* `impl` blocks attach methods to a type.
* Implicit `self` parameter (type determined by the `impl` target).
* No static methods in `impl` blocks.

```kern
type Point = struct { x: i32, y: i32 };

impl Point {
    pub fn area() i32 { self.x * self.y }
}

impl *mut Point {
    pub fn move_by(dx: i32, dy: i32) void {
        self.x += dx;
        self.y += dy;
    }
}
```

### 5.3 Generics

Monomorphisation.

* Function‑level: `fn map[T, U](input: T) U { … }`
* Impl‑block level: `impl [T: Copy] List[T] { … }`

### 5.4 Traits

Define a set of function signatures; first parameter is implicit `self`.

```kern
type Addable = trait {
    add: fn(Self) Self,
};

type Reader = trait {
    read: fn([]u8) usize,
};
```

Implementation:

```kern
impl i32 : Addable {
    pub fn add(other: i32) i32 { self + other }
}

impl *mut File : Reader {
    pub fn read(buf: []u8) usize { … }
}
```

Traits can require the implementation of other traits (pure semantic composition).

```kern
type ReadWriter: Reader + Writer = trait {
    flush: fn(Self) void,
};
```

### 5.5 Trait Objects

A trait object is a built-in primitive representing a fat pointer (data pointer + vtable pointer). Treat it as a cohesive whole, similar to a slice (`[]T`) or a function pointer (`fn`). It does not require a `*` prefix.

```kern
type File = struct { ... };
impl *mut File : Reader { ... }

let file = mut File.{ ... };
// Step 1: Obtain the concrete pointer
let p = file.&; 

// Step 2: Explicitly cast to a trait object
let r = p as mut Reader; 

// Step 3: Call methods directly (no pointer dereferencing syntax required)
let bytes_read = r.read(buf);

```

* **Syntax and Mutability**: Trait objects use the core mutability modifiers. `Reader` represents an immutable trait object, while `mut Reader` represents a mutable one.
* **Pointer Matching Rule**: If a trait method signature contains `Self` passed or returned by value, it can be implemented by any type for use in generics (static dispatch). However, converting such an implementation to a trait object via `as` is **strictly forbidden** unless the implementation target is explicitly a pointer type (e.g., `impl *mut T : Trait`). This guarantees the compiler always knows the exact stack size (the size of a pointer) during dynamic dispatch.
* **Explicit Upcasting**: Implicit conversion between trait objects is forbidden. To convert a combined trait object into a base trait object, use the explicit `as` operator to adjust the vtable: `let r: mut Reader = rw as mut Reader;`.

### 5.6 Error Handling

No built‑in policy. No exceptions, no panic, no built‑in `Result`. Use `union` + `enum` or integer error codes.

## 6. Control Flow

### 6.1 Conditional Expressions

`if` is an expression.

```kern
let a = if (b < 10) 10 else 20;
let c = if (d > 100) {
    process(d);
    e
} else {
    e - 20
};
```

### 6.2 Switch Expressions

Enhanced C‑style `switch`. No fallthrough.

```kern
let result = switch (val) {
    1..10 => 10,
    11, 12, 13 => 20,
    14..=15 => 30,
    19, 20 => {
        printf("very big!");
        40
    },
    else => 0,
};
```

* **Exhaustiveness**: Switch expressions must be exhaustive. When matching on an `enum`, if all defined variants are covered, an `else =>` branch is **not required** (and potentially warned against as dead code), because Kern guarantees the enum cannot hold unrepresented values. For integer types, an `else =>` branch is mandatory unless the entire type range is covered.

### 6.3 For Loops

Only `for` (no `while`, `do‑while`).

```kern
for (let i = 0; i < 10; i += 1) { … }
for (; cond ;) { … }          // while
for (;;) { … }                // infinite loop
```

### 6.4 Defer

Executes an expression or block when the **current lexical scope (block `{\}`)** exits (LIFO). `defer` is strictly block‑scoped, not function‑scoped.

```kern
let ptr = malloc(1024);
defer free(ptr);

```

### 6.5 Blocks and Expressions

Blocks evaluate to their last expression.

* `expr` – value is used.
* `expr;` – value is discarded (unit/void).

**Evaluation Order with Defer:**
When a block `{ … }` is evaluated as an expression and contains `defer` statements, the exact exit sequence is:

1. **Evaluate**: Compute the value of the final expression.
2. **Execute**: Run all `defer` statements registered in the current block in LIFO (last-in, first-out) order.
3. **Yield**: Pass the computed value to the outer context.

> **Warning**: Returning a pointer to a resource that is freed by a `defer` within the exact same block will result in a dangling pointer. Kern prioritizes explicit execution order over implicit memory protection; such cases are treated as programmer logic errors.

## 7. Modules

### 7.1 Module Resolution

* **Absolute import**: `use path.to.module;` – resolved from project root or external packages.
* **Relative import**: `use .utils;`, `use ..common.types;` – resolved relative to current file.
* No support for `...` or deeper backtracking.

### 7.2 Directory Modules (`init.kn`)

A directory becomes a module if it contains `init.kn`.

* `init.kn` may import its sub‑modules (for re‑export).
* Sub‑modules **must not** import `init.kn` (breaks DAG).
* **Multi-pass Type Resolution**: Kern uses multi-pass parsing. Circular type dependencies (e.g., `Node` contains `*Edge` and `Edge` contains `*Node`) across different module files are fully supported without forward declarations.

### 7.3 Import Syntax

```kern
use std.io;                     // module as namespace
use std.math.PI;                // import item
use std.math.geometry.{ Point, Circle, calculate_area }; // grouping
use std.net.http as h;          // rename module
use std.math.{ max as maximum, min }; // rename item
```

### 7.4 Visibility and Re‑export

* `pub` makes definition visible outside module.
* `pub use` re‑exports items (common in `init.kn`).

### 7.5 Idiom: Static Methods via Modules

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

Kern is designed to interoperate seamlessly with C and assembly language, which is critical for operating system development. In Kern, the C Application Binary Interface (ABI) is the universal language for all external communication.

### 8.1 Exporting Functions to C/Assembly
To make a Kern function callable from an external C program, assembly file, or the linker (e.g., an OS entry point), use the `pub extern` modifiers. 

The `extern` keyword instructs the compiler to use the standard C calling convention and disables name mangling, ensuring the symbol matches the function name exactly.

```kern
// OS entry point called by the bootloader
pub extern fn _start() void {
    // ...
}

```

### 8.2 Importing External Functions and Statics

To call functions or access global variables defined in other languages (like C) or assembly, use an `extern` block. By default, everything inside an `extern` block assumes the C ABI.
Crucially, external C functions can use the `...` syntax to support C-style variadic arguments. External static variables must be declared using the `T.{undef}` literal syntax to maintain consistent declaration semantics.

```kern
extern {
    // Import a standard C function
    fn malloc(size: usize) *mut u8;

    // Import a variadic C function (e.g., for Kern's internal print implementation)
    fn printf(format: *u8, ...) i32;

    // Import an external global variable (e.g., defined in a linker script)
    static MULTIBOOT_MAGIC = u32.{undef};
}

```

## 9. Algebraic Data Types (ADT) and Pattern Matching

To provide robust state management and error handling without introducing exceptions or implicit control flow, Kern introduces Algebraic Data Types (`adt`).

An `adt` is implemented at the physical memory level as a Tagged Union (a hidden scalar discriminant tag followed by a union aligned to its largest variant).

### 9.1 Defining ADTs

ADTs allow for the definition of enumerations where variants can carry distinct data payloads. The syntax strictly follows a `Variant: Type` mapping.

```kern
pub type Option[T] = adt {
    Some: T,
    None,
};

pub type Result[T, E] = adt {
    Ok: T,
    Err: E,
};

```

### 9.2 Elided Initialization Syntax

Where the type context is explicit (e.g., function return types, explicitly typed declarations), ADTs can be initialized using an elided literal syntax `.{ Variant: value }`. This maintains visual consistency with standard `struct` scalar literals while reducing visual noise.

```kern
fn safe_divide(a: i32, b: i32) Result[i32, i32] {
    if (b == 0) {
        // Implicitly inferred as Result[i32, i32].{ Err: -1 }
        return .{ Err: -1 }; 
    }
    return .{ Ok: a / b };
}

```

### 9.3 Pattern Matching (`match`)

Destructuring an `adt` requires the `match` expression. Kern strictly avoids closure-like syntaxes (such as `Variant(val)`) to prevent conceptual overlap with function calls. Instead, `match` bindings perfectly mirror the `adt` definition syntax using a colon (`:`).

* **Syntax and Elision**: Branches are evaluated by matching the ADT variants. You can use the fully qualified path (e.g., `Result[i32, i32].Ok`) or the elided dot-prefix syntax (e.g., `.Ok`) since the type being matched is strictly inferred. Data extraction is performed by mapping the variant to a local binding name: `Variant: binding_name`.
* **Exhaustiveness and `else`**: `match` blocks must be strictly exhaustive. You must either explicitly match all defined variants of the `adt`, or provide a catch-all `else =>` branch. This behaves exactly like `switch` statements for enums.

```kern
let res = safe_divide(10, 0);

// Using the elided syntax (mirroring the definition and initialization)
match (res) {
    .Ok: val => printf("Result: %d\n", val),
    .Err: code => printf("Error code: %d\n", code),
}

// Using the full path syntax and an `else` branch
match (res) {
    Result[i32, i32].Ok: val => {
        printf("Success: %d\n", val);
    },
    else => {
        printf("An error occurred.\n");
    },
}

```

**Interactions and Edge Cases**:

* **No Direct Access**: It is a strict compile-time error to attempt to access an ADT's internal payload without a `match` statement. This enforces memory safety over unions.
* **Nested Matches**: `match` expressions evaluate to a value, allowing them to be bound directly to variables (e.g., `let x = match (res) { ... };`).
* **Empty Variants**: For variants that carry no data payload (e.g., `None`), the colon and binding name are simply omitted (e.g., `.None => { ... }`).

## 10. Stateless Anonymous Functions (Lambdas)

To support inline callbacks and default trait implementations without violating Kern's strict memory rules, the language supports stateless anonymous functions.

### 10.1 Strict Statelessness

Anonymous functions use the `fn(...) ReturnType { ... }` syntax.
Crucially, Kern **strictly forbids environmental capturing (closures)**. An anonymous function cannot access local variables from its enclosing scope. This physical limitation guarantees that anonymous functions compile down to pure, static function pointers (`fn`), entirely preventing use-after-free bugs caused by stack-allocated environments escaping their scope.

```kern
let arr = mut [3]i32.{ 3, 1, 2 };

// Safe, zero-allocation callback
arr.sort(fn(a: i32, b: i32) bool {
    return a < b;
});

```

### 10.2 Interaction with Traits (Default Implementations)

Anonymous functions act as the mechanism for providing default implementations for `trait` methods. Since a `trait` is logically a template for a VTable, providing a default method is semantically identical to providing a default value for a struct field.

```kern
type Shape = trait {
    id: fn() i32,
    
    // Providing a default method via an anonymous function
    area: fn() i32 = fn() i32 {
        return 0;
    },
    
    print_id: fn() void = fn() void {
        printf("ID: %d\n\0" as *u8, self.id());
    }
};

```

**Interactions and Edge Cases**:

* **Implicit `self` Injection**: When an anonymous function is used as the default value for a `trait` field, the compiler's semantic analyzer (Sema) implicitly injects the `self` context into the anonymous function's scope. This allows the default method to call other trait methods (like `self.id()` in the example above) while maintaining the explicit symmetry of the trait declaration syntax.

## Experimental Features (Unstable)

> **Notice**: The features described below are currently experimental. They are scheduled for inclusion in the `0.3.x` and `0.4.x` compiler iterations. The syntax and semantic analysis (Sema) rules are under active development and may be subject to minor adjustments before stabilization.

## 11. Inline Assembly (`@asm`)

To fulfill its role as a systems-level language, Kern provides direct access to hardware via the `@asm` intrinsic. To maintain Kern's philosophy of "explicit over implicit", inline assembly does not use format strings with hidden index bindings. Instead, it leverages Kern's elided struct literal syntax (`.{ ... }`) to create a strict, named mapping between CPU registers and Kern variables.

### 11.1 Syntax and Register Binding

The `@asm` intrinsic takes a single argument: an anonymous configuration object containing the assembly template, register bindings, and compiler directives.

* `asm`: The raw assembly string.
* `outputs`: A mapping of CPU registers to Kern **mutable pointers** (`*mut T`).
* `inputs`: A mapping of CPU registers to Kern scalar values.
* `clobbers`: A list of strings indicating states or registers destroyed by the assembly (e.g., `"memory"`).
* `volatile`: A boolean indicating whether the optimizer should preserve this assembly block even if its outputs appear unused.

```kern
pub fn outb_and_read(port: u16, data: u8) u8 {
    let status = mut u8.{undef};

    @asm(.{
        asm: "out dx, al \n in al, dx",
        outputs: .{
            al: status.&      // Binds the 'al' register to the mutable pointer of 'status'
        },
        inputs: .{
            dx: port,         // Injects 'port' into the 'dx' register
            al: data          // Injects 'data' into the 'al' register
        },
        clobbers: .{ "memory" }, // Informs the compiler that memory state was altered
        volatile: true        // Prevents the optimizer from reordering or deleting
    });

    return status;
}

```

## 12. AST Attributes and Metadata (`#[...]`)

Kern completely rejects traditional C-style preprocessor macros. Instead, it introduces an **Attribute Mini-Language** to handle conditional compilation and AST node metadata injection.

### 12.1 The Boolean Evaluator Model

The core rule of Kern's attribute system is that the content inside `#[...]` is always evaluated as a **boolean expression with potential side effects**.

* **Condition Pruning**: If the expression evaluates to `false`, the immediately following AST node is completely pruned from the compilation process.
* **Short-circuiting**: The evaluator supports logical operators (`and`, `or`, `not`) and strictly short-circuits.
* **Metadata Functions**: Built-in attribute functions (like `export_name`) always evaluate to `true` but carry the *side effect* of attaching metadata to the AST node.

### 12.2 Examples and Idioms

External environment variables (like target OS or architecture) are injected via the compiler CLI (e.g., `kernc -D os="windows"`).

```kern
// 1. Pure Conditional Compilation
#[os == "linux" or os == "macos"]
pub fn unix_syscall() { ... }

// 2. Metadata Injection (Evaluates to true, attaches metadata)
#[export_name("_start") and cold]
pub fn kernel_entry() { ... }

// 3. Combined Logic with Short-circuiting
// If 'os' is not "windows", the evaluation stops, and the node is pruned.
// The 'export_name' side effect will NEVER execute on Linux.
#[os == "windows" and cold and export_name("NtCreateFile")]
pub fn create_file_win() { ... }

```

### 12.3 Built-in Attributes

* **Linkage**: `export_name(String)`, `link_section(String)`.
* **Layout**: `packed`, `align(Integer)`.
* **Optimization**: `cold`, `inline(always)`, `inline(never)`.
* **Diagnostic**: `deprecated(String)`, `allow(String)`.

## 13. Compiler Intrinsics (`@...`)

Intrinsics are special functions implemented directly within the Kern compiler backend (e.g., LLVM). They are prefixed with `@` to strictly separate them from user-defined functions and prevent namespace pollution. Intrinsics are used for operations that cannot be safely or efficiently expressed in pure Kern code.

### 13.1 Type Information and Casts

* `@sizeof[T]() -> usize`: Evaluates to the memory footprint of type `T` at compile time.
* `@intCast[T: Integer, U: Integer](val: T) -> U`: Performs bit-width truncation or zero/sign-extension between integer types.
* `@intToFloat[T: Integer, U: Float](val: T) -> U`: Converts an integer representation to a floating-point representation.
* `@floatCast[T: Float, U: Float](val: T) -> U`: Converts between different floating-point precisions.
* `@floatToInt[T: Float, U: Integer](val: T) -> U`: Truncates a floating-point value into an integer.

### 13.2 Hardware and Bit Manipulation (Planned)

* `@popcount(val)`: Returns the number of 1-bits in the value.
* `@clz(val)` / `@ctz(val)`: Counts leading/trailing zeros. Highly optimized for page table and bitmap management.

### 13.3 Control Flow Optimization (Planned)

* `@unreachable() -> !`: Informs the compiler optimizer that a specific code path is physically impossible to reach. Often used within exhaustive `match` blocks handling hardware constraints to eliminate dead assembly branches.
