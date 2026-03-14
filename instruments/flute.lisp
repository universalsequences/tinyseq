; Waveguide flute — faithful translation of reference patch
;
; Signal flow:
;   gate → one-pole envelope (h7)
;   jet_in = env * (pressure + noise*noise_amt + bore_prev*coupling)  ← gated so note-off stops excitation
;   jet_del = delay(jet_in, period * jetRatio)           ← embouchure section
;   jet_nl  = jet_del - jet_del³                         ← jet nonlinearity
;   allpass1: +1 = (clip(jet_nl,-1,1) + bore_prev*reflection - h3) + h2*loop_gain
;   brightness LP: mix2 = mix(+1, h4, 0.95 - brightness*0.9)
;   bore delay: delay2 = delay(mix2, period*(1-jetRatio)) → written to h5 (bore_prev)
;   allpass2: +2 = (delay2*0.5 - h6) + h1*loop_gain
;   output = +2

(def gate     (in 1  @name gate))
(def pitch    (in 2  @name pitch))
(def velocity (in 3  @name velocity))
(def trigger  (in 4  @name trigger))
(def mod1     (in 5  @name mod1 @modulator 1))
(def mod2     (in 6  @name mod2 @modulator 2))
(def mod3     (in 7  @name mod3 @modulator 3))
(def mod4     (in 8  @name mod4 @modulator 4))
(def mod5     (in 9  @name mod5 @modulator 5))
(def mod6     (in 10 @name mod6 @modulator 6))

(param attack     @default 100  @min 1    @max 1000 @unit ms)
  ; Gate envelope attack time
(param release    @default 200  @min 1    @max 2000 @unit ms)
  ; Gate envelope release time
(param vibRate    @default 5    @min 0.1  @max 10   @unit Hz  @mod true @mod-mode additive)
  ; Vibrato rate
(param vibDepth   @default 0.5  @min 0    @max 20   @unit Hz  @mod true @mod-mode additive)
  ; Vibrato depth in Hz — 0=none, keep low (0.5-2Hz) for subtle, higher for expressive
(param jetRatio   @default 0.5  @min 0.1  @max 0.9)
  ; Jet delay as fraction of period: 0.1=bright/tight, 0.9=dark/loose
(param pressure   @default 0.8  @min 0.1  @max 1.5  @mod true @mod-mode additive)
  ; Breath pressure — higher = more driven, can overblow
(param noise_amt  @default 0.1  @min 0    @max 0.5  @mod true @mod-mode additive)
  ; Breath turbulence — more = airier/breathier attack
(param coupling   @default 0.3  @min 0.0  @max 0.7)
  ; Bore→jet coupling: controls how much the bore feeds back into excitation
(param reflection @default 0.85 @min 0.5  @max 0.99)
  ; End reflection coefficient — higher = more resonant/sustained
(param brightness @default 0.5  @min 0.0  @max 0.9  @mod true @mod-mode additive)
  ; Bore LP brightness: 0=dark/woody, 0.9=bright/airy
(param loop_gain  @default 0.982 @min 0.9  @max 0.999)
  ; Allpass loop decay: 0.9=fast decay, 0.999=infinite sustain (wild/drone mode)
(param gain       @default 0.22 @min 0    @max 1)

; ── Histories ──
(make-history h7)  ; smoothed gate envelope
(make-history h5)  ; bore delay output (coupling source)
(make-history h3)  ; allpass 1 input state
(make-history h2)  ; allpass 1 output state
(make-history h4)  ; brightness LP state
(make-history h6)  ; allpass 2 input state
(make-history h1)  ; allpass 2 output state

; ── Gate envelope: asymmetric one-pole smoother ──
; coeff close to 1 = slow (large attack/release ms), close to 0 = fast
(def env_prev  (read-history h7))
(def att_c     (exp (/ -1.0 (max 1.0 (* attack 44.1)))))
(def rel_c     (exp (/ -1.0 (max 1.0 (* release 44.1)))))
(def env       (mix gate env_prev (gswitch (gt gate env_prev) att_c rel_c)))

; ── Pitch with vibrato (vibDepth in Hz, added directly to frequency) ──
(def vib_sig   (* (sin (* twopi (phasor (mod vibRate)))) (mod vibDepth)))
(def pitch_hz  (max 20.0 (+ pitch vib_sig)))
(def period    (/ 44100.0 pitch_hz))

; ── Jet excitation ──
; All terms gated by env: when gate closes, energy input stops and bore rings down
(def bore_prev (read-history h5))
(def jet_in    (* env (+ pressure
                         (* (noise) noise_amt)
                         (* bore_prev coupling))))

; Jet section: embouchure-to-bore delay
(def jet_del   (delay jet_in (* period jetRatio)))

; Jet nonlinearity: x − x³  (cubic waveguide jet function)
(def jet_nl    (- jet_del (* jet_del jet_del jet_del)))

; ── First allpass bore section (open-end reflection + DC block) ──
(def ap1_in    (+ (clip jet_nl -1 1) (* bore_prev reflection)))
(def +1        (+ (- ap1_in (read-history h3)) (* (read-history h2) loop_gain)))

; ── Brightness: one-pole LP in bore ──
; coeff = 0.95 - brightness*0.9: higher coeff = darker (more LP)
(def mix2      (mix +1 (read-history h4) (- 0.95 (* brightness 0.9))))

; ── Bore back-propagation delay ──
(def delay2    (delay mix2 (* period (- 1.0 jetRatio))))

; DC block inside the loop: strips low-end before bore_prev feeds back into jet and allpass1
; y = x - lpf(x, 0.999) — one-pole HP at the source, keeps headroom clean
(make-history h_dc)
(def delay2_dc (- delay2 (write-history h_dc (mix delay2 (read-history h_dc) 0.999))))

; ── Second allpass bore section ──
(def +2        (+ (- (* delay2_dc 0.5) (read-history h6)) (* (read-history h1) loop_gain)))

; Velocity scaling on output
(def vel_scale (+ 0.4 (* 0.6 velocity)))

(out (* +2 vel_scale gain) 1 @name audio)

; ── Write histories ──
(write-history h7 env)
(write-history h5 delay2_dc)
(write-history h3 ap1_in)
(write-history h2 +1)
(write-history h4 mix2)
(write-history h6 (* delay2 0.5))
(write-history h1 +2)
