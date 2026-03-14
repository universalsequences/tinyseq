; Atmospheric Jungle Pad/Chord
; Inspired by 90s intelligent D&B (LTJ Bukem, Omni Trio).
; Lush, multi-oscillator chords with slow, evolving filters.

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

(defmacro tri (ph)
  (triangle ph))

; --- Parameters ---
(param amp_attack_ms    @default 80   @min 1    @max 2000 @unit ms)
(param amp_decay_ms     @default 1200 @min 1    @max 5000 @unit ms)
(param amp_sustain      @default 0.85 @min 0    @max 1)
(param amp_release_ms   @default 1500 @min 1    @max 5000 @unit ms)

(param filt_attack_ms   @default 400  @min 1    @max 4000 @unit ms)
(param filt_decay_ms    @default 2000 @min 1    @max 8000 @unit ms)
(param filt_sustain     @default 0.4  @min 0    @max 1)
(param filt_release_ms  @default 2000 @min 1    @max 8000 @unit ms)

(param cutoff           @default 1200 @min 40   @max 8000 @unit Hz @mod true @mod-mode additive)
(param resonance        @default 1.2  @min 0.35 @max 4.0  @mod true @mod-mode additive)
(param filter_env_amt   @default 2500 @min 0    @max 6000 @unit Hz @mod true @mod-mode additive)

(param chord_type       @default 1    @min 0    @max 2) ; 0: Maj7, 1: Min9, 2: Min11
(param detune_width     @default 0.25 @min 0    @max 1 @mod true @mod-mode additive)
(param lfo_rate         @default 0.3  @min 0.01 @max 5 @unit Hz)
(param lfo_depth        @default 400  @min 0    @max 2000 @unit Hz)

(param noise_grit       @default 0.04 @min 0    @max 0.2)
(param drive            @default 1.2  @min 1    @max 3)
(param gain             @default 0.12 @min 0    @max 1)

; --- Signal Path ---
(def amp_env (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def filt_env (adsr gate trigger filt_attack_ms filt_decay_ms filt_sustain filt_release_ms))

; LFO for movement
(def mod_lfo (sin (* twopi (phasor lfo_rate))))

; Chord Voicings
; Maj7: 0, 4, 7, 11
; Min9: 0, 3, 7, 10, 14
; Min11: 0, 3, 7, 10, 14, 17
(def d_amt (* (mod detune_width) 0.15))

(def r0 (semi_ratio (+ 0 (* d_amt -1.2))))
(def r1 (mix (semi_ratio (+ 4 (* d_amt 0.8)))  (semi_ratio (+ 3 (* d_amt 0.7)))  (gt chord_type 0.5)))
(def r2 (semi_ratio (+ 7 (* d_amt -0.5))))
(def r3 (mix (semi_ratio (+ 11 (* d_amt 1.1))) (semi_ratio (+ 10 (* d_amt 1.3))) (gt chord_type 0.5)))
(def r4 (semi_ratio (+ 14 (* d_amt -0.9)))) ; The 9th

; Oscillators (Stacked Saws and Tris for smoothness)
(def o0 (+ (saw (phasor (* pitch r0))) (* 0.5 (tri (phasor (* pitch r0 0.999))))))
(def o1 (+ (saw (phasor (* pitch r1))) (* 0.5 (tri (phasor (* pitch r1 1.001))))))
(def o2 (+ (saw (phasor (* pitch r2))) (* 0.5 (tri (phasor (* pitch r2 0.998))))))
(def o3 (+ (saw (phasor (* pitch r3))) (* 0.5 (tri (phasor (* pitch r3 1.002))))))
(def o4 (* (saw (phasor (* pitch r4))) (gt chord_type 0.5))) ; Add 9th if Min9/11

(def mixed (+ (* (+ o0 o1 o2 o3 o4) 0.2) (* (noise) noise_grit)))

; Filter with LFO movement
(def f_cutoff (clip (+ (mod cutoff) 
                       (* filt_env (mod filter_env_amt))
                       (* mod_lfo lfo_depth)) 
                    40 10000))

; 24dB-ish Ladder (Cascaded)
(def lp1 (biquad mixed f_cutoff (mod resonance) 1 0))
(def lp2 (biquad lp1 (* f_cutoff 1.1) (* (mod resonance) 0.8) 1 0))

(def voiced (* (tanh (* lp2 drive)) amp_env))

(out (* voiced gain) 1 @name audio)
