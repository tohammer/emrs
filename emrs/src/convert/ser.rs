//! `serde::Serializer` that builds Lisp values directly.

use crate::ffi::env::{Env, Exit, Value};
use crate::names::to_kebab;
use serde::ser::{self, Serialize};
use std::fmt;

#[derive(Debug)]
pub(crate) enum SerError {
    /// A Lisp non-local exit is pending.
    Exit,
    Msg(String),
}

impl fmt::Display for SerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SerError::Exit => f.write_str("Lisp non-local exit"),
            SerError::Msg(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for SerError {}

impl ser::Error for SerError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        SerError::Msg(msg.to_string())
    }
}

impl From<Exit> for SerError {
    fn from(_: Exit) -> Self {
        SerError::Exit
    }
}

type Result<T> = std::result::Result<T, SerError>;

/// Convert `value` to a Lisp value. Must run on the Emacs thread.
pub(crate) fn to_lisp<T: Serialize + ?Sized>(env: &Env, value: &T) -> Result<Value> {
    value.serialize(Ser { env })
}

fn keyword(env: &Env, name: &str) -> Result<Value> {
    Ok(env.intern(&format!(":{}", to_kebab(name)))?)
}

#[derive(Clone, Copy)]
struct Ser<'e> {
    env: &'e Env,
}

/// Collects elements and ends as a Lisp list. Used for sequences, tuples,
/// structs (keyword/value pairs) and all non-unit enum variants (keyword
/// first).
struct ListSer<'e> {
    env: &'e Env,
    items: Vec<Value>,
}

impl<'e> ListSer<'e> {
    fn new(env: &'e Env, capacity: usize) -> Self {
        ListSer { env, items: Vec::with_capacity(capacity) }
    }

    fn variant(env: &'e Env, variant: &str, capacity: usize) -> Result<Self> {
        let mut list = ListSer::new(env, capacity + 1);
        list.items.push(keyword(env, variant)?);
        Ok(list)
    }

    fn push<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.items.push(to_lisp(self.env, value)?);
        Ok(())
    }

    fn field<T: Serialize + ?Sized>(&mut self, key: &str, value: &T) -> Result<()> {
        self.items.push(keyword(self.env, key)?);
        self.push(value)
    }

    fn finish(self) -> Result<Value> {
        Ok(self.env.list(&self.items)?)
    }
}

/// Collects `(KEY . VALUE)` pairs and ends as an alist.
struct MapSer<'e> {
    env: &'e Env,
    pairs: Vec<Value>,
    key: Option<Value>,
}

impl<'e> ser::Serializer for Ser<'e> {
    type Ok = Value;
    type Error = SerError;
    type SerializeSeq = ListSer<'e>;
    type SerializeTuple = ListSer<'e>;
    type SerializeTupleStruct = ListSer<'e>;
    type SerializeTupleVariant = ListSer<'e>;
    type SerializeMap = MapSer<'e>;
    type SerializeStruct = ListSer<'e>;
    type SerializeStructVariant = ListSer<'e>;

    fn serialize_bool(self, v: bool) -> Result<Value> {
        Ok(if v { self.env.t()? } else { self.env.nil()? })
    }

    fn serialize_i8(self, v: i8) -> Result<Value> {
        self.serialize_i64(v as i64)
    }

    fn serialize_i16(self, v: i16) -> Result<Value> {
        self.serialize_i64(v as i64)
    }

    fn serialize_i32(self, v: i32) -> Result<Value> {
        self.serialize_i64(v as i64)
    }

    fn serialize_i64(self, v: i64) -> Result<Value> {
        Ok(self.env.make_i64(v)?)
    }

    fn serialize_i128(self, v: i128) -> Result<Value> {
        Ok(self.env.make_big(v < 0, v.unsigned_abs())?)
    }

    fn serialize_u8(self, v: u8) -> Result<Value> {
        self.serialize_i64(v as i64)
    }

    fn serialize_u16(self, v: u16) -> Result<Value> {
        self.serialize_i64(v as i64)
    }

    fn serialize_u32(self, v: u32) -> Result<Value> {
        self.serialize_i64(v as i64)
    }

    fn serialize_u64(self, v: u64) -> Result<Value> {
        Ok(self.env.make_big(false, v as u128)?)
    }

    fn serialize_u128(self, v: u128) -> Result<Value> {
        Ok(self.env.make_big(false, v)?)
    }

    fn serialize_f32(self, v: f32) -> Result<Value> {
        self.serialize_f64(v as f64)
    }

    fn serialize_f64(self, v: f64) -> Result<Value> {
        Ok(self.env.make_float(v)?)
    }

    fn serialize_char(self, v: char) -> Result<Value> {
        self.serialize_i64(v as i64)
    }

    fn serialize_str(self, v: &str) -> Result<Value> {
        Ok(self.env.make_string(v)?)
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<Value> {
        Ok(self.env.make_unibyte(v)?)
    }

    fn serialize_none(self) -> Result<Value> {
        self.serialize_unit()
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<Value> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Value> {
        Ok(self.env.nil()?)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Value> {
        self.serialize_unit()
    }

    fn serialize_unit_variant(self, _name: &'static str, _index: u32, variant: &'static str) -> Result<Value> {
        ListSer::variant(self.env, variant, 0)?.finish()
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _name: &'static str, value: &T) -> Result<Value> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Value> {
        let mut list = ListSer::variant(self.env, variant, 1)?;
        list.push(value)?;
        list.finish()
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<ListSer<'e>> {
        Ok(ListSer::new(self.env, len.unwrap_or(0)))
    }

    fn serialize_tuple(self, len: usize) -> Result<ListSer<'e>> {
        Ok(ListSer::new(self.env, len))
    }

    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<ListSer<'e>> {
        Ok(ListSer::new(self.env, len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<ListSer<'e>> {
        ListSer::variant(self.env, variant, len)
    }

    fn serialize_map(self, len: Option<usize>) -> Result<MapSer<'e>> {
        Ok(MapSer { env: self.env, pairs: Vec::with_capacity(len.unwrap_or(0)), key: None })
    }

    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<ListSer<'e>> {
        Ok(ListSer::new(self.env, len * 2))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<ListSer<'e>> {
        ListSer::variant(self.env, variant, len * 2)
    }
}

impl ser::SerializeSeq for ListSer<'_> {
    type Ok = Value;
    type Error = SerError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.push(value)
    }

    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::SerializeTuple for ListSer<'_> {
    type Ok = Value;
    type Error = SerError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.push(value)
    }

    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::SerializeTupleStruct for ListSer<'_> {
    type Ok = Value;
    type Error = SerError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.push(value)
    }

    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::SerializeTupleVariant for ListSer<'_> {
    type Ok = Value;
    type Error = SerError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.push(value)
    }

    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::SerializeStruct for ListSer<'_> {
    type Ok = Value;
    type Error = SerError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        self.field(key, value)
    }

    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::SerializeStructVariant for ListSer<'_> {
    type Ok = Value;
    type Error = SerError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        self.field(key, value)
    }

    fn end(self) -> Result<Value> {
        self.finish()
    }
}

impl ser::SerializeMap for MapSer<'_> {
    type Ok = Value;
    type Error = SerError;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<()> {
        self.key = Some(to_lisp(self.env, key)?);
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        let key = self.key.take().expect("serialize_value called before serialize_key");
        let value = to_lisp(self.env, value)?;
        self.pairs.push(self.env.cons(key, value)?);
        Ok(())
    }

    fn end(self) -> Result<Value> {
        Ok(self.env.list(&self.pairs)?)
    }
}

#[cfg(test)]
mod tests {
    use super::to_lisp;
    use crate::ffi::fake::Fake;
    use serde::Serialize;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn ser<T: Serialize>(value: &T) -> String {
        let mut fake = Fake::new();
        let env = fake.env();
        let lisp = to_lisp(&env, value).expect("serialization failed");
        fake.print(lisp)
    }

    #[derive(Serialize)]
    struct Match {
        path: PathBuf,
        line_number: u64,
        preview: Option<String>,
    }

    #[derive(Serialize)]
    struct Unit;

    #[derive(Serialize)]
    struct Meters(f64);

    #[derive(Serialize)]
    struct Pair(i32, String);

    #[derive(Serialize)]
    enum Event {
        Ready,
        Result(String),
        Pair(i32, i32),
        SearchStarted { id: u64, case_fold: bool },
        Found(Match),
    }

    #[test]
    fn scalars() {
        assert_eq!(ser(&true), "t");
        assert_eq!(ser(&false), "nil");
        assert_eq!(ser(&-5i8), "-5");
        assert_eq!(ser(&70000u32), "70000");
        assert_eq!(ser(&1.5f64), "1.5");
        assert_eq!(ser(&'a'), "97");
        assert_eq!(ser(&"hé \"x\""), "\"hé \\\"x\\\"\"");
        assert_eq!(ser(&()), "nil");
    }

    #[test]
    fn large_integers_become_bignums() {
        assert_eq!(ser(&u64::MAX), "18446744073709551615");
        assert_eq!(ser(&i64::MIN), "-9223372036854775808");
        assert_eq!(ser(&u128::MAX), "340282366920938463463374607431768211455");
        assert_eq!(ser(&i128::MIN), "-170141183460469231731687303715884105728");
    }

    #[test]
    fn options_and_unit_types() {
        assert_eq!(ser(&None::<i32>), "nil");
        assert_eq!(ser(&Some(3)), "3");
        assert_eq!(ser(&Unit), "nil");
        assert_eq!(ser(&Meters(2.5)), "2.5");
    }

    #[test]
    fn sequences_and_tuples_are_lists() {
        assert_eq!(ser(&vec![1, 2, 3]), "(1 2 3)");
        assert_eq!(ser(&Vec::<i32>::new()), "nil");
        assert_eq!(ser(&(1, "a")), "(1 \"a\")");
        assert_eq!(ser(&Pair(1, "a".into())), "(1 \"a\")");
    }

    #[test]
    fn bytes_need_serde_bytes_style_serialization() {
        struct Raw(Vec<u8>);
        impl Serialize for Raw {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_bytes(&self.0)
            }
        }
        assert_eq!(ser(&Raw(vec![0, 200, 255])), "#b(0 200 255)");
        assert_eq!(ser(&vec![0u8, 200]), "(0 200)", "plain Vec<u8> is a list");
    }

    #[test]
    fn maps_are_alists() {
        let mut map = BTreeMap::new();
        map.insert("a".to_string(), 1);
        map.insert("b".to_string(), 2);
        assert_eq!(ser(&map), "((\"a\" . 1) (\"b\" . 2))");
        assert_eq!(ser(&BTreeMap::<String, i32>::new()), "nil");
    }

    #[test]
    fn structs_are_plists_with_kebab_keywords() {
        let m = Match { path: "/a.rs".into(), line_number: 42, preview: None };
        assert_eq!(ser(&m), "(:path \"/a.rs\" :line-number 42 :preview nil)");
    }

    #[test]
    fn enum_variants_are_keyword_headed_lists() {
        assert_eq!(ser(&Event::Ready), "(:ready)");
        assert_eq!(ser(&Event::Result("x".into())), "(:result \"x\")");
        assert_eq!(ser(&Event::Pair(1, 2)), "(:pair 1 2)");
        assert_eq!(ser(&Event::SearchStarted { id: 7, case_fold: true }), "(:search-started :id 7 :case-fold t)");
        let m = Match { path: "/a".into(), line_number: 1, preview: Some("p".into()) };
        assert_eq!(ser(&Event::Found(m)), "(:found (:path \"/a\" :line-number 1 :preview \"p\"))");
    }

    #[test]
    fn custom_serialize_errors_are_reported() {
        struct Bad;
        impl Serialize for Bad {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("cannot encode"))
            }
        }
        let mut fake = Fake::new();
        let env = fake.env();
        let err = to_lisp(&env, &vec![Bad]).unwrap_err();
        assert_eq!(err.to_string(), "cannot encode");
    }
}
