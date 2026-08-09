//! Dump the parsed RTSA metadata for a `.rtsa` file without streaming
//! its samples. Used to verify whether the center frequency (and other
//! tuning info) is recoverable from the file's chunk metadata.
//!
//! Usage: cargo run -p sdr-aaronia-rs --example dump_metadata --release -- <FILE.rtsa>

use sdr_aaronia_rs::RtsaSource;

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: dump_metadata <FILE.rtsa>");

    let source = RtsaSource::open(&path)?;
    let m = source.metadata();

    println!("== RTSA metadata for {path} ==");
    println!("file_format_version : {}", m.file_format_version);
    println!("num_streams         : {}", m.num_streams);
    println!("primary_stream_id   : {}", m.primary_stream_id);
    println!("stream_type         : {:?}", m.stream_type);
    println!();
    println!(
        "sample_rate         : {} Hz ({:.6} MHz)",
        m.sample_rate,
        m.sample_rate / 1e6
    );
    println!(
        "center_frequency    : {:?} ({})",
        m.center_frequency,
        m.center_frequency
            .map(|f| format!("{:.6} MHz", f / 1e6))
            .unwrap_or_else(|| "None".into())
    );
    println!(
        "bandwidth           : {} Hz ({:.6} MHz)",
        m.bandwidth,
        m.bandwidth / 1e6
    );
    println!("total_samples       : {}", m.total_samples);
    println!("total_sample_chunks : {}", m.total_sample_chunks);
    println!("sample_data_size    : {} bytes", m.sample_data_size);
    println!();
    println!("sub_streams         : {}", m.sub_streams.len());
    for (i, s) in m.sub_streams.iter().enumerate() {
        let center = s.frequency_start + s.frequency_span / 2.0;
        println!(
            "  [{i}] id={}/{} name={:?} start={:.6} MHz step={} span={:.6} MHz  => center {:.6} MHz",
            s.stream_id,
            s.sub_stream_id,
            s.name,
            s.frequency_start / 1e6,
            s.frequency_step,
            s.frequency_span / 1e6,
            center / 1e6,
        );
    }
    println!();
    println!("antennas            : {}", m.antennas.len());
    for (i, a) in m.antennas.iter().enumerate() {
        println!("  [{i}] id={} name={:?}", a.antenna_id, a.name);
    }

    Ok(())
}
