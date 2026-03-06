; ADSR test synth
; Uses the injected compile-time adsr macro.

(def gate (in 1 @name gate))
(def raw_pitch (in 2 @name pitch))
(def velocity (in 3 @name velocity))

(defmacro onepole (sig a) 
  (make-history h)
  (write-history h (mix sig (read-history h) a)))

(param attack_ms  @default 30  @min 1 @max 5000)
(param decay_ms   @default 120 @min 1 @max 5000)
(param sustain @default 0.7   @min 0     @max 1)
(param release_ms @default 180 @min 1 @max 5000)

(def level (adsr attack_ms decay_ms sustain release_ms))

(def detune (* 2 (cos (* twopi (phasor 3)))))

(def fmenv (scale (adsr 1 30 0.1 300) 0 1 0.2 1))

(def pitch (+ detune raw_pitch))

(def op2  (cos (* twopi (phasor (* 1 pitch)))))
(def op1  (cos (+ (* fmenv op2 2) (* twopi (phasor (/ pitch 4))))))
(out (* op1 level (onepole velocity 0.99)) 1 @name audio)
