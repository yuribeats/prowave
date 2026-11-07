#!/usr/bin/env python3
"""Synthesized drum one-shots for the public templates' Channel Racks (CC0: made here from
sine waves, square waves and seeded noise; no recording is used).

    python3 tools/templates/drum_samples.py      # writes crates/prowave-app/templates/samples/

Deterministic: the noise comes from a fixed-seed generator, so the same files come out every
time. 44.1 kHz, 16-bit, mono WAV.

Three families stand in for the machines the templates name: "808" (TR-808, CR-78: analog
tones and square-wave metal), "909" (TR-909, TR-505: punchier, noisier) and "acoustic"
(LinnDrum, break-style kits: short, with more body and click). Claps, rims, rides, crashes,
shakers and the cowbell are shared."""
import math
import os
import random
import struct
import wave

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
OUT = os.path.join(ROOT, 'crates', 'prowave-app', 'templates-public', 'samples')
SR = 44100
TOKEN = '$TEMPLATE_SAMPLES'


class Noise:
    def __init__(self, seed):
        self.r = random.Random(seed)

    def __call__(self):
        return self.r.uniform(-1.0, 1.0)


def env(t, attack, decay):
    """Linear attack then exponential decay (decay = time to -60 dB)."""
    a = t / attack if attack > 0 and t < attack else 1.0
    return a * math.exp(-6.9 * t / decay)


class OnePole:
    """One-pole low-pass (hp=True: the matching high-pass)."""

    def __init__(self, hz, hp=False):
        self.a = math.exp(-2 * math.pi * hz / SR)
        self.y = 0.0
        self.hp = hp

    def __call__(self, x):
        self.y = (1 - self.a) * x + self.a * self.y
        return x - self.y if self.hp else self.y


class BandPass:
    """Two-pole resonator band-pass (RBJ constant-peak)."""

    def __init__(self, hz, q):
        w = 2 * math.pi * hz / SR
        alpha = math.sin(w) / (2 * q)
        a0 = 1 + alpha
        self.b = (alpha / a0, 0.0, -alpha / a0)
        self.a = (-2 * math.cos(w) / a0, (1 - alpha) / a0)
        self.x1 = self.x2 = self.y1 = self.y2 = 0.0

    def __call__(self, x):
        y = (self.b[0] * x + self.b[2] * self.x2 - self.a[0] * self.y1 - self.a[1] * self.y2)
        self.x2, self.x1, self.y2, self.y1 = self.x1, x, self.y1, y
        return y


def render(seconds, fn):
    return [fn(i / SR) for i in range(int(seconds * SR))]


def kick(base, drop, drop_time, decay, click=0.0, drive=1.0, seconds=None, seed=1):
    n = Noise(seed)
    phase = [0.0]
    lp = OnePole(4000)

    def f(t):
        hz = base + drop * math.exp(-t / drop_time)
        phase[0] += 2 * math.pi * hz / SR
        x = math.sin(phase[0]) * env(t, 0.001, decay)
        x += click * lp(n()) * env(t, 0.0005, 0.012)
        return math.tanh(drive * x) / math.tanh(drive)
    return render(seconds or decay, f)


def snare(tone_hz, tone_decay, noise_decay, noise_hp, tone_mix, seed=2, seconds=None):
    n = Noise(seed)
    hp = OnePole(noise_hp, hp=True)
    lp = OnePole(9000)
    p1, p2 = [0.0], [0.0]

    def f(t):
        p1[0] += 2 * math.pi * tone_hz * (1 + 0.5 * math.exp(-t / 0.01)) / SR
        p2[0] += 2 * math.pi * tone_hz * 1.62 / SR
        tone = (math.sin(p1[0]) + 0.6 * math.sin(p2[0])) * env(t, 0.0005, tone_decay)
        noise = lp(hp(n())) * env(t, 0.0008, noise_decay)
        return tone_mix * tone + (1 - tone_mix) * 1.6 * noise
    return render(seconds or max(tone_decay, noise_decay), f)


METAL = [205.3, 304.4, 369.6, 522.7, 540.0, 800.0]  # the 808's six square oscillators


def metal(decay, hp_hz=7000, seconds=None, noise=0.0, seed=3):
    n = Noise(seed)
    hp1, hp2 = OnePole(hp_hz, hp=True), OnePole(hp_hz, hp=True)
    bp = BandPass(10000, 0.8)
    phases = [0.0] * len(METAL)

    def f(t):
        s = 0.0
        for k, hz in enumerate(METAL):
            phases[k] = (phases[k] + hz * 2.0 / SR) % 1.0
            s += 1.0 if phases[k] < 0.5 else -1.0
        s = s / len(METAL) + noise * n()
        return 1.8 * bp(hp2(hp1(s))) * env(t, 0.0005, decay)
    return render(seconds or decay, f)


def noise_hat(decay, hp_hz=8000, seed=4, seconds=None):
    n = Noise(seed)
    hp1, hp2 = OnePole(hp_hz, hp=True), OnePole(hp_hz, hp=True)

    def f(t):
        return 0.9 * hp2(hp1(n())) * env(t, 0.0005, decay)
    return render(seconds or decay, f)


def clap(seed=5):
    n = Noise(seed)
    bp = BandPass(1200, 1.2)
    bursts = [0.0, 0.011, 0.023, 0.034]

    def f(t):
        e = sum(env(t - b, 0.0005, 0.03) for b in bursts if t >= b)
        tail = env(t - 0.034, 0.001, 0.22) if t >= 0.034 else 0.0
        return 2.5 * bp(n()) * max(e * 0.7, tail)
    return render(0.3, f)


def rim(seed=6):
    n = Noise(seed)
    bp = BandPass(1700, 6)
    p = [0.0]

    def f(t):
        p[0] += 2 * math.pi * 500 / SR
        return (0.6 * math.sin(p[0]) + 3.0 * bp(n())) * env(t, 0.0003, 0.045)
    return render(0.06, f)


def cowbell():
    p = [0.0, 0.0]
    bp = BandPass(700, 1.5)

    def f(t):
        out = 0.0
        for k, hz in enumerate((540.0, 800.0)):
            p[k] = (p[k] + hz / SR) % 1.0
            out += 1.0 if p[k] < 0.5 else -1.0
        e = 0.6 * env(t, 0.0005, 0.06) + 0.4 * env(t, 0.0005, 0.35)
        return 1.2 * bp(out) * e
    return render(0.4, f)


def shaker(seed=7):
    n = Noise(seed)
    bp = BandPass(6500, 1.0)

    def f(t):
        return 2.2 * bp(n()) * env(t, 0.03, 0.12)
    return render(0.15, f)


def crash(seed=8):
    n = Noise(seed)
    hp = OnePole(4000, hp=True)
    m = metal(1.6, hp_hz=5000, noise=0.0, seed=9)

    def f(t):
        return 0.8 * hp(n()) * env(t, 0.001, 1.6)
    return [a + 0.6 * b for a, b in zip(render(1.6, f), m)]


SAMPLES = {
    # 808 family (TR-808, CR-78)
    'kick-808.wav': lambda: kick(48, 70, 0.03, 1.1, click=0.2, drive=1.5),
    'snare-808.wav': lambda: snare(180, 0.12, 0.2, 1500, 0.55),
    'hat-808.wav': lambda: metal(0.06),
    'openhat-808.wav': lambda: metal(0.45),
    # 909 family (TR-909, TR-505)
    'kick-909.wav': lambda: kick(52, 140, 0.02, 0.45, click=0.6, drive=2.2),
    'snare-909.wav': lambda: snare(190, 0.09, 0.28, 900, 0.35),
    'hat-909.wav': lambda: noise_hat(0.05, 8500),
    'openhat-909.wav': lambda: noise_hat(0.4, 7500, seed=10),
    # acoustic family (LinnDrum, break-style kits)
    'kick-acoustic.wav': lambda: kick(58, 90, 0.015, 0.35, click=0.9, drive=2.8),
    'snare-acoustic.wav': lambda: snare(210, 0.08, 0.22, 600, 0.45, seed=11),
    'hat-acoustic.wav': lambda: noise_hat(0.07, 6500, seed=12),
    'openhat-acoustic.wav': lambda: noise_hat(0.55, 6000, seed=13),
    # shared
    'clap.wav': clap,
    'rim.wav': rim,
    'ride.wav': lambda: metal(1.2, hp_hz=4000, noise=0.15, seed=14),
    'crash.wav': crash,
    'shaker.wav': shaker,
    'cowbell.wav': cowbell,
}


def family(name):
    n = name.lower()
    if '808' in n or 'cr-78' in n:
        return '808'
    if '909' in n or '505' in n:
        return '909'
    return 'acoustic'


def sample_for(name):
    """The one-shot of a rack channel, from its name ("Kick (808)", "Snare / Clap (909)",
    "Shaker (CR-78)", ...)."""
    n = name.lower()
    fam = family(name)
    if 'clap' in n:
        return 'clap.wav'
    if 'rim' in n or 'clave' in n:
        return 'rim.wav'
    if 'shaker' in n:
        return 'shaker.wav'
    if 'cowbell' in n:
        return 'cowbell.wav'
    if 'ride' in n:
        return 'ride.wav'
    if 'crash' in n or 'cymbal' in n:
        return 'crash.wav'
    if 'kick' in n:
        return f'kick-{fam}.wav'
    if 'snare' in n:
        return f'snare-{fam}.wav'
    if 'open' in n:
        return f'openhat-{fam}.wav'
    if 'metal' in n:
        return 'hat-808.wav'
    if 'hat' in n:
        return f'hat-{fam}.wav'
    raise SystemExit(f'no one-shot for rack channel "{name}"')


def write(path, xs, peak_db=-1.0):
    peak = max(abs(x) for x in xs) or 1.0
    g = 10 ** (peak_db / 20) / peak
    # 2 ms fade-out so no file ends on a click.
    fade = int(0.002 * SR)
    xs = [x * (min(1.0, (len(xs) - i) / fade)) for i, x in enumerate(xs)]
    with wave.open(path, 'wb') as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(b''.join(struct.pack('<h', int(round(max(-1, min(1, x * g)) * 32767)))
                               for x in xs))


def main():
    os.makedirs(OUT, exist_ok=True)
    for name, make in SAMPLES.items():
        write(os.path.join(OUT, name), make())
    with open(os.path.join(OUT, 'LICENSE.txt'), 'w') as f:
        f.write('These one-shots are synthesized by tools/templates/drum_samples.py (sine and '
                'square waves, filtered seeded noise); no recording is used.\n'
                'Dedicated to the public domain: CC0 1.0 Universal, '
                'https://creativecommons.org/publicdomain/zero/1.0/\n')
    print(f'wrote {len(SAMPLES)} one-shots to {OUT}')


if __name__ == '__main__':
    main()
