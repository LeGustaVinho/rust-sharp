# Transactions, failable conditions, and loops in R#

**Language proposal.** `try` propagates a failure. `guard` rejects the current test or iteration attempt. `transaction` and `transactional` identify the state to restore when a failure leaves a protected scope.

| Where failure occurs | Where control goes | What happens to state |
| --- | --- | --- |
| A `transaction` block or `transactional` method returns `Err` or `None` | The caller receives that result. | Restore the resources protected at that level. |
| A `guard` fails in an `if` condition | Evaluate the next `else if`, enter the `else`, or continue after the `if`. | Keep effects already performed by the test. |
| A `guard` fails in a loop's `when` filter | Advance to the next candidate. | Keep effects performed by `when`. |
| A clause rejects an attempt in `for (...) { ... } do { ... }` | Advance the generator responsible for the attempt. | Restore the protected writes made by that attempt. |
| An operation fails inside an accepted `if` branch or `do` body | Follow the error handling written in the body. | An enclosing transaction can still roll back the whole operation. |

## 1. Transaction blocks and methods

### Declaration and outcome

`transaction (write a, write b) { ... }` is an expression. Its header explicitly lists the external resources the block can modify. A `transactional` method takes its resource list from its `write this` receiver and `write` parameters. The first version supports synchronous functions and methods returning `Result<T, E>` or `Option<T>`, including the `T?` spelling.

```csharp
public transactional Result<void, TransferError> TransferTo(
    write this, write Account target, long amount)
{
    try this.Debit(amount);
    return target.Credit(amount);
}

var result = transaction (write source, write target) {
    try source.TransferTo(write target, 30);
    return CheckLimit(read target);
};
```

Given opening balances of `100` and `20`, these outcomes define the contract.

| Scenario | Result | Final balances |
| --- | --- | --- |
| Debit, credit, and limit check all succeed | `Ok(())` | `70`, `50` |
| Debit succeeds and credit fails | The credit error | `100`, `20` |
| The transfer succeeds and `CheckLimit` subsequently fails | The limit error | `100`, `20` |

### Execution rules

1. Evaluate each resource target once, from left to right. Establish all checkpoints before entering the body. If preparing a resource fails, restore the resources already prepared in reverse order and return the failure.
2. `Ok(value)` and `Some(value)` commit the current level, including `Ok(false)` and `Some(false)`. `Err(error)` and `None` roll back that level. The caller receives the corresponding result.
3. Inside a `transaction` block, `return` and `try` target that block. A domain error can be raised with `return Err(TransferError.LimitExceeded);`. Error conversion for `try` follows Rust's rules. Converting `Option` to `Result` requires specifying which error represents `None`.
4. A program can handle a failure with `match`, `is`, or `guard` and continue the transaction. Diagnose discarded fallible results inside the protected flow, including explicit discards such as `_ = operation();`.
5. Each nested transaction block or transactional call establishes a checkpoint. The caller can recover from an inner failure. After inner success, an outer failure can still undo the inner changes. A nested call can modify resources protected by the caller and its own local state.
6. An unwinding panic restores the active resources and continues unwinding. In the first version, diagnose `await` inside a transaction and `break` or `continue` that would cross an explicit transaction boundary. Loops wholly within the transaction support their usual jumps. [Rust destructors and drop scopes](https://doc.rust-lang.org/reference/destructors.html)

The same rules apply to optional results. If only one seat is available, the first reservation succeeds, the second returns `None`, and the first seat becomes available again.

```csharp
int? firstSeat = transaction (write seats) {
    int first = try seats.Reserve();
    try seats.Reserve();
    return Some(first);
};
```

### Extending rollback to types and libraries

Expose a public `TransactionalResource` trait in `rsharp-prelude`. Each implementation specifies its checkpoint type and supplies operations to capture state, restore it, and release a checkpoint on success. Restoration and release must be synchronous and infallible. If capturing a checkpoint can fail, the adapter must leave its own resource intact on that failure.

- Support scalars, `String`, arrays, tuples, owned collections, and structures whose components have verifiable restoration behavior. Allow full snapshots and change logs.
- A `ByteBuffer` that supports overwriting can snapshot its original `Vec<u8>`. An append-only buffer can save its length and call `truncate` on rollback. Overwriting one byte without changing the length distinguishes the guarantees of these two adapters.
- Accept writes to fields and elements of listed resources and to owned local values. Check transitive calls, overloaded operators, destructors, interior mutability, and escaping references. Diagnose overlapping targets and unknown effects at the operation responsible. For external libraries, trust only the operations covered by their adapter. A blanket `Clone` implementation would be unsound as a rollback promise because cloning an `Arc` can preserve access to the same mutable state. [Rust `Clone` semantics](https://doc.rust-lang.org/std/clone/trait.Clone.html)

## 2. Failable `if`, `else if`, and conditional expressions

`guard expr` evaluates its operand once. On `Ok(value)` or `Some(value)`, it extracts `value` and continues the test. On `Err` or `None`, it immediately rejects the **current condition**. Each `else if` starts a new condition with its own `guard` scope.

```csharp
if (guard CheckPrimary(id) || CheckBackup(id)) {
    Allow(id);
} else if (guard HasManualApproval(id)) {
    AllowManually(id);
} else {
    Deny(id);
}
```

| `CheckPrimary(id)` | Calls to `CheckBackup(id)` | Next action |
| --- | --- | --- |
| `Ok(true)` | Zero | Call `Allow`. |
| `Ok(false)` | One | Call `Allow` if the backup approves, otherwise evaluate the `else if`. |
| `Err(error)` | Zero | Evaluate the `else if` immediately. |

The table also specifies the Rust lowering. Using `unwrap_or(false)` for the first operand would incorrectly call the backup in the third case.

- **Scope.** A `guard` belongs to the nearest enclosing test that handles it. Rejection in an `if` without an `else` continues after the statement. Entering the branch ends that condition's failure context. A `try` in the body propagates to its own enclosing boundary.
- **Test effects.** If `CheckPrimary` increments a counter and returns `Err`, the increment remains. If `CheckPrimary` is itself `transactional`, the call restores its protected resources before delivering the error to `guard`. The operation's author can therefore choose which changes roll back.
- **Existing expression forms.** Apply the same local failure rule to an `if` used as a value, the condition of `?:`, and a `when` guard on a `match` arm. A rejected match guard moves to the next arm. Preserve the scope of bindings introduced by `is` and the short-circuit ordering of `&&` and `||`.
- **Failable alternatives.** `otherwise` tries the right operand after a local failure of the left operand and restores the first attempt's speculative effects. `fails(expr)` reports whether the attempt would fail and discards its changes on either outcome. Both operators use checkpoints and require verifiable effects. `||` keeps its usual Boolean meaning.
- **Access to the error.** Writing `guard` explicitly discards the error detail. Use `match` on the `Result` when the alternate branch needs to distinguish a missing user from an unavailable service, for example.

## 3. Two forms of `for`, each with an optional `when`

```csharp
for (init; test; step) { body }

for (init; test; step) when (filter) { body }

for (...) { clauses } do { body }

for (...) when (filter) { clauses } do { body }
```

### Classic `for`

The middle condition controls termination. The optional `when` determines whether the current candidate enters the body. If the filter rejects the candidate, the loop runs the step **exactly once**.

```csharp
for (mut int i = 0; i < 5; i++) {
    Visit(i);                              // 0, 1, 2, 3, 4
}

for (mut int i = 0; i < 5; i++) when (i % 2 == 0) {
    Visit(i);                              // 0, 2, 4
}
```

Preserve `for (;;)`, existing labels, `continue`, and `break`. A `continue` goes through the target loop's step. A `break` exits that loop without another step.

### `for` with speculative preparation and a `do` body

The parentheses can be empty or contain `write` resources, a generator, classic loop control, or resources followed by either form of loop control. When the parentheses supply no iteration control, the preparation block must contain a generator. Definitions and tests may precede it.

```csharp
mut int stock = 5;
mut int checks = 0;

List<int> accepted = for (write stock, read str raw in requests)
    when (CountAndParse(raw, write checks) is Ok(int qty) && qty > 0)
{
    stock -= qty;
    stock >= 0;
} do {
    return qty;
};
```

Assume `CountAndParse` increments `checks` once per call. For `["3", "bad", "4", "2", "0"]`, the required outcome is **`accepted = [3, 2]`, `stock = 0`, and `checks = 5`**. The `when` rejects `"bad"` and `"0"`. The request for `"4"` passes the filter and reduces the provisional stock from `2` to `-2`. Preparation rejects that attempt and restores `2`, allowing the next request to succeed.

### Evaluation order, scope, and nesting

1. Obtain a primary candidate, evaluate the optional `when`, execute the remaining preparation clauses in order, and enter `do` on success. Evaluate `when` once per primary candidate. Its pattern bindings, such as `qty` above, are available during preparation and in the body.
2. With `for () when (...)`, run the filter when the first generator in the block supplies its candidate. The filter can use that item and names available when producing it. If a clause before the first generator fails, the expression produces zero items.
3. A standalone Boolean expression in preparation is a filter. `qty > 0;` can reject an attempt. `bool positive = qty > 0;` binds the Boolean value and continues. `guard` extracts a `Result` or `Option` in an intermediate declaration.
4. An inner generator may depend on earlier bindings and opens its own checkpoint level. Clauses between generators run once per outer candidate. Reaching an inner generator accepts the outer prefix. Rejecting a child restores that child's protected writes and preserves accepted siblings. An empty inner generator contributes zero items. Preserve the source iterator's advance so the next candidate can be reached.
5. In expression position, the final `return` in `do` supplies one `List<T>` element. Zero accepted candidates produce an empty list. Multiple generators in a single `for` produce a flat list. Returning another `for` from the `do` produces nested lists. In statement position, execute the body without allocating a result list.
6. A `do` body can contain several statements. If `Process()` propagates an error after acceptance, ordinary body-level error handling applies. Enclose the loop in `transaction` and list its resources when a later failure must also undo previously accepted work.

This example puts both generators in the first block and omits `when`.

```csharp
List<int> amounts = for () {
    Order order in orders;
    order.IsOpen;
    Line line in order.Lines;
    int price = guard FindPrice(line.Sku);
    int amount = price * line.Quantity;
    amount >= 100;
} do {
    return amount;
};
```

A closed order never opens its line iterator. If one line has no price, the loop still considers the next line of the same order. Index/value and key/value iteration use tuple patterns and the appropriate iterator. Collection preserves the order supplied by that source. [Verse generators, filters, and collection](https://verselang.github.io/book/07_control/#for-expressions)

### Other loops

`foreach`, `while`, and `loop` also accept `when (...)` as a simple filter. In `while (cursor.Next() is Some(Row row)) when (guard IsValid(read row))`, the cursor advances before the filter. An invalid row proceeds to the next read. In a `while` or `loop`, putting the only advance inside the body can cause the same rejected candidate to repeat indefinitely.

## 4. Compiler, tools, and bidirectional conversion

1. **Parser and public AST.** Represent the transactional function modifier, transaction blocks, `guard`, and the attempt operators. Reuse the condition representation for `if`, patterns, and `when`. Keep a classic `for` node and add a `for ... do` node with ordered clauses. After iteration control and the optional `when`, recognize `do` following the first balanced block.
2. **Semantic analysis.** Resolve resources, receivers, calls, and cross-module effects before emission. Enforce the same contract for free functions, instance methods, trait declarations, and implementations. Point diagnostics at the responsible R# source, including irreversible operations, overlapping borrows, and references that would escape protection.
3. **R# to Rust.** Generate stable Rust. Use scope guards for restoration, `match` or `let ... else` for local failures, and nested loops for multiple generators. Preserve public method signatures, evaluation order, borrow scopes, destructor behavior, and labels. Read-only headers need no checkpoints. Support `--prelude embedded`, `crate`, and `none`. With `none`, the project explicitly supplies the runtime when it is needed.
4. **Rust to R#.** Attach versioned markers to generated expansions whose original R# spelling could be ambiguous. Validate the entire expansion with `syn` before reconstructing `transactional`, `transaction`, failable `if`, `when`, or either `for` form. Recover the syntax after `rustfmt`. When a marker is missing or a structural edit invalidates it, translate the Rust actually present. A handwritten `Drop`, `match`, or `continue` alone cannot establish transactional intent.
5. **User-facing integration.** Update the language specification, runnable examples, English documentation, file and project CLI conversion, LSP diagnostics and scope information, VS Code grammar, and Rust synchronization.

## 5. Acceptance criteria and limitations

| Area | Cases to compile and execute |
| --- | --- |
| Transactions | Cover success, explicit failure, `try`, `None`, `Ok(false)`, failure when preparing the second resource, handled inner failure, inner success followed by outer failure, and panic unwinding. Assert the returned error, every participant's final state, and restoration order. |
| Conditionals | Cover `Ok(true)`, `Ok(false)`, `Err`, `Some`, and `None`. Count calls across `&&` and `\|\|`. Exercise `else if`, omitted `else`, value-producing `if`, pattern bindings, errors in the branch body, test effects that persist, and attempts restored by `otherwise` or `fails`. |
| Loops | Cover all four syntax combinations. Assert filter call counts, the step after rejection or `continue`, exit on `break`, stock and check count, dependent and empty generators, and a rejected child between two accepted siblings. Compare the flat result `[13, 14, 23, 24]` with `[[13, 14], [23, 24]]` from nested expressions. |
| Interoperability | Execute both R# → Rust → R# and Rust → R# → Rust. Compare values, errors, state, and effect order. Include `rustfmt`, edited or removed markers, handwritten Rust, multiple modules, and `--verify --no-raw-rust`. To assess Verse parity, run the case with writes before and after the second generator in Verse as well, including an empty inner generator. |

**Limits of the guarantee.** Sending an email, publishing to a socket, and writing terminal output are irreversible under this contract. For a purchase receipt, one option is to store the order and the intent to send in the same database transaction, then publish after commit. Coordinating independent databases requires a separate protocol. Snapshots use memory proportional to the protected state. Change logs use memory proportional to the mutations. Exclusive borrows last as long as the protection. Recovery from process termination or `panic=abort` depends on the resource's durability.

**Verification baseline.** At revision `a4b6de5`, the existing suites passed. Executable Rust references checked the balances, short-circuit calls, stock, filter calls, and both collection shapes. The editor measured **100% line and branch coverage**. The Rust workspace measured **99.87% line coverage and 93.58% branch coverage**. The implementation must meet the project's 100% target. The language syntax is proposed, and the comparison of effects between generators still needs to be executed in Verse.
