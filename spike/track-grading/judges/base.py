"""Abstract K-way ranking judge interface.

A judge takes K audio clips + a polar-prompt axis description and returns
an ordering low-pole → high-pole. The runner doesn't care which model does
the work — only that the contract holds.

Why an abstract base instead of duck-typing: round-7.6 will run two judges
(Qwen3-Omni archive + Music Flamingo) on overlapping pair sets, and we need
a uniform error taxonomy so the tournament runner can tell "model is down"
from "this particular response was malformed" from "the audio file went
missing." Bare exceptions get caught and logged but don't kill the worker.
"""
from __future__ import annotations

import abc
from dataclasses import dataclass, field
from typing import Sequence


# K is fixed at 4 across the round-7.6 codebase. Bumping requires
# regenerating prompt templates and parser regexes. If you ever want K=3
# or K=5, change LETTERS and parse_choice; nothing else.
K = 4
LETTERS = ("A", "B", "C", "D")


class ParseError(ValueError):
    """LLM response could not be mapped to a strict 4-letter ranking.

    Examples: response was empty, used letters outside A-D, contained
    repeats, or had something other than exactly 4 chars in the answer
    line. The runner retries once on ParseError, then drops the call.
    """


class InferenceError(RuntimeError):
    """Underlying serving layer failed (HTTP timeout, 5xx, GPU OOM, etc.).

    The runner treats this as transient: backs off briefly and retries.
    Repeated InferenceError on the same call drops the call to disk
    with a fail marker but doesn't kill the worker pool.
    """


@dataclass
class ScoreResult:
    """One pointwise (K=1) score produced by a single-audio judge.

    Music Flamingo is single-audio per prompt, so K=4 N-way ranking
    is architecturally infeasible. Instead each (track, axis) cell is
    scored 0-100 directly. Stack all cells into an (N, A) matrix; the
    linear probe trains against that matrix in place of BT priors.
    """
    track_id: int
    axis_id: str
    score: float                              # 0-100 inclusive
    raw_response: str                         # full LLM output
    wall_time_s: float
    judge_id: str
    extras: dict = field(default_factory=dict)


@dataclass
class RankingResult:
    """One K-way ranking produced by a judge.

    `letter_to_track` maps presentation letter (A/B/C/D) to the actual
    track id. `ranking_low_to_high` is the ordered tuple of letters from
    LOW pole (least HIGH-pole-ish) to HIGH pole. Combine these two to
    derive pairwise observations downstream:
        for i, j in itertools.combinations(range(K), 2):
            track_low  = letter_to_track[ranking_low_to_high[i]]
            track_high = letter_to_track[ranking_low_to_high[j]]
            wins[(track_high, track_low)] += 1.0
    """
    track_tuple: tuple[int, ...]                # 4 track IDs
    letter_to_track: dict[str, int]             # presentation order map
    ranking_low_to_high: tuple[str, ...]        # 4 letters
    raw_response: str                           # full LLM output incl reasoning
    wall_time_s: float
    judge_id: str                               # "qwen3_omni" / "music_flamingo"
    extras: dict = field(default_factory=dict)  # per-judge debug


class Judge(abc.ABC):
    """K-way ranking judge.

    Subclasses implement `_call_llm()` (pure I/O — send K WAV bytes and
    a prompt to the model, return raw response text). The base class
    handles audio decode, presentation-order shuffle, and response
    parsing identically across judges so behavior is comparable.
    """

    judge_id: str = "abstract"

    def __init__(self, *, sample_rate: int, model_name: str):
        self.sample_rate = sample_rate     # judge-required input SR (e.g. 16000)
        self.model_name = model_name        # for record-keeping

    # ── public API ─────────────────────────────────────────────────

    @abc.abstractmethod
    def is_alive(self) -> bool:
        """Quick health check before the runner schedules work."""

    @abc.abstractmethod
    def rank(
        self,
        track_tuple: Sequence[int],
        audio_arrays: Sequence,             # K float32 mono numpy arrays
        prompt_text: str,
        *,
        presentation_order: Sequence[str] | None = None,  # default = LETTERS
        max_tokens: int = 80,
        temperature: float = 0.0,
    ) -> RankingResult:
        """Send K clips + prompt, parse ranking. Raises Parse/Inference on fail."""

    # ── shared helpers (concrete) ──────────────────────────────────

    @staticmethod
    def parse_ranking(text: str) -> tuple[str, ...]:
        """Extract a strict 4-letter ranking from LLM output.

        Looks at the first non-empty line; collects A/B/C/D characters in
        order; rejects if length ≠ 4 or any duplicate. The first line
        constraint stops us from scraping reasoning text below.
        """
        if not text:
            raise ParseError("empty response")
        first_line = ""
        for line in text.strip().splitlines():
            s = line.strip()
            if s:
                first_line = s
                break
        if not first_line:
            raise ParseError("no non-empty line in response")
        # Allow some surrounding punctuation/whitespace; pull only A-D
        letters = [c for c in first_line.upper() if c in "ABCD"]
        # Some models echo "Answer: BACD" or "ranking: B A C D" — we just
        # take the first 4 distinct ABCD chars in order.
        seen: list[str] = []
        for c in letters:
            if c not in seen:
                seen.append(c)
            if len(seen) == 4:
                break
        if len(seen) != 4:
            raise ParseError(
                f"could not extract 4 distinct A-D letters from: {first_line!r}"
            )
        return tuple(seen)

    @staticmethod
    def shuffle_letters(rng) -> tuple[str, ...]:
        """Return a randomised presentation order for the K letters.

        Used to wash positional bias: per-call presentation differs even
        when the same 4 tracks come up again under BALD. The runner
        records the shuffle in the cache JSON so audits can reconstruct
        what the judge actually saw.
        """
        order = list(LETTERS)
        rng.shuffle(order)
        return tuple(order)
