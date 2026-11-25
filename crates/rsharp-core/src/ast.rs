/// Root AST for an `.rsharp` source file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileAst {
    pub inner_attrs: Vec<Attribute>,
    pub items: Vec<Item>,
}

/// `[Attr]` or `[module: Attr]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub inner: bool,
    pub name: String,
    /// Optional raw argument string inside parentheses, e.g. `Debug, Clone`.
    pub args: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Private,
    Public,
    Internal,
    Super,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub docs: Vec<String>,
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub kind: ItemKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    Namespace(NamespaceDecl),
    Using(UsingDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Union(UnionDecl),
    Trait(TraitDecl),
    Impl(ImplDecl),
    Alias(AliasDecl),
    Const(ConstDecl),
    Static(StaticDecl),
    Fn(FnDecl),
    ExternBlock(ExternBlock),
    MacroDecl(MacroDecl),
    MacroCall(MacroCall),
    InlineRust(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamespaceDecl {
    pub path: Vec<String>,
    /// `None` for file-scoped `namespace Foo.Bar;`, `Some(items)` for `namespace Foo { ... }`.
    pub items: Option<Vec<Item>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsingDecl {
    pub path: Vec<String>,
    pub wildcard: bool,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenericParam {
    pub kind: GenericParamKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GenericParamKind {
    Lifetime(String),
    Type {
        name: String,
        bounds: Vec<TypeBound>,
    },
    Const {
        ty: Type,
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhereClause {
    pub predicates: Vec<WherePredicate>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WherePredicate {
    Type {
        ty: Type,
        bounds: Vec<TypeBound>,
    },
    Lifetime {
        life: String,
        outlives: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeBound {
    Trait(TypePath),
    Lifetime(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDecl {
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub where_clause: Option<WhereClause>,
    pub kind: StructKind,
    /// Inline C#-style methods defined directly inside the struct body.
    pub methods: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StructKind {
    Unit,
    Tuple(Vec<TupleField>),
    Named(Vec<FieldDecl>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldDecl {
    pub docs: Vec<String>,
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub ty: Type,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TupleField {
    pub attrs: Vec<Attribute>,
    pub vis: Visibility,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDecl {
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub where_clause: Option<WhereClause>,
    pub variants: Vec<EnumVariant>,
    /// Inline C#-style methods defined inside the enum body.
    pub methods: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumVariant {
    pub docs: Vec<String>,
    pub attrs: Vec<Attribute>,
    pub name: String,
    pub kind: VariantKind,
    pub discriminant: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VariantKind {
    Unit,
    Tuple(Vec<VariantTupleField>),
    Named(Vec<FieldDecl>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct VariantTupleField {
    pub ty: Type,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnionDecl {
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub where_clause: Option<WhereClause>,
    pub fields: Vec<FieldDecl>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitDecl {
    pub is_unsafe: bool,
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub supertraits: Vec<TypeBound>,
    pub where_clause: Option<WhereClause>,
    pub items: Vec<TraitItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TraitItem {
    Fn(Item),
    AssocType {
        attrs: Vec<Attribute>,
        name: String,
        bounds: Vec<TypeBound>,
        default: Option<Type>,
    },
    AssocConst {
        attrs: Vec<Attribute>,
        ty: Type,
        name: String,
        default: Option<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImplDecl {
    pub is_unsafe: bool,
    pub generics: Vec<GenericParam>,
    pub trait_path: Option<TypePath>,
    pub target_ty: Type,
    pub where_clause: Option<WhereClause>,
    pub items: Vec<ImplItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImplItem {
    Fn(Item),
    AssocType {
        attrs: Vec<Attribute>,
        name: String,
        ty: Type,
    },
    AssocConst(Item),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AliasDecl {
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub target: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConstDecl {
    pub ty: Type,
    pub name: String,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StaticDecl {
    pub is_mut: bool,
    pub ty: Type,
    pub name: String,
    pub value: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExternBlock {
    pub abi: Option<String>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MacroDecl {
    pub name: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MacroCall {
    pub path: Vec<String>,
    pub delim: MacroDelim,
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroDelim {
    Paren,
    Bracket,
    Brace,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnDecl {
    pub is_const: bool,
    pub is_async: bool,
    pub is_unsafe: bool,
    pub is_static: bool,
    pub extern_abi: Option<String>,
    pub return_ty: Type,
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub where_clause: Option<WhereClause>,
    pub body: Option<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Receiver(ReceiverKind),
    Normal {
        is_mut: bool,
        ty: Type,
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReceiverKind {
    Read(Option<String>),
    Write(Option<String>),
    Take,
    Typed { ty: Type },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypePath {
    pub segments: Vec<TypePathSegment>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypePathSegment {
    pub name: String,
    pub args: Vec<GenericArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GenericArg {
    Lifetime(String),
    Type(Type),
    AssocBinding { name: String, ty: Type },
    ConstExpr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Void,
    Never,
    Dynamic,
    Path(TypePath),
    Read {
        life: Option<String>,
        inner: Box<Type>,
    },
    Write {
        life: Option<String>,
        inner: Box<Type>,
    },
    Slice(Box<Type>),
    ReadPtr(Box<Type>),
    WritePtr(Box<Type>),
    Optional(Box<Type>),
    /// `T[N]` (1D) or `T[R, C, ...]` (multidimensional matrix).
    Array {
        elem: Box<Type>,
        dims: Vec<Expr>,
    },
    /// `(T1, T2)` or named tuple `(T1 Name1, T2 Name2)`.
    Tuple(Vec<TupleTypeElem>),
    DynTrait(Vec<TypeBound>),
    ImplTrait(Vec<TypeBound>),
    FnPtr {
        params: Vec<Type>,
        ret: Box<Type>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct TupleTypeElem {
    pub ty: Type,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Local {
        is_shadow: bool,
        is_mut: bool,
        ty: Type,
        name: String,
        init: Option<Expr>,
    },
    DestructureTuple {
        bindings: Vec<(bool, Type, String)>,
        init: Expr,
    },
    Discard(Expr),
    Item(Box<Item>),
    Return(Option<Expr>),
    Break {
        label: Option<String>,
        value: Option<Expr>,
    },
    Continue {
        label: Option<String>,
    },
    If(IfStmt),
    Require {
        expr: Expr,
        pat: Pattern,
        else_block: Block,
    },
    While {
        label: Option<String>,
        cond: Condition,
        body: Block,
    },
    For {
        label: Option<String>,
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        step: Option<Expr>,
        body: Block,
    },
    Foreach {
        label: Option<String>,
        pat: Pattern,
        iter: Expr,
        body: Block,
    },
    Loop {
        label: Option<String>,
        body: Block,
    },
    Match(MatchExpr),
    Unsafe(Block),
    InlineRust(String),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfStmt {
    pub cond: Condition,
    pub then_block: Block,
    pub else_branch: Option<ElseBranch>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ElseBranch {
    ElseIf(Box<IfStmt>),
    Else(Block),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    Bool(Expr),
    IsPattern {
        expr: Expr,
        negated: bool,
        pat: Pattern,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchExpr {
    pub scrutinee: Box<Expr>,
    pub arms: Vec<MatchArm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pat: Pattern,
    pub guard: Option<Expr>,
    pub body: MatchArmBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchArmBody {
    Block(Block),
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Wildcard,
    Lit(Literal),
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        inclusive: bool,
    },
    /// Typed binding e.g. `int x`, `mut int x`, `read int x`, `write int x`.
    Binding {
        is_mut: bool,
        ref_mode: Option<RefMode>,
        ty: Option<Type>,
        name: String,
    },
    Tuple(Vec<Pattern>),
    TupleStruct {
        path: Vec<String>,
        fields: Vec<Pattern>,
    },
    Struct {
        path: Vec<String>,
        fields: Vec<StructPatField>,
        rest: bool,
    },
    Path(Vec<String>),
    Or(Vec<Pattern>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefMode {
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructPatField {
    pub ty: Option<Type>,
    pub is_mut: bool,
    pub name: String,
    pub pat: Option<Pattern>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(String),
    Float(String),
    Str(String),
    Char(String),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Lit(Literal),
    InterpString(Vec<InterpExprPart>),
    Ident(String),
    This,
    SelfType,
    Path {
        segments: Vec<PathExprSegment>,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Assign {
        op: AssignOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `i++` / `i--` / `++i` / `--i`
    Step {
        inc: bool,
        target: Box<Expr>,
    },
    Cast {
        expr: Box<Expr>,
        ty: Type,
    },
    Ternary {
        cond: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
    },
    Coalesce {
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Try(Box<Expr>),
    Await(Box<Expr>),
    Read(Box<Expr>),
    Write(Box<Expr>),
    Take(Box<Expr>),
    Deref(Box<Expr>),
    ReadSlice(Box<Expr>),
    WriteSlice(Box<Expr>),
    Range {
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        inclusive: bool,
    },
    Call {
        callee: Box<Expr>,
        type_args: Vec<GenericArg>,
        args: Vec<Expr>,
    },
    Member {
        object: Box<Expr>,
        field: String,
        type_args: Vec<GenericArg>,
    },
    TupleIndex {
        object: Box<Expr>,
        index: String,
    },
    Index {
        object: Box<Expr>,
        indices: Vec<Expr>,
    },
    ArrayLit(Vec<Expr>),
    ArrayRepeat {
        value: Box<Expr>,
        count: Box<Expr>,
    },
    TupleLit(Vec<Expr>),
    StructLit {
        path: Vec<String>,
        fields: Vec<(String, Expr)>,
    },
    StructUpdate {
        base: Box<Expr>,
        fields: Vec<(String, Expr)>,
    },
    Lambda {
        is_move: bool,
        params: Vec<(bool, Type, String)>,
        ret_ty: Option<Type>,
        body: LambdaBody,
    },
    Match(MatchExpr),
    LabeledBlock {
        label: String,
        block: Block,
    },
    AsyncBlock {
        is_move: bool,
        block: Block,
    },
    UnsafeBlock(Block),
    InlineRust(String),
    MacroCall(MacroCall),
    IsPattern {
        expr: Box<Expr>,
        negated: bool,
        pat: Pattern,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum InterpExprPart {
    Literal(String),
    Expr { expr: Expr, format: Option<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PathExprSegment {
    pub name: String,
    pub type_args: Vec<GenericArg>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    Assign,
    AddAssign,
    SubAssign,
    MulAssign,
    DivAssign,
    RemAssign,
    BitAndAssign,
    BitOrAssign,
    BitXorAssign,
    ShlAssign,
    ShrAssign,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LambdaBody {
    Expr(Box<Expr>),
    Block(Block),
}
