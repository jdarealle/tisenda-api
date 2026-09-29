fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/tei.proto");
    tonic_prost_build::configure()
        .build_server(false)
        .compile_protos(&["proto/tei.proto"], &["proto"])?;
    Ok(())
}
