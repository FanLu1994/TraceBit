'use strict';
// TraceBit main process.
const { app, BrowserWindow, Tray, Menu, Notification, powerMonitor, protocol, net, shell, globalShortcut } = require('electron');
const path = require('path');
const fs = require('fs');

const { Settings } = require('./settings');
const { DayStore, localDateKey } = require('./store');
const { Recorder } = require('./recorder');
const { Analyzer } = require('./analyzer');

const IS_SMOKE = process.argv.includes('--smoke');

// Custom schemes must be registered before the app is ready.
protocol.registerSchemesAsPrivileged([
  { scheme: 'tbimg', privileges: { standard: true, secure: true, supportFetchAPI: false } },
]);

// single instance lock
if (!app.requestSingleInstanceLock()) {
  app.quit();
  return;
}

let mainWindow = null;
let tray = null;
let settings = null;
let store = null;
let recorder = null;
let analyzer = null;
let autoReportTimer = null;

function dataDir() { return path.join(app.getPath('userData'), 'data'); }

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1240,
    height: 840,
    minWidth: 960,
    minHeight: 620,
    backgroundColor: '#101014',
    show: false,
    title: 'TraceBit',
    icon: path.join(__dirname, '..', '..', 'assets', 'tray.png'),
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      spellcheck: false,
    },
  });
  mainWindow.removeMenu();
  mainWindow.webContents.setWindowOpenHandler(({ url }) => {
    if (/^https?:\/\//i.test(url)) shell.openExternal(url);
    return { action: 'deny' };
  });
  mainWindow.webContents.on('will-navigate', (event, url) => {
    if (!url.startsWith('file://')) {
      event.preventDefault();
      if (/^https?:\/\//i.test(url)) shell.openExternal(url);
    }
  });
  mainWindow.loadFile(path.join(__dirname, '..', 'renderer', 'index.html'));
  mainWindow.once('ready-to-show', () => mainWindow.show());
  // Some graphics configurations never emit ready-to-show. A finished local
  // document is sufficient to show the window and must not leave an invisible
  // background process with no obvious way to open it.
  mainWindow.webContents.once('did-finish-load', () => {
    if (mainWindow && !mainWindow.isVisible()) mainWindow.show();
  });
  mainWindow.on('closed', () => { mainWindow = null; });
  mainWindow.on('close', (e) => {
    if (settings.get().app.closeToTray && !app.isQuitting) {
      e.preventDefault();
      mainWindow.hide();
    }
  });
}

function showWindow() {
  if (!mainWindow || mainWindow.isDestroyed()) return createWindow();
  if (mainWindow.isMinimized()) mainWindow.restore();
  mainWindow.show();
  mainWindow.focus();
}

function updateTray() {
  if (!tray) return;
  const rec = settings.get().recording.enabled && recorder?.running;
  tray.setToolTip(rec ? 'TraceBit — 正在记录' : 'TraceBit — 已暂停');
  tray.setContextMenu(Menu.buildFromTemplate([
    { label: rec ? '正在记录' : '已暂停', enabled: false },
    { type: 'separator' },
    { label: rec ? '暂停记录' : '恢复记录', click: () => toggleRecording() },
    { label: '显示主窗口', click: () => showWindow() },
    { label: '生成今日日报', click: () => { showWindow(); mainWindow.webContents.send('goto', 'report'); } },
    { type: 'separator' },
    { label: '退出', click: () => { app.isQuitting = true; app.quit(); } },
  ]));
}

function toggleRecording() {
  const cur = settings.get();
  const isActive = cur.recording.enabled && recorder.running;
  if (isActive) {
    recorder.stop();
    settings.patch({ recording: { enabled: false } });
  } else {
    // Also covers a failed recorder whose setting was left enabled.
    settings.patch({ recording: { enabled: true } });
    recorder.start();
  }
  updateTray();
  broadcastStatus();
}

function broadcastStatus() {
  if (!mainWindow) return;
  mainWindow.webContents.send('status', {
    ...recorder.status(),
    enabled: settings.get().recording.enabled,
    dataDir: dataDir(),
    totalSnapshotBytes: store.totalSnapshotBytes(),
  });
}

// ------------------------------------------------------------- day payload

function dayPayload(dateKey) {
  const s = settings.get();
  const stats = store.dayStats(dateKey, {
    idleThresholdMs: s.recording.idleThresholdMs,
    categorize: require('./categories').categorize,
  });
  const stays = stats.stays.map((st) => ({
    start: st.start, end: st.end, app: st.app, name: st.name, title: st.title,
    category: require('./categories').categorize(st.name, st.app, st.title),
  }));
  return {
    dateKey,
    exists: stats.events > 0,
    events: stats.events,
    stays,
    totalActiveMs: stats.totalActiveMs,
    firstActive: stats.firstActive,
    lastActive: stats.lastActive,
    sessions: stats.sessions.map((x) => ({ start: x.start, end: x.end, apps: [...x.apps] })),
    apps: stats.apps.map((a) => ({
      name: a.name, app: a.app, ms: a.ms, switches: a.switches,
      category: a.category,
      titles: [...a.titles.entries()].sort((x, y) => y[1] - x[1]).slice(0, 6).map(([t, c]) => ({ t, c })),
    })).sort((a, b) => b.ms - a.ms),
    hourly: stats.hourly,
    switches: stats.switches,
    longestBlock: stats.longestBlock,
    meetingsMinutes: 0,
    longIdles: stats.longIdles,
    longestIdleMs: stats.longestIdleMs,
    screenshots: stats.screenshots,
    snapshotBytes: store.daySnapshotBytes(dateKey),
    lockEvents: stats.lockEvents,
  };
}

// ------------------------------------------------------------ auto report

function scheduleAutoReport() {
  if (autoReportTimer) { clearTimeout(autoReportTimer); autoReportTimer = null; }
  const ai = settings.get().ai;
  if (!ai.autoReport || !ai.baseUrl || !ai.model) return;
  const [h, m] = String(ai.autoReportTime || '21:30').split(':').map((x) => parseInt(x, 10) || 0);
  const now = new Date();
  const target = new Date(now);
  target.setHours(h, m, 0, 0);
  if (target <= now) target.setDate(target.getDate() + 1);
  const delay = target.getTime() - now.getTime();
  console.log(`[Main] auto report scheduled at ${target.toLocaleString()}`);
  autoReportTimer = setTimeout(async () => {
    autoReportTimer = null;
    try {
      const dk = localDateKey(Date.now());
      const report = await analyzer.generateReport(dk);
      if (Notification.isSupported()) {
        new Notification({ title: 'TraceBit 日报已生成', body: `${dk} 的行为日报已就绪` }).show();
      }
      if (mainWindow) mainWindow.webContents.send('report-generated', { dateKey: dk });
    } catch (err) {
      console.error('[Main] auto report failed:', err.message);
    }
    scheduleAutoReport();
  }, delay);
}

// ------------------------------------------------------------------ boot

app.whenReady().then(async () => {
  settings = new Settings(app.getPath('userData'));
  store = new DayStore(dataDir());
  recorder = new Recorder({ settings, store, electron: { desktopCapturer: require('electron').desktopCapturer, screen: require('electron').screen, powerMonitor } });
  analyzer = new Analyzer({ settings, store, reportsDir: path.join(app.getPath('userData'), 'reports') });

  // image protocol: tbimg://p/<encoded relative path>
  try {
    protocol.handle('tbimg', (request) => {
      const url = new URL(request.url);
      const rel = decodeURIComponent(url.hostname === 'p' ? url.pathname.slice(1) : url.hostname + url.pathname);
      const abs = path.resolve(dataDir(), rel);
      const relative = path.relative(dataDir(), abs);
      if (relative.startsWith('..') || path.isAbsolute(relative)) return new Response('forbidden', { status: 403 });
      return net.fetch('file:///' + abs.replace(/\\/g, '/'));
    });
  } catch (err) {
    console.error('[Main] protocol failed:', err.message);
  }

  if (IS_SMOKE) {
    const code = await runSmokeTests();
    app.exit(code);
    return;
  }

  if (settings.get().app.autostart) app.setLoginItemSettings({ openAtLogin: true });

  createWindow();

  tray = new Tray(path.join(__dirname, '..', '..', 'assets', 'tray.png'));
  updateTray();
  tray.on('click', () => showWindow());

  if (settings.get().recording.enabled) recorder.start();
  recorder.onStatus(broadcastStatus);
  scheduleAutoReport();

  app.on('second-instance', () => showWindow());
}).catch((err) => {
  console.error('[Main] startup failed:', err);
  require('electron').dialog.showErrorBox('TraceBit 启动失败', err.stack || err.message || String(err));
  app.exit(1);
});

app.on('before-quit', () => {
  app.isQuitting = true;
  recorder?.stop();
});

app.on('window-all-closed', () => {
  // keep running in tray
});

// ------------------------------------------------------------------- IPC

const { ipcMain, dialog } = require('electron');

ipcMain.handle('get-state', () => {
  return {
    settings: settings.get(),
    status: {
      ...recorder.status(),
      enabled: settings.get().recording.enabled,
      dataDir: dataDir(),
      totalSnapshotBytes: store.totalSnapshotBytes(),
    },
    today: localDateKey(Date.now()),
  };
});

ipcMain.handle('save-settings', (_e, partial) => {
  const prev = settings.get();
  const prevRecording = { ...prev.recording };
  const next = settings.patch(partial);
  // Apply the enable switch, then restart when capture/timing options change.
  if (partial.recording?.enabled !== undefined) {
    if (partial.recording.enabled) recorder.start(); else recorder.stop();
  }
  if (partial.recording && recorder.running) {
    const recordingChanged = Object.keys(partial.recording).some((key) =>
      key !== 'enabled' && partial.recording[key] !== prevRecording[key]);
    if (recordingChanged) recorder.restart();
  }
  if (partial.app?.autostart !== undefined) {
    app.setLoginItemSettings({ openAtLogin: !!partial.app.autostart });
  }
  if (partial.ai) scheduleAutoReport();
  updateTray();
  broadcastStatus();
  return next;
});

ipcMain.handle('get-day', (_e, dateKey) => dayPayload(dateKey));
ipcMain.handle('get-report', (_e, dateKey) => analyzer.getReport(dateKey));
ipcMain.handle('get-report-list', () => analyzer.listReports());
ipcMain.handle('generate-report', async (_e, dateKey) => {
  try {
    const report = await analyzer.generateReport(dateKey);
    return { ok: true, report };
  } catch (err) {
    return { ok: false, error: err.message };
  }
});
ipcMain.handle('test-ai', async () => {
  try {
    const r = await analyzer.testConnection();
    return { ok: true, ...r };
  } catch (err) {
    return { ok: false, error: err.message };
  }
});
ipcMain.handle('toggle-recording', () => {
  toggleRecording();
  return {
    ...recorder.status(),
    enabled: settings.get().recording.enabled,
    dataDir: dataDir(),
    totalSnapshotBytes: store.totalSnapshotBytes(),
  };
});
ipcMain.handle('open-data-dir', () => { shell.openPath(dataDir()); return { ok: true }; });
ipcMain.handle('open-snapshot', (_e, rel) => {
  const abs = path.resolve(dataDir(), rel);
  const relative = path.relative(dataDir(), abs);
  if (relative.startsWith('..') || path.isAbsolute(relative)) return { ok: false };
  shell.openPath(abs);
  return { ok: true };
});
ipcMain.handle('purge-data', (_e, scope) => {
  if (scope === 'screenshots') {
    fs.rmSync(store.snapshotsDir, { recursive: true, force: true });
    fs.mkdirSync(store.snapshotsDir, { recursive: true });
  } else {
    store.purgeAll();
  }
  return { ok: true };
});

// ------------------------------------------------------------ smoke tests

async function runSmokeTests() {
  const failures = [];
  const t = (name, fn) => Promise.resolve().then(fn)
    .then(() => console.log('  ✓', name))
    .catch((e) => { failures.push(name); console.error('  ✗', name, e.message); });

  console.log('[Smoke] TraceBit self-test');

  await t('store append/derive', () => {
    const smokeDir = path.join(app.getPath('userData'), 'smoke-data');
    fs.rmSync(smokeDir, { recursive: true, force: true });
    const tmp = new DayStore(smokeDir);
    const base = Date.now();
    // simulate: app A 10:00-10:05, tick idle, app B 10:10-10:15
    tmp.append({ ts: base, kind: 'app_focus', data: { app: 'Code', name: 'code', title: 'a.js', pid: 1, i: 1000 } });
    tmp.append({ ts: base + 60000, kind: 'app_focus', data: { app: 'Code', name: 'code', title: 'b.js', pid: 1, i: 2000 } });
    tmp.append({ ts: base + 120000, kind: 'app_focus', data: { app: 'Chrome', name: 'chrome', title: 'x.com', pid: 2, i: 1000 } });
    fs.mkdirSync(path.join(smokeDir, 'snapshots', 'x'), { recursive: true });
    fs.writeFileSync(path.join(smokeDir, 'snapshots', 'x', 'y.jpg'), 'test');
    tmp.append({ ts: base + 150000, kind: 'screenshot', data: { file: 'snapshots/x/y.jpg', w: 1600, h: 900, bytes: 100, trigger: 'test', diff: null } });
    tmp.append({ ts: base + 180000, kind: 'tick', data: { i: 5000 } });
    tmp.append({ ts: base + 240000, kind: 'tick', data: { i: 65000 } });
    // Same window becomes active again: no new app_focus event is emitted.
    tmp.append({ ts: base + 300000, kind: 'tick', data: { i: 1000 } });
    const stats = tmp.dayStats(localDateKey(base), { idleThresholdMs: 60000 });
    if (stats.totalActiveMs !== 176000) throw new Error(`activeMs=${stats.totalActiveMs}`);
    if (stats.apps.length !== 2) throw new Error(`apps=${stats.apps.length}`);
    if (stats.switches !== 1) throw new Error(`switches=${stats.switches}`);
    if (stats.screenshots.length !== 1) throw new Error('screenshots');
    fs.rmSync(smokeDir, { recursive: true, force: true });
  });

  await t('analyzer skeleton + prompt', () => {
    const { buildSkeleton, skeletonMarkdown } = require('./analyzer');
    const fake = {
      dateKey: '2026-01-01', apps: [{ name: 'code', app: 'VS Code', titles: new Map([['x.js - p', 3]]) }],
      stays: [{ start: 0, end: 300000, app: 'VS Code', name: 'code', title: 'x.js', titles: new Set(['x.js']) }],
      sessions: [{ start: 0, end: 300000, apps: new Set(['code']), activeMs: 300000 }],
      hourly: new Array(24).fill(0), switches: 5, totalActiveMs: 300000,
      firstActive: 0, lastActive: 300000, screenshots: [{ ts: 0, file: 'f' }],
      longIdles: 1, longestIdleMs: 900000,
      longestBlock: { app: 'code', ms: 300000, startedAt: 0 },
    };
    const skel = buildSkeleton(fake, settings.get());
    const md = skeletonMarkdown(skel);
    if (!md.includes('VS Code')) throw new Error('missing app');
    if (!skel.categories.length) throw new Error('no categories');
  });

  await t('AI report end-to-end with local mock', async () => {
    const http = require('http');
    const aiDataDir = path.join(app.getPath('userData'), 'smoke-ai-data');
    const aiReportsDir = path.join(app.getPath('userData'), 'smoke-ai-reports');
    fs.rmSync(aiDataDir, { recursive: true, force: true });
    fs.rmSync(aiReportsDir, { recursive: true, force: true });
    const day = new DayStore(aiDataDir);
    const noon = new Date().setHours(12, 0, 0, 0);
    const dateKey = localDateKey(noon);
    day.append({ ts: noon, kind: 'app_focus', data: { app: 'VS Code', name: 'code', title: 'secret@example.com - project', pid: 1, i: 1000 } });
    day.append({ ts: noon + 120000, kind: 'tick', data: { i: 1000 } });

    let requestBody = '';
    const server = http.createServer((req, res) => {
      let body = '';
      req.on('data', (chunk) => { body += chunk; });
      req.on('end', () => {
        requestBody = body;
        res.writeHead(200, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({
          model: 'mock-model',
          choices: [{ message: { content: `# ${dateKey} 行为日报\n\n## 一、今日概览\n测试。\n\n## 四、省力优化建议\n测试建议。` } }],
        }));
      });
    });
    await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
    const previousAI = { ...settings.get().ai };
    try {
      const port = server.address().port;
      settings.data.ai = { baseUrl: `http://127.0.0.1:${port}/v1`, apiKey: 'mock-key', model: 'mock-model', autoReport: false, autoReportTime: '21:30' };
      const mockAnalyzer = new Analyzer({ settings, store: day, reportsDir: aiReportsDir });
      const report = await mockAnalyzer.generateReport(dateKey);
      if (!report.markdown.includes('省力优化建议')) throw new Error('report markdown missing');
      if (!requestBody.includes('mock-model')) throw new Error('model not sent');
      if (requestBody.includes('secret@example.com')) throw new Error('email was not redacted');
      if (!requestBody.includes('[邮箱]')) throw new Error('redaction marker missing');
      if (!mockAnalyzer.getReport(dateKey)) throw new Error('report not persisted');
    } finally {
      settings.data.ai = previousAI;
      await new Promise((resolve) => server.close(resolve));
      fs.rmSync(aiDataDir, { recursive: true, force: true });
      fs.rmSync(aiReportsDir, { recursive: true, force: true });
    }
  });

  await t('poller ps1 exists outside asar', () => {
    const p = path.join(__dirname, 'ps', 'poller.ps1')
      .replace(`app.asar${path.sep}`, `app.asar.unpacked${path.sep}`);
    if (!fs.existsSync(p)) throw new Error(`missing: ${p}`);
    if (app.isPackaged && !p.includes('app.asar.unpacked')) throw new Error('packaged poller is still inside asar');
    const s = fs.readFileSync(p, 'utf-8');
    if (!s.includes('GetForegroundWindow')) throw new Error('bad script');
  });

  console.log(failures.length ? `[Smoke] FAILED: ${failures.join(', ')}` : '[Smoke] all passed');
  return failures.length ? 1 : 0;
}
