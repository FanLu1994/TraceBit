'use strict';
// TraceBit Recorder — a port of the activity-recording concepts used by
// desktop assistants (sessions/events/snapshots), re-implemented standalone:
//   - persistent PowerShell poller for foreground app/title + system idle
//   - screenshots on app-switch (debounced) + heartbeat, deduped via
//     thumbnail hash + luma histogram + pixel diff (same approach family as
//     upstream: hash fast-path, hist distance ~0.05, pixel diff ~2% @tol 30)
//   - paused while locked/suspended, sensitive apps never captured
//   - storage rotation under a byte cap
const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const readline = require('readline');

const THUMB_W = 160;
const THUMB_H = 90;
const HIST_BINS = 64;
const HIST_CHANGE_THRESHOLD = 0.05;
const PIXEL_DIFF_THRESHOLD = 0.02;
const PIXEL_TOLERANCE = 30;

class Recorder {
  constructor({ settings, store, electron }) {
    this.settings = settings;
    this.store = store;
    this.electron = electron; // { desktopCapturer, screen, powerMonitor, app }
    this.running = false;
    this.locked = false;
    this.suspended = false;
    this.idle = false;
    this.frontmost = null; // {app, name, title, pid}
    this.poller = null;
    this.pollerFailures = 0;
    this.lastError = null;
    this.rl = null;
    this.heartbeatTimer = null;
    this.rotationTimer = null;
    this.pendingCaptureTimer = null;
    this.captureInFlight = false;
    this.prevGray = null;   // Buffer of THUMB_W*THUMB_H luma bytes
    this.prevHist = null;   // Float32Array(HIST_BINS)
    this.prevHash = null;
    this.statusListeners = new Set();
  }

  cfg() { return this.settings.get().recording; }
  privacyCfg() { return this.settings.get().privacy; }
  storageCfg() { return this.settings.get().storage; }

  onStatus(fn) { this.statusListeners.add(fn); }
  emitStatus() {
    const s = this.status();
    for (const fn of this.statusListeners) { try { fn(s); } catch {} }
  }
  status() {
    return {
      running: this.running,
      locked: this.locked,
      idle: this.idle,
      frontmost: this.frontmost,
      pollerAlive: !!this.poller && !this.poller.killed,
      lastError: this.lastError,
    };
  }

  start() {
    if (this.running) return;
    const cfg = this.cfg();
    if (!cfg.enabled) return;
    this.running = true;
    this.pollerFailures = 0;
    this.lastError = null;
    this.startPoller();
    this.installPowerHooks();
    // heartbeat screenshots
    this.heartbeatTimer = setInterval(() => {
      if (!this.running || this.locked || this.suspended || this.idle) return;
      if (!cfg.screenshots) return;
      this.capture('heartbeat').catch((e) => console.error('[Recorder] heartbeat:', e.message));
    }, Math.max(60000, cfg.heartbeatMs));
    // storage rotation check every 30 min
    this.rotationTimer = setInterval(() => this.rotate(), 30 * 60000);
    this.rotate();
    console.log('[Recorder] started');
    this.emitStatus();
  }

  stop() {
    if (!this.running) return;
    this.running = false;
    this.stopPoller();
    this.removePowerHooks();
    if (this.heartbeatTimer) { clearInterval(this.heartbeatTimer); this.heartbeatTimer = null; }
    if (this.rotationTimer) { clearInterval(this.rotationTimer); this.rotationTimer = null; }
    if (this.pendingCaptureTimer) { clearTimeout(this.pendingCaptureTimer); this.pendingCaptureTimer = null; }
    this.prevGray = this.prevHist = this.prevHash = null;
    console.log('[Recorder] stopped');
    this.emitStatus();
  }

  restart() { this.stop(); this.start(); }

  // ------------------------------------------------------------ power hooks
  installPowerHooks() {
    const { powerMonitor } = this.electron;
    this._pm = [
      ['lock-screen', () => this.onLock()],
      ['unlock-screen', () => this.onUnlock()],
      ['suspend', () => this.onSuspend()],
      ['resume', () => this.onResume()],
    ];
    for (const [ev, fn] of this._pm) powerMonitor.on(ev, fn);
  }
  removePowerHooks() {
    if (!this._pm) return;
    const { powerMonitor } = this.electron;
    for (const [ev, fn] of this._pm) { try { powerMonitor.removeListener(ev, fn); } catch {} }
    this._pm = null;
  }
  onLock() {
    if (!this.running) return;
    this.locked = true;
    this.clearDedup();
    this.store.append({ kind: 'lock' });
    this.emitStatus();
  }
  onUnlock() {
    if (!this.running) return;
    this.locked = false;
    this.clearDedup();
    this.store.append({ kind: 'unlock' });
    this.emitStatus();
  }
  onSuspend() {
    if (!this.running) return;
    this.suspended = true;
    this.clearDedup();
    this.store.append({ kind: 'suspend' });
  }
  onResume() {
    if (!this.running) return;
    this.suspended = false;
    this.clearDedup();
    this.store.append({ kind: 'resume' });
  }
  clearDedup() { this.prevGray = this.prevHist = this.prevHash = null; }

  // ---------------------------------------------------------------- poller
  startPoller() {
    const cfg = this.cfg();
    // PowerShell is an external process and cannot read scripts from inside
    // app.asar. electron-builder unpacks this directory for packaged builds.
    const script = path.join(__dirname, 'ps', 'poller.ps1')
      .replace(`app.asar${path.sep}`, `app.asar.unpacked${path.sep}`);
    const args = [
      '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
      '-File', script,
      '-IntervalMs', String(cfg.pollMs),
      '-TickMs', String(cfg.tickMs),
      '-TitleThrottleMs', String(cfg.titleThrottleMs),
    ];
    const child = spawn('powershell.exe', args, { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] });
    this.poller = child;
    this.rl = readline.createInterface({ input: child.stdout });
    this.rl.on('line', (line) => this.onPollerLine(line));
    child.stderr.on('data', (d) => {
      const s = String(d).trim();
      if (s) console.error('[Recorder] poller stderr:', s.slice(0, 300));
    });
    child.on('error', (err) => {
      this.lastError = `无法启动轮询进程：${err.message}`;
      console.error('[Recorder] poller spawn error:', err.message);
      this.emitStatus();
    });
    child.on('exit', (code) => {
      this.poller = null;
      if (!this.running) return;
      this.pollerFailures++;
      this.lastError = `轮询进程退出（代码 ${code ?? '未知'}）`;
      this.emitStatus();
      const delay = Math.min(30000, 1000 * Math.pow(2, Math.min(this.pollerFailures, 5)));
      console.warn(`[Recorder] poller exited (${code}), restarting in ${delay}ms`);
      if (this.pollerFailures > 20) {
        console.error('[Recorder] poller failed repeatedly, stopping');
        this.lastError = '轮询进程多次启动失败，请点击“开始记录”重试';
        this.stop();
        return;
      }
      setTimeout(() => { if (this.running && !this.poller) this.startPoller(); }, delay);
    });
  }
  stopPoller() {
    if (this.rl) { try { this.rl.close(); } catch {} this.rl = null; }
    if (this.poller) { try { this.poller.kill(); } catch {} this.poller = null; }
  }

  onPollerLine(line) {
    // The poller stays alive across lock/suspend so it can resume cleanly,
    // but lock-screen apps and titles must never enter the activity log.
    if (this.locked || this.suspended) return;
    const t = line.trim();
    if (!t || !t.startsWith('{')) return;
    let ev;
    try { ev = JSON.parse(t); } catch { return; }
    if (typeof ev.t !== 'number') return;
    this.pollerFailures = 0;
    this.lastError = null;
    const idleMs = typeof ev.i === 'number' ? ev.i : -1;
    const idleThreshold = this.cfg().idleThresholdMs;
    const wasIdle = this.idle;

    if (ev.k === 'f') {
      const fm = { app: ev.app || ev.name || '?', name: ev.name || '', title: ev.title || '', pid: ev.pid || 0 };
      const appChanged = !this.frontmost || this.frontmost.name !== fm.name || this.frontmost.app !== fm.app;
      this.frontmost = fm;
      this.store.append({
        kind: 'app_focus',
        data: { app: fm.app, name: fm.name, title: fm.title, pid: fm.pid, i: idleMs },
      });
      if (idleMs >= 0 && idleMs >= idleThreshold) {
        this.idle = true;
      } else {
        this.idle = false;
      }
      // schedule a screenshot on app change (debounced)
      if (appChanged && !this.idle && !this.locked && !this.suspended && this.cfg().screenshots) {
        this.scheduleFocusCapture();
      }
    } else if (ev.k === 't') {
      const nowIdle = idleMs >= 0 && idleMs >= idleThreshold;
      if (nowIdle !== this.idle) {
        this.idle = nowIdle;
        this.store.append({ kind: 'tick', data: { i: idleMs } });
        if (!nowIdle && this.cfg().screenshots && !this.locked && !this.suspended) {
          // returning from idle — grab a frame to resume the visual trail
          this.capture('resume').catch(() => {});
        }
      } else if (!nowIdle) {
        // active tick — keep for attribution
        this.store.append({ kind: 'tick', data: { i: idleMs } });
      }
      // idle ticks are not stored (no information)
    }
    if (wasIdle !== this.idle) this.emitStatus();
    else this.emitStatus(); // frontmost may have changed
  }

  scheduleFocusCapture() {
    const debounce = this.cfg().focusDebounceMs;
    if (this.pendingCaptureTimer) clearTimeout(this.pendingCaptureTimer);
    this.pendingCaptureTimer = setTimeout(() => {
      this.pendingCaptureTimer = null;
      this.capture('app_focus').catch(() => {});
    }, debounce);
  }

  // ------------------------------------------------------------- sensitive
  isSensitive() {
    const fm = this.frontmost;
    if (!fm) return false;
    const patterns = this.privacyCfg().sensitivePatterns || [];
    const hay = `${fm.app || ''}\n${fm.title || ''}`.toLowerCase();
    return patterns.some((p) => {
      const s = String(p).toLowerCase().trim();
      return s && hay.includes(s);
    });
  }

  // ------------------------------------------------------------- capturing
  async capture(trigger) {
    if (!this.running || !this.cfg().screenshots) return null;
    if (this.locked || this.suspended || this.captureInFlight) return null;
    if (this.isSensitive()) return null;
    this.captureInFlight = true;
    try {
      const { desktopCapturer, screen } = this.electron;
      const maxW = this.storageCfg().maxSnapWidth || 1600;
      const primary = screen.getPrimaryDisplay();
      const sources = await desktopCapturer.getSources({
        types: ['screen'],
        thumbnailSize: { width: maxW, height: Math.round(maxW * 0.6) },
      });
      if (!sources.length) return null;
      const src = sources.find((s) => String(s.display_id) === String(primary.id)) || sources[0];
      const img = src.thumbnail;
      if (!img || img.isEmpty()) return null;

      // dedup on a tiny grayscale thumbnail
      const small = img.resize({ width: THUMB_W, height: THUMB_H, quality: 'low' });
      const bmp = small.toBitmap(); // BGRA
      if (!bmp || bmp.length < THUMB_W * THUMB_H * 4) return null;
      const gray = Buffer.alloc(THUMB_W * THUMB_H);
      for (let p = 0; p < THUMB_W * THUMB_H; p++) {
        const b = bmp[p * 4], g = bmp[p * 4 + 1], r = bmp[p * 4 + 2];
        gray[p] = (r * 77 + g * 150 + b * 29) >> 8;
      }
      const hash = crypto.createHash('sha1').update(gray).digest('hex');
      let diffPct = null;
      if (this.prevGray) {
        if (this.prevHash === hash) {
          return null; // identical frame
        }
        const hist = histogram(gray);
        const histDist = histDistance(hist, this.prevHist);
        if (histDist < HIST_CHANGE_THRESHOLD) {
          const pct = pixelDiff(gray, this.prevGray, PIXEL_TOLERANCE);
          diffPct = pct;
          if (pct < PIXEL_DIFF_THRESHOLD) return null; // effectively unchanged
        } else {
          diffPct = null; // clearly different
        }
      }

      // persist
      const q = this.storageCfg().jpegQuality || 72;
      const jpeg = img.toJPEG(q);
      const dateKey = localDateKey(Date.now());
      const dir = this.store.snapshotDir(dateKey);
      fs.mkdirSync(dir, { recursive: true });
      const d = new Date();
      const p2 = (n) => String(n).padStart(2, '0');
      const name = `${p2(d.getHours())}${p2(d.getMinutes())}${p2(d.getSeconds())}-${crypto.randomBytes(3).toString('hex')}.jpg`;
      const file = path.join(dir, name);
      fs.writeFileSync(file, jpeg);
      const size = img.getSize();
      const rel = path.join('snapshots', dateKey, name);
      this.store.append({
        kind: 'screenshot',
        data: {
          file: rel, w: size.width, h: size.height, bytes: jpeg.length,
          trigger, diff: diffPct, app: this.frontmost?.app || null, title: this.frontmost?.title || null,
        },
      });
      // update dedup state
      this.prevGray = gray;
      this.prevHash = hash;
      this.prevHist = histogram(gray);
      return rel;
    } catch (err) {
      console.error('[Recorder] capture failed:', err.message);
      return null;
    } finally {
      this.captureInFlight = false;
    }
  }

  // -------------------------------------------------------------- rotation
  rotate() {
    try {
      const st = this.storageCfg();
      const res = this.store.enforceSnapshotCap(st.maxSnapshotBytes);
      this.store.enforceRetention(st.retentionDays);
      if (res.deleted.length) console.log(`[Recorder] rotation: now ${(
        (res.total || this.store.totalSnapshotBytes()) / 1e6).toFixed(1)} MB`);
    } catch (err) {
      console.error('[Recorder] rotate failed:', err.message);
    }
  }
}

function histogram(gray) {
  const hist = new Float32Array(HIST_BINS);
  for (let i = 0; i < gray.length; i++) hist[gray[i] >> 2]++;
  const total = gray.length || 1;
  for (let i = 0; i < HIST_BINS; i++) hist[i] /= total;
  return hist;
}

function histDistance(a, b) {
  if (!a || !b || a.length !== b.length) return 1;
  let d = 0;
  for (let i = 0; i < a.length; i++) d += Math.abs(a[i] - b[i]);
  return d / 2; // total mass difference
}

function pixelDiff(a, b, tol) {
  if (!a || !b || a.length !== b.length) return 1;
  let changed = 0;
  const n = a.length;
  for (let i = 0; i < n; i++) {
    if (Math.abs(a[i] - b[i]) > tol) changed++;
  }
  return changed / n;
}

const { localDateKey } = require('./store');

module.exports = { Recorder };
