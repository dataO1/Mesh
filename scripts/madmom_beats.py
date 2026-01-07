#!/usr/bin/env python3
"""
Madmom DBN beat detection for mesh-cue.

This script uses madmom's RNNBeatProcessor + DBNBeatTrackingProcessor pipeline
to detect beat positions and calculate BPM from audio files.

Usage:
    python madmom_beats.py <wav_path> [--min-bpm MIN] [--max-bpm MAX]

Output:
    JSON to stdout with keys: beats, bpm, confidence
"""

import sys
import json
import argparse
import numpy as np

from madmom.features.beats import RNNBeatProcessor, DBNBeatTrackingProcessor


def calculate_bpm_from_beats(beats: np.ndarray) -> float:
    """Calculate BPM from beat positions using median inter-beat interval.

    Uses median instead of mean for robustness against outliers
    (missed beats or extra detected beats).
    """
    if len(beats) < 2:
        return 0.0

    # Calculate inter-beat intervals
    intervals = np.diff(beats)

    # Filter out unreasonable intervals (< 0.2s = 300 BPM, > 2s = 30 BPM)
    valid_intervals = intervals[(intervals > 0.2) & (intervals < 2.0)]

    if len(valid_intervals) == 0:
        return 0.0

    # Use median interval (robust to outliers)
    median_interval = np.median(valid_intervals)

    return 60.0 / median_interval


def main():
    parser = argparse.ArgumentParser(
        description='Madmom DBN beat detection for mesh-cue'
    )
    parser.add_argument('wav_path', help='Path to WAV audio file')
    parser.add_argument(
        '--min-bpm', type=float, default=55.0,
        help='Minimum expected BPM (default: 55)'
    )
    parser.add_argument(
        '--max-bpm', type=float, default=215.0,
        help='Maximum expected BPM (default: 215)'
    )
    args = parser.parse_args()

    try:
        # RNNBeatProcessor: Neural network beat activation function
        # Returns activation values at 100 fps (frames per second)
        processor = RNNBeatProcessor()
        activations = processor(args.wav_path)

        # DBNBeatTrackingProcessor: Dynamic Bayesian Network inference
        # Converts activations to discrete beat positions
        tracker = DBNBeatTrackingProcessor(
            fps=100,
            min_bpm=args.min_bpm,
            max_bpm=args.max_bpm
        )
        beats = tracker(activations)

        # Calculate BPM from median inter-beat interval
        bpm = calculate_bpm_from_beats(beats)

        # Output JSON result
        result = {
            "beats": beats.tolist(),
            "bpm": float(bpm),
            # DBN provides implicit confidence through transition model
            # Use 0.85 as reasonable default for DBN tracker
            "confidence": 0.85
        }

        print(json.dumps(result))
        sys.exit(0)

    except Exception as e:
        # Output error as JSON for Rust to parse
        error_result = {
            "error": str(e),
            "beats": [],
            "bpm": 0.0,
            "confidence": 0.0
        }
        print(json.dumps(error_result), file=sys.stderr)
        sys.exit(1)


if __name__ == '__main__':
    main()
