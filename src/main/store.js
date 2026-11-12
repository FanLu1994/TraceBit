'use strict';
// TraceBit day store.
// Events live in plain JSONL files: <userData>/data/days/YYYY-MM-DD.jsonl
// Screenshots: <userData>/data/snapshots/YYYY-MM-DD/HHmmss-xxxx.jpg
//
// Event kinds:
//   app_focus  {app, name, title, pid, i}   — foreground app/title changed
//   tick       {i}                           — periodic activity tick (i = idleMs)
//   screenshot {file, w, h, bytes, trigger, diff}
//   site_visit {site, app, name, pid, i}       - opt-in browser hostname only
//   lock / unlock / resume / suspend
//   note       {text}                        — user note (future)
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');

function localDateKey(ts) {
  const d = new Date(ts);
  const p = (n) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

function hhmm(ts) {
  const d = new Date(ts);
  const p = (n) => String(n).padStart(2, '0');
  return `${p(d.getHours())}:${p(d.getMinutes())}`;
}

class DayStore {
  constructor(dataDir) {
    this.dataDir = dataDir;
    this.daysDir = path.join(dataDir, 'days');
    this.snapshotsDir = path.join(dataDir, 'snapshots');
    fs.mkdirSync(this.daysDir, { recursive: true });
    fs.mkdirSync(this.snapshotsDir, { recursive: true });
    this._cache = new Map(); // dateKey -> {mtimeMs, events}
  }

  // ---------------------------------------------------------------- writing

  append(event) {
    const ts = event.ts || Date.now();
    const dateKey = localDateKey(ts);
    const file = this._dayFile(dateKey);
    const line = JSON.stringify({ ...event, ts });
    try {
      fs.appendFileSync(file, line + '\n', 'utf-8');
      const c = this._cache.get(dateKey);
      if (c) c.events.push(JSON.parse(line));
    } catch (err) {
      console.error('[Store] append failed:', err.message);
    }
    return dateKey;
  }

  _dayFile(dateKey) {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(dateKey)) throw new Error('bad dateKey');
    return path.join(this.daysDir, `${dateKey}.jsonl`);
  }

  snapshotDir(dateKey) {
    return path.join(this.snapshotsDir, dateKey);
  }

  // ---------------------------------------------------------------- reading

  readDay(dateKey) {
    const file = this._dayFile(dateKey);
    let mtimeMs = 0;
    try {
      mtimeMs = fs.statSync(file).mtimeMs;
    } catch {
      return [];
    }
    const cached = this._cache.get(dateKey);
    if (cached && cached.mtimeMs === mtimeMs) return cached.events;
    const events = [];
    try {
      const lines = fs.readFileSync(file, 'utf-8').split('\n');
      for (const line of lines) {
        const t = line.trim();
        if (!t) continue;
        try { events.push(JSON.parse(t)); } catch { /* tolerate torn line */ }
      }
    } catch (err) {
      console.error('[Store] readDay failed:', err.message);
      return [];
    }
    this._cache.set(dateKey, { mtimeMs, events });
    if (this._cache.size > 40) {
      // drop oldest entries
      const first = this._cache.keys().next().value;
      this._cache.delete(first);
    }
    return events;
  }

  listDays() {
    const out = [];
    try {
      for (const f of fs.readdirSync(this.daysDir)) {
        const m = /^(\d{4}-\d{2}-\d{2})\.jsonl$/.exec(f);
        if (m) out.push(m[1]);
      }
    } catch {}
    return out.sort();
  }

  // ------------------------------------------------------------- derivation
  // Turn the raw event stream into a timeline of "stays" (contiguous time in
  // one app), sessions (groups of stays), and per-app stats.

  derive(events, opts = {}) {
    const idleThresholdMs = opts.idleThresholdMs || 60000;
    const sessionGapMs = opts.sessionGapMs || 600000;
    const stays = [];
    let cur = null;      // {start, end, app, name, title, titles:Set, pid}
    let lastFocus = null;
    let idle = false;
    let lastEventTs = null;
    let switches = 0;
    let firstActive = null;
    let lastActive = null;

    const openStay = (start, data) => {
      const title = data.title || '';
      cur = {
        start, end: start,
        app: data.app || data.name || '?',
        name: data.name || '',
        title,
        titles: new Set(title ? [title] : []),
        pid: data.pid || 0,
      };
    };

    const closeStay = (endTs) => {
      if (cur) {
        cur.end = Math.max(cur.start, endTs);
        if (cur.end - cur.start < 1000) cur.end = cur.start + 1000;
        stays.push(cur);
        cur = null;
      }
    };

    for (const ev of events) {
      const t = ev.ts;
      lastEventTs = t;
      const i = typeof ev.data?.i === 'number' ? ev.data.i : null;
      const isIdleNow = i !== null && i >= 0 && i >= idleThresholdMs;

      if (ev.kind === 'app_focus') {
        lastFocus = ev.data;
        if (isIdleNow) {
          // Focus event while idle (rare): remember it for the next active
          // tick, but do not count the idle period as activity.
          idle = true;
          closeStay(t - Math.max(0, i || 0));
          continue;
        }
        idle = false;
        firstActive = firstActive || t;
        lastActive = t;
        const sameApp = cur && cur.name === (ev.data.name || '') && cur.app === (ev.data.app || '');
        if (cur && !sameApp) {
          closeStay(t);
          switches++;
        }
        if (!cur) {
          openStay(t, ev.data);
        } else {
          cur.end = t;
          if (ev.data.title) cur.title = ev.data.title;
        }
        if (ev.data.title) {
          cur.titles.add(ev.data.title);
          if (cur.titles.size > 60) { // keep bounded
            const arr = [...cur.titles];
            cur.titles = new Set(arr.slice(-60));
          }
        }
      } else if (ev.kind === 'tick') {
        if (isIdleNow) {
          if (!idle) {
            idle = true;
            closeStay(t - Math.max(0, i));
          }
        } else {
          idle = false;
          firstActive = firstActive || t;
          lastActive = t;
          if (!cur && lastFocus) {
            // Returning to the same window after idle does not produce a new
            // foreground event. Resume attribution from the last known app,
            // starting at the actual last-input timestamp reported by Windows.
            openStay(t - Math.max(0, i), lastFocus);
          }
          if (cur) cur.end = t;
        }
      } else if (ev.kind === 'lock' || ev.kind === 'suspend') {
        closeStay(t);
        idle = true;
      } else if (ev.kind === 'unlock' || ev.kind === 'resume') {
        idle = false;
      }
    }

    // An open stay ends at the last observed event. Do not invent active time
    // beyond the evidence in the log; the next tick will extend it.
    if (cur && lastEventTs !== null) closeStay(lastEventTs);

    // Drop stays that ended before they began and clamp unconfirmed tails
    const cleanStays = stays.filter((s) => s.end > s.start);

    // sessions: group stays with gap < sessionGapMs
    const sessions = [];
    for (const s of cleanStays) {
      const last = sessions[sessions.length - 1];
      if (last && s.start - last.end < sessionGapMs) {
        last.end = s.end;
        last.apps.add(s.name || s.app);
        last.activeMs += s.end - s.start;
      } else {
        sessions.push({ start: s.start, end: s.end, activeMs: s.end - s.start, apps: new Set([s.name || s.app]) });
      }
    }

    // per-app stats
    const apps = new Map(); // name -> {name, app, category, ms, switches, titles Map(title->count), lastTitle}
    for (const s of cleanStays) {
      const key = s.name || s.app;
      if (!apps.has(key)) {
        apps.set(key, {
          name: key,
          app: s.app,
          category: opts.categorize ? opts.categorize(s.name, s.app, s.title) : '其他',
          ms: 0,
          switches: 0,
          titles: new Map(),
        });
      }
      const a = apps.get(key);
      a.ms += s.end - s.start;
      for (const t of s.titles) a.titles.set(t, (a.titles.get(t) || 0) + 1);
    }
    // app-level switch counts (A->B or B->A transitions involving this app)
    for (let i = 1; i < cleanStays.length; i++) {
      const prev = cleanStays[i - 1];
      const now = cleanStays[i];
      if ((prev.name || prev.app) !== (now.name || now.app)) {
        const a = apps.get(now.name || now.app);
        if (a) a.switches++;
      }
    }

    // hourly active minutes
    const hourly = new Array(24).fill(0);
    for (const s of cleanStays) {
      let t = s.start;
      const end = s.end;
      while (t < end) {
        const d = new Date(t);
        const nextHour = new Date(d).setMinutes(60, 0, 0);
        const segEnd = Math.min(end, nextHour);
        hourly[d.getHours()] += segEnd - t;
        t = segEnd;
      }
    }

    const totalActiveMs = cleanStays.reduce((acc, s) => acc + (s.end - s.start), 0);

    return {
      stays: cleanStays,
      sessions,
      apps: [...apps.values()],
      hourly,
      switches,
      totalActiveMs,
      firstActive,
      lastActive,
    };
  }

  dayStats(dateKey, opts = {}) {
    const events = this.readDay(dateKey);
    const d = this.derive(events, opts);
    const screenshots = events.filter((e) => e.kind === 'screenshot')
      .filter((e) => {
        try { return fs.existsSync(path.join(this.dataDir, e.data.file)); } catch { return false; }
      })
      .map((e) => ({ ts: e.ts, file: e.data.file, w: e.data.w, h: e.data.h, bytes: e.data.bytes, trigger: e.data.trigger, diff: e.data.diff }));
    const sitesByHost = new Map();
    for (const e of events) {
      if (e.kind !== 'site_visit') continue;
      const site = String(e.data?.site || '').trim().toLowerCase();
      if (!site || site.length > 253 || /[\s/?#]/.test(site)) continue;
      const item = sitesByHost.get(site) || {
        site, visits: 0, firstSeen: e.ts, lastSeen: e.ts, app: e.data?.app || '',
      };
      item.visits++;
      item.lastSeen = e.ts;
      item.app = e.data?.app || item.app;
      sitesByHost.set(site, item);
    }
    const sites = [...sitesByHost.values()].sort((a, b) => b.lastSeen - a.lastSeen);
    const lockEvents = events.filter((e) => e.kind === 'lock' || e.kind === 'suspend').length;
    // idle summary: count long idle gaps (derived from session gaps)
    let longIdles = 0;
    let longestIdleMs = 0;
    for (let i = 1; i < d.sessions.length; i++) {
      const gap = d.sessions[i].start - d.sessions[i - 1].end;
      if (gap >= 5 * 60000) longIdles++;
      if (gap > longestIdleMs) longestIdleMs = gap;
    }
    // longest focus block: consecutive stays on the same app
    let longestBlock = null;
    let blockStart = null;
    let blockApp = null;
    let blockEnd = null;
    for (const s of d.stays) {
      const key = s.name || s.app;
      // A same-app stay separated by an idle gap is a new focus block, not
      // one continuous stretch of concentration.
      if (blockApp === key && s.start - blockEnd <= 60000) {
        blockEnd = s.end;
      } else {
        if (blockApp !== null && (!longestBlock || blockEnd - blockStart > longestBlock.ms)) {
          longestBlock = { app: blockApp, ms: blockEnd - blockStart, startedAt: blockStart };
        }
        blockApp = key; blockStart = s.start; blockEnd = s.end;
      }
    }
    if (blockApp !== null && (!longestBlock || blockEnd - blockStart > longestBlock.ms)) {
      longestBlock = { app: blockApp, ms: blockEnd - blockStart, startedAt: blockStart };
    }
    return {
      dateKey,
      events: events.length,
      ...d,
      screenshots,
      sites,
      lockEvents,
      longIdles,
      longestIdleMs,
      longestBlock,
    };
  }

  // ------------------------------------------------------------ maintenance

  listSnapshotDays() {
    const out = [];
    try {
      for (const f of fs.readdirSync(this.snapshotsDir)) {
        const m = /^(\d{4}-\d{2}-\d{2})$/.exec(f);
        if (m && fs.statSync(path.join(this.snapshotsDir, f)).isDirectory()) out.push(m[1]);
      }
    } catch {}
    return out.sort();
  }

  daySnapshotBytes(dateKey) {
    const dir = this.snapshotDir(dateKey);
    let total = 0;
    try {
      for (const f of fs.readdirSync(dir)) {
        try { total += fs.statSync(path.join(dir, f)).size; } catch {}
      }
    } catch {}
    return total;
  }

  totalSnapshotBytes() {
    return this.listSnapshotDays().reduce((acc, d) => acc + this.daySnapshotBytes(d), 0);
  }

  // Enforce the snapshot byte cap: delete oldest day dirs until under target.
  enforceSnapshotCap(maxBytes) {
    const target = Math.floor(maxBytes * 0.8);
    let total = this.totalSnapshotBytes();
    if (total <= maxBytes) return { total, deleted: [] };
    const deleted = [];
    for (const day of this.listSnapshotDays()) {
      if (total <= target) break;
      const dir = this.snapshotDir(day);
      let dayBytes = 0;
      try {
        for (const f of fs.readdirSync(dir)) {
          try { dayBytes += fs.statSync(path.join(dir, f)).size; } catch {}
        }
        fs.rmSync(dir, { recursive: true, force: true });
      } catch {}
      total -= dayBytes;
      deleted.push(day);
      console.log(`[Store] rotation: removed snapshots for ${day} (${(dayBytes / 1e6).toFixed(1)} MB)`);
    }
    return { total, deleted };
  }

  enforceRetention(retentionDays) {
    const cutoff = Date.now() - retentionDays * 86400000;
    const cutoffKey = localDateKey(cutoff);
    const removed = [];
    for (const day of this.listDays()) {
      if (day < cutoffKey) {
        try {
          fs.unlinkSync(this._dayFile(day));
          this._cache.delete(day);
          removed.push(day);
        } catch {}
      }
    }
    for (const day of this.listSnapshotDays()) {
      if (day < cutoffKey) {
        try { fs.rmSync(this.snapshotDir(day), { recursive: true, force: true }); } catch {}
      }
    }
    return removed;
  }

  purgeAll() {
    try {
      fs.rmSync(this.daysDir, { recursive: true, force: true });
      fs.rmSync(this.snapshotsDir, { recursive: true, force: true });
      fs.mkdirSync(this.daysDir, { recursive: true });
      fs.mkdirSync(this.snapshotsDir, { recursive: true });
      this._cache.clear();
    } catch (err) {
      console.error('[Store] purge failed:', err.message);
    }
  }
}

module.exports = { DayStore, localDateKey, hhmm };
