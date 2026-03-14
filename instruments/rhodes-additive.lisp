; Rhodes Suitcase Electric Piano
; Pure Additive Synthesis (No FM or Subtractive)
; Modelled after the physics of struck tines and resonating tone bars

(def gate     (in 1  @name gate))
(def pitch    (in 2  @name pitch))
(def velocity (in 3  @name velocity))
(def trigger  (in 4  @name trigger))

; Modulator inputs required by the sequencer for @mod parameters
(def mod1     (in 5  @name mod1 @modulator 1))
(def mod2     (in 6  @name mod2 @modulator 2))
(def mod3     (in 7  @name mod3 @modulator 3))
(def mod4     (in 8  @name mod4 @modulator 4))
(def mod5     (in 9  @name mod5 @modulator 5))
(def mod6     (in 10 @name mod6 @modulator 6))

; --- Sound Shaping Parameters ---
(param decay      @default 1200 @min 100  @max 5000 @unit ms @mod true @mod-mode additive)
(param tine_vol   @default 0.25 @min 0    @max 1    @mod true @mod-mode additive)
(param harmonic_2 @default 0.40 @min 0    @max 1    @mod true @mod-mode additive)
(param harmonic_4 @default 0.20 @min 0    @max 1    @mod true @mod-mode additive) ; The "Bark" partial
(param bark_amt   @default 0.50 @min 0    @max 1)   ; Velocity sensitivity for H4
(param detune     @default 0.05 @min 0    @max 0.5  @mod true @mod-mode additive)
(param gain       @default 0.25 @min 0    @max 1    @mod true @mod-mode additive)

; --- Suitcase Stereo Pan (Vibrato) ---
(param vib_speed  @default 4.5  @min 0.1  @max 12   @unit Hz)
(param vib_depth  @default 0.6  @min 0    @max 1    @mod true @mod-mode additive)

; --- Envelopes ---
; Main amplitude envelope for the tone bar resonance
(def amp_env (adsr gate trigger 2 (mod decay) 0.0 150))

; Very fast decay for the initial metal tine strike
(def tine_env (adsr gate trigger 1 25 0.0 1))

; --- Additive Engine ---
(def fundamental (sin (* twopi (phasor pitch))))

; Harmonic 2 (Octave)
(def h2 (sin (* twopi (phasor (* pitch (+ 2.0 (* (mod detune) 0.01)))))))

; Harmonic 4 (Two octaves up) - Drives the "Bark" sound
; It becomes more prominent and slightly saturated at high velocities
(def vel_bark (pow velocity (+ 1.0 (- 1.0 bark_amt))))
(def h4_raw (sin (* twopi (phasor (* pitch (+ 4.0 (* (mod detune) 0.02)))))))
(def h4 (* h4_raw vel_bark (mod harmonic_4)))

; Metallic Tine Strike (High frequency, non-harmonic)
(def tine_freq (* pitch 14.5))
(def tine_osc (sin (* twopi (phasor tine_freq))))
(def tine_sig (* tine_osc tine_env (mod tine_vol) velocity))

; --- Mix ---
(def tone_bar (+ fundamental (* h2 (mod harmonic_2)) h4))
(def dry_sig  (+ (* tone_bar amp_env velocity) tine_sig))

; Subtle saturation to mimic the Rhodes preamp when played hard
(def driven (tanh (* dry_sig (+ 1.0 (* velocity bark_amt 2.0)))))

; --- Suitcase Stereo Auto-Pan ---
(def pan_lfo (* 0.5 (+ 1.0 (sin (* twopi (phasor vib_speed))))))
(def pan_depth (mod vib_depth))

(def left_gain  (+ (- 1.0 pan_depth) (* pan_depth (- 1.0 pan_lfo))))
(def right_gain (+ (- 1.0 pan_depth) (* pan_depth pan_lfo)))

(out (* driven left_gain (mod gain))  1 @name left)
(out (* driven right_gain (mod gain)) 2 @name right)
