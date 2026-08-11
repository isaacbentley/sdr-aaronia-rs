use num_complex::Complex32;
use std::path::Path;

use crate::{Result, Error};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::sdk_sink::{SdkSink, SdkSinkConfig};

pub enum UnifiedSinkBackend {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    Sdk(SdkSink),
    Stub, // Placeholder for HTTP sink or when no SDK is available
}

/// A unified transmission sink that abstracts over native SDK and HTTP TX.
pub struct UnifiedSink {
    backend: Option<UnifiedSinkBackend>,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    config: SdkSinkConfig,
}

impl UnifiedSink {
    pub fn new() -> Self {
        Self {
            backend: None,
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            config: SdkSinkConfig::default(),
        }
    }

    pub fn with_sdk() -> Self {
        Self::new()
    }

    pub async fn initialize(&mut self) -> Result<()> {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            let mut sdk_sink = SdkSink::with_config(self.config.clone());
            sdk_sink.initialize().await?;
            self.backend = Some(UnifiedSinkBackend::Sdk(sdk_sink));
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            self.backend = Some(UnifiedSinkBackend::Stub);
        }
        Ok(())
    }

    pub async fn stop_streaming(&mut self) -> Result<()> {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        if let Some(UnifiedSinkBackend::Sdk(ref mut sdk_sink)) = self.backend {
            sdk_sink.stop_streaming().await?;
        }
        Ok(())
    }

    pub fn get_master_stream_time(&mut self) -> Result<f64> {
        match &mut self.backend {
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            Some(UnifiedSinkBackend::Sdk(sink)) => sink.get_master_stream_time(),
            _ => Err(Error::Sdk("Sink not initialized".to_string())),
        }
    }

    pub fn write_samples(
        &mut self,
        channel: i32,
        start_time_s: f64,
        end_time_s: f64,
        samples: &[Complex32],
    ) -> Result<()> {
        match &mut self.backend {
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            Some(UnifiedSinkBackend::Sdk(sink)) => {
                let burst = crate::native_sdk::TxBurst {
                    start_time: start_time_s,
                    end_time: end_time_s,
                    flags: crate::native_sdk::tx_flags::SEGMENT_START | crate::native_sdk::tx_flags::SEGMENT_END | crate::native_sdk::tx_flags::PUSH,
                };
                sink.write_samples(channel, burst, samples)
            },
            _ => Err(Error::Sdk("Sink not initialized or unsupported platform".to_string())),
        }
    }
}

impl Default for UnifiedSink {
    fn default() -> Self {
        Self::new()
    }
}

pub struct AaroniaSinkBuilder {
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    config: SdkSinkConfig,
}

impl AaroniaSinkBuilder {
    pub fn new() -> Self {
        Self {
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            config: SdkSinkConfig::default(),
        }
    }

    pub fn build(self) -> UnifiedSink {
        let mut sink = UnifiedSink::new();
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        {
            sink.config = self.config;
        }
        sink
    }
}

impl Default for AaroniaSinkBuilder {
    fn default() -> Self {
        Self::new()
    }
}
