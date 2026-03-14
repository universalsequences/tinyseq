; Orchestral Strings - Physical Modeling (Bowed String)
; Uses a sustained noise exciter through a feedback delay line

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

(param bow_pressure    @default 0.5  @min 0    @max 1    @mod true @mod-mode additive)
(param bow_attack_ms   @default 150  @min 10   @max 1000 @unit ms)
(param brightness      @default 0.6  @min 0    @max 1    @mod true @mod-mode additive)
(param decay_time_ms   @default 1500 @min 100  @max 8000 @unit ms @mod true @mod-mode additive)
(param vibrato_rate    @default 5.5  @min 0    @max 10   @unit Hz)
(param vibrato_depth   @default 0.15 @min 0    @max 2    @mod true @mod-mode additive)
(param ensemble        @default 0.3  @min 0    @max 1    @mod true @mod-mode additive)
(param gain            @default 0.2  @min 0    @max 1)

; ── Vibrato LFO (Sine via phasor) ──
(def vib_lfo (sin (* 6.28318 (phasor vibrato_rate))))
(def pitch_mod (+ pitch (* (mod vibrato_depth) 0.02 vib_lfo pitch)))

; ── Bow Exciter ──
; Sustained noise burst for bowing
(def bow_env (adsr gate trigger bow_attack_ms 200 0.8 200))
(def bow_noise (biquad (noise) 2000 0.7 1 1)) ; HPF noise to remove DC
(def exciter (* bow_noise bow_env (mod bow_pressure) velocity))

; ── Karplus-Strong Feedback Loop ──
(def sr 44100.0)
(def ks_delay (max 1.0 (- (/ sr (max 20.0 pitch_mod)) 1.5)))

; Damping / Decay
(def decay_time_s (/ (mod decay_time_ms) 1000.0))
(def stretch (min 0.999 (exp (/ -1.0 (* (max 20.0 pitch_mod) decay_time_s)))))
(def bright_coef (clip (mod brightness) 0.01 0.99))

(make-history ks_buf)
(make-history ks_lp_prev)
(def ks_delayed (delay (read-history ks_buf) ks_delay))
(def ks_avg (+ (* 0.5 ks_delayed) (* 0.5 (read-history ks_lp_prev))))
(write-history ks_lp_prev ks_delayed)
(def ks_lp (mix ks_avg ks_delayed bright_coef))
(def ks_next (+ exciter (* ks_lp stretch)))
(write-history ks_buf ks_next)

; ── Second String (Ensemble) ──
(def ens_detune (* (mod ensemble) 0.008 (sin (* 6.28318 (phasor 0.3)))))
(def ks2_delay (max 1.0 (- (/ sr (max 20.0 (* pitch_mod (+ 1.0 ens_detune)))) 1.5)))
(make-history ks_buf2)
(make-history ks_lp_prev2)
(def ks2_delayed (delay (read-history ks_buf2) ks2_delay))
(def ks2_avg (+ (* 0.5 ks2_delayed) (* 0.5 (read-history ks_lp_prev2))))
(write-history ks_lp_prev2 ks2_delayed)
(def ks2_lp (mix ks2_avg ks2_delayed bright_coef))
(def ks2_next (+ exciter (* ks2_lp stretch)))
(write-history ks_buf2 ks2_next)

(def mono_mix (mix ks_next ks2_next (* 0.5 (mod ensemble))))

; ── Body Resonances (Approximate violin body) ──
(def b1 (biquad mono_mix 280 1.2 1 2)) ; Bandpass
(def b2 (biquad mono_mix 450 1.5 1 2))
(def b3 (biquad mono_mix 1200 1.0 1 2))
(def body_mix (+ (* mono_mix 0.6) (* 0.2 (+ b1 b2 b3))))

(out (* body_mix (adsr gate trigger 10 10 1.0 100) gain) 1 @name audio)
