//! Renders a 440 Hz sine at -12 dBFS through an Audio Unit effect, silently, and reports how
//! much of the output is not the tone (or its harmonics): a check for hosts that feed or read
//! a unit wrongly, which sounds like ring modulation or bit crushing.
//!
//! cargo run --release -p prowave-engine --example au_tone -- 'aufx:GCLP:SDIG' [block] [rate]
#[cfg(target_os = "macos")]
fn main() {
    use prowave_engine::au::{find, AuInstance};
    let args: Vec<String> = std::env::args().collect();
    let id = args.get(1).cloned().unwrap_or_default();
    let block: usize = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(64);
    let rate: u32 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(48_000);
    let Some(info) = find(&id) else {
        eprintln!("{id}: not found");
        std::process::exit(2);
    };
    let mut unit = match AuInstance::new(&info, rate, 4096) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let total = rate as usize * 2;
    let f = 440.0f64;
    let input: Vec<f32> = (0..total)
        .map(|i| (0.25 * (2.0 * std::f64::consts::PI * f * i as f64 / rate as f64).sin()) as f32)
        .collect();
    let (mut ol, mut or) = (vec![0.0f32; total], vec![0.0f32; total]);
    let started = std::time::Instant::now();
    let mut worst = 0f64;
    let mut at = 0;
    while at < total {
        let t0 = std::time::Instant::now();
        let n = block.min(total - at);
        let (l, r) = (&mut ol[at..at + n], &mut or[at..at + n]);
        unit.process((&input[at..at + n], &input[at..at + n]), (l, r));
        worst = worst.max(t0.elapsed().as_secs_f64() / (n as f64 / rate as f64));
        at += n;
    }
    let load = started.elapsed().as_secs_f64() / (total as f64 / rate as f64);
    println!(
        "DSP load: average {:.0}%, worst block {:.0}% of its time budget",
        load * 100.0,
        worst * 100.0
    );
    // Analyse the second second: fit the tone and its first 8 harmonics by least squares on
    // sin/cos pairs, and call the rest "other".
    let s = &ol[rate as usize..];
    let n = s.len();
    let power: f64 = s.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / n as f64;
    let mut tonal = 0.0;
    for h in 1..=8 {
        let w = 2.0 * std::f64::consts::PI * f * h as f64 / rate as f64;
        let (mut a, mut b) = (0.0, 0.0);
        for (i, x) in s.iter().enumerate() {
            let t = (rate as usize + i) as f64;
            a += *x as f64 * (w * t).sin();
            b += *x as f64 * (w * t).cos();
        }
        let (a, b) = (2.0 * a / n as f64, 2.0 * b / n as f64);
        tonal += (a * a + b * b) / 2.0;
    }
    let other = (power - tonal).max(1e-20);
    let peak = s.iter().fold(0f32, |m, x| m.max(x.abs()));
    println!(
        "{} block {block} @ {rate} Hz: out peak {:.3}, tone+harmonics {:.1} dB, other {:.1} dB, other/total {:.1} dB, latency {:.2} ms",
        info.label(),
        peak,
        10.0 * tonal.max(1e-20).log10(),
        10.0 * other.log10(),
        10.0 * (other / power.max(1e-20)).log10(),
        unit.latency_seconds() * 1e3,
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {}
