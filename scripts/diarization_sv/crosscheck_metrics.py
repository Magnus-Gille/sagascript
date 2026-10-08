"""Offline independent metric checks. Does not invoke any diarization model."""
import argparse, ast, hashlib, importlib.metadata, json, os, subprocess
from math import isfinite
from pathlib import Path
import numpy as np
from scipy.optimize import linear_sum_assignment
from pyannote.core import Annotation, Segment, Timeline
from pyannote.metrics.diarization import DiarizationErrorRate
from crosscheck_metric_compare import MISSING, compare_metric

p = argparse.ArgumentParser()
p.add_argument('--binary', type=Path, required=True)
p.add_argument('--scratch', type=Path, required=True, help='New output directory; never overwritten')
p.add_argument('--dscore-source', type=Path, required=True, help='Pinned public dscore metrics.py from e02f949ac6592279300a2c33d03daf9e0c12fd27')
a = p.parse_args()
out = a.scratch
out.mkdir(parents=True, exist_ok=False)
SOURCE = a.dscore_source
if not SOURCE.is_file() or SOURCE.stat().st_size > 1024 * 1024:
    p.error('dscore source must be a regular file below 1 MiB')
scorer_bytes = SOURCE.read_bytes()
if hashlib.sha256(scorer_bytes).hexdigest() != 'a02d0f1c8c78872dbd9c5d4da34819f31ea971822fa40675608502b091fccdc6':
    p.error('dscore source does not match the inspected pinned revision')
# Execute only the inspected numeric JER function, never module imports or its Perl runner.
tree = ast.parse(scorer_bytes.decode('utf-8'))
function = next(node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'jer')
namespace = {'np': np, 'linear_sum_assignment': linear_sum_assignment}
exec(compile(ast.Module(body=[function], type_ignores=[]), str(SOURCE), 'exec'), namespace)
env = {k: os.environ[k] for k in ('HOME','USER','LOGNAME','PATH','LANG','TMPDIR') if k in os.environ}
version = subprocess.check_output([str(a.binary), '--version'], env=env, text=True).strip()
template = {'schema_version': 1, 'source_sha256': 'a'*64, 'duration_seconds': 1.0,
            'build_revision': 'synthetic-fixture', 'build_version': 'synthetic-fixture',
            'parameters': {'threshold': .34, 'min_segment_seconds': .3, 'min_gap_seconds': .3,
                           'min_speaker_seconds': 8.0, 'absorb_max_distance': .75, 'hint_merge_max_distance': .75},
            'activity': [], 'regions': [], 'attributions': []}

def union(turns):
    by_speaker = {}
    for start, end, speaker in turns:
        by_speaker.setdefault(speaker, []).append((start, end))
    result = []
    for speaker, intervals in sorted(by_speaker.items()):
        merged = []
        for start, end in sorted(intervals):
            if merged and start <= merged[-1][1]: merged[-1] = (merged[-1][0], max(end, merged[-1][1]))
            else: merged.append((start, end))
        result.extend((start, end, speaker) for start, end in merged)
    return result

def annotation(turns):
    result = Annotation()
    for index, (start, end, speaker) in enumerate(union(turns)):
        result[Segment(start, end), index] = speaker
    return result

def independent_jer(ref, hyp, uem, duration):
    # Synthetic boundaries are grid-aligned. Public cases use a 1ms independent frame grid.
    times = (np.arange(int(np.ceil(duration * 1000))) + .5) / 1000
    keep = np.zeros(times.size, bool)
    for start, end in uem: keep |= (times >= start) & (times < end)
    times = times[keep]
    def matrix(turns):
        speakers = sorted({s for _, _, s in turns})
        rows = np.zeros((len(speakers), times.size), bool)
        for start, end, speaker in turns:
            rows[speakers.index(speaker)] |= (times >= start) & (times < end)
        return rows[rows.any(axis=1)]
    r, h = matrix(ref), matrix(hyp)
    rd, hd = r.sum(axis=1).astype(float), h.sum(axis=1).astype(float)
    cm = r.astype(float) @ h.T.astype(float)
    return namespace['jer']({'test': rd}, {'test': hd}, {'test': cm})[1] / 100

def has_original_reference_speech(ref, uem):
    """Whether any reference speaker support survives in the original UEM."""
    return any(max(start, uem_start) < min(end, uem_end)
               for start, end, _ in ref
               for uem_start, uem_end in uem)

fixtures = [
    ('perfect', [(0,4,'a'),(4,8,'b')], [(0,4,'x'),(4,8,'y')], [(0,10)], 10),
    ('miss-overlap', [(1,5,'a'),(2,4,'b')], [(1,5,'x')], [(0,8)], 8),
    ('three-way-overlap', [(1,5,'a'),(1,5,'b'),(1,5,'c')], [(1,5,'x'),(1,5,'y')], [(0,8)], 8),
    ('silence-false-alarm', [(1,3,'a')], [(1,3,'x'),(5,7,'x')], [(0,8)], 8),
    ('extra-cluster', [(1,7,'a')], [(1,4,'x'),(4,7,'y')], [(0,8)], 8),
    ('uem-hole', [(0,8,'a'),(3,5,'b')], [(0,3,'x'),(3,5,'z'),(5,8,'x')], [(0,3),(5,9)], 9),
    ('identity-switch', [(0,4,'a'),(4,8,'b'),(8,12,'a'),(12,16,'b')], [(0,4,'x'),(4,8,'y'),(8,12,'y'),(12,16,'x')], [(0,16)], 16),
    ('duplicate-union', [(0,4,'a'),(1,3,'a'),(4,6,'a')], [(0,6,'x')], [(0,8)], 8),
    ('boundary-shift', [(1,3,'a'),(4,6,'b')], [(1.2,3.2,'x'),(3.8,5.8,'y')], [(0,8)], 8),
    ('no-hypothesis', [(1,3,'a'),(2,4,'b')], [], [(0,8)], 8),
    ('iou-mapping', [(0,1,'a'),(1,17,'b'),(17,20,'a'),(20,25,'b')], [(0,10,'x'),(10,17,'y'),(25,34,'x'),(34,41,'y')], [(0,42)], 42),
    # The reference turn is outside the UEM: raw false-alarm seconds remain
    # defined while normalized DER and native JER are intentionally null.
    ('zero-reference-speaker-time', [(8,9,'a')], [(1,3,'x')], [(0,8)], 8),
    # The native collar removes this short reference turn from DER, while
    # JER still scores its original-UEM support and remains a real number.
    ('collar-erases-short-reference', [(4.0,4.1,'a')], [(4.0,4.1,'x')], [(0,8)], 8),
]
receipts = []
for name, ref, hyp, uem, duration in fixtures:
    case = out / name
    case.mkdir()
    recording = 'a' * 64
    (case/'ref.rttm').write_text(''.join(f'SPEAKER {recording} 1 {s:.6f} {e-s:.6f} <NA> <NA> {speaker} <NA> <NA>\n' for s,e,speaker in ref))
    (case/'uem.txt').write_text(''.join(f'{recording} 1 {s:.6f} {e:.6f}\n' for s,e in uem))
    report = dict(template, source_sha256=recording, duration_seconds=duration,
                  activity=[{'start':s,'end':e,'speakers':[speaker]} for s,e,speaker in hyp],
                  regions=[], attributions=[], asr_segments=[], diagnostics_included=False)
    # Activity is represented as disjoint sets rather than overlapping tuples.
    bounds = sorted({v for s,e,_ in hyp for v in (s,e)})
    report['activity'] = [{'start':s,'end':e,'speakers':sorted({speaker for hs,he,speaker in hyp if hs < e and he > s})} for s,e in zip(bounds,bounds[1:]) if any(hs < e and he > s for hs,he,_ in hyp)]
    (case/'hyp.json').write_text(json.dumps(report))
    expected_jer = independent_jer(ref,hyp,uem,duration)
    for collar in (0.0, .25):
        raw = subprocess.check_output([str(a.binary),'diarization','evaluate','--reference',str(case/'ref.rttm'),'--hypothesis',str(case/'hyp.json'),'--uem',str(case/'uem.txt'),'--collar',str(collar)], env=env, text=True)
        native = json.loads(raw)
        (case/f'native-{collar}.json').write_text(raw)
        details = DiarizationErrorRate(collar=2*collar, skip_overlap=False)(annotation(ref),annotation(hyp),uem=Timeline([Segment(s,e) for s,e in uem]), detailed=True)
        reference_seconds = details['total']
        normalized_der = details['diarization error rate'] if reference_seconds > 0 else None
        if normalized_der is not None and not isfinite(normalized_der):
            normalized_der = None
        # Native JER has no defined speaker-average when UEM contains no
        # reference speaker-time. Keep that semantic limit explicit instead
        # of comparing dscore's no-reference sentinel to native null.
        normalized_jer = expected_jer if has_original_reference_speech(ref, uem) else None
        expected = {'der':normalized_der, 'reference_speaker_seconds':reference_seconds, 'miss_seconds':details['missed detection'], 'false_alarm_seconds':details['false alarm'], 'confusion_seconds':details['confusion'], 'jer':normalized_jer}
        differences = {}
        metric_passes = {}
        for key, value in expected.items():
            differences[key], metric_passes[key] = compare_metric(native['metrics'].get(key, MISSING), value)
        receipts.append({'case':name,'collar_seconds':collar,'expected':expected,'differences':differences,'metric_passes':metric_passes,'pass':all(metric_passes.values())})
result = {'binary_version':version,'binary_sha256':hashlib.sha256(a.binary.read_bytes()).hexdigest(),'dscore_revision':'e02f949ac6592279300a2c33d03daf9e0c12fd27','dscore_source_sha256':hashlib.sha256(SOURCE.read_bytes()).hexdigest(),'pyannote_metrics_version':importlib.metadata.version('pyannote.metrics'),'jer_domain':'independent JER on original UEM with a 1ms grid and IoU-optimal assignment; undefined only when no reference speaker support intersects original UEM','der_domain':'independent DER on explicit UEM with native half-width collar; pyannote uses collar=2*half-width; undefined DER remains null when reference speaker-time is zero','checks':receipts,'passed':all(item['pass'] for item in receipts)}
(out/'receipt.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps({'passed':result['passed'],'checks':len(receipts),'failures':[{'case':r['case'],'collar':r['collar_seconds'],'differences':r['differences']} for r in receipts if not r['pass']]}))
raise SystemExit(0 if result['passed'] else 1)
