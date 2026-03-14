; Karplus-Strong physical modeling: plucked string synthesis
; Feedback delay line (length = sr/pitch) with two-point average LP in loop
; Assumes sample rate = 44100 Hz

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

; ── String parameters ──
(param brightness      @default 0.55  @min 0    @max 1    @mod true @mod-mode additive)
  ; 0=darkest (pure two-point average), 1=brightest (no filtering)
(param decay_time_ms   @default 2000  @min 50   @max 12000 @unit ms @mod true @mod-mode additive)
  ; Perceptual sustain time — frequency-independent
(param body_tone       @default 0.0   @min 0    @max 1)
  ; comb-filters the noise exciter for tonal color (0=raw noise, 1=narrowband)
(param exciter_cycles  @default 2.0   @min 0.5  @max 16)
  ; Exciter length in string periods — scales automatically with pitch
(param pluck_pos       @default 0.0   @min 0    @max 0.45)
  ; Comb filter on exciter: 0=bridge (bright), 0.45=neck (warm)
(param detune_cents    @default 0     @min 0    @max 30   @unit cents @mod true @mod-mode additive)
  ; Second string for 12-string / unison effect

(param vel_to_brightness @default 0.35 @min 0   @max 1)
(param vel_to_level      @default 0.6  @min 0   @max 1)
(param gain            @default 0.18  @min 0    @max 1)

; ── Exciter: burst length scaled to pitch so it works across the whole range ──
(def exciter_decay_ms  (max 0.5 (* exciter_cycles 1000.0 (/ 1.0 (max 20.0 pitch)))))
(def exciter_env       (adsr gate trigger 1 exciter_decay_ms 0.0 1))
; Noise-only exciter — sine tone exciter causes random DC step at trigger phase,
; which enters the loop and takes a very long time to decay at high stretch values
(def exciter_raw       (noise))
; Pluck position: comb filter (skip when pluck_pos=0 to avoid 1-sample artefact)
(def pluck_dly         (max 2.0 (* pluck_pos 80.0)))
(def exciter_combed    (+ exciter_raw (* (delay exciter_raw pluck_dly) pluck_pos 0.6)))
(def vel_level         (+ (- 1 vel_to_level) (* vel_to_level velocity)))
; HP the exciter to strip DC before it enters the loop
(def exciter_hp        (biquad exciter_combed 40 0.7 1 1))
(def exciter_sig       (* exciter_hp exciter_env vel_level))

; ── Karplus-Strong feedback loop ──
; Two-point average LP has exactly 0.5 samples group delay → subtract 1.5 total
; (1.0 for make-history cell + 0.5 for filter)
(def ks_delay  (max 1.0 (- (/ 44100.0 (max 20.0 pitch)) 1.5)))

; Frequency-independent decay: same decay_time_ms at any pitch
; stretch = exp(-1 / (pitch * decay_time_s)), guaranteed < 1 so no blowup
(def decay_time_s (max 0.05 (/ (mod decay_time_ms) 1000.0)))
(def stretch      (min 0.9999 (exp (/ -1.0 (* (max 20.0 pitch) decay_time_s)))))

; Brightness: blend two-point average (dark) with passthrough (bright)
(def vel_bright   (+ (- 1 vel_to_brightness) (* vel_to_brightness velocity)))
(def bright_coef  (clip (* vel_bright (mod brightness)) 0 1))

(make-history ks_buf)
(make-history ks_lp_prev)
(def ks_delayed  (delay (read-history ks_buf) ks_delay))
(def ks_avg      (+ (* 0.5 ks_delayed) (* 0.5 (read-history ks_lp_prev))))
(write-history ks_lp_prev ks_delayed)
(def ks_lp       (mix ks_avg ks_delayed bright_coef))
(def ks_next     (+ exciter_sig (* ks_lp stretch)))
(write-history ks_buf ks_next)

; ── Second detuned string (12-string / chorus effect) ──
(def ln2          (log 2))
(def detune_ratio (exp (* ln2 (/ (mod detune_cents) 1200))))
(def ks2_delay    (max 1.0 (- (/ 44100.0 (max 20.0 (* pitch detune_ratio))) 1.5)))
(def stretch2     (min 0.9999 (exp (/ -1.0 (* (max 20.0 (* pitch detune_ratio)) decay_time_s)))))

(make-history ks_buf2)
(make-history ks_lp_prev2)
(def ks2_delayed  (delay (read-history ks_buf2) ks2_delay))
(def ks2_avg      (+ (* 0.5 ks2_delayed) (* 0.5 (read-history ks_lp_prev2))))
(write-history ks_lp_prev2 ks2_delayed)
(def ks2_lp       (mix ks2_avg ks2_delayed bright_coef))
(def ks2_next     (+ exciter_sig (* ks2_lp stretch2)))
(write-history ks_buf2 ks2_next)

(def detune_blend (clip (/ (mod detune_cents) 30.0) 0 1))
; Normalize the two-string sum so blending doesn't increase level
(def string_out   (mix ks_next (* 0.5 (+ ks_next ks2_next)) detune_blend))
; HP the output to strip any residual DC that accumulated in the loop
(def out_hp       (biquad string_out 30 0.7 1 1))

(out (* out_hp gain) 1 @name audio)
