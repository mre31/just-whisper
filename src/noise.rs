use crate::audio;
use anyhow::{Context, Result, ensure};
use df::tract::{DfParams, DfTract, RuntimeParams};
use ndarray::{ArrayView2, ArrayViewMut2};
use std::sync::atomic::{AtomicBool, Ordering};

/// Owned by the engine thread. The pristine model is never processed so every
/// take starts with fresh recurrent, normalization and overlap-add state.
#[derive(Default)]
pub struct Processor {
    template: Option<DfTract>,
}

impl Processor {
    pub fn release(&mut self) {
        self.template = None;
    }

    /// Input is 48 kHz mono PCM; output is 16 kHz mono PCM for Whisper.
    pub fn prepare(
        &mut self,
        samples: Vec<f32>,
        enabled: bool,
        abort: &AtomicBool,
    ) -> Result<Vec<f32>> {
        check_cancelled(abort)?;
        if !audio::has_speech(&samples, audio::CAPTURE_RATE) {
            return Ok(Vec::new());
        }
        let samples = if enabled {
            self.enhance(&samples, abort)?
        } else {
            samples
        };
        check_cancelled(abort)?;
        let samples = df::transforms::resample(
            ArrayView2::from_shape((1, samples.len()), &samples)?,
            audio::CAPTURE_RATE,
            audio::SAMPLE_RATE,
            None,
        )
        .context("Ses Whisper için 16 kHz'e dönüştürülemedi")?;
        check_cancelled(abort)?;
        Ok(samples.iter().map(|s| s.clamp(-1.0, 1.0)).collect())
    }

    fn enhance(&mut self, samples: &[f32], abort: &AtomicBool) -> Result<Vec<f32>> {
        check_cancelled(abort)?;
        if self.template.is_none() {
            // Disable the optional post-filter to avoid extra speech distortion.
            let model = DfTract::new(DfParams::default(), &RuntimeParams::default()).context(
                "DeepFilterNet3 yüklenemedi. Gürültü azaltmayı ayarlardan kapatabilirsiniz",
            )?;
            ensure!(
                model.sr == audio::CAPTURE_RATE,
                "DeepFilterNet3 örnekleme hızı uyumsuz"
            );
            self.template = Some(model);
        }
        let mut model = self.template.as_ref().unwrap().clone();
        let delay = model.fft_size - model.hop_size + model.lookahead * model.hop_size;
        enhance_frames(samples, model.hop_size, delay, abort, |input, output| {
            model.process(
                ArrayView2::from_shape((1, input.len()), input)?,
                ArrayViewMut2::from_shape((1, output.len()), output)?,
            )?;
            Ok(())
        })
        .context("DeepFilterNet3 ses temizleme işlemi tamamlanamadı")
    }
}

fn check_cancelled(abort: &AtomicBool) -> Result<()> {
    ensure!(!abort.load(Ordering::SeqCst), "Ses işleme iptal edildi");
    Ok(())
}

fn enhance_frames(
    samples: &[f32],
    hop: usize,
    delay: usize,
    abort: &AtomicBool,
    mut process: impl FnMut(&[f32], &mut [f32]) -> Result<()>,
) -> Result<Vec<f32>> {
    let mut input = vec![0.0; hop];
    let mut output = vec![0.0; hop];
    let mut enhanced = Vec::with_capacity(samples.len());
    // Pad the partial frame and flush the delayed tail, then discard only the
    // initial algorithmic delay. Keep exactly the original number of samples.
    for start in (0..samples.len() + delay).step_by(hop) {
        check_cancelled(abort)?;
        input.fill(0.0);
        if start < samples.len() {
            let end = (start + hop).min(samples.len());
            input[..end - start].copy_from_slice(&samples[start..end]);
        }
        process(&input, &mut output)?;
        ensure!(
            output.iter().all(|s| s.is_finite()),
            "Filtre geçersiz ses üretti"
        );
        let first = delay.saturating_sub(start).min(hop);
        let last = (samples.len() + delay - start).min(hop);
        enhanced.extend_from_slice(&output[first..last]);
    }
    Ok(enhanced)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn partial_frames_and_delay_preserve_first_and_last_samples() {
        for length in [1, 479, 480, 481, 997, 4800] {
            for delay in [0, 480, 1440] {
                let samples: Vec<_> = (1..=length).map(|n| n as f32).collect();
                let mut queued = VecDeque::from(vec![0.0; delay]);
                let result = enhance_frames(
                    &samples,
                    480,
                    delay,
                    &AtomicBool::new(false),
                    |input, output| {
                        queued.extend(input);
                        for sample in output {
                            *sample = queued.pop_front().unwrap();
                        }
                        Ok(())
                    },
                )
                .unwrap();
                assert_eq!(result, samples, "length {length}, delay {delay}");
            }
        }
    }

    #[test]
    fn cancellation_stops_between_frames() {
        let abort = AtomicBool::new(false);
        let mut calls = 0;
        let result = enhance_frames(&vec![0.1; 4800], 480, 1440, &abort, |_, _| {
            calls += 1;
            abort.store(true, Ordering::SeqCst);
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn disabled_filter_resamples_without_losing_duration_or_tail() {
        let length = audio::CAPTURE_RATE + 7;
        let samples: Vec<_> = (0..length)
            .map(|i| {
                0.1 * (std::f32::consts::TAU * 1000.0 * i as f32 / audio::CAPTURE_RATE as f32).sin()
            })
            .collect();
        let mut processor = Processor::default();
        let output = processor
            .prepare(samples, false, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(output.len(), length.div_ceil(3));
        assert!(output[..320].iter().any(|s| s.abs() > 0.05));
        assert!(output[output.len() - 320..].iter().any(|s| s.abs() > 0.05));
        assert!(processor.template.is_none());
    }

    #[test]
    fn silence_and_cancel_do_not_load_model() {
        let mut processor = Processor::default();
        assert!(
            processor
                .prepare(
                    vec![0.0; audio::CAPTURE_RATE],
                    true,
                    &AtomicBool::new(false)
                )
                .unwrap()
                .is_empty()
        );
        assert!(
            processor
                .prepare(vec![0.1; audio::CAPTURE_RATE], true, &AtomicBool::new(true))
                .is_err()
        );
        assert!(processor.template.is_none());
    }

    #[test]
    fn real_model_reduces_noise_and_resets_between_takes() {
        let mut seed = 17_u32;
        let samples: Vec<_> = (0..audio::CAPTURE_RATE / 2 + 37)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32 * 0.03
            })
            .collect();
        let mut processor = Processor::default();
        let abort = AtomicBool::new(false);
        let first = processor.enhance(&samples, &abort).unwrap();
        let second = processor.enhance(&samples, &abort).unwrap();
        assert_eq!(first.len(), samples.len());
        assert_eq!(
            first, second,
            "The preceding take must not change filter state"
        );
        let energy = |s: &[f32]| s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32;
        assert!(energy(&first[9600..]) < energy(&samples[9600..]) * 0.5);
    }
}
