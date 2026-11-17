//! Deflate the emoji names table (`src/emoji_names.tsv`, the source kept in
//! git) into `OUT_DIR`: every client embeds it, and packed it takes about a
//! third of the room. `emoji_names` inflates it on first use.

fn main() {
    let src = "src/emoji_names.tsv";
    println!("cargo::rerun-if-changed={src}");
    let table = std::fs::read(src).expect("read src/emoji_names.tsv");
    let packed = miniz_oxide::deflate::compress_to_vec(&table, 10);
    let out = std::path::Path::new(&std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("emoji_names.tsv.deflate");
    std::fs::write(out, packed).expect("write emoji_names.tsv.deflate");
}
