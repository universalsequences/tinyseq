
# Notes

## Custom Sound
- dgen server that lets you write "lisp" expressions to create synths real quick
- compiles the synth and adds it to the graph (cached compilation)
- create a library of synths

- In order to do this need a super specific API for "synths": ("gate", "pitch freq") inputs
- (defun-synth (pitch env) list expressions to create synth)
- Server is a swift wrapper around the dgen library that 1. parses the lisp 

## Jaki?

## Cirklon
- current track is shown with cirklon view instead
- sliders for each parameter 
- each step should have several parameters


[tr 1] [kick df] [pattern 1]
[trn] | dur | vel | spd | aux_a | aux_b
_
_     _     _
_  _  _  _  _
_  _  _  _  _
o  o  o  o  o 
 
transpose is -32 to 32 
when selecting a step typing a number and hitting enter should select it
should show curren steps parameter value

should have easy way to go between tracks 
only show one track at a time with some info of the other tracks

# effects
- comb filter
- phaser
- chorus
- filter

- all p-lockable 
- tracks each have their own fx chain

# Remember
- the goal of this should be to quickly do sampling ---
- 

# Modes 
I want to expand the UI to have multiple regions that are all shown vertically one after each other but "focused" -- controlled by tabs (which hould no longer control param selected)
## cirklon (placed at top where it currently is)
## track/instrument parameters: 
Show as several parameters vertically (up/down selects parameter -- once selected can do the "type value and enter" for boolean enter just toggles)
gate: true/false (when false it plays until it completes) attack/release
swing: 50-75% (default 50) 
## track effects

This is where things get interesting. I want all effect parameters to be p-lockable.
There should be a standard set of effects on each track, with each effect laid out horizontally (arrow left/right changes effect selected) and enclosed in a box with effect name at top and again parameters for the effect vertically laid out with arrow up/down controlling which effect parameter we're in and typing to control the value or enter for toggling booleans

If steps are selected (via the gang system) any parameter edits are p-locked to the step, i.e. they switch smoothly at the step and goes back to the track-wide fx parameter setting when it hits a step that has no p-lock for that parameter. Whenever we toggle a step off it should clear the p-locks. P-locked steps should have some sort of visualization in the step-sequencer showing that it has a p-lock

Parameter layout:
[param name] [value]
[slider visualizing the value] (if its numerical)

### multi-mode filter
  - enabled: true/false
  - mode: dropdown with "lowpass" (default), "hipass", "bandpass"
  - cut: 50-10000
  - res: 0.5-16.0

### stereo delay: 
 - wet (wet is always first)
 - synced: true/false (either syncs to bpm or is raw ms)
 - delay time: based on synced it either shows a synced time like 1/4 (enter should bring up a dropdown that you can arrow up or down and hit enter to lock in the exact setting: 1/32, 1/16, 1/16t, 1/8, 1/8t, 1/8., 1/4, 1/4t, 1/4., 1/2, 1
 - feedback: 0-1
 - dampening: 0-1
 - stereo-width: 1-2 (default 1) multiplies delaytime when computing right-ears delay time


