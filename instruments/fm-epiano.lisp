; 4-Operator FM Electric Piano
; Optimized for classic FM "Tine" and "Reed" EP sounds

(def gate     (in 1  @name gate))
(def pitch    (in 2  @name pitch))
(def velocity (in 3  @name velocity))
(def trigger  (in 4  @name trigger))

; -- Modulation Inputs --
(def mod1 (in 5  @name mod1 @modulator 1))
(def mod2 (in 6  @name mod2 @modulator 2))
(def mod3 (in 7  @name mod3 @modulator 3))
(def mod4 (in 8  @name mod4 @modulator 4))
(def mod5 (in 9  @name mod5 @modulator 5))
(def mod6 (in 10 @name mod6 @modulator 6))

; -- Global Envelopes --
(param amp_attack_ms   @default 1    @min 0    @max 100  @unit ms)
(param amp_decay_ms    @default 2000 @min 10   @max 8000 @unit ms)
(param amp_sustain     @default 0.05 @min 0    @max 1)
(param amp_release_ms  @default 300  @min 10   @max 5000 @unit ms)

; -- Operator Ratios (Carrier/Modulator Pairs) --
(param ratio_c1   @default 1.0   @min 0.5  @max 16   @mod true @mod-mode additive) ; Body Carrier
(param ratio_m1   @default 1.0   @min 0.5  @max 16   @mod true @mod-mode additive) ; Body Modulator
(param ratio_c2   @default 1.0   @min 0.5  @max 16   @mod true @mod-mode additive) ; Tine Carrier
(param ratio_m2   @default 14.0  @min 0.5  @max 32   @mod true @mod-mode additive) ; Tine Modulator (Chime)

; -- Modulation Indices --
(param body_mod   @default 1.2   @min 0    @max 10   @mod true @mod-mode additive)
(param tine_mod   @default 4.0   @min 0    @max 12   @mod true @mod-mode additive)
(param tine_mix   @default 0.4   @min 0    @max 1    @mod true @mod-mode additive) ; Balance between Body and Tine

; -- Dynamics --
(param vel_sens   @default 0.7   @min 0    @max 1)    ; How much velocity affects brightness
(param tine_decay @default 250   @min 10   @max 2000 @unit ms) ; Tine brightness decay

; -- Tremolo --
(param trem_rate  @default 4.5   @min 0.1  @max 12   @unit hz @mod true @mod-mode additive)
(param trem_depth @default 0.0   @min 0    @max 1    @mod true @mod-mode additive)

; -- Master --
(param gain       @default 0.3   @min 0    @max 1)

; -- DSP Logic --

; Envelopes
(def env_amp (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def env_tine (adsr gate trigger 1 tine_decay 0 20)) ; Very fast decay for the "tine" hit

; Velocity scaling for timbre (brightness)
(def vel_factor (+ (- 1 vel_sens) (* vel_sens velocity)))

; Operators
(def p1 (phasor (* pitch ratio_c1)))
(def p2 (phasor (* pitch ratio_m1)))
(def p3 (phasor (* pitch ratio_c2)))
(def p4 (phasor (* pitch ratio_m2)))

; Pair 1: Body (Modulator 1 -> Carrier 1)
; We use a subtle decay on the body mod too for realism
(def op_m1 (sin (* twopi p2)))
(def body_env_idx (* body_mod vel_factor (+ 0.2 (* 0.8 env_amp))))
(def op_c1 (sin (+ (* twopi p1) (* op_m1 body_env_idx))))

; Pair 2: Tine (Modulator 2 -> Carrier 2)
; Tine is very percussive, use env_tine
(def op_m2 (sin (* twopi p4)))
(def tine_env_idx (* tine_mod vel_factor env_tine))
(def op_c2 (sin (+ (* twopi p3) (* op_m2 tine_env_idx))))

; Mix
(def sig (+ (* op_c1 (- 1 tine_mix)) (* op_c2 tine_mix env_tine)))

; Tremolo
(def trem_lfo (+ 1 (* (sin (* twopi (phasor trem_rate))) trem_depth)))
(def trem_sig (* sig trem_lfo))

; Final Output
(def final_amp (* trem_sig env_amp velocity gain))
(out (tanh final_amp) 1 @name audio)
