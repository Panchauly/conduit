use std::path::PathBuf;

fn main() {
    // SAFETY: single-threaded build script.
    unsafe {
        std::env::set_var(
            "PROTOC",
            protoc_bin_vendored::protoc_bin_path().expect("vendored protoc"),
        );
    }

    let root: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf();
    let proto = root.join("proto/conduit/v1/ingest.proto");
    let include = root.join("proto");

    println!("cargo:rerun-if-changed={}", proto.display());

    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(&[proto], &[include])
        .expect("compile ingest.proto");
}
