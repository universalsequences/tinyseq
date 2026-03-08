; Prophet-5 inspired polysynth
; Inputs: gate (ch 1), pitch_hz (ch 2), velocity (ch 3)
; Helper input injected at compile time: trigger (ch 4)
; Helper macro injected at compile time: (adsr attack_ms decay_ms sustain release_ms)

(def gate (in 1 @name gate))
(def pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))

(defmacro pulse_from_phase (phase width)
  (scale (lt phase width) 0 1 -1 1))

(defmacro smooth (sig amt)
  (make-history h)
  (def y (mix sig (read-history h) amt))
  (write-history h y))

(defmacro weighted_mix3 (a wa b wb c wc)
  (def total (+ wa wb wc))
  (/ (+ (* a wa) (* b wb) (* c wc)) (max 1 total)))

(param amp_attack_ms    @default 6    @min 1    @max 5000 @unit ms)
(param amp_decay_ms     @default 140  @min 1    @max 5000 @unit ms)
(param amp_sustain      @default 0.78 @min 0    @max 1)
(param amp_release_ms   @default 260  @min 1    @max 5000 @unit ms)

(param filt_attack_ms   @default 5    @min 1    @max 5000 @unit ms)
(param filt_decay_ms    @default 220  @min 1    @max 5000 @unit ms)
(param filt_sustain     @default 0.0  @min 0    @max 1)
(param filt_release_ms  @default 320  @min 1    @max 5000 @unit ms)

(param cutoff           @default 920  @min 30   @max 12000 @unit Hz)
(param resonance        @default 1.0  @min 0.3  @max 4.5)
(param filter_env_amt   @default 1800 @min -5000 @max 5000 @unit Hz)
(param keytrack         @default 0.4  @min 0    @max 2)
(param filter_vel_amt   @default 0.35 @min 0    @max 1)
(param filter_drive     @default 1.25 @min 0.5  @max 4)

(param osc_a_level      @default 0.8  @min 0    @max 1)
(param osc_b_level      @default 0.75 @min 0    @max 1)
(param noise_level      @default 0.02 @min 0    @max 0.4)

(param osc_a_semi       @default 0    @min -24  @max 24 @unit st)
(param osc_b_semi       @default 0    @min -36  @max 36 @unit st)
(param osc_a_fine_cents @default 0    @min -50  @max 50 @unit cents)
(param osc_b_fine_cents @default 2    @min -50  @max 50 @unit cents)

(param osc_a_saw        @default 1    @min 0    @max 1)
(param osc_a_pulse      @default 0    @min 0    @max 1)
(param osc_b_saw        @default 1    @min 0    @max 1)
(param osc_b_tri        @default 0    @min 0    @max 1)
(param osc_b_pulse      @default 0    @min 0    @max 1)

(param osc_a_pw         @default 0.5  @min 0.05 @max 0.95)
(param osc_b_pw         @default 0.5  @min 0.05 @max 0.95)
(param sync             @default 0    @min 0    @max 1)

(param osc_b_keytrack   @default 1    @min 0    @max 1)
(param osc_b_lfo_mode   @default 0    @min 0    @max 1)
(param osc_b_lfo_rate   @default 7    @min 0.05 @max 40 @unit Hz)

(param lfo_rate         @default 4.8  @min 0.05 @max 20 @unit Hz)
(param lfo_tri          @default 1    @min 0    @max 1)
(param lfo_saw          @default 0    @min 0    @max 1)
(param lfo_square       @default 0    @min 0    @max 1)
(param lfo_to_a_st      @default 0    @min -2   @max 2 @unit st)
(param lfo_to_b_st      @default 0    @min -2   @max 2 @unit st)
(param lfo_to_a_pw      @default 0    @min -0.45 @max 0.45)
(param lfo_to_b_pw      @default 0    @min -0.45 @max 0.45)
(param lfo_to_filter    @default 0    @min -2500 @max 2500 @unit Hz)

(param poly_env_to_a_st  @default 0    @min -24  @max 24 @unit st)
(param poly_oscb_to_a_st @default 0    @min -24  @max 24 @unit st)
(param poly_env_to_a_pw  @default 0    @min -0.45 @max 0.45)
(param poly_oscb_to_a_pw @default 0    @min -0.45 @max 0.45)
(param poly_env_to_filter @default 0   @min -4000 @max 4000 @unit Hz)
(param poly_oscb_to_filter @default 0  @min -4000 @max 4000 @unit Hz)

(param amp_vel_amt      @default 0.35 @min 0    @max 1)
(param vintage          @default 0.12 @min 0    @max 1)
(param gain             @default 0.16 @min 0    @max 1)

(def amp_env (adsr amp_attack_ms amp_decay_ms amp_sustain amp_release_ms))
(def filt_env (adsr filt_attack_ms filt_decay_ms filt_sustain filt_release_ms))

(def ln2 (log 2))

(def lfo_phase (phasor lfo_rate))
(def lfo_tri_sig (triangle lfo_phase))
(def lfo_saw_sig (scale lfo_phase 0 1 -1 1))
(def lfo_square_sig (pulse_from_phase lfo_phase 0.5))
(def lfo_mod (weighted_mix3 lfo_tri_sig lfo_tri lfo_saw_sig lfo_saw lfo_square_sig lfo_square))

(def note_rand_a (latch (noise) trigger))
(def note_rand_b (latch (noise) trigger))
(def note_rand_pw (latch (noise) trigger))
(def note_rand_cutoff (latch (noise) trigger))

(def drift_a_cents (* note_rand_a vintage 5))
(def drift_b_cents (* note_rand_b vintage 5))
(def drift_pw (* note_rand_pw vintage 0.04))
(def drift_cutoff (* note_rand_cutoff vintage 180))

(def osc_b_pitch_semi (+ osc_b_semi (/ (+ osc_b_fine_cents drift_b_cents) 100)))
(def osc_b_ratio (exp (* ln2 (/ osc_b_pitch_semi 12))))
(def osc_b_keyboard_freq (* (mix 440.0 pitch osc_b_keytrack) osc_b_ratio))
(def osc_b_freq (mix osc_b_keyboard_freq osc_b_lfo_rate osc_b_lfo_mode))

(def osc_b_phase (phasor osc_b_freq))
(make-history osc_b_phase_hist)
(def osc_b_phase_prev (read-history osc_b_phase_hist))
(def osc_b_wrap (* (gt sync 0.5) (lt osc_b_phase osc_b_phase_prev)))
(write-history osc_b_phase_hist osc_b_phase)

(def osc_b_pw_mod (clip (+ osc_b_pw (* lfo_mod lfo_to_b_pw) drift_pw) 0.04 0.96))
(def osc_b_saw_sig (scale osc_b_phase 0 1 -1 1))
(def osc_b_tri_sig (triangle osc_b_phase))
(def osc_b_pulse_sig (pulse_from_phase osc_b_phase osc_b_pw_mod))
(def osc_b_mix (weighted_mix3 osc_b_saw_sig osc_b_saw osc_b_tri_sig osc_b_tri osc_b_pulse_sig osc_b_pulse))
(def osc_b_filter_mod (smooth osc_b_mix 0.995))

(def osc_a_pitch_mod (+ (* lfo_mod lfo_to_a_st) (* filt_env poly_env_to_a_st) (* osc_b_mix poly_oscb_to_a_st)))
(def osc_a_pitch_semi (+ osc_a_semi (/ (+ osc_a_fine_cents drift_a_cents) 100) osc_a_pitch_mod))
(def osc_a_ratio (exp (* ln2 (/ osc_a_pitch_semi 12))))
(def osc_a_freq (* pitch osc_a_ratio))
(def osc_a_phase (phasor osc_a_freq osc_b_wrap))
(def osc_a_pw_mod
  (clip (+ osc_a_pw (* lfo_mod lfo_to_a_pw) (* filt_env poly_env_to_a_pw) (* osc_b_mix poly_oscb_to_a_pw) drift_pw) 0.04 0.96))
(def osc_a_saw_sig (scale osc_a_phase 0 1 -1 1))
(def osc_a_pulse_sig (pulse_from_phase osc_a_phase osc_a_pw_mod))
(def osc_a_mix (weighted_mix3 osc_a_saw_sig osc_a_saw osc_a_pulse_sig osc_a_pulse 0 0))

(def osc_b_pitch_lfo_ratio (exp (* ln2 (/ (* lfo_mod lfo_to_b_st) 12))))
(def osc_b_audio_phase (phasor (* osc_b_freq osc_b_pitch_lfo_ratio)))
(def osc_b_audio_mix
  (weighted_mix3
    (scale osc_b_audio_phase 0 1 -1 1) osc_b_saw
    (triangle osc_b_audio_phase) osc_b_tri
    (pulse_from_phase osc_b_audio_phase osc_b_pw_mod) osc_b_pulse))

(def mixer (+ (* osc_a_mix osc_a_level) (* osc_b_audio_mix osc_b_level) (* (noise) noise_level)))
(def driven (tanh (* mixer filter_drive)))

(def filter_env_scaled (* filt_env (* filter_env_amt (+ (- 1 filter_vel_amt) (* filter_vel_amt velocity)))))
(def filter_mod (+ cutoff (* pitch keytrack) filter_env_scaled (* filt_env poly_env_to_filter) (* osc_b_filter_mod poly_oscb_to_filter) (* lfo_mod lfo_to_filter) drift_cutoff))
(def safe_filter_cutoff (min 10000 (max 30 filter_mod)))
(def safe_resonance (min 3.2 (max 0.35 resonance)))

(def filter_stage1 (biquad driven safe_filter_cutoff safe_resonance 1 0))
(def filter_stage2 (biquad (tanh (* filter_stage1 (+ 0.6 (* 0.4 filter_drive)))) safe_filter_cutoff safe_resonance 1 0))

(def amp_velocity (+ (- 1 amp_vel_amt) (* amp_vel_amt velocity)))
(out (* (tanh (* filter_stage2 1.15)) amp_env amp_velocity gain) 1 @name audio)
