//! Template values: the small dynamic tree a page's context is built from,
//! and the operations the generated template code performs on it -- lookup,
//! truthiness, equality, and HTML-escaped output.
//!
//! Lookups of a missing name or attribute yield `Undefined`, which renders
//! as nothing and is false, as minijinja's lenient mode does.

use std::borrow::Cow;
use std::fmt::Write as _;

#[derive(Clone, Debug, Default)]
pub enum Value {
    #[default]
    Undefined,
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Cow<'static, str>),
    List(Vec<Value>),
    /// Insertion-ordered; keys are unique.
    Map(Vec<(&'static str, Value)>),
}

static UNDEFINED: Value = Value::Undefined;
static TRUE: Value = Value::Bool(true);
static FALSE: Value = Value::Bool(false);

impl Value {
    pub fn map() -> Self {
        Self::Map(Vec::new())
    }

    pub fn str(s: impl Into<String>) -> Self {
        Self::Str(Cow::Owned(s.into()))
    }

    /// Inserts or replaces `key` in a map; a no-op on anything else.
    pub fn set(&mut self, key: &'static str, value: Value) {
        if let Self::Map(entries) = self {
            if let Some(entry) = entries.iter_mut().find(|(k, _)| *k == key) {
                entry.1 = value;
            } else {
                entries.push((key, value));
            }
        }
    }

    pub fn set_str(&mut self, key: &'static str, s: impl Into<String>) {
        self.set(key, Self::str(s));
    }

    pub fn set_bool(&mut self, key: &'static str, b: bool) {
        self.set(key, Self::Bool(b));
    }

    pub fn push(&mut self, value: Value) {
        if let Self::List(items) = self {
            items.push(value);
        }
    }
}

/// A chain of name bindings: a template's loop variables and `set` names in
/// front of the render context.
#[derive(Debug)]
pub struct Scope<'a> {
    parent: Option<&'a Scope<'a>>,
    name: &'static str,
    value: &'a Value,
    /// Only on the root scope.
    context: Option<&'a Value>,
}

impl<'a> Scope<'a> {
    pub fn root(context: &'a Value) -> Self {
        Self { parent: None, name: "", value: &UNDEFINED, context: Some(context) }
    }

    pub fn bind(parent: &'a Scope<'a>, name: &'static str, value: &'a Value) -> Self {
        Self { parent: Some(parent), name, value, context: None }
    }
}

pub fn lookup<'a>(scope: &Scope<'a>, name: &str) -> &'a Value {
    let mut s = Some(scope);
    while let Some(frame) = s {
        if let Some(context) = frame.context {
            return attr(context, name);
        }
        if frame.name == name {
            return frame.value;
        }
        s = frame.parent;
    }
    &UNDEFINED
}

pub fn attr<'a>(v: &'a Value, name: &str) -> &'a Value {
    match v {
        Value::Map(entries) => {
            entries.iter().find(|(k, _)| *k == name).map_or(&UNDEFINED, |e| &e.1)
        }
        _ => &UNDEFINED,
    }
}

pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Undefined | Value::None => false,
        Value::Bool(b) => *b,
        Value::Int(i) => *i != 0,
        Value::Float(f) => *f != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::List(items) => !items.is_empty(),
        Value::Map(entries) => !entries.is_empty(),
    }
}

// Numbers compare as minijinja compares them: exactly, as f64.
#[allow(clippy::cast_precision_loss, clippy::float_cmp)]
pub fn equal(a: &Value, b: &Value) -> bool {
    let number = |v: &Value| match v {
        Value::Int(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        _ => None,
    };
    match (a, b) {
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::None, Value::None) | (Value::Undefined, Value::Undefined) => true,
        _ => match (number(a), number(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

pub fn is_none(v: &Value) -> bool {
    matches!(v, Value::None)
}

pub fn from_bool(b: bool) -> &'static Value {
    if b { &TRUE } else { &FALSE }
}

pub fn length(v: &Value) -> usize {
    match v {
        Value::List(items) => items.len(),
        Value::Map(entries) => entries.len(),
        _ => 0,
    }
}

pub fn index(v: &Value, i: usize) -> &Value {
    match v {
        Value::List(items) => items.get(i).unwrap_or(&UNDEFINED),
        _ => &UNDEFINED,
    }
}

pub fn html_escape(out: &mut String, s: &str) {
    let mut run = 0;
    for (i, c) in s.char_indices() {
        let escaped = match c {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\'' => "&#x27;",
            '/' => "&#x2f;",
            _ => continue,
        };
        out.push_str(&s[run..i]);
        out.push_str(escaped);
        run = i + 1;
    }
    out.push_str(&s[run..]);
}

/// Appends `v` as minijinja prints it into HTML: strings auto-escaped,
/// booleans as true/false, none as "none", undefined as nothing.
pub fn render(out: &mut String, v: &Value) {
    match v {
        Value::Undefined | Value::List(_) | Value::Map(_) => {}
        Value::None => out.push_str("none"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Value::Float(f) => {
            // minijinja prints a float with Display and appends ".0" to one
            // that has no fractional part shown.
            let text = format!("{f}");
            out.push_str(&text);
            if !text.contains(['.', 'N', 'i']) {
                out.push_str(".0");
            }
        }
        Value::Str(s) => html_escape(out, s),
    }
}
