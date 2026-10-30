use std::borrow::Cow;

use pretty::{Pretty, RcAllocator, RcDoc};

use crate::gen_dsl::choice::tree::Label;
use crate::gen_dsl::lang::{
    BaseType, BinOp, Const, GenDef, GenModule, GenTerm, GenTermKind, Id, IndexTerm,
    IntegerBaseType, IntegerType, LogicalConstraint, Pattern, PatternKind, ScopeName, Sctype, Sign,
    Sym, Term, UnOp, bits_residue,
};

pub const DEFAULT_DSL_WIDTH: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DslFormatter {
    width: usize,
}

impl Default for DslFormatter {
    fn default() -> Self {
        Self::with_width(DEFAULT_DSL_WIDTH)
    }
}

impl DslFormatter {
    pub const fn with_width(width: usize) -> Self {
        Self {
            width: if width == 0 { 1 } else { width },
        }
    }

    pub const fn width(self) -> usize {
        self.width
    }

    pub fn render_term(&self, term: &GenTerm) -> String {
        render(&self.term(term), self.width)
    }

    pub fn render_def(&self, def: &GenDef) -> String {
        render(&self.def(def), self.width)
    }

    pub fn render_defs<'a>(&self, defs: impl IntoIterator<Item = &'a GenDef>) -> String {
        let separator = Doc::concat(vec![Doc::text(";"), Doc::hardline(), Doc::hardline()]);
        let docs = defs.into_iter().map(|def| self.def(def));
        let mut out = render(&Doc::join(separator, docs), self.width);
        out.push('\n');
        out
    }

    pub fn render_ctx(&self, ctx: &GenModule) -> String {
        self.render_defs(ctx.defs.iter().map(|(_, def)| def))
    }

    fn def(&self, def: &GenDef) -> Doc {
        let args = fixed_comma_list(
            def.iargs
                .iter()
                .map(|(name, bt)| Doc::concat(vec![self.bt(bt), Doc::text(" "), self.sym(name)])),
        );
        Doc::group(Doc::concat(vec![
            Doc::text("generator "),
            self.bt(&def.oarg),
            Doc::text(" "),
            self.sym(&def.name),
            raw_parens(args),
            Doc::text(" "),
            block(self.term(&def.body)),
        ]))
    }

    fn term(&self, term: &GenTerm) -> Doc {
        match &term.term {
            GenTermKind::Arbitrary => Doc::concat(vec![
                Doc::text("arbitrary<"),
                self.bt(&term.bt),
                Doc::text(">()"),
            ]),
            GenTermKind::WriteChoice { label, value, rest } => self.then(
                Doc::concat(vec![
                    Doc::text("write_choice<"),
                    self.bt(&value.bt),
                    Doc::text(">("),
                    self.label(label),
                    Doc::text(", "),
                    self.index(value, 0),
                    Doc::text(");"),
                ]),
                rest,
            ),
            GenTermKind::Call { fsym, args } => Doc::concat(vec![
                self.sym(fsym),
                raw_parens(Doc::nest(
                    2,
                    comma_list(args.iter().map(|arg| self.index(arg, 0))),
                )),
            ]),
            GenTermKind::Asgn {
                addr,
                ct,
                value,
                rest,
            } => self.then(
                Doc::concat(vec![
                    self.sctype(ct),
                    Doc::text(" "),
                    self.index(addr, 0),
                    Doc::text(" := "),
                    self.index(value, 0),
                    Doc::text(";"),
                ]),
                rest,
            ),
            GenTermKind::LetStar { var, value, rest } => self.then(
                Doc::concat(vec![
                    Doc::text("let* "),
                    self.sym(var),
                    Doc::text(" : "),
                    self.bt(&value.bt),
                    Doc::text(" ="),
                    Doc::nest(2, Doc::concat(vec![Doc::line(), self.term(value)])),
                    Doc::text(";"),
                ]),
                rest,
            ),
            GenTermKind::Return(value) => {
                Doc::concat(vec![Doc::text("return "), self.index(value, 0)])
            }
            GenTermKind::Assert { constraint, rest } => self.then(
                Doc::concat(vec![
                    Doc::text("assert"),
                    raw_parens(Doc::concat(vec![
                        Doc::nest(
                            2,
                            Doc::concat(vec![Doc::line(), self.constraint(constraint)]),
                        ),
                        Doc::line(),
                    ])),
                    Doc::text(";"),
                ]),
                rest,
            ),
            GenTermKind::Ite { cond, then_, else_ } => Doc::concat(vec![
                Doc::text("if "),
                raw_parens(self.index(cond, 0)),
                Doc::text(" "),
                block(self.term(then_)),
                Doc::text(" else "),
                block(self.term(else_)),
            ]),
            GenTermKind::Map {
                var,
                var_bt,
                perm,
                body,
            } => Doc::concat(vec![
                Doc::text("map"),
                Doc::text(" "),
                raw_parens(Doc::concat(vec![
                    self.bt(var_bt),
                    Doc::text(" "),
                    self.sym(var),
                    Doc::text("; "),
                    self.index(perm, 0),
                ])),
                block(self.term(body)),
            ]),
            GenTermKind::Scope { name, body } => Doc::concat(vec![
                Doc::text("scope "),
                self.scope_name(name),
                Doc::text(" "),
                block(self.term(body)),
            ]),
        }
    }

    fn then(&self, first: Doc, rest: &GenTerm) -> Doc {
        Doc::concat(vec![first, Doc::line(), self.term(rest)])
    }

    fn constraint(&self, constraint: &LogicalConstraint) -> Doc {
        match constraint {
            LogicalConstraint::T(term) => self.index(term, 0),
            LogicalConstraint::Forall {
                var,
                permission,
                body,
            } => Doc::group(Doc::concat(vec![
                c_app(Doc::text("forall"), vec![self.sym(&var.0), self.bt(&var.1)]),
                Doc::text(". "),
                c_app(
                    Doc::text("implies"),
                    vec![self.index(permission, 0), self.index(body, 0)],
                ),
            ])),
        }
    }

    fn bt(&self, bt: &BaseType) -> Doc {
        match bt {
            BaseType::Unit => Doc::text("void"),
            BaseType::Bool => Doc::text("boolean"),
            BaseType::Integer => Doc::text("integer"),
            BaseType::MemByte => Doc::text("mem_byte"),
            BaseType::Bits(sign, width) => Doc::text(format!("{}{}", sign_prefix(*sign), width)),
            BaseType::Real => Doc::text("real"),
            BaseType::AllocId => Doc::text("alloc_id"),
            BaseType::Loc(None) => Doc::text("pointer"),
            BaseType::Loc(Some(ct)) => {
                Doc::concat(vec![Doc::text("pointer<"), self.sctype(ct), Doc::text(">")])
            }
            BaseType::CType => Doc::text("ctype"),
            BaseType::Struct(tag) => Doc::concat(vec![Doc::text("struct "), self.sym(tag)]),
            BaseType::Datatype(tag) => Doc::concat(vec![Doc::text("datatype "), self.sym(tag)]),
            BaseType::Record(members) => raw_braces(Doc::join(
                Doc::text(","),
                members.iter().map(|(id, member)| {
                    Doc::concat(vec![self.bt(member), Doc::text(" "), self.id(id)])
                }),
            )),
            BaseType::Map(key, value) => Doc::concat(vec![
                Doc::text("map"),
                Doc::text("<"),
                self.bt(key),
                Doc::text(", "),
                self.bt(value),
                Doc::text(">"),
            ]),
            BaseType::List(element) => Doc::concat(vec![
                Doc::text("cn_list<"),
                self.bt(element),
                Doc::text(">"),
            ]),
            BaseType::Tuple(elements) => Doc::concat(vec![
                Doc::text("cn_tuple<"),
                underscore_list(elements.iter().map(|element| self.bt(element))),
                Doc::text(">"),
            ]),
            BaseType::Set(element) => {
                Doc::concat(vec![Doc::text("cn_set<"), self.bt(element), Doc::text(">")])
            }
            BaseType::Option(element) => Doc::concat(vec![
                Doc::text("cn_option<"),
                self.bt(element),
                Doc::text(">"),
            ]),
        }
    }

    fn sctype(&self, ct: &Sctype) -> Doc {
        match ct {
            Sctype::Void => Doc::text("void"),
            Sctype::Integer(integer) => self.integer_type(integer),
            Sctype::Array(element, length) => Doc::concat(vec![
                self.sctype(element),
                raw_brackets(match length {
                    Some(length) => Doc::text(length.to_string()),
                    None => Doc::nil(),
                }),
            ]),
            Sctype::Pointer(target) => Doc::concat(vec![self.sctype(target), Doc::text("*")]),
            Sctype::Struct(tag) => Doc::concat(vec![Doc::text("struct "), self.sym(tag)]),
            Sctype::Function {
                return_type,
                args,
                variadic,
            } => {
                let rendered_args: Vec<Doc> = args
                    .iter()
                    .map(|(arg, _register)| self.sctype(arg))
                    .collect();
                let args = if *variadic {
                    match rendered_args.is_empty() {
                        true => Doc::text(", ..."),
                        false => {
                            Doc::concat(vec![fixed_comma_list(rendered_args), Doc::text(", ...")])
                        }
                    }
                } else {
                    fixed_comma_list(rendered_args)
                };
                Doc::concat(vec![
                    self.sctype(&return_type.1),
                    Doc::text(" "),
                    raw_parens(args),
                ])
            }
            Sctype::Byte => Doc::text("byte"),
        }
    }

    fn integer_type(&self, integer: &IntegerType) -> Doc {
        match integer {
            IntegerType::Char => Doc::text("char"),
            IntegerType::Bool => Doc::text("_Bool"),
            IntegerType::Signed(base) => match spelled_as_typedef(base) {
                true => self.integer_base(base),
                false => Doc::concat(vec![Doc::text("signed "), self.integer_base(base)]),
            },
            IntegerType::Unsigned(base) => match spelled_as_typedef(base) {
                true => Doc::concat(vec![Doc::text("u"), self.integer_base(base)]),
                false => Doc::concat(vec![Doc::text("unsigned "), self.integer_base(base)]),
            },
            IntegerType::Enum(tag) => Doc::concat(vec![Doc::text("enum "), self.sym(tag)]),
            IntegerType::WcharT => Doc::text("wchar_t"),
            IntegerType::WintT => Doc::text("wint_t"),
            IntegerType::SizeT => Doc::text("size_t"),
            IntegerType::PtrdiffT => Doc::text("ptrdiff_t"),
            IntegerType::PtraddrT => Doc::text("ptraddr_t"),
        }
    }

    fn integer_base(&self, base: &IntegerBaseType) -> Doc {
        match base {
            IntegerBaseType::Ichar => Doc::text("ichar"),
            IntegerBaseType::Short => Doc::text("short"),
            IntegerBaseType::Int => Doc::text("int"),
            IntegerBaseType::Long => Doc::text("long"),
            IntegerBaseType::LongLong => Doc::text("long_long"),
            IntegerBaseType::IntN(width) => Doc::text(format!("int{width}_t")),
            IntegerBaseType::IntLeastN(width) => Doc::text(format!("int_least{width}_t")),
            IntegerBaseType::IntFastN(width) => Doc::text(format!("int_fast{width}_t")),
            IntegerBaseType::Intmax => Doc::text("intmax_t"),
            IntegerBaseType::Intptr => Doc::text("intptr_t"),
        }
    }

    fn index(&self, index: &IndexTerm, parent_prec: u8) -> Doc {
        match &index.term {
            Term::Const(constant) => self.constant(constant),
            Term::Sym(symbol) => self.sym(symbol),
            Term::Unop(op, operand) => self.unop(*op, operand, parent_prec),
            Term::Binop(op, left, right) => self.binop(*op, left, right, parent_prec),
            Term::Ite(condition, then_, else_) => self.wrap_prec(
                parent_prec,
                2,
                Doc::group(flow(
                    Doc::line(),
                    vec![
                        self.index(condition, 3),
                        Doc::text("?"),
                        self.index(then_, 3),
                        Doc::text(":"),
                        self.index(else_, 3),
                    ],
                )),
            ),
            Term::EachI {
                start,
                var,
                end,
                body,
            } => Doc::group(Doc::concat(vec![
                c_app(
                    Doc::text("for"),
                    vec![
                        Doc::text(start.to_string()),
                        self.sym(&var.0),
                        Doc::text(end.to_string()),
                    ],
                ),
                Doc::line(),
                Doc::group(Doc::concat(vec![
                    Doc::nest(
                        2,
                        Doc::concat(vec![Doc::text("{"), Doc::line0(), self.index(body, 0)]),
                    ),
                    Doc::line0(),
                    Doc::text("}"),
                ])),
            ])),
            Term::Tuple(items) => braces(fixed_semicolon_list(
                items.iter().map(|item| self.index(item, 0)),
            )),
            Term::NthTuple(n, value) => self.wrap_prec(
                parent_prec,
                15,
                Doc::concat(vec![
                    self.index(value, 15),
                    Doc::text(format!(".member{n}")),
                ]),
            ),
            Term::Struct(_, fields) => self.struct_value(fields),
            Term::StructMember(value, member) | Term::RecordMember(value, member) => self
                .wrap_prec(
                    parent_prec,
                    15,
                    Doc::concat(vec![self.index(value, 15), Doc::text("."), self.id(member)]),
                ),
            Term::StructUpdate {
                target,
                member,
                value,
            }
            | Term::RecordUpdate {
                target,
                member,
                value,
            } => braces(fixed_comma_list(vec![
                Doc::concat(vec![
                    Doc::text("."),
                    self.id(member),
                    Doc::text(": "),
                    self.index(value, 0),
                ]),
                Doc::concat(vec![Doc::text(".."), self.index(target, 0)]),
            ])),
            Term::Record(fields) => self.record_value(fields),
            Term::Constructor(name, fields) => Doc::concat(vec![
                self.sym(name),
                Doc::text(" "),
                braces(fixed_comma_list(fields.iter().map(|(id, value)| {
                    Doc::concat(vec![self.id(id), Doc::text(": "), self.index(value, 0)])
                }))),
            ]),
            Term::MemberShift(base, _, member) => self.wrap_prec(
                parent_prec,
                14,
                Doc::concat(vec![
                    Doc::text("&"),
                    self.index(base, 15),
                    Doc::text("->"),
                    self.id(member),
                ]),
            ),
            Term::ArrayShift { base, ct: _, index } => self.wrap_prec(
                parent_prec,
                14,
                Doc::concat(vec![
                    Doc::text("&"),
                    self.index(base, 15),
                    raw_brackets(self.index(index, 0)),
                ]),
            ),
            Term::CopyAllocId { addr, loc } => self.call(
                "copy_alloc_id",
                vec![self.index(addr, 0), self.index(loc, 0)],
            ),
            Term::HasAllocId(value) => self.call("has_alloc_id", vec![self.index(value, 0)]),
            Term::SizeOf(ct) => self.call("sizeof", vec![self.sctype(ct)]),
            Term::OffsetOf(tag, member) => {
                self.call("offsetof", vec![self.sym(tag), self.id(member)])
            }
            Term::Nil(bt) => Doc::concat(vec![Doc::text("nil<"), self.bt(bt), Doc::text(">")]),
            Term::Cons(head, tail) => {
                self.call("cons", vec![self.index(head, 0), self.index(tail, 0)])
            }
            Term::Head(value) => self.call("hd", vec![self.index(value, 0)]),
            Term::Tail(value) => self.call("tl", vec![self.index(value, 0)]),
            Term::Representable(ct, value) => {
                self.generic_call("repr", self.sctype(ct), vec![self.index(value, 0)])
            }
            Term::Good(ct, value) => {
                self.generic_call("good", self.sctype(ct), vec![self.index(value, 0)])
            }
            Term::Aligned { t, align } => {
                self.call("aligned", vec![self.index(t, 0), self.index(align, 0)])
            }
            Term::WrapI(integer, value) => self.generic_call(
                "wrapI",
                self.integer_type(integer),
                vec![self.index(value, 0)],
            ),
            Term::MapConst(_, value) => self.call("const", vec![self.index(value, 0)]),
            Term::MapSet(map, key, value) => self.wrap_prec(
                parent_prec,
                15,
                Doc::concat(vec![
                    self.index(map, 15),
                    raw_brackets(Doc::concat(vec![
                        self.index(key, 0),
                        Doc::text(" = "),
                        self.index(value, 0),
                    ])),
                ]),
            ),
            Term::MapGet(map, key) => self.wrap_prec(
                parent_prec,
                15,
                Doc::concat(vec![self.index(map, 15), raw_brackets(self.index(key, 0))]),
            ),
            Term::MapDef((var, _), body) => raw_brackets(Doc::concat(vec![
                self.sym(var),
                Doc::text(" -> "),
                self.index(body, 0),
            ])),
            Term::Apply(name, args) => {
                c_app(self.sym(name), args.iter().map(|arg| self.index(arg, 0)))
            }
            Term::Let((name, value), body) => parens(Doc::group(Doc::concat(vec![
                Doc::text("let "),
                self.sym(name),
                Doc::text(" = "),
                self.index(value, 0),
                Doc::text(" in "),
                self.index(body, 0),
            ]))),
            Term::Match(value, cases) => {
                let cases: Vec<Doc> = cases
                    .iter()
                    .map(|(pattern, body)| {
                        Doc::concat(vec![
                            self.pattern(pattern),
                            Doc::text(" => "),
                            braces(self.index(body, 0)),
                        ])
                    })
                    .collect();
                Doc::concat(vec![
                    Doc::text("match "),
                    self.index(value, 0),
                    Doc::text(" "),
                    braces(Doc::group(Doc::nest(2, Doc::join(Doc::line0(), cases)))),
                ])
            }
            Term::Cast(bt, value) => self.wrap_prec(
                parent_prec,
                14,
                Doc::concat(vec![Doc::align(parens(self.bt(bt))), self.index(value, 14)]),
            ),
            Term::CnNone(bt) => self.call("None", vec![self.bt(bt)]),
            Term::CnSome(value) => self.call("Some", vec![self.index(value, 0)]),
            Term::IsSome(value) => self.call("is_some", vec![self.index(value, 0)]),
            Term::GetOpt(value) => self.call("get_opt", vec![self.index(value, 0)]),
        }
    }

    fn unop(&self, op: UnOp, value: &IndexTerm, parent_prec: u8) -> Doc {
        match op {
            UnOp::Not => match &value.term {
                Term::Binop(BinOp::Eq, left, right) => {
                    self.infix(parent_prec, (9, "!=", 9, 9), left, right)
                }
                Term::Binop(BinOp::Lt | BinOp::LtPointer, left, right) => {
                    self.infix(parent_prec, (10, ">=", 10, 10), left, right)
                }
                Term::Binop(BinOp::Le | BinOp::LePointer, left, right) => {
                    self.infix(parent_prec, (10, ">", 10, 10), left, right)
                }
                _ => self.wrap_prec(
                    parent_prec,
                    14,
                    Doc::concat(vec![Doc::text("!"), self.index(value, 14)]),
                ),
            },
            UnOp::Negate => self.wrap_prec(
                parent_prec,
                14,
                Doc::concat(vec![Doc::text("-"), self.index(value, 14)]),
            ),
            UnOp::BwCompl => self.wrap_prec(
                parent_prec,
                14,
                Doc::concat(vec![Doc::text("~"), self.index(value, 14)]),
            ),
            UnOp::BwClz => self.call("bw_clz_uf", vec![self.index(value, 0)]),
            UnOp::BwCtz => self.call("bw_ctz_uf", vec![self.index(value, 0)]),
            UnOp::BwFfs => self.call("bw_ffs_uf", vec![self.index(value, 0)]),
            UnOp::BwFls => self.call("bw_fls_uf", vec![self.index(value, 0)]),
        }
    }

    fn binop(&self, op: BinOp, left: &IndexTerm, right: &IndexTerm, parent_prec: u8) -> Doc {
        match op {
            BinOp::Implies => self.call("implies", vec![self.index(left, 0), self.index(right, 0)]),
            BinOp::Exp => self.call("power", vec![self.index(left, 0), self.index(right, 0)]),
            BinOp::Mod => self.call("mod", vec![self.index(left, 0), self.index(right, 0)]),
            BinOp::Min => self.call("min", vec![self.index(left, 0), self.index(right, 0)]),
            BinOp::Max => self.call("max", vec![self.index(left, 0), self.index(right, 0)]),
            BinOp::SetUnion => self.call("union", vec![self.index(left, 0), self.index(right, 0)]),
            BinOp::SetIntersection => {
                self.call("inter", vec![self.index(left, 0), self.index(right, 0)])
            }
            BinOp::SetDifference => self.call(
                "difference",
                vec![self.index(left, 0), self.index(right, 0)],
            ),
            BinOp::SetMember => {
                self.call("member", vec![self.index(left, 0), self.index(right, 0)])
            }
            BinOp::Subset => self.call("subset", vec![self.index(left, 0), self.index(right, 0)]),
            BinOp::And => self.infix(parent_prec, (5, "&&", 5, 5), left, right),
            BinOp::Or => self.infix(parent_prec, (4, "||", 4, 4), left, right),
            BinOp::Add => self.infix(parent_prec, (12, "+", 12, 12), left, right),
            BinOp::Sub => self.infix(parent_prec, (12, "-", 12, 13), left, right),
            BinOp::Mul => self.infix(parent_prec, (13, "*", 13, 13), left, right),
            BinOp::Div => self.infix(parent_prec, (13, "/", 14, 14), left, right),
            BinOp::Rem => self.infix(parent_prec, (13, "%", 14, 14), left, right),
            BinOp::BwXor => self.infix(parent_prec, (7, "^", 7, 7), left, right),
            BinOp::BwAnd => self.infix(parent_prec, (8, "&", 8, 8), left, right),
            BinOp::BwOr => self.infix(parent_prec, (6, "|", 6, 6), left, right),
            BinOp::ShiftLeft => self.infix(parent_prec.max(1), (0, "<<", 1, 1), left, right),
            BinOp::ShiftRight => self.infix(parent_prec.max(1), (0, ">>", 1, 1), left, right),
            BinOp::Lt | BinOp::LtPointer => self.infix(parent_prec, (10, "<", 10, 10), left, right),
            BinOp::Le | BinOp::LePointer => {
                self.infix(parent_prec, (10, "<=", 10, 10), left, right)
            }
            BinOp::Eq => self.infix(parent_prec, (9, "==", 9, 9), left, right),
        }
    }

    fn infix(
        &self,
        parent_prec: u8,
        (prec, op, left_prec, right_prec): (u8, &'static str, u8, u8),
        left: &IndexTerm,
        right: &IndexTerm,
    ) -> Doc {
        self.wrap_prec(
            parent_prec,
            prec,
            Doc::group(flow(
                Doc::concat(vec![Doc::line(), Doc::text(format!("{op} "))]),
                vec![self.index(left, left_prec), self.index(right, right_prec)],
            )),
        )
    }

    fn wrap_prec(&self, parent_prec: u8, own_prec: u8, body: Doc) -> Doc {
        if parent_prec > own_prec {
            parens(body)
        } else {
            body
        }
    }

    fn constant(&self, constant: &Const) -> Doc {
        match constant {
            Const::Z(value) => Doc::text(value.to_string()),
            Const::Bits((sign, width), value) => {
                let decimal = Doc::text(format!("{}'{}{}", value, sign_prefix(*sign), width));
                if value < &num_bigint::BigInt::from(16) {
                    decimal
                } else {
                    Doc::concat(vec![
                        decimal,
                        Doc::text(" /* 0x"),
                        Doc::text(bits_residue(*width, value).to_str_radix(16)),
                        Doc::text(" */"),
                    ])
                }
            }
            Const::Q(value) => Doc::text(value.to_string()),
            Const::MemByte { alloc_id, value } => braces(Doc::concat(vec![
                self.optional_alloc_id(alloc_id.as_ref()),
                Doc::text("; "),
                Doc::text(format!("0x{}", value.to_str_radix(16))),
            ])),
            Const::Pointer { alloc_id, addr } => braces(Doc::concat(vec![
                self.alloc_id(alloc_id),
                Doc::text("; "),
                Doc::text(format!("0x{}", addr.to_str_radix(16))),
            ])),
            Const::AllocId(value) => self.alloc_id(value),
            Const::Bool(value) => Doc::text(value.to_string()),
            Const::Unit => Doc::text("void"),
            Const::Null => Doc::text("NULL"),
            Const::CTypeConst(ct) => {
                Doc::concat(vec![Doc::text("'"), self.sctype(ct), Doc::text("'")])
            }
            Const::InputDefault(bt) => self.call("default", vec![self.bt(bt)]),
            Const::Unknown(bt) => self.call("unknown", vec![self.bt(bt)]),
        }
    }

    fn field_assign(&self, id: &Id, value: &IndexTerm) -> Doc {
        Doc::group(Doc::concat(vec![
            Doc::group(Doc::concat(vec![
                Doc::text("."),
                self.id(id),
                Doc::text(" ="),
            ])),
            Doc::text(" "),
            Doc::align(self.index(value, 0)),
        ]))
    }

    fn record_value(&self, fields: &[(Id, IndexTerm)]) -> Doc {
        let fields = flow(
            Doc::concat(vec![Doc::line0(), Doc::text(", ")]),
            fields
                .iter()
                .map(|(id, value)| self.field_assign(id, value)),
        );
        Doc::align(Doc::concat(vec![Doc::text("{ "), fields, Doc::text(" }")]))
    }

    fn struct_value(&self, fields: &[(Id, IndexTerm)]) -> Doc {
        let fields = Doc::join(
            Doc::concat(vec![Doc::text(","), Doc::hardline()]),
            fields
                .iter()
                .map(|(id, value)| self.field_assign(id, value)),
        );
        Doc::concat(vec![
            Doc::text("{"),
            Doc::hardline(),
            fields,
            Doc::text(" "),
            Doc::text("}"),
        ])
    }

    fn pattern(&self, pattern: &Pattern) -> Doc {
        match &pattern.pattern {
            PatternKind::Sym(name) => self.sym(name),
            PatternKind::Wild => Doc::text("_"),
            PatternKind::Constructor(name, fields) => Doc::concat(vec![
                self.sym(name),
                Doc::text(" "),
                raw_braces(fixed_comma_list(fields.iter().map(|(id, pattern)| {
                    Doc::concat(vec![self.id(id), Doc::text(": "), self.pattern(pattern)])
                }))),
            ]),
        }
    }

    fn scope_name(&self, name: &ScopeName) -> Doc {
        match name {
            ScopeName::Str(name) => Doc::text(format!("{name:?}")),
            ScopeName::Index(index) => self.index(index, 0),
            ScopeName::Sid(id) => Doc::text(id.to_string()),
        }
    }

    fn sym(&self, symbol: &Sym) -> Doc {
        Doc::text(sym_text(symbol))
    }

    fn id(&self, id: &Id) -> Doc {
        Doc::text(id.0.clone())
    }

    fn label(&self, label: &Label) -> Doc {
        Doc::text(label.0.to_string())
    }

    fn alloc_id(&self, value: &num_bigint::BigInt) -> Doc {
        Doc::text(format!("@{value}"))
    }

    fn optional_alloc_id(&self, value: Option<&num_bigint::BigInt>) -> Doc {
        match value {
            Some(value) => self.alloc_id(value),
            None => Doc::text("@empty"),
        }
    }

    fn call(&self, name: &'static str, args: Vec<Doc>) -> Doc {
        c_app(Doc::text(name), args)
    }

    fn generic_call(&self, name: &'static str, ty: Doc, args: Vec<Doc>) -> Doc {
        c_app(
            Doc::concat(vec![Doc::text(name), Doc::text("<"), ty, Doc::text(">")]),
            args,
        )
    }
}

fn sign_prefix(sign: Sign) -> &'static str {
    match sign {
        Sign::Signed => "i",
        Sign::Unsigned => "u",
    }
}

fn spelled_as_typedef(base: &IntegerBaseType) -> bool {
    matches!(
        base,
        IntegerBaseType::IntN(_)
            | IntegerBaseType::IntLeastN(_)
            | IntegerBaseType::IntFastN(_)
            | IntegerBaseType::Intmax
            | IntegerBaseType::Intptr
    )
}

fn comma_list(items: impl IntoIterator<Item = Doc>) -> Doc {
    Doc::join(Doc::concat(vec![Doc::text(","), Doc::line()]), items)
}

fn fixed_comma_list(items: impl IntoIterator<Item = Doc>) -> Doc {
    Doc::join(Doc::text(", "), items)
}

fn fixed_semicolon_list(items: impl IntoIterator<Item = Doc>) -> Doc {
    Doc::join(Doc::text("; "), items)
}

fn underscore_list(items: impl IntoIterator<Item = Doc>) -> Doc {
    Doc::join(Doc::text("_"), items)
}

fn raw_parens(body: Doc) -> Doc {
    raw_delimited("(", body, ")")
}

fn raw_brackets(body: Doc) -> Doc {
    raw_delimited("[", body, "]")
}

fn raw_braces(body: Doc) -> Doc {
    raw_delimited("{", body, "}")
}

fn raw_delimited(open: &'static str, body: Doc, close: &'static str) -> Doc {
    Doc::concat(vec![Doc::text(open), body, Doc::text(close)])
}

fn c_app(function: Doc, args: impl IntoIterator<Item = Doc>) -> Doc {
    Doc::group(Doc::concat(vec![
        function,
        Doc::group(raw_parens(Doc::nest(
            2,
            flow(Doc::concat(vec![Doc::line0(), Doc::text(", ")]), args),
        ))),
    ]))
}

fn flow(separator: Doc, items: impl IntoIterator<Item = Doc>) -> Doc {
    let mut items = items.into_iter();
    let Some(first) = items.next() else {
        return Doc::nil();
    };
    items.fold(first, |prefix, item| {
        Doc::concat(vec![
            prefix,
            Doc::group(Doc::concat(vec![separator.clone(), item])),
        ])
    })
}

fn parens(body: Doc) -> Doc {
    tight_delimited("(", body, ")")
}

fn braces(body: Doc) -> Doc {
    tight_delimited("{", body, "}")
}

fn tight_delimited(open: &'static str, body: Doc, close: &'static str) -> Doc {
    if body.is_nil() {
        return Doc::concat(vec![Doc::text(open), Doc::text(close)]);
    }
    Doc::group(Doc::concat(vec![
        Doc::text(open),
        Doc::nest(2, Doc::concat(vec![Doc::line0(), body])),
        Doc::line0(),
        Doc::text(close),
    ]))
}

fn block(body: Doc) -> Doc {
    if body.is_nil() {
        return Doc::text("{}");
    }
    Doc::concat(vec![
        Doc::text("{"),
        Doc::nest(2, Doc::concat(vec![Doc::line(), body])),
        Doc::line(),
        Doc::text("}"),
    ])
}

#[derive(Clone, Debug)]
struct Doc(RcDoc<'static>);

impl Doc {
    fn nil() -> Self {
        Doc(RcDoc::nil())
    }

    fn text(value: impl Into<Cow<'static, str>>) -> Self {
        Doc(RcDoc::text(value))
    }

    fn line() -> Self {
        Doc(RcDoc::line())
    }

    fn line0() -> Self {
        Doc(RcDoc::line_())
    }

    fn hardline() -> Self {
        Doc(RcDoc::hardline())
    }

    fn concat(docs: impl IntoIterator<Item = Doc>) -> Self {
        Doc(RcDoc::concat(docs.into_iter().map(|doc| doc.0)))
    }

    fn join(separator: Doc, docs: impl IntoIterator<Item = Doc>) -> Self {
        Doc(RcDoc::intersperse(
            docs.into_iter().map(|doc| doc.0),
            separator.0,
        ))
    }

    fn nest(indent: isize, body: Doc) -> Self {
        Doc(body.0.nest(indent))
    }

    fn align(body: Doc) -> Self {
        Doc(body.0.pretty(&RcAllocator).align().into_doc())
    }

    fn group(body: Doc) -> Self {
        Doc(body.0.group())
    }

    fn is_nil(&self) -> bool {
        matches!(*self.0, pretty::Doc::Nil)
    }
}

fn render(doc: &Doc, width: usize) -> String {
    let mut out = String::new();
    doc.0
        .render_fmt(width, &mut out)
        .expect("writing to a String cannot fail");
    out
}

pub fn sym_text(symbol: &Sym) -> String {
    match &symbol.name {
        Some(name) => format!("{name}{{{}}}", symbol.num),
        None => format!("a_{}", symbol.num),
    }
}
