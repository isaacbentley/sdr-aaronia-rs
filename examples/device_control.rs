//! Health checks, inputs list, recording control, and license probing.
//! Run with: `cargo run --example device_control --features http`

use sdr_aaronia_rs::http_endpoints::{AuthMethod, HttpEndpointsClient};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = HttpEndpointsClient::new("http://localhost:54664".to_string(), AuthMethod::None)?;

    println!("Querying device info...");
    if let Ok(info) = client.get_info().await {
        println!("Server UUID: {}", info.uuid);
    }

    println!("\nQuerying device health...");
    if let Ok(health) = client.get_health_status().await {
        // Just print debug info
        println!("Health: {:?}", health);
    }

    println!("\nListing current inputs:");
    if let Ok(inputs) = client.get_inputs().await {
        for input in inputs {
            println!("- Input: {}", input);
        }
    }

    Ok(())
}
