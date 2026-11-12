'use strict';
// TraceBit settings — stored at userData/settings.json
const fs = require('fs');
const path = require('path');

const DEFAULTS = {
  recording: {
    enabled: true,          // master switch
    screenshots: true,      // capture screen snapshots
    browserSites: false,    // opt-in: record active browser site hostnames locally
    pollMs: 2000,           // foreground poll interval
    tickMs: 30000,          // periodic activity tick
    titleThrottleMs: 8000,  // min gap between title-change events
    focusDebounceMs: 2000,  // debounce before focus-change screenshot
    heartbeatMs: 300000,    // periodic screenshot while active
    idleThresholdMs: 60000, // considered idle after this much no input
    sessionGapMs: 600000,   // gap that splits two sessions
  },
  privacy: {
    sensitivePatterns: [
      '1password', 'bitwarden', 'lastpass', 'dashlane', 'keepass',
      '密码', 'password', '网银', '银行', 'bank', '支付宝', 'alipay',
      '钱包', 'wallet', 'metamask', 'inprivate', 'incognito', '隐私', '私密',
    ],
  },
  storage: {
    maxSnapshotBytes: 2 * 1024 * 1024 * 1024, // 2 GB cap for screenshots
    retentionDays: 180,      // drop day files older than this
    jpegQuality: 72,
    maxSnapWidth: 1600,
  },
  ai: {
    baseUrl: '',            // OpenAI-compatible base URL, e.g. https://api.deepseek.com/v1
    apiKey: '',
    model: '',
    autoReport: false,      // auto-generate daily report
    autoReportTime: '21:30',
  },
  app: {
    autostart: false,
    closeToTray: true,
  },
};

function deepMerge(base, override) {
  const out = { ...base };
  if (!override || typeof override !== 'object') return out;
  for (const key of Object.keys(override)) {
    const v = override[key];
    if (v === undefined) continue;
    if (base[key] && typeof base[key] === 'object' && !Array.isArray(base[key]) &&
        v && typeof v === 'object' && !Array.isArray(v)) {
      out[key] = deepMerge(base[key], v);
    } else {
      out[key] = v;
    }
  }
  return out;
}

class Settings {
  constructor(userDataPath) {
    this.file = path.join(userDataPath, 'settings.json');
    this.data = structuredClone(DEFAULTS);
    this.load();
  }

  load() {
    try {
      if (fs.existsSync(this.file)) {
        const raw = JSON.parse(fs.readFileSync(this.file, 'utf-8'));
        this.data = deepMerge(DEFAULTS, raw);
      }
    } catch (err) {
      console.error('[Settings] load failed:', err.message);
    }
    return this.data;
  }

  save() {
    try {
      fs.mkdirSync(path.dirname(this.file), { recursive: true });
      fs.writeFileSync(this.file, JSON.stringify(this.data, null, 2), 'utf-8');
    } catch (err) {
      console.error('[Settings] save failed:', err.message);
    }
  }

  get() { return this.data; }

  patch(partial) {
    this.data = deepMerge(this.data, partial);
    this.save();
    return this.data;
  }
}

module.exports = { Settings, DEFAULTS };
