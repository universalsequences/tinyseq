; 8-bit NES/Gameboy style synth
; Simple pulse, triangle, and noise oscillators with bit-crushing.

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

(defmacro pulse_from_phase (phase width)
  (scale (lt phase width) 0 1 -1 1))

(defmacro crush (sig levels)
  (def safe_levels (max 2 levels))
  (def norm (* (+ sig 1) 0.5))
  (def stepped (/ (floor (* norm safe_levels)) safe_levels))
  (- (* stepped 2) 1))

(param amp_attack   @default 2    @min 1   @max 1000 @unit ms)
(param amp_decay    @default 100  @min 1   @max 2000 @unit ms)
(param amp_sustain  @default 0.5  @min 0   @max 1)
(param amp_release  @default 150  @min 1   @max 3000 @unit ms)

(param pulse_width  @default 0.5  @min 0.05 @max 0.95 @mod true @mod-mode additive)
(param pulse_mix    @default 0.8  @min 0    @max 1)
(param tri_mix      @default 0.2  @min 0    @max 1)
(param noise_mix    @default 0.0  @min 0    @max 1)

(param bit_depth    @default 8    @min 1    @max 16)
(param gain         @default 0.3  @min 0    @max 1)

(def env (adsr gate trigger amp_attack amp_decay amp_sustain amp_release))

(def ph (phasor pitch))
(def p_wave (pulse_from_phase ph (clip (mod pulse_width) 0.05 0.95)))
(def t_wave (triangle ph))
(def n_wave (noise))

(def combined (+ (* p_wave pulse_mix) 
                 (* t_wave tri_mix) 
                 (* n_wave noise_mix)))

(def quantized (crush combined (pow 2 bit_depth)))

(out (* quantized env velocity gain) 1 @name audio)
