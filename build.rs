fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/tei/v1/tei.proto");
    println!("cargo:rerun-if-changed=migrations");
    tonic_prost_build::configure()
        .build_server(false)
        .compile_protos(&["proto/tei/v1/tei.proto"], &["proto"])?;
    Ok(())
}
