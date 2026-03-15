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
(param hardness     @default 0.5  @min 0.0  @max 1.0   @mod true @mod-mode additive)
(param tine_decay   @default 3.0  @min 0.5  @max 8.0   @unit s   @mod true @mod-mode additive)
(param tone         @default 0.6  @min 0.1  @max 0.95  @mod true @mod-mode additive)
(param pickup_bark  @default 0.4  @min 0.0  @max 1.0   @mod true @mod-mode additive)
(param tremolo_rate @default 5.0  @min 0.5  @max 12.0  @unit hz)
(param trem_depth   @default 0.2  @min 0.0  @max 1.0   @mod true @mod-mode additive)
(param gain         @default 0.3  @min 0.0  @max 1.0)

; --- Exciter: Hammer Impact ---
; Fast envelope for the strike
(def strike_env (adsr gate trigger 0.5 25 0.0 10))
; Metal \"clink\" (high frequency noise) + \"thump\" (low frequency pulse)
(def clink      (biquad (noise) 8000 0.5 1 2)) ; Bandpass
(def hammer     (+ (* clink (mod hardness)) (sin (* 3.14159 (phasor (+ 50 (* 200 (mod hardness))))))))
(def exciter    (* hammer strike_env velocity))

; --- Tine Model (Main Waveguide) ---
(def safe_pitch (max 20.0 pitch))
(def delay_len  (/ 44100.0 safe_pitch))

; One-pole LP for the loop damping
(def lp_freq    (+ 1000.0 (* (mod tone) 14000.0)))
(def lp_g       (exp (/ (* -6.283 lp_freq) 44100.0)))

; Feedback coefficient for T60 decay
(def stretch    (min 0.9999 (pow 0.001 (/ 1.0 (* (mod tine_decay) safe_pitch)))))

; Feedback Loop
(make-history loop_hist)
(make-history lp_hist)

(def feedback_in (read-history loop_hist))
(def loop_node   (+ exciter (* feedback_in stretch)))
(def dly_sig     (delay loop_node (max 1.0 (- delay_len 1.0))))

; LP Filter: y = x(1-g) + y_prev(g)
(def lp_out      (+ (* dly_sig (- 1.0 lp_g)) (* (read-history lp_hist) lp_g)))
(write-history lp_hist lp_out)
(write-history loop_hist lp_out)

; --- Overtone (The \"Ping\") ---
; Tines have a strong inharmonic 6th-ish harmonic
(def overtone_pitch (* safe_pitch 6.27))
(def ot_delay       (/ 44100.0 overtone_pitch))
(make-history ot_loop_hist)
(def ot_feedback    (read-history ot_loop_hist))
(def ot_node        (+ (* exciter 0.5) (* ot_feedback 0.95))) ; Shorter decay
(def ot_out         (delay ot_node (max 1.0 ot_delay)))
(write-history ot_loop_hist ot_out)

; --- Mix and Pickup Saturation ---
(def raw_mix    (+ lp_out (* ot_out 0.3)))
; Tines \"bark\" when hit hard due to pickup proximity
(def drive      (+ 1.0 (* (mod pickup_bark) velocity 5.0)))
(def pickup_out (tanh (* raw_mix drive)))

; --- Tremolo & Output ---
(def trem_lfo   (+ 0.7 (* 0.3 (sin (* 6.283 (phasor tremolo_rate))))))
(def final_sig  (mix pickup_out (* pickup_out trem_lfo) (mod trem_depth)))

; DC Block
(make-history dc_hist)
(def dc_lp (mix final_sig (read-history dc_hist) 0.998))
(write-history dc_hist dc_lp)

(out (* (- final_sig dc_lp) gain) 1 @name audio)