//! Open a local RTSA capture, print metadata, and read samples efficiently.
//! Run with: `cargo run --example read_rtsa_file --features file`

use sdr_aaronia_rs::{AaroniaConfig, AaroniaSource};
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let file_path = if args.len() > 1 {
        &args[1]
    } else {
        println!("Usage: cargo run --example read_rtsa_file <path_to_rtsa_file>");
        return Ok(());
    };

    println!("Opening RTSA file: {}", file_path);
    let config = AaroniaConfig::from_file(file_path);
    let mut source = AaroniaSource::new(config).await?;

    let mut read_samples = 0;
    let mut buffer = Vec::with_capacity(1024 * 64);

    while let Ok(read) = source.read_samples(&mut buffer, 1024 * 64).await {
        if read == 0 {
            break;
        }
        read_samples += read;
        buffer.clear();
    }

    println!("Successfully read {} samples from the file.", read_samples);
    Ok(())
}
