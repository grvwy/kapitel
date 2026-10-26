"""Generate a 1,000-book synthetic library and measure the real index and API."""
import http.cookiejar
import json
from pathlib import Path
import statistics
import time
import urllib.request

opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
request = urllib.request.Request('http://localhost:8080/api/session', data=json.dumps({'username': 'owner', 'password': 'synthetic browser test password'}).encode(), headers={'Origin': 'http://localhost:8080', 'Content-Type': 'application/json'})
with opener.open(request) as response:
    assert response.status == 200
with opener.open('http://localhost:8080/api/books') as response:
    existing = json.load(response)['total']
for number in range(1000 - existing):
    path = Path('/library') / f'Synthetic shelf {number:04}'
    path.mkdir(exist_ok=True)
    (path / 'book.pdf').write_bytes(f'%PDF-1.4\nSynthetic catalog fixture {number}\n'.encode())
for attempt in range(120):
    with opener.open('http://localhost:8080/api/books') as response:
        if json.load(response)['total'] == 1000:
            break
    time.sleep(1)
else:
    raise AssertionError('The 1,000-book library did not finish indexing')
samples = []
for _ in range(30):
    started = time.perf_counter()
    with opener.open('http://localhost:8080/api/books') as response:
        data = json.load(response)
    samples.append((time.perf_counter() - started) * 1000)
    assert data['total'] == 1000 and len(data['books']) == 50
print(json.dumps({'books': 1000, 'page_size': 50, 'median_ms': round(statistics.median(samples), 2), 'p95_ms': round(sorted(samples)[28], 2)}))
