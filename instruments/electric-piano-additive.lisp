; Electric Piano (Additive Synthesis)
; Models the interaction of a struck tine and resonating tone bar.
; No FM or subtractive synthesis used.

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
(param decay        @default 2500  @min 100  @max 8000 @unit ms @mod true @mod-mode additive)
(param release      @default 200   @min 10   @max 1000 @unit ms @mod true @mod-mode additive)
(param bark_amt     @default 0.5   @min 0    @max 1    @mod true @mod-mode additive) ; Gain of higher harmonics at high velocity
(param tine_level   @default 0.4   @min 0    @max 1    @mod true @mod-mode additive) ; High frequency "ping"
(param trem_speed   @default 4.5   @min 0.1  @max 15   @unit Hz @mod true @mod-mode additive)
(param trem_depth   @default 0.0   @min 0    @max 1    @mod true @mod-mode additive)
(param drive        @default 0.2   @min 0    @max 1    @mod true @mod-mode additive)
(param gain         @default 0.5   @min 0    @max 1)

; --- Envelopes ---
; Main sustain envelope (percussive decay)
(def env_main (adsr gate trigger 1 (mod decay) 0.1 (mod release)))
; Tine "ping" envelope (very fast)
(def env_tine (adsr gate trigger 1 40 0.0 5))
; Thump envelope (very fast)
(def env_thump (adsr gate trigger 2 15 0.0 1))

; --- Oscillators (Additive partials) ---
; Fundamental
(def osc1 (sin (* twopi (phasor pitch))))
; 2nd Harmonic (Octave)
(def osc2 (sin (* twopi (phasor (* pitch 2.001))))) ; Slight detune for "width"
; 3rd Harmonic (Fifth)
(def osc3 (sin (* twopi (phasor (* pitch 3.0)))))
; 4th Harmonic (Octave 2) - contributes to "bark"
(def osc4 (sin (* twopi (phasor (* pitch 4.002)))))

; High Frequency Tine (Inharmonic strike)
; Typically around 12-15x fundamental
(def tine_osc (sin (* twopi (phasor (* pitch 13.7)))))

; Hammer Thump (Low frequency component)
(def thump_osc (sin (* twopi (phasor 65.0))))

; --- Velocity Scaling ---
; EPs get much brighter/grittier when hit hard.
(def vel_sq (* velocity velocity))
(def bark_scaler (* vel_sq (mod bark_amt)))

; --- Mixing ---
; Base tone
(def sig_base (+ (* osc1 1.0) 
                 (* osc2 0.4)
                 (* osc3 0.1)))

; "Bark" component - triggered more by velocity
(def sig_bark (* osc4 bark_scaler 0.8))

; Tine strike
(def sig_tine (* tine_osc (mod tine_level) env_tine velocity))

; Hammer thump
(def sig_thump (* thump_osc env_thump velocity 0.3))

; Total mix before tremolo/drive
(def raw_mix (+ (* sig_base env_main) (* sig_bark env_main) sig_tine sig_thump))

; --- Nonlinearity (Drive) ---
; Real Rhodes pickups saturate slightly, especially with high bark.
(def drive_sig (tanh (* raw_mix (+ 1.0 (* (mod drive) 3.0)))))

; --- Tremolo ---
(def trem_lfo (+ 0.5 (* 0.5 (sin (* twopi (phasor (mod trem_speed)))))))
(def trem_mod (+ (- 1.0 (mod trem_depth)) (* (mod trem_depth) trem_lfo)))
(def final_sig (* drive_sig trem_mod))

(out (* final_sig (mod gain)) 1 @name audio)
