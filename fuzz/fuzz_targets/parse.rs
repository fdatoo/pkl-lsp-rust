#![no_main]
//! Fuzz the parser: shove arbitrary bytes through `pkl_syntax::parse`
//! and make sure it never panics. UTF-8 is required by the lexer, so we
//! parse only when the bytes round-trip as valid UTF-8.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    let _ = pkl_syntax::parse(s);
});
