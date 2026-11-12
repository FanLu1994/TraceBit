'use strict';
// Evidence-based tool recommendations. Detection stays local and never uses
// websites, snapshots, or raw window titles as search input.
const { isMeeting, normalizeProc } = require('./categories');

const MINUTE = 60000;

const RESOURCE_CATALOG = {
  just: {
    name: 'just', type: '开源项目', url: 'https://github.com/casey/just',
    description: '把重复的项目命令整理成带名称的任务入口。',
    benefit: '减少记忆命令和在编辑器、终端之间往返的成本。',
    setupCost: '低，通常先整理 3 到 5 条常用命令。',
    permissions: '命令文件会执行本机命令，使用前先审阅任务内容。',
  },
  vscodeTasks: {
    name: 'VS Code Tasks', type: '内置自动化', url: 'https://code.visualstudio.com/docs/editor/tasks',
    description: '把构建、测试和常用脚本留在编辑器内运行。',
    benefit: '减少为了固定命令而切换到终端的次数。',
    setupCost: '低，可从一条最常用命令开始。',
    permissions: '任务会执行本机命令，不要运行来源不明的任务配置。',
  },
  lazygit: {
    name: 'lazygit', type: '开源项目', url: 'https://github.com/jesseduffield/lazygit',
    description: '在终端内集中处理提交、分支、暂存和变基等 Git 操作。',
    benefit: '适合 Git 操作频繁时减少命令重复和上下文切换。',
    setupCost: '中，需要熟悉基本 Git 概念和少量快捷键。',
    permissions: '会读写 Git 仓库，破坏性操作前仍需确认和备份。',
  },
  fetchMcp: {
    name: 'Fetch MCP Server', type: 'MCP', url: 'https://github.com/modelcontextprotocol/servers/tree/main/src/fetch',
    description: '让兼容的 Agent 在用户请求下读取网页并提取内容。',
    benefit: '减少复制网页正文到笔记或对话中的手工步骤。',
    setupCost: '中，需要支持 MCP 的客户端和基础配置。',
    permissions: '会产生联网请求；不要抓取未授权或敏感页面，启用前核对服务器来源。',
  },
  agentSkills: {
    name: '阅读研究 Skill 工作流', type: 'Skill', url: 'https://github.com/anthropics/skills',
    description: '参考官方 Skill 结构，把资料读取、来源整理和摘要写成可复用流程。',
    benefit: '适合反复进行资料收集和笔记整理的场景。',
    setupCost: '中，需要先定义输入、输出和引用要求。',
    permissions: 'Skill 能驱动宿主工具，安装前应审阅指令和允许调用的能力。',
  },
  searxng: {
    name: 'SearXNG', type: '开源项目', url: 'https://github.com/searxng/searxng',
    description: '可自托管的元搜索服务，为研究工作流提供统一搜索入口。',
    benefit: '适合需要控制搜索数据流向的高级用户。',
    setupCost: '高，需要部署和维护服务。',
    permissions: '上游搜索仍可能看到查询词，部署前检查日志和代理配置。',
  },
  whispercpp: {
    name: 'whisper.cpp', type: '开源项目', url: 'https://github.com/ggml-org/whisper.cpp',
    description: '在本机把会议录音转写成文本，再接续摘要流程。',
    benefit: '减少手工整理逐字稿的工作量，音频不必交给云端转写服务。',
    setupCost: '中，需要准备模型并确认本机性能。',
    permissions: '录音前必须取得参与者同意，音频和转写稿应按敏感资料管理。',
  },
  meetingSkill: {
    name: '会议总结 Skill 工作流', type: 'Skill', url: 'https://github.com/anthropics/skills',
    description: '参考官方 Skill 结构，把转写稿整理成决议、行动项和负责人。',
    benefit: '把重复的报告格式和检查项固化下来。',
    setupCost: '中，需要先用一份已核对纪要校准模板。',
    permissions: '转写稿可能含敏感信息，优先本地处理并限制工具权限。',
  },
  powerToys: {
    name: 'Microsoft PowerToys', type: '开源项目', url: 'https://github.com/microsoft/PowerToys',
    description: 'PowerRename 等工具可处理批量重命名和常见 Windows 操作。',
    benefit: '减少重复的文件命名和窗口整理动作。',
    setupCost: '低，可先只启用一个实际需要的模块。',
    permissions: '批量修改前先备份并小范围试跑，避免误改文件名。',
  },
  autohotkey: {
    name: 'AutoHotkey', type: '开源项目', url: 'https://github.com/AutoHotkey/AutoHotkey',
    description: '将明确、重复的 Windows 操作写成可审阅的自动化脚本。',
    benefit: '适合固定流程，不适合尚未弄清规则的一次性操作。',
    setupCost: '中到高，需要编写、测试并维护脚本。',
    permissions: '脚本可模拟输入和操作文件，只运行自己理解且来源可信的脚本。',
  },
  bruno: {
    name: 'Bruno', type: '开源项目', url: 'https://github.com/usebruno/bruno',
    description: '把 API 请求、环境和集合格式保存在本地项目中。',
    benefit: '减少重复填写请求和在不同窗口之间复制参数。',
    setupCost: '低到中，可先迁移最常用的一组请求。',
    permissions: '环境文件可能含令牌，提交仓库前检查密钥和敏感地址。',
  },
  hoppscotch: {
    name: 'Hoppscotch', type: '开源项目', url: 'https://github.com/hoppscotch/hoppscotch',
    description: '用于组织、发送和协作管理 API 请求。',
    benefit: '适合需要复用请求集合和统一调试入口的场景。',
    setupCost: '低到中，可从单个项目开始。',
    permissions: '请求会接触接口和凭据，自托管或云同步前确认数据流向。',
  },
};

const EDITORS = new Set(['code', 'cursor', 'idea', 'idea64', 'webstorm64', 'pycharm64', 'goland64', 'rubymine64', 'clion64', 'rider', 'datagrip64', 'studio64', 'devenv', 'subl', 'sublime_text', 'windsurf', 'trae']);
const TERMINALS = new Set(['windowsterminal', 'cmd', 'powershell', 'pwsh', 'conhost', 'wezterm', 'alacritty', 'hyper', 'warp', 'mintty', 'putty']);
const BROWSERS = new Set(['chrome', 'msedge', 'firefox', 'brave', 'opera', 'vivaldi', 'arc', '360se', '360chrome', 'qqbrowser']);
const DOCUMENTS = new Set(['winword', 'excel', 'powerpnt', 'onenote', 'typora', 'obsidian', 'notion', 'wps', 'et', 'wpp', 'acrord32', 'acrobat', 'yuque']);
const FILE_MANAGERS = new Set(['explorer', 'totalcmd64', 'doublecmd', 'q-dir']);
const API_APPS = new Set(['postman', 'insomnia', 'bruno', 'hoppscotch']);

function proc(stay) { return normalizeProc(stay?.name); }
function inSet(set) { return (stay) => set.has(proc(stay)); }
function sumMs(stays, predicate) {
  return stays.reduce((total, stay) => total + (predicate(stay) ? Math.max(0, stay.end - stay.start) : 0), 0);
}
function crossTransitions(stays, left, right) {
  let count = 0;
  for (let i = 1; i < stays.length; i++) {
    const a = stays[i - 1], b = stays[i];
    if ((left(a) && right(b)) || (right(a) && left(b))) count++;
  }
  return count;
}
function involvingTransitions(stays, predicate) {
  let count = 0;
  for (let i = 1; i < stays.length; i++) {
    if (predicate(stays[i - 1]) || predicate(stays[i])) count++;
  }
  return count;
}
function matchingTitleCount(apps, pattern) {
  let count = 0;
  for (const app of apps) {
    for (const [title, occurrences] of app.titles || []) {
      if (pattern.test(String(title || ''))) count += Math.max(1, occurrences || 1);
    }
  }
  return count;
}
function roundedMinutes(ms) { return Math.round(ms / MINUTE); }
function resources(ids) {
  return ids.map((id) => ({
    id,
    ...RESOURCE_CATALOG[id],
    source: 'curated',
    verification: '内置精选，未联网复核',
    verifiedAt: null,
  }));
}

function buildRecommendations(stats) {
  const stays = Array.isArray(stats?.stays) ? stats.stays : [];
  const apps = Array.isArray(stats?.apps) ? stats.apps : [];
  const isEditor = inSet(EDITORS), isTerminal = inSet(TERMINALS);
  const isBrowser = inSet(BROWSERS), isDocument = inSet(DOCUMENTS);
  const isFileManager = inSet(FILE_MANAGERS), isApiApp = inSet(API_APPS);
  const opportunities = [];

  const editorTerminalSwitches = crossTransitions(stays, isEditor, isTerminal);
  if (editorTerminalSwitches >= 4) {
    const gitEvidence = matchingTitleCount(apps, /(?:^|\W)(?:git|github|gitlab|commit|merge|pull request)(?:\W|$)|提交|合并|分支/i) > 0;
    const ids = gitEvidence ? ['lazygit', 'just', 'vscodeTasks'] : ['just', 'vscodeTasks'];
    const activeMs = sumMs(stays, (stay) => isEditor(stay) || isTerminal(stay));
    opportunities.push({
      id: 'development-loop',
      title: '把开发命令收成固定入口',
      evidence: `编辑器与终端之间切换 ${editorTerminalSwitches} 次，相关应用活跃 ${roundedMinutes(activeMs)} 分钟。`,
      summary: '先固化重复的项目命令；如果切换主要来自 Git 操作，再使用可视化 Git 工具。',
      searchQuery: 'open source command runner Git GUI Windows',
      score: editorTerminalSwitches,
      resources: resources(ids),
    });
  }

  const browserDocumentSwitches = crossTransitions(stays, isBrowser, isDocument);
  if (browserDocumentSwitches >= 4) {
    const activeMs = sumMs(stays, (stay) => isBrowser(stay) || isDocument(stay));
    opportunities.push({
      id: 'research-loop',
      title: '减少资料复制和整理步骤',
      evidence: `浏览器与文档或笔记应用之间切换 ${browserDocumentSwitches} 次，相关应用活跃 ${roundedMinutes(activeMs)} 分钟。`,
      summary: '将网页读取、来源引用和摘要整理做成可复用流程，保留人工核对来源这一步。',
      searchQuery: 'open source research assistant MCP fetch Skill',
      score: browserDocumentSwitches * 1.2,
      resources: resources(['fetchMcp', 'agentSkills', 'searxng']),
    });
  }

  const meetingMs = sumMs(stays, (stay) => isMeeting(stay.name, stay.app, stay.title));
  if (meetingMs >= 30 * MINUTE) {
    opportunities.push({
      id: 'meeting-notes',
      title: '把会议转写和纪要拆开自动化',
      evidence: `会议相关活动共 ${roundedMinutes(meetingMs)} 分钟。`,
      summary: '在参与者同意的前提下先本地转写，再用固定模板生成决议和行动项，并人工复核。',
      searchQuery: 'open source local meeting transcription summary',
      score: roundedMinutes(meetingMs) / 4,
      resources: resources(['whispercpp', 'meetingSkill']),
    });
  }

  const fileMs = sumMs(stays, isFileManager);
  const fileSwitches = involvingTransitions(stays, isFileManager);
  const fileTitleHits = matchingTitleCount(apps, /重命名|批量|整理文件|rename|batch rename|copy files|move files/i);
  if (fileMs >= 15 * MINUTE && (fileSwitches >= 4 || fileTitleHits >= 2)) {
    opportunities.push({
      id: 'file-workflow',
      title: '自动化明确的文件整理动作',
      evidence: `文件管理相关窗口活跃 ${roundedMinutes(fileMs)} 分钟，与其他应用切换 ${fileSwitches} 次。`,
      summary: '批量重命名先用成熟工具；只有规则明确且反复出现的文件动作才值得写成脚本。',
      searchQuery: 'open source Windows batch rename file automation',
      score: fileSwitches + roundedMinutes(fileMs) / 5,
      resources: resources(['powerToys', 'autohotkey']),
    });
  }

  const apiTitleHits = matchingTitleCount(apps, /\b(?:api|openapi|swagger|graphql|postman)\b|接口调试|接口文档/i);
  const apiMs = sumMs(stays, isApiApp);
  if (apiTitleHits >= 2 || apiMs >= 10 * MINUTE) {
    const evidence = apiTitleHits >= 2
      ? `检测到 ${apiTitleHits} 次 API 或接口相关窗口标题。`
      : `API 调试工具活跃 ${roundedMinutes(apiMs)} 分钟。`;
    opportunities.push({
      id: 'api-workflow',
      title: '复用 API 请求和调试环境',
      evidence,
      summary: '把常用请求、环境和示例响应整理成集合并纳入密钥检查，减少重复填写。',
      searchQuery: 'open source API client request collection',
      score: apiTitleHits * 2 + roundedMinutes(apiMs) / 5,
      resources: resources(['bruno', 'hoppscotch']),
    });
  }

  return opportunities
    .sort((a, b) => b.score - a.score)
    .slice(0, 3)
    .map(({ score, ...opportunity }) => opportunity);
}

function recommendationsMarkdown(recommendations, search = {}) {
  if (!recommendations.length) return '';
  const lines = ['## 六、可工具化的操作', ''];
  if (search.enabled) {
    lines.push(`> 本地规则结合联网检索：执行 ${search.queries || 0} 次抽象关键词查询，新增 ${search.added || 0} 个待审核候选，验证 ${search.verified || 0} 个来源链接。搜索服务不会收到窗口标题、网站域名或行为统计。`);
  } else {
    lines.push('> 以下来自本地行为规则和内置精选资源，未进行联网搜索。');
  }
  if (search.errors?.length) {
    lines.push(`> 联网检索提示：${search.errors[0]}。已保留本地精选结果。`);
  }
  lines.push('');

  for (const opportunity of recommendations) {
    lines.push(`### ${opportunity.title}`);
    lines.push(`- 操作依据：${opportunity.evidence}`);
    lines.push(`- 优化方向：${opportunity.summary}`);
    for (const resource of opportunity.resources) {
      const checked = resource.verifiedAt
        ? `，检查于 ${new Date(resource.verifiedAt).toLocaleString('zh-CN', { hour12: false })}`
        : '';
      lines.push(`- [${resource.name}](${resource.url})（${resource.type}）：${resource.description} 预期收益：${resource.benefit} 配置成本：${resource.setupCost} 权限与隐私：${resource.permissions} 来源状态：${resource.verification}${checked}。`);
    }
    lines.push('');
  }
  lines.push('以上资源只供人工评估，TraceBit 不会自动下载、安装或启用。');
  return lines.join('\n');
}

module.exports = { buildRecommendations, recommendationsMarkdown, RESOURCE_CATALOG };
