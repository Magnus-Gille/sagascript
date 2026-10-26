//! WebRTC AEC3 on 10ms aligned frames. CPU DSP; ASR stays on the ANE host.
use webrtc_audio_processing::{config::EchoCanceller, Config, Processor};
pub const FRAME: usize = 160;
pub struct EchoCanceller3 {
    processor: Processor,
}
impl EchoCanceller3 {
    pub fn new() -> Result<Self, String> {
        let processor = Processor::new(16_000).map_err(|e| e.to_string())?;
        processor.set_config(Config {
            echo_canceller: Some(EchoCanceller::Full {
                stream_delay_ms: None,
            }),
            ..Default::default()
        });
        Ok(Self { processor })
    }
    pub fn process(
        &mut self,
        microphone: &[f32; FRAME],
        system: &[f32; FRAME],
    ) -> Result<[f32; FRAME], String> {
        if microphone.iter().chain(system).any(|v| !v.is_finite()) {
            return Err("Non-finite AEC input".into());
        }
        let mut reference = vec![system.to_vec()];
        self.processor
            .process_render_frame(&mut reference)
            .map_err(|e| e.to_string())?;
        let mut capture = vec![microphone.to_vec()];
        self.processor
            .process_capture_frame(&mut capture)
            .map_err(|e| e.to_string())?;
        let mut result = [0.; FRAME];
        result.copy_from_slice(&capture[0]);
        if result.iter().any(|v| !v.is_finite()) {
            return Err("Non-finite AEC output".into());
        }
        Ok(result)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delayed_echo_is_suppressed_and_double_talk_is_retained() {
        let mut processor = EchoCanceller3::new().unwrap();
        let mut seed = 123456789u32;
        let far: Vec<_> = (0..320_000)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                seed as i32 as f32 / i32::MAX as f32 * 0.18
            })
            .collect();
        let mut before = 0f64;
        let mut after = 0f64;
        let mut near_signal = Vec::new();
        let mut clean_signal = Vec::new();
        for i in (0..far.len()).step_by(FRAME) {
            let mut microphone = [0.; FRAME];
            let mut reference = [0.; FRAME];
            let mut near = [0.; FRAME];
            reference.copy_from_slice(&far[i..i + FRAME]);
            for j in 0..FRAME {
                let n = i + j;
                if n >= 960 {
                    microphone[j] = far[n - 960] * 0.6;
                }
                if n >= 240_000 {
                    near[j] = 0.14 * (n as f32 * std::f32::consts::TAU * 337. / 16_000.).sin()
                        + 0.08 * (n as f32 * std::f32::consts::TAU * 701. / 16_000.).sin();
                    microphone[j] += near[j];
                }
            }
            let clean = processor.process(&microphone, &reference).unwrap();
            if (160_000..240_000).contains(&i) {
                before += microphone
                    .iter()
                    .map(|x| f64::from(*x).powi(2))
                    .sum::<f64>();
                after += clean.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>();
            }
            if i >= 272_000 {
                near_signal.extend(near);
                clean_signal.extend(clean);
            }
        }
        assert!(
            10. * (before / after).log10() > 15.,
            "echo attenuation insufficient"
        );
        // AEC3 introduces a block-processing delay. Compare the known near-end
        // waveform over a bounded 20ms lag, not phase-sensitive zero-lag energy.
        let n = near_signal.len() - 320;
        let energy = near_signal[..n]
            .iter()
            .map(|v| f64::from(*v).powi(2))
            .sum::<f64>();
        let (lag, gain) = (0..=320)
            .map(|lag| {
                let projection = (0..n)
                    .map(|i| f64::from(near_signal[i]) * f64::from(clean_signal[i + lag]))
                    .sum::<f64>()
                    / energy;
                (lag, projection)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        let residual = (0..n)
            .map(|i| (f64::from(clean_signal[i + lag]) - f64::from(near_signal[i])).powi(2))
            .sum::<f64>()
            / energy;
        assert!(
            (0.75..=1.2).contains(&gain),
            "near-end gain {gain} at lag {lag}"
        );
        assert!(
            residual < 0.3,
            "double-talk residual {residual} at lag {lag}"
        );
    }
}
