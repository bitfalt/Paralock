//! Regenerates `src/lightwalletd/proto.rs` from the vendored protos with tonic-prost-build and protox (no `protoc`).
//!
//! ```sh
//! cd crates/paralock/tools/protogen && cargo run -- ../../proto /tmp/out
//! # then prepend the 3-line header of src/lightwalletd/proto.rs to /tmp/out/cash.z.wallet.sdk.rpc.rs, replace the file
//! # and run `cargo fmt -p paralock`
//! ```
fn main() {
    let proto_dir = std::env::args().nth(1).expect("proto dir");
    let out_dir = std::env::args().nth(2).expect("out dir");
    std::fs::create_dir_all(&out_dir).expect("out dir");
    let fds = protox::compile(["service.proto", "compact_formats.proto"], [proto_dir.as_str()]).expect("compile");
    tonic_prost_build::configure()
        .build_server(false)
        .build_client(true)
        .out_dir(&out_dir)
        .compile_fds(fds)
        .expect("generate");
}
