; Roland TB-303 inspired acid bass
; Single VCO (saw/square), diode ladder filter approximation, accent & slide

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

(defmacro pulse_from_phase (phase width)
  (scale (lt phase width) 0 1 -1 1))

; ── Amp (gate-following VCA, sustain=1 so note holds while gate is open) ──
(param amp_attack_ms   @default 2    @min 1    @max 300  @unit ms)
(param amp_decay_ms    @default 5000 @min 10   @max 5000 @unit ms)
(param amp_sustain     @default 1.0  @min 0    @max 1)
(param amp_release_ms  @default 15   @min 1    @max 500  @unit ms)

; ── Filter ──
(param cutoff          @default 600  @min 60   @max 7000  @unit Hz @mod true @mod-mode additive)
(param resonance       @default 3.2  @min 0.35 @max 4.5   @mod true @mod-mode additive)
(param env_amt         @default 3800 @min 0    @max 9000  @unit Hz @mod true @mod-mode additive)
(param env_decay_ms    @default 300  @min 10   @max 3000  @unit ms @mod true @mod-mode additive)
(param accent_amt      @default 0.65 @min 0    @max 1)   ; velocity → extra filter opening
(param keytrack        @default 0.4  @min 0    @max 2)

; ── Oscillator ──
(param wave_mix        @default 0.0  @min 0    @max 1     @mod true @mod-mode additive)  ; 0=saw, 1=square
(param pulse_width     @default 0.5  @min 0.05 @max 0.95  @mod true @mod-mode additive)
(param tune_semi       @default 0    @min -12  @max 12    @unit st)

; ── Character ──
(param drive           @default 1.6  @min 0.5  @max 5     @mod true @mod-mode additive)
(param slide_time      @default 0    @min 0    @max 500   @unit ms)
(param gain            @default 0.13 @min 0    @max 1)

; ── Envelopes ──
(def amp_env  (adsr gate trigger amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
; Filter env: one-shot (sustain=0), triggered per note, decays independently of gate
(def filt_env (adsr gate trigger 2 (mod env_decay_ms) 0.0 5))

; Accent: velocity scales how much the filter env opens
(def accent_vel (* (+ (- 1 accent_amt) (* accent_amt velocity))
                   (+ 0.6 (* 0.4 velocity))))

; ── Portamento (slide) ──
; alpha → 1 for small slide_time (fast), → 0 for large (slow)
(def slide_alpha (- 1.0 (exp (/ -3.0 (max 0.1 (* slide_time 44.1))))))
(make-history slide_hist)
(def raw_pitch (* pitch (exp (/ (* (log 2) tune_semi) 12))))
(def slid_pitch (+ (* slide_alpha raw_pitch)
                   (* (- 1.0 slide_alpha) (read-history slide_hist))))
(write-history slide_hist slid_pitch)

; ── Oscillator ──
(def ph      (phasor slid_pitch))
(def saw_sig (scale ph 0 1 -1 1))
(def sq_sig  (pulse_from_phase ph (clip (mod pulse_width) 0.05 0.95)))
(def osc     (mix saw_sig sq_sig (clip (mod wave_mix) 0 1)))

; ── Diode ladder filter (2x biquad LP + inter-stage saturation) ──
; The classic 303 "squelch" comes from resonance tracking the fast filter envelope
(def filter_cutoff
  (min 9200 (max 60
    (+ (mod cutoff)
       (* slid_pitch (mod keytrack))
       (* filt_env (mod env_amt) accent_vel)))))
(def safe_res (clip (mod resonance) 0.35 4.5))
(def driven   (tanh (* osc (mod drive))))
(def f1       (biquad driven filter_cutoff safe_res 1 0))
(def f2       (biquad (tanh (* f1 1.15)) (* filter_cutoff 0.97) (* safe_res 0.9) 1 0))
(def out_sig  (* (tanh (* f2 0.85)) amp_env accent_vel gain))

(out out_sig 1 @name audio)
