#![no_main]
//! Fuzz the full analyzer pipeline.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let parsed = pkl_syntax::parse(s);
    let _ = pkl_analyze::analyze(&parsed.module, parsed.diagnostics);
});
