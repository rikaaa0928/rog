fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::compile_protos("proto/rog.proto")?;
    tonic_build::compile_protos("proto/rog2.proto")?;
    Ok(())
}
