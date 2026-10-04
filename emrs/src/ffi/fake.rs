//! A minimal in-process stand-in for Emacs, for unit tests.
//!
//! `Fake` owns a tiny Lisp heap and an `emacs_env` whose function pointers
//! operate on it, so the serializer and deserializer run under `cargo test`
//! without Emacs. `read` and `print` use Lisp syntax plus two extensions:
//! `#b(1 2 255)` is a unibyte string and `#h(k v ...)` is a hash table.

use super::env::{Env, Value};
use super::sys::{self, emacs_env, emacs_value};
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::{c_char, c_int, c_void};

#[derive(Clone, Debug, PartialEq)]
enum Obj {
    Sym(String),
    Int { neg: bool, mag: u128 },
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Cons(usize, usize),
    Vector(Vec<usize>),
    Hash(Vec<(usize, usize)>),
}

#[derive(Default)]
struct Heap {
    objs: Vec<Obj>,
    symbols: HashMap<String, usize>,
    pending: Option<(usize, usize)>,
}

impl Heap {
    fn alloc(&mut self, obj: Obj) -> usize {
        self.objs.push(obj);
        self.objs.len() - 1
    }

    fn sym(&mut self, name: &str) -> usize {
        if let Some(&i) = self.symbols.get(name) {
            return i;
        }
        let i = self.alloc(Obj::Sym(name.to_string()));
        self.symbols.insert(name.to_string(), i);
        i
    }

    fn int(&mut self, n: i128) -> usize {
        self.alloc(Obj::Int { neg: n < 0, mag: n.unsigned_abs() })
    }

    fn is_nil(&self, i: usize) -> bool {
        self.objs[i] == Obj::Sym("nil".into())
    }

    fn list(&mut self, items: &[usize], tail: Option<usize>) -> usize {
        let mut list = tail.unwrap_or_else(|| self.sym("nil"));
        for &item in items.iter().rev() {
            list = self.alloc(Obj::Cons(item, list));
        }
        list
    }

    fn signal(&mut self, symbol: &str, data: usize) {
        let symbol = self.sym(symbol);
        self.pending = Some((symbol, data));
    }

    fn print(&self, i: usize, out: &mut String) {
        match &self.objs[i] {
            Obj::Sym(name) => out.push_str(name),
            Obj::Int { neg, mag } => {
                if *neg {
                    out.push('-');
                }
                out.push_str(&mag.to_string());
            }
            Obj::Float(d) => out.push_str(&format!("{d:?}")),
            Obj::Str(s) => {
                out.push('"');
                for c in s.chars() {
                    if c == '"' || c == '\\' {
                        out.push('\\');
                    }
                    out.push(c);
                }
                out.push('"');
            }
            Obj::Bytes(bytes) => {
                let parts: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
                out.push_str(&format!("#b({})", parts.join(" ")));
            }
            Obj::Cons(..) => {
                out.push('(');
                let mut cur = i;
                let mut first = true;
                loop {
                    match &self.objs[cur] {
                        Obj::Cons(car, cdr) => {
                            if !first {
                                out.push(' ');
                            }
                            first = false;
                            self.print(*car, out);
                            cur = *cdr;
                        }
                        _ if self.is_nil(cur) => break,
                        _ => {
                            out.push_str(" . ");
                            self.print(cur, out);
                            break;
                        }
                    }
                }
                out.push(')');
            }
            Obj::Vector(items) => {
                out.push('[');
                for (n, item) in items.iter().enumerate() {
                    if n > 0 {
                        out.push(' ');
                    }
                    self.print(*item, out);
                }
                out.push(']');
            }
            Obj::Hash(pairs) => {
                out.push_str("#h(");
                for (n, (k, v)) in pairs.iter().enumerate() {
                    if n > 0 {
                        out.push(' ');
                    }
                    self.print(*k, out);
                    out.push(' ');
                    self.print(*v, out);
                }
                out.push(')');
            }
        }
    }
}

struct Reader<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    heap: &'a mut Heap,
}

impl Reader<'_> {
    fn skip_ws(&mut self) {
        while self.chars.peek().is_some_and(|c| c.is_whitespace()) {
            self.chars.next();
        }
    }

    fn items_until(&mut self, close: char) -> (Vec<usize>, Option<usize>) {
        let mut items = Vec::new();
        let mut tail = None;
        loop {
            self.skip_ws();
            match self.chars.peek() {
                Some(&c) if c == close => {
                    self.chars.next();
                    return (items, tail);
                }
                None => panic!("fake reader: missing {close}"),
                _ => {}
            }
            let item = self.read();
            if matches!(&self.heap.objs[item], Obj::Sym(s) if s == ".") {
                tail = Some(self.read());
            } else {
                items.push(item);
            }
        }
    }

    fn read(&mut self) -> usize {
        self.skip_ws();
        match self.chars.next().expect("fake reader: unexpected end") {
            '(' => {
                let (items, tail) = self.items_until(')');
                self.heap.list(&items, tail)
            }
            '[' => {
                let (items, _) = self.items_until(']');
                self.heap.alloc(Obj::Vector(items))
            }
            '"' => {
                let mut s = String::new();
                loop {
                    match self.chars.next().expect("fake reader: unterminated string") {
                        '"' => break,
                        '\\' => s.push(self.chars.next().unwrap()),
                        c => s.push(c),
                    }
                }
                self.heap.alloc(Obj::Str(s))
            }
            '#' => {
                let kind = self.chars.next().unwrap();
                assert_eq!(self.chars.next(), Some('('), "fake reader: expected ( after #{kind}");
                let (items, _) = self.items_until(')');
                match kind {
                    'b' => {
                        let bytes = items
                            .iter()
                            .map(|&i| match self.heap.objs[i] {
                                Obj::Int { mag, .. } => mag as u8,
                                _ => panic!("fake reader: #b takes integers"),
                            })
                            .collect();
                        self.heap.alloc(Obj::Bytes(bytes))
                    }
                    'h' => {
                        let pairs = items.chunks(2).map(|kv| (kv[0], kv[1])).collect();
                        self.heap.alloc(Obj::Hash(pairs))
                    }
                    other => panic!("fake reader: unknown #{other}"),
                }
            }
            '?' => {
                let c = self.chars.next().unwrap();
                self.heap.int(c as i128)
            }
            first => {
                let mut token = String::from(first);
                while let Some(&c) = self.chars.peek() {
                    if c.is_whitespace() || "()[]\"".contains(c) {
                        break;
                    }
                    token.push(c);
                    self.chars.next();
                }
                let digits = token.strip_prefix('-').unwrap_or(&token);
                if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                    let mag: u128 = digits.parse().expect("fake reader: integer too large");
                    self.heap.alloc(Obj::Int { neg: token.starts_with('-') && mag != 0, mag })
                } else if let (true, Ok(d)) = (digits.starts_with(|c: char| c.is_ascii_digit()), token.parse::<f64>()) {
                    self.heap.alloc(Obj::Float(d))
                } else {
                    self.heap.sym(&token)
                }
            }
        }
    }
}

/// A fake Emacs. Create one per test.
pub(crate) struct Fake {
    heap: Box<Heap>,
    env: Box<emacs_env>,
}

impl Fake {
    pub(crate) fn new() -> Fake {
        let mut heap = Box::new(Heap::default());
        heap.sym("nil");
        heap.sym("t");
        let env = Box::new(emacs_env {
            size: std::mem::size_of::<emacs_env>() as isize,
            private_members: &mut *heap as *mut Heap as *mut c_void,
            make_global_ref: None,
            free_global_ref: None,
            non_local_exit_check,
            non_local_exit_clear,
            non_local_exit_get: None,
            non_local_exit_signal,
            non_local_exit_throw: None,
            make_function,
            funcall,
            intern,
            type_of,
            is_not_nil,
            eq,
            extract_integer,
            make_integer,
            extract_float,
            make_float,
            copy_string_contents,
            make_string,
            make_user_ptr: None,
            get_user_ptr: None,
            set_user_ptr: None,
            get_user_finalizer: None,
            set_user_finalizer: None,
            vec_get,
            vec_set: None,
            vec_size,
            should_quit: None,
            process_input: None,
            extract_time: None,
            make_time: None,
            extract_big_integer,
            make_big_integer,
            get_function_finalizer: None,
            set_function_finalizer: None,
            open_channel,
            make_interactive: None,
            make_unibyte_string,
        });
        Fake { heap, env }
    }

    pub(crate) fn env(&mut self) -> Env {
        unsafe { Env::from_raw(&mut *self.env) }
    }

    /// Parse one Lisp expression into the fake heap.
    pub(crate) fn read(&mut self, text: &str) -> Value {
        let index = Reader { chars: text.chars().peekable(), heap: &mut self.heap }.read();
        Value(to_value(index))
    }

    /// Print a value in Lisp syntax.
    pub(crate) fn print(&self, value: Value) -> String {
        let mut out = String::new();
        self.heap.print(to_index(value.0), &mut out);
        out
    }

    /// The pending signal as `(SYMBOL . DATA)`, if any.
    pub(crate) fn pending(&self) -> Option<String> {
        self.heap.pending.map(|(symbol, data)| {
            let mut out = String::from("(");
            self.heap.print(symbol, &mut out);
            out.push_str(" . ");
            self.heap.print(data, &mut out);
            out.push(')');
            out
        })
    }
}

fn to_value(index: usize) -> emacs_value {
    (index + 1) as emacs_value
}

fn to_index(value: emacs_value) -> usize {
    value as usize - 1
}

unsafe fn heap<'a>(env: *mut emacs_env) -> &'a mut Heap {
    unsafe { &mut *((*env).private_members as *mut Heap) }
}

unsafe extern "C" fn non_local_exit_check(env: *mut emacs_env) -> c_int {
    unsafe { heap(env).pending.is_some() as c_int }
}

unsafe extern "C" fn non_local_exit_clear(env: *mut emacs_env) {
    unsafe { heap(env).pending = None }
}

unsafe extern "C" fn non_local_exit_signal(env: *mut emacs_env, symbol: emacs_value, data: emacs_value) {
    unsafe { heap(env).pending = Some((to_index(symbol), to_index(data))) }
}

unsafe extern "C" fn make_function(
    env: *mut emacs_env,
    _min: isize,
    _max: isize,
    _func: sys::emacs_function,
    _doc: *const c_char,
    _data: *mut c_void,
) -> emacs_value {
    unsafe { to_value(heap(env).sym("fake-function")) }
}

unsafe extern "C" fn funcall(
    env: *mut emacs_env,
    func: emacs_value,
    nargs: isize,
    args: *mut emacs_value,
) -> emacs_value {
    let h = unsafe { heap(env) };
    let args: Vec<usize> = (0..nargs as usize).map(|i| to_index(unsafe { *args.add(i) })).collect();
    let name = match &h.objs[to_index(func)] {
        Obj::Sym(name) => name.clone(),
        _ => String::new(),
    };
    let nil = h.sym("nil");
    let result = match name.as_str() {
        "list" => h.list(&args, None),
        "vector" => h.alloc(Obj::Vector(args)),
        "cons" => h.alloc(Obj::Cons(args[0], args[1])),
        "car" | "cdr" => match h.objs[args[0]] {
            Obj::Cons(car, cdr) => {
                if name == "car" {
                    car
                } else {
                    cdr
                }
            }
            _ if h.is_nil(args[0]) => nil,
            _ => {
                h.signal("wrong-type-argument", args[0]);
                nil
            }
        },
        "proper-list-p" => {
            let mut cur = args[0];
            let mut len = 0;
            while let Obj::Cons(_, cdr) = h.objs[cur] {
                cur = cdr;
                len += 1;
            }
            if h.is_nil(cur) { h.int(len) } else { nil }
        }
        "symbol-name" => match h.objs[args[0]].clone() {
            Obj::Sym(s) => h.alloc(Obj::Str(s)),
            _ => {
                h.signal("wrong-type-argument", args[0]);
                nil
            }
        },
        "emrs--hash-to-alist" => match h.objs[args[0]].clone() {
            Obj::Hash(pairs) => {
                let conses: Vec<usize> = pairs.iter().map(|&(k, v)| h.alloc(Obj::Cons(k, v))).collect();
                h.list(&conses, None)
            }
            _ => {
                h.signal("wrong-type-argument", args[0]);
                nil
            }
        },
        _ => {
            h.signal("void-function", to_index(func));
            nil
        }
    };
    to_value(result)
}

unsafe extern "C" fn intern(env: *mut emacs_env, name: *const c_char) -> emacs_value {
    let name = unsafe { CStr::from_ptr(name) }.to_str().unwrap();
    unsafe { to_value(heap(env).sym(name)) }
}

unsafe extern "C" fn type_of(env: *mut emacs_env, arg: emacs_value) -> emacs_value {
    let h = unsafe { heap(env) };
    let name = match &h.objs[to_index(arg)] {
        Obj::Sym(_) => "symbol",
        Obj::Int { .. } => "integer",
        Obj::Float(_) => "float",
        Obj::Str(_) | Obj::Bytes(_) => "string",
        Obj::Cons(..) => "cons",
        Obj::Vector(_) => "vector",
        Obj::Hash(_) => "hash-table",
    };
    to_value(h.sym(name))
}

unsafe extern "C" fn is_not_nil(env: *mut emacs_env, arg: emacs_value) -> bool {
    unsafe { !heap(env).is_nil(to_index(arg)) }
}

unsafe extern "C" fn eq(env: *mut emacs_env, a: emacs_value, b: emacs_value) -> bool {
    let h = unsafe { heap(env) };
    let (a, b) = (to_index(a), to_index(b));
    a == b || matches!((&h.objs[a], &h.objs[b]), (Obj::Int { .. }, Obj::Int { .. }) if h.objs[a] == h.objs[b])
}

unsafe extern "C" fn extract_integer(env: *mut emacs_env, arg: emacs_value) -> i64 {
    let h = unsafe { heap(env) };
    match h.objs[to_index(arg)] {
        Obj::Int { neg, mag } if mag <= i64::MAX as u128 => {
            if neg {
                -(mag as i64)
            } else {
                mag as i64
            }
        }
        Obj::Int { .. } => {
            h.signal("overflow-error", to_index(arg));
            0
        }
        _ => {
            h.signal("wrong-type-argument", to_index(arg));
            0
        }
    }
}

unsafe extern "C" fn make_integer(env: *mut emacs_env, n: i64) -> emacs_value {
    unsafe { to_value(heap(env).int(n as i128)) }
}

unsafe extern "C" fn extract_float(env: *mut emacs_env, arg: emacs_value) -> f64 {
    let h = unsafe { heap(env) };
    match h.objs[to_index(arg)] {
        Obj::Float(d) => d,
        _ => {
            h.signal("wrong-type-argument", to_index(arg));
            0.0
        }
    }
}

unsafe extern "C" fn make_float(env: *mut emacs_env, d: f64) -> emacs_value {
    unsafe { to_value(heap(env).alloc(Obj::Float(d))) }
}

unsafe extern "C" fn copy_string_contents(
    env: *mut emacs_env,
    value: emacs_value,
    buf: *mut c_char,
    len: *mut isize,
) -> bool {
    let h = unsafe { heap(env) };
    let bytes = match &h.objs[to_index(value)] {
        Obj::Str(s) => s.as_bytes().to_vec(),
        Obj::Bytes(b) => b.clone(),
        _ => {
            h.signal("wrong-type-argument", to_index(value));
            return false;
        }
    };
    let needed = bytes.len() as isize + 1;
    unsafe {
        if buf.is_null() {
            *len = needed;
            return true;
        }
        if *len < needed {
            *len = needed;
            h.signal("args-out-of-range", to_index(value));
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, bytes.len());
        *buf.add(bytes.len()) = 0;
        *len = needed;
    }
    true
}

unsafe extern "C" fn make_string(env: *mut emacs_env, s: *const c_char, len: isize) -> emacs_value {
    let bytes = unsafe { std::slice::from_raw_parts(s as *const u8, len as usize) };
    let s = String::from_utf8(bytes.to_vec()).unwrap();
    unsafe { to_value(heap(env).alloc(Obj::Str(s))) }
}

unsafe extern "C" fn make_unibyte_string(env: *mut emacs_env, s: *const c_char, len: isize) -> emacs_value {
    let bytes = unsafe { std::slice::from_raw_parts(s as *const u8, len as usize) };
    unsafe { to_value(heap(env).alloc(Obj::Bytes(bytes.to_vec()))) }
}

unsafe extern "C" fn vec_get(env: *mut emacs_env, vector: emacs_value, index: isize) -> emacs_value {
    let h = unsafe { heap(env) };
    match &h.objs[to_index(vector)] {
        Obj::Vector(items) if (index as usize) < items.len() => to_value(items[index as usize]),
        _ => {
            h.signal("args-out-of-range", to_index(vector));
            to_value(h.sym("nil"))
        }
    }
}

unsafe extern "C" fn vec_size(env: *mut emacs_env, vector: emacs_value) -> isize {
    let h = unsafe { heap(env) };
    match &h.objs[to_index(vector)] {
        Obj::Vector(items) => items.len() as isize,
        _ => {
            h.signal("wrong-type-argument", to_index(vector));
            0
        }
    }
}

unsafe extern "C" fn extract_big_integer(
    env: *mut emacs_env,
    arg: emacs_value,
    sign: *mut c_int,
    count: *mut isize,
    magnitude: *mut sys::emacs_limb_t,
) -> bool {
    let h = unsafe { heap(env) };
    let Obj::Int { neg, mag } = h.objs[to_index(arg)] else {
        h.signal("wrong-type-argument", to_index(arg));
        return false;
    };
    let limb_bits = 8 * std::mem::size_of::<sys::emacs_limb_t>();
    unsafe {
        *sign = if mag == 0 {
            0
        } else if neg {
            -1
        } else {
            1
        };
        if mag == 0 {
            return true;
        }
        let needed = (128 - mag.leading_zeros() as usize).div_ceil(limb_bits) as isize;
        if magnitude.is_null() {
            *count = needed;
            return true;
        }
        if *count < needed {
            h.signal("args-out-of-range", to_index(arg));
            return false;
        }
        for i in 0..needed as usize {
            *magnitude.add(i) = (mag >> (i * limb_bits)) as sys::emacs_limb_t;
        }
        *count = needed;
    }
    true
}

unsafe extern "C" fn make_big_integer(
    env: *mut emacs_env,
    sign: c_int,
    count: isize,
    magnitude: *const sys::emacs_limb_t,
) -> emacs_value {
    let limb_bits = 8 * std::mem::size_of::<sys::emacs_limb_t>();
    let mut mag: u128 = 0;
    for i in 0..count as usize {
        mag |= (unsafe { *magnitude.add(i) } as u128) << (i * limb_bits);
    }
    unsafe { to_value(heap(env).alloc(Obj::Int { neg: sign < 0 && mag != 0, mag })) }
}

unsafe extern "C" fn open_channel(_env: *mut emacs_env, _pipe: emacs_value) -> c_int {
    -1
}

#[cfg(test)]
mod tests {
    use super::Fake;

    #[test]
    fn read_print_round_trip() {
        let mut fake = Fake::new();
        for text in [
            "nil",
            "42",
            "-7",
            "1.5",
            "\"a \\\"q\\\" b\"",
            ":key",
            "(1 2 3)",
            "(a . b)",
            "(:x 1 :y (2 3))",
            "[1 \"two\" three]",
            "#b(0 127 255)",
            "#h(\"a\" 1 \"b\" 2)",
        ] {
            let value = fake.read(text);
            assert_eq!(fake.print(value), text);
        }
    }

    #[test]
    fn env_calls_work_against_the_fake() {
        let mut fake = Fake::new();
        let env = fake.env();
        let one = env.make_i64(1).unwrap();
        let s = env.make_string("hé").unwrap();
        let list = env.list(&[one, s]).unwrap();
        assert_eq!(env.type_name(list).unwrap(), "cons");
        assert_eq!(env.extract_i64(env.car(list).unwrap()).unwrap(), 1);
        assert_eq!(env.string(env.car(env.cdr(list).unwrap()).unwrap()).unwrap(), "hé");
        let big = env.make_big(true, u64::MAX as u128 + 5).unwrap();
        assert_eq!(env.extract_big(big).unwrap(), Some((true, u64::MAX as u128 + 5)));
        assert_eq!(fake.print(list), "(1 \"hé\")");
    }

    #[test]
    fn errors_become_pending_exits() {
        let mut fake = Fake::new();
        let env = fake.env();
        let s = env.make_string("x").unwrap();
        assert!(env.extract_i64(s).is_err());
        assert!(env.intern("still-pending").is_err(), "calls fail while an exit is pending");
        env.clear_exit();
        assert!(env.intern("ok").is_ok());
        assert!(fake.pending().is_none());
    }
}
