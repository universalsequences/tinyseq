; 909-style kick drum instrument

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

(param tune         @default 45   @min 30   @max 60    @unit Hz @mod true @mod-mode additive)
(param pitch_depth  @default 120  @min 0    @max 500   @unit Hz @mod true @mod-mode additive)
(param pitch_decay  @default 20   @min 1    @max 100   @unit ms @mod true @mod-mode additive)
(param amp_decay    @default 350  @min 20   @max 1500  @unit ms @mod true @mod-mode additive)
(param click_level  @default 0.2  @min 0    @max 1     @mod true @mod-mode additive)
(param drive        @default 1.5  @min 0.5  @max 4     @mod true @mod-mode additive)
(param gain         @default 0.7  @min 0    @max 1)

; --- Envelopes ---
; Pitch drop envelope (simple exponential decay using adsr with zero sustain)
(def p_env (adsr trigger trigger 0 (mod pitch_decay) 0 10))
; Amp envelope
(def a_env (adsr gate trigger 1 (mod amp_decay) 0 10))

; --- Sound Generation ---
(def base_freq (mod tune))
(def inst_freq (+ base_freq (* p_env (mod pitch_depth))))

; Sine wave for the thump
(def ph (phasor inst_freq))
(def body (sin (* ph 6.2831853)))

; Noise click for the attack (very short envelope)
(def c_env (adsr trigger trigger 1 5 0 5))
(def click (* (noise) c_env (mod click_level)))

; --- Saturation and Output ---
(def mixed (+ body click))
(def saturated (tanh (* mixed (mod drive))))
(def out_sig (* saturated a_env velocity gain))

(out out_sig 1 @name audio)
