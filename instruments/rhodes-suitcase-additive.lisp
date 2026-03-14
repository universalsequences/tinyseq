; Rhodes Suitcase Style Electric Piano (Additive Synthesis)
; Models the interaction of a struck tine and resonating tone bar.
; Uses additive oscillators with velocity-controlled "bark".
; Stereo auto-pan included for authentic Suitcase model sound.

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

; --- Parameters ---
(param decay        @default 2800  @min 100  @max 8000 @unit ms @mod true @mod-mode additive)
(param bark_amt     @default 0.65  @min 0    @max 1    @mod true @mod-mode additive) ; High velocity growl
(param tine_ping    @default 0.35  @min 0    @max 1    @mod true @mod-mode additive) ; Strike sharpness
(param vibrato_spd  @default 5.0   @min 0.1  @max 15   @unit Hz @mod true @mod-mode additive)
(param vibrato_dep  @default 0.4   @min 0    @max 1    @mod true @mod-mode additive) ; Auto-pan depth
(param detune       @default 0.05  @min 0    @max 0.2  @mod true @mod-mode additive) ; Beating between tine/bar
(param gain         @default 0.35  @min 0    @max 1)

; --- Envelopes ---
; Percussive decay for sustain
(def env_main (adsr gate trigger 1 (mod decay) 0.05 150))
; Fast decay for the initial strike
(def env_strike (adsr gate trigger 1 35 0.0 5))

; --- Additive Partials ---
(def dt (mod detune))
; 1: Fundamental (The resonant tone bar)
(def bar1 (sin (* twopi (phasor pitch))))
; 2: First harmonic (The tine vibration) - slightly detuned for "growl"
(def tine1 (sin (* twopi (phasor (* pitch (+ 2.0 (* dt 0.1)))))))
; 3: Second harmonic (More bark)
(def tine2 (sin (* twopi (phasor (* pitch (+ 4.0 (* dt 0.2)))))))
; 4: High frequency metallic ping (Tine strike)
(def tine_high (sin (* twopi (phasor (* pitch (+ 14.5 dt))))))

; --- Velocity Mapping ---
(def vel_curve (* velocity velocity)) ; Exponential velocity for "bark"
(def bark_gain (* vel_curve (mod bark_amt)))

; --- Summation (Additive) ---
; Combine the sustain components
(def sustain_mix (+ (* bar1 0.8) 
                    (* tine1 0.4) 
                    (* tine2 bark_gain)))

; Strike component (Decays quickly)
(def strike_mix (* tine_high (mod tine_ping) env_strike velocity))

; Combined Mono Signal
(def mono_sig (+ (* sustain_mix env_main) strike_mix))

; --- Soft Drive (Pickup Saturation) ---
; Tanh provides a nice warm saturation similar to vintage pickups.
(def driven (tanh (* mono_sig (+ 1.0 (* (mod bark_amt) 2.0)))))

; --- Stereo Auto-Pan (Suitcase Vibrato) ---
(def pan_lfo (sin (* twopi (phasor (mod vibrato_spd)))))
(def depth   (mod vibrato_dep))

; Calculate Left/Right gains based on pan depth
(def gain_l (+ (- 1.0 (* depth 0.5)) (* depth 0.5 pan_lfo)))
(def gain_r (+ (- 1.0 (* depth 0.5)) (* depth 0.5 (* -1.0 pan_lfo))))

(out (* driven gain_l (mod gain)) 1 @name left)
(out (* driven gain_r (mod gain)) 2 @name right)
