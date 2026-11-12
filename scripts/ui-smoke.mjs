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
  await evaluate(`document.querySelector('#toasts').innerHTML = ''`);
  await screenshot('settings-1240.png');

  await evaluate(`setView('report')`);
  await waitFor(() => evaluate(`Boolean(document.querySelector('#report-ai-base'))`), 'inline AI setup');
  assert(await evaluate(`document.querySelector('#report-ai-save').textContent.includes('保存并测试')`), 'AI setup needs a clear save/test action');
  await screenshot('report-setup-1240.png');
  await evaluate(`(()=>{document.querySelector('#report-ai-base').value=${JSON.stringify(mockBaseURL)};document.querySelector('#report-ai-key').value='ui-mock-key';document.querySelector('#report-ai-model').value='ui-mock-model';document.querySelector('#report-ai-save').click()})()`);
  await waitFor(() => evaluate(`state.settings.ai.model === 'ui-mock-model' && Boolean(document.querySelector('[data-action="generate-report"]'))`), 'inline AI save/test flow');
  assert(await evaluate(`window.tracebit.getState().then(s => s.settings.ai.model === 'ui-mock-model')`), 'Inline AI setup was not persisted');

  await evaluate(`setView('today')`);
  await waitFor(() => evaluate(`Boolean(document.querySelector('.status-strip'))`), 'today render');
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
  assert(consoleErrors.length === 0, `Renderer exceptions: ${JSON.stringify(consoleErrors)}`);

  console.log('UI smoke passed');
  console.log('  ✓ pause/resume updates runtime, persisted settings and settings checkbox');
  console.log('  ✓ primary action has explicit start/pause labels');
  console.log('  ✓ unconfigured AI report offers inline setup');
  console.log('  ✓ inline AI save/test reaches a configured report page');
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
