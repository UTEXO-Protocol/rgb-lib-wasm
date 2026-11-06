use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use rgb_lib_wasm::solve_pow;
use sha2::{Digest, Sha256};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = setTimeout)]
    fn set_timeout(handler: &js_sys::Function, ms: i32) -> JsValue;
}

const TRANSFER_ID: &str = "abababababababababababababababababababababababababababababababab";
// at difficulty 22 the nonce for these inputs is 1139765
const POW_TIMESTAMP: &str = "1700000000";

/// Solve the proof of work while a 10 ms timer records when the event loop runs.
/// Return the nonce, the elapsed ms, the number of ticks and the largest gap in ms.
async fn solve_with_ticker(difficulty: u32) -> (String, f64, usize, f64) {
    let ticks = Rc::new(RefCell::new(Vec::new()));
    let stop = Rc::new(Cell::new(false));
    let (ticker_ticks, ticker_stop) = (ticks.clone(), stop.clone());
    wasm_bindgen_futures::spawn_local(async move {
        while !ticker_stop.get() {
            let sleep = js_sys::Promise::new(&mut |resolve, _| {
                set_timeout(&resolve, 10);
            });
            let _ = JsFuture::from(sleep).await;
            ticker_ticks.borrow_mut().push(js_sys::Date::now());
        }
    });

    let start = js_sys::Date::now();
    let nonce = solve_pow(TRANSFER_ID, POW_TIMESTAMP, difficulty).await;
    stop.set(true);
    let end = js_sys::Date::now();

    let digest = Sha256::digest(format!("{TRANSFER_ID}{POW_TIMESTAMP}{nonce}"));
    assert!(u128::from_be_bytes(digest[..16].try_into().unwrap()).leading_zeros() >= difficulty);

    let mut times = vec![start];
    times.extend(ticks.borrow().iter().filter(|t| **t <= end));
    times.push(end);
    let max_gap = times.windows(2).map(|w| w[1] - w[0]).fold(0.0, f64::max);
    (nonce, end - start, times.len() - 2, max_gap)
}

#[wasm_bindgen_test]
async fn pow_yields_to_event_loop() {
    let (nonce, elapsed, ticks, max_gap) = solve_with_ticker(22).await;
    web_sys::console::log_1(
        &format!("difficulty 22: nonce {nonce}, {elapsed} ms, {ticks} ticks, max gap {max_gap} ms")
            .into(),
    );
    assert!(
        ticks >= 2,
        "the event loop did not run during the proof of work"
    );
    assert!(
        max_gap < 500.0,
        "the event loop was blocked for {max_gap} ms"
    );
}

/// Measure the proof of work at the highest difficulty (28). It is slow, so it runs only on demand.
#[wasm_bindgen_test]
#[ignore]
async fn pow_max_difficulty_time() {
    let (nonce, elapsed, ticks, max_gap) = solve_with_ticker(28).await;
    let rate = (nonce.parse::<f64>().unwrap() + 1.0) / (elapsed / 1000.0);
    let mean_secs = 2f64.powi(28) / rate;
    web_sys::console::log_1(
        &format!(
            "difficulty 28: nonce {nonce}, {elapsed} ms, {ticks} ticks, max gap {max_gap} ms, \
             {rate:.0} hashes/s, mean time {mean_secs:.0} s"
        )
        .into(),
    );
    assert!(
        ticks >= 2,
        "the event loop did not run during the proof of work"
    );
}
