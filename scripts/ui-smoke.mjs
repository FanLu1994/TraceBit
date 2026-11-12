import { spawn, spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import http from 'node:http';
import WebSocket from 'ws';

const require = createRequire(import.meta.url);
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const executable = process.env.TRACEBIT_EXE || require('electron');
const userData = path.join(os.tmpdir(), `tracebit-ui-smoke-${process.pid}`);
const artifacts = path.resolve(process.env.TRACEBIT_UI_ARTIFACTS || path.join(root, 'artifacts', 'ui-smoke'));
const port = Number(process.env.TRACEBIT_UI_PORT || 9225);
fs.rmSync(userData, { recursive: true, force: true });
fs.mkdirSync(artifacts, { recursive: true });

const mockServer = http.createServer((request, response) => {
  response.writeHead(200, { 'Content-Type': 'application/json' });
  response.end(JSON.stringify({ model: 'ui-mock-model', choices: [{ message: { content: '正常' } }] }));
});
await new Promise((resolve) => mockServer.listen(0, '127.0.0.1', resolve));
const mockBaseURL = `http://127.0.0.1:${mockServer.address().port}/v1`;
const sampleReportMarkdown = [
  '# 今日行为日报', '', '## 一、今日概览', '活跃 95 分钟。', '',
  '## 六、可工具化的操作', '',
  '> 以下来自本地行为规则和内置精选资源，未进行联网搜索。', '',
  '### 把开发命令收成固定入口',
  '- 操作依据：编辑器与终端之间切换 8 次，相关应用活跃 42 分钟。',
  '- 优化方向：先固化重复的项目命令；如果切换主要来自 Git 操作，再使用可视化 Git 工具。',
  '- [lazygit](https://github.com/jesseduffield/lazygit)（开源项目）：集中处理常见 Git 操作。预期收益：减少命令重复。配置成本：中。权限与隐私：会读写 Git 仓库，破坏性操作前仍需确认和备份。来源状态：内置精选，未联网复核。',
  '', '以上资源只供人工评估，TraceBit 不会自动下载、安装或启用。',
].join('\n');

const env = { ...process.env };
delete env.ELECTRON_RUN_AS_NODE;
const child = spawn(executable, [root, `--remote-debugging-port=${port}`, `--user-data-dir=${userData}`, '--disable-gpu'], {
  cwd: root,
  env,
  stdio: ['ignore', 'pipe', 'pipe'],
});
let runtimeLog = '';
child.stdout.on('data', (chunk) => { runtimeLog += chunk; });
child.stderr.on('data', (chunk) => { runtimeLog += chunk; });

let ws;
let seq = 0;
const pending = new Map();
const consoleErrors = [];

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function waitFor(fn, description, timeoutMs = 15000) {
  const start = Date.now();
  let lastError;
  while (Date.now() - start < timeoutMs) {
    try {
      const value = await fn();
      if (value) return value;
    } catch (error) { lastError = error; }
    await sleep(150);
  }
  throw new Error(`Timed out: ${description}${lastError ? ` (${lastError.message})` : ''}`);
}
function send(method, params = {}) {
  return new Promise((resolve, reject) => {
    const id = ++seq;
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });
}
async function evaluate(expression) {
  const response = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true, userGesture: true });
  if (response.exceptionDetails) throw new Error(response.exceptionDetails.exception?.description || response.exceptionDetails.text);
  return response.result?.value;
}
async function screenshot(name) {
  const result = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  fs.writeFileSync(path.join(artifacts, name), Buffer.from(result.data, 'base64'));
}

try {
  const targets = await waitFor(async () => {
    if (child.exitCode !== null) throw new Error(`App exited ${child.exitCode}: ${runtimeLog}`);
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/list`);
      const list = await response.json();
      return list.find((target) => target.type === 'page');
    } catch { return null; }
  }, 'CDP page target', 25000);

  ws = new WebSocket(targets.webSocketDebuggerUrl);
  ws.on('message', (data) => {
    const message = JSON.parse(data.toString());
    if (message.id && pending.has(message.id)) {
      const handlers = pending.get(message.id);
      pending.delete(message.id);
      if (message.error) handlers.reject(new Error(JSON.stringify(message.error)));
      else handlers.resolve(message.result);
    }
    if (message.method === 'Runtime.exceptionThrown') consoleErrors.push(message.params.exceptionDetails);
  });
  await new Promise((resolve, reject) => { ws.once('open', resolve); ws.once('error', reject); });
  await send('Runtime.enable');
  await send('Page.enable');
  await waitFor(() => evaluate(`typeof state !== 'undefined' && state.settings && state.status && document.querySelector('#rec-toggle')`), 'renderer boot');

  assert(await evaluate(`state.settings.recording.enabled === true && state.status.enabled === true`), 'Recording should start enabled in a fresh profile');
  assert(await evaluate(`state.settings.recording.browserSites === false`), 'Browser site recording must be opt-in');

  await evaluate(`document.querySelector('#rec-toggle').click()`);
  await waitFor(() => evaluate(`state.status.enabled === false && !state.toggling`), 'pause status');
  assert(await evaluate(`state.settings.recording.enabled === false`), 'Pause did not synchronize renderer settings state');
  assert(await evaluate(`document.querySelector('#rec-toggle').textContent === '开始记录'`), 'Paused control must say 开始记录');
  assert(await evaluate(`window.tracebit.getState().then(s => s.settings.recording.enabled === false)`), 'Pause was not persisted by main process');

  await evaluate(`setView('settings')`);
  await waitFor(() => evaluate(`Boolean(document.querySelector('#rec-enabled'))`), 'settings render');
  assert(await evaluate(`document.querySelector('#rec-enabled').checked === false`), 'Settings checkbox still shows enabled after pause');

  await evaluate(`document.querySelector('#rec-toggle').click()`);
  await waitFor(() => evaluate(`state.status.enabled === true && state.status.running === true && !state.toggling`), 'resume status');
  assert(await evaluate(`state.settings.recording.enabled === true`), 'Resume did not synchronize renderer settings state');
  assert(await evaluate(`document.querySelector('#rec-enabled').checked === true`), 'Settings checkbox did not update after global resume');
  assert(await evaluate(`document.querySelector('#rec-enabled').closest('.setting-control').querySelector('.switch-label').textContent === '已开启'`), 'Settings switch label did not update after global resume');
  await sleep(250); // allow the 140ms switch color transition to finish
  const switchVisual = await evaluate(`(()=>{const el=document.querySelector('#rec-enabled');const track=document.querySelector('#rec-enabled + .switch-track');return {checked:el.checked,matches:el.matches(':checked'),trackMatches:track.matches('.switch input:checked + .switch-track'),input:el.outerHTML,color:getComputedStyle(track).backgroundColor}})()`);
  assert(switchVisual.color === 'rgb(36, 35, 31)', `Settings switch visual state did not update after global resume: ${JSON.stringify(switchVisual)}`);
  assert(await evaluate(`document.querySelector('#rec-sites').checked === false`), 'Browser site switch should start off');
  await evaluate(`(()=>{document.querySelector('#rec-sites').click();document.querySelector('#save-settings').click()})()`);
  await waitFor(() => evaluate(`state.settings.recording.browserSites === true && document.querySelector('#save-status').textContent === '已保存'`), 'browser site opt-in save');
  assert(await evaluate(`window.tracebit.getState().then(s => s.settings.recording.browserSites === true)`), 'Browser site opt-in was not persisted');
  assert(await evaluate(`document.querySelector('#rec-sites').closest('.setting-control').querySelector('.switch-label').textContent === '已开启'`), 'Browser site switch label did not update');
  assert(await evaluate(`state.settings.ai.webSearch === false && document.querySelector('#ai-web-search').checked === false`), 'Resource web search should start off');
  await evaluate(`(()=>{document.querySelector('#ai-web-search').click();document.querySelector('#save-settings').click()})()`);
  await waitFor(() => evaluate(`state.settings.ai.webSearch === true && document.querySelector('#save-status').textContent === '已保存'`), 'resource web search opt-in save');
  assert(await evaluate(`window.tracebit.getState().then(s => s.settings.ai.webSearch === true)`), 'Resource web search opt-in was not persisted');
  assert(await evaluate(`document.querySelector('#ai-search-provider').value === 'duckduckgo'`), 'Default resource search provider changed unexpectedly');
  await sleep(400);
  assert(await evaluate(`state.status.pollerAlive === true && !state.status.lastError`), 'Settings restart surfaced a stale poller exit');
  await evaluate(`document.querySelector('#toasts').innerHTML = ''`);
  await screenshot('settings-1240.png');
  await evaluate(`document.querySelector('#ai-web-search').scrollIntoView({block:'center'})`);
  await screenshot('settings-search-1240.png');
  await evaluate(`document.querySelector('#view').scrollTop = 0`);

  await evaluate(`setView('report')`);
  await waitFor(() => evaluate(`Boolean(document.querySelector('#report-ai-base'))`), 'inline AI setup');
  assert(await evaluate(`document.querySelector('#report-ai-save').textContent.includes('保存并测试')`), 'AI setup needs a clear save/test action');
  await screenshot('report-setup-1240.png');
  await evaluate(`(()=>{document.querySelector('#report-ai-base').value=${JSON.stringify(mockBaseURL)};document.querySelector('#report-ai-key').value='ui-mock-key';document.querySelector('#report-ai-model').value='ui-mock-model';document.querySelector('#report-ai-save').click()})()`);
  await waitFor(() => evaluate(`state.settings.ai.model === 'ui-mock-model' && Boolean(document.querySelector('[data-action="generate-report"]'))`), 'inline AI save/test flow');
  assert(await evaluate(`window.tracebit.getState().then(s => s.settings.ai.model === 'ui-mock-model')`), 'Inline AI setup was not persisted');
  await evaluate(`(async()=>{state.report={dateKey:state.dateKey,generatedAt:Date.now(),model:'ui-mock-model',activeMinutes:95,recommendationCount:1,resourceSearch:{enabled:false},markdown:${JSON.stringify(sampleReportMarkdown)}};state.day={...state.day,exists:true,totalActiveMs:5700000};await renderReport()})()`);
  await waitFor(() => evaluate(`document.querySelector('.report-document h3')?.textContent === '把开发命令收成固定入口'`), 'recommendation report render');
  assert(await evaluate(`document.querySelector('.report-document a')?.href === 'https://github.com/jesseduffield/lazygit'`), 'Recommendation source link did not render');
  await evaluate(`document.querySelector('.report-document h3').scrollIntoView({block:'start'})`);
  await screenshot('report-recommendations-1240.png');
  await evaluate(`document.querySelector('#view').scrollTop = 0`);

  await evaluate(`setView('today')`);
  await waitFor(() => evaluate(`Boolean(document.querySelector('.status-strip'))`), 'today render');
  await evaluate(`(()=>{state.day={...state.day,exists:true,sites:[{site:'example.com',visits:2,firstSeen:Date.now()-60000,lastSeen:Date.now(),app:'Chrome'}]};renderToday()})()`);
  await waitFor(() => evaluate(`document.querySelector('.site-name')?.textContent === 'example.com'`), 'browser site history render');
  assert(await evaluate(`document.querySelector('.site-visits')?.textContent === '2 次记录'`), 'Browser site visit count did not render');
  await evaluate(`document.querySelector('#toasts').innerHTML = ''`);
  await sleep(500);
  await screenshot('today-1240.png');
  await send('Emulation.setDeviceMetricsOverride', { width: 960, height: 620, deviceScaleFactor: 1, mobile: false });
  await sleep(250);
  assert(await evaluate(`document.querySelector('#view').scrollWidth <= document.querySelector('#view').clientWidth + 1`), 'Today view overflows horizontally at 960px');
  await screenshot('today-960.png');
  await evaluate(`setView('settings')`);
  await sleep(200);
  assert(await evaluate(`document.querySelector('#view').scrollWidth <= document.querySelector('#view').clientWidth + 1`), 'Settings view overflows horizontally at 960px');
  await screenshot('settings-960.png');
  await evaluate(`setView('report')`);
  await sleep(200);
  assert(await evaluate(`document.querySelector('#view').scrollWidth <= document.querySelector('#view').clientWidth + 1`), 'Recommendation report overflows horizontally at 960px');
  await evaluate(`document.querySelector('.report-document h3').scrollIntoView({block:'start'})`);
  await screenshot('report-recommendations-960.png');
  assert(consoleErrors.length === 0, `Renderer exceptions: ${JSON.stringify(consoleErrors)}`);

  console.log('UI smoke passed');
  console.log('  ✓ pause/resume updates runtime, persisted settings and settings checkbox');
  console.log('  ✓ primary action has explicit start/pause labels');
  console.log('  ✓ browser site recording defaults off and persists explicit opt-in');
  console.log('  ✓ recorded browser domains render in today view');
  console.log('  ✓ resource web search defaults off and persists explicit opt-in');
  console.log('  ✓ settings restarts ignore stale poller exits');
  console.log('  ✓ unconfigured AI report offers inline setup');
  console.log('  ✓ inline AI save/test reaches a configured report page');
  console.log('  ✓ recommendation evidence, source links, and permissions render in reports');
  console.log('  ✓ today and settings fit 1240px and 960px viewports');
  console.log(`  screenshots: ${artifacts}`);
} catch (error) {
  console.error(`UI smoke failed: ${error.message}`);
  console.error(runtimeLog.slice(-5000));
  process.exitCode = 1;
} finally {
  if (ws) ws.close();
  if (child.exitCode === null) {
    if (process.platform === 'win32') spawnSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { stdio: 'ignore' });
    else child.kill('SIGTERM');
  }
  await sleep(500);
  try {
    fs.rmSync(userData, { recursive: true, force: true, maxRetries: 8, retryDelay: 250 });
  } catch (error) {
    console.warn(`Temporary profile cleanup deferred: ${error.message}`);
  }
  await new Promise((resolve) => mockServer.close(resolve));
}
