use std::hint::black_box;

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
#[should_panic(expected = "attempt to add with overflow")]
fn release_build_panics_on_overflow() {
    let _ = black_box(u64::MAX) + black_box(1);
}
