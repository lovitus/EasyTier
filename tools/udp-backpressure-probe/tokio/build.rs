use std::{env, fs, path::PathBuf};

fn section<'a>(source: &'a str, begin: &str, end: &str) -> &'a str {
    assert_eq!(source.matches(begin).count(), 1, "ambiguous source start");
    assert_eq!(source.matches(end).count(), 1, "ambiguous source end");
    &source[source.find(begin).unwrap()..source.find(end).unwrap()]
}

fn main() {
    let source_path = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("../../mesh-udp-flush/adapter.rs.in");
    println!("cargo:rerun-if-changed={}", source_path.display());
    let source = fs::read_to_string(source_path).unwrap();
    let mut extracted = String::new();
    for name in ["WIRE", "MAX_BYTES"] {
        let prefix = format!("    const {name}:");
        let lines: Vec<_> = source
            .lines()
            .filter(|line| line.starts_with(&prefix))
            .collect();
        assert_eq!(lines.len(), 1);
        extracted.push_str(lines[0]);
        extracted.push('\n');
    }
    extracted.push_str(section(
        &source,
        "    fn group_end(",
        "\n    struct WriterStats {",
    ));
    extracted.push_str(section(
        &source,
        "    async fn send_group(",
        "\n    pub(super) async fn forward(",
    ));
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("actual_send.rs");
    fs::write(output, extracted).unwrap();
}
