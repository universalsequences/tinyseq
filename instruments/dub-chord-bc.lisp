; Dub Chord Synth - Classic Basic Channel / Chain Reaction Style
; Single note triggers a minor 7th chord with a filtered echo tail.

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger (in 4 @name trigger))
(def mod1 (in 5 @name mod1 @modulator 1))
(def mod2 (in 6 @name mod2 @modulator 2))
(def mod3 (in 7 @name mod3 @modulator 3))
(def mod4 (in 8 @name mod4 @modulator 4))
(def mod5 (in 9 @name mod5 @modulator 5))
(def mod6 (in 10 @name mod6 @modulator 6))

(defmacro semi_ratio (semi)
  (exp (/ (* (log 2) semi) 12)))

(defmacro saw (ph)
  (scale ph 0 1 -1 1))

; --- Parameters ---
(param amp_attack_ms    @default 2    @min 1    @max 1000 @unit ms)
(param amp_decay_ms     @default 120  @min 1    @max 2000 @unit ms)
(param amp_sustain      @default 0.1  @min 0    @max 1)
(param amp_release_ms   @default 400  @min 1    @max 5000 @unit ms)

(param filt_attack_ms   @default 2    @min 1    @max 1000 @unit ms)
(param filt_decay_ms    @default 220  @min 1    @max 2000 @unit ms)
(param filt_sustain     @default 0.05 @min 0    @max 1)
(param filt_release_ms  @default 300  @min 1    @max 5000 @unit ms)

(param cutoff           @default 800  @min 40   @max 10000 @unit Hz @mod true @mod-mode additive)
(param resonance        @default 2.5  @min 0.5  @max 4.5 @mod true @mod-mode additive)
(param filter_env_amt   @default 3500 @min 0    @max 8000 @unit Hz @mod true @mod-mode additive)
(param detune           @default 0.12 @min 0    @max 1)
(param noise_level      @default 0.05 @min 0    @max 0.4)
(param drive            @default 1.5  @min 1    @max 6 @mod true @mod-mode additive)

; Dub Echo Params
(param delay_time_ms    @default 375  @min 10   @max 2000 @unit ms)
(param delay_fbk        @default 0.65 @min 0    @max 0.98 @mod true @mod-mode additive)
(param delay_mix        @default 0.4  @min 0    @max 1 @mod true @mod-mode additive)
(param delay_cutoff     @default 1500 @min 100  @max 8000 @unit Hz)

(param gain             @default 0.15 @min 0    @max 1)

; --- Signal Path ---
(def amp_env (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def filt_env (adsr gate trigger filt_attack_ms filt_decay_ms filt_sustain filt_release_ms))

; Chord Ratios (Minor 7th: 0, 3, 7, 10)
(def detune_mod (* detune 0.05))
(def r0 (semi_ratio (+ 0 (* detune_mod -1))))
(def r1 (semi_ratio (+ 3 (* detune_mod 0.5))))
(def r2 (semi_ratio (+ 7 (* detune_mod -0.3))))
(def r3 (semi_ratio (+ 10 (* detune_mod 1.2))))

; Oscillators
(def chord_sum (+ (saw (phasor (* pitch r0)))
                  (saw (phasor (* pitch r1)))
                  (saw (phasor (* pitch r2)))
                  (saw (phasor (* pitch r3)))))
(def mixed (+ (* chord_sum 0.25) (* (noise) noise_level)))

; Filter
(def f_cutoff (clip (+ (mod cutoff) (* filt_env (mod filter_env_amt))) 40 12000))
(def filtered (biquad mixed f_cutoff (mod resonance) 1 0))

; Saturation
(def driven (tanh (* filtered (mod drive))))
(def voiced (* driven amp_env))

; --- Dub Echo (Integrated Delay) ---
(make-history delay_hist)
(def delay_time_samples (* delay_time_ms 44.1))
(def feedback_sig (read-history delay_hist))
; Filter the feedback loop (classic dub technique)
(def filtered_feedback (biquad feedback_sig delay_cutoff 0.7 1 0))
(def delay_in (+ voiced (* filtered_feedback (mod delay_fbk))))
(def delayed_sig (delay delay_in delay_time_samples))
(write-history delay_hist delayed_sig)

(def final_mix (mix voiced delayed_sig (mod delay_mix)))

(out (* final_mix gain) 1 @name audio)
