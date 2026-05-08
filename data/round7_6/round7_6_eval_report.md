# Round-7.6 V18 evaluation report

Held-out test set size: **1553 tracks** (out of 15314 aligned).

## Headline metrics (vs consensus on held-out test)

| Model | PA | Spearman | R² | Notes |
|---|---:|---:|---:|---|
| **V18 student (deployed)** | **0.8116** | +0.8194 | +0.6758 | linear probe over MuQ-MuLan |
| V18 teacher | 0.9311 | — | — | privileged: audio + caption + tags |
| V15 (deployed) | 0.7021 | — | — | round-6 reference |
| V17b polar blend | 0.7302 | — | — | round-7.5 reference |

Teacher → student distillation gap: **+0.1195 pp** (spec G6 target ≤ 5 pp).

## Per-cluster diagnostic (caption-emb K-means)

Audible-genre breakdown via K-means on the 768d bge-base caption embeddings. Replaces per-genre breakdown — uses what the model actually heard, not the unreliable everynoise tags (per G7).

Sorted by **mean predicted intensity** (low → high):

| k | n_test | mean_int | PA(student) | top-3 nearest captions |
|---|---:|---:|---:|---|
| 2 | 61 | +0.151 | 0.666 | This track is an intimate Acoustic Folk‑Singer‑Songwriter piece that leans toward a melancholic, reflective ballad style; This track is an Acoustic Folk piece rooted in the Singer‑Songwriter tradition, blending gentle fingerpicked acoustic  |
| 13 | 34 | +0.211 | 0.617 | This track is a Neo‑Soul piece that blends classic soul warmth with contemporary jazz‑inflected R&B, creating a smooth, intimate stylistic hybrid; This track is a Neo‑Soul piece that blends contemporary R&B smoothness with jazz‑inflected ha |
| 9 | 96 | +0.267 | 0.601 | This track is an Ambient Pop/Dream Pop piece that blends ethereal synth textures with a gently melancholic pop sensibility; This track is a dreamy, melancholic Synth‑Pop piece that leans heavily into Dream‑Pop aesthetics, blending lush elec |
| 10 | 69 | +0.273 | 0.653 | This track is a dark ambient / drone composition, blending deep, evolving synth textures with a minimalist, atmospheric sound‑design aesthetic; This track is a Dark Ambient / Cinematic Ambient piece that blends brooding, atmospheric sound‑d |
| 11 | 11 | +0.279 | 0.436 | This track is an uplifting Roots Reggae piece that blends classic Jamaican skank rhythms with a warm, organic production aesthetic; This track is a classic Roots Reggae piece rooted in the traditional Jamaican sound while incorporating a po |
| 1 | 83 | +0.363 | 0.751 | This track is an energetic Latin Pop / Reggaeton piece that blends the bright, melodic sensibilities of contemporary Latin pop with the driving dembow groove of modern reggaeton; This track is an energetic Reggaeton‑Latin Pop hybrid that bl |
| 3 | 57 | +0.378 | 0.685 | This track is a high‑energy Dancehall piece that leans toward a modern, club‑oriented sub‑genre, blending classic Jamaican rhythmic sensibility with polished electronic production; This track is a high‑energy Dancehall piece that leans towa |
| 12 | 92 | +0.407 | 0.649 | This track is a high‑energy Progressive House piece that leans heavily into Trance‑style melodic sensibilities, creating a polished blend of driving club grooves and soaring, euphoric synth lines; This track is a high‑energy Progressive Hou |
| 19 | 57 | +0.432 | 0.706 | This track is an energetic Indie Rock/Alternative Rock piece that blends driving, guitar‑forward aggression with a polished, modern production aesthetic; This track is an energetic Indie Rock/Alternative Rock piece that blends bright, major |
| 4 | 82 | +0.435 | 0.617 | This track is a high‑energy Tech House piece that leans toward a minimal, hypnotic sub‑genre, blending driving four‑on‑the‑floor club rhythms with a slightly distorted synth bass for a gritty edge; This track is an energetic Tech House/Mini |
| 15 | 55 | +0.445 | 0.608 | This track is a gritty East Coast Hip‑Hop piece rooted in classic Boom‑Bap, blending raw lo‑fi sampling with a street‑wise lyrical swagger; This track is a classic East Coast Hip‑Hop piece rooted in the Boom Bap tradition, blending gritty s |
| 17 | 107 | +0.447 | 0.707 | This track is a high‑energy Electro House piece that blends classic four‑on‑the‑floor club rhythm with bright, melodic synth work, creating a polished, dance‑floor‑ready aesthetic; This track is a high‑energy Electro House piece that leans  |
| 14 | 42 | +0.466 | 0.633 | This track is a high‑energy Latin Trap piece that blends contemporary trap production with a distinctly urban Latin flair; This track is a high‑energy Latin Trap piece that blends contemporary trap production with unmistakable Latin‑urban f |
| 5 | 40 | +0.472 | 0.638 | This track is a French Trap/Drill piece that blends the hard‑hitting, syncopated drum patterns of drill with the melodic, bass‑driven aesthetic of contemporary trap; This track is an aggressive French Hip‑Hop piece that fuses contemporary T |
| 7 | 166 | +0.497 | 0.712 | This track is a modern Trap‑Hip‑Hop piece that blends hard‑hitting trap percussion with melodic, auto‑tuned rap delivery; This track is an aggressive, confidence‑dripping Trap‑Hip‑Hop piece that leans heavily into modern, high‑fidelity trap |
| 18 | 57 | +0.640 | 0.688 | This track is an aggressive Industrial Techno piece that fuses the relentless drive of hard‑edged techno with the harsh, mechanical textures of industrial music; This track is an aggressive Industrial Techno piece that leans heavily into a  |
| 8 | 49 | +0.670 | 0.611 | This track is a high‑energy Hardstyle piece, leaning toward the raw‑hardstyle sub‑genre with a hard‑hitting, euphoric blend of pounding kicks and aggressive synth work; This track is a high‑energy Hardstyle piece, rooted in the raw hardstyl |
| 6 | 109 | +0.673 | 0.732 | This track is a high‑energy Punk Rock / Pop‑Punk anthem that blends the raw aggression of classic punk with the melodic hooks of modern pop‑punk; This track is an aggressive, high‑energy Punk Rock / Hardcore Punk piece that blends raw, lo‑f |
| 16 | 109 | +0.789 | 0.727 | This track is an aggressive blend of Metalcore and Deathcore, marrying the crushing, down‑tuned riffage of modern deathcore with the tight, breakdown‑focused songwriting of contemporary metalcore; This track is an aggressive Metalcore piece |
| 0 | 177 | +0.864 | 0.577 | This track is an aggressive Thrash Metal piece, rooted firmly in the classic thrash tradition while pushing toward a raw, high‑octane extreme metal aesthetic; This track is an aggressive Thrash Metal piece, rooted firmly in the classic thra |

## Spec compliance (Appendix D rubric)

- **G3** test PA ≥ 0.75: PASS (0.8116)
- **G6** distill gap ≤ 5 pp: FAIL (+0.1195)
- **G9** CPU latency (1000-track dot): PASS (0.00 ms total, 0.00 µs/track)
