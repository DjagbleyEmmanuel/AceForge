fn main() {
    tauri_build::build();

    // minimp3 (CC0), vendored so MP3 can be decoded without reaching for the
    // network at build time. Only the decoder is compiled; the encoder that
    // ships in the same upstream file is left off.
    cc::Build::new()
        .file("src/vendor/minimp3.c")
        .flag_if_supported("-Wno-conversion")
        .flag_if_supported("-Wno-sign-conversion")
        .warnings(false)
        .compile("minimp3");

    println!("cargo:rerun-if-changed=src/vendor/minimp3.c");
}
