/// Root AST for an `.rsharp` source file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileAst {
    pub shebang: Option<String>,
    pub inner_docs: Vec<String>,
    pub inner_attrs: Vec<Attribute>,
    pub items: Vec<Item>,
}

/// Delimiter form for attribute arguments.
#[derive(Debug, Clone, PartialEq)]
pub enum AttrArgs {
    Paren(String),
    Bracket(String),
    Brace(String),
    Eq(String),
}

/// `[Attr]`, `[unsafe Attr]`, or `[module: Attr]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub inner: bool,
    pub is_unsafe: bool,
    pub name: String,
    pub args: Option<AttrArgs>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Visibility {
    Private,
    Public,
    Internal,
    Super,
    This,
    InPath(Vec<String>),
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
    ExternCrate(ExternCrateDecl),
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
    pub inner_docs: Vec<String>,
    pub inner_attrs: Vec<Attribute>,
    /// `None` for file-scoped `namespace Foo.Bar;`, `Some(items)` for `namespace Foo { ... }`.
    pub items: Option<Vec<Item>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsingDecl {
    pub tree: UsingTree,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UsingTree {
    Path {
        global: bool,
        prefix: Vec<String>,
        wildcard: bool,
        alias: Option<String>,
    },
    Group {
        global: bool,
        prefix: Vec<String>,
        items: Vec<UsingTree>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExternCrateDecl {
    pub name: String,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenericParam {
    pub attrs: Vec<Attribute>,
    pub kind: GenericParamKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GenericParamKind {
    Lifetime {
        name: String,
        outlives: Vec<String>,
    },
    Type {
        name: String,
        bounds: Vec<TypeBound>,
        default: Option<Type>,
    },
    Const {
        ty: Type,
        name: String,
        default: Option<Box<Expr>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct WhereClause {
    pub predicates: Vec<WherePredicate>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WherePredicate {
    Type {
        lifetimes: Vec<String>,
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
    Relaxed(TypePath),
    Forall {
        lifetimes: Vec<String>,
        trait_path: TypePath,
    },
    Paren(Box<TypeBound>),
    Lifetime(String),
    PreciseCapture(Vec<CaptureArg>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum CaptureArg {
    Lifetime(String),
    TypeOrConst(String),
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
    pub attrs: Vec<Attribute>,
    pub ty: Type,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnionDecl {
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub where_clause: Option<WhereClause>,
    pub fields: Vec<FieldDecl>,
    /// Inline C#-style methods defined directly inside the union body.
    pub methods: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraitDecl {
    pub is_unsafe: bool,
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub supertraits: Vec<TypeBound>,
    pub where_clause: Option<WhereClause>,
    pub inner_docs: Vec<String>,
    pub inner_attrs: Vec<Attribute>,
    pub items: Vec<TraitItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TraitItem {
    Fn {
        docs: Vec<String>,
        attrs: Vec<Attribute>,
        vis: Visibility,
        decl: FnDecl,
    },
    AssocType {
        docs: Vec<String>,
        attrs: Vec<Attribute>,
        name: String,
        generics: Vec<GenericParam>,
        bounds: Vec<TypeBound>,
        where_clause: Option<WhereClause>,
        default: Option<Type>,
    },
    AssocConst {
        docs: Vec<String>,
        attrs: Vec<Attribute>,
        ty: Type,
        name: String,
        default: Option<Expr>,
    },
    MacroCall {
        attrs: Vec<Attribute>,
        call: MacroCall,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImplDecl {
    pub is_unsafe: bool,
    pub generics: Vec<GenericParam>,
    pub trait_path: Option<TypePath>,
    pub target_ty: Type,
    pub where_clause: Option<WhereClause>,
    pub inner_docs: Vec<String>,
    pub inner_attrs: Vec<Attribute>,
    pub items: Vec<ImplItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImplItem {
    Fn {
        docs: Vec<String>,
        attrs: Vec<Attribute>,
        vis: Visibility,
        decl: FnDecl,
    },
    AssocType {
        attrs: Vec<Attribute>,
        name: String,
        generics: Vec<GenericParam>,
        where_clause: Option<WhereClause>,
        ty: Type,
    },
    AssocConst(Item),
    MacroCall {
        attrs: Vec<Attribute>,
        call: MacroCall,
    },
    RawRust(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AliasDecl {
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub where_clause: Option<WhereClause>,
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
    pub is_safe: bool,
    pub is_unsafe: bool,
    pub is_mut: bool,
    pub ty: Type,
    pub name: String,
    /// `None` only inside `extern` blocks.
    pub value: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExternBlock {
    pub is_unsafe: bool,
    pub abi: Option<String>,
    pub inner_docs: Vec<String>,
    pub inner_attrs: Vec<Attribute>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MacroDecl {
    pub name: String,
    pub delim: MacroDelim,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MacroCall {
    pub global: bool,
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
pub struct VariadicParam {
    pub attrs: Vec<Attribute>,
    pub is_mut: bool,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnDecl {
    pub is_const: bool,
    pub is_async: bool,
    pub is_safe: bool,
    pub is_unsafe: bool,
    pub is_static: bool,
    pub extern_abi: Option<String>,
    pub return_ty: Type,
    pub name: String,
    pub generics: Vec<GenericParam>,
    pub params: Vec<Param>,
    pub variadic: Option<VariadicParam>,
    pub where_clause: Option<WhereClause>,
    pub body: Option<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Receiver {
        attrs: Vec<Attribute>,
        kind: ReceiverKind,
    },
    Normal {
        attrs: Vec<Attribute>,
        is_mut: bool,
        ty: Type,
        name: String,
    },
    Pattern {
        attrs: Vec<Attribute>,
        pat: Pattern,
        explicit_ty: Option<Type>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReceiverKind {
    Read(Option<String>),
    Write(Option<String>),
    Take { is_mut: bool },
    Typed { is_mut: bool, ty: Type },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub inner_docs: Vec<String>,
    pub inner_attrs: Vec<Attribute>,
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypePath {
    pub global: bool,
    pub segments: Vec<TypePathSegment>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypePathSegment {
    pub name: String,
    pub args: TypePathArgs,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypePathArgs {
    None,
    Angle(Vec<GenericArg>),
    Parenthesized {
        params: Vec<Type>,
        ret: Option<Box<Type>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum GenericArg {
    Lifetime(String),
    Type(Type),
    AssocBinding {
        name: String,
        generics: Vec<GenericArg>,
        ty: Type,
    },
    AssocBound {
        name: String,
        generics: Vec<GenericArg>,
        bounds: Vec<TypeBound>,
    },
    ConstExpr(Expr),
    ConstBlock(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnPtrParam {
    pub attrs: Vec<Attribute>,
    pub name: Option<String>,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Void,
    Never,
    Dynamic,
    /// Partial placeholder `_` inside generic arguments or casts (never standalone local variables).
    Infer,
    Path(TypePath),
    QualifiedPath {
        self_ty: Box<Type>,
        trait_path: Option<TypePath>,
        segments: Vec<TypePathSegment>,
    },
    Grouped(Box<Type>),
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
        lifetimes: Vec<String>,
        is_unsafe: bool,
        extern_abi: Option<String>,
        params: Vec<FnPtrParam>,
        variadic: Option<Vec<Attribute>>,
        ret: Box<Type>,
    },
    Macro(MacroCall),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TupleTypeElem {
    pub ty: Type,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub attrs: Vec<Attribute>,
    pub kind: StmtKind,
}

impl Stmt {
    pub fn plain(kind: StmtKind) -> Self {
        Self {
            attrs: Vec::new(),
            kind,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Local {
        is_shadow: bool,
        is_mut: bool,
        ty: Type,
        name: String,
        init: Option<Expr>,
    },
    Destructure {
        pat: Pattern,
        explicit_ty: Option<Type>,
        init: Expr,
    },
    DestructureAssign {
        pat: Pattern,
        value: Expr,
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
        ty: Option<Type>,
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
    Block(Block),
    Empty,
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
    LetChain(Vec<ChainItem>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChainItem {
    IsPattern { expr: Expr, pat: Pattern },
    Bool(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchExpr {
    pub scrutinee: Box<Expr>,
    pub arms: Vec<MatchArm>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub attrs: Vec<Attribute>,
    pub pat: Pattern,
    pub guard: Option<Condition>,
    pub body: MatchArmBody,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchArmBody {
    Block(Block),
    Expr(Expr),
}

/// Path in pattern position, supporting `global::`, `<Type [as Trait]>::`, and generic args.
#[derive(Debug, Clone, PartialEq)]
pub struct PatPath {
    pub global: bool,
    pub qself: Option<(Box<Type>, Option<TypePath>)>,
    pub segments: Vec<TypePathSegment>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Wildcard,
    Rest,
    Lit(Literal),
    Const(Box<Expr>),
    Range {
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        inclusive: bool,
    },
    /// Typed binding e.g. `int x`, `mut int x`, `read int x`, `write int x`.
    Binding {
        is_mut: bool,
        ty: Option<Type>,
        name: String,
    },
    /// Explicit borrow-out binding `borrow read string s` (`ref s`) / `borrow write string s` (`ref mut s`).
    BorrowRef {
        mode: RefMode,
        ty: Type,
        name: String,
    },
    /// Reference-stripping pattern `deref int val` (`&val`) or `deref write int val` (`&mut val`).
    DerefPat {
        is_mut: bool,
        inner: Box<Pattern>,
    },
    /// Parenthesized pattern `(1 | 2)`.
    Paren(Box<Pattern>),
    /// Subpattern binding `<pat> as [borrow (read|write) | mut] <Type> <name>` (`[ref] [mut] name @ pat`).
    BindAt {
        ref_mode: Option<RefMode>,
        is_mut: bool,
        ty: Type,
        name: String,
        subpat: Box<Pattern>,
    },
    Tuple(Vec<Pattern>),
    Slice(Vec<SlicePatElem>),
    TupleStruct {
        path: PatPath,
        fields: Vec<Pattern>,
    },
    Struct {
        path: PatPath,
        fields: Vec<StructPatField>,
        rest: bool,
    },
    Path(PatPath),
    Or(Vec<Pattern>),
    Macro(MacroCall),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SlicePatElem {
    Pat(Pattern),
    Rest,
    RestBind {
        ref_mode: Option<RefMode>,
        is_mut: bool,
        ty: Option<Type>,
        name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefMode {
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructPatField {
    pub attrs: Vec<Attribute>,
    pub ref_mode: Option<RefMode>,
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
    RawStr(String),
    Byte(String),
    ByteStr { value: String, raw: bool },
    CStr { value: String, raw: bool },
    Char(String),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructFieldInit {
    pub attrs: Vec<Attribute>,
    pub name: String,
    pub value: Option<Expr>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Lit(Literal),
    Infer,
    Attributed {
        attrs: Vec<Attribute>,
        expr: Box<Expr>,
    },
    InterpString(Vec<InterpExprPart>),
    Ident(String),
    This,
    SelfType,
    Path {
        global: bool,
        segments: Vec<PathExprSegment>,
    },
    QualifiedMember {
        self_ty: Box<Type>,
        trait_path: Option<TypePath>,
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
    RawReadPtr(Box<Expr>),
    RawWritePtr(Box<Expr>),
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
    /// Explicitly parenthesized expression preserved when needed to disambiguate
    /// field calls like `(t.run)(args)` from method calls `t.run(args)`.
    Group(Box<Expr>),
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
        path: TypePath,
        fields: Vec<StructFieldInit>,
        rest: Option<Box<Expr>>,
    },
    StructUpdate {
        base: Box<Expr>,
        fields: Vec<StructFieldInit>,
    },
    Lambda {
        is_async: bool,
        is_move: bool,
        params: Vec<Param>,
        ret_ty: Option<Type>,
        body: LambdaBody,
    },
    Return(Option<Box<Expr>>),
    Break {
        label: Option<String>,
        value: Option<Box<Expr>>,
    },
    Continue {
        label: Option<String>,
    },
    IfExpr {
        cond: Box<Condition>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
    },
    While {
        label: Option<String>,
        cond: Box<Condition>,
        body: Block,
    },
    Foreach {
        label: Option<String>,
        pat: Pattern,
        iter: Box<Expr>,
        body: Block,
    },
    Match(MatchExpr),
    Loop {
        label: Option<String>,
        block: Block,
    },
    LabeledBlock {
        label: String,
        block: Block,
    },
    ConstBlock(ConstBody),
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
pub enum ConstBody {
    Expr(Box<Expr>),
    Block(Block),
}

#[derive(Debug, Clone, PartialEq)]
pub enum InterpExprPart {
    Literal(String),
    Expr {
        expr: Box<Expr>,
        format: Option<String>,
    },
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
