'use strict';
// App categorization — process name (stable ASCII) -> category.
// Labels come from FileVersionInfo and may be localized; the process name
// is the reliable key, with title patterns as a fallback for browsers etc.

const CATEGORIES = ['开发', '浏览器', '即时通讯', '办公文档', '会议', '设计', '娱乐', '工具', '系统', '其他'];

const CATEGORY_COLORS = {
  '开发': '#4e8ed9',
  '浏览器': '#45b8ac',
  '即时通讯': '#d9a44e',
  '办公文档': '#b57edc',
  '会议': '#e06c75',
  '设计': '#e0876c',
  '娱乐': '#7ec46c',
  '工具': '#8a919f',
  '系统': '#4a4f58',
  '其他': '#6e7480',
};

// Windows process names (lowercase) -> category
const PROC_MAP = {
  // 开发
  code: '开发', cursor: '开发', idea: '开发', idea64: '开发', webstorm64: '开发', pycharm64: '开发',
  goland64: '开发', rubymine64: '开发', clion64: '开发', rider: '开发', datagrip64: '开发',
  studio64: '开发', devenv: '开发', subl: '开发', sublime_text: '开发', hx: '开发', hm: '开发',
  windowsterminal: '开发', cmd: '开发', powershell: '开发', pwsh: '开发', conhost: '开发',
  wezterm: '开发', alacritty: '开发', hyper: '开发', warp: '开发', mintty: '开发', putty: '开发',
  'windsurf': '开发', 'trae': '开发', 'rssfs': '开发',
  // 浏览器
  chrome: '浏览器', msedge: '浏览器', firefox: '浏览器', brave: '浏览器', opera: '浏览器',
  vivaldi: '浏览器', arc: '浏览器', '360se': '浏览器', '360chrome': '浏览器', 'qqbrowser': '浏览器',
  '搜狗浏览器': '浏览器', liebao: '浏览器', maxthon: '浏览器',
  // 即时通讯
  wechat: '即时通讯', weixin: '即时通讯', wechatappex: '即时通讯', wechatstore: '即时通讯',
  qq: '即时通讯', tim: '即时通讯', dingtalk: '即时通讯', feishu: '即时通讯', lark: '即时通讯',
  telegram: '即时通讯', slack: '即时通讯', discord: '即时通讯', wexin: '即时通讯',
  wxwork: '即时通讯', teams: '会议', msteams: '会议',
  // 办公文档
  winword: '办公文档', excel: '办公文档', powerpnt: '办公文档', onenote: '办公文档',
  typora: '办公文档', obsidian: '办公文档', notion: '办公文档', wps: '办公文档',
  et: '办公文档', wpp: '办公文档', acrord32: '办公文档', 'acrobat': '办公文档', yuque: '办公文档',
  // 会议
  zoom: '会议', wemeetapp: '会议', 'wemeet': '会议', dyta: '会议', teamshare_x: '会议',
  voovmeeting: '会议', 'feishu meetings': '会议', webex: '会议',
  // 设计
  photoshop: '设计', illustrator: '设计', figma: '设计', xd: '设计', blender: '设计',
  afterfx: '设计', premiere: '设计', 'premiere pro': '设计', 'sketch': '设计', canva: '设计', axe: '设计', 'mastergo': '设计', 'jsdesign': '设计', '即时设计': '设计',
  // 娱乐
  steam: '娱乐', spotify: '娱乐', cloudmusic: '娱乐', qqmusic: '娱乐', kugou: '娱乐',
  kwmusic: '娱乐', bilibili: '娱乐', gamebar: '娱乐', 'mpc-hc64': '娱乐', potplayer: '娱乐', 'potplayermini64': '娱乐',
  // 工具
  'totalcmd64': '工具', everything: '工具', listary: '工具', snipaste: '工具', picpick: '工具',
  bandizip: '工具', '7zfm': '工具', winrar: '工具', 'notepad++': '工具', 'notepad++64': '工具',
  calc: '工具', calculatorapp: '工具', 'systemsettings': '系统', mspaint: '工具', gimp: '工具',
  // 系统
  explorer: '系统', applicationframehost: '系统', searchhost: '系统', textinputhost: '系统',
  shellexperiencehost: '系统', startmenuexperiencehost: '系统', dwm: '系统', logonui: '系统',
  'taskmgr': '工具', regedit: '系统', sihost: '系统', ctfmon: '系统', backgroundtaskhost: '系统',
};

// Title patterns (lowercase substring match) refined by category, used when
// the process itself is a browser/host but the content tells more.
const TITLE_MEETING_PATTERNS = [
  '会议', '腾讯会议', 'zoom meeting', 'meeting', '视频会议', '线上会议', 'webex', 'google meet',
];

const TITLE_DEV_PATTERNS = ['vscode', 'visual studio code', '— vscode', '.git', 'git ', 'terminal', 'powershell', 'cmd.exe'];
const TITLE_DOC_PATTERNS = ['.docx', '.xlsx', '.pptx', '.doc', '.xls', '.ppt', 'word', 'excel', 'powerpoint', 'typora', 'obsidian', 'notion', '语雀', '飞书文档'];
const TITLE_ENT_PATTERNS = ['bilibili', '哔哩哔哩', 'youtube', '哔', '网易云', 'spotify', 'steam', '斗鱼', '虎牙', 'twitch', '爱奇艺', '腾讯视频', '优酷', '芒果tv', 'netflix'];

function normalizeProc(name) {
  return String(name || '').toLowerCase().replace(/\.exe$/, '').trim();
}

function categorize(procName, appLabel, title) {
  const proc = normalizeProc(procName);
  if (PROC_MAP[proc]) return PROC_MAP[proc];
  const hay = `${appLabel || ''} ${title || ''}`.toLowerCase();
  if (TITLE_MEETING_PATTERNS.some((p) => hay.includes(p))) {
    // A meeting title inside a browser/Feishu still counts as meeting time
    if (['浏览器', '即时通讯'].includes(PROC_MAP[proc] || '') || true) {
      // only reclassify browser/IM hosts, keep others as-is below
      const base = categorizeByLabel(appLabel);
      if (base === '浏览器' || base === '即时通讯' || base === '其他') return '会议';
    }
  }
  const byLabel = categorizeByLabel(appLabel);
  if (byLabel) return byLabel;
  if (TITLE_DEV_PATTERNS.some((p) => hay.includes(p))) return '开发';
  if (TITLE_DOC_PATTERNS.some((p) => hay.includes(p))) return '办公文档';
  if (TITLE_ENT_PATTERNS.some((p) => hay.includes(p))) return '娱乐';
  return '其他';
}

function categorizeByLabel(label) {
  const l = String(label || '').toLowerCase();
  if (!l) return null;
  if (/code|visual studio|webstorm|pycharm|goland|idea|terminal|powershell/.test(l)) return '开发';
  if (/chrome|edge|firefox|browser|浏览器/.test(l)) return '浏览器';
  if (/微信|wechat|飞书|feishu|lark|qq|dingtalk|钉钉|telegram|slack|discord/.test(l)) return '即时通讯';
  if (/word|excel|powerpoint|typora|obsidian|notion|wps|语雀/.test(l)) return '办公文档';
  if (/zoom|会议|meeting|teams/.test(l)) return '会议';
  if (/photoshop|illustrator|figma|blender|premiere|after effects|设计/.test(l)) return '设计';
  if (/spotify|网易云音乐|qq音乐|bilibil|哔哩哔哩|steam|视频/.test(l)) return '娱乐';
  if (/explorer|设置|文件资源管理器|任务管理器/.test(l)) return '系统';
  return null;
}

function isMeeting(procName, appLabel, title) {
  const proc = normalizeProc(procName);
  if (['zoom', 'wemeetapp', 'wemeet', 'dyta', 'teamshare_x', 'voovmeeting', 'webex', 'teams', 'msteams'].includes(proc)) return true;
  const hay = `${appLabel || ''} ${title || ''}`.toLowerCase();
  return TITLE_MEETING_PATTERNS.some((p) => hay.includes(p));
}

module.exports = { CATEGORIES, CATEGORY_COLORS, categorize, isMeeting, normalizeProc };
