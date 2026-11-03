import sys, importlib.util, tempfile, io, argparse
parser = argparse.ArgumentParser()
parser.add_argument('--model-root', required=True, help='Directory containing the downloaded model folder')
args = parser.parse_args()
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'src-tauri/resources'))
import numpy as np
import soundfile as sf
from fastapi.testclient import TestClient
from wake_kws import WakeKws
spec=importlib.util.spec_from_file_location('stt_server','src-tauri/resources/stt-server.py')
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
m._SERVER_TOKEN='test-token'
m.kws=WakeKws(args.model_root, tempfile.gettempdir(), lambda a,s:0)
c=TestClient(m.app)
headers={'authorization':'Bearer test-token','origin':'tauri://localhost'}
assert c.get('/kws/status').status_code==403
assert c.get('/kws/status',headers={'origin':'tauri://localhost'}).status_code==401
assert c.get('/kws/status',headers=headers).json()['ready']
config=dict(phrase='light up',owner_only=False)
r=c.post('/kws/session',json=config,headers=headers);assert r.status_code==200,r.text
sid=r.json()['session']
assert c.post(f'/kws/audio/{sid}?sequence=2',content=b'\0\0'*3200,headers=headers).status_code==422
samples,sr=sf.read(m.kws.path/'test_wavs/en_0.wav',dtype='float32')
from scipy.signal import resample_poly
from math import gcd
g=gcd(sr,16000); samples=resample_poly(samples,16000//g,sr//g)
f=io.BytesIO();sf.write(f,samples,16000,format='WAV',subtype='PCM_16')
import json
r=c.post('/kws/check',headers=headers,files={'file':('wake.wav',f.getvalue(),'audio/wav')},data={'config':json.dumps(config)})
assert r.status_code==200,r.text
assert r.json()['keyword_match'] and r.json()['speaker_match'],r.text
assert not m.kws.sessions
assert c.post('/speaker/train',content=b'').status_code==410
print('HTTP auth, sequence rejection, real WAV detection, cleanup and retired training: PASS')
