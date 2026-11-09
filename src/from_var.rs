//! `FromVar` — the typed hot-path seam: `Var` in, plain structs out.
//!
//! Prototyping code passes `Var` around (configs, entity definitions, route
//! bodies); the hot path converts once with `#[derive(FromVar)]` and works
//! with real structs. Reads are **strict** — no silent zero/empty coercion
//! like the `Into<Var>` reads; a type mismatch is an error with the field
//! path attached (`"user.stats.score": expected int, found string`).
//!
//! ```
//! use hyg::{a, o, FromVar, Var};
//!
//! #[derive(FromVar, Debug)]
//! struct User {
//!     name: String,
//!     score: i64,
//!     #[from_var(default)]
//!     tags: Vec<String>,
//! }
//!
//! let user = User::from_var(&Var::from(o!{ "name": "bob", "score": 9i64, "tags": a!["admin"] })).unwrap();
//! assert_eq!(user.name, "bob");
//! assert_eq!(user.tags, vec!["admin".to_string()]);
//! ```

use crate::var::{Quat, Vec3, Vec4, Var};

/// Why a `FromVar` conversion failed, with the field path attached
/// (`"user.stats.score": expected int, found string`).
#[derive(Debug, Clone)]
pub struct FromVarError {
    /// Dotted field path from the conversion root ("" at the root itself).
    pub path: String,
    pub message: String,
}

impl FromVarError {
    pub fn new(message: impl Into<String>) -> Self {
        FromVarError { path: String::new(), message: message.into() }
    }

    /// Extend the path with an outer field name (`score` → `stats.score`).
    #[must_use]
    pub fn at(mut self, segment: impl Into<String>) -> Self {
        let segment = segment.into();
        self.path = if self.path.is_empty() {
            segment
        } else {
            format!("{segment}.{}", self.path)
        };
        self
    }

    fn expected(expected: &str, found: &Var) -> Self {
        Self::new(format!("expected {expected}, found {}", found.ty()))
    }
}

impl std::fmt::Display for FromVarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{:?}: {}", self.path, self.message)
        }
    }
}

impl std::error::Error for FromVarError {}

/// A typed read from a `Var`: strict, path-tracked, no coercion. Derive it
/// for structs (`#[derive(FromVar)]`) or implement it for your own types.
pub trait FromVar: Sized {
    fn from_var(v: &Var) -> Result<Self, FromVarError>;
}

// ---- scalar impls (strict — the typed reads the hot path can trust) ----

impl FromVar for Var {
    fn from_var(v: &Var) -> Result<Self, FromVarError> { Ok(v.clone()) }
}

impl FromVar for String {
    fn from_var(v: &Var) -> Result<Self, FromVarError> {
        v.as_str()
            .map(str::to_string)
            .ok_or_else(|| FromVarError::expected("string", v))
    }
}

impl FromVar for bool {
    fn from_var(v: &Var) -> Result<Self, FromVarError> {
        match v {
            Var::Bool(b) => Ok(*b),
            other => Err(FromVarError::expected("bool", other)),
        }
    }
}

impl FromVar for i64 {
    fn from_var(v: &Var) -> Result<Self, FromVarError> {
        match v {
            Var::I64(n) => Ok(*n),
            other => Err(FromVarError::expected("int", other)),
        }
    }
}

impl FromVar for i32 {
    fn from_var(v: &Var) -> Result<Self, FromVarError> {
        match v {
            Var::I64(n) => i32::try_from(*n)
                .map_err(|_| FromVarError::new(format!("int {n} does not fit in i32"))),
            other => Err(FromVarError::expected("int", other)),
        }
    }
}

impl FromVar for f64 {
    fn from_var(v: &Var) -> Result<Self, FromVarError> {
        match v {
            Var::F64(n) => Ok(*n),
            other => Err(FromVarError::expected("float", other)),
        }
    }
}

impl FromVar for f32 {
    fn from_var(v: &Var) -> Result<Self, FromVarError> {
        match v {
            Var::F64(n) => Ok(*n as f32),
            other => Err(FromVarError::expected("float", other)),
        }
    }
}

// ---- math types ----

macro_rules! math_from_var {
    ($($ty:ident => $variant:ident),*) => {
        $(
            impl FromVar for $ty {
                fn from_var(v: &Var) -> Result<Self, FromVarError> {
                    match v {
                        Var::$variant(m) => Ok(*m),
                        other => Err(FromVarError::expected(stringify!($ty), other)),
                    }
                }
            }
        )*
    };
}

math_from_var!(Vec3 => Vec3, Vec4 => Vec4, Quat => Quat);

// ---- containers ----

impl<T: FromVar> FromVar for Option<T> {
    fn from_var(v: &Var) -> Result<Self, FromVarError> {
        if v.ty() == crate::Ty::Null {
            Ok(None)
        } else {
            T::from_var(v).map(Some)
        }
    }
}

impl<T: FromVar> FromVar for Vec<T> {
    fn from_var(v: &Var) -> Result<Self, FromVarError> {
        match v {
            Var::List(l) => l
                .iter()
                .into_iter()
                .enumerate()
                .map(|(i, item)| T::from_var(&item).map_err(|e| e.at(i.to_string())))
                .collect(),
            other => Err(FromVarError::expected("list", other)),
        }
    }
}
