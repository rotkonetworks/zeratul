fn main() -> Result<(), Box<dyn std::error::Error>> {
    // compile zidecar proto
    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        .compile(&["proto/zidecar.proto"], &["proto"])?;

    // compile lightwalletd proto
    tonic_build::configure()
        .build_server(false)
        .build_client(true)
        .compile(&["proto/lightwalletd.proto"], &["proto"])?;

    Ok(())
}
