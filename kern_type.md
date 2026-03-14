# The Kern Type System: A Top-Down Bidirectional Model 

Kern implements what is known in programming language theory as **Top-Down Bidirectional Type Checking** (or **Contextual Typing**). Unlike languages that try to "guess" types from the bottom up, Kern enforces a model where types flow from explicit sources of truth down to the leaf nodes of the expression tree.

---

## I. The Three Iron Rules of Type Flow

To understand Kern, one must adopt three fundamental principles that govern how types are assigned and propagated:

### 1. Anchors of Truth (Canonical Sources)
In Kern, types are never "guessed." They are imposed by absolute sources of truth. There are only three anchors that define a type:
1.  **Explicit Constructors**: e.g., `i32.{ 10 }`, `mut File.{ ... }`.
2.  **Function Signatures**: Explicitly defined parameters and return types.
3.  **Struct Field Definitions**: The declared type of a member within a structure.

### 2. Sponge Literals (Contextual Inception)
Pure literals such as `10`, `"hello"`, or `.{}` (anonymous initializers) **do not have an intrinsic physical type** in the AST. They act like "sponges"—they are initially type-neutral and exist only to absorb the **Expected Type** flowing down from a Truth Anchor.

*   In `let x = i32.{ 10 };`, the literal `10` absorbs `i32`.
*   In `let y = u8.{ 10 };`, the literal `10` absorbs `u8`.
*   *Note: If no context is provided (e.g., `let a = 10;`), Kern injects a pragmatic default (e.g., `mut usize`) to maintain ergonomics, but this is treated as a fallback, not the primary rule.*

### 3. Type Sinking & Assimilation
Types flow downward from the Left-Hand Side (LHS) to the Right-Hand Side (RHS). The LHS is always the "authority."
*   **In Assignments and Operations**: In `curr /= 10;`, the type of `curr` (`mut u64`) is the truth. This type "sinks" into the RHS, assimilating the literal `10` into a `u64` instantly.
*   **In Casting and Math**: In `buf.[i] = @intCast[u64, u8](digit) + 48;`, the `@intCast` produces a `u8`. Because the result of the addition is being assigned to a `u8` slot, the literal `48` is assimilated into the `u8` type context.

---

## II. Case Study: The `stdout()` Pattern

How should we handle a situation where a function `stdout()` returns an immutable `File`, but we need a mutable instance to perform writes?

### ❌ Option A: `let out = stdout() as mut File;`
**Status: PROHIBITED.**
In Kern (Section 4.4), the `as` operator is strictly limited to **bit-pattern preserving conversions** (e.g., pointer-to-pointer or pointer-to-usize). Using `as` to force a change in mutability on a value is a violation of memory safety and the "explicit-over-implicit" philosophy.

### ❌ Option B: `let mut out = stdout();`
**Status: SYNTAX ERROR.**
In many languages (like Rust), mutability is an attribute of the **variable name**. In Kern, mutability is an intrinsic part of the **Type System** (`TypeKind::Mut`). `mut` is a type qualifier, not a binding modifier. You cannot prefix an expression call with a type qualifier.

### ✅ Option C: `let out = mut File.{ stdout() };`
**Status: THE KERN WAY (Canonical).**
This syntax perfectly illustrates the Kern philosophy:
1.  `stdout()` yields a `File` value.
2.  We require a **mutable** memory slot to store it.
3.  We use the **Scalar Initialization Syntax**: `Type.{ value }`.
4.  `mut File` is the Truth Anchor. The compiler allocates mutable stack memory and copies the result of `stdout()` into it.

---

## III. Core Differentiators for Systems Programmers

When transitioning from languages like Rust or C++, keep these two distinctions in mind:

1.  **Mutability of Memory, Not Names**: 
    Do not think "I am declaring a mutable variable." Think "I am initializing a piece of **mutable memory type**." In LLVM terms, `mut Type.{...}` maps directly to an `alloca` instruction that is not marked as `constant`.

2.  **Assembly of Trait Objects, Not Coercion**: 
    Kern rejects "Unsized Coercion." A Trait Object is a concrete struct (data pointer + vtable pointer). You do not "cast" a pointer to a trait; you **explicitly assemble** the trait object: `mut Writer.{ out_file.& }`.

---

### Summary
Kern's type system does not attempt to be "smart" by guessing intent. Instead, it is **disciplined**—it provides a rigid pipeline where explicit type information flows from the programmer's definitions down to the smallest literal, ensuring that "what you see is exactly what the machine executes."