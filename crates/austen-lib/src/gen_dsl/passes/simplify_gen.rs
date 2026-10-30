use super::lift::lift_constraints_with_options;
use super::member_indirection::member_indirection;
use super::{
    ChoiceEffects, GoodHandling, MAX_PUSH_GROWTH, PassOptions, PassProfile, inline_term,
    partial_evaluation, prune_branches, prune_branches_with_paths, push_pull_bounded,
    remove_unused, simplify_index_term,
};
use crate::gen_dsl::lang::gen_module::GenModule;
use crate::gen_dsl::lang::gen_term::GenTerm;
use crate::gen_dsl::lang::sym::SymGen;

pub const SIMPLIFY_FUEL: u32 = 5;

pub fn simplify_gen(
    term: GenTerm,
    ctx: &GenModule,
    syms: &mut SymGen,
    passes: PassProfile,
    options: PassOptions,
    effects: &ChoiceEffects,
    observe: &mut dyn FnMut(&'static str, u32, bool),
) -> GenTerm {
    let mut push_budget = MAX_PUSH_GROWTH;
    simplify_gen_with_push_budget(
        term,
        ctx,
        syms,
        passes,
        options,
        effects,
        &mut push_budget,
        observe,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn simplify_gen_with_push_budget(
    term: GenTerm,
    ctx: &GenModule,
    syms: &mut SymGen,
    passes: PassProfile,
    options: PassOptions,
    effects: &ChoiceEffects,
    push_budget: &mut usize,
    observe: &mut dyn FnMut(&'static str, u32, bool),
) -> GenTerm {
    let mut current = term;
    for iteration in 0..SIMPLIFY_FUEL {
        let before = current.clone();
        let mut step = |name, t: GenTerm, f: &mut dyn FnMut(GenTerm) -> GenTerm| {
            let out = f(t.clone());
            observe(name, iteration, out != t);
            out
        };
        if passes.lift_constraints {
            current = step("lift_constraints", current, &mut |t| {
                lift_constraints_with_options(t, syms, options)
            });
        }
        current = step("prune_branches", current, &mut |t| {
            if options.path_pruning {
                prune_branches_with_paths(t)
            } else {
                prune_branches(t)
            }
        });
        current = step("simplify_index_term", current, &mut |t| {
            simplify_index_term(t, ctx, GoodHandling::Keep)
        });
        current = step("remove_unused", current, &mut remove_unused);
        current = step("inline_term", current, &mut |t| inline_term(t, syms));
        current = step("push_pull", current, &mut |t| {
            push_pull_bounded(t, syms, effects, push_budget)
        });
        current = step("partial_evaluation", current, &mut |t| {
            partial_evaluation(t, ctx)
        });
        if !options.experimental_product_arg_destruction {
            current = step("member_indirection", current, &mut |t| {
                member_indirection(t, syms)
            });
        }
        if current == before {
            break;
        }
    }
    current
}
