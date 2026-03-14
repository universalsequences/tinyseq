; DJ Rashad / Footwork Inspired Chord Stabs
; Soulful, snappy, and harmonically rich.

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

; --- Envelope & Dynamics ---
(param amp_attack_ms    @default 1    @min 1    @max 500 @unit ms)
(param amp_decay_ms     @default 180  @min 1    @max 2000 @unit ms)
(param amp_sustain      @default 0.1  @min 0    @max 1)
(param amp_release_ms   @default 100  @min 1    @max 2000 @unit ms)

(param filt_attack_ms   @default 1    @min 1    @max 500 @unit ms)
(param filt_decay_ms    @default 160  @min 1    @max 2000 @unit ms)
(param filt_sustain     @default 0.0  @min 0    @max 1)
(param filt_release_ms  @default 120  @min 1    @max 2000 @unit ms)

; --- Tone Control ---
(param chord_mode       @default 1    @min 0    @max 2) ; 0=Min7, 1=Min9, 2=Maj7
(param cutoff           @default 600  @min 40   @max 12000 @unit Hz @mod true @mod-mode additive)
(param resonance        @default 2.8  @min 0.5  @max 4.8 @mod true @mod-mode additive)
(param filter_env_amt   @default 4500 @min 0    @max 9000 @unit Hz @mod true @mod-mode additive)
(param filter_env_vel   @default 0.4  @min 0    @max 1) ; Velocity sensitivity for filter pluck
(param detune           @default 0.08 @min 0    @max 0.5 @mod true @mod-mode additive)
(param drive            @default 2.5  @min 1    @max 10 @mod true @mod-mode additive)

; --- Rhythmic Delay ---
(param delay_time_ms    @default 187  @min 10   @max 1000 @unit ms) ; 1/16th or 1/8th triplet vibe
(param delay_fbk        @default 0.35 @min 0    @max 0.9 @mod true @mod-mode additive)
(param delay_mix        @default 0.25 @min 0    @max 1 @mod true @mod-mode additive)
(param gain             @default 0.2  @min 0    @max 1)

; --- Signal Generation ---
(def amp_env (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def filt_env (adsr gate trigger filt_attack_ms filt_decay_ms filt_sustain filt_release_ms))

; Chord Math
(def s0 (+ 0 (* (mod detune) -1)))
(def s1 (+ (mix 3 3 (eq chord_mode 0)) (mix 3 4 (eq chord_mode 2)))) ; 3 for Min, 4 for Maj
(def s2 (+ 7 (* (mod detune) 0.5)))
(def s3 (+ (mix 10 10 (eq chord_mode 0)) (mix 11 11 (eq chord_mode 2)))) ; 10 for Min7, 11 for Maj7
(def s4 (mix 14 0 (lt chord_mode 1))) ; Add 9th for mode 1

; Oscillators
(def v1 (saw (phasor (* pitch (semi_ratio s0)))))
(def v2 (saw (phasor (* pitch (semi_ratio s1)))))
(def v3 (saw (phasor (* pitch (semi_ratio s2)))))
(def v4 (saw (phasor (* pitch (semi_ratio s3)))))
(def v5 (saw (phasor (* pitch (semi_ratio s4)))))

(def mixed (* (+ v1 v2 v3 v4 (* v5 (gt chord_mode 0))) 0.2))

; Filter with Velocity
(def dyn_env_amt (* (mod filter_env_amt) (+ (- 1 filter_env_vel) (* filter_env_vel velocity))))
(def f_cutoff (clip (+ (mod cutoff) (* filt_env dyn_env_amt)) 40 14000))

; 4-Pole Ladder Approximation (Stacked Biquads)
(def lp1 (biquad mixed f_cutoff (mod resonance) 1 0))
(def lp2 (biquad (tanh (* lp1 1.2)) f_cutoff (mod resonance) 1 0))

; Saturation & Amp
(def driven (tanh (* lp2 (mod drive))))
(def voiced (* driven amp_env velocity))

; --- Delay Loop ---
(make-history dl_hist)
(def dl_time_samples (* delay_time_ms 44.1))
(def dl_fbk_sig (read-history dl_hist))
(def dl_in (+ voiced (* dl_fbk_sig (mod delay_fbk))))
(def dl_out (delay dl_in dl_time_samples))
(write-history dl_hist dl_out)

(out (* (mix voiced dl_out (mod delay_mix)) gain) 1 @name audio)
