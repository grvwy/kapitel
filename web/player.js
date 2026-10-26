export function duration(seconds) {
  if (!Number.isFinite(seconds)) return '0:00';
  seconds = Math.max(0, Math.floor(seconds));
  const hours = Math.floor(seconds / 3600);
  return `${hours ? `${hours}:` : ''}${hours ? String(Math.floor(seconds / 60) % 60).padStart(2, '0') : Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
}

export class Player {
  constructor(api, profile, notify, render) {
    this.api = api; this.profile = profile; this.notify = notify; this.render = render;
    this.audio = document.querySelector('#audio'); this.sequence = 0; this.sessionId = null;
    this.book = null; this.files = []; this.fileIndex = 0; this.loading = false; this.pending = null;
    this.syncPromise = null; this.lastCheckpoint = 0; this.sleepDeadline = null; this.disposed = false;
    this.storageKey = `kapitel:position:${profile.id}`;
    this.audio.playbackRate = profile.playback_rate;
    this.controller = new AbortController();
    const signal = this.controller.signal;
    this.audio.addEventListener('timeupdate', () => {
      this.update(); this.checkSleep();
      if (!this.audio.paused && Date.now() - this.lastCheckpoint >= 5000) this.checkpoint();
    }, { signal });
    this.audio.addEventListener('pause', () => { if (!this.loading) this.checkpoint(); this.update(); }, { signal });
    this.audio.addEventListener('play', () => this.update(), { signal });
    this.audio.addEventListener('seeked', () => { if (!this.loading) this.checkpoint(); this.update(); }, { signal });
    this.audio.addEventListener('ended', () => this.nextFile(), { signal });
    this.audio.addEventListener('error', () => { if (this.book) this.notify('This audio could not be played. Check the file and your connection.'); }, { signal });
    document.addEventListener('visibilitychange', () => {
      this.checkSleep(); if (document.hidden) this.checkpoint(); else this.flush();
    }, { signal });
    window.addEventListener('online', () => this.flush(), { signal });
    document.addEventListener('keydown', event => {
      if (['INPUT', 'TEXTAREA', 'SELECT', 'BUTTON', 'A'].includes(event.target.tagName) || !this.book || event.altKey || event.ctrlKey || event.metaKey) return;
      if (event.code === 'Space') { event.preventDefault(); this.toggle(); }
      if (event.code === 'ArrowLeft') { event.preventDefault(); this.skip(-10); }
      if (event.code === 'ArrowRight') { event.preventDefault(); this.skip(10); }
    }, { signal });
    this.timer = setInterval(() => { this.checkSleep(); if (!this.audio.paused) this.checkpoint(); else if (this.pending) this.flush(); }, 5000);
    this.recover();
    this.bindControls(document.querySelector('#player-dock'));
    document.querySelector('#dock-position').addEventListener('change', event => this.seek(Number(event.target.value)), { signal });
    if ('mediaSession' in navigator) {
      const handlers = { play: () => this.play(), pause: () => this.audio.pause(), stop: () => this.stop(), seekbackward: details => this.skip(-(details.seekOffset || 10)), seekforward: details => this.skip(details.seekOffset || 10), previoustrack: () => this.chapter(-1), nexttrack: () => this.chapter(1), seekto: details => this.seek(details.seekTime) };
      for (const [action, handler] of Object.entries(handlers)) { try { navigator.mediaSession.setActionHandler(action, handler); } catch { /* Browser does not implement this action. */ } }
    }
  }
  setStatus(message) { document.querySelector('#sync-status').textContent = message; }
  persist() {
    try { if (this.pending) localStorage.setItem(this.storageKey, JSON.stringify(this.pending)); else localStorage.removeItem(this.storageKey); }
    catch { this.notify('Local recovery is unavailable. Keep a connection until your position is saved.'); }
  }
  async recover() {
    try { const stored = JSON.parse(localStorage.getItem(this.storageKey) || 'null'); if (stored?.sessionId && stored?.update) { this.pending = stored; await this.flush(); } }
    catch { this.notify('A local recovery point could not be read.'); }
  }
  async start(detail, restart = false) {
    if (this.loading) return;
    this.audio.pause(); await this.flush();
    if (this.pending) { this.notify('Your previous position is not saved yet. Reconnect before switching books.'); return; }
    this.loading = true;
    try {
      const result = await this.api('/api/playback-sessions', { method: 'POST', body: { book_id: detail.book.id, restart } });
      this.book = detail.book; this.files = detail.files.filter(file => file.kind === 'audio');
      this.sessionId = result.playback_session_id; this.sequence = 0;
      this.fileIndex = this.files.findIndex(file => file.id === result.position.media_file_id);
      if (this.fileIndex < 0) throw new Error('The saved audio file is unavailable.');
      document.querySelector('#player-dock').hidden = false; document.querySelector('#player-link').hidden = false;
      document.body.classList.add('has-player');
      document.querySelector('#dock-title').textContent = this.book.title;
      document.querySelector('#dock-author').textContent = this.book.author;
      if ('mediaSession' in navigator) navigator.mediaSession.metadata = new MediaMetadata({ title: this.book.title, artist: this.book.author, artwork: this.book.has_cover ? [{ src: `${location.origin}/api/books/${this.book.id}/cover`, type: 'image/jpeg' }] : [] });
      await this.loadFile(result.position.playback_position_seconds);
      this.setStatus('Saved'); this.render();
      await this.play();
    } catch (error) { this.notify(error.message); }
    finally { this.loading = false; this.update(); }
  }
  async loadFile(position = 0) {
    const file = this.files[this.fileIndex];
    if (!file?.available) throw new Error('This part is unavailable. Restore the file and scan the library.');
    this.loading = true;
    this.audio.src = `/api/media/${file.id}`;
    this.audio.playbackRate = this.profile.playback_rate;
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => finish(new Error('Audio is taking too long to load. Check your connection.')), 15000);
      const loaded = () => { this.audio.currentTime = Math.min(position, Number.isFinite(this.audio.duration) ? this.audio.duration : position); finish(); };
      const failed = () => finish(new Error('This file cannot be played by this browser.'));
      const finish = error => { clearTimeout(timer); this.audio.removeEventListener('loadedmetadata', loaded); this.audio.removeEventListener('error', failed); error ? reject(error) : resolve(); };
      this.audio.addEventListener('loadedmetadata', loaded, { once: true }); this.audio.addEventListener('error', failed, { once: true }); this.audio.load();
    });
    this.loading = false; this.update();
  }
  async play() {
    if (!this.book) return;
    if (!this.sessionId) { this.notify('Playback moved to another session. Open the book and choose Resume to take over.'); return; }
    try { await this.audio.play(); }
    catch { this.notify('Tap Play to continue listening.'); }
  }
  toggle() { if (this.audio.paused) this.play(); else this.audio.pause(); }
  stop() { this.audio.pause(); this.checkpoint(); }
  seek(position) {
    if (this.loading || !this.sessionId || !Number.isFinite(this.audio.duration)) return;
    this.audio.currentTime = Math.max(0, Math.min(position, this.audio.duration)); this.update();
  }
  async skip(seconds) {
    if (!this.book || this.loading || !this.sessionId) return;
    let index = this.fileIndex; let position = this.audio.currentTime + seconds;
    while (position < 0 && index > 0) { index--; position += this.files[index].duration_seconds; }
    while (position > this.files[index].duration_seconds && index < this.files.length - 1) { position -= this.files[index].duration_seconds; index++; }
    if (index === this.fileIndex) this.seek(position); else await this.moveTo(index, Math.max(0, position));
  }
  chapters() {
    return this.files.flatMap((file, fileIndex) => file.chapters.length ? file.chapters.map(chapter => ({ ...chapter, fileIndex })) : [{ title: file.title, start_seconds: 0, fileIndex }]);
  }
  chapter(direction) {
    const chapters = this.chapters(); if (!chapters.length) return;
    let current = 0;
    chapters.forEach((chapter, index) => { if (chapter.fileIndex < this.fileIndex || (chapter.fileIndex === this.fileIndex && chapter.start_seconds <= this.audio.currentTime + 0.05)) current = index; });
    if (direction < 0 && this.audio.currentTime - chapters[current].start_seconds > 3) this.moveTo(chapters[current].fileIndex, chapters[current].start_seconds);
    else { const target = chapters[Math.max(0, Math.min(chapters.length - 1, current + direction))]; this.moveTo(target.fileIndex, target.start_seconds); }
  }
  async moveTo(index, position = 0, forcePlay = false) {
    if (this.loading || !this.sessionId) return;
    const wasPlaying = !this.audio.paused || forcePlay;
    this.checkpoint(); this.loading = true; this.audio.pause();
    try { await this.flush(); this.fileIndex = index; await this.loadFile(position); this.checkpoint(); if (wasPlaying) await this.play(); this.render(); }
    catch (error) { this.notify(error.message); }
    finally { this.loading = false; this.update(); }
  }
  nextFile() { this.checkpoint(); if (this.fileIndex < this.files.length - 1) this.moveTo(this.fileIndex + 1, 0, true); }
  checkpoint() {
    if (!this.sessionId || this.loading || !this.files[this.fileIndex] || this.disposed) return;
    this.lastCheckpoint = Date.now();
    this.pending = { sessionId: this.sessionId, update: { media_file_id: this.files[this.fileIndex].id, playback_position_seconds: this.audio.currentTime, sequence_number: ++this.sequence } };
    this.persist(); this.setStatus('Saving…'); this.flush();
  }
  async flush() {
    if (this.syncPromise) { await this.syncPromise; return; }
    if (!this.pending || this.disposed) return;
    const snapshot = this.pending;
    this.syncPromise = (async () => {
      try {
        await this.api(`/api/playback-sessions/${snapshot.sessionId}/position`, { method: 'PUT', body: snapshot.update, keepalive: true });
        if (this.pending === snapshot) { this.pending = null; this.persist(); this.setStatus('Saved'); }
      } catch (error) {
        if (['session_replaced', 'stale_update', 'sign_in_required'].includes(error.code)) {
          this.sessionId = null; this.audio.pause(); this.pending = null; this.persist(); this.setStatus('Playback moved'); this.notify(error.message);
        } else this.setStatus('Not synced · reconnect to save');
      }
    })();
    await this.syncPromise; this.syncPromise = null;
    if (this.pending && this.pending !== snapshot) await this.flush();
  }
  async setRate(rate) {
    try { await this.api('/api/session', { method: 'PATCH', body: { playback_rate: rate } }); this.profile.playback_rate = rate; this.audio.playbackRate = rate; }
    catch (error) { this.notify(error.message); }
  }
  setSleep(minutes) {
    this.sleepMinutes = minutes;
    clearTimeout(this.sleepTimeout); this.sleepDeadline = minutes ? Date.now() + minutes * 60000 : null;
    if (minutes) this.sleepTimeout = setTimeout(() => this.checkSleep(), minutes * 60000);
    this.notify(minutes ? `Playback will stop in ${minutes} minutes.` : 'Sleep timer is off.');
  }
  checkSleep() { if (this.sleepDeadline && Date.now() >= this.sleepDeadline) { this.sleepDeadline = null; this.sleepMinutes = 0; const select = document.querySelector('#sleep'); if (select) select.value = '0'; this.stop(); this.notify('Sleep timer finished. Your place is saved when sync completes.'); } }
  bindControls(root) {
    for (const button of root.querySelectorAll('[data-player]')) button.addEventListener('click', () => {
      ({ toggle: () => this.toggle(), back: () => this.skip(-10), forward: () => this.skip(10), previous: () => this.chapter(-1), next: () => this.chapter(1), stop: () => this.stop() })[button.dataset.player]?.();
    }, { signal: this.controller.signal });
  }
  update() {
    const maximum = Number.isFinite(this.audio.duration) ? this.audio.duration : 1;
    for (const range of document.querySelectorAll('[data-position],#dock-position')) { range.max = maximum; if (document.activeElement !== range) range.value = this.audio.currentTime; }
    for (const label of document.querySelectorAll('[data-time],#dock-time')) label.textContent = `${duration(this.audio.currentTime)} / ${duration(maximum)}`;
    for (const button of document.querySelectorAll('[data-player="toggle"]')) { button.textContent = this.audio.paused ? 'Play' : 'Pause'; button.setAttribute('aria-label', this.audio.paused ? 'Play' : 'Pause'); }
    if ('mediaSession' in navigator) {
      navigator.mediaSession.playbackState = this.audio.paused ? 'paused' : 'playing';
      try { if (Number.isFinite(this.audio.duration) && this.audio.duration > 0) navigator.mediaSession.setPositionState({ duration: this.audio.duration, playbackRate: this.audio.playbackRate, position: Math.min(this.audio.currentTime, this.audio.duration) }); } catch { /* Optional browser capability. */ }
    }
  }
  async dispose() {
    this.audio.pause(); this.checkpoint(); await this.flush();
    if (this.pending) return false;
    this.destroy();
    this.persist(); return true;
  }
  destroy() {
    this.disposed = true; clearInterval(this.timer); clearTimeout(this.sleepTimeout); this.controller.abort();
    this.audio.pause();
    this.audio.removeAttribute('src'); this.audio.load();
    if ('mediaSession' in navigator) { navigator.mediaSession.metadata = null; navigator.mediaSession.playbackState = 'none'; }
    document.querySelector('#player-dock').hidden = true; document.querySelector('#player-link').hidden = true; document.body.classList.remove('has-player');
  }
}
