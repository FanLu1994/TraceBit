'use strict';
// TraceBit preload — minimal, typed bridge.
const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('tracebit', {
  getState: () => ipcRenderer.invoke('get-state'),
  saveSettings: (partial) => ipcRenderer.invoke('save-settings', partial),
  getDay: (dateKey) => ipcRenderer.invoke('get-day', dateKey),
  getReport: (dateKey) => ipcRenderer.invoke('get-report', dateKey),
  getReportList: () => ipcRenderer.invoke('get-report-list'),
  generateReport: (dateKey) => ipcRenderer.invoke('generate-report', dateKey),
  testAI: () => ipcRenderer.invoke('test-ai'),
  toggleRecording: () => ipcRenderer.invoke('toggle-recording'),
  openDataDir: () => ipcRenderer.invoke('open-data-dir'),
  openSnapshot: (rel) => ipcRenderer.invoke('open-snapshot', rel),
  purgeData: (scope) => ipcRenderer.invoke('purge-data', scope),
  onStatus: (fn) => ipcRenderer.on('status', (_e, s) => fn(s)),
  onGoto: (fn) => ipcRenderer.on('goto', (_e, view) => fn(view)),
  onReportGenerated: (fn) => ipcRenderer.on('report-generated', (_e, d) => fn(d)),
});
