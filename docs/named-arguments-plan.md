# Named arguments in R#

**Proposed contract.** A caller can use the names already present in a function or method declaration. The name selects the parameter. Execution evaluates the receiver first and each argument once, from left to right as written. Rust callers keep the ordinary positional signature. [C# named arguments](https://learn.microsoft.com/dotnet/csharp/programming-guide/classes-and-structs/named-and-optional-arguments) · [Rust operand evaluation order](https://doc.rust-lang.org/reference/expressions.html#evaluation-order-of-operands)

## Call syntax

Assume `int pack(int left, int right)` returns `10 * left + right`.

| R# call | Result or diagnostic |
| --- | --- |
| `pack(1, 2)`, `pack(left: 1, 2)`, `pack(1, right: 2)` | All return `12`. |
| `pack(right: 2, left: 1)` | Returns `12`. The expression for `right` runs first. |
| `pack(right: 2, 1)` | A positional argument follows an out-of-position named argument. |
| `pack(1, left: 2)` | Two arguments supply `left`. |
| `pack(width: 1, right: 2)`, `pack(right: 2)` | Unknown `width`, or missing `left`, respectively. |

Match case-sensitive declaration identifiers, including their existing `@keyword` escape. Each required slot must be filled exactly once. Optional/default values are a separate language feature. Keep `read`, `write`, `take`, `try` and `await` on the value side of the colon. An implicit `this` has no argument slot.

Use the existing colon only at the start of an argument. Preserve the existing positional expression in `f(exitHere: { break exitHere 7; })`, even if the function has a parameter named `exitHere`. Give the existing labeled block, `loop`, `while` and `foreach` forms precedence. Parentheses around the value disambiguate a named argument, e.g. `f(value: ({ return 7; }))` or `f(value: (loop { break 7; }))`. This decision is syntactic and independent of name resolution.

## Implementation order

1. **Parse and bind.** In `rsharp-core`, change public `Expr::Call.args` from `Vec<Expr>` to a source-ordered list of values with an optional label and its span. Retain the call span for missing-argument diagnostics. Update both callers of `parse_arg_list`, generic calls, the chain emitter and the special `panic` path. Keep indices, initializers, attributes and macro token trees on their existing grammar. Reuse parameter declarations. A binder should return a parameter index for each written argument and diagnose unknown, repeated, omitted and excess arguments at the call site. Preserve the existing fast path for entirely positional calls. Document the public Rust AST source-compatibility change.
2. **Resolve the actual declaration.** Extend declaration collection in `codegen_rust.rs` to index ordered parameter names by declaration identity, module and owner. Start with local and free functions, forward references and qualified associated functions. Resolve lexical shadowing and import aliases before binding. Add inherent methods for proven receiver types, then trait methods using the trait declaration's names, even when an implementation renames its bindings. Distinguish `obj.run(...)` from `(obj.run)(...)`. A missing or ambiguous signature gets a diagnostic suggesting positional syntax. A matching method name alone cannot establish identity. [Rust method lookup](https://doc.rust-lang.org/reference/expressions/method-call-expr.html)
3. **Emit Rust with the same effects.** If the written order matches the signature, emit the existing positional call plus the round-trip comment described below. For a reordered free function with scalar arguments, evaluate the expressions in a tuple in source order, then pass the bound values in parameter order. Start with known scalar types and expressions for which the expected type and temporary scopes are preserved. Use fresh bindings that cannot shadow source names. The example starts the counter at zero. Each call to `next` increments it and returns its new value.

   ```csharp
   pack(right: next(write counter), left: next(write counter))
   ```

   ```rust
   /*rsharp:named:v1 right,left*/
   match (next(&mut counter), next(&mut counter)) {
       (arg0, arg1) => pack(arg1, arg0),
   }
   ```

   The result must be `21`, with the counter at `2`. For reordered methods, keep native method-call syntax so Rust evaluates the receiver first and can reserve a mutable borrow. One implementation for statement contexts is a fresh, late-initialized local per saved value. Evaluate the tuple inside the first Rust argument, return that slot and move the saved values into the remaining slots. Scope declarations to the actual invocation, including loops and closures. Enable each additional shape only after checking expected-type coercions, implicit reborrows, temporary lifetimes, observable `Drop`, panic and early exits. Issue a call-site diagnostic for unsupported reorderings. A `let` per argument can drop a guard at the end of its initializer and change what the next argument observes. [Rust drop scopes](https://doc.rust-lang.org/reference/destructors.html#drop-scopes) · [Two-phase borrows](https://rustc-dev-guide.rust-lang.org/borrow-check/two-phase-borrows.html)

4. **Recover written arguments in `from-rust`.** Plain Rust `pack(1, 2)` imports as positional R#. It could have originated from several R# spellings. For generated Rust, attach one versioned ordinary comment to each named call or its generated expression. Record the labels in written order and use `_` for a positional argument. Read it from Rust source alongside the `syn` spans. Before restoring labels or folding generated bindings, resolve the signature, validate the complete expression and every binding use, and verify that re-emitting the candidate gives the same Rust token structure modulo formatting and fresh names. Include any generated declarations in that check. On a missing, stale or invalid marker, translate the actual Rust, including its evaluation sequence. The guarantee covers labels, order and behavior. Formatting and redundant escapes can normalize. [Rust call grammar](https://doc.rust-lang.org/reference/expressions/call-expr.html)
5. **Integrate project and editor paths.** Project emission in `parallel.rs` currently transpiles each file independently. Collect a read-only signature index across the project's modules before parallel binding and emission. Use the same resolution for single-file commands when project context is available, including `check`, `build`, `run`, preview, save synchronization and reverse verification. Keep standalone conversion usable when all needed declarations are local. Teach LSP diagnostics, parameter navigation and rename to associate labels with the resolved declaration. In particular, renaming a parameter must update its named callers across files. Update the syntax specification, an example and the TextMate grammar where needed.

## Boundaries and acceptance

- Renaming a public parameter becomes source-breaking for R# callers that name it. Rust positional callers retain their ABI and calling convention. Parameter names in Rust dependencies can change across releases. Support external APIs only when their actual declaration and active configuration are available and reproducible in the forward compiler.
- Destructured parameters and `_`, anonymous function-pointer or `Fn` slots, tuple constructors/variants and a variadic tail have no stable name to bind by default. Leave those slots positional. Macros own their token syntax. Keep struct-field initialization and indexing separate. An explicit signature can be considered later for callable values.
- Rust containing reordered calls is longer. Project resolution adds an indexing pass. Removing the round-trip comment or changing the generated structure may leave explicit temporaries or a `match` in the recovered R#. Handwritten Rust stays editable and executable with ordinary Rust tools.
- Add parser/binder cases to `unit_lexer_parser` and `semantic_tests` for the table, escapes, trailing commas, generics, nested ternaries, labeled loops/blocks, shadowing, trait/impl name differences and unresolved callees. Include interpolated expressions with format specifiers, cross-file aliases and same-named methods on different types.
- In `differential_tests`, compile both programs and compare results and event logs. Cover exactly-once evaluation, receiver effects, early return/`?`, short-circuit branches, non-`Copy` moves, repeated mutable reborrows, closure coercions, a mutating method that reads the receiver in an argument, async and const contexts, and a guard with observable destruction. Require a diagnostic for every deliberately unsupported shape.
- In `roundtrip_tests` and the CLI/LSP suites, cover named, mixed and positional calls in both directions, nested calls, `rustfmt`, edited/removed markers, handwritten lookalikes, and project runs with and without rust-analyzer. Require identical observable behavior after a second conversion and restored label order for intact generated calls. Today's `--verify` re-transpiles the text. The differential fixtures must also compile and execute it. Run the full workspace and editor suites and check the repository's 100% line and branch coverage gate.
