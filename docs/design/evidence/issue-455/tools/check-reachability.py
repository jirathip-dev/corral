"""Read back served artifacts and compare exact local bytes. No Serve changes."""
from pathlib import Path
import urllib.request,hashlib,json,datetime,os
ROOT=Path(__file__).resolve().parents[1]
# Portability: bases and served path prefix are overridable so the packaged
# copy can be checked from any localhost mount; defaults keep the canonical
# existing gallery behavior.
BASES=[b for b in os.environ.get('CORRAL455_BASES','http://127.0.0.1:8777,https://jirathips-macbook-air.tail8c3301.ts.net:8444').split(',') if b]
PREFIX=os.environ.get('CORRAL455_PREFIX','/corral/455-immersive-herd')
paths=['index.html','variant-a.html','variant-b.html','evidence/b-day-390x844.png','evidence/day-realtime.mp4','evidence/night-realtime.mp4']
result={'checkedAt':datetime.datetime.now(datetime.timezone.utc).isoformat(),'phoneDeviceTest':False,'networkConfigurationChanged':False,'targets':[]}
for base in BASES:
 for rel in paths:
  url=f'{base}{PREFIX}/{rel}'
  item={'url':url}
  try:
   with urllib.request.urlopen(url,timeout=15) as r:data=r.read();item['status']=r.status
   item['sha256']=hashlib.sha256(data).hexdigest();item['bytes']=len(data);item['matchesLocal']=data==(ROOT/rel).read_bytes()
  except Exception as e:item['error']=str(e);item['matchesLocal']=False
  result['targets'].append(item)
(ROOT/'evidence/reachability.json').write_text(json.dumps(result,indent=2)+'\n')
print(json.dumps(result,indent=2))
