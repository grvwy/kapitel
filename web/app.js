import { Player, duration } from './player.js';

const content = document.querySelector('#content');
let profile = null, csrfToken = null, player = null, renderVersion = 0, noticeTimeout;
export function element(tag, attributes = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attributes)) {
    if (key.startsWith('on')) node.addEventListener(key.slice(2), value);
    else if (key === 'class') node.className = value;
    else if (key === 'text') node.textContent = value;
    else if (value !== false && value != null) node.setAttribute(key, value === true ? '' : value);
  }
  for (const child of children.flat()) if (child != null) node.append(typeof child === 'string' ? document.createTextNode(child) : child);
  return node;
}
function notify(message) {
  const notice = document.querySelector('#notice'); notice.textContent = message; notice.hidden = false;
  clearTimeout(noticeTimeout); noticeTimeout = setTimeout(() => { notice.hidden = true; }, 9000);
}
async function api(path, options = {}) {
  const headers = { ...options.headers };
  if (options.body) headers['Content-Type'] = 'application/json';
  if (csrfToken) headers['X-CSRF-Token'] = csrfToken;
  let response;
  try { response = await fetch(path, { credentials: 'same-origin', ...options, body: options.body ? JSON.stringify(options.body) : undefined, headers }); }
  catch { throw new Error('Cannot reach Kapitel. Check your connection.'); }
  if (!response.ok) {
    const body = await response.json().catch(() => null);
    const error = new Error(body?.error?.message || `The request failed (${response.status}).`); error.code = body?.error?.code; throw error;
  }
  return response.status === 204 || response.status === 202 ? null : response.json();
}
function button(label, action, className = '') { return element('button', { type: 'button', class: className, onclick: action }, label); }
function field(label, name, value = '', type = 'text') {
  return element('label', { class: 'field' }, element('span', {}, label), element('input', { name, type, value, required: ['username', 'password', 'title'].includes(name), autocomplete: type === 'password' ? 'new-password' : 'off' }));
}
function artwork(book, className = '') {
  return element('div', { class: `artwork tone-${parseInt(book.id.slice(0, 2), 16) % 6} ${className}` }, book.has_cover ? element('img', { src: `/api/books/${book.id}/cover`, alt: '', loading: 'lazy', width: 360, height: 480 }) : [element('span', { class: 'cover-author' }, book.author || 'YOUR LIBRARY'), element('span', { class: 'cover-title' }, book.title), element('span', { class: 'cover-mark', 'aria-hidden': 'true' }, 'k.')]);
}
function card(book) {
  return element('a', { href: `#book/${book.id}`, class: 'book-card' }, artwork(book), element('div', { class: 'card-info' }, element('span', { class: 'eyebrow' }, book.has_audio ? 'AUDIOBOOK' : 'EBOOK'), element('h3', {}, book.title), element('p', { class: 'muted' }, book.author || 'Unknown author'), !book.available ? element('span', { class: 'badge' }, 'Unavailable') : null));
}
function loginPage() {
  player?.destroy(); player = null; profile = null; csrfToken = null;
  document.querySelector('#header').hidden = true;
  const error = element('p', { class: 'error', role: 'alert' });
  const username = field('Username', 'username'); username.querySelector('input').autocomplete = 'username';
  const password = field('Password', 'password', '', 'password'); password.querySelector('input').autocomplete = 'current-password';
  const form = element('form', { class: 'login-form', onsubmit: async event => {
    event.preventDefault(); const submit = form.querySelector('button'); submit.disabled = true; error.textContent = '';
    try {
      const data = new FormData(form); const result = await api('/api/session', { method: 'POST', body: { username: data.get('username'), password: data.get('password') } });
      setSession(result); await route();
    } catch (failure) { error.textContent = failure.message; } finally { submit.disabled = false; }
  } }, username, password, error, element('button', { type: 'submit', class: 'primary' }, 'Open my library'));
  content.replaceChildren(element('section', { class: 'login' }, element('span', { class: 'brand-mark large', 'aria-hidden': 'true' }, 'k.'), element('p', { class: 'eyebrow' }, 'KAPITEL'), element('h1', {}, 'Your library.', element('br'), 'Your place.'), element('p', { class: 'muted intro' }, 'A little space for your next chapter.'), form));
}
function setSession(result) {
  player?.destroy();
  profile = result.profile; csrfToken = result.csrf_token;
  document.querySelector('#header').hidden = false; document.querySelector('#admin-link').hidden = !profile.is_admin;
  player = new Player(api, profile, notify, () => { if (location.hash === '#player') route(); });
  document.querySelector('#sign-out').textContent = `${profile.username} · Sign out`;
}
async function libraryPage(version) {
  const parameters = new URLSearchParams((location.hash.split('?')[1] || ''));
  const [library, recent] = await Promise.all([api(`/api/books?${parameters}`), api('/api/books?sort=recent&limit=6')]);
  if (version !== renderVersion) return;
  const search = element('input', { type: 'search', name: 'search', placeholder: 'Search your books or authors', value: parameters.get('search') || '', 'aria-label': 'Search books or authors' });
  const filters = element('div', { class: 'filters', role: 'group', 'aria-label': 'Media type' });
  for (const [kind, label] of [['', 'All books'], ['audio', 'Audiobooks'], ['ebook', 'Ebooks']]) filters.append(button(label, () => { parameters.set('kind', kind); parameters.delete('offset'); location.hash = `library?${parameters}`; }, (parameters.get('kind') || '') === kind ? 'selected' : 'quiet'));
  const form = element('form', { class: 'search', onsubmit: event => { event.preventDefault(); parameters.set('search', search.value); parameters.delete('offset'); location.hash = `library?${parameters}`; } }, search, element('button', { type: 'submit' }, 'Search'));
  const nodes = [element('div', { class: 'page-heading' }, element('div', {}, element('p', { class: 'eyebrow' }, 'MAKE ROOM FOR A GOOD BOOK'), element('h1', {}, 'Your library')), element('p', { class: 'muted' }, 'Pick up where you left off.'))];
  if (!parameters.get('search') && !parameters.get('kind') && !parameters.get('offset') && recent.books.length) nodes.push(element('section', { class: 'continue-section' }, element('h2', {}, 'Continue listening'), element('div', { class: 'continue-grid' }, recent.books.map(book => element('a', { class: 'continue-card', href: `#book/${book.id}` }, artwork(book), element('div', {}, element('p', { class: 'eyebrow' }, 'YOUR NEXT CHAPTER'), element('h3', {}, book.title), element('p', { class: 'muted' }, book.author), element('span', { class: 'continue-label' }, 'Continue →')))))));
  nodes.push(element('div', { class: 'library-tools' }, filters, form), element('div', { class: 'section-heading' }, element('h2', {}, 'On your shelf'), element('span', { class: 'muted' }, `${library.total} ${library.total === 1 ? 'book' : 'books'}`)));
  nodes.push(library.books.length ? element('div', { class: 'book-grid' }, library.books.map(card)) : element('div', { class: 'empty-state' }, element('h2', {}, parameters.get('search') ? 'No books found' : 'A new chapter starts here'), element('p', { class: 'muted' }, parameters.get('search') ? 'Try another title or author.' : 'Your books will appear here after the library is scanned.'), profile.is_admin ? element('a', { href: '#admin' }, 'Check library status →') : null));
  const offset = Number(parameters.get('offset') || 0); const pagination = element('div', { class: 'pagination' });
  if (offset > 0) pagination.append(button('← Previous', () => { parameters.set('offset', Math.max(0, offset - 50)); location.hash = `library?${parameters}`; }));
  if (offset + 50 < library.total) pagination.append(button('Next →', () => { parameters.set('offset', offset + 50); location.hash = `library?${parameters}`; }));
  nodes.push(pagination); content.replaceChildren(...nodes);
}
async function bookPage(bookId, version) {
  const detail = await api(`/api/books/${bookId}`); if (version !== renderVersion) return;
  const { book, files, position } = detail; document.title = `${book.title} — Kapitel`;
  const audioFiles = files.filter(file => file.kind === 'audio');
  const actions = element('div', { class: 'book-actions' });
  if (book.has_audio) {
    actions.append(button(position ? 'Resume listening' : 'Start listening', async () => { await player.start(detail); if (player.book?.id === book.id) location.hash = 'player'; }, 'primary'));
    if (position) actions.append(button('Start over', () => { if (confirm('Start this book again from the beginning?')) player.start(detail, true); }, 'quiet'));
  }
  for (const file of files.filter(file => file.kind === 'ebook')) actions.append(element('span', { class: 'badge' }, file.content_type === 'application/epub+zip' ? 'EPUB' : file.content_type === 'application/pdf' ? 'PDF' : 'AZW3'));
  if (!book.has_audio) actions.append(element('p', { class: 'muted' }, 'In your ebook collection. Reading in Kapitel is coming later.'));
  const notes = [book.narrator && `Narrated by ${book.narrator}`, book.publication_year, book.language, audioFiles.length && `${duration(audioFiles.reduce((total, file) => total + file.duration_seconds, 0))} total`].filter(Boolean).join(' · ');
  const nodes = [element('a', { href: '#library', class: 'back-link' }, '← Your library'), element('section', { class: 'book-detail' }, artwork(book), element('div', { class: 'book-description' }, element('p', { class: 'eyebrow' }, book.has_audio ? 'AUDIOBOOK' : 'EBOOK'), element('h1', {}, book.title), element('p', { class: 'author' }, book.author || 'Unknown author'), element('p', { class: 'muted' }, notes), position ? element('p', { class: 'saved-place' }, `Your saved place: ${duration(position.playback_position_seconds)} in ${files.find(file => file.id === position.media_file_id)?.title || 'your last part'}`) : null, actions, element('p', { class: 'description' }, book.description)))];
  if (audioFiles.length) nodes.push(element('section', { class: 'parts' }, element('h2', {}, 'Inside this book'), element('ol', {}, audioFiles.map(file => element('li', {}, element('span', {}, file.title), element('span', { class: 'muted' }, `${duration(file.duration_seconds)}${file.available ? '' : ' · Unavailable'}`))))));
  if (profile.is_admin) nodes.push(metadataEditor(detail)); content.replaceChildren(...nodes);
}
function metadataEditor(detail) {
  const { book, files } = detail; const error = element('p', { role: 'alert', class: 'error' });
  const form = element('form', { class: 'metadata-form', onsubmit: async event => {
    event.preventDefault(); const data = new FormData(form); const metadata = Object.fromEntries(['title', 'author', 'narrator', 'publication_year', 'language', 'description'].map(key => [key, data.get(key)]));
    try { await api(`/api/admin/books/${book.id}`, { method: 'PATCH', body: { metadata } }); notify('Book details saved.'); route(); } catch (failure) { error.textContent = failure.message; }
  } }, ['title', 'author', 'narrator', 'publication_year', 'language'].map(key => field(key.replaceAll('_', ' '), key, book[key])), element('label', { class: 'field full' }, 'Description', element('textarea', { name: 'description', rows: 5 }, book.description)), error, element('button', { type: 'submit', class: 'primary' }, 'Save details'));
  const audio = files.filter(file => file.kind === 'audio');
  const orderList = element('ol', { class: 'order-list' });
  function renderOrder() { orderList.replaceChildren(...audio.map((file, index) => element('li', {}, element('span', {}, file.title), button('↑', () => { if (index) { [audio[index - 1], audio[index]] = [audio[index], audio[index - 1]]; renderOrder(); } }), button('↓', () => { if (index < audio.length - 1) { [audio[index + 1], audio[index]] = [audio[index], audio[index + 1]]; renderOrder(); } })))); }
  renderOrder();
  return element('details', { class: 'editor' }, element('summary', {}, 'Edit book details'), form, audio.length > 1 ? element('div', {}, element('h3', {}, 'Part order'), orderList, button('Save part order', async () => { try { await api(`/api/admin/books/${book.id}`, { method: 'PATCH', body: { audio_file_order: audio.map(file => file.id) } }); notify('Part order saved. Reopen playback to use it.'); route(); } catch (failure) { notify(failure.message); } })) : null);
}
function playerPage() {
  if (!player.book) { content.replaceChildren(element('div', { class: 'empty-state' }, element('h1', {}, 'Find your next chapter'), element('a', { href: '#library' }, 'Choose a book →'))); return; }
  const book = player.book;
  const controls = element('div', { class: 'listening-controls' }, [['previous', 'Previous chapter'], ['back', '−10s'], ['toggle', player.audio.paused ? 'Play' : 'Pause'], ['forward', '+10s'], ['next', 'Next chapter']].map(([action, label]) => element('button', { type: 'button', 'data-player': action, class: action === 'toggle' ? 'primary' : '' }, label)));
  const range = element('input', { type: 'range', min: 0, max: 1, step: 0.1, value: 0, 'data-position': true, 'aria-label': 'Position in current part', onchange: event => player.seek(Number(event.target.value)) });
  const speed = element('select', { id: 'speed', 'aria-label': 'Speed', onchange: event => player.setRate(Number(event.target.value)) }, [0.75, 1, 1.25, 1.5, 1.75, 2].map(rate => element('option', { value: rate, selected: rate === profile.playback_rate }, `${rate}×`)));
  const sleep = element('select', { id: 'sleep', 'aria-label': 'Sleep timer', onchange: event => player.setSleep(Number(event.target.value)) }, [0, 15, 30, 45, 60].map(minutes => element('option', { value: minutes, selected: minutes === (player.sleepMinutes || 0) }, minutes ? `${minutes} minutes` : 'Off')));
  const chapters = element('ol', { class: 'chapter-list' }, player.chapters().map(chapter => element('li', {}, button(chapter.title, () => player.moveTo(chapter.fileIndex, chapter.start_seconds)), element('span', { class: 'muted' }, duration(chapter.start_seconds)))));
  content.replaceChildren(element('section', { class: 'listening' }, element('a', { href: `#book/${book.id}`, class: 'back-link' }, '← Book details'), artwork(book), element('p', { class: 'eyebrow' }, 'A LITTLE TIME, JUST FOR YOU'), element('h1', {}, book.title), element('p', { class: 'author' }, book.author), element('p', { class: 'muted' }, player.files[player.fileIndex]?.title), controls, range, element('p', { class: 'time-row', 'data-time': true }), element('div', { class: 'player-options' }, element('label', {}, 'Speed ', speed), element('label', {}, 'Sleep timer ', sleep), button('Stop', () => player.stop(), 'quiet')), element('details', {}, element('summary', {}, 'Chapters & parts'), chapters)));
  player.bindControls(controls); player.update();
}
async function adminPage(version) {
  if (!profile.is_admin) throw new Error('Administrator access is required.');
  const [status, profiles] = await Promise.all([api('/api/admin/scan'), api('/api/admin/profiles')]); if (version !== renderVersion) return;
  const error = element('p', { role: 'alert', class: 'error' });
  const newProfile = element('form', { class: 'profile-form', onsubmit: async event => {
    event.preventDefault(); const data = new FormData(newProfile);
    try { await api('/api/admin/profiles', { method: 'POST', body: { username: data.get('username'), password: data.get('password'), is_admin: data.get('is_admin') === 'on' } }); notify('Profile created.'); route(); } catch (failure) { error.textContent = failure.message; }
  } }, field('Username', 'username'), field('Password · at least 15 characters', 'password', '', 'password'), element('label', {}, element('input', { type: 'checkbox', name: 'is_admin' }), ' Administrator'), error, element('button', { type: 'submit', class: 'primary' }, 'Create profile'));
  const profileList = element('div', { class: 'profile-list' }, profiles.map(account => {
    const reset = element('details', {}, element('summary', {}, 'Reset password'));
    const resetForm = element('form', { onsubmit: async event => { event.preventDefault(); try { await api(`/api/admin/profiles/${account.id}`, { method: 'PATCH', body: { password: new FormData(resetForm).get('password') } }); notify('Password reset. Existing sessions are signed out.'); resetForm.reset(); } catch (failure) { notify(failure.message); } } }, field('New password', 'password', '', 'password'), element('button', { type: 'submit' }, 'Save password'));
    reset.append(resetForm);
    return element('div', { class: 'profile-row' }, element('div', {}, element('strong', {}, account.username), element('span', { class: 'muted' }, ` · ${account.is_admin ? 'Administrator' : 'Listener'} · ${account.enabled ? 'Active' : 'Disabled'}`)), button(account.enabled ? 'Disable' : 'Enable', async () => { try { await api(`/api/admin/profiles/${account.id}`, { method: 'PATCH', body: { enabled: !account.enabled } }); route(); } catch (failure) { notify(failure.message); } }, 'quiet'), reset);
  }));
  content.replaceChildren(element('div', { class: 'page-heading' }, element('div', {}, element('p', { class: 'eyebrow' }, 'KEEP THINGS IN ORDER'), element('h1', {}, 'Your library, managed.'))), element('section', { class: 'panel' }, element('div', { class: 'section-heading' }, element('h2', {}, 'Library status'), button(status.scan.running ? 'Scanning…' : 'Scan now', async () => { try { await api('/api/admin/scan', { method: 'POST' }); notify('Library scan requested.'); setTimeout(() => { if (location.hash === '#admin') route(); }, 1000); } catch (failure) { notify(failure.message); } })), element('p', { class: 'muted' }, status.scan.message || 'Waiting for the first scan.'), element('p', {}, `${status.issue_count} items need attention`), element('ul', { class: 'issue-list' }, status.issues.map(issue => element('li', {}, element('strong', {}, issue.path), element('p', { class: 'muted' }, issue.message))))), element('section', { class: 'panel' }, element('h2', {}, 'Profiles'), profileList, element('details', {}, element('summary', {}, 'Create a profile'), newProfile)));
}
async function route() {
  if (!profile) { loginPage(); return; }
  const version = ++renderVersion; const hash = location.hash.slice(1) || 'library'; document.title = 'Kapitel — Your library';
  try {
    if (hash.startsWith('book/')) await bookPage(hash.slice(5), version);
    else if (hash === 'player') playerPage();
    else if (hash === 'admin') await adminPage(version);
    else await libraryPage(version);
  } catch (error) {
    if (version !== renderVersion) return;
    content.replaceChildren(element('section', { class: 'empty-state' }, element('h1', {}, 'Something needs attention'), element('p', {}, error.message), button('Try again', route), error.code === 'sign_in_required' ? button('Sign in', () => { profile = null; csrfToken = null; loginPage(); }) : element('a', { href: '#library' }, 'Back to library')));
  }
}
window.addEventListener('hashchange', route);
document.querySelector('#sign-out').addEventListener('click', async () => {
  if (player && !await player.dispose()) { notify('Your position has not synced. Reconnect before signing out.'); return; }
  try { await api('/api/session', { method: 'DELETE' }); profile = null; csrfToken = null; player = null; loginPage(); } catch (error) { notify(error.message); }
});
try { setSession(await api('/api/session')); await route(); } catch { loginPage(); }
