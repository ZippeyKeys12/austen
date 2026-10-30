use crate::gen_dsl::{BaseType, Id, Sign, Sym};

pub fn sym_name(sym: &Sym) -> String {
    match &sym.name {
        Some(name) => format!("{name}__sym_{}", sym.num),
        None => format!("anon__sym_{}", sym.num),
    }
}

pub fn binder_name(sym: &Sym, n: u64) -> String {
    format!("{}__q{n}", sym_name(sym))
}

pub fn fn_name(sym: &Sym) -> String {
    format!("{}__func", sym_name(sym))
}

pub fn struct_sort_name(tag: &Sym) -> String {
    format!("struct__{}", sym_name(tag))
}

pub fn struct_con_name(tag: &Sym) -> String {
    format!("mk_{}", struct_sort_name(tag))
}

pub fn struct_field_name(tag: &Sym, member: &Id) -> String {
    format!("{}__{}", struct_sort_name(tag), member.0)
}

pub fn datatype_sort_name(tag: &Sym) -> String {
    format!("datatype__{}", sym_name(tag))
}

pub fn datatype_con_name(tag: &Sym, constructor: &Sym) -> String {
    format!(
        "{}__constructor__{}",
        datatype_sort_name(tag),
        sym_name(constructor)
    )
}

pub fn datatype_field_name(tag: &Sym, constructor: &Sym, member: &Id) -> String {
    format!(
        "{}__field__{}",
        datatype_con_name(tag, constructor),
        member.0
    )
}

pub fn tuple_sort_name(components: &[BaseType]) -> String {
    let mut s = String::from("tuple");
    for bt in components {
        s.push('_');
        s.push_str(&type_key(bt));
    }
    s
}

pub fn tuple_field_name(index: usize) -> String {
    format!("get_{index}")
}

pub fn list_sort_name(elem: &BaseType) -> String {
    format!("cn_list__{}", type_key(elem))
}

pub fn option_sort_name(elem: &BaseType) -> String {
    format!("cn_option__{}", type_key(elem))
}

pub fn type_key(bt: &BaseType) -> String {
    let mut out = String::new();
    write_key(bt, &mut out);
    out
}

fn write_key(bt: &BaseType, out: &mut String) {
    use std::fmt::Write as _;
    match bt {
        BaseType::Unit => out.push_str("unit"),
        BaseType::Bool => out.push_str("bool"),
        BaseType::Integer => out.push_str("int"),
        BaseType::Real => out.push_str("real"),
        BaseType::MemByte => out.push_str("membyte"),
        BaseType::AllocId => out.push_str("allocid"),
        BaseType::CType => out.push_str("ctype"),
        BaseType::Loc(_) => out.push_str("ptr"),
        BaseType::Bits(Sign::Signed, w) => {
            let _ = write!(out, "i{w}");
        }
        BaseType::Bits(Sign::Unsigned, w) => {
            let _ = write!(out, "u{w}");
        }
        BaseType::Struct(tag) => {
            let _ = write!(out, "struct[{}]", sym_name(tag));
        }
        BaseType::Datatype(tag) => {
            let _ = write!(out, "datatype[{}]", sym_name(tag));
        }
        BaseType::Map(k, v) => {
            out.push_str("map[");
            write_key(k, out);
            out.push(';');
            write_key(v, out);
            out.push(']');
        }
        BaseType::List(t) => wrap("list", t, out),
        BaseType::Set(t) => wrap("set", t, out),
        BaseType::Option(t) => wrap("opt", t, out),
        BaseType::Tuple(ts) => {
            out.push_str("tup[");
            for (i, t) in ts.iter().enumerate() {
                if i > 0 {
                    out.push(';');
                }
                write_key(t, out);
            }
            out.push(']');
        }
        BaseType::Record(ms) => {
            out.push_str("rec[");
            for (i, (id, t)) in ms.iter().enumerate() {
                if i > 0 {
                    out.push(';');
                }
                let _ = write!(out, "{}=", id.0);
                write_key(t, out);
            }
            out.push(']');
        }
    }
}

fn wrap(head: &str, inner: &BaseType, out: &mut String) {
    out.push_str(head);
    out.push('[');
    write_key(inner, out);
    out.push(']');
}
