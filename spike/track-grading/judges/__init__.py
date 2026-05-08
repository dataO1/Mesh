"""Pluggable K-way ranking judge backends for round-7.6 LLM tournaments.

Each judge exposes the same interface so the tournament runner can swap
backends without touching call-site logic. Backends differ in:
  - Underlying model (Qwen3-Omni-30B-AWQ vs Music Flamingo 7B bf16)
  - Wire protocol (vLLM OpenAI-compatible HTTP)
  - License (Qwen research / MF NVIDIA OneWay Noncommercial Academic)
  - Throughput (Qwen3 ~3 calls/s, MF ~1.2 calls/s on RTX 5090 Mobile)
  - Music-task accuracy (MF beats Qwen3 by ~22 pp on MuChoMusic)

The base interface in `base.py` is intentionally narrow: build a chat
payload from K audios + a polar prompt, POST it, parse the ranking. All
parallelism (worker pool, BALD scheduler, atomic JSON cache) lives in the
tournament runner — judges are stateless.
"""
from .base import (
    Judge,
    RankingResult,
    ScoreResult,
    ParseError,
    InferenceError,
    K,
    LETTERS,
)
from .qwen3_omni import Qwen3OmniJudge
from .music_flamingo import MusicFlamingoJudge

__all__ = [
    "Judge", "RankingResult", "ScoreResult", "ParseError", "InferenceError",
    "K", "LETTERS",
    "Qwen3OmniJudge", "MusicFlamingoJudge",
]
