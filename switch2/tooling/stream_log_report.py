#!/usr/bin/env python3
"""Summarize Switch 2 Punktfunk streaming logs without treating idle gaps as freezes.

Host-independent, standard-library-only diagnostic. It never changes the
stream, bitrate, decoder, or renderer. Times are those recorded in UTC.
"""
from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from statistics import median
from typing import Any

STATS = re.compile(
    r"\b(?P<fps>\d+) fps · (?P<mbps>[\d.]+) Mb/s · target (?P<target>\d+) Mb/s"
)
WINDOW = re.compile(
    r"(?P<pictures>\d+) pictures, (?P<unseen>\d+) unseen "
    r"\((?P<replaced>\d+) replaced, (?P<full>\d+) queue full\)"
)
P95 = re.compile(r"decode to present p50 [\d.]+ p95 (?P<p95>[\d.]+)")
RENDER = re.compile(r"ticks=(?P<ticks>\d+) distinct_frames=(?P<distinct>\d+)")
PRESENT_CADENCE = re.compile(
    r"present_gap_samples=(?P<samples>\d+) "
    r"present_gap_over_1p5x=(?P<slow>\d+) "
    r"present_gap_over_2x=(?P<very_slow>\d+) "
    r"present_gap_max_us=(?P<max_us>\d+)"
)
POOL = re.compile(r"nv12_pool_reuses=(?P<reuses>\d+) nv12_pool_allocations=(?P<allocations>\d+)")
GAP = re.compile(r"silence_ms=(?P<silence>\d+) unpolled_ms=(?P<unpolled>\d+)")
INGRESS = re.compile(r"frames_dropped=(?P<dropped>\d+) rejected=(?P<rejected>\d+)")
RECOVERY = re.compile(r"recovery resuming after (?P<ms>\d+)ms")
AUDIO = re.compile(r"underruns (?P<silent>\d+) silent, (?P<audible>\d+) audible")
AHEAD = re.compile(r"audio_ahead_ms=(?P<low>\d+)-(?P<high>\d+)")
AV = re.compile(r"a/v (?P<offset>[+-]\d+) ms smoothed")
WELCOME = re.compile(r"connected: codec=(?P<codec>\d+).*wire_budget_kbps=(?P<budget>\d+)")


def _number_summary(values: list[float]) -> dict[str, float | None]:
    return {"median": round(median(values), 2) if values else None,
            "max": round(max(values), 2) if values else None}


def analyze(text: str) -> dict[str, Any]:
    """Extract explicit observations; absence of video packets != a visible freeze."""
    active: list[dict[str, float | int | str]] = []
    all_stats: list[dict[str, float | int | str]] = []
    frames: list[tuple[int, int]] = []
    present_cadence: list[tuple[int, int, int, int, int, bool]] = []
    gaps: list[tuple[int, int]] = []
    recoveries: list[int] = []
    ingress: list[tuple[int, int]] = []
    audio_audible = 0
    audio_windows = 0
    audio_ahead: list[int] = []
    av_offsets: list[int] = []
    pool: dict[str, int] | None = None
    codec: int | None = None
    budget: int | None = None
    errors = 0
    for line in text.splitlines():
        if ' ERROR ' in line or ' PANIC ' in line:
            errors += 1
        if 'session::connect connected:' in line:
            if m := WELCOME.search(line):
                codec, budget = int(m['codec']), int(m['budget'])
        if 'runtime::stream stats:' in line:
            m = STATS.search(line)
            if m:
                record: dict[str, float | int | str] = {
                    'utc': line[:19] if len(line) >= 19 else '?',
                    'fps': int(m['fps']), 'mbps': float(m['mbps']),
                    'target_mbps': int(m['target']),
                }
                if p := P95.search(line):
                    record['decode_to_present_p95_ms'] = float(p['p95'])
                if w := WINDOW.search(line):
                    record.update({k: int(w[k]) for k in ('pictures', 'unseen', 'replaced', 'full')})
                all_stats.append(record)
                if int(m['fps']) >= 115:
                    active.append(record)
        if 'switch2 video displayed:' in line:
            render = RENDER.search(line)
            if render:
                ticks, distinct = int(render['ticks']), int(render['distinct'])
                frames.append((ticks, distinct))
                # Use an explicitly active display window for a second view,
                # but do not label a slow interval as a proven visible stutter.
                if cadence := PRESENT_CADENCE.search(line):
                    present_cadence.append((
                        int(cadence['samples']), int(cadence['slow']),
                        int(cadence['very_slow']), int(cadence['max_us']),
                        ticks, ticks >= 115 and distinct >= 115,
                    ))
        if 'nv12_pool_reuses=' in line:
            if m := POOL.search(line):
                pool = {k: int(m[k]) for k in ('reuses', 'allocations')}
        if 'data receive gap' in line:
            if m := GAP.search(line):
                gaps.append((int(m['silence']), int(m['unpolled'])))
        if 'data wire ingress' in line:
            if m := INGRESS.search(line):
                ingress.append((int(m['dropped']), int(m['rejected'])))
        if 'recovery resuming after' in line:
            if m := RECOVERY.search(line):
                recoveries.append(int(m['ms']))
        if ' audio[10s]' in line:
            if m := AUDIO.search(line):
                audio_windows += 1
                audio_audible += int(m['audible'])
            if m := AHEAD.search(line):
                audio_ahead.append(int(m['high']))
            if m := AV.search(line):
                av_offsets.append(int(m['offset']))
    # 'rejected' is a reassembly/anti-replay counter. It must never be
    # interpreted as lost video frames without correlating FEC/drop counters.
    return {
        'codec': codec, 'welcome_budget_mbps': round(budget / 1000, 1) if budget else None,
        'stats_windows': len(all_stats), 'active_115fps_windows': len(active),
        'active_mbps': _number_summary([float(w['mbps']) for w in active]),
        'active_decode_p95_ms': _number_summary([
            float(w['decode_to_present_p95_ms']) for w in active
            if 'decode_to_present_p95_ms' in w]),
        'presenter_window_samples': len(frames),
        'presenter_distinct_ge_115': sum(t >= 115 and d >= 115 for t, d in frames),
        'present_cadence_windows': len(present_cadence),
        'present_gap_samples': sum(v[0] for v in present_cadence),
        'present_gap_over_1p5x': sum(v[1] for v in present_cadence),
        'present_gap_over_2x': sum(v[2] for v in present_cadence),
        'present_gap_max_ms': (round(max(v[3] for v in present_cadence) / 1000, 2)
                               if present_cadence else None),
        'present_active_windows': sum(v[5] for v in present_cadence),
        'present_active_gap_over_2x': sum(v[2] for v in present_cadence if v[5]),
        'max_gap_ms': max((s for s, _ in gaps), default=None),
        'reported_receive_gaps': len(gaps),
        'max_unpolled_ms': max((u for _, u in gaps), default=None),
        'frames_dropped_ingress': sum(d for d, _ in ingress),
        'packets_rejected_not_frame_losses': sum(r for _, r in ingress),
        'recovery_events': len(recoveries), 'max_recovery_ms': max(recoveries, default=None),
        'nv12_pool': pool,
        'audio_windows': audio_windows, 'audible_underruns': audio_audible,
        'audio_ahead_max_ms': max(audio_ahead, default=None),
        'audio_av_offset_median_ms': round(median(av_offsets), 1) if av_offsets else None,
        'error_lines': errors,
        'note': ('Receive gaps may be idle/static host periods. Replaced pictures are an '
                 'intentional low-latency policy; neither alone proves a visible freeze.'),
    }


def format_report(result: dict[str, Any], label: str) -> str:
    pool = result['nv12_pool'] or {}
    a = result['active_mbps']; p = result['active_decode_p95_ms']
    return '\n'.join([
        f'=== {label} ===',
        f"codec={result['codec']} budget={result['welcome_budget_mbps']} Mb/s",
        f"windows={result['stats_windows']} at>=115fps={result['active_115fps_windows']} "
        f"bitrate active median/max={a['median']}/{a['max']} Mb/s",
        f"decode->present active p95 median/max={p['median']}/{p['max']} ms",
        f"presenter windows={result['presenter_window_samples']} "
        f"with>=115 distinct frames={result['presenter_distinct_ge_115']}",
        f"NV12 pool reuses={pool.get('reuses', 'unavailable')} "
        f"allocations={pool.get('allocations', 'unavailable')}",
        f"wire gaps={result['reported_receive_gaps']} max={result['max_gap_ms']} ms "
        f"unpolled max={result['max_unpolled_ms']} ms",
        f"ingress frames dropped={result['frames_dropped_ingress']} "
        f"packets rejected (not frame losses)={result['packets_rejected_not_frame_losses']}",
        f"recovery events={result['recovery_events']} "
        f"max recovery={result['max_recovery_ms']} ms",
        f"audio windows={result['audio_windows']} audible underruns={result['audible_underruns']} "
        f"max audio ahead={result['audio_ahead_max_ms']} ms "
        f"median A/V offset={result['audio_av_offset_median_ms']} ms",
        f"error lines={result['error_lines']}",
        f"NOTE: {result['note']}",
    ])


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('log', type=Path, help='new Punktfunk client log')
    parser.add_argument('--compare', type=Path, help='previous Punktfunk client log')
    parser.add_argument('--json', action='store_true', help='emit machine-readable results')
    args = parser.parse_args()
    result = analyze(args.log.read_text(encoding='utf-8', errors='replace'))
    previous = (analyze(args.compare.read_text(encoding='utf-8', errors='replace'))
                if args.compare else None)
    if args.json:
        print(json.dumps({'current': result, 'previous': previous}, indent=2))
    else:
        if previous:
            print(format_report(previous, str(args.compare)), '\n')
        print(format_report(result, str(args.log)))
        if previous:
            print('\nDifferent scenes/idle intervals make aggregate FPS and bitrate non-comparable '
                  'as a strict performance benchmark. Use identical moving scenes for A/B.')


if __name__ == '__main__':
    main()
