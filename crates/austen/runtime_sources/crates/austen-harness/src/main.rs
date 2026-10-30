use austen_harness::{AustenTarget, LINK_FACTS_VERSION, LinkFacts};

mod shim {
    pub const FUNCTION_NAMES: &[&str] = &[];
    pub const ARITIES: &[usize] = &[];
    pub const GLOBAL_LAYOUT: &str = "austen-global-layout-v1:[[],[]]";

    pub unsafe fn call(_fn_id: usize, _args: &[i128]) -> i128 {
        0
    }
}

unsafe extern "C" fn call(fn_id: usize, args: *const i128, n_args: usize) -> i128 {
    unsafe { shim::call(fn_id, core::slice::from_raw_parts(args, n_args)) }
}

fn main() {
    let target = AustenTarget {
        n_functions: shim::FUNCTION_NAMES.len(),
        arities: shim::ARITIES.as_ptr(),
        call,
    };
    let facts = LinkFacts {
        size: core::mem::size_of::<LinkFacts>(),
        version: LINK_FACTS_VERSION,
        flags: 0,
        layout: shim::GLOBAL_LAYOUT.as_ptr(),
        layout_len: shim::GLOBAL_LAYOUT.len(),
        globals: core::ptr::null(),
        global_count: 0,
        target: &raw const target,
        driver: None,
        hooks: core::ptr::null(),
    };
    unsafe { austen_harness::austen_harness_main_with_link_facts(&raw const facts) }
}
