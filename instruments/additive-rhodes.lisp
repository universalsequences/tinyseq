; Additive Rhodes-style Electric Piano
; Simulates the sound of struck tines and resonating tone bars using pure sine summation.

(def gate     (in 1 @name gate))
(def pitch    (in 2 @name pitch))
(def velocity (in 3 @name velocity))
(def trigger  (in 4 @name trigger))

; --- Global Parameters ---
(param gain          @default 0.15 @min 0    @max 1   @mod true @mod-mode additive)
(param attack        @default 2    @min 1    @max 100 @unit ms)
(param decay         @default 1200 @min 10   @max 5000 @unit ms @mod true @mod-mode additive)
(param sustain       @default 0.0  @min 0    @max 1)
(param release       @default 250  @min 10   @max 2000 @unit ms)

; --- Timbre Parameters ---
(param tine_vol      @default 0.4  @min 0    @max 1   @mod true @mod-mode additive) ; The high "ping"
(param harmonic_2    @default 0.6  @min 0    @max 1   @mod true @mod-mode additive) ; The body thickness
(param harmonic_4    @default 0.3  @min 0    @max 1   @mod true @mod-mode additive) ; The "bark"
(param detune        @default 0.05 @min 0    @max 0.5 @mod true @mod-mode additive) ; Slight beating

; --- Suitcase Vibrato (Pan) ---
(param vib_speed     @default 4.5  @min 0.1  @max 15  @unit hz @mod true @mod-mode additive)
(param vib_depth     @default 0.6  @min 0    @max 1   @mod true @mod-mode additive)

; --- Synthesis Logic ---

; Velocity curves
(def vel_sq (* velocity velocity))
(def bark_amt (+ 0.2 (* 0.8 vel_sq))) ; Louder/brighter harmonics at high velocity

; Envelopes
(def main_env (adsr gate trigger attack (mod decay) sustain release))
(def tine_env (adsr gate trigger 1 80 0 10)) ; Fast strike decay

; Phasers
(def ph1 (phasor pitch))
(def ph2 (phasor (* pitch (+ 2.0 (* (mod detune) 0.01)))))
(def ph4 (phasor (* pitch (+ 4.0 (* (mod detune) 0.02)))))
(def ph_tine (phasor (* pitch 14.5)))

; Summing the Partials
; Fundamental (Body)
(def partial_1 (sin (* twopi ph1)))
; 2nd Harmonic (Warmth)
(def partial_2 (* (sin (* twopi ph2)) (mod harmonic_2)))
; 4th Harmonic (Bark - scaled by velocity)
(def partial_4 (* (sin (* twopi ph4)) (mod harmonic_4) bark_amt))
; Tine Strike (Metallic Transient)
(def partial_tine (* (sin (* twopi ph_tine)) (mod tine_vol) tine_env bark_amt))

(def mixed (+ partial_1 partial_2 partial_4 partial_tine))

; Apply main envelope and gain
(def final_mono (* mixed main_env (mod gain) velocity))

; Stereo Suitcase Vibrato (Auto-pan)
(def lfo_val (sin (* twopi (phasor (mod vib_speed)))))
(def pan_mod (* lfo_val (mod vib_depth)))

; Stereo Output
(out (* final_mono (- 1.0 (* 0.5 (+ 1.0 pan_mod)))) 1 @name left)
(out (* final_mono (+ 1.0 (* 0.5 (+ 1.0 pan_mod)))) 2 @name right)
