//! `serde::Deserializer` that reads Lisp values directly.

use crate::ffi::env::{Env, Exit, Value};
use crate::names::to_kebab;
use serde::de::{self, DeserializeOwned, DeserializeSeed, IntoDeserializer, Visitor};
use std::fmt;

/// Maximum nesting depth of a decoded value.
const MAX_DEPTH: usize = 128;

#[derive(Debug)]
pub(crate) enum DeError {
    /// A Lisp non-local exit is pending.
    Exit,
    /// `path` holds the location of the offending value, innermost first.
    Msg { msg: String, path: Vec<String> },
}

impl DeError {
    fn msg(msg: impl Into<String>) -> DeError {
        DeError::Msg { msg: msg.into(), path: Vec::new() }
    }

    /// Record that the error happened inside `segment`.
    fn at(self, segment: impl Into<String>) -> DeError {
        match self {
            DeError::Exit => DeError::Exit,
            DeError::Msg { msg, mut path } => {
                path.push(segment.into());
                DeError::Msg { msg, path }
            }
        }
    }
}

impl fmt::Display for DeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeError::Exit => f.write_str("Lisp non-local exit"),
            DeError::Msg { msg, path } if path.is_empty() => f.write_str(msg),
            DeError::Msg { msg, path } => {
                let path: Vec<&str> = path.iter().rev().map(String::as_str).collect();
                write!(f, "at {}: {}", path.join(" "), msg)
            }
        }
    }
}

impl std::error::Error for DeError {}

impl de::Error for DeError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        DeError::msg(msg.to_string())
    }
}

impl From<Exit> for DeError {
    fn from(_: Exit) -> Self {
        DeError::Exit
    }
}

type Result<T> = std::result::Result<T, DeError>;

/// Convert a Lisp value to `T`. Must run on the Emacs thread.
pub(crate) fn from_lisp<T: DeserializeOwned>(env: &Env, value: Value) -> Result<T> {
    T::deserialize(De { env, value, depth: 0 })
}

#[derive(PartialEq)]
enum Kind {
    Nil,
    Symbol,
    Int,
    Float,
    Str,
    Cons,
    Vector,
    Hash,
    Other(String),
}

fn kind_of(env: &Env, value: Value) -> Result<Kind> {
    if !env.is_not_nil(value) {
        return Ok(Kind::Nil);
    }
    Ok(match env.type_name(value)?.as_str() {
        "symbol" | "boolean" => Kind::Symbol,
        "integer" => Kind::Int,
        "float" => Kind::Float,
        "string" => Kind::Str,
        "cons" => Kind::Cons,
        "vector" => Kind::Vector,
        "hash-table" => Kind::Hash,
        other => Kind::Other(other.to_string()),
    })
}

fn describe(kind: &Kind) -> &str {
    match kind {
        Kind::Nil => "nil",
        Kind::Symbol => "symbol",
        Kind::Int => "integer",
        Kind::Float => "float",
        Kind::Str => "string",
        Kind::Cons => "list",
        Kind::Vector => "vector",
        Kind::Hash => "hash-table",
        Kind::Other(name) => name,
    }
}

/// The name of `value` without the colon if it is a keyword.
fn keyword_name(env: &Env, value: Value) -> Result<Option<String>> {
    if kind_of(env, value)? != Kind::Symbol {
        return Ok(None);
    }
    let name = env.symbol_name(value)?;
    Ok(name.strip_prefix(':').map(str::to_string))
}

/// Kebab-case lookup table for the field or variant names of a type.
fn name_table(names: &'static [&'static str]) -> Result<Vec<(String, &'static str)>> {
    let mut table: Vec<(String, &'static str)> = Vec::with_capacity(names.len());
    for name in names {
        let kebab = to_kebab(name);
        if let Some((_, other)) = table.iter().find(|(k, _)| *k == kebab) {
            return Err(DeError::msg(format!("names `{other}` and `{name}` both map to :{kebab}")));
        }
        table.push((kebab, name));
    }
    Ok(table)
}

#[derive(Clone, Copy)]
struct De<'e> {
    env: &'e Env,
    value: Value,
    depth: usize,
}

impl<'e> De<'e> {
    fn kind(&self) -> Result<Kind> {
        kind_of(self.env, self.value)
    }

    fn mismatch(&self, expected: &str) -> DeError {
        match self.kind() {
            Ok(kind) => DeError::msg(format!("expected {expected}, got {}", describe(&kind))),
            Err(e) => e,
        }
    }

    fn int_parts(&self) -> Result<(bool, u128)> {
        if self.kind()? != Kind::Int {
            return Err(self.mismatch("integer"));
        }
        self.env.extract_big(self.value)?.ok_or_else(|| DeError::msg("integer out of range"))
    }

    fn signed(&self) -> Result<i128> {
        let (negative, magnitude) = self.int_parts()?;
        let limit = 1u128 << 127;
        if negative && magnitude <= limit {
            Ok((magnitude as i128).wrapping_neg())
        } else if !negative && magnitude < limit {
            Ok(magnitude as i128)
        } else {
            Err(DeError::msg("integer out of range"))
        }
    }

    fn unsigned(&self) -> Result<u128> {
        match self.int_parts()? {
            (true, magnitude) if magnitude != 0 => Err(DeError::msg("expected a non-negative integer")),
            (_, magnitude) => Ok(magnitude),
        }
    }

    fn seq(&self) -> Result<Seq<'e>> {
        let src = match self.kind()? {
            Kind::Nil | Kind::Cons => {
                ensure_proper_list(self.env, self.value)?;
                Src::List(self.value)
            }
            Kind::Vector => Src::Vector { vector: self.value, next: 0, len: self.env.vec_size(self.value)? },
            _ => return Err(self.mismatch("list or vector")),
        };
        Ok(Seq { env: self.env, depth: self.depth, index: 0, src })
    }

    fn plist(&self, fields: &'static [&'static str]) -> Result<Plist<'e>> {
        match self.kind()? {
            Kind::Nil | Kind::Cons => {
                ensure_proper_list(self.env, self.value)?;
                Ok(Plist { env: self.env, depth: self.depth, cur: self.value, names: name_table(fields)?, value: None })
            }
            _ => Err(self.mismatch("plist")),
        }
    }

    fn alist(&self) -> Result<Alist<'e>> {
        let cur = match self.kind()? {
            Kind::Nil | Kind::Cons => {
                ensure_proper_list(self.env, self.value)?;
                self.value
            }
            Kind::Hash => self.env.call("emrs--hash-to-alist", &[self.value])?,
            _ => return Err(self.mismatch("alist or hash table")),
        };
        Ok(Alist { env: self.env, depth: self.depth, cur, value: None })
    }
}

/// A deserializer for a value nested one level below `depth`.
fn child<'e>(env: &'e Env, value: Value, depth: usize) -> Result<De<'e>> {
    if depth >= MAX_DEPTH {
        return Err(DeError::msg(format!("value is nested deeper than {MAX_DEPTH} levels")));
    }
    Ok(De { env, value, depth: depth + 1 })
}

/// Fail unless `value` is nil or a proper list. One call into Lisp rejects
/// dotted and circular lists before any element is read.
fn ensure_proper_list(env: &Env, value: Value) -> Result<()> {
    if !env.is_not_nil(value) || env.is_not_nil(env.call("proper-list-p", &[value])?) {
        Ok(())
    } else {
        Err(DeError::msg("expected a proper list, got a dotted or circular list"))
    }
}

/// Pop the next element of a proper list.
fn pop(env: &Env, cur: &mut Value) -> Result<Option<Value>> {
    match kind_of(env, *cur)? {
        Kind::Nil => Ok(None),
        Kind::Cons => {
            let car = env.car(*cur)?;
            *cur = env.cdr(*cur)?;
            Ok(Some(car))
        }
        _ => Err(DeError::msg("improper list")),
    }
}

macro_rules! de_signed {
    ($($method:ident $visit:ident $ty:ty),*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
            let n = self.signed()?;
            let n = <$ty>::try_from(n)
                .map_err(|_| DeError::msg(format!("integer {n} out of range for {}", stringify!($ty))))?;
            visitor.$visit(n)
        }
    )*};
}

macro_rules! de_unsigned {
    ($($method:ident $visit:ident $ty:ty),*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
            let n = self.unsigned()?;
            let n = <$ty>::try_from(n)
                .map_err(|_| DeError::msg(format!("integer {n} out of range for {}", stringify!($ty))))?;
            visitor.$visit(n)
        }
    )*};
}

impl<'de> de::Deserializer<'de> for De<'_> {
    type Error = DeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.kind()? {
            Kind::Nil => visitor.visit_unit(),
            Kind::Symbol => {
                let name = self.env.symbol_name(self.value)?;
                if name == "t" { visitor.visit_bool(true) } else { visitor.visit_string(name) }
            }
            Kind::Int => match self.int_parts()? {
                (false, m) if m <= u64::MAX as u128 => visitor.visit_u64(m as u64),
                (false, m) => visitor.visit_u128(m),
                (true, _) => match i64::try_from(self.signed()?) {
                    Ok(n) => visitor.visit_i64(n),
                    Err(_) => visitor.visit_i128(self.signed()?),
                },
            },
            Kind::Float => self.deserialize_f64(visitor),
            Kind::Str => self.deserialize_string(visitor),
            Kind::Vector => self.deserialize_seq(visitor),
            Kind::Hash => self.deserialize_map(visitor),
            Kind::Cons => {
                ensure_proper_list(self.env, self.value)?;
                let head = self.env.car(self.value)?;
                if keyword_name(self.env, head)?.is_some() {
                    visitor.visit_map(self.plist(&[])?)
                } else {
                    self.deserialize_seq(visitor)
                }
            }
            Kind::Other(name) => Err(DeError::msg(format!("cannot decode a value of type {name}"))),
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_bool(self.env.is_not_nil(self.value))
    }

    de_signed!(
        deserialize_i8 visit_i8 i8,
        deserialize_i16 visit_i16 i16,
        deserialize_i32 visit_i32 i32,
        deserialize_i64 visit_i64 i64,
        deserialize_i128 visit_i128 i128
    );

    de_unsigned!(
        deserialize_u8 visit_u8 u8,
        deserialize_u16 visit_u16 u16,
        deserialize_u32 visit_u32 u32,
        deserialize_u64 visit_u64 u64,
        deserialize_u128 visit_u128 u128
    );

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_f64(visitor)
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.kind()? {
            Kind::Float => visitor.visit_f64(self.env.extract_float(self.value)?),
            Kind::Int => visitor.visit_f64(self.signed()? as f64),
            _ => Err(self.mismatch("number")),
        }
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.kind()? {
            Kind::Int => {
                let c = u32::try_from(self.unsigned()?).ok().and_then(char::from_u32);
                visitor.visit_char(c.ok_or_else(|| DeError::msg("integer is not a valid character"))?)
            }
            Kind::Str => {
                let s = self.env.string(self.value)?;
                let mut chars = s.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => visitor.visit_char(c),
                    _ => Err(DeError::msg("expected a string of length 1")),
                }
            }
            _ => Err(self.mismatch("character")),
        }
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_string(visitor)
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.kind()? {
            Kind::Str => visitor.visit_string(self.env.string(self.value)?),
            _ => Err(self.mismatch("string")),
        }
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_byte_buf(visitor)
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        match self.kind()? {
            Kind::Str => visitor.visit_byte_buf(self.env.string_bytes(self.value)?),
            _ => Err(self.mismatch("string")),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        if self.env.is_not_nil(self.value) { visitor.visit_some(self) } else { visitor.visit_none() }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        if self.env.is_not_nil(self.value) { Err(self.mismatch("nil")) } else { visitor.visit_unit() }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_seq(self.seq()?)
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value> {
        let mut seq = self.seq()?;
        let value = visitor.visit_seq(&mut seq)?;
        seq.expect_end(len)?;
        Ok(value)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value> {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_map(self.alist()?)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        visitor.visit_map(self.plist(fields)?)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value> {
        let (name, rest) = if let Some(name) = keyword_name(self.env, self.value)? {
            (name, None)
        } else if self.kind()? == Kind::Cons {
            ensure_proper_list(self.env, self.value)?;
            let head = self.env.car(self.value)?;
            let name = keyword_name(self.env, head)?
                .ok_or_else(|| DeError::msg("expected a keyword at the head of the list"))?;
            (name, Some(self.env.cdr(self.value)?))
        } else {
            return Err(self.mismatch("keyword or keyword-headed list"));
        };
        let segment = format!("(:{name})");
        visitor.visit_enum(EnumDe { env: self.env, depth: self.depth, name, rest, variants }).map_err(|e| e.at(segment))
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        self.deserialize_string(visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value> {
        visitor.visit_unit()
    }
}

enum Src {
    List(Value),
    Vector { vector: Value, next: isize, len: isize },
}

struct Seq<'e> {
    env: &'e Env,
    depth: usize,
    index: usize,
    src: Src,
}

impl Seq<'_> {
    fn next_value(&mut self) -> Result<Option<Value>> {
        match &mut self.src {
            Src::List(cur) => pop(self.env, cur),
            Src::Vector { vector, next, len } => {
                if *next >= *len {
                    return Ok(None);
                }
                let value = self.env.vec_get(*vector, *next)?;
                *next += 1;
                Ok(Some(value))
            }
        }
    }

    /// Fail if elements remain after a fixed-length read of `len`.
    fn expect_end(&mut self, len: usize) -> Result<()> {
        match self.next_value()? {
            None => Ok(()),
            Some(_) => Err(DeError::msg(format!("expected {len} elements, got more"))),
        }
    }
}

impl<'de> de::SeqAccess<'de> for Seq<'_> {
    type Error = DeError;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>> {
        let Some(value) = self.next_value()? else {
            return Ok(None);
        };
        let index = self.index;
        self.index += 1;
        seed.deserialize(child(self.env, value, self.depth)?).map(Some).map_err(|e| e.at(format!("[{index}]")))
    }
}

struct Plist<'e> {
    env: &'e Env,
    depth: usize,
    cur: Value,
    names: Vec<(String, &'static str)>,
    /// Value and keyword name of the key returned last.
    value: Option<(Value, String)>,
}

impl<'de> de::MapAccess<'de> for Plist<'_> {
    type Error = DeError;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        let Some(key) = pop(self.env, &mut self.cur)? else {
            return Ok(None);
        };
        let keyword = keyword_name(self.env, key)?.ok_or_else(|| DeError::msg("expected a keyword key in plist"))?;
        let value =
            pop(self.env, &mut self.cur)?.ok_or_else(|| DeError::msg(format!("missing value for :{keyword}")))?;
        let name = match self.names.iter().find(|(kebab, _)| *kebab == keyword) {
            Some((_, original)) => original.to_string(),
            None => keyword.clone(),
        };
        self.value = Some((value, keyword));
        let de: de::value::StringDeserializer<DeError> = name.into_deserializer();
        seed.deserialize(de).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        let (value, keyword) = self.value.take().expect("next_value_seed called before next_key_seed");
        seed.deserialize(child(self.env, value, self.depth)?).map_err(|e| e.at(format!(":{keyword}")))
    }
}

struct Alist<'e> {
    env: &'e Env,
    depth: usize,
    cur: Value,
    value: Option<Value>,
}

impl<'de> de::MapAccess<'de> for Alist<'_> {
    type Error = DeError;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>> {
        let Some(entry) = pop(self.env, &mut self.cur)? else {
            return Ok(None);
        };
        if kind_of(self.env, entry)? != Kind::Cons {
            return Err(DeError::msg("alist entry must be a cons"));
        }
        self.value = Some(self.env.cdr(entry)?);
        seed.deserialize(child(self.env, self.env.car(entry)?, self.depth)?).map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value> {
        let value = self.value.take().expect("next_value_seed called before next_key_seed");
        seed.deserialize(child(self.env, value, self.depth)?)
    }
}

struct EnumDe<'e> {
    env: &'e Env,
    depth: usize,
    /// Variant keyword without the colon.
    name: String,
    /// The list after the keyword; `None` for a bare keyword.
    rest: Option<Value>,
    variants: &'static [&'static str],
}

impl<'de, 'e> de::EnumAccess<'de> for EnumDe<'e> {
    type Error = DeError;
    type Variant = Self;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self)> {
        let table = name_table(self.variants)?;
        let Some((_, original)) = table.iter().find(|(kebab, _)| *kebab == self.name) else {
            let known: Vec<String> = table.iter().map(|(kebab, _)| format!(":{kebab}")).collect();
            return Err(DeError::msg(format!("unknown variant, expected one of {}", known.join(" "))));
        };
        let de: de::value::StrDeserializer<DeError> = (*original).into_deserializer();
        let value = seed.deserialize(de)?;
        Ok((value, self))
    }
}

impl<'de> de::VariantAccess<'de> for EnumDe<'_> {
    type Error = DeError;

    fn unit_variant(self) -> Result<()> {
        match self.rest {
            Some(rest) if self.env.is_not_nil(rest) => Err(DeError::msg("expected no values")),
            _ => Ok(()),
        }
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value> {
        let mut rest = self.rest.ok_or_else(|| DeError::msg("expected exactly one value"))?;
        let value = pop(self.env, &mut rest)?.ok_or_else(|| DeError::msg("expected exactly one value"))?;
        if self.env.is_not_nil(rest) {
            return Err(DeError::msg("expected exactly one value"));
        }
        seed.deserialize(child(self.env, value, self.depth)?)
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value> {
        let rest = self.rest.ok_or_else(|| DeError::msg(format!("expected {len} values")))?;
        let mut seq = Seq { env: self.env, depth: self.depth, index: 0, src: Src::List(rest) };
        let value = visitor.visit_seq(&mut seq)?;
        seq.expect_end(len)?;
        Ok(value)
    }

    fn struct_variant<V: Visitor<'de>>(self, fields: &'static [&'static str], visitor: V) -> Result<V::Value> {
        let cur = match self.rest {
            Some(rest) => rest,
            None => self.env.nil()?,
        };
        visitor.visit_map(Plist { env: self.env, depth: self.depth, cur, names: name_table(fields)?, value: None })
    }
}

#[cfg(test)]
mod tests {
    use super::from_lisp;
    use crate::ffi::fake::Fake;
    use serde::Deserialize;
    use serde::de::DeserializeOwned;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn de<T: DeserializeOwned>(text: &str) -> Result<T, String> {
        let mut fake = Fake::new();
        let value = fake.read(text);
        let env = fake.env();
        from_lisp(&env, value).map_err(|e| e.to_string())
    }

    #[derive(Deserialize, Debug, PartialEq)]
    struct Match {
        path: PathBuf,
        line_number: u64,
        preview: Option<String>,
    }

    #[derive(Deserialize, Debug, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Strict {
        a: i32,
    }

    #[derive(Deserialize, Debug, PartialEq)]
    enum Request {
        Stop,
        Search(String),
        Pair(i32, i32),
        Log { rev: String, limit: usize },
        HTTPGet(Match),
    }

    #[test]
    fn scalars() {
        assert_eq!(de::<bool>("t"), Ok(true));
        assert_eq!(de::<bool>("nil"), Ok(false));
        assert_eq!(de::<bool>("0"), Ok(true), "any non-nil value is true");
        assert_eq!(de::<i8>("-5"), Ok(-5));
        assert_eq!(de::<u64>("18446744073709551615"), Ok(u64::MAX));
        assert_eq!(de::<i128>("-170141183460469231731687303715884105728"), Ok(i128::MIN));
        assert_eq!(de::<u128>("340282366920938463463374607431768211455"), Ok(u128::MAX));
        assert_eq!(de::<f64>("1.5"), Ok(1.5));
        assert_eq!(de::<f64>("2"), Ok(2.0), "integers are accepted for floats");
        assert_eq!(de::<String>("\"hé\""), Ok("hé".to_string()));
        assert_eq!(de::<()>("nil"), Ok(()));
    }

    #[test]
    fn integer_range_and_type_errors() {
        assert_eq!(de::<u8>("256"), Err("integer 256 out of range for u8".into()));
        assert_eq!(de::<i8>("-129"), Err("integer -129 out of range for i8".into()));
        assert_eq!(de::<u32>("-1"), Err("expected a non-negative integer".into()));
        assert_eq!(de::<i32>("1.5"), Err("expected integer, got float".into()));
        assert_eq!(de::<i32>("nil"), Err("expected integer, got nil".into()));
        assert_eq!(de::<String>("sym"), Err("expected string, got symbol".into()));
        assert_eq!(de::<()>("1"), Err("expected nil, got integer".into()));
    }

    #[test]
    fn chars() {
        assert_eq!(de::<char>("?a"), Ok('a'));
        assert_eq!(de::<char>("\"é\""), Ok('é'));
        assert_eq!(de::<char>("\"ab\""), Err("expected a string of length 1".into()));
        assert_eq!(de::<char>("55296"), Err("integer is not a valid character".into()));
    }

    #[test]
    fn options() {
        assert_eq!(de::<Option<i32>>("nil"), Ok(None));
        assert_eq!(de::<Option<i32>>("3"), Ok(Some(3)));
    }

    #[test]
    fn bytes_from_unibyte_and_multibyte_strings() {
        #[derive(Debug, PartialEq)]
        struct Raw(Vec<u8>);
        impl<'de> Deserialize<'de> for Raw {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                struct V;
                impl serde::de::Visitor<'_> for V {
                    type Value = Raw;
                    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                        f.write_str("bytes")
                    }
                    fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Raw, E> {
                        Ok(Raw(v))
                    }
                }
                d.deserialize_byte_buf(V)
            }
        }
        assert_eq!(de::<Raw>("#b(0 200 255)"), Ok(Raw(vec![0, 200, 255])));
        assert_eq!(de::<Raw>("\"é\""), Ok(Raw(vec![0xC3, 0xA9])));
    }

    #[test]
    fn sequences_from_lists_and_vectors() {
        assert_eq!(de::<Vec<i32>>("(1 2 3)"), Ok(vec![1, 2, 3]));
        assert_eq!(de::<Vec<i32>>("[1 2 3]"), Ok(vec![1, 2, 3]));
        assert_eq!(de::<Vec<i32>>("nil"), Ok(vec![]));
        assert_eq!(de::<Vec<i32>>("(1 . 2)"), Err("expected a proper list, got a dotted or circular list".into()));
        assert_eq!(de::<Request>("(:pair 1 . 2)"), Err("expected a proper list, got a dotted or circular list".into()));
        assert_eq!(de::<Vec<i32>>("(1 \"x\")"), Err("at [1]: expected integer, got string".into()));
        assert_eq!(de::<Vec<i32>>("5"), Err("expected list or vector, got integer".into()));
    }

    #[test]
    fn tuples_check_their_length() {
        assert_eq!(de::<(i32, String)>("(1 \"a\")"), Ok((1, "a".to_string())));
        assert_eq!(de::<(i32, i32)>("(1 2 3)"), Err("expected 2 elements, got more".into()));
        assert!(de::<(i32, i32)>("(1)").unwrap_err().contains("invalid length 1"));
    }

    #[test]
    fn maps_from_alists_and_hash_tables() {
        let expected: BTreeMap<String, i32> = [("a".to_string(), 1), ("b".to_string(), 2)].into();
        assert_eq!(de::<BTreeMap<String, i32>>("((\"a\" . 1) (\"b\" . 2))"), Ok(expected.clone()));
        assert_eq!(de::<BTreeMap<String, i32>>("#h(\"a\" 1 \"b\" 2)"), Ok(expected));
        assert_eq!(de::<BTreeMap<String, i32>>("nil"), Ok(BTreeMap::new()));
        assert_eq!(de::<BTreeMap<String, i32>>("(1 2)"), Err("alist entry must be a cons".into()));
    }

    #[test]
    fn structs_from_plists() {
        let expected = Match { path: "/a.rs".into(), line_number: 42, preview: None };
        assert_eq!(de::<Match>("(:path \"/a.rs\" :line-number 42 :preview nil)"), Ok(expected));
        let reordered = de::<Match>("(:line-number 1 :path \"/b\")").unwrap();
        assert_eq!(reordered, Match { path: "/b".into(), line_number: 1, preview: None });
    }

    #[test]
    fn struct_errors_name_the_path() {
        assert_eq!(
            de::<Match>("(:path \"/a\" :line-number \"x\")"),
            Err("at :line-number: expected integer, got string".into())
        );
        assert!(de::<Match>("(:path \"/a\")").unwrap_err().contains("missing field `line_number`"));
        assert_eq!(de::<Match>("(:path)"), Err("missing value for :path".into()));
        assert_eq!(de::<Match>("(path \"/a\")"), Err("expected a keyword key in plist".into()));
        assert_eq!(de::<Match>("5"), Err("expected plist, got integer".into()));
        assert_eq!(de::<Vec<Match>>("((:path 1))"), Err("at [0] :path: expected string, got integer".into()));
    }

    #[test]
    fn unknown_keys_follow_serde_attributes() {
        assert!(de::<Match>("(:path \"/a\" :line-number 1 :extra 9)").is_ok(), "ignored by default");
        assert!(de::<Strict>("(:a 1 :extra 9)").unwrap_err().contains("unknown field `extra`"));
    }

    #[test]
    fn enums_from_keyword_lists() {
        assert_eq!(de::<Request>("(:stop)"), Ok(Request::Stop));
        assert_eq!(de::<Request>(":stop"), Ok(Request::Stop), "bare keyword for a unit variant");
        assert_eq!(de::<Request>("(:search \"hello\")"), Ok(Request::Search("hello".into())));
        assert_eq!(de::<Request>("(:pair 1 2)"), Ok(Request::Pair(1, 2)));
        assert_eq!(de::<Request>("(:log :rev \"HEAD\" :limit 20)"), Ok(Request::Log { rev: "HEAD".into(), limit: 20 }));
        assert_eq!(
            de::<Request>("(:http-get (:path \"/a\" :line-number 1))"),
            Ok(Request::HTTPGet(Match { path: "/a".into(), line_number: 1, preview: None }))
        );
    }

    #[test]
    fn enum_errors() {
        assert_eq!(
            de::<Request>("(:nope)"),
            Err("at (:nope): unknown variant, expected one of :stop :search :pair :log :http-get".into())
        );
        assert_eq!(de::<Request>("(:stop 1)"), Err("at (:stop): expected no values".into()));
        assert_eq!(de::<Request>("(:search)"), Err("at (:search): expected exactly one value".into()));
        assert_eq!(de::<Request>("(:search \"a\" \"b\")"), Err("at (:search): expected exactly one value".into()));
        assert_eq!(de::<Request>("(:pair 1 2 3)"), Err("at (:pair): expected 2 elements, got more".into()));
        assert_eq!(de::<Request>("(:log :rev 5 :limit 1)"), Err("at (:log) :rev: expected string, got integer".into()));
        assert_eq!(de::<Request>("(stop)"), Err("expected a keyword at the head of the list".into()));
        assert_eq!(de::<Request>("7"), Err("expected keyword or keyword-headed list, got integer".into()));
    }

    #[test]
    fn conflicting_names_are_rejected() {
        #[derive(Deserialize, Debug)]
        #[allow(non_snake_case, dead_code)]
        struct Clash {
            fooBar: i32,
            foo_bar: i32,
        }
        assert_eq!(de::<Clash>("(:foo-bar 1)").unwrap_err(), "names `fooBar` and `foo_bar` both map to :foo-bar");
    }

    #[test]
    fn nesting_is_limited() {
        #[derive(Deserialize, Debug)]
        struct Nest(#[allow(dead_code)] Vec<Nest>);
        let ok = format!("{}{}", "(".repeat(100), ")".repeat(100));
        assert!(de::<Nest>(&ok).is_ok());
        let deep = format!("{}{}", "(".repeat(200), ")".repeat(200));
        assert_eq!(
            de::<Nest>(&deep).unwrap_err().rsplit(": ").next().unwrap(),
            "value is nested deeper than 128 levels"
        );
    }

    #[test]
    fn self_describing_values() {
        #[derive(Deserialize, Debug, PartialEq, Clone)]
        #[serde(untagged)]
        enum Any {
            Bool(bool),
            Int(i64),
            Float(f64),
            Text(String),
            List(Vec<Any>),
            Map(BTreeMap<String, Any>),
            Unit(()),
        }
        assert_eq!(de::<Any>("nil"), Ok(Any::Unit(())));
        assert_eq!(de::<Any>("t"), Ok(Any::Bool(true)));
        assert_eq!(de::<Any>("-3"), Ok(Any::Int(-3)));
        assert_eq!(de::<Any>("1.5"), Ok(Any::Float(1.5)));
        assert_eq!(de::<Any>("\"s\""), Ok(Any::Text("s".into())));
        assert_eq!(de::<Any>("sym"), Ok(Any::Text("sym".into())));
        assert_eq!(de::<Any>("[1 (2)]"), Ok(Any::List(vec![Any::Int(1), Any::List(vec![Any::Int(2)])])));
        let map: BTreeMap<String, Any> = [("a".to_string(), Any::Int(1))].into();
        assert_eq!(de::<Any>("(:a 1)"), Ok(Any::Map(map.clone())));
        assert_eq!(de::<Any>("#h(\"a\" 1)"), Ok(Any::Map(map)));
    }
}
