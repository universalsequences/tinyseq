(define-instrument fm-epiano-pm
  (
   ;; --- Global & Volume ---
   (gain "Gain" 0.5 0 1 1)
   (velocity-sens "Vel Sens" 0.6 0 1 1)
   
   ;; --- Pair 1: Body (Fundamental/Mellow) ---
   (body-ratio "Body Mod Ratio" 1.0 0.5 8.0 1.0)
   (body-idx "Body Mod Idx" 1.5 0 10 1.0 @mod-mode:add)
   (body-decay "Body Decay" 1.2 0.1 5.0 1.0)
   
   ;; --- Pair 2: Tine (High Chime) ---
   (tine-ratio "Tine Mod Ratio" 14.0 1.0 20.0 1.0)
   (tine-idx "Tine Mod Idx" 0.5 0 5 1.0 @mod-mode:add) ; Reduced default for "taming"
   (tine-decay "Tine Decay" 0.4 0.05 2.0 1.0)
   (tine-mix "Tine Mix" 0.3 0 1 1 @mod-mode:add)
   
   ;; --- Taming & Tone ---
   (tone-lpf "Tone LPF" 4000 500 15000 1.0 @mod-mode:add)
   (release "Release" 0.3 0.01 2.0 1.0)
   )
  
  (let* ((vel-scale (+ (- 1.0 velocity-sens) (* velocity-sens velocity)))
         
         ;; Envelopes
         (amp-env (adsr 0.005 1.0 0.0 release gate))
         (body-env (exp-decay body-decay gate))
         (tine-env (exp-decay tine-decay gate))
         
         ;; Modulators (using phase modulation style)
         ;; Note: In PM, we feed the output of the modulator into the :phase of the carrier
         (mod1 (* (sin-osc (* freq body-ratio)) body-env body-idx vel-scale))
         (mod2 (* (sin-osc (* freq tine-ratio)) tine-env tine-idx vel-scale))
         
         ;; Carriers
         (sig-body (sin-osc freq :phase mod1))
         (sig-tine (sin-osc (* freq 1.001) :phase mod2)) ; slight detune for tine
         
         ;; Mix and Filter
         (mix (+ (* sig-body (- 1.0 tine-mix)) (* sig-tine tine-mix)))
         (final-sig (lowpass mix tone-lpf 0.7)))
    
    (* final-sig amp-env gain velocity)))