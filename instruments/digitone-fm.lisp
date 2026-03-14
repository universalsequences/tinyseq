; Digitone-style 4-operator FM Synthesizer
; Operators: C (Carrier), A, B1, B2 (Modulators)
; Features: Resonant Filter, Overdrive, FM Algorithms

(def gate     (in 1  @name gate))
(def pitch    (in 2  @name pitch))
(def velocity (in 3  @name velocity))
(def trigger  (in 4  @name trigger))

; --- Global Amp Envelope ---
(param amp_a  @default 2    @min 0    @max 5000 @unit ms)
(param amp_d  @default 400  @min 1    @max 5000 @unit ms)
(param amp_s  @default 0.6  @min 0    @max 1)
(param amp_r  @default 200  @min 1    @max 5000 @unit ms)

; --- FM Parameters ---
(param algo      @default 1    @min 1    @max 8)
(param harm      @default 1.0  @min 0.25 @max 16) ; Harmonics offset
(param ratio_a   @default 1.0  @min 0.25 @max 16  @mod true)
(param ratio_b   @default 2.0  @min 0.25 @max 16  @mod true)
(param ratio_c   @default 1.0  @min 0.25 @max 16  @mod true)

(param lev_a     @default 0.5  @min 0    @max 4   @mod true)
(param lev_b     @default 0.2  @min 0    @max 4   @mod true)
(param feedback  @default 0.0  @min 0    @max 2   @mod true)

; --- Filter & Drive ---
(param cutoff    @default 5000 @min 20   @max 18000 @unit hz @mod true)
(param resonance @default 0.2  @min 0    @max 0.95)
(param drive     @default 0.1  @min 0    @max 5)

; --- Envelopes ---
(def a_env (adsr gate trigger 5 250 0.1 100))
(def b_env (adsr gate trigger 10 500 0.2 200))
(def m_env (adsr gate trigger amp_a amp_d amp_s amp_r))

; --- Oscillators & Feedback ---
(make-history fb_hist)
(def fb_val (read-history fb_hist))

; Ratios multiplied by harmonics
(def f_a (* pitch ratio_a harm))
(def f_b (* pitch ratio_b harm))
(def f_c (* pitch ratio_c harm))

(def ph_a (phasor f_a))
(def ph_b (phasor f_b))
(def ph_c (phasor f_c))

; Effective levels with envelopes
(def gain_a (* lev_a a_env))
(def gain_b (* lev_b b_env))

; --- Algorithms ---
; 1: A -> B -> C (Cascade)
; 2: (A + B) -> C (Dual modulator)
; 3: A -> C + B -> C (Parallel modulators)
; 4: A -> B & B -> C (Chain + Split)
; For simplicity, let's implement 4 core ones first.

; Algo 1: Cascade A -> B -> C
(def op_a1 (sin (+ (* twopi ph_a) (* fb_val feedback))))
(def op_b1 (sin (+ (* twopi ph_b) (* op_a1 gain_a))))
(def op_c1 (sin (+ (* twopi ph_c) (* op_b1 gain_b))))
(def out1 op_c1)

; Algo 2: (A + B) -> C
(def op_a2 (sin (+ (* twopi ph_a) (* fb_val feedback))))
(def op_b2 (sin (* twopi ph_b)))
(def op_c2 (sin (+ (* twopi ph_c) (* op_a2 gain_a) (* op_b2 gain_b))))
(def out2 op_c2)

; Algo 3: Additive A + B + C (Rich pad style)
(def op_a3 (sin (+ (* twopi ph_a) (* fb_val feedback))))
(def op_b3 (sin (* twopi ph_b)))
(def op_c3 (sin (* twopi ph_c)))
(def out3 (+ (* op_c3 0.6) (* op_a3 gain_a 0.3) (* op_b3 gain_b 0.3)))

; Algo 4: Complex Feedback A -> B -> C with B self-fb
(def op_a4 (sin (* twopi ph_a)))
(def op_b4 (sin (+ (* twopi ph_b) (* op_a4 gain_a) (* fb_val feedback))))
(def op_c4 (sin (+ (* twopi ph_c) (* op_b4 gain_b))))
(def out4 op_c4)

(def sig (selector algo out1 out2 out3 out4 out1 out1 out1 out1))
(write-history fb_hist sig)

; --- Processing ---
(def driven (tanh (* sig (+ 1 drive))))
(def filtered (svf driven cutoff resonance @type lp))

(out (* filtered m_env 0.3) 1 @name audio)
