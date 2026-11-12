'use strict';
// TraceBit Analyzer — builds a compact, redacted skeleton of one day and asks
// an OpenAI-compatible LLM for a behavior report with effort-saving advice.
const fs = require('fs');
const path = require('path');
const { categorize, isMeeting, CATEGORY_COLORS } = require('./categories');

const WEEKDAYS = ['周日', '周一', '周二', '周三', '周四', '周五', '周六'];

function fmtMinutes(ms) {
  const m = Math.round(ms / 60000);
  if (m < 60) return `${m}分钟`;
  const h = Math.floor(m / 60);
  const r = m % 60;
  return r ? `${h}小时${r}分` : `${h}小时`;
}

function hhmm(ts) {
  const d = new Date(ts);
  const p = (n) => String(n).padStart(2, '0');
  return `${p(d.getHours())}:${p(d.getMinutes())}`;
}

// Redact obviously sensitive bits before anything leaves the machine.
function redactText(s) {
  let t = String(s || '');
  t = t.replace(/[\w.+-]+@[\w-]+\.[\w.]+/g, '[邮箱]');
  t = t.replace(/\b(?:sk-[A-Za-z0-9_-]{8,}|ghp_[A-Za-z0-9]{8,}|AKIA[0-9A-Z]{12,})\b/g, '[密钥]');
  t = t.replace(/\b\d{16,19}\b/g, '[号码]');
  if (t.length > 120) t = t.slice(0, 120) + '…';
  return t;
}

// Group similar titles ("a.js - proj - Code" vs "b.js - proj - Code") by
// trimming volatile tails so the LLM sees patterns, not noise.
function summarizeTitles(titleMap, max = 5) {
  const entries = [...titleMap.entries()]
    .filter(([t]) => t && t.trim())
    .sort((a, b) => b[1] - a[1]);
  const groups = new Map(); // prefix -> {count, sample}
  for (const [t, c] of entries) {
    let key = t.split(/[-–—|·»]/)[0].trim() || t.trim();
    if (key.length > 40) key = key.slice(0, 40);
    if (!groups.has(key)) groups.set(key, { count: 0, sample: t });
    groups.get(key).count += c;
  }
  return [...groups.values()]
    .sort((a, b) => b.count - a.count)
    .slice(0, max)
    .map((g) => ({ title: redactText(g.sample), occurrences: g.count }));
}

function buildSkeleton(stats, settings) {
  const d = new Date(stats.dateKey + 'T00:00:00');
  const weekday = WEEKDAYS[d.getDay()];

  const categories = new Map();
  const apps = stats.apps
    .map((a) => ({
      app: redactText(a.app || a.name),
      proc: a.name,
      category: categorize(a.name, a.app, [...a.titles.keys()][0] || ''),
      ms: a.ms,
      switches: a.switches,
      titles: summarizeTitles(a.titles, 3),
    }))
    .sort((a, b) => b.ms - a.ms);
  for (const a of apps) {
    categories.set(a.category, (categories.get(a.category) || 0) + a.ms);
  }
  const catList = [...categories.entries()]
    .map(([name, ms]) => ({ name, minutes: Math.round(ms / 60000), pct: Math.round((ms / Math.max(1, stats.totalActiveMs)) * 100) }))
    .sort((a, b) => b.minutes - a.minutes);

  const meetings = { minutes: 0, count: 0, items: [] };
  const seenMeeting = new Map();
  for (const s of stats.stays) {
    if (isMeeting(s.name, s.app, s.title) && s.end - s.start >= 60000) {
      const key = s.name || s.app;
      const item = seenMeeting.get(key) || { app: s.app || s.name, minutes: 0, sample: redactText(s.title) };
      item.minutes += Math.round((s.end - s.start) / 60000);
      seenMeeting.set(key, item);
    }
  }
  meetings.items = [...seenMeeting.values()].sort((a, b) => b.minutes - a.minutes);
  meetings.count = meetings.items.length;
  meetings.minutes = meetings.items.reduce((acc, i) => acc + i.minutes, 0);

  const activeHours = stats.totalActiveMs / 3600000;

  return {
    date: stats.dateKey,
    weekday,
    firstActivity: stats.firstActive ? hhmm(stats.firstActive) : null,
    lastActivity: stats.lastActive ? hhmm(stats.lastActive) : null,
    totalActiveMinutes: Math.round(stats.totalActiveMs / 60000),
    sessionCount: stats.sessions.length,
    categories: catList,
    apps: apps.slice(0, 14).map((a) => ({
      app: a.app, category: a.category, minutes: Math.round(a.ms / 60000),
      switches: a.switches, topTitles: a.titles,
    })),
    hourlyActiveMinutes: stats.hourly.map((m) => Math.round(m / 60000)),
    contextSwitches: stats.switches,
    switchesPerActiveHour: activeHours > 0.1 ? Math.round((stats.switches / activeHours) * 10) / 10 : null,
    longestFocusBlock: stats.longestBlock ? {
      app: redactText(stats.longestBlock.app),
      minutes: Math.round(stats.longestBlock.ms / 60000),
      startedAt: hhmm(stats.longestBlock.startedAt),
    } : null,
    meetings,
    longIdleBreaks: stats.longIdles,
    longestIdleMinutes: Math.round(stats.longestIdleMs / 60000),
    screenshotCount: stats.screenshots.length,
    notes: '所有标题已截断并脱敏；时间为本地时间。',
  };
}

function skeletonMarkdown(skel) {
  const lines = [];
  lines.push(`日期：${skel.date}（${skel.weekday}）`);
  lines.push(`首次活动：${skel.firstActivity ?? '无'}　最后活动：${skel.lastActivity ?? '无'}`);
  lines.push(`总活跃时长：${skel.totalActiveMinutes} 分钟，活动段数：${skel.sessionCount}`);
  lines.push(`应用切换次数：${skel.contextSwitches}${skel.switchesPerActiveHour != null ? `（每小时 ${skel.switchesPerActiveHour} 次）` : ''}`);
  if (skel.longestFocusBlock) {
    lines.push(`最长专注块：${skel.longestFocusBlock.app} ${skel.longestFocusBlock.minutes} 分钟（始于 ${skel.longestFocusBlock.startedAt}）`);
  }
  lines.push('');
  lines.push('分类时长（分钟 / 占比）：');
  for (const c of skel.categories) lines.push(`- ${c.name}: ${c.minutes} 分钟（${c.pct}%）`);
  lines.push('');
  lines.push('应用明细（分钟）：');
  for (const a of skel.apps) {
    lines.push(`- [${a.category}] ${a.app} — ${a.minutes} 分钟，切换 ${a.switches} 次`);
    for (const t of a.topTitles) lines.push(`    · "${t.title}" ×${t.occurrences}`);
  }
  lines.push('');
  if (skel.meetings.count > 0) {
    lines.push(`会议：共 ${skel.meetings.minutes} 分钟`);
    for (const m of skel.meetings.items) lines.push(`- ${m.app} ${m.minutes} 分钟（如 "${m.sample}"）`);
  } else {
    lines.push('会议：无');
  }
  lines.push('');
  lines.push('每小时活跃分钟（0-23 点）：' + skel.hourlyActiveMinutes.join(','));
  lines.push(`长休息（≥5分钟）次数：${skel.longIdleBreaks}，最长休息：${skel.longestIdleMinutes} 分钟`);
  return lines.join('\n');
}

const SYSTEM_PROMPT = `你是一名注重实证的个人效率与行为分析教练。用户会给你一份由本地软件记录的、某一天电脑使用行为的结构化数据（已脱敏）。

你的任务：写一份「行为日报」，帮助用户理解自己的一天，并给出具体、可执行的省力优化建议。

硬性规则：
- 只依据记录中的事实。不得编造任何数字、应用名、时间、事件。数据里没有的就不要写。
- 语言：简体中文。语气简洁、专业、克制，不堆砌形容词，不使用 emoji。
- 直接输出 Markdown 正文，不要代码块包裹，不要开场白和结尾客套。

输出结构（严格遵守）：

# {日期} 行为日报

## 一、今日概览
3-5 句话：活跃时长、主要时间去向、一天的整体节奏（上午/下午/晚间分别做了什么类型的事）。

## 二、时间去哪儿了
- 用列表列出各分类的时长与占比（只列数据中存在的分类）。
- 指出 1-2 个最突出的时间消耗点，引用具体数字。

## 三、行为模式观察
3-5 条基于数据的观察，例如：切换频率与碎片化、长时间会议、深夜使用、某个应用反复进出、专注块长度等。每条必须引用具体数字（时长、次数、时间点）。

## 四、省力优化建议
3-6 条建议，按预期收益从高到低排序。每条格式：

**建议标题**
- 现象：（引用数据中的具体事实）
- 做法：（今天就可以执行的具体动作，不要空话）
- 预期收益：（估算能省下的时间或精力，说明依据）

要求：建议必须针对记录中真实出现的模式。禁止没有任何数据支撑的泛泛建议（如“多休息”“做好时间管理”）。如果数据中存在明显的重复性操作（如反复切换某两个应用、频繁进出同一网站/工具），优先给出消除或自动化该重复的具体方案。

## 五、明日一个实验
给出一个 24 小时内可验证的小实验（具体做法 + 如何判断有效），用于验证上面某条建议。`;

async function chatCompletion(ai, messages, { maxTokens = 2048, timeoutMs = 120000 } = {}) {
  let base = String(ai.baseUrl || '').trim().replace(/\/+$/, '');
  if (!base) throw new Error('未配置 AI 接口地址');
  if (!/\/v\d+(\/|$)/.test(base) && !base.endsWith('/chat/completions')) {
    base = base + '/v1';
  }
  const url = base.endsWith('/chat/completions') ? base : base + '/chat/completions';
  if (!ai.model) throw new Error('未配置模型名称');

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const res = await fetch(url, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        ...(ai.apiKey ? { Authorization: `Bearer ${ai.apiKey}` } : {}),
      },
      body: JSON.stringify({
        model: ai.model,
        messages,
        temperature: 0.3,
        max_tokens: maxTokens,
      }),
      signal: controller.signal,
    });
    if (!res.ok) {
      const text = await res.text().catch(() => '');
      throw new Error(`AI 接口返回 ${res.status}：${text.slice(0, 200)}`);
    }
    const data = await res.json();
    const content = data?.choices?.[0]?.message?.content;
    if (!content) throw new Error('AI 返回内容为空');
    return { text: String(content).trim(), model: data.model || ai.model };
  } finally {
    clearTimeout(timer);
  }
}

class Analyzer {
  constructor({ settings, store, reportsDir }) {
    this.settings = settings;
    this.store = store;
    this.reportsDir = reportsDir;
    fs.mkdirSync(reportsDir, { recursive: true });
  }

  buildSkeleton(dateKey) {
    const stats = this.store.dayStats(dateKey, {
      idleThresholdMs: this.settings.get().recording.idleThresholdMs,
      categorize,
    });
    return { stats, skeleton: buildSkeleton(stats, this.settings.get()) };
  }

  reportFile(dateKey) { return path.join(this.reportsDir, `${dateKey}.json`); }

  getReport(dateKey) {
    try {
      return JSON.parse(fs.readFileSync(this.reportFile(dateKey), 'utf-8'));
    } catch { return null; }
  }

  listReports() {
    const out = [];
    try {
      for (const f of fs.readdirSync(this.reportsDir)) {
        const m = /^(\d{4}-\d{2}-\d{2})\.json$/.exec(f);
        if (!m) continue;
        try {
          const r = JSON.parse(fs.readFileSync(path.join(this.reportsDir, f), 'utf-8'));
          out.push({ dateKey: m[1], generatedAt: r.generatedAt, model: r.model, activeMinutes: r.activeMinutes });
        } catch {}
      }
    } catch {}
    return out.sort((a, b) => (a.dateKey < b.dateKey ? 1 : -1));
  }

  async generateReport(dateKey) {
    const ai = this.settings.get().ai;
    const { stats, skeleton } = this.buildSkeleton(dateKey);
    if (!stats.events) throw new Error('当天没有记录数据');
    if (stats.totalActiveMs < 60000) throw new Error('当天活跃时长不足 1 分钟，无法生成有意义的报告');

    const userPrompt = `以下是用户 ${dateKey} 的电脑使用行为记录（本地记录、已脱敏）：

${skeletonMarkdown(skeleton)}

请依据以上数据写出这一天的行为日报。`;

    const { text, model } = await chatCompletion(ai, [
      { role: 'system', content: SYSTEM_PROMPT },
      { role: 'user', content: userPrompt },
    ]);

    const report = {
      dateKey,
      markdown: text,
      model,
      generatedAt: Date.now(),
      activeMinutes: Math.round(stats.totalActiveMs / 60000),
      switchCount: stats.switches,
      appCount: stats.apps.length,
    };
    fs.writeFileSync(this.reportFile(dateKey), JSON.stringify(report, null, 2), 'utf-8');
    return report;
  }

  async testConnection() {
    const ai = this.settings.get().ai;
    const { text, model } = await chatCompletion(ai, [
      { role: 'user', content: '请只回复两个字：正常' },
    ], { maxTokens: 16, timeoutMs: 30000 });
    return { ok: true, model, reply: text.slice(0, 40) };
  }
}

module.exports = { Analyzer, buildSkeleton, redactText, chatCompletion, skeletonMarkdown };
