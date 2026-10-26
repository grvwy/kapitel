import { chromium, webkit } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { gzipSync } from 'node:zlib';

const base = process.env.TEST_BASE_URL || 'http://localhost:8080';
const deadline = setTimeout(() => { console.error('Browser test exceeded five minutes'); process.exit(1); }, 300000);
deadline.unref();
await mkdir('/results', { recursive: true });
for (let attempt = 0; ; attempt++) {
  try { if ((await fetch(`${base}/health`)).ok) break; } catch { /* Wait for the test service. */ }
  if (attempt > 60) throw new Error('Kapitel did not start');
  await new Promise(resolve => setTimeout(resolve, 1000));
}
let payload = 0;
for (const asset of ['/', '/app.js', '/player.js', '/style.css']) {
  const response = await fetch(`${base}${asset}`); assert.equal(response.status, 200);
  payload += gzipSync(Buffer.from(await response.arrayBuffer())).length;
}
assert(payload < 100 * 1024, `Frontend exceeds budget: ${payload}`);
console.log(`Initial frontend: ${payload} bytes gzip`);

for (const [name, engine] of Object.entries({ chromium, webkit })) {
  const browser = await engine.launch({ headless: true });
  const desktop = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
  const mobile = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  desktop.setDefaultTimeout(15000); mobile.setDefaultTimeout(15000);
  const first = await desktop.newPage(), second = await mobile.newPage(); const errors = [];
  try {
  for (const page of [first, second]) {
    page.on('pageerror', error => errors.push(error.message));
    await page.goto(base);
    await page.getByLabel('Username', { exact: true }).fill(name);
    await page.getByLabel('Password', { exact: true }).fill('synthetic browser test password');
    await page.getByRole('button', { name: 'Open my library' }).click();
    await page.getByRole('heading', { name: 'Your library', exact: true }).waitFor();
  }
  for (let attempt = 0; ; attempt++) {
    if (await first.locator('.book-card').count() === 8) break;
    if (attempt > 40) throw new Error(`${name}: library never indexed`);
    await first.reload(); await new Promise(resolve => setTimeout(resolve, 1000));
  }
  await first.screenshot({ path: `/results/${name}-library.png`, fullPage: true });
  await second.reload();
  assert(await second.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'Mobile library overflows');
  await second.screenshot({ path: `/results/${name}-mobile.png`, fullPage: true });
  const csrf = await first.evaluate(async () => (await (await fetch('/api/session')).json()).csrf_token);
  const listing = await first.evaluate(async () => (await (await fetch('/api/books')).json()));
  const book = listing.books.find(book => book.title === 'The Quiet Hours'); assert(book);
  const m4b = listing.books.find(book => book.title === 'A Field Guide to Elsewhere'); assert(m4b);
  const chapterData = await first.evaluate(async id => (await (await fetch(`/api/books/${id}`)).json()), m4b.id);
  assert.equal(chapterData.files[0].chapters.length, 2, 'M4B chapter metadata missing');
  console.log(`${name}: starting MP3 and cross-device checks`);
  await first.bringToFront();
  await first.goto(`${base}/#book/${book.id}`);
  await first.getByRole('button', { name: /Start listening|Resume listening/ }).click();
  await first.locator('.listening').waitFor();
  await first.waitForFunction(() => document.querySelector('#audio').readyState >= 1);
  await first.evaluate(() => { const audio = document.querySelector('#audio'); audio.pause(); audio.currentTime = 9.25; });
  await first.waitForFunction(() => document.querySelector('#sync-status').textContent === 'Saved');
  // Read the server state: the test must not pass on a stale Saved label.
  await first.waitForFunction(async id => { const state = await (await fetch(`/api/books/${id}`)).json(); return Math.abs(state.position?.playback_position_seconds - 9.25) < 0.1; }, book.id);
  await second.bringToFront();
  await second.goto(`${base}/#book/${book.id}`);
  await second.getByRole('button', { name: 'Resume listening' }).click();
  await second.locator('.listening').waitFor();
  await second.waitForFunction(() => document.querySelector('#audio').readyState >= 1);
  assert(Math.abs(await second.locator('#audio').evaluate(audio => audio.currentTime) - 9.25) < 1, 'Cross-device resume mismatch');
  await second.evaluate(() => { const audio = document.querySelector('#audio'); audio.pause(); audio.currentTime = 3.5; });
  await second.waitForFunction(async id => { const state = await (await fetch(`/api/books/${id}`)).json(); return Math.abs(state.position?.playback_position_seconds - 3.5) < 0.1; }, book.id);
  await first.bringToFront();
  await first.evaluate(() => { document.querySelector('#audio').currentTime = 18; });
  await first.waitForFunction(() => document.querySelector('#sync-status').textContent === 'Playback moved');
  await second.bringToFront();
  const retained = await second.evaluate(async id => (await (await fetch(`/api/books/${id}`)).json()).position.playback_position_seconds, book.id);
  assert.equal(retained, 3.5, 'Old tab overwrote a deliberate rewind');
  await second.screenshot({ path: `/results/${name}-player.png`, fullPage: true });
  await second.getByLabel('Speed', { exact: true }).selectOption('1.5');
  await second.waitForFunction(() => document.querySelector('#audio').playbackRate === 1.5);
  await second.getByLabel('Sleep timer', { exact: true }).selectOption('15');
  await second.getByRole('button', { name: 'Stop', exact: true }).click();
  assert.equal(await second.locator('#audio').evaluate(audio => audio.paused), true);
  await second.locator('nav').getByRole('link', { name: 'Library', exact: true }).click();
  assert.equal(await second.locator('#audio').evaluate(audio => audio.currentTime), 3.5, 'Navigation reset the player');
  assert(await second.evaluate(() => document.documentElement.scrollWidth <= innerWidth), 'Mobile player overflows');
  console.log(`${name}: starting network recovery checks`);
  await mobile.setOffline(true);
  await second.evaluate(() => { document.querySelector('#audio').currentTime = 5.5; });
  await second.waitForFunction(() => document.querySelector('#sync-status').textContent.startsWith('Not synced'));
  await mobile.setOffline(false);
  await second.waitForFunction(async id => { const state = await (await fetch(`/api/books/${id}`)).json(); return Math.abs(state.position?.playback_position_seconds - 5.5) < 0.1; }, book.id);
  await second.goto(`${base}/#book/${m4b.id}`);
  console.log(`${name}: starting M4B chapter checks`);
  await second.getByRole('button', { name: /Start listening|Resume listening/ }).click();
  await second.locator('.listening').waitFor();
  await second.waitForFunction(() => document.querySelector('#audio').readyState >= 1);
  await second.evaluate(() => document.querySelector('#audio').pause());
  await second.getByRole('button', { name: 'Next chapter', exact: true }).click();
  await second.waitForFunction(() => Math.abs(document.querySelector('#audio').currentTime - 12) < 0.2);
  await second.getByRole('button', { name: 'Previous chapter', exact: true }).click();
  await second.waitForFunction(() => document.querySelector('#audio').currentTime < 0.2);
  const blocked = await desktop.request.post(`${base}/api/admin/scan`, { headers: { Origin: 'https://evil.example', 'X-CSRF-Token': csrf } });
  assert.equal(blocked.status(), 403);
  assert.deepEqual(errors, [], `${name} JavaScript errors`);
  console.log(`${name}: catalog, MP3/M4B, chapters, handoff, rewind, stale-tab protection, network recovery, speed, timer, navigation and layout passed`);
  } catch (error) {
    for (const [label, page] of [['desktop', first], ['mobile', second]]) {
      await page.screenshot({ path: `/results/${name}-${label}-failure.png`, fullPage: true }).catch(() => {});
      console.error(`${name} ${label}: ${await page.locator('body').innerText().catch(() => 'Page unavailable')}`);
    }
    throw error;
  } finally { await browser.close(); }
}
