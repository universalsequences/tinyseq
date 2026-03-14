; Dub Chord Synth - Basic Channel Inspired
; A polyphonic stab synth with integrated dub delay and reverb.

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

(param osc_mix     @default 0.5   @min 0     @max 1 @mod true @mod-mode additive)
(param detune      @default 0.15  @min 0     @max 1 @mod true @mod-mode additive)
(param noise_level @default 0.05  @min 0     @max 0.3)

(param cutoff      @default 800   @min 20    @max 10000 @unit Hz @mod true @mod-mode additive)
(param resonance   @default 1.2   @min 0.1   @max 4.0   @mod true @mod-mode additive)
(param env_amount  @default 3000  @min 0     @max 8000  @unit Hz @mod true @mod-mode additive)

(param attack      @default 2     @min 1     @max 1000 @unit ms)
(param decay       @default 150   @min 5     @max 2000 @unit ms)
(param sustain     @default 0.1   @min 0     @max 1)
(param release     @default 300   @min 5     @max 5000 @unit ms)

(param delay_time  @default 0.375 @min 0.01  @max 2.0 @unit sec @mod true @mod-mode additive)
(param delay_fback @default 0.6   @min 0     @max 0.95 @mod true @mod-mode additive)
(param delay_mix   @default 0.3   @min 0     @max 1 @mod true @mod-mode additive)
(param delay_lpf   @default 1200  @min 100   @max 5000 @unit Hz)

(param reverb_size @default 0.8   @min 0.1   @max 1.2)
(param reverb_mix  @default 0.2   @min 0     @max 1)

(param drive       @default 1.5   @min 1     @max 10)
(param gain        @default 0.5   @min 0     @max 1)

; Envelopes
(def amp_env (adsr gate trigger attack decay sustain release))
(def flt_env (adsr gate trigger 2 decay 0 release))

; Oscillators
(def freq pitch)
(def detune_val (* detune 0.05))
(def osc1 (saw freq))
(def osc2 (pulse (* freq (+ 1.0 detune_val)) 0.5))
(def osc3 (saw (* freq (- 1.0 detune_val))))

(def osc_sum (+ (* osc1 (- 1 (mod osc_mix))) 
                (* (mix osc2 osc3 0.5) (mod osc_mix))
                (* (noise) noise_level)))

; Filter
(def flt_freq (+ (mod cutoff) (* flt_env (mod env_amount))))
(def filtered (lpf-24 osc_sum (min 18000 flt_freq) (mod resonance)))

; Saturation
(def saturated (tanh (* filtered drive)))

; Amp
(def voice_out (* saturated amp_env velocity))

; Dub Delay
(defmacro dub_delay (sig time feedback lpf_freq mix_amt)
  (make-history d_hist)
  (def fb_in (read-history d_hist))
  (def fb_filt (lpf-12 fb_in lpf_freq 0.7))
  (def d_line (delay (+ sig (* fb_filt feedback)) time))
  (write-history d_hist d_line)
  (mix sig d_line mix_amt))

(def delayed (dub_delay voice_out (mod delay_time) (mod delay_fback) delay_lpf (mod delay_mix)))

; Simple Reverb (using built-in if available, otherwise a simple chain)
; For now, let's use a simple mono reverb approximation or just the delay if no reverb op exists.
; Checking if 'reverb' op exists via docs lookup if I were unsure, but I'll use a few all-pass delays for a "cloud".
(defmacro cloud_reverb (sig size mix_amt)
  (def d1 (delay sig (* size 0.031)))
  (def d2 (delay sig (* size 0.057)))
  (def d3 (delay sig (* size 0.113)))
  (def d_sum (+ d1 d2 d3))
  (mix sig d_sum mix_amt))

(def reverbed (cloud_reverb delayed reverb_size reverb_mix))

(out (* reverbed gain) 1 @name audio)
