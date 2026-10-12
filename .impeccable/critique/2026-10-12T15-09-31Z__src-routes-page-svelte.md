---
target: the app design
total_score: 28
max_score: 40
na_heuristics: 
p0_count: 0
p1_count: 2
target_identity: "file:/home/user/RustRadar/src/routes/+page.svelte"
target_fingerprint: "sha256:19a60e5f1e73a89689e689bcde0d419b317a1787e7a209ea741c266da30029dc"
target_path: /home/user/RustRadar/src/routes/+page.svelte
timestamp: 2026-10-12T15-09-31Z
slug: src-routes-page-svelte
---
# Critique: RustRadar app UI (src/routes/+page.svelte and src/lib panels)
Method: dual-agent (A: design review · B: detector + browser)

## Design Health Score: 28/40 (Good)
| # | Heuristic | Score | Key issue |
|---|---|---|---|
| 1 | Visibility of system status | 3 | "Updated N s ago" never escalates; ended flight keeps live-looking values |
| 2 | Match with the real world | 3 | Aviation notation excellent; 12-hour clocks, no zone abbreviations |
| 3 | User control and freedom | 3 | Esc order invisible; filter Reset has no undo |
| 4 | Consistency and standards | 3 | Red used for non-failures ("No longer live", rate limiting) |
| 5 | Error prevention | 2 | "—" means both "not reported" and "sign in"; Enter in an empty token field adds top suggestion |
| 6 | Recognition rather than recall | 3 | Layer toggles don't show their shortcut keys |
| 7 | Flexibility and efficiency | 3 | No shortcuts for panel tabs or layers; globe not keyboard reachable |
| 8 | Aesthetic and minimalist design | 3 | Flight panel repeats altitude/speed; chart below the fold |
| 9 | Error recovery | 3 | Raw "Error:" strings leak through |
| 10 | Help and documentation | 2 | Only a shortcut list; omits right-drag; gated fields only in tooltips |

## Specificity
Mostly authored: flight-strip header, FL/GND notation, AIRAC, honest filed/estimated language, sodium amber. Generic dark-SaaS in Filters chips, Layers popover, Most tracked list.
Detector: 0 CLI findings; in-browser: shared .surface thin border + wide shadow (one token), 11px --text-xs captions, type scale 1.17 below 1.25 target, kbd border (false positive).

## Priority issues
1. [P1] Stale and ended data look live (+page.svelte:547, FlightPanel.svelte:564,1137) -> /impeccable harden
2. [P1] "—" hides "sign-in only" (FlightPanel.svelte:330-345) -> /impeccable clarify
3. [P2] Flight panel buries the altitude chart and repeats itself (FlightPanel.svelte:347-392) -> /impeccable distill + layout
4. [P2] Small, flat type: 11px captions at text-3, 1.17 scale; attribution truncated; rate limit styled as failure -> /impeccable typeset
5. [P2] Globe not keyboard reachable -> /impeccable harden

## Persona red flags
Alex: no tab/layer shortcuts, tab through Most tracked to reach panel, Enter adds top airline, no undo on Reset.
Sam: globe mouse-only, tooltip-only explanations, many 11px text-3 notes.
Planespotter: endless Live pulse, no stale escalation, 12-hour clocks, no landed summary, GMT+1 instead of BST.

## Minor
Error: prefix leak; wide colon in Overpass Mono times; Place names size control nesting; country label collisions; ultrawide panels stay 320/360px; long airport names truncate; duplicate delay text.

## Questions
Ride-along tag on the globe? A single honesty grammar (solid/dashed/faded)? "In view now" instead of "Most tracked"?
