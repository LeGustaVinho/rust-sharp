use crate::parser::prec;

/// Emitted binding-power precedence of a `syn::Expr` when formatted to R#,
/// mirroring `crate::parser::prec` so parenthesization cannot drift out of sync with `parser.rs`.
pub(crate) fn syn_emitted_prec(expr: &syn::Expr, unwrapped_single_tail: Option<&syn::Expr>) -> u8 {
    match expr {
        syn::Expr::Assign(_)
        | syn::Expr::Closure(_)
        | syn::Expr::Return(_)
        | syn::Expr::Break(_)
        | syn::Expr::Continue(_)
        | syn::Expr::Yield(_) => prec::ASSIGN,
        syn::Expr::If(_) => prec::TERNARY,
        syn::Expr::Range(_) => prec::RANGE,
        syn::Expr::Try(_)
        | syn::Expr::Await(_)
        | syn::Expr::Unary(_)
        | syn::Expr::Reference(_)
        | syn::Expr::RawAddr(_) => prec::UNARY,
        syn::Expr::Block(_) if unwrapped_single_tail.is_some() => {
            syn_emitted_prec(unwrapped_single_tail.unwrap(), None)
        }
        _ => prec::ATOM,
    }
}

/// Parenthesizes a unary/borrow/deref operand whenever its emitted precedence is looser than
/// `prec::UNARY` (e.g. `&if ...`, `&|| ...`, `&(a..b)`, `!(a = b)`).
pub(crate) fn fmt_unary_operand(text: String, child_prec: u8) -> String {
    if child_prec < prec::UNARY {
        format!("({text})")
    } else {
        text
    }
}

/// Parenthesizes a postfix receiver (`.method()`, `.field`, `(...)`, `[idx]`) whenever its
/// emitted precedence is looser than `prec::POSTFIX` or caller disambiguation requires parens.
pub(crate) fn fmt_receiver(text: String, child_prec: u8, force_parens: bool) -> String {
    if force_parens || child_prec < prec::POSTFIX {
        format!("({text})")
    } else {
        text
    }
}

/// Formats an R# range expression or pattern (`start..end` / `start..=end`),
/// preserving spacing after float-dot literals (`0. ..=1.`).
pub(crate) fn fmt_range(start: &str, inclusive: bool, end: &str) -> String {
    let op = if inclusive { "..=" } else { ".." };
    if start.ends_with('.') {
        format!("{start} {op}{end}")
    } else {
        format!("{start}{op}{end}")
    }
}

/// Formats a `deref [write]` pattern, parenthesizing inner patterns that would otherwise collide
/// with `deref read` or bind looser than `deref`.
pub(crate) fn fmt_deref_pat(is_mut: bool, inner: &str, needs_parens: bool) -> String {
    let kw = if is_mut { "deref write" } else { "deref" };
    if needs_parens {
        format!("{kw} ({inner})")
    } else {
        format!("{kw} {inner}")
    }
}

/// Formats an R# array/matrix type `T[dims]` or `(T)[dims]`.
pub(crate) fn fmt_type_array(elem: &str, elem_needs_parens: bool, dims: &[String]) -> String {
    if elem_needs_parens {
        format!("({elem})[{}]", dims.join(", "))
    } else {
        format!("{elem}[{}]", dims.join(", "))
    }
}

/// Formats an R# reference type `read[<life>] T` or `write[<life>] T`.
pub(crate) fn fmt_type_ref(is_mut: bool, life: Option<&str>, inner: &str) -> String {
    let kw = if is_mut { "write" } else { "read" };
    match life {
        Some(l) => format!("{kw}<{l}> {inner}"),
        None => format!("{kw} {inner}"),
    }
}

/// Formats an R# raw pointer type `read ptr<T>` or `write ptr<T>`.
pub(crate) fn fmt_type_ptr(is_mut: bool, inner: &str) -> String {
    let kw = if is_mut { "write" } else { "read" };
    format!("{kw} ptr<{inner}>")
}

/// Formats an R# tuple type `void`, `(T,)`, or `(T1, T2)`.
pub(crate) fn fmt_type_tuple(elems: &[String]) -> String {
    match elems {
        [] => "void".to_string(),
        [only] => format!("({only},)"),
        many => format!("({})", many.join(", ")),
    }
}
