'use strict';
/* TraceBit renderer: task-first UI, no frameworks. */

const $ = (selector, root = document) => root.querySelector(selector);
const $$ = (selector, root = document) => [...root.querySelectorAll(selector)];

const state = {
  view: 'today',
  dateKey: localDateKey(Date.now()),
  today: null,
  settings: null,
  status: null,
  day: null,
  report: null,
  reportList: [],
  generating: false,
  toggling: false,
  reportError: '',
  settingsDirty: false,
  lightboxShots: [],
  lightboxIndex: 0,
  lastFocused: null,
};

const CAT_COLORS = {
  '开发': '#4277a8', '浏览器': '#3f897d', '即时通讯': '#b37b24', '办公文档': '#8a6aa8',
  '会议': '#b95b5b', '设计': '#b86e52', '娱乐': '#64864f', '工具': '#7c8285',
  '系统': '#8c8c86', '其他': '#92928b',
};
const TRIGGER_LABELS = { app_focus: '切换', heartbeat: '定时', resume: '返回', manual: '手动' };

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
function fmtMs(ms) {
  if (ms == null) return '暂无';
  const minutes = Math.round(ms / 60000);
  if (minutes < 60) return `${minutes}分钟`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest ? `${hours}小时${rest}分` : `${hours}小时`;
}
function fmtBytes(bytes) {
  if (!bytes) return '0 MB';
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
}
function esc(value) {
  return String(value ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}
function catColor(category) { return CAT_COLORS[category] || CAT_COLORS['其他']; }
function fullDate(dateKey) {
  const date = new Date(`${dateKey}T12:00:00`);
  return new Intl.DateTimeFormat('zh-CN', { month: 'long', day: 'numeric', weekday: 'long' }).format(date);
}
function hasAI() {
  const ai = state.settings?.ai;
  return Boolean(ai?.baseUrl && ai?.model);
}
function isRecording() {
  return Boolean(state.status?.enabled && state.status?.running);
}
function reportExcerpt(markdown) {
  return String(markdown || '')
    .replace(/[#*>`\[\]()_-]/g, '')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, 116);
}

function toast(message, kind = '', duration = 3600) {
  const el = document.createElement('div');
  el.className = `toast ${kind}`;
  el.textContent = message;
  $('#toasts').appendChild(el);
  setTimeout(() => el.remove(), duration);
}

/* Markdown is always escaped before adding simple formatting. */
function renderMarkdown(source) {
  const lines = String(source || '').split('\n');
  const out = [];
  let list = null;
  let table = null;
  const inline = (text) => esc(text)
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
    .replace(/(^|[^*])\*([^*\n]+)\*/g, '$1<em>$2</em>')
    .replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noreferrer">$1</a>');
  const closeList = () => { if (list) { out.push(`</${list}>`); list = null; } };
  const closeTable = () => { if (table) { out.push('</tbody></table>'); table = null; } };

  for (const raw of lines) {
    const line = raw.replace(/\s+$/, '');
    if (!line.trim()) { closeList(); closeTable(); continue; }
    if (/^\|/.test(line)) {
      const cells = line.split('|').slice(1, -1).map((cell) => cell.trim());
      if (/^[\s|:-]+$/.test(line)) continue;
      if (!table) {
        closeList();
        table = true;
        out.push(`<table><thead><tr>${cells.map((cell) => `<th>${inline(cell)}</th>`).join('')}</tr></thead><tbody>`);
      } else {
        out.push(`<tr>${cells.map((cell) => `<td>${inline(cell)}</td>`).join('')}</tr>`);
      }
      continue;
    }
    closeTable();
    const heading = /^(#{1,4})\s+(.*)$/.exec(line);
    if (heading) {
      closeList();
      out.push(`<h${heading[1].length}>${inline(heading[2])}</h${heading[1].length}>`);
      continue;
    }
    if (/^(---+|\*\*\*)$/.test(line.trim())) { closeList(); out.push('<hr>'); continue; }
    if (/^>\s?/.test(line)) { closeList(); out.push(`<blockquote>${inline(line.replace(/^>\s?/, ''))}</blockquote>`); continue; }
    const unordered = /^[-*•]\s+(.*)$/.exec(line);
    const ordered = /^(\d+)[.、]\s+(.*)$/.exec(line);
    if (unordered) {
      if (list !== 'ul') { closeList(); list = 'ul'; out.push('<ul>'); }
      out.push(`<li>${inline(unordered[1])}</li>`);
      continue;
    }
    if (ordered) {
      if (list !== 'ol') { closeList(); list = 'ol'; out.push('<ol>'); }
      out.push(`<li>${inline(ordered[2])}</li>`);
      continue;
    }
    closeList();
    out.push(`<p>${inline(line)}</p>`);
  }
  closeList();
  closeTable();
  return out.join('\n');
}

/* Navigation and global recording control */
function setView(view) {
  state.view = view;
  $$('#nav .nav-item').forEach((button) => button.classList.toggle('active', button.dataset.view === view));
  render();
}

function recordMode() {
  const status = state.status;
  if (!status) return 'paused';
  if (status.enabled && status.running && status.lastError && !status.pollerAlive) return 'error';
  if (!status.enabled || !status.running) return 'paused';
  if (status.locked) return 'locked';
  if (status.idle) return 'idle';
  return 'active';
}

function updateRecUI() {
  if (!state.status || !state.settings) return;
  state.settings.recording.enabled = state.status.enabled;
  const mode = recordMode();
  const status = state.status;
  const active = isRecording();
  const content = {
    active: ['正在记录', status.frontmost?.app ? `当前：${status.frontmost.app}` : '等待前台应用活动'],
    idle: ['记录中 · 空闲', '无键鼠输入，空闲时间不计入活动'],
    locked: ['锁屏，已暂停采集', '解锁后自动继续，锁屏内容不会记录'],
    paused: ['记录已暂停', '点击开始记录后，才会采集新的活动'],
    error: ['记录服务异常', status.lastError || '采集服务暂时不可用'],
  }[mode];

  const sidebarTitles = { idle: '空闲中', locked: '锁屏暂停' };
  $('#rec-status-title').textContent = sidebarTitles[mode] || content[0];
  $('#rec-status-detail').textContent = content[1];
  $('#rec-dot').className = `status-dot ${mode === 'active' ? 'is-active' : mode === 'paused' ? 'is-paused' : mode === 'error' ? 'is-error' : 'is-idle'}`;
  const error = $('#rec-error');
  error.hidden = !status.lastError;
  error.textContent = status.lastError || '';

  const button = $('#rec-toggle');
  button.textContent = state.toggling ? (active ? '正在暂停…' : '正在开启…') : (active ? '暂停记录' : (status.lastError ? '重试开启' : '开始记录'));
  button.disabled = state.toggling;
  button.className = `button record-button ${active ? 'secondary' : 'primary'}`;

  const headerDot = $('#current-record-dot');
  if (headerDot) headerDot.className = $('#rec-dot').className;
  const headerTitle = $('#current-record-title');
  if (headerTitle) headerTitle.textContent = content[0];
  const headerDetail = $('#current-record-detail');
  if (headerDetail) headerDetail.textContent = content[1];
  const settingToggle = $('#rec-enabled');
  if (settingToggle && !state.settingsDirty) {
    settingToggle.checked = Boolean(status.enabled);
    const label = $('.switch-label', settingToggle.closest('.setting-control'));
    if (label) label.textContent = status.enabled ? '已开启' : '已暂停';
  }
}

async function toggleRecording() {
  if (state.toggling) return;
  state.toggling = true;
  updateRecUI();
  try {
    const next = await window.tracebit.toggleRecording();
    state.status = next;
    state.settings.recording.enabled = next.enabled;
    updateRecUI();
    toast(isRecording() ? '记录已开启' : '记录已暂停', 'success');
    if (state.view === 'today' && isRecording()) {
      setTimeout(() => loadDay(), 2500);
    }
  } catch (error) {
    toast(`操作失败：${error.message}`, 'error', 6000);
  } finally {
    state.toggling = false;
    updateRecUI();
  }
}

/* Shared page pieces */
function pageHead(title, subtitle, kicker = '', withDate = true) {
  return `
    <header class="page-head">
      <div class="page-heading">
        ${kicker ? `<div class="page-kicker">${esc(kicker)}</div>` : ''}
        <h1 class="page-title">${esc(title)}</h1>
        <p class="page-subtitle">${esc(subtitle)}</p>
      </div>
      <div class="page-actions">${withDate ? dateNavHTML() : ''}</div>
    </header>`;
}

function dateNavHTML() {
  return `
    <div class="date-nav">
      <button class="icon-button" id="date-prev" aria-label="前一天">‹</button>
      <input type="date" id="date-pick" aria-label="选择日期" value="${state.dateKey}" max="${state.today}">
      <button class="icon-button" id="date-next" aria-label="后一天" ${state.dateKey >= state.today ? 'disabled' : ''}>›</button>
      ${state.dateKey !== state.today ? '<button class="button secondary small" id="date-today">回到今天</button>' : ''}
    </div>`;
}

function wireDateNav() {
  $('#date-prev')?.addEventListener('click', () => shiftDay(-1));
  $('#date-next')?.addEventListener('click', () => shiftDay(1));
  $('#date-pick')?.addEventListener('change', (event) => {
    if (!event.target.value) return;
    state.dateKey = event.target.value;
    loadDay();
  });
  $('#date-today')?.addEventListener('click', () => {
    state.dateKey = state.today;
    loadDay();
  });
}

function shiftDay(delta) {
  const date = new Date(`${state.dateKey}T12:00:00`);
  date.setDate(date.getDate() + delta);
  state.dateKey = localDateKey(date.getTime());
  loadDay();
}

async function loadDay() {
  try {
    const [day, report] = await Promise.all([
      window.tracebit.getDay(state.dateKey),
      window.tracebit.getReport(state.dateKey),
    ]);
    state.day = day;
    state.report = report;
    state.reportError = '';
    if (state.view !== 'settings') await render();
  } catch (error) {
    toast(`读取记录失败：${error.message}`, 'error', 6000);
  }
}

function statusStrip(day) {
  const activeHours = (day?.totalActiveMs || 0) / 3600000;
  const switchRate = activeHours > 0.2 ? `${(day.switches / activeHours).toFixed(1)} 次/小时` : `${day?.switches || 0} 次切换`;
  return `
    <section class="status-strip" aria-label="今日概览">
      <div class="current-record">
        <span id="current-record-dot" class="status-dot is-paused"></span>
        <div class="current-record-text">
          <strong id="current-record-title">正在检查</strong>
          <span id="current-record-detail" class="current-record-detail">正在读取记录状态</span>
        </div>
      </div>
      <div class="metric"><span class="metric-label">活跃时长</span><strong class="metric-value">${fmtMs(day?.totalActiveMs || 0)}</strong><span class="metric-note">${day?.firstActive ? `${hhmm(day.firstActive)} 开始` : '暂无活动'}</span></div>
      <div class="metric"><span class="metric-label">应用切换</span><strong class="metric-value">${day?.switches || 0}</strong><span class="metric-note">${switchRate}</span></div>
      <div class="metric"><span class="metric-label">最长专注</span><strong class="metric-value">${day?.longestBlock ? fmtMs(day.longestBlock.ms) : '暂无'}</strong><span class="metric-note">${day?.longestBlock ? esc(day.longestBlock.app) : '不足 1 分钟'}</span></div>
      <div class="metric"><span class="metric-label">本地快照</span><strong class="metric-value">${day?.screenshots?.length || 0}</strong><span class="metric-note">${fmtBytes(day?.snapshotBytes || 0)}</span></div>
    </section>`;
}

/* Today */
function renderToday() {
  const day = state.day;
  const isToday = state.dateKey === state.today;
  const view = $('#view');
  const hasEvents = Boolean(day?.exists);

  view.innerHTML = `
    ${pageHead(isToday ? '今天' : state.dateKey, fullDate(state.dateKey), '每日活动')}
    ${statusStrip(day)}
    ${hasEvents ? todayContent(day) : emptyToday()}
  `;
  wireDateNav();
  wireTimeline();
  wireAppRows();
  wireSnapshots();
  updateRecUI();
}

function emptyToday() {
  const active = isRecording();
  return `
    <section class="panel empty-state">
      <h2>${active ? '记录已经开启' : '这一天还没有活动记录'}</h2>
      <p>${active
        ? '正常使用电脑即可。切换应用或窗口后，时间线、应用排行和屏幕快照会自动出现在这里。'
        : '开启后，TraceBit 会在本机记录前台应用和去重后的屏幕快照。你可以随时在左侧暂停。'}</p>
      ${active ? '' : '<button class="button primary" data-action="toggle-recording">开始记录</button>'}
    </section>`;
}

function todayContent(day) {
  return `
    <section class="panel">
      <div class="panel-head">
        <h2 class="panel-title">24 小时时间线</h2>
        <span class="panel-hint">颜色代表活动类型，悬停查看窗口</span>
        <div class="panel-actions"><span class="panel-hint">${day.sessions.length} 段活动 · ${day.longIdles} 次长休息</span></div>
      </div>
      ${timelineHTML(day)}
    </section>

    <div class="content-grid">
      <section class="panel">
        <div class="panel-head">
          <h2 class="panel-title">应用使用</h2>
          <span class="panel-hint">按活跃时长排序</span>
        </div>
        <div class="app-table-head"><span>应用</span><span>类型</span><span style="text-align:right">时长</span><span style="text-align:right">切换</span><span></span></div>
        <div>${appListHTML(day)}</div>
      </section>
      <div class="side-stack">
        ${categoryPanel(day)}
        ${aiPanel()}
      </div>
    </div>

    <section class="panel">
      <div class="panel-head">
        <h2 class="panel-title">屏幕快照</h2>
        <span class="panel-hint">只保留画面有变化的帧，点击图片放大</span>
        <div class="panel-actions"><span class="panel-hint">${day.screenshots.length} 张</span></div>
      </div>
      ${snapshotsHTML(day)}
    </section>`;
}

function timelineHTML(day) {
  const width = 1000;
  const trackY = 28;
  const trackH = 42;
  const x = (timestamp) => {
    const date = new Date(timestamp);
    return ((date.getHours() * 60 + date.getMinutes() + date.getSeconds() / 60) / 1440) * width;
  };
  let grid = '';
  for (let hour = 0; hour <= 24; hour += 3) {
    const tx = (hour / 24) * width;
    grid += `<line x1="${tx}" y1="${trackY - 6}" x2="${tx}" y2="${trackY + trackH + 6}" stroke="#e5e0d7" stroke-width="1" vector-effect="non-scaling-stroke"/>`;
    grid += `<text x="${Math.max(10, Math.min(tx, width - 12))}" y="${trackY + trackH + 28}" fill="#8b857b" font-size="11" text-anchor="middle">${hour}时</text>`;
  }
  const segments = (day.stays || []).map((stay) => {
    const x1 = x(stay.start);
    const x2 = Math.max(x(stay.end), x1 + 2.4);
    return `<rect x="${x1.toFixed(1)}" y="${trackY}" width="${(x2 - x1).toFixed(1)}" height="${trackH}" rx="2" fill="${catColor(stay.category)}" data-start="${stay.start}" data-end="${stay.end}" data-app="${esc(stay.app)}" data-title="${esc(stay.title || '')}"/>`;
  }).join('');
  const nowX = state.dateKey === state.today ? x(Date.now()) : -100;
  return `
    <div class="timeline-wrap">
      <svg class="timeline-svg" viewBox="0 0 ${width} 100" preserveAspectRatio="none" aria-label="今日应用时间线">
        <rect x="0" y="${trackY}" width="${width}" height="${trackH}" rx="4" fill="#eeeae2"/>
        ${grid}
        ${segments}
        ${nowX >= 0 ? `<line x1="${nowX}" y1="${trackY - 10}" x2="${nowX}" y2="${trackY + trackH + 10}" stroke="#24231f" stroke-width="1.5" vector-effect="non-scaling-stroke"/><text x="${Math.min(nowX, width - 25)}" y="12" fill="#24231f" font-size="11">现在</text>` : ''}
      </svg>
      <div class="timeline-legend"><span>每段是一个持续使用的应用</span><span>${day.stays.length} 个应用片段</span></div>
      <div class="timeline-tooltip" id="timeline-tooltip"></div>
    </div>`;
}

function wireTimeline() {
  const tooltip = $('#timeline-tooltip');
  if (!tooltip) return;
  $$('.timeline-svg rect[data-app]').forEach((rect) => {
    rect.addEventListener('mousemove', (event) => {
      tooltip.style.display = 'block';
      tooltip.style.left = `${Math.min(event.clientX + 14, window.innerWidth - 360)}px`;
      tooltip.style.top = `${event.clientY + 14}px`;
      tooltip.innerHTML = `
        <div class="tip-time">${hhmm(Number(rect.dataset.start))} – ${hhmm(Number(rect.dataset.end))}</div>
        <div class="tip-app">${esc(rect.dataset.app)}</div>
        ${rect.dataset.title ? `<div class="tip-title">${esc(rect.dataset.title)}</div>` : ''}`;
    });
    rect.addEventListener('mouseleave', () => { tooltip.style.display = 'none'; });
  });
}

function appListHTML(day) {
  if (!day.apps.length) return '<div class="empty-row">还没有可归类的应用活动</div>';
  const max = Math.max(1, ...day.apps.map((app) => app.ms));
  return day.apps.slice(0, 12).map((app, index) => `
    <div class="app-row">
      <div class="app-row-main">
        <div class="app-identity">
          <div class="app-name" title="${esc(app.app || app.name)}">${esc(app.app || app.name)}</div>
          <div class="app-mini-bar"><span style="width:${(app.ms / max * 100).toFixed(1)}%;background:${catColor(app.category)}"></span></div>
        </div>
        <span class="app-category"><i class="category-swatch" style="background:${catColor(app.category)}"></i>${esc(app.category)}</span>
        <span class="app-number">${fmtMs(app.ms)}</span>
        <span class="app-switch-count">${app.switches}</span>
        ${app.titles.length ? `<button class="app-expand" data-expand="${index}" aria-label="展开 ${esc(app.app)} 的窗口标题" aria-expanded="false">▾</button>` : '<span></span>'}
      </div>
      ${app.titles.length ? `<div class="app-titles hidden" data-titles="${index}"><ul>${app.titles.map((item) => `
        <li><span class="title-count">×${item.c}</span><span class="title-text" title="${esc(item.t)}">${esc(item.t)}</span></li>`).join('')}</ul></div>` : ''}
    </div>`).join('');
}

function wireAppRows() {
  $$('[data-expand]').forEach((button) => {
    button.addEventListener('click', () => {
      const titles = $(`[data-titles="${button.dataset.expand}"]`);
      if (!titles) return;
      const hidden = titles.classList.toggle('hidden');
      button.textContent = hidden ? '▾' : '▴';
      button.setAttribute('aria-expanded', String(!hidden));
    });
  });
}

function categoryPanel(day) {
  const totals = new Map();
  for (const app of day.apps) totals.set(app.category, (totals.get(app.category) || 0) + app.ms);
  const categories = [...totals.entries()].sort((a, b) => b[1] - a[1]);
  const total = Math.max(1, day.totalActiveMs);
  return `
    <section class="panel">
      <div class="panel-head"><h2 class="panel-title">时间构成</h2></div>
      <div class="category-list">
        ${categories.length ? categories.map(([category, ms]) => `
          <div class="category-row">
            <div class="category-line">
              <span class="category-name"><i class="category-swatch" style="background:${catColor(category)}"></i>${esc(category)}</span>
              <span class="category-time">${fmtMs(ms)}</span>
              <span class="category-percent">${Math.round(ms / total * 100)}%</span>
            </div>
            <div class="category-bar"><span style="width:${(ms / total * 100).toFixed(1)}%;background:${catColor(category)}"></span></div>
          </div>`).join('') : '<div class="empty-row">暂无分类数据</div>'}
      </div>
    </section>`;
}

function aiPanel() {
  const isToday = state.dateKey === state.today;
  const label = isToday ? '今日' : state.dateKey;
  if (state.report) {
    return `
      <section class="panel ai-panel">
        <div class="panel-head"><h2 class="panel-title">${label} AI 日报</h2><span class="panel-hint">已生成</span></div>
        <div class="ai-content">
          <div class="ai-report-link">${esc(state.report.dateKey)} 行为分析</div>
          <p>${esc(reportExcerpt(state.report.markdown))}…</p>
          <div class="ai-actions"><button class="button brand small" data-action="go-report">查看完整日报</button></div>
        </div>
      </section>`;
  }
  return `
    <section class="panel ai-panel">
      <div class="panel-head"><h2 class="panel-title">${label} AI 日报</h2></div>
      <div class="ai-content">
        <p>${hasAI()
          ? '模型已配置。把今天的应用、切换和专注数据整理成行为复盘，以及明天能直接执行的省力建议。'
          : '连接一个模型后，就能把今天的记录变成行为复盘和具体省力建议。窗口标题会脱敏，截图不会发送。'}</p>
        <div class="ai-actions">
          ${hasAI()
            ? `<button class="button brand small" data-action="generate-report" ${state.generating ? 'disabled' : ''}>${state.generating ? '正在生成…' : '生成今日日报'}</button>`
            : '<button class="button brand small" data-action="setup-ai">配置 AI</button>'}
        </div>
      </div>
    </section>`;
}

function snapshotsHTML(day) {
  state.lightboxShots = day.screenshots.slice().reverse();
  if (!state.lightboxShots.length) return '<div class="empty-snapshots">还没有快照。只有画面发生变化时才会保存，不会持续录屏。</div>';
  return `<div class="snapshot-grid">${state.lightboxShots.slice(0, 6).map((shot, index) => `
    <div class="snapshot" data-shot-index="${index}" role="button" tabindex="0" aria-label="查看 ${hhmm(shot.ts)} 的快照">
      <img src="tbimg://p/${encodeURIComponent(shot.file)}" loading="lazy" width="320" height="180" alt="${hhmm(shot.ts)} 的屏幕快照">
      <div class="snapshot-caption"><span>${hhmm(shot.ts)}</span><span class="snapshot-trigger">${TRIGGER_LABELS[shot.trigger] || shot.trigger}</span></div>
    </div>`).join('')}</div>`;
}

function wireSnapshots() {
  $$('[data-shot-index]').forEach((element) => {
    const open = () => openLightbox(Number(element.dataset.shotIndex));
    element.addEventListener('click', open);
    element.addEventListener('keydown', (event) => {
      if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); open(); }
    });
  });
}

/* AI report */
async function renderReport() {
  state.reportList = await window.tracebit.getReportList();
  const view = $('#view');
  const day = state.day;
  const report = state.report;

  view.innerHTML = `
    ${pageHead('AI 行为日报', '看清时间去向，拿到今天就能用的省力建议', '每日复盘')}
    ${!hasAI() ? reportAISetup() : `
      <section class="panel">
        <div class="panel-head report-toolbar">
          <h2 class="panel-title">${state.dateKey === state.today ? '今日' : state.dateKey} 报告</h2>
          <span class="toolbar-status">${day?.exists ? `已记录 ${fmtMs(day.totalActiveMs)} · ${day.switches} 次切换` : '当天没有活动记录'}</span>
          <div class="panel-actions">
            <button class="button primary small" data-action="generate-report" ${state.generating || !day?.exists ? 'disabled' : ''}>${report ? '重新生成' : '生成日报'}</button>
          </div>
        </div>
        <div id="report-content">${reportContentHTML(report)}</div>
      </section>`}
    ${state.reportList.length ? reportHistoryHTML() : ''}
  `;
  wireDateNav();
  wireReportSetup();
  $$('[data-report-date]').forEach((button) => {
    button.addEventListener('click', () => {
      state.dateKey = button.dataset.reportDate;
      loadDay();
    });
  });
}

function reportAISetup() {
  const ai = state.settings.ai;
  return `
    <section class="panel setup-panel">
      <div class="panel-head"><h2 class="panel-title">先连接一个 AI 模型</h2></div>
      <div class="panel-body">
        <p class="setup-intro">支持 DeepSeek、通义、Kimi、OpenAI 和本地 Ollama。保存后可立即测试，成功后就能生成日报。接口密钥只保存在本机设置文件中。</p>
        ${state.reportError ? `<div class="notice error">${esc(state.reportError)}</div>` : ''}
        <div class="form-grid">
          <div class="form-field full"><label for="report-ai-base">接口地址</label><input id="report-ai-base" type="text" value="${esc(ai.baseUrl)}" placeholder="https://api.deepseek.com/v1"><span class="field-hint">本地 Ollama：http://localhost:11434/v1</span></div>
          <div class="form-field"><label for="report-ai-key">API Key</label><input id="report-ai-key" type="password" value="${esc(ai.apiKey)}" placeholder="sk-…，本地模型可留空"></div>
          <div class="form-field"><label for="report-ai-model">模型名</label><input id="report-ai-model" type="text" value="${esc(ai.model)}" placeholder="deepseek-chat"></div>
        </div>
        <div class="form-actions">
          <button class="button primary" id="report-ai-save">保存并测试</button>
          <span id="report-ai-result" class="inline-result"></span>
        </div>
      </div>
    </section>
    <section class="panel setup-panel">
      <div class="panel-body privacy-note">发送给模型的内容：活跃时长、应用切换、应用名和脱敏后的窗口标题。<br>不会发送：屏幕快照、API Key、键盘输入内容。涉及敏感标题时，建议使用本地模型。</section>
    </section>`;
}

function reportContentHTML(report) {
  if (state.generating) {
    return `<div class="generating-state"><div class="generating-line"><span class="spinner"></span>正在生成行为日报</div><p>正在整理当天记录并请求模型，通常需要几十秒。生成期间可以继续使用电脑。</p></div>`;
  }
  if (state.reportError) {
    return `<div class="panel-body"><div class="notice error">${esc(state.reportError)}</div><button class="button secondary" data-action="generate-report">重试生成</button></div>`;
  }
  if (!report) {
    return `<div class="empty-state"><h2>还没有这一天的日报</h2><p>点击右上角“生成日报”。TraceBit 会先在本机整理和脱敏数据，再把统计发送给你配置的模型。</p></div>`;
  }
  return `
    <div class="report-meta"><span>生成于 ${new Date(report.generatedAt).toLocaleString('zh-CN')}</span><span>模型 ${esc(report.model || '未知')}</span><span>活跃 ${report.activeMinutes ?? '未知'} 分钟</span></div>
    <article class="report-document md">${renderMarkdown(report.markdown)}</article>`;
}

function reportHistoryHTML() {
  return `
    <section class="panel">
      <div class="panel-head"><h2 class="panel-title">历史日报</h2><span class="panel-hint">${state.reportList.length} 份</span></div>
      <div>${state.reportList.slice(0, 30).map((item) => `
        <button class="report-history-row" data-report-date="${item.dateKey}">
          <span class="report-history-date">${item.dateKey}</span>
          <span class="report-history-meta">${item.activeMinutes ?? '未知'} 分钟活跃 · ${esc(item.model || '')}</span>
          <span class="report-history-arrow">查看 ›</span>
        </button>`).join('')}</div>
    </section>`;
}

function wireReportSetup() {
  const button = $('#report-ai-save');
  if (!button) return;
  button.addEventListener('click', async () => {
    const result = $('#report-ai-result');
    button.disabled = true;
    result.className = 'inline-result';
    result.innerHTML = '<span class="spinner"></span> 正在保存并测试';
    try {
      const ai = {
        baseUrl: $('#report-ai-base').value.trim(),
        apiKey: $('#report-ai-key').value.trim(),
        model: $('#report-ai-model').value.trim(),
      };
      if (!ai.baseUrl || !ai.model) throw new Error('接口地址和模型名不能为空');
      state.settings = await window.tracebit.saveSettings({ ai });
      const test = await window.tracebit.testAI();
      if (!test.ok) throw new Error(test.error);
      result.className = 'inline-result success';
      result.textContent = `连接正常：${test.model}`;
      toast('AI 已连接，可以生成日报了', 'success');
      setTimeout(() => renderReport(), 500);
    } catch (error) {
      result.className = 'inline-result error';
      result.textContent = error.message;
      state.reportError = error.message;
    } finally {
      button.disabled = false;
    }
  });
}

async function generateReport() {
  if (state.generating) return;
  if (!hasAI()) { setView('report'); return; }
  if (!state.day?.exists) { toast('这一天还没有可分析的记录', 'error'); return; }
  state.generating = true;
  state.reportError = '';
  await render();
  try {
    const response = await window.tracebit.generateReport(state.dateKey);
    if (!response.ok) throw new Error(response.error);
    state.report = response.report;
    toast('行为日报已生成', 'success');
  } catch (error) {
    state.reportError = error.message;
    toast(`生成失败：${error.message}`, 'error', 6500);
  } finally {
    state.generating = false;
    await render();
  }
}

/* Settings */
function switchHTML(id, checked, label) {
  return `<label class="switch"><input type="checkbox" id="${id}" ${checked ? 'checked' : ''}><span class="switch-track"></span></label>${label ? `<span class="switch-label">${esc(label)}</span>` : ''}`;
}

function renderSettings() {
  state.settingsDirty = false;
  const settings = state.settings;
  const rec = settings.recording;
  const ai = settings.ai;
  const storage = settings.storage;
  $('#view').innerHTML = `
    ${pageHead('设置', '采集、隐私、存储和模型都集中在这里', '偏好设置', false)}
    <div class="settings-form">
      <section class="settings-section">
        <div class="settings-section-head"><h2 class="settings-section-title">记录</h2><p class="settings-section-desc">开始和暂停也可以直接使用左侧固定按钮</p></div>
        <div class="settings-block">
          ${settingRow('启用行为记录', '与左侧“开始记录 / 暂停记录”保持同步', switchHTML('rec-enabled', rec.enabled, rec.enabled ? '已开启' : '已暂停'))}
          ${settingRow('保存屏幕快照', '只在应用切换、定时心跳且画面变化时保存', switchHTML('rec-shots', rec.screenshots, rec.screenshots ? '已开启' : '已关闭'))}
          ${settingRow('快照心跳间隔', '活跃期间定时检查画面，单位：分钟', '<input type="number" id="rec-heartbeat" min="1" max="60" value="' + Math.round(rec.heartbeatMs / 60000) + '">')}
          ${settingRow('空闲阈值', '无键鼠输入超过该时间后，不计入活跃时长', '<input type="number" id="rec-idle" min="1" max="30" value="' + Math.round(rec.idleThresholdMs / 60000) + '"><span class="switch-label">分钟</span>')}
        </div>
      </section>

      <section class="settings-section">
        <div class="settings-section-head"><h2 class="settings-section-title">AI 分析</h2><p class="settings-section-desc">OpenAI 兼容接口，密钥只保存在本机</p></div>
        <div class="settings-block">
          ${settingRow('接口地址', '例如 https://api.deepseek.com/v1', `<input type="text" id="ai-base" value="${esc(ai.baseUrl)}" placeholder="https://api.deepseek.com/v1">`)}
          ${settingRow('API Key', '本地 Ollama 可以留空', `<input type="password" id="ai-key" value="${esc(ai.apiKey)}" placeholder="sk-…">`)}
          ${settingRow('模型名', '例如 deepseek-chat、qwen-plus、gpt-4o-mini', `<input type="text" id="ai-model" value="${esc(ai.model)}" placeholder="deepseek-chat">`)}
          ${settingRow('每日自动生成', '到点分析当天记录并发送系统通知', `${switchHTML('ai-auto', ai.autoReport)}<input type="time" id="ai-auto-time" value="${esc(ai.autoReportTime)}">`)}
          ${settingRow('连接检查', '先保存当前设置，再发送一条极短测试消息', '<button class="button secondary" id="ai-test">保存并测试连接</button><span id="ai-test-result" class="inline-result"></span>')}
        </div>
      </section>

      <section class="settings-section">
        <div class="settings-section-head"><h2 class="settings-section-title">隐私与存储</h2><p class="settings-section-desc">敏感窗口不截图，快照永远不会发送给 AI</p></div>
        <div class="settings-block">
          ${settingRow('敏感内容关键词', '每行一个。前台应用或标题命中时跳过截图', `<textarea id="priv-patterns">${esc((settings.privacy.sensitivePatterns || []).join('\n'))}</textarea>`, true)}
          ${settingRow('快照占用上限', `超出后删除最旧日期。当前 ${fmtBytes(state.status?.totalSnapshotBytes || 0)}`, '<input type="number" id="st-cap" min="1" max="100" value="' + Math.round(storage.maxSnapshotBytes / 1024 ** 3) + '"><span class="switch-label">GB</span>')}
          ${settingRow('数据保留天数', '超期的每日记录和快照会自动清理', `<input type="number" id="st-retention" min="7" max="3650" value="${storage.retentionDays}"><span class="switch-label">天</span>`)}
          ${settingRow('开机自启', '登录 Windows 后在后台启动 TraceBit', switchHTML('app-autostart', settings.app.autostart))}
          ${settingRow('数据目录', `<span class="storage-path">${esc(state.status?.dataDir || '')}</span>`, '<button class="button secondary small" id="open-data">打开目录</button><button class="button danger small" id="purge-shots">清空快照</button><button class="button danger small" id="purge-all">清空全部记录</button>')}
        </div>
      </section>

      <div class="settings-savebar">
        <button class="button primary" id="save-settings">保存设置</button>
        <span id="save-status" class="save-status">修改后请点击保存，记录开关除外</span>
      </div>
    </div>
  `;
  wireSettings();
}

function settingRow(name, description, control, alignStart = false) {
  return `<div class="setting-row ${alignStart ? 'align-start' : ''}"><div><div class="setting-name">${name}</div><div class="setting-description">${description}</div></div><div class="setting-control">${control}</div></div>`;
}

function wireSettings() {
  $('#save-settings').addEventListener('click', () => saveSettings(false));
  $('#ai-test').addEventListener('click', () => testSettingsAI());
  $$('.settings-form input, .settings-form textarea, .settings-form select').forEach((input) => {
    input.addEventListener('change', () => { state.settingsDirty = true; });
    input.addEventListener('input', () => { state.settingsDirty = true; });
  });
  [['#rec-enabled', '已开启', '已暂停'], ['#rec-shots', '已开启', '已关闭']].forEach(([selector, onText, offText]) => {
    $(selector).addEventListener('change', (event) => {
      const label = $('.switch-label', event.target.closest('.setting-control'));
      if (label) label.textContent = event.target.checked ? onText : offText;
    });
  });
  $('#open-data').addEventListener('click', () => window.tracebit.openDataDir());
  wirePurgeButton($('#purge-shots'), 'screenshots', '确认清空快照？', '快照已清空');
  wirePurgeButton($('#purge-all'), 'all', '确认清空全部记录？', '全部记录已清空');
}

function wirePurgeButton(button, scope, armedText, doneText) {
  button.addEventListener('click', async () => {
    if (!button.dataset.armed) {
      button.dataset.armed = '1';
      button.textContent = armedText;
      setTimeout(() => { button.dataset.armed = ''; button.textContent = scope === 'all' ? '清空全部记录' : '清空快照'; }, 4000);
      return;
    }
    await window.tracebit.purgeData(scope);
    button.dataset.armed = '';
    button.textContent = scope === 'all' ? '清空全部记录' : '清空快照';
    toast(doneText, 'success');
  });
}

async function saveSettings(silent) {
  const saveButton = $('#save-settings');
  const saveStatus = $('#save-status');
  if (saveButton) saveButton.disabled = true;
  if (saveStatus) saveStatus.textContent = '正在保存…';
  try {
    const partial = {
      recording: {
        enabled: $('#rec-enabled').checked,
        screenshots: $('#rec-shots').checked,
        heartbeatMs: Math.max(60000, (parseFloat($('#rec-heartbeat').value) || 5) * 60000),
        idleThresholdMs: Math.max(30000, (parseFloat($('#rec-idle').value) || 1) * 60000),
      },
      privacy: { sensitivePatterns: $('#priv-patterns').value.split('\n').map((item) => item.trim()).filter(Boolean) },
      storage: {
        maxSnapshotBytes: Math.max(1, parseFloat($('#st-cap').value) || 2) * 1024 ** 3,
        retentionDays: Math.max(7, parseInt($('#st-retention').value, 10) || 180),
      },
      ai: {
        baseUrl: $('#ai-base').value.trim(),
        apiKey: $('#ai-key').value.trim(),
        model: $('#ai-model').value.trim(),
        autoReport: $('#ai-auto').checked,
        autoReportTime: $('#ai-auto-time').value || '21:30',
      },
      app: { autostart: $('#app-autostart').checked },
    };
    state.settings = await window.tracebit.saveSettings(partial);
    state.settingsDirty = false;
    const fresh = await window.tracebit.getState();
    state.status = fresh.status;
    updateRecUI();
    if (!silent) toast('设置已保存', 'success');
    if (saveStatus) saveStatus.textContent = '已保存';
    return true;
  } catch (error) {
    if (saveStatus) saveStatus.textContent = `保存失败：${error.message}`;
    toast(`保存失败：${error.message}`, 'error', 6000);
    return false;
  } finally {
    if (saveButton) saveButton.disabled = false;
  }
}

async function testSettingsAI() {
  const result = $('#ai-test-result');
  result.className = 'inline-result';
  result.innerHTML = '<span class="spinner"></span> 正在测试';
  const saved = await saveSettings(true);
  if (!saved) { result.textContent = '设置保存失败'; result.className = 'inline-result error'; return; }
  try {
    const response = await window.tracebit.testAI();
    if (!response.ok) throw new Error(response.error);
    result.className = 'inline-result success';
    result.textContent = `连接正常：${response.model}`;
    toast('AI 接口可用', 'success');
  } catch (error) {
    result.className = 'inline-result error';
    result.textContent = error.message;
  }
}

/* Lightbox */
function openLightbox(index) {
  if (!state.lightboxShots.length) return;
  state.lightboxIndex = Math.max(0, Math.min(index, state.lightboxShots.length - 1));
  state.lastFocused = document.activeElement;
  updateLightbox();
  $('#lightbox').classList.remove('hidden');
  $('.lightbox-close').focus();
}
function updateLightbox() {
  const shot = state.lightboxShots[state.lightboxIndex];
  if (!shot) return;
  $('#lightbox-img').src = `tbimg://p/${encodeURIComponent(shot.file)}`;
  $('#lightbox-cap').textContent = `${hhmm(shot.ts)} · ${TRIGGER_LABELS[shot.trigger] || shot.trigger} · ${state.lightboxIndex + 1} / ${state.lightboxShots.length}`;
  const multiple = state.lightboxShots.length > 1;
  $('.lightbox-prev').disabled = !multiple;
  $('.lightbox-next').disabled = !multiple;
}
function moveLightbox(delta) {
  const count = state.lightboxShots.length;
  if (!count) return;
  state.lightboxIndex = (state.lightboxIndex + delta + count) % count;
  updateLightbox();
}
function closeLightbox() {
  $('#lightbox').classList.add('hidden');
  state.lastFocused?.focus?.();
}

/* Global actions and boot */
document.addEventListener('click', (event) => {
  const target = event.target.closest('[data-action]');
  if (!target) return;
  const action = target.dataset.action;
  if (action === 'toggle-recording') toggleRecording();
  if (action === 'setup-ai') setView('report');
  if (action === 'go-report') setView('report');
  if (action === 'generate-report') generateReport();
});

$$('#nav .nav-item').forEach((button) => button.addEventListener('click', () => setView(button.dataset.view)));
window.tracebit.onStatus((status) => {
  state.status = status;
  updateRecUI();
});
window.tracebit.onGoto((view) => setView(view));
window.tracebit.onReportGenerated(async ({ dateKey }) => {
  toast('今日日报已自动生成', 'success');
  if (state.dateKey === dateKey) {
    state.report = await window.tracebit.getReport(dateKey);
    if (state.view !== 'settings') await render();
  }
});

$('.lightbox-close').addEventListener('click', closeLightbox);
$('.lightbox-prev').addEventListener('click', () => moveLightbox(-1));
$('.lightbox-next').addEventListener('click', () => moveLightbox(1));
$('#lightbox-open').addEventListener('click', () => {
  const shot = state.lightboxShots[state.lightboxIndex];
  if (shot) window.tracebit.openSnapshot(shot.file);
});
$('#lightbox').addEventListener('click', (event) => { if (event.target === $('#lightbox')) closeLightbox(); });
document.addEventListener('keydown', (event) => {
  if ($('#lightbox').classList.contains('hidden')) return;
  if (event.key === 'Escape') closeLightbox();
  if (event.key === 'ArrowLeft') moveLightbox(-1);
  if (event.key === 'ArrowRight') moveLightbox(1);
});

function render() {
  if (state.view === 'today') { renderToday(); return Promise.resolve(); }
  if (state.view === 'report') return renderReport();
  renderSettings();
  return Promise.resolve();
}

setInterval(() => {
  if (state.view === 'today' && state.dateKey === state.today && !document.hidden && !state.generating) loadDay();
}, 30000);

async function boot() {
  try {
    const initial = await window.tracebit.getState();
    state.settings = initial.settings;
    state.status = initial.status;
    state.today = initial.today;
    state.dateKey = initial.today;
    updateRecUI();
    await loadDay();
  } catch (error) {
    $('#view').innerHTML = `<div class="notice error">应用初始化失败：${esc(error.message)}</div>`;
    toast(`初始化失败：${error.message}`, 'error', 8000);
  }
}

boot();
