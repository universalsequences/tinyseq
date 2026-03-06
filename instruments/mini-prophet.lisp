; Mini Prophet-style polysynth
; Inputs: gate (ch 1), pitch_hz (ch 2), velocity (ch 3)
; Helper input injected at compile time: trigger (ch 4)
; Helper macro injected at compile time: (adsr attack_ms decay_ms sustain release_ms)

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))

(defmacro pulse_from_phase (phase width)
  (scale (lt phase width) 0 1 -1 1))

(param amp_attack_ms   @default 8    @min 1    @max 5000 @unit ms)
(param amp_decay_ms    @default 180  @min 1    @max 5000 @unit ms)
(param amp_sustain     @default 0.72 @min 0    @max 1)
(param amp_release_ms  @default 420  @min 1    @max 5000 @unit ms)

(param filt_attack_ms  @default 12   @min 1    @max 5000 @unit ms)
(param filt_decay_ms   @default 260  @min 1    @max 5000 @unit ms)
(param filt_sustain    @default 0.22 @min 0    @max 1)
(param filt_release_ms @default 520  @min 1    @max 5000 @unit ms)

(param cutoff          @default 420  @min 30   @max 12000 @unit Hz)
(param resonance       @default 1.4  @min 0.3  @max 8)
(param filter_env_amt  @default 1400 @min 0    @max 8000 @unit Hz)
(param keytrack        @default 0.35 @min 0    @max 2)

(param detune_cents    @default 7    @min 0    @max 35 @unit cents)
(param pulse_width     @default 0.46 @min 0.05 @max 0.95)
(param pulse_mix       @default 0.3  @min 0    @max 1)
(param sub_level       @default 0.18 @min 0    @max 1)
(param noise_level     @default 0.015 @min 0   @max 0.25)
(param osc2_level      @default 0.95 @min 0    @max 1)

(param drive           @default 1.4  @min 0.5  @max 4)
(param vibrato_rate    @default 5.2  @min 0.05 @max 12 @unit Hz)
(param vibrato_depth   @default 0.015 @min 0   @max 0.25 @unit st)
(param gain            @default 0.18 @min 0    @max 1)

(def amp_env (adsr amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def filt_env (adsr filt_attack_ms filt_decay_ms filt_sustain filt_release_ms))

(def lfo (sin (* twopi (phasor vibrato_rate))))
(def ln2 (log 2))
(def vibrato_ratio (exp (* ln2 (/ (* lfo vibrato_depth) 12))))
(def detune_ratio (exp (* ln2 (/ detune_cents 1200))))

(def phase1 (phasor (* pitch vibrato_ratio detune_ratio)))
(def phase2 (phasor (/ (* pitch vibrato_ratio) detune_ratio)))
(def phase_sub (phasor (/ pitch 2)))

(def saw1 (scale phase1 0 1 -1 1))
(def saw2 (scale phase2 0 1 -1 1))
(def pulse1 (pulse_from_phase phase1 pulse_width))
(def pulse2 (pulse_from_phase phase2 pulse_width))
(def sub (pulse_from_phase phase_sub 0.5))

(def osc1 (mix saw1 pulse1 pulse_mix))
(def osc2 (mix saw2 pulse2 pulse_mix))
(def osc_mix (+ osc1 (* osc2 osc2_level) (* sub sub_level) (* (noise) noise_level)))

(def voiced (* osc_mix (+ 0.35 (* 0.65 velocity))))
(def driven (tanh (* voiced drive)))
(def filter_cutoff (clip (+ cutoff (* filt_env filter_env_amt) (* pitch keytrack)) 30 10000))
(def filtered (biquad driven filter_cutoff resonance 1 0))
(def body filtered)
(out (* body amp_env velocity gain) 1 @name audio)
