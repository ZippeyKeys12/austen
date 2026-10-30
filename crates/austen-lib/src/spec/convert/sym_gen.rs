use crate::gen_dsl::sym_scan::{
    max_sym_num_in_base_type, max_sym_num_in_constraint, max_sym_num_in_datatype,
    max_sym_num_in_logical_function, max_sym_num_in_sctype, max_sym_num_in_term,
};
use crate::gen_dsl::{BaseType, IndexTerm, LogicalConstraint, Sctype, Sym, SymGen};
use crate::spec::{ArgType, Lat, PredicateDef, Request, RequestName, SpecModule};

fn scan_base_type(bt: &BaseType, max: &mut u64) {
    *max = (*max).max(max_sym_num_in_base_type(bt));
}

fn scan_sctype(ct: &Sctype, max: &mut u64) {
    *max = (*max).max(max_sym_num_in_sctype(ct));
}

fn scan_term(it: &IndexTerm, max: &mut u64) {
    *max = (*max).max(max_sym_num_in_term(it));
}

fn scan_constraint(lc: &LogicalConstraint, max: &mut u64) {
    *max = (*max).max(max_sym_num_in_constraint(lc));
}

pub fn seeded(module: &SpecModule) -> SymGen {
    SymGen::starting_at(module.next_sym.unwrap_or_else(|| max_sym_num(module) + 1))
}

pub fn max_sym_num(module: &SpecModule) -> u64 {
    let mut max = 0;
    for ty in &module.types {
        if let Some(tag) = &ty.tag {
            bump(&mut max, tag);
        }
    }
    for global in &module.globals {
        bump(&mut max, &global.address);
        scan_sctype(&global.ty, &mut max);
    }
    for pred in &module.predicates {
        scan_predicate(pred, &mut max);
    }
    for func in &module.functions {
        if let Some(at) = &func.internal {
            scan_arg_type(at, &mut max);
        }
        let ens = &func.ensures;
        bump(&mut max, &ens.var.0);
        scan_base_type(&ens.var.1, &mut max);
        scan_lat(&ens.body, &mut |&(), _| {}, &mut max);
    }
    for f in &module.logical_functions {
        max = max.max(max_sym_num_in_logical_function(f));
    }
    for datatype in &module.datatypes {
        max = max.max(max_sym_num_in_datatype(datatype));
    }
    max
}

fn bump(max: &mut u64, sym: &Sym) {
    *max = (*max).max(sym.num);
}

fn scan_predicate(pred: &PredicateDef, max: &mut u64) {
    bump(max, &pred.name);
    bump(max, &pred.pointer);
    for (sym, bt) in &pred.iargs {
        bump(max, sym);
        scan_base_type(bt, max);
    }
    scan_base_type(&pred.oarg, max);
    for clause in pred.clauses.iter().flatten() {
        scan_term(&clause.guard, max);
        scan_lat(&clause.packing_ft, &mut |it, m| scan_term(it, m), max);
    }
}

fn scan_arg_type<I>(at: &ArgType<I>, max: &mut u64) {
    match at {
        ArgType::Computational((sym, bt), rest) | ArgType::Ghost((sym, bt), rest) => {
            bump(max, sym);
            scan_base_type(bt, max);
            scan_arg_type(rest, max);
        }
        ArgType::L(lat) => scan_lat(lat, &mut |_, _| {}, max),
    }
}

fn scan_lat<I>(lat: &Lat<I>, scan_i: &mut impl FnMut(&I, &mut u64), max: &mut u64) {
    match lat {
        Lat::Define((sym, it), rest) => {
            bump(max, sym);
            scan_term(it, max);
            scan_lat(rest, scan_i, max);
        }
        Lat::Resource((sym, (req, bt)), rest) => {
            bump(max, sym);
            scan_request(req, max);
            scan_base_type(bt, max);
            scan_lat(rest, scan_i, max);
        }
        Lat::Constraint(lc, rest) => {
            scan_constraint(lc, max);
            scan_lat(rest, scan_i, max);
        }
        Lat::I(i) => scan_i(i, max),
    }
}

fn scan_request(req: &Request, max: &mut u64) {
    match req {
        Request::P {
            name,
            pointer,
            iargs,
        } => {
            scan_request_name(name, max);
            scan_term(pointer, max);
            iargs.iter().for_each(|it| scan_term(it, max));
        }
        Request::Q {
            name,
            pointer,
            q,
            step,
            permission,
            iargs,
        } => {
            scan_request_name(name, max);
            scan_term(pointer, max);
            bump(max, &q.0);
            scan_base_type(&q.1, max);
            scan_sctype(step, max);
            scan_term(permission, max);
            iargs.iter().for_each(|it| scan_term(it, max));
        }
    }
}

fn scan_request_name(name: &RequestName, max: &mut u64) {
    match name {
        RequestName::Owned(ct, _) => scan_sctype(ct, max),
        RequestName::PName(sym) => bump(max, sym),
    }
}
