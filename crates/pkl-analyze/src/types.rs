//! Internal type representation used by the type inferrer.
//!
//! [`Ty`] is intentionally lightweight — it captures enough structure for
//! member-access resolution and hover output without committing to a full
//! Hindley-Milner style inference. Where the analyzer doesn't yet know the
//! exact type it returns [`Ty::Unknown`]; the LSP treats that as "no
//! information available" rather than as an error.

use std::fmt;

use pkl_syntax::cst;

/// A type known to the analyzer. Variants line up roughly with
/// [`pkl_stdlib`] entries plus structural forms.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    /// Type is not (yet) known. Member-access lookup falls back to giving up.
    Unknown,
    Any,
    /// `null`
    Null,
    /// `nothing` — the bottom type.
    Nothing,
    Boolean,
    Int,
    Float,
    Number,
    /// Pkl's `String`.
    Str,
    Char,
    Bytes,
    Duration,
    DataSize,
    Regex,
    Resource,
    Class,
    TypeAlias,
    Module,
    Dynamic,
    Pair(Box<Ty>, Box<Ty>),
    List(Box<Ty>),
    Set(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    Listing(Box<Ty>),
    Mapping(Box<Ty>, Box<Ty>),
    Nullable(Box<Ty>),
    Union(Vec<Ty>),
    Function {
        params: Vec<Ty>,
        ret: Box<Ty>,
    },
    /// Catch-all for types we don't model specially: user-defined classes,
    /// stdlib types we haven't broken out (e.g. `Pair<A,B>` with raw names),
    /// and stdlib type variables (`T`, `R`, ...).
    Named {
        name: String,
        args: Vec<Ty>,
    },
}

impl Ty {
    /// Bare name that the stdlib catalogue would key on, e.g. `"String"`,
    /// `"List"`, `"Duration"`. Returns `None` when there is no useful name
    /// (functions, unions, top, unknown).
    pub fn stdlib_name(&self) -> Option<&str> {
        Some(match self {
            Ty::Any => "Any",
            Ty::Null => "Null",
            Ty::Nothing => return None,
            Ty::Boolean => "Boolean",
            Ty::Int => "Int",
            Ty::Float => "Float",
            Ty::Number => "Number",
            Ty::Str => "String",
            Ty::Char => "Char",
            Ty::Bytes => "Bytes",
            Ty::Duration => "Duration",
            Ty::DataSize => "DataSize",
            Ty::Regex => "Regex",
            Ty::Resource => "Resource",
            Ty::Class => "Class",
            Ty::TypeAlias => "TypeAlias",
            Ty::Module => "Module",
            Ty::Dynamic => "Dynamic",
            Ty::Pair(_, _) => "Pair",
            Ty::List(_) => "List",
            Ty::Set(_) => "Set",
            Ty::Map(_, _) => "Map",
            Ty::Listing(_) => "Listing",
            Ty::Mapping(_, _) => "Mapping",
            Ty::Nullable(inner) => return inner.stdlib_name(),
            Ty::Named { name, .. } => name,
            Ty::Unknown | Ty::Union(_) | Ty::Function { .. } => return None,
        })
    }

    /// Look up a primitive `Ty` from its Pkl name. Returns `None` for
    /// non-primitive references, which callers can then wrap in
    /// [`Ty::Named`].
    pub fn from_name(name: &str) -> Option<Ty> {
        Some(match name {
            "Any" => Ty::Any,
            "Null" => Ty::Null,
            "nothing" | "Nothing" => Ty::Nothing,
            "Boolean" => Ty::Boolean,
            "Int" => Ty::Int,
            "Float" => Ty::Float,
            "Number" => Ty::Number,
            "String" => Ty::Str,
            "Char" => Ty::Char,
            "Bytes" => Ty::Bytes,
            "Duration" => Ty::Duration,
            "DataSize" => Ty::DataSize,
            "Regex" => Ty::Regex,
            "Resource" => Ty::Resource,
            "Class" => Ty::Class,
            "TypeAlias" => Ty::TypeAlias,
            "Module" | "module" => Ty::Module,
            "Dynamic" => Ty::Dynamic,
            _ => return None,
        })
    }

    /// Convert a CST [`cst::Type`] to an internal [`Ty`]. Anything we
    /// can't recognise becomes [`Ty::Named`] so the analyzer can still
    /// reason about its identity.
    pub fn from_cst_type(ty: &cst::Type) -> Ty {
        match ty {
            cst::Type::Named(n) => {
                let bare = n
                    .name()
                    .and_then(|qn| qn.segments().last().map(|t| cst::ident_text(&t)))
                    .unwrap_or_default();
                let args: Vec<Ty> = n
                    .type_arguments()
                    .map(|tal| tal.arguments().map(|t| Ty::from_cst_type(&t)).collect())
                    .unwrap_or_default();
                build_named(&bare, args)
            }
            cst::Type::Nullable(n) => {
                let inner = n
                    .inner()
                    .map(|t| Ty::from_cst_type(&t))
                    .unwrap_or(Ty::Unknown);
                Ty::Nullable(Box::new(inner))
            }
            cst::Type::Union(u) => Ty::Union(u.members().map(|t| Ty::from_cst_type(&t)).collect()),
            cst::Type::Function(f) => Ty::Function {
                params: f.parameters().map(|t| Ty::from_cst_type(&t)).collect(),
                ret: Box::new(
                    f.result()
                        .map(|t| Ty::from_cst_type(&t))
                        .unwrap_or(Ty::Unknown),
                ),
            },
            cst::Type::Parenthesized(p) => p
                .inner()
                .map(|t| Ty::from_cst_type(&t))
                .unwrap_or(Ty::Unknown),
            cst::Type::StringLiteral(_) => Ty::Str,
            cst::Type::Unknown(_) => Ty::Unknown,
            cst::Type::Nothing(_) => Ty::Nothing,
            cst::Type::Module(_) => Ty::Module,
            cst::Type::Error(_) => Ty::Unknown,
        }
    }

    /// Strip a leading `Nullable` so `String?.length` looks up on `String`.
    pub fn unwrap_nullable(&self) -> &Ty {
        match self {
            Ty::Nullable(inner) => inner.unwrap_nullable(),
            other => other,
        }
    }

    /// Positional type arguments visible at the top level of this `Ty`,
    /// in the order their stdlib declaration would list them. Returns an
    /// empty vector when the type has no arguments.
    pub fn type_args(&self) -> Vec<Ty> {
        match self {
            Ty::List(t) | Ty::Set(t) | Ty::Listing(t) => vec![(**t).clone()],
            Ty::Map(k, v) | Ty::Mapping(k, v) => vec![(**k).clone(), (**v).clone()],
            Ty::Pair(a, b) => vec![(**a).clone(), (**b).clone()],
            Ty::Nullable(inner) => inner.type_args(),
            Ty::Named { args, .. } => args.clone(),
            _ => Vec::new(),
        }
    }

    /// Substitute every type variable referenced in `env` with its
    /// corresponding `Ty`. Variables are represented as bare `Named` types
    /// with no arguments (e.g. `T`, `R`).
    pub fn substitute(&self, env: &std::collections::HashMap<String, Ty>) -> Ty {
        match self {
            Ty::Named { name, args } if args.is_empty() => {
                env.get(name).cloned().unwrap_or_else(|| self.clone())
            }
            Ty::Named { name, args } => Ty::Named {
                name: name.clone(),
                args: args.iter().map(|a| a.substitute(env)).collect(),
            },
            Ty::List(inner) => Ty::List(Box::new(inner.substitute(env))),
            Ty::Set(inner) => Ty::Set(Box::new(inner.substitute(env))),
            Ty::Listing(inner) => Ty::Listing(Box::new(inner.substitute(env))),
            Ty::Map(k, v) => Ty::Map(Box::new(k.substitute(env)), Box::new(v.substitute(env))),
            Ty::Mapping(k, v) => {
                Ty::Mapping(Box::new(k.substitute(env)), Box::new(v.substitute(env)))
            }
            Ty::Pair(a, b) => Ty::Pair(Box::new(a.substitute(env)), Box::new(b.substitute(env))),
            Ty::Nullable(inner) => Ty::Nullable(Box::new(inner.substitute(env))),
            Ty::Union(members) => Ty::Union(members.iter().map(|m| m.substitute(env)).collect()),
            Ty::Function { params, ret } => Ty::Function {
                params: params.iter().map(|p| p.substitute(env)).collect(),
                ret: Box::new(ret.substitute(env)),
            },
            _ => self.clone(),
        }
    }
}

/// Best-effort unification: walk `declared` and `actual` in parallel and,
/// whenever `declared` is a bare type variable from `type_vars`, bind it
/// to the corresponding piece of `actual`. Existing bindings are kept.
/// No error is reported on shape mismatches — the inferrer prefers
/// "I don't know" over false confidence.
pub fn unify(
    declared: &Ty,
    actual: &Ty,
    type_vars: &std::collections::HashSet<String>,
    env: &mut std::collections::HashMap<String, Ty>,
) {
    if let Ty::Named { name, args } = declared {
        if args.is_empty() && type_vars.contains(name) {
            env.entry(name.clone()).or_insert_with(|| actual.clone());
            return;
        }
    }
    match (declared, actual) {
        (Ty::List(a), Ty::List(b))
        | (Ty::Set(a), Ty::Set(b))
        | (Ty::Listing(a), Ty::Listing(b)) => unify(a, b, type_vars, env),
        (Ty::Map(k1, v1), Ty::Map(k2, v2)) | (Ty::Mapping(k1, v1), Ty::Mapping(k2, v2)) => {
            unify(k1, k2, type_vars, env);
            unify(v1, v2, type_vars, env);
        }
        (Ty::Pair(a1, b1), Ty::Pair(a2, b2)) => {
            unify(a1, a2, type_vars, env);
            unify(b1, b2, type_vars, env);
        }
        (Ty::Nullable(a), Ty::Nullable(b)) => unify(a, b, type_vars, env),
        // `T?` can also accept non-null `T`.
        (Ty::Nullable(a), other) => unify(a, other, type_vars, env),
        (
            Ty::Function {
                params: pa,
                ret: ra,
            },
            Ty::Function {
                params: pb,
                ret: rb,
            },
        ) => {
            for (a, b) in pa.iter().zip(pb.iter()) {
                unify(a, b, type_vars, env);
            }
            unify(ra, rb, type_vars, env);
        }
        (Ty::Named { name: na, args: aa }, Ty::Named { name: nb, args: ab }) if na == nb => {
            for (a, b) in aa.iter().zip(ab.iter()) {
                unify(a, b, type_vars, env);
            }
        }
        _ => {}
    }
}

/// Build a [`Ty`] from a bare type name (no qualifier) and parsed arguments.
fn build_named(name: &str, mut args: Vec<Ty>) -> Ty {
    if let Some(prim) = Ty::from_name(name) {
        // Only primitive zero-arity types are returned bare; if args are
        // present we keep them on a `Named` wrapper so the original shape
        // (e.g. `Boolean<X>` — nonsensical but well-formed input) is
        // preserved.
        if args.is_empty() {
            return prim;
        }
    }
    match (name, args.len()) {
        ("List", 1) => Ty::List(Box::new(args.pop().unwrap())),
        ("Set", 1) => Ty::Set(Box::new(args.pop().unwrap())),
        ("Map", 2) => {
            let v = args.pop().unwrap();
            let k = args.pop().unwrap();
            Ty::Map(Box::new(k), Box::new(v))
        }
        ("Pair", 2) => {
            let b = args.pop().unwrap();
            let a = args.pop().unwrap();
            Ty::Pair(Box::new(a), Box::new(b))
        }
        ("Listing", 1) => Ty::Listing(Box::new(args.pop().unwrap())),
        ("Mapping", 2) => {
            let v = args.pop().unwrap();
            let k = args.pop().unwrap();
            Ty::Mapping(Box::new(k), Box::new(v))
        }
        ("Collection", 1) => Ty::Named {
            name: "Collection".into(),
            args,
        },
        _ => Ty::Named {
            name: name.into(),
            args,
        },
    }
}

// ----------------------------------------------------------------------
// Signature parsing: extract the return type from a stdlib signature
// string. The catalogue we ship is hand-curated so the format is uniform:
//
//   property:  `name: ReturnType`
//   method:    `name(params): ReturnType`
//
// Methods always emit `): ReturnType`; we split on the last occurrence.

/// A stdlib (or stdlib-shaped) signature broken down for generic
/// instantiation. The original string remains available via the
/// `StdlibMember` so we don't need to re-render it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedSignature {
    /// Type-parameter names declared on this signature, e.g. `["R"]` for
    /// `map<R>(...)`.
    pub type_params: Vec<String>,
    /// Each parameter's declared type, in order. Empty for properties.
    pub param_types: Vec<Ty>,
    /// Return type of the signature, or the property's value type.
    pub return_ty: Ty,
}

/// Parse a stdlib signature, recognising both property (`name: Type`) and
/// method (`name<Generics>(params): Type`) forms. Returns a degenerate
/// `ParsedSignature` with `Ty::Unknown` when the input is malformed.
pub fn parse_signature(signature: &str) -> ParsedSignature {
    let s = signature.trim();
    // Walk the name.
    let mut p = SigParser::new(s);
    p.read_ident();
    p.skip_ws();
    let mut type_params: Vec<String> = Vec::new();
    if p.eat(b'<') {
        loop {
            p.skip_ws();
            let name = p.read_ident();
            if !name.is_empty() {
                type_params.push(name);
            }
            p.skip_ws();
            if p.eat(b',') {
                continue;
            }
            p.eat(b'>');
            break;
        }
    }
    p.skip_ws();
    if !p.eat(b'(') {
        // Property: `name: Type`.
        if !p.eat(b':') {
            return ParsedSignature {
                type_params,
                param_types: Vec::new(),
                return_ty: Ty::Unknown,
            };
        }
        let rest = &p.src[p.pos..];
        return ParsedSignature {
            type_params,
            param_types: Vec::new(),
            return_ty: parse_type_string(rest.trim()),
        };
    }
    // Method: parse `(params): Type`.
    let mut param_types = Vec::new();
    loop {
        p.skip_ws();
        if p.eat(b')') {
            break;
        }
        // Param name (optional). Read up to `:` or `,` or `)`.
        let param_start = p.pos;
        while p.pos < p.src.len() {
            let c = p.src.as_bytes()[p.pos];
            if c == b':' || c == b',' || c == b')' {
                break;
            }
            p.pos += 1;
        }
        let _name = p.src[param_start..p.pos].trim().to_string();
        if p.eat(b':') {
            p.skip_ws();
            // Parameter type may include `...` for variadic.
            let ty_str = p.read_until_top_level_comma_or_rparen();
            let ty_str = ty_str.trim_end_matches("...");
            param_types.push(parse_type_string(ty_str.trim()));
        }
        p.skip_ws();
        if p.eat(b',') {
            continue;
        }
        p.eat(b')');
        break;
    }
    p.skip_ws();
    if !p.eat(b':') {
        return ParsedSignature {
            type_params,
            param_types,
            return_ty: Ty::Unknown,
        };
    }
    let rest = &p.src[p.pos..];
    ParsedSignature {
        type_params,
        param_types,
        return_ty: parse_type_string(rest.trim()),
    }
}

struct SigParser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> SigParser<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.src.len() && self.src.as_bytes()[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.pos < self.src.len() && self.src.as_bytes()[self.pos] == c {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn read_ident(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.src.len() {
            let c = self.src.as_bytes()[self.pos];
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        self.src[start..self.pos].to_string()
    }

    /// Read characters until we hit a `,` or `)` at brace-depth zero.
    /// We only decrement `depth_lt` when it's positive so the `>` in `->`
    /// doesn't unbalance the angle-bracket counter.
    fn read_until_top_level_comma_or_rparen(&mut self) -> &'a str {
        let start = self.pos;
        let mut depth_lt: i32 = 0;
        let mut depth_paren: i32 = 0;
        while self.pos < self.src.len() {
            let c = self.src.as_bytes()[self.pos];
            match c {
                b'<' => depth_lt += 1,
                b'>' if depth_lt > 0 => depth_lt -= 1,
                b'(' => depth_paren += 1,
                b')' if depth_paren > 0 => depth_paren -= 1,
                b',' | b')' if depth_lt == 0 && depth_paren == 0 => break,
                _ => {}
            }
            self.pos += 1;
        }
        &self.src[start..self.pos]
    }
}

/// Parse the return-type portion of an [`StdlibMember`] signature into a
/// [`Ty`]. Returns [`Ty::Unknown`] if the signature looks malformed.
///
/// [`StdlibMember`]: pkl_stdlib::StdlibMember
pub fn return_type_of_signature(signature: &str) -> Ty {
    // Method signature: take what's after the last `): `.
    if let Some(idx) = signature.rfind("): ") {
        let rest = &signature[idx + 3..];
        return parse_type_string(rest.trim());
    }
    // Property signature: the first `:` outside any `<...>` separates
    // name and type.
    let bytes = signature.as_bytes();
    let mut depth = 0i32;
    for (i, b) in bytes.iter().enumerate() {
        match *b {
            b'<' => depth += 1,
            b'>' => depth -= 1,
            b':' if depth == 0 => {
                let rest = &signature[i + 1..];
                return parse_type_string(rest.trim());
            }
            _ => {}
        }
    }
    Ty::Unknown
}

/// Parse a type-expression string (no leading whitespace) into a [`Ty`].
///
/// Supports identifiers, nullable `T?`, single-argument and multi-argument
/// generic applications (`List<X>`, `Map<K, V>`), and function types
/// `(A, B) -> C`. Unknown shapes fall back to `Ty::Unknown`.
pub fn parse_type_string(input: &str) -> Ty {
    let mut p = TyParser::new(input);
    let t = p.parse_type();
    if !p.is_done() {
        // Trailing garbage — accept what we have anyway.
        return t;
    }
    t
}

struct TyParser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> TyParser<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    fn is_done(&self) -> bool {
        self.pos >= self.src.len()
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn skip_ws(&mut self) {
        while self.pos < self.src.len() && self.src.as_bytes()[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.skip_ws();
        if self.pos < self.src.len() && self.src.as_bytes()[self.pos] == c {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse_type(&mut self) -> Ty {
        // A union, eventually.
        let first = self.parse_nullable();
        let mut union = vec![first];
        while self.eat(b'|') {
            union.push(self.parse_nullable());
        }
        if union.len() == 1 {
            union.into_iter().next().unwrap()
        } else {
            Ty::Union(union)
        }
    }

    fn parse_nullable(&mut self) -> Ty {
        let mut t = self.parse_primary();
        while self.eat(b'?') {
            t = Ty::Nullable(Box::new(t));
        }
        t
    }

    fn parse_primary(&mut self) -> Ty {
        self.skip_ws();
        if self.eat(b'(') {
            // Either parenthesised type or function type `(A, B) -> R`.
            let mut params = Vec::new();
            if !self.eat(b')') {
                params.push(self.parse_type());
                while self.eat(b',') {
                    params.push(self.parse_type());
                }
                if !self.eat(b')') {
                    return Ty::Unknown;
                }
            }
            self.skip_ws();
            if self.rest().starts_with("->") {
                self.pos += 2;
                let ret = self.parse_type();
                return Ty::Function {
                    params,
                    ret: Box::new(ret),
                };
            }
            if params.len() == 1 {
                return params.into_iter().next().unwrap();
            }
            return Ty::Unknown;
        }
        // Bare identifier, optionally followed by `<...>`.
        let start = self.pos;
        while self.pos < self.src.len() {
            let c = self.src.as_bytes()[self.pos];
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if start == self.pos {
            return Ty::Unknown;
        }
        let name = &self.src[start..self.pos];
        let mut args = Vec::new();
        if self.eat(b'<') && !self.eat(b'>') {
            args.push(self.parse_type());
            while self.eat(b',') {
                args.push(self.parse_type());
            }
            if !self.eat(b'>') {
                return Ty::Unknown;
            }
        }
        build_named(name, args)
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Unknown => f.write_str("?"),
            Ty::Any => f.write_str("Any"),
            Ty::Null => f.write_str("Null"),
            Ty::Nothing => f.write_str("nothing"),
            Ty::Boolean => f.write_str("Boolean"),
            Ty::Int => f.write_str("Int"),
            Ty::Float => f.write_str("Float"),
            Ty::Number => f.write_str("Number"),
            Ty::Str => f.write_str("String"),
            Ty::Char => f.write_str("Char"),
            Ty::Bytes => f.write_str("Bytes"),
            Ty::Duration => f.write_str("Duration"),
            Ty::DataSize => f.write_str("DataSize"),
            Ty::Regex => f.write_str("Regex"),
            Ty::Resource => f.write_str("Resource"),
            Ty::Class => f.write_str("Class"),
            Ty::TypeAlias => f.write_str("TypeAlias"),
            Ty::Module => f.write_str("Module"),
            Ty::Dynamic => f.write_str("Dynamic"),
            Ty::Pair(a, b) => write!(f, "Pair<{}, {}>", a, b),
            Ty::List(t) => write!(f, "List<{}>", t),
            Ty::Set(t) => write!(f, "Set<{}>", t),
            Ty::Map(k, v) => write!(f, "Map<{}, {}>", k, v),
            Ty::Listing(t) => write!(f, "Listing<{}>", t),
            Ty::Mapping(k, v) => write!(f, "Mapping<{}, {}>", k, v),
            Ty::Nullable(inner) => write!(f, "{}?", inner),
            Ty::Union(members) => {
                for (i, m) in members.iter().enumerate() {
                    if i > 0 {
                        f.write_str(" | ")?;
                    }
                    write!(f, "{}", m)?;
                }
                Ok(())
            }
            Ty::Function { params, ret } => {
                f.write_str("(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", p)?;
                }
                write!(f, ") -> {}", ret)
            }
            Ty::Named { name, args } => {
                f.write_str(name)?;
                if !args.is_empty() {
                    f.write_str("<")?;
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        write!(f, "{}", a)?;
                    }
                    f.write_str(">")?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_primitive_return_type() {
        assert_eq!(return_type_of_signature("length: Int"), Ty::Int);
        assert_eq!(
            return_type_of_signature("contains(other: String): Boolean"),
            Ty::Boolean
        );
    }

    #[test]
    fn parses_generic_return_type() {
        let ty = return_type_of_signature("map<R>(transform: (T) -> R): List<R>");
        // `R` is a type variable we can't resolve — becomes a Named.
        match ty {
            Ty::List(inner) => assert!(matches!(*inner, Ty::Named { ref name, .. } if name == "R")),
            other => panic!("expected List<R>, got {:?}", other),
        }
    }

    #[test]
    fn parses_nullable_return_type() {
        assert_eq!(
            return_type_of_signature("firstOrNull: T?"),
            Ty::Nullable(Box::new(Ty::Named {
                name: "T".into(),
                args: vec![],
            }))
        );
    }

    #[test]
    fn parses_function_argument() {
        let t = parse_type_string("(Int) -> Boolean");
        assert_eq!(
            t,
            Ty::Function {
                params: vec![Ty::Int],
                ret: Box::new(Ty::Boolean),
            }
        );
    }

    #[test]
    fn parses_map_return() {
        let t = return_type_of_signature("groupBy<K>(key: (T) -> K): Map<K, List<T>>");
        assert!(matches!(t, Ty::Map(_, _)));
    }

    #[test]
    fn parses_method_signature_with_generics_and_params() {
        let p = parse_signature("map<R>(transform: (T) -> R): List<R>");
        assert_eq!(p.type_params, vec!["R".to_string()]);
        assert_eq!(p.param_types.len(), 1);
        assert!(matches!(p.param_types[0], Ty::Function { .. }));
        match p.return_ty {
            Ty::List(inner) => {
                assert!(matches!(*inner, Ty::Named { ref name, .. } if name == "R"));
            }
            other => panic!("expected List<R>, got {:?}", other),
        }
    }

    #[test]
    fn parses_property_signature() {
        let p = parse_signature("length: Int");
        assert!(p.type_params.is_empty());
        assert!(p.param_types.is_empty());
        assert_eq!(p.return_ty, Ty::Int);
    }

    #[test]
    fn substitute_walks_generics() {
        use std::collections::HashMap;
        let ty = parse_type_string("List<T>");
        let mut env = HashMap::new();
        env.insert("T".to_string(), Ty::Int);
        match ty.substitute(&env) {
            Ty::List(inner) => assert_eq!(*inner, Ty::Int),
            other => panic!("expected List<Int>, got {:?}", other),
        }
    }

    #[test]
    fn unify_extracts_bindings() {
        use std::collections::{HashMap, HashSet};
        let mut type_vars = HashSet::new();
        type_vars.insert("T".to_string());
        type_vars.insert("R".to_string());
        let declared = parse_type_string("(T) -> R");
        let actual = Ty::Function {
            params: vec![Ty::Int],
            ret: Box::new(Ty::Str),
        };
        let mut env = HashMap::new();
        unify(&declared, &actual, &type_vars, &mut env);
        assert_eq!(env.get("T"), Some(&Ty::Int));
        assert_eq!(env.get("R"), Some(&Ty::Str));
    }

    #[test]
    fn type_args_pulls_apart_generics() {
        let t = parse_type_string("Map<String, Int>");
        let args = t.type_args();
        assert_eq!(args, vec![Ty::Str, Ty::Int]);
    }
}
