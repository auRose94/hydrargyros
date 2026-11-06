//! `Var` — the universal dynamic value, a Rust reimagining of gold's `var`.
//!
//! Copy-on-write: `List`/`Object` payloads are shared through `Arc<Mutex<…>>`,
//! so passing a `Var` by value is a cheap pointer bump, exactly like gold's
//! `shared_ptr`-backed `var`. Because the payload is interior-mutable, a write
//! through one handle is visible through every other handle that shares it —
//! which is what lets the `VarRef` proxy mutate the original in place,
//! faithfully reproducing gold's `varRef`. `Arc`/`Mutex` (rather than
//! `Rc`/`RefCell`) makes the whole value graph `Send + Sync`: the same `Var`
//! crosses threads, async tasks, and route handlers.

use std::collections::HashMap;
use std::fmt;
use std::ops::{Add, Div, Mul, Neg, Sub};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::list::List;
use crate::object::Object;

/// A value's runtime type tag. Kept for introspection (`as_*` / `is_*`), the
/// way gold has `types`. Most code won't match on this — it's there when you
/// need it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    Null,
    Bool,
    I64,
    F64,
    Str,
    List,
    Object,
    Func,
    Vec3,
    Vec4,
    Quat,
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Ty::Null => "null",
            Ty::Bool => "bool",
            Ty::I64 => "int",
            Ty::F64 => "float",
            Ty::Str => "string",
            Ty::List => "list",
            Ty::Object => "object",
            Ty::Func => "function",
            Ty::Vec3 => "vec3",
            Ty::Vec4 => "vec4",
            Ty::Quat => "quat",
        };
        f.write_str(s)
    }
}

/// A 3-component float vector (mirrors gold's vec3f).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}
impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Vec3 { x, y, z }
    }

    /// A vector with the same value in all components.
    #[must_use]
    pub fn splat(v: f32) -> Self {
        Vec3 { x: v, y: v, z: v }
    }

    /// Component-wise scale (transform composition).
    #[must_use]
    pub fn scaled(self, by: Vec3) -> Self {
        Vec3 {
            x: self.x * by.x,
            y: self.y * by.y,
            z: self.z * by.z,
        }
    }

    /// Dot product.
    #[must_use]
    pub fn dot(self, other: Vec3) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    /// Cross product.
    #[must_use]
    pub fn cross(self, other: Vec3) -> Self {
        Vec3 {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }
}

impl std::ops::Add for Vec3 {
    type Output = Vec3;
    fn add(self, other: Vec3) -> Vec3 {
        Vec3 {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, other: Vec3) -> Vec3 {
        Vec3 {
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
        }
    }
}

impl std::ops::SubAssign for Vec3 {
    fn sub_assign(&mut self, other: Vec3) {
        *self = *self - other;
    }
}

impl Vec3 {
    /// Euclidean length.
    #[must_use]
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    /// This vector scaled to length 1 — the zero vector stays the zero
    /// vector (no NaNs from a degenerate direction; camera look-at relies
    /// on that guard).
    #[must_use]
    pub fn normalize(self) -> Vec3 {
        let len = self.length();
        if len == 0.0 {
            return Vec3::splat(0.0);
        }
        Vec3 {
            x: self.x / len,
            y: self.y / len,
            z: self.z / len,
        }
    }
}

/// A 4-component float vector.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}
impl Vec4 {
    pub fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Vec4 { x, y, z, w }
    }
}

/// A quaternion.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}
impl Quat {
    pub fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Quat { x, y, z, w }
    }

    /// Unit quaternion from an axis and an angle in radians.
    #[must_use]
    pub fn axis_angle(axis: Vec3, radians: f32) -> Self {
        let len = (axis.x * axis.x + axis.y * axis.y + axis.z * axis.z).sqrt();
        if len == 0.0 {
            return Quat {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                w: 1.0,
            };
        }
        let (ax, ay, az) = (axis.x / len, axis.y / len, axis.z / len);
        let half = radians / 2.0;
        let (s, c) = (half.sin(), half.cos());
        Quat {
            x: ax * s,
            y: ay * s,
            z: az * s,
            w: c,
        }
    }

    /// The Hamilton product — composing rotations applies `self` first,
    /// then `other` (`(a * b).rotate(v) == a.rotate(b.rotate(v))`).
    #[must_use]
    pub fn compose(self, other: Quat) -> Quat {
        Quat {
            w: self.w * other.w - self.x * other.x - self.y * other.y - self.z * other.z,
            x: self.w * other.x + self.x * other.w + self.y * other.z - self.z * other.y,
            y: self.w * other.y - self.x * other.z + self.y * other.w + self.z * other.x,
            z: self.w * other.z + self.x * other.y - self.y * other.x + self.z * other.w,
        }
    }

    /// Rotate a vector by this quaternion (`v + 2q × (q × v + wv)`).
    #[must_use]
    pub fn rotate(self, v: Vec3) -> Vec3 {
        let q = Vec3 {
            x: self.x,
            y: self.y,
            z: self.z,
        };
        let t = q.cross(v).scaled(Vec3::splat(2.0));
        v + q.cross(t) + t.scaled(Vec3::splat(self.w))
    }
}

/// Global counter handing out unique function ids. `Relaxed` is enough — ids
/// only need to be distinct, never ordered.
static FUNC_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Process-global id → Func registry backing serde round-tripping: a `Func`
/// serializes as the one-key map `{"__func__": id}`, and deserialization
/// restores the handle by looking the id up here. Functions register in
/// [`Func::new`] — the single creation choke point, so every `func!` expansion
/// is covered automatically. Entries are never removed (no GC): every function
/// simply lives for the life of the process, which is the acceptable shape for
/// a prototyping framework — gold's lang registry worked the same way.
static FUNC_REGISTRY: OnceLock<Mutex<HashMap<u64, Func>>> = OnceLock::new();

fn func_registry() -> &'static Mutex<HashMap<u64, Func>> {
    FUNC_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

// The registry is shared state like everything else in the value graph —
// extend lib.rs's Send + Sync proof to it.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<OnceLock<Mutex<HashMap<u64, Func>>>>();
};

/// Restore a `Func` handle from its serialized id (the value of
/// `{"__func__": id}`). A live id returns a clone of the registered function;
/// anything else — unknown id, non-integer — degrades to `Null` rather than
/// erroring: the id is a *within-process* handle (closures can't cross
/// processes), so an unregistered id can only come from another process or
/// from data written before the registry existed, and `Null` keeps that data
/// lenient the way the old all-functions-die behavior did.
fn func_from_handle(id: &Var) -> Var {
    let Some(id) = id.as_i64() else {
        return Var::Null;
    };
    let registry = crate::lock(func_registry());
    registry
        .get(&(id as u64))
        .map(|f| Var::Func(f.clone()))
        .unwrap_or(Var::Null)
}

/// A function body: takes the argument list, produces a `Var`.
type FuncBody = Arc<dyn Fn(&[Var]) -> Var + Send + Sync>;

/// An id-bearing function value — Mercury's `func`, the gold `func` equivalent.
///
/// A `Func` wraps a closure *and* carries a stable id. That id is the workaround
/// for the fact that closures have no inspectable identity: two functions can be
/// told apart (compared, shown in debug output) even though their bodies can't
/// be compared. The body is shared through an `Arc`, so cloning a `Func` is a
/// cheap pointer bump.
///
/// The id is also the serde handle: a `Func` serializes as the one-key map
/// `{"__func__": id}`, and deserialization restores the handle from the
/// process-global `FUNC_REGISTRY` (an unknown id restores as `Null`).
pub struct Func {
    id: u64,
    body: FuncBody,
}

impl Func {
    /// Wrap a closure, assigning it a fresh, process-unique id. The closure
    /// must be `Send + Sync` — functions cross threads like every other `Var`.
    ///
    /// This is the single creation choke point, so registration in
    /// `FUNC_REGISTRY` happens here: every `func!` expansion lands here too.
    /// Clones share the id (and the `Arc`'d body), so the registered clone
    /// equals the original.
    pub fn new(f: impl Fn(&[Var]) -> Var + Send + Sync + 'static) -> Self {
        let func = Func {
            id: FUNC_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
            body: Arc::new(f),
        };
        crate::lock(func_registry()).insert(func.id, func.clone());
        func
    }

    /// The stable id of this function.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Call the function with `args`.
    pub fn call(&self, args: &[Var]) -> Var {
        (self.body)(args)
    }
}

impl Clone for Func {
    fn clone(&self) -> Self {
        Func {
            id: self.id,
            body: Arc::clone(&self.body),
        }
    }
}

impl PartialEq for Func {
    fn eq(&self, other: &Self) -> bool {
        // Functions compare by id — the only identity we can inspect.
        self.id == other.id
    }
}

impl std::fmt::Debug for Func {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Func").field("id", &self.id).finish()
    }
}

/// The universal dynamic value.
///
/// ```
/// use hydrargyros::Var;
/// let v = Var::from(42i64);
/// assert_eq!(v.as_i64(), Some(42));
/// assert_eq!(v.ty(), hydrargyros::Ty::I64);
/// ```
#[derive(Clone)]
pub enum Var {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    Str(String),
    List(List),
    Object(Object),
    Func(Func),
    Vec3(Vec3),
    Vec4(Vec4),
    Quat(Quat),
}

impl std::fmt::Debug for Var {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Var::Null => f.write_str("Null"),
            Var::Bool(b) => f.debug_tuple("Bool").field(b).finish(),
            Var::I64(n) => f.debug_tuple("I64").field(n).finish(),
            Var::F64(n) => f.debug_tuple("F64").field(n).finish(),
            Var::Str(s) => f.debug_tuple("Str").field(s).finish(),
            Var::List(l) => f.debug_tuple("List").field(l).finish(),
            Var::Object(o) => f.debug_tuple("Object").field(o).finish(),
            Var::Func(func) => std::fmt::Debug::fmt(func, f),
            Var::Vec3(v) => f.debug_tuple("Vec3").field(v).finish(),
            Var::Vec4(v) => f.debug_tuple("Vec4").field(v).finish(),
            Var::Quat(v) => f.debug_tuple("Quat").field(v).finish(),
        }
    }
}

impl PartialEq for Var {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Var::Null, Var::Null) => true,
            (Var::Bool(a), Var::Bool(b)) => a == b,
            (Var::I64(a), Var::I64(b)) => a == b,
            (Var::F64(a), Var::F64(b)) => a == b,
            (Var::Str(a), Var::Str(b)) => a == b,
            (Var::List(a), Var::List(b)) => a == b,
            (Var::Object(a), Var::Object(b)) => a == b,
            (Var::Vec3(a), Var::Vec3(b)) => a == b,
            (Var::Vec4(a), Var::Vec4(b)) => a == b,
            (Var::Quat(a), Var::Quat(b)) => a == b,
            // functions compare by id — clones share it, fresh Funcs never collide
            (Var::Func(a), Var::Func(b)) => a == b,
            _ => false,
        }
    }
}

/// The JS-style `null` value. Write it as a bare literal instead of
/// `Var::Null` / `hydrargyros::Ty::Null`:
///
/// ```
/// use hydrargyros::Null;
/// let v = hydrargyros::Var::from(Null);
/// assert_eq!(v.ty(), hydrargyros::Ty::Null);
/// ```
#[allow(non_upper_case_globals)]
pub const Null: Var = Var::Null;

impl Var {
    /// The runtime type tag.
    pub fn ty(&self) -> Ty {
        match self {
            Var::Null => Ty::Null,
            Var::Bool(_) => Ty::Bool,
            Var::I64(_) => Ty::I64,
            Var::F64(_) => Ty::F64,
            Var::Str(_) => Ty::Str,
            Var::List(_) => Ty::List,
            Var::Object(_) => Ty::Object,
            Var::Func(_) => Ty::Func,
            Var::Vec3(_) => Ty::Vec3,
            Var::Vec4(_) => Ty::Vec4,
            Var::Quat(_) => Ty::Quat,
        }
    }

    /// Truthiness, JS-style: null/0/""/false are falsy.
    pub fn is_truthy(&self) -> bool {
        match self {
            Var::Null => false,
            Var::Bool(b) => *b,
            Var::I64(n) => *n != 0,
            Var::F64(n) => *n != 0.0,
            Var::Str(s) => !s.is_empty(),
            _ => true, // objects/lists/functions are always truthy
        }
    }

    // ---- numeric reads (None on type mismatch) ----

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Var::I64(n) => Some(*n),
            Var::F64(n) => Some(*n as i64),
            Var::Bool(b) => Some(*b as i64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Var::F64(n) => Some(*n),
            Var::I64(n) => Some(*n as f64),
            Var::Bool(b) => Some(*b as i64 as f64),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Var::Bool(b) => Some(*b),
            _ => Some(self.is_truthy()),
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Var::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<String> {
        match self {
            Var::Str(s) => Some(s.clone()),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&List> {
        match self {
            Var::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Var::Object(o) => Some(o),
            _ => None,
        }
    }

    // ---- property access (gold's v["key"] / v[i]) ----
    //
    // Rust's `Index` can't return by value or reach behind interior mutability,
    // so reads use `.get()` (null-safe, prototype-walking for objects). Writes
    // use `.set()` or the `.at()` proxy — see `VarRef`.

    /// Read `key` (objects, walking the prototype chain) or empty for misses
    /// and non-objects. The JS `obj[k]` → `undefined` semantics, minus the
    /// silent-any footgun: pair with `.as_*()` or `Into<…>` for typed reads.
    pub fn get(&self, key: &str) -> Var {
        match self {
            Var::Object(o) => o.get(key),
            _ => Var::Null,
        }
    }

    /// Read index `i` of a list; empty for non-lists / out of range.
    pub fn get_idx(&self, i: usize) -> Var {
        match self {
            Var::List(l) => l.get(i),
            _ => Var::Null,
        }
    }

    /// Read a dotted path: `v.get_path("stats.score")`.
    pub fn get_path(&self, path: &str) -> Var {
        let mut cur = self.clone();
        for part in path.split('.') {
            cur = cur.get(part);
        }
        cur
    }

    /// Write `key` on an object (no-op-ish: panics on non-objects, which is the
    /// safe default vs gold's silent no-op).
    pub fn set(&self, key: &str, value: impl Into<Var>) {
        match self {
            Var::Object(o) => o.set(key, value.into()),
            _ => panic!("cannot set key {key:?} on non-object {}", self.ty()),
        }
    }

    /// Write index `i` of a list (grows the list if needed, JS-style).
    pub fn set_idx(&self, i: usize, value: impl Into<Var>) {
        match self {
            Var::List(l) => l.set(i, value.into()),
            _ => panic!("cannot set index on non-list {}", self.ty()),
        }
    }

    /// Write a dotted path: `v.set_path("stats.score", 10)`. Creates missing
    /// intermediate objects along the way (JS `obj.a.b = 1` semantics).
    pub fn set_path(&self, path: &str, value: impl Into<Var>) {
        if let Var::Object(_) = self {
            set_path_impl(self, path, value.into());
        } else {
            panic!("cannot set_path on non-object {}", self.ty());
        }
    }

    /// Return a `VarRef` proxy for `key` — the gold-like handle for chained
    /// read/write: `v.at("a").at("b").put(5)`.
    pub fn at(&self, key: &str) -> crate::proxy::VarRef {
        crate::proxy::VarRef::object(self.clone(), key)
    }

    /// `VarRef` proxy for a list index.
    pub fn at_idx(&self, i: usize) -> crate::proxy::VarRef {
        crate::proxy::VarRef::index(self.clone(), i)
    }

    // ---- call (gold's var operator()) ----
    pub fn call(&self, args: &[Var]) -> Var {
        match self {
            Var::Func(f) => f.call(args),
            _ => panic!("cannot call non-function {}", self.ty()),
        }
    }
}

// ---- stringification (gold's `var.getString()` for every type) ----

impl fmt::Display for Var {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Var::Null => f.write_str("null"),
            Var::Bool(b) => write!(f, "{b}"),
            Var::I64(n) => write!(f, "{n}"),
            Var::F64(n) => write!(f, "{n}"),
            Var::Str(s) => f.write_str(s),
            Var::Vec3(v) => write!(f, "vec3({}, {}, {})", v.x, v.y, v.z),
            Var::Vec4(v) => write!(f, "vec4({}, {}, {}, {})", v.x, v.y, v.z, v.w),
            Var::Quat(v) => write!(f, "quat({}, {}, {}, {})", v.x, v.y, v.z, v.w),
            Var::List(_) => f.write_str("[...]"),
            Var::Object(_) => f.write_str("{...}"),
            Var::Func(func) => write!(f, "<function {}>", func.id()),
        }
    }
}

// ---- arithmetic (gold overloads +,-,*,/ on var) ----
//
// Numeric ops widen: int + float -> float. Mismatched/unsupported types panic
// with a clear message; a production build might return an error Var instead.

impl Add for Var {
    type Output = Var;
    fn add(self, rhs: Var) -> Var {
        if let (Some(a), Some(b)) = (self.as_f64(), rhs.as_f64()) {
            return Var::F64(a + b);
        }
        if let (Some(a), Some(b)) = (self.as_i64(), rhs.as_i64()) {
            return Var::I64(a + b);
        }
        panic!("cannot add {self} + {rhs}");
    }
}

impl Sub for Var {
    type Output = Var;
    fn sub(self, rhs: Var) -> Var {
        if let (Some(a), Some(b)) = (self.as_f64(), rhs.as_f64()) {
            return Var::F64(a - b);
        }
        if let (Some(a), Some(b)) = (self.as_i64(), rhs.as_i64()) {
            return Var::I64(a - b);
        }
        panic!("cannot subtract {self} - {rhs}");
    }
}

impl Mul for Var {
    type Output = Var;
    fn mul(self, rhs: Var) -> Var {
        if let (Some(a), Some(b)) = (self.as_f64(), rhs.as_f64()) {
            return Var::F64(a * b);
        }
        if let (Some(a), Some(b)) = (self.as_i64(), rhs.as_i64()) {
            return Var::I64(a * b);
        }
        panic!("cannot multiply {self} * {rhs}");
    }
}

impl Div for Var {
    type Output = Var;
    fn div(self, rhs: Var) -> Var {
        if let (Some(a), Some(b)) = (self.as_f64(), rhs.as_f64()) {
            return Var::F64(a / b);
        }
        if let (Some(a), Some(b)) = (self.as_i64(), rhs.as_i64()) {
            return Var::I64(a / b);
        }
        panic!("cannot divide {self} / {rhs}");
    }
}

impl Neg for Var {
    type Output = Var;
    fn neg(self) -> Var {
        match self {
            Var::F64(n) => Var::F64(-n),
            Var::I64(n) => Var::I64(-n),
            other => panic!("cannot negate {other}"),
        }
    }
}

// ---- ergonomic conversions (gold's implicit constructor set) ----

impl From<()> for Var {
    fn from(_: ()) -> Self {
        Var::Null
    }
}
impl From<bool> for Var {
    fn from(b: bool) -> Self {
        Var::Bool(b)
    }
}
impl From<i64> for Var {
    fn from(n: i64) -> Self {
        Var::I64(n)
    }
}
impl From<i32> for Var {
    fn from(n: i32) -> Self {
        Var::I64(n as i64)
    }
}
impl From<u64> for Var {
    fn from(n: u64) -> Self {
        Var::I64(n as i64)
    }
}
impl From<usize> for Var {
    fn from(n: usize) -> Self {
        Var::I64(n as i64)
    }
}
impl From<f64> for Var {
    fn from(n: f64) -> Self {
        Var::F64(n)
    }
}
impl From<f32> for Var {
    fn from(n: f32) -> Self {
        Var::F64(n as f64)
    }
}
impl From<&str> for Var {
    fn from(s: &str) -> Self {
        Var::Str(s.to_string())
    }
}
impl From<String> for Var {
    fn from(s: String) -> Self {
        Var::Str(s)
    }
}
impl From<Vec3> for Var {
    fn from(v: Vec3) -> Self {
        Var::Vec3(v)
    }
}
impl From<Vec4> for Var {
    fn from(v: Vec4) -> Self {
        Var::Vec4(v)
    }
}
impl From<Quat> for Var {
    fn from(v: Quat) -> Self {
        Var::Quat(v)
    }
}

impl From<List> for Var {
    fn from(l: List) -> Self {
        Var::List(l)
    }
}
impl From<Object> for Var {
    fn from(o: Object) -> Self {
        Var::Object(o)
    }
}
impl From<Func> for Var {
    fn from(f: Func) -> Self {
        Var::Func(f)
    }
}

// ---- serde: one trait covers every serde format (JSON/BSON/CBOR/MsgPack/…) ----
//
// `Var` is the universal dynamic value, so it serializes to/from a serde data
// model. A closure still has no data-model form, but `Func` carries a stable
// id: it serializes as the one-key map `{"__func__": id}`, and deserialization
// restores the handle from `FUNC_REGISTRY` (see `func_from_handle`). Unknown
// ids degrade to `Null` — a within-process handle can't mean anything on the
// far side of a process boundary, and `Null` keeps data written before the
// registry existed lenient. A bare `null` still deserializes to `Null`.

impl serde::Serialize for Var {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        match self {
            Var::Null => serializer.serialize_none(),
            Var::Bool(b) => serializer.serialize_bool(*b),
            Var::I64(n) => serializer.serialize_i64(*n),
            Var::F64(n) => serializer.serialize_f64(*n),
            Var::Str(s) => serializer.serialize_str(s),
            Var::List(l) => {
                use serde::ser::SerializeSeq;
                let mut seq = serializer.serialize_seq(Some(l.len()))?;
                for item in l.iter() {
                    seq.serialize_element(&item)?;
                }
                seq.end()
            }
            Var::Object(o) => {
                let mut map = serializer.serialize_map(None)?;
                for (k, v) in o.entries() {
                    map.serialize_entry(k.as_str(), &v)?;
                }
                map.end()
            }
            Var::Vec3(v) => serializer.serialize_some(v),
            Var::Vec4(v) => serializer.serialize_some(v),
            Var::Quat(v) => serializer.serialize_some(v),
            // the id-bearing handle: deserialization restores the function
            // from FUNC_REGISTRY (unknown ids come back Null)
            Var::Func(f) => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("__func__", &f.id)?;
                map.end()
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for Var {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Route through serde's own value model (Option/Seq/Map/Scalar) and
        // reconstruct the matching `Var`. Using a visitor keeps it format-
        // agnostic.
        use serde::de::{self, MapAccess, SeqAccess, Visitor};
        use std::fmt;

        struct VarVisitor;

        impl<'de> Visitor<'de> for VarVisitor {
            type Value = Var;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any JSON-like value")
            }

            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Var, E> {
                Ok(Var::Bool(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Var, E> {
                Ok(Var::I64(v))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Var, E> {
                Ok(Var::I64(v as i64))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Var, E> {
                Ok(Var::F64(v))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Var, E> {
                Ok(Var::Str(v.to_string()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Var, E> {
                Ok(Var::Str(v))
            }
            fn visit_none<E: de::Error>(self) -> Result<Var, E> {
                Ok(Var::Null)
            }
            fn visit_unit<E: de::Error>(self) -> Result<Var, E> {
                Ok(Var::Null)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Var, A::Error> {
                let mut items: Vec<Var> = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Var::List(List::from_iter(items)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Var, A::Error> {
                // Buffer the entries first: the one-key map `{"__func__": id}`
                // is a serialized `Func` handle (see `Serialize`) and must be
                // intercepted before it turns into a plain object.
                let mut entries: Vec<(String, Var)> = Vec::new();
                while let Some((k, v)) = map.next_entry::<String, Var>()? {
                    entries.push((k, v));
                }
                if let [(key, id)] = entries.as_slice() {
                    if key.as_str() == "__func__" {
                        return Ok(func_from_handle(id));
                    }
                }
                let o = Object::new();
                for (k, v) in entries {
                    o.set(k, v);
                }
                Ok(Var::Object(o))
            }
        }

        deserializer.deserialize_any(VarVisitor)
    }
}

/// Typed read conversions — the JS-flavored way to pull a concrete value out
/// of a `Var`. `let s: String = v.get("name").into();`
///
/// `Null` converts to the zero/empty value (JS-ish coercion: null → 0/""/false),
/// and a type mismatch coerces via `to_string()` for strings or saturates to
/// zero for numbers. For strict reads, use the `as_*()` methods instead.
impl From<Var> for String {
    fn from(v: Var) -> Self {
        v.to_string()
    }
}
impl From<Var> for i64 {
    fn from(v: Var) -> Self {
        v.as_i64().unwrap_or(0)
    }
}
impl From<Var> for f64 {
    fn from(v: Var) -> Self {
        v.as_f64().unwrap_or(0.0)
    }
}
impl From<Var> for bool {
    fn from(v: Var) -> Self {
        v.is_truthy()
    }
}

/// Recursively write `path` ("a.b.c") on an object, creating intermediate
/// objects. Because payloads are `Arc<Mutex>`, this mutates the shared data
/// in place, visible through every handle (on any thread) — matching gold's
/// `varRef`.
pub(crate) fn set_path_impl(target: &Var, path: &str, value: Var) {
    let (first, rest) = match path.split_once('.') {
        Some((f, r)) => (f, Some(r)),
        None => (path, None),
    };
    match target {
        Var::Object(o) => match rest {
            Some(rest) => {
                // descend: ensure child is an object, then recurse on the tail
                let mut child = o.get(first);
                if !matches!(child, Var::Object(_)) {
                    child = Var::from(Object::new());
                    o.set(first, child.clone());
                }
                set_path_impl(&child, rest, value);
            }
            None => o.set(first, value),
        },
        other => panic!("cannot descend into {other}"),
    }
}
