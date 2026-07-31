#![no_main]

use covy_ingest::lcov::LcovIngestor;
use covy_ingest::Ingestor;
use libfuzzer_sys::fuzz_target;

const MAX_FUZZ_INPUT_BYTES: usize = 256 * 1024;

fuzz_target!(|input: &[u8]| {
    if input.len() > MAX_FUZZ_INPUT_BYTES {
        return;
    }

    let _ = LcovIngestor.parse(input);
});
