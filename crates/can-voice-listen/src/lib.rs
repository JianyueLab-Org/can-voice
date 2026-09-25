//! Receive-only bridge core for the web listener.
//!
//! This crate owns the server-side CAN voice connection. It deliberately has
//! no transmit API: callers can select RX frequencies, receive decoded PCM,
//! and shut the connection down.

use can_voice_client::{Config as VoiceConfig, VoiceClient};
use can_voice_proto::control::Sub;

#[derive(Debug, Clone)]
pub struct Config {
    pub server: String,
    pub server_name: String,
    pub token: String,
    pub client_id: String,
    pub extra_roots: Vec<Vec<u8>>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Voice(#[from] can_voice_client::client::Error),
    #[error("frequency must be between 118000 and 136975 kHz on a 5 kHz raster")]
    InvalidFrequency,
}

pub struct ListenGateway {
    voice: VoiceClient,
    frames: tokio::sync::broadcast::Receiver<Vec<i16>>,
}

impl ListenGateway {
    pub async fn connect(config: Config, frequency_khz: u32) -> Result<Self, Error> {
        validate_frequency(frequency_khz)?;
        let voice = VoiceClient::connect(VoiceConfig {
            server: config.server,
            server_name: config.server_name,
            token: config.token,
            client_id: config.client_id,
            follow: String::new(),
            station: String::new(),
            input_device: None,
            output_device: None,
            audio_devices: false,
            extra_roots: config.extra_roots,
        })
        .await?;
        let frames = voice.audio_frames();
        voice.set_subscription(subscription(frequency_khz));
        Ok(Self { voice, frames })
    }

    pub fn frames(&mut self) -> &mut tokio::sync::broadcast::Receiver<Vec<i16>> {
        &mut self.frames
    }

    pub fn set_frequency(&self, frequency_khz: u32) -> Result<(), Error> {
        validate_frequency(frequency_khz)?;
        self.voice.set_subscription(subscription(frequency_khz));
        Ok(())
    }

    pub async fn shutdown(self) {
        self.voice.shutdown().await;
    }
}

fn subscription(frequency_khz: u32) -> Sub {
    Sub {
        rx: vec![frequency_khz],
        tx: Vec::new(),
        xc: Vec::new(),
    }
}

fn validate_frequency(frequency_khz: u32) -> Result<(), Error> {
    if (118000..=136975).contains(&frequency_khz) && frequency_khz % 5 == 0 {
        Ok(())
    } else {
        Err(Error::InvalidFrequency)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_subscription_has_rx_only() {
        let sub = subscription(118500);
        assert_eq!(sub.rx, [118500]);
        assert!(sub.tx.is_empty());
        assert!(sub.xc.is_empty());
    }

    #[test]
    fn invalid_frequency_is_rejected_before_network_connect() {
        for frequency in [117995, 118501, 136980] {
            assert!(matches!(
                validate_frequency(frequency),
                Err(Error::InvalidFrequency)
            ));
        }
    }
}
