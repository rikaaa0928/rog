fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_prost_build::compile_protos("proto/rog.proto")?;
    tonic_prost_build::compile_protos("proto/rog_reverse.proto")?;
    Ok(())
}
