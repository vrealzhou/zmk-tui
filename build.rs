use std::path::PathBuf;

fn main() {
    let proto_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("proto/zmk");
    let files = [
        "studio.proto",
        "core.proto",
        "keymap.proto",
        "behaviors.proto",
        "meta.proto",
    ]
    .map(|name| proto_root.join(name));

    let protoc = protoc_bin_vendored::protoc_bin_path().expect("bundled protoc is available");

    let mut config = prost_build::Config::new();
    config.protoc_executable(protoc);
    config
        .compile_protos(&files, &[&proto_root])
        .expect("ZMK Studio protobuf definitions compile");

    println!("cargo:rerun-if-changed=proto/zmk");
}
