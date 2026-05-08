"""Qwen3-Omni-30B-A3B-Instruct-AWQ-4bit judge — vLLM OpenAI-compat HTTP.

Refactor of the inline judge call from `run_nway_tournaments_r7_5.py` so
the same model-call code is reachable through the abstract Judge
interface. Wire format and prompt structure are unchanged from round 7.5.

vLLM serve command (separate process; same as round 7.5):
    bash spike/track-grading/serve_qwen3_omni.sh
"""
from __future__ import annotations

import base64
import io
import os
import time
from typing import Sequence

import numpy as np
import requests
import soundfile as sf

from .base import Judge, RankingResult, ParseError, InferenceError, K, LETTERS

DEFAULT_VLLM_URL = os.environ.get(
    "QWEN3_VLLM_URL",
    "http://localhost:8000/v1/chat/completions",
)
DEFAULT_MODEL = "cpatonn/Qwen3-Omni-30B-A3B-Instruct-AWQ-4bit"


class Qwen3OmniJudge(Judge):
    """30B audio-LM via vLLM. Established baseline from round 7.5."""

    judge_id = "qwen3_omni"

    def __init__(
        self,
        *,
        url: str = DEFAULT_VLLM_URL,
        model_name: str = DEFAULT_MODEL,
        request_timeout_s: float = 240.0,
    ):
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
        # Map letter -> track_id following the requested presentation order:
        # the i-th letter in `order` gets the i-th track in `track_tuple`.
        letter_to_track = {order[i]: int(track_tuple[i]) for i in range(K)}

        b64_clips = [_audio_to_b64_wav(a, self.sample_rate) for a in audio_arrays]

        # Construct content: K input_audio blocks (in order[]) + final text prompt
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
                {"role": "system", "content": "You are an expert music analyst."},
                {"role": "user", "content": content},
            ],
            "max_tokens": max_tokens,
            "temperature": temperature,
        }
        t0 = time.time()
        try:
            r = requests.post(self.url, json=payload, timeout=self.request_timeout_s)
            r.raise_for_status()
            text = r.json()["choices"][0]["message"]["content"]
        except requests.exceptions.RequestException as e:
            raise InferenceError(f"qwen3_omni POST failed: {e}") from e
        wall = time.time() - t0

        # parse_ranking raises ParseError on malformed output
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


def _audio_to_b64_wav(audio: np.ndarray, sample_rate: int) -> str:
    """Encode mono float32 audio as base64 WAV (PCM_16)."""
    buf = io.BytesIO()
    sf.write(buf, audio.astype(np.float32), sample_rate, format="WAV", subtype="PCM_16")
    return base64.b64encode(buf.getvalue()).decode("ascii")
