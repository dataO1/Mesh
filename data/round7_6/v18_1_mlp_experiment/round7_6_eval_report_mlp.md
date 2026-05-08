# Round-7.6 V18 evaluation report

Held-out test set size: **3985 tracks** (out of 39913 aligned).

## Headline metrics (vs consensus on held-out test)

| Model | PA | Spearman | R² | Notes |
|---|---:|---:|---:|---|
| **V18 student (deployed)** | **0.8174** | +0.8264 | +0.6889 | linear probe over MuQ-MuLan |
| V18 teacher | 0.9400 | — | — | privileged: audio + caption + tags |
| V15 (deployed) | 0.7013 | — | — | round-6 reference |
| V17b polar blend | 0.7270 | — | — | round-7.5 reference |

Teacher → student distillation gap: **+0.1226 pp** (spec G6 target ≤ 5 pp).

## Per-cluster diagnostic (caption-emb K-means)

Audible-genre breakdown via K-means on the 768d bge-base caption embeddings. Replaces per-genre breakdown — uses what the model actually heard, not the unreliable everynoise tags (per G7).

Sorted by **mean predicted intensity** (low → high):

| k | n_test | mean_int | PA(student) | top-3 nearest captions |
|---|---:|---:|---:|---|
| 18 | 123 | +0.222 | 0.665 | This track is a dark ambient / drone composition, blending deep, evolving synth textures with a minimalist, atmospheric sound‑design aesthetic; This track is a Dark Ambient / Drone piece that blends deep, evolving synth textures with a mini |
| 0 | 129 | +0.248 | 0.737 | This track is a Neo‑Soul piece that blends classic soul warmth with contemporary jazz‑inflected R&B, creating a smooth, intimate stylistic hybrid; This track is a Contemporary R&B / Neo‑Soul piece that blends smooth, jazz‑inflected chord vo |
| 12 | 268 | +0.251 | 0.775 | This track is a melancholic Indie Folk / Acoustic Rock piece that blends intimate singer‑songwriter storytelling with a clean, natural acoustic aesthetic; This track is an Indie Rock composition that leans toward a melancholic‑introspective |
| 1 | 238 | +0.281 | 0.681 | This track is a melancholic Dream Pop/Indie Pop piece that blends airy synth‑based soundscapes with a gentle electronic beat, creating an ethereal, introspective atmosphere; This track is a dreamy, melancholic Synth‑Pop piece that leans hea |
| 13 | 177 | +0.332 | 0.782 | This track is a polished Brazilian Pop‑R&B song that blends contemporary urban pop sensibilities with smooth R&B groove and subtle Latin‑flavored harmonic color; This track is an energetic Latin‑Pop song that blends contemporary dance‑floor |
| 5 | 73 | +0.345 | 0.683 | This track is an energetic Afro‑Pop/Afrobeats piece that blends contemporary electronic dance production with the rhythmic vitality of West‑African pop; This track is a vibrant Afrobeat‑Afropop hybrid that fuses West African rhythmic sensib |
| 8 | 108 | +0.377 | 0.752 | This track is an energetic Reggae‑Dancehall piece that blends classic one‑drop reggae rhythms with the punchier, vocal‑centric swagger of modern dancehall; This track is an energetic Dancehall‑Reggae piece that blends classic one‑drop regga |
| 16 | 179 | +0.405 | 0.647 | This track is a high‑energy Dance‑Pop/Eurodance anthem that blends bright, club‑ready synth work with a polished, radio‑friendly pop sensibility; This track is an energetic Eurodance‑style Dance‑Pop song that blends bright, club‑ready synth |
| 10 | 88 | +0.421 | 0.658 | This track is a high‑energy Reggaeton piece that leans into contemporary urban Latin pop, blending the classic dembow rhythm with modern synth‑driven production; This track is a contemporary Reggaeton piece that leans toward a polished, clu |
| 4 | 245 | +0.434 | 0.630 | This track is a high‑energy Progressive House piece that leans heavily into Trance‑style melodic sensibilities, creating a polished blend of driving club grooves and soaring, euphoric synth lines; This track is a high‑energy Progressive Hou |
| 17 | 142 | +0.453 | 0.611 | This track is a gritty East Coast Hip‑Hop piece rooted in classic Boom‑Bap, blending raw lo‑fi sampling with a street‑wise lyrical swagger; This track is a raw, gritty East Coast Hip‑Hop piece rooted in classic Boom‑Bap aesthetics, blending |
| 11 | 96 | +0.472 | 0.653 | This track is a confident, assertive Latin Trap piece that blends the hard‑hitting rhythmic drive of contemporary trap with the melodic sensibilities of urban Latin music; This track is a high‑energy Latin Trap piece that blends contemporar |
| 3 | 211 | +0.480 | 0.661 | This track is an energetic French Hip‑Hop/Trap piece that blends gritty street‑rap delivery with modern trap production aesthetics; This track is a French Hip‑Hop piece rooted in contemporary Trap, blending the gritty rhythmic drive of stre |
| 15 | 310 | +0.486 | 0.672 | This track is a high‑energy Tech House piece that leans into a minimal‑tech sub‑genre, blending a driving four‑on‑the‑floor groove with subtle, evolving synth textures; This track is an energetic Tech House piece that leans toward a minimal |
| 19 | 337 | +0.524 | 0.732 | This track is a high‑energy Trap‑Hip‑Hop piece that blends modern trap production with a gritty hip‑hop attitude; This track is a hard‑hitting Trap‑Hip‑Hop piece that leans into a dark, street‑level aesthetic, blending the booming low‑end o |
| 14 | 314 | +0.627 | 0.723 | This track is a high‑energy Pop‑Punk song that blends classic punk‑rock aggression with melodic pop sensibilities; This track is a high‑energy Punk Rock piece that leans heavily into Pop‑Punk sensibilities, blending raw, aggressive guitar‑d |
| 7 | 140 | +0.651 | 0.677 | This track is an aggressive Industrial Techno piece that fuses the relentless drive of hard‑edged techno with the harsh, mechanical textures of industrial music; This track is an aggressive, high‑intensity Industrial Techno piece that fuses |
| 2 | 141 | +0.736 | 0.607 | This track is a high‑energy Hardstyle piece that leans toward the raw‑hardstyle sub‑genre, blending the genre’s signature pounding kick with aggressive, screeching lead synths and a distorted, punchy ; This track is a high‑energy Hardstyle  |
| 6 | 280 | +0.753 | 0.739 | This track is a high‑energy, aggressive instrumental piece that sits squarely in the Metalcore/Deathcore arena, blending the crushing riff‑driven intensity of modern metalcore with the brutal, low‑end; This track is an aggressive blend of M |
| 9 | 386 | +0.835 | 0.548 | This track is an aggressive Thrash Metal piece, rooted firmly in the classic thrash tradition while pushing toward a raw, high‑octane extreme metal aesthetic; This track is an aggressive Thrash Metal piece, rooted firmly in the classic thra |

## Spec compliance (Appendix D rubric)

- **G3** test PA ≥ 0.75: PASS (0.8174)
- **G6** distill gap ≤ 5 pp: FAIL (+0.1226)
- **G9** CPU latency (1000-track dot): PASS (1.40 ms total, 1.40 µs/track)
