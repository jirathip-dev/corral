# Final visual review — corrected single R2 direction

Mechanical validity, visual review, and Guy approval are separate. Guy approval is
still pending. Final images were inspected at native output sizes with a fresh
vision-capable GPT-5.6-Luna Codex read-only invocation (max effort); command raw exit 0.
No image changes occurred after this review. Superseded auxiliary descriptions that
misidentified facing direction were not used as the final gate.

## Reproduce the independent visual inspection

From this R2 directory (read-only; no artifact writes):

    codex exec -m gpt-5.6-luna -s read-only --skip-git-repo-check -c 'model_reasoning_effort="max"' -i v1-r2-idle-anatomy-study.png -i v1-r2-day-390x844.png -i v1-r2-night-390x844.png -o /tmp/corral-r2-visual-review.txt 'Inspect at native size. Judge equine joint/hoof/contact anatomy and premium illustrated volume, same-identity standing/shift/grazing/RM, shared Day/Night composition, gate containment, native labels, stars/Milky Way and moon rims. Separate mandatory failures from optional polish. No tools or edits.'

Shipping invocation used those exact model, sandbox, effort and image inputs with
a longer bounded correction checklist; its raw exit was 0. A future reviewer is
independent and may disagree. The exact shipping verdict and hashes follow.

## Exact reviewed image hashes

- `v1-r2-day-390x844.png`: `0e6aaba0084f1ad97b1f6f1271c0f57c3d24458996b1ccb201ffdba1cbcfc1b3`
- `v1-r2-idle-anatomy-study.png`: `1d436ec549b189a0180a59dc991970e2a82522c9bcc9689759cbfa8553038329`
- `v1-r2-night-390x844.png`: `8a7b1cfb85985746d50549810368ee53694e82f7e96e33a499cb35fddb381d5e`

## Independent reviewer output (verbatim)

Facing direction first: Image 1’s four poses face right. In Images 2–3, birch-clearing, oak-before-dark, and spruce-hollow face right; willow-bend’s grazing horse faces left.

- Image 1 — **PASS.** Native-size anatomy reads: knees, hocks, fetlocks, hooves, planted weight, lifted hind hoof, grazing load, and selected static stand are distinguishable. No blocking toy/paper-doll/trunk read.
- Image 2 — **PASS.** Layered ranch, physical gate containment, rail/post depth, hoof shadows, card gaps, unified HUD, exact fixtures, and fully visible unknown summary all hold.
- Image 3 — **PASS.** Shared composition and fixtures remain intact. Moon, granular Milky Way, dark-horse modeling, and moon-facing rim light read successfully.

Mandatory failures: **none**.

Optional polish only: slightly strengthen the darkest fetlock/hoof separations and make the night horse’s rim highlight a touch brighter on low-brightness displays.

## Designer findings and audit

Horse first-read is equine across standing, weight-shift and grazing. Head/poll bends
rather than forming a trunk; joint/hoof/contact corrections preserve original identity.
RM intentionally matches calm standing, not an accidental mid-animation freeze.
The two smaller upper sprites were enlarged to the same 148×112 art viewport as the
paddock/study. Native DOM probes independently confirm gate layering, label separation,
all five raw summaries, tap-zone size and horizontal paddock reachability.

Ten-tell self-audit: no decorative hero/marketing composition; no fake metrics or
filler; no generic three-card layout; no gratuitous gradient text; no oversized empty
headline; no arbitrary icon collage; no ornamental fantasy controls; no symmetric
card-grid replacement of V1; no motion spectacle; no repeated hero-plus-cards rhythm.
Composition remains Monitor: urgency at front rail, then repository traversal.
Shading/gradients serve material volume and atmospheric distance, not UI decoration.

Optional reviewer polish (not a mandatory failure): stronger darkest hoof/fetlock
separation and slightly brighter Night rims for low-brightness displays. No new style
or extra image was opened for optional polish. The orchestrator/Guy may re-inspect all
three exact images; this records a visual gate, not human acceptance.
