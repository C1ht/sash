// SPDX-License-Identifier: Apache-2.0

//! The small JSON writer the recorder needs (no parser): values are built in code and
//! written out once.

use std::fmt::{self, Write};

use crate::layout::{Rect, WindowId};

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn obj<'a>(fields: impl IntoIterator<Item = (&'a str, Json)>) -> Json {
        Json::Obj(
            fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }
}

fn write_str(out: &mut fmt::Formatter<'_>, s: &str) -> fmt::Result {
    out.write_char('"')?;
    for c in s.chars() {
        match c {
            '"' => out.write_str("\\\"")?,
            '\\' => out.write_str("\\\\")?,
            '\n' => out.write_str("\\n")?,
            '\r' => out.write_str("\\r")?,
            '\t' => out.write_str("\\t")?,
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32)?,
            c => out.write_char(c)?,
        }
    }
    out.write_char('"')
}

impl fmt::Display for Json {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Json::Null => out.write_str("null"),
            Json::Bool(b) => write!(out, "{b}"),
            Json::Int(n) => write!(out, "{n}"),
            Json::Num(n) if n.is_finite() => write!(out, "{n}"),
            Json::Num(_) => out.write_str("null"),
            Json::Str(s) => write_str(out, s),
            Json::Arr(items) => {
                out.write_char('[')?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.write_char(',')?;
                    }
                    write!(out, "{item}")?;
                }
                out.write_char(']')
            }
            Json::Obj(fields) => {
                out.write_char('{')?;
                for (i, (key, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.write_char(',')?;
                    }
                    write_str(out, key)?;
                    write!(out, ":{value}")?;
                }
                out.write_char('}')
            }
        }
    }
}

impl From<bool> for Json {
    fn from(b: bool) -> Self {
        Json::Bool(b)
    }
}

macro_rules! from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Json {
            fn from(n: $t) -> Self {
                Json::Int(n as i64)
            }
        }
    )*};
}
from_int!(i32, i64, u32, u64, usize, u16);

impl From<f64> for Json {
    fn from(n: f64) -> Self {
        Json::Num(n)
    }
}

impl From<&str> for Json {
    fn from(s: &str) -> Self {
        Json::Str(s.to_string())
    }
}

impl From<String> for Json {
    fn from(s: String) -> Self {
        Json::Str(s)
    }
}

impl From<Vec<Json>> for Json {
    fn from(items: Vec<Json>) -> Self {
        Json::Arr(items)
    }
}

impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(value: Option<T>) -> Self {
        value.map_or(Json::Null, Into::into)
    }
}

/// [x, y, w, h].
impl From<Rect> for Json {
    fn from(r: Rect) -> Self {
        Json::Arr(vec![r.x.into(), r.y.into(), r.w.into(), r.h.into()])
    }
}

impl From<WindowId> for Json {
    fn from(id: WindowId) -> Self {
        Json::Int(id.0 as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_valid_json() {
        let value = Json::obj([
            ("s", "a \"q\" \\ \n\u{1}".into()),
            ("n", 3.into()),
            ("f", 0.5.into()),
            ("nan", f64::NAN.into()),
            ("none", Option::<u32>::None.into()),
            ("arr", vec![true.into(), Json::Null].into()),
        ]);
        assert_eq!(
            value.to_string(),
            r#"{"s":"a \"q\" \\ \n\u0001","n":3,"f":0.5,"nan":null,"none":null,"arr":[true,null]}"#
        );
    }
}
