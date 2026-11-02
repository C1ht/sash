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

/// Parses JSON text (replay reads bundles with it; tests only).
#[cfg(test)]
pub fn parse(text: &str) -> Result<Json, String> {
    let mut parser = Parser {
        bytes: text.as_bytes(),
        at: 0,
    };
    let value = parser.value()?;
    parser.space();
    if parser.at != parser.bytes.len() {
        return Err(format!("trailing text at {}", parser.at));
    }
    Ok(value)
}

#[cfg(test)]
struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

#[cfg(test)]
impl Parser<'_> {
    fn space(&mut self) {
        while self.at < self.bytes.len() && self.bytes[self.at].is_ascii_whitespace() {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> Result<(), String> {
        self.space();
        if self.bytes.get(self.at) == Some(&byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(format!("expected '{}' at {}", byte as char, self.at))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.space();
        match self.bytes.get(self.at) {
            Some(b'{') => {
                self.at += 1;
                let mut fields = Vec::new();
                self.space();
                if self.bytes.get(self.at) == Some(&b'}') {
                    self.at += 1;
                    return Ok(Json::Obj(fields));
                }
                loop {
                    self.space();
                    let key = self.string()?;
                    self.eat(b':')?;
                    fields.push((key, self.value()?));
                    self.space();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b'}') => {
                            self.at += 1;
                            return Ok(Json::Obj(fields));
                        }
                        _ => return Err(format!("expected ',' or '}}' at {}", self.at)),
                    }
                }
            }
            Some(b'[') => {
                self.at += 1;
                let mut items = Vec::new();
                self.space();
                if self.bytes.get(self.at) == Some(&b']') {
                    self.at += 1;
                    return Ok(Json::Arr(items));
                }
                loop {
                    items.push(self.value()?);
                    self.space();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(Json::Arr(items));
                        }
                        _ => return Err(format!("expected ',' or ']' at {}", self.at)),
                    }
                }
            }
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.word("true", Json::Bool(true)),
            Some(b'f') => self.word("false", Json::Bool(false)),
            Some(b'n') => self.word("null", Json::Null),
            Some(_) => self.number(),
            None => Err("unexpected end".into()),
        }
    }

    fn word(&mut self, word: &str, value: Json) -> Result<Json, String> {
        if self.bytes[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(format!("unexpected text at {}", self.at))
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.at;
        while self.at < self.bytes.len()
            && matches!(
                self.bytes[self.at],
                b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
            )
        {
            self.at += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at]).map_err(|e| e.to_string())?;
        if let Ok(n) = text.parse::<i64>() {
            return Ok(Json::Int(n));
        }
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("bad number {text:?} at {start}"))
    }

    fn string(&mut self) -> Result<String, String> {
        self.eat(b'"')?;
        let mut out = String::new();
        loop {
            let Some(&byte) = self.bytes.get(self.at) else {
                return Err("unterminated string".into());
            };
            self.at += 1;
            match byte {
                b'"' => return Ok(out),
                b'\\' => {
                    let escaped = *self.bytes.get(self.at).ok_or("bad escape")?;
                    self.at += 1;
                    match escaped {
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let hex = std::str::from_utf8(&self.bytes[self.at..self.at + 4])
                                .map_err(|e| e.to_string())?;
                            let code = u32::from_str_radix(hex, 16).map_err(|e| e.to_string())?;
                            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                            self.at += 4;
                        }
                        other => out.push(other as char),
                    }
                }
                _ => {
                    // A UTF-8 sequence: copy it whole.
                    let start = self.at - 1;
                    let len = match byte {
                        0..=0x7f => 1,
                        0xc0..=0xdf => 2,
                        0xe0..=0xef => 3,
                        _ => 4,
                    };
                    self.at = start + len;
                    out.push_str(
                        std::str::from_utf8(&self.bytes[start..self.at])
                            .map_err(|e| e.to_string())?,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
impl Json {
    /// The value of `key` in an object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Int(n) => Some(*n),
            Json::Num(n) => Some(*n as i64),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Int(n) => Some(*n as f64),
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
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

    #[test]
    fn parses_what_it_writes() {
        let value = Json::obj([
            ("s", "a \"q\" \\ \n\u{1} é".into()),
            ("n", (-3).into()),
            ("f", 0.5.into()),
            ("none", Json::Null),
            (
                "arr",
                vec![true.into(), false.into(), Json::Arr(vec![])].into(),
            ),
            ("obj", Json::obj([])),
        ]);
        assert_eq!(parse(&value.to_string()), Ok(value));
        assert!(parse("{\"a\":1,}").is_err());
        assert!(parse("[1 2]").is_err());
    }
}
