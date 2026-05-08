"""Music Flamingo (NVIDIA, Nov 2025) judge — vLLM OpenAI-compat HTTP.

Backbone: Audio Flamingo 3 = Qwen2.5-7B + AF-Whisper encoder, fine-tuned
on MF-Skills (5.2M music-aware caption examples). On MuChoMusic, MF
scores 74.6% vs Qwen3-Omni-30B's 52.1% — +22pp on music understanding,
which is why we're swapping the judge in round 7.6.

Configuration:
  - bf16 weights (no quantization) per user preference: quality > speed.
    Trade-off: ~1.0-1.4 K=4 calls/sec sustained on RTX 5090 Mobile vs
    Qwen3-Omni's 3.0-3.3 calls/sec, but model is 7B vs 30B and its
    output supposedly aligns better with human music perception.
  - Multi-audio per turn supported (limit_mm_per_prompt={"audio":4}).
  - Encoder cache via vLLM's content-based mm_hash: each unique audio
    file is encoded once across all calls touching it. We pass stable
    `multi_modal_uuids` per request to make cache keys deterministic.

License: NVIDIA OneWay Noncommercial Academic — research use only.
Mesh's commercial deployment cannot use MF-derived signals; this is for
internal research/labeling only. See research note for full license
analysis.

vLLM serve command:
    bash spike/track-grading/serve_music_flamingo.sh
"""
from __future__ import annotations

import base64
import io
import math
import os
import time
from typing import Sequence

import numpy as np
import requests
import soundfile as sf

from .base import (
    Judge,
    RankingResult,
    ScoreResult,
    ParseError,
    InferenceError,
    K,
    LETTERS,
)

# Likert bucket tokens. Single-digit ASCII guarantees one-token encoding in
# Qwen2.5's tokenizer (and most BPE tokenizers), so we can read first-token
# logprobs cleanly.
LIKERT_BUCKETS = ("1", "2", "3", "4", "5")

DEFAULT_VLLM_URL = os.environ.get(
    "MF_VLLM_URL",
    "http://localhost:8001/v1/chat/completions",  # different port from Qwen3
)
DEFAULT_MODEL = "nvidia/music-flamingo-2601-hf"


class MusicFlamingoJudge(Judge):
    """7B music-specialized audio-LM via vLLM."""

    judge_id = "music_flamingo"

    def __init__(
        self,
        *,
        url: str = DEFAULT_VLLM_URL,
        model_name: str = DEFAULT_MODEL,
        request_timeout_s: float = 300.0,
    ):
        # Music Flamingo expects 16 kHz mono audio (Whisper standard).
        super().__init__(sample_rate=16_000, model_name=model_name)
        self.url = url
        self.health_url = url.rsplit("/v1/", 1)[0] + "/health"
        self.request_timeout_s = request_timeout_s

    def is_alive(self) -> bool:
        try:
            r = requests.get(self.health_url, timeout=5)
            return r.status_code == 200
        except Exception:
            return False

    def rank(
        self,
        track_tuple: Sequence[int],
        audio_arrays: Sequence[np.ndarray],
        prompt_text: str,
        *,
        presentation_order: Sequence[str] | None = None,
        max_tokens: int = 80,
        temperature: float = 0.0,
    ) -> RankingResult:
        if len(track_tuple) != K or len(audio_arrays) != K:
            raise ValueError(f"need exactly {K} tracks/arrays")
        order = tuple(presentation_order) if presentation_order else LETTERS
        if sorted(order) != sorted(LETTERS):
            raise ValueError(f"presentation_order must be a permutation of {LETTERS}")
        letter_to_track = {order[i]: int(track_tuple[i]) for i in range(K)}

        # Music Flamingo uses input_audio blocks like other vLLM-served
        # multimodal models. We pass stable per-track UUIDs so vLLM's
        # mm_hash cache returns the encoded audio without re-running the
        # AF-Whisper forward pass on subsequent tuples that include the
        # same track.
        b64_clips = [_audio_to_b64_wav(a, self.sample_rate) for a in audio_arrays]
        content: list[dict] = []
        for i, letter in enumerate(order):
            content.append({
                "type": "input_audio",
                "input_audio": {"data": b64_clips[i], "format": "wav"},
            })
        content.append({"type": "text", "text": prompt_text})

        payload = {
            "model": self.model_name,
            "messages": [
                {"role": "system",
                 "content": "You are a careful music analyst with strong "
                            "perception of timbre, mood, rhythm, and "
                            "production style."},
                {"role": "user", "content": content},
            ],
            "max_tokens": max_tokens,
            "temperature": temperature,
            # Stable cache UUIDs — vLLM uses these for the content-based
            # encoder cache. Same track id across calls → encoder hit.
            "extra_body": {
                "multi_modal_uuids": {
                    "audio": [f"track_{int(track_tuple[i])}" for i in range(K)]
                }
            },
        }
        t0 = time.time()
        try:
            r = requests.post(self.url, json=payload, timeout=self.request_timeout_s)
            r.raise_for_status()
            text = r.json()["choices"][0]["message"]["content"]
        except requests.exceptions.RequestException as e:
            raise InferenceError(f"music_flamingo POST failed: {e}") from e
        wall = time.time() - t0

        ranking = self.parse_ranking(text)

        return RankingResult(
            track_tuple=tuple(int(t) for t in track_tuple),
            letter_to_track=letter_to_track,
            ranking_low_to_high=ranking,
            raw_response=text,
            wall_time_s=round(wall, 3),
            judge_id=self.judge_id,
            extras={"presentation_order": list(order)},
        )


    def score(
        self,
        track_id: int,
        audio_array: np.ndarray,
        prompt_text: str,
        axis_id: str,
        *,
        max_tokens: int = 16,
        temperature: float = 0.0,
    ) -> ScoreResult:
        """Single-audio pointwise rating in [0, 100].

        Music Flamingo's audio-token limit is 1 per prompt, so K=4 N-way
        ranking is impossible. We rate each (track, axis) cell directly.
        """
        b64 = _audio_to_b64_wav(audio_array, self.sample_rate)
        payload = {
            "model": self.model_name,
            "messages": [
                {"role": "system",
                 "content": "You are a careful music analyst with strong "
                            "perception of timbre, mood, rhythm, and "
                            "production style."},
                {"role": "user", "content": [
                    {"type": "input_audio",
                     "input_audio": {"data": b64, "format": "wav"}},
                    {"type": "text", "text": prompt_text},
                ]},
            ],
            "max_tokens": max_tokens,
            "temperature": temperature,
            "extra_body": {
                "multi_modal_uuids": {"audio": [f"track_{int(track_id)}"]}
            },
        }
        t0 = time.time()
        try:
            r = requests.post(self.url, json=payload, timeout=self.request_timeout_s)
            r.raise_for_status()
            text = r.json()["choices"][0]["message"]["content"]
        except requests.exceptions.RequestException as e:
            raise InferenceError(f"music_flamingo POST failed: {e}") from e
        wall = time.time() - t0

        score = _parse_score(text)
        return ScoreResult(
            track_id=int(track_id),
            axis_id=axis_id,
            score=float(score),
            raw_response=text,
            wall_time_s=round(wall, 3),
            judge_id=self.judge_id,
        )


    def score_likert(
        self,
        track_id: int,
        audio_array: np.ndarray,
        prompt_text: str,
        axis_id: str,
        *,
        top_logprobs: int = 10,
        max_tokens: int = 4,
        temperature: float = 0.0,
    ) -> ScoreResult:
        """5-bucket Likert pointwise score with log-prob recovery.

        Music Flamingo was trained on captions + MCQ + classification, NOT
        scalar rating, so a "0-100 integer" prompt at T=0 collapses to
        the modal "50" token on subjective axes. Replacing the ask with
        "pick one of {1,2,3,4,5}" puts the model in its strong
        categorical mode, and reading first-token logprobs over those
        5 single-digit tokens recovers a continuous 0-100 score:

            score = sum_{i=1..5} p_i * (i - 1) / 4 * 100

        where p_i is the softmax over only the 5 digit tokens.
        Diagnostic bucket_probs are kept in extras.
        """
        b64 = _audio_to_b64_wav(audio_array, self.sample_rate)
        payload = {
            "model": self.model_name,
            "messages": [
                {"role": "system",
                 "content": "You are a careful music analyst with strong "
                            "perception of timbre, mood, rhythm, and "
                            "production style."},
                {"role": "user", "content": [
                    {"type": "input_audio",
                     "input_audio": {"data": b64, "format": "wav"}},
                    {"type": "text", "text": prompt_text},
                ]},
            ],
            "max_tokens": max_tokens,
            "temperature": temperature,
            "logprobs": True,
            "top_logprobs": top_logprobs,
            "extra_body": {
                "multi_modal_uuids": {"audio": [f"track_{int(track_id)}"]}
            },
        }
        t0 = time.time()
        try:
            r = requests.post(self.url, json=payload, timeout=self.request_timeout_s)
            r.raise_for_status()
            data = r.json()["choices"][0]
            text = data["message"]["content"]
            logprobs = data.get("logprobs") or {}
        except requests.exceptions.RequestException as e:
            raise InferenceError(f"music_flamingo POST failed: {e}") from e
        wall = time.time() - t0

        bucket_probs = _likert_probs_from_logprobs(logprobs)
        # Score: weighted average of bucket index (0-indexed) over [0, 100].
        score = sum(p * i / (len(LIKERT_BUCKETS) - 1) * 100
                    for i, p in enumerate(bucket_probs))

        return ScoreResult(
            track_id=int(track_id),
            axis_id=axis_id,
            score=float(score),
            raw_response=text,
            wall_time_s=round(wall, 3),
            judge_id=self.judge_id,
            extras={
                "bucket_probs": bucket_probs,
                "method": "likert5_logprobs",
            },
        )


    def caption(
        self,
        track_id: int,
        audio_array: np.ndarray,
        prompt_text: str | None = None,
        *,
        max_tokens: int = 256,
        temperature: float = 0.7,
        top_p: float = 0.9,
    ) -> dict:
        """Free-form rich caption for a clip — MF's #1 trained task.

        MF's training corpus contains 3.4M long captions (avg 452 words);
        on MusicCaps it scores 8.8/10 GPT5. This is the regime where the
        model is strongest. We use NVIDIA's recommended decoding
        (T=0.7, top_p=0.9, do_sample=True). max_tokens=256 yields ~190-
        word captions, enough to encode timbre + mood + instrumentation
        + structure without paying for full 600-word generation.

        Returns a plain dict (not ScoreResult) since captions don't have
        a numeric score:
            {track_id, caption, raw_response, wall_time_s, judge_id,
             prompt_tokens?, completion_tokens?}
        """
        if prompt_text is None:
            prompt_text = (
                "Describe this music clip in rich detail. Cover the "
                "instrumentation, production style, mood, rhythm and "
                "groove, harmony, vocal qualities (if any), structural "
                "events (buildup, drop, breakdown), and the overall "
                "energy. Use specific musical vocabulary."
            )
        b64 = _audio_to_b64_wav(audio_array, self.sample_rate)
        payload = {
            "model": self.model_name,
            "messages": [
                {"role": "system",
                 "content": "You are a careful music analyst with strong "
                            "perception of timbre, mood, rhythm, and "
                            "production style."},
                {"role": "user", "content": [
                    {"type": "input_audio",
                     "input_audio": {"data": b64, "format": "wav"}},
                    {"type": "text", "text": prompt_text},
                ]},
            ],
            "max_tokens": max_tokens,
            "temperature": temperature,
            "top_p": top_p,
            "extra_body": {
                "multi_modal_uuids": {"audio": [f"track_{int(track_id)}"]}
            },
        }
        t0 = time.time()
        try:
            r = requests.post(self.url, json=payload, timeout=self.request_timeout_s)
            r.raise_for_status()
            data = r.json()
            text = data["choices"][0]["message"]["content"]
        except requests.exceptions.RequestException as e:
            raise InferenceError(f"music_flamingo caption POST failed: {e}") from e
        wall = time.time() - t0
        return {
            "track_id": int(track_id),
            "caption": text,
            "raw_response": text,
            "wall_time_s": round(wall, 3),
            "judge_id": self.judge_id,
            "model": self.model_name,
            "prompt_tokens": data.get("usage", {}).get("prompt_tokens"),
            "completion_tokens": data.get("usage", {}).get("completion_tokens"),
        }


def _likert_probs_from_logprobs(logprobs: dict) -> list[float]:
    """Extract renormalized softmax over the 5 Likert bucket tokens.

    vLLM's chat completion logprobs schema (OpenAI-compatible):
        logprobs.content: list[{token, logprob, top_logprobs: [{token, logprob}]}]

    Reads the FIRST generated token's top_logprobs, finds entries whose
    token text is one of {"1","2","3","4","5"} (or starts with a single
    digit + maybe whitespace), exponentiates and renormalizes. Mass on
    other tokens is silently dropped — if the model put 90% on '50' or
    'I', that becomes 0 effective probability and the renorm distributes
    over whatever digit tokens DID appear in top_logprobs.

    If no digit tokens appear at all, returns uniform [0.2]*5 with a
    warning shape preserved (caller can detect via low entropy on the
    raw_response).
    """
    if not logprobs:
        return [0.2] * 5
    content = logprobs.get("content") or []
    if not content:
        return [0.2] * 5
    first = content[0]
    top = first.get("top_logprobs") or []
    bucket_logits = {b: -math.inf for b in LIKERT_BUCKETS}
    for entry in top:
        tok = (entry.get("token") or "").strip()
        # vLLM may emit token bytes like ' 1' or '1' depending on tokenizer
        if tok in bucket_logits:
            bucket_logits[tok] = max(bucket_logits[tok], entry["logprob"])
            continue
        # Fallback: token is e.g. "1." or " 1" — strip non-digits, keep first
        ds = "".join(c for c in tok if c.isdigit())
        if ds and ds[0] in bucket_logits:
            cand = ds[0]
            bucket_logits[cand] = max(bucket_logits[cand], entry["logprob"])
    # If none matched, fall back to uniform — caller flags via raw response.
    raw_logits = [bucket_logits[b] for b in LIKERT_BUCKETS]
    if all(math.isinf(x) for x in raw_logits):
        return [0.2] * 5
    # Stable softmax over 5 buckets.
    m = max(x for x in raw_logits if not math.isinf(x))
    exps = [math.exp(x - m) if not math.isinf(x) else 0.0 for x in raw_logits]
    z = sum(exps)
    return [e / z for e in exps] if z > 0 else [0.2] * 5


def _parse_score(text: str) -> int:
    """Extract a 0-100 integer from the model's response.

    Handles:  '42'  '42/100'  'Score: 42'  '42.'  '  42  '
    Strict on out-of-range and non-numeric. Used by the pointwise
    runner — caller catches ParseError and counts/persists the failure.
    """
    if not text:
        raise ParseError("empty response")
    digits = ""
    for ch in text.strip():
        if ch.isdigit():
            digits += ch
            if len(digits) >= 3:  # 100 is the longest valid score
                break
        elif digits:
            break  # stop at first non-digit after digits started
    if not digits:
        raise ParseError(f"no digits in response: {text!r}")
    try:
        v = int(digits)
    except ValueError:
        raise ParseError(f"not an int: {digits!r}") from None
    if not (0 <= v <= 100):
        raise ParseError(f"out of range [0,100]: {v}")
    return v


def _audio_to_b64_wav(audio: np.ndarray, sample_rate: int) -> str:
    buf = io.BytesIO()
    sf.write(buf, audio.astype(np.float32), sample_rate, format="WAV", subtype="PCM_16")
    return base64.b64encode(buf.getvalue()).decode("ascii")
