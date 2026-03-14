; Physical model of a saxophone (Waveguide synthesis)
; Refined reed-excited conical bore model

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
(param reed_stiffness   @default 0.85  @min 0.4  @max 1.5   @mod true @mod-mode additive)
(param brightness       @default 0.7   @min 0.2  @max 0.98  @mod true @mod-mode additive)
(param noise_amt        @default 0.05  @min 0.0  @max 0.2)
(param growl_amt        @default 0.0   @min 0.0  @max 1.0   @mod true @mod-mode additive)
(param vib_depth        @default 0.1   @min 0.0  @max 1.0)
(param gain             @default 0.3   @min 0.0  @max 1.0)

; --- LFOs ---
; 5.5Hz Vibrato
(def vib_lfo (sin (* 2 3.14159 (phasor 5.5))))
(def pitch_mod (+ pitch (* vib_lfo vib_depth 5.0))) ; +/- 5Hz vibrato

; 35Hz Growl
(def growl_lfo (+ 0.7 (* 0.3 (sin (* 2 3.14159 (phasor 35.0))))))

; --- Excitation (Breath) ---
(def env (adsr gate trigger 40 200 0.8 150))
(def breath_noise (* (noise) noise_amt))
; Breath pressure: combine env, growl, and noise
(def breath_press (* env (mix 1.0 growl_lfo (mod growl_amt))))
(def total_breath (+ breath_press breath_noise))

; --- Waveguide (Bore) ---
; Delay length for a conical bore is a full wavelength (c/f)
(def delay_samples (max 2.0 (/ 44100.0 (max 20.0 pitch_mod))))

(make-history bore_buf)
(def loop_back (delay (read-history bore_buf) delay_samples))

; --- Reed Model (The Engine) ---
; The reed is a pressure-controlled valve. 
; Differential pressure = breath - feedback
(def p_diff (- total_breath (* loop_back 0.9))) ; 0.9 reflection coeff

; Reed table approximation: y = x - x^3
; This creates the non-linear oscillation.
; Stiffness scales the input to the non-linearity.
(def x (* p_diff (mod reed_stiffness)))
(def reed_val (clip (- x (* x (* x x))) -1.0 1.0))

; --- The Loop ---
; Mix the reed excitation with the reflected wave
(def p_wave (+ (* loop_back 0.95) (* reed_val 0.4)))

; Low-pass filter simulates the damping of high frequencies in the bore
(def bore_lp (biquad p_wave (* (mod brightness) 10000.0) 0.5 1 0))

(write-history bore_buf bore_lp)

; --- Output Stage ---
; High-pass filter at 200Hz to simulate bell radiation and remove DC offset
(def out_sig (biquad bore_lp 200 0.707 1 1))

(out (* out_sig gain velocity) 1 @name audio)
