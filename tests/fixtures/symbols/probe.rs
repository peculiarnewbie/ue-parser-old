#![allow(dead_code)]

#[no_mangle]
pub extern "C" fn ue_parser_probe_marker() -> u32 {
    0xC0_FFEEu32
}

fn main() {
    let value = ue_parser_probe_marker();
    std::process::exit(i32::from(value as u8));
}
