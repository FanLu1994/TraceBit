'use strict';
// Optional web search for resource recommendations. Callers pass only static,
// abstract queries. Search failures never block local report generation.

const DEFAULT_TIMEOUT_MS = 8000;
const MAX_RESULTS = 5;

function cleanText(value, max = 180) {
  return String(value || '')
    .replace(/<[^>]*>/g, ' ')
    .replace(/[\[\]()*_`#|<>\\]/g, ' ')
    .replace(/[\u0000-\u001f\u007f]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, max);
}

function decodeHtml(value) {
  const named = { amp: '&', quot: '"', apos: "'", lt: '<', gt: '>', nbsp: ' ' };
  return String(value || '').replace(/&(#x[\da-f]+|#\d+|amp|quot|apos|lt|gt|nbsp);/gi, (match, entity) => {
    if (entity[0] === '#') {
      const hex = entity[1]?.toLowerCase() === 'x';
      const code = parseInt(entity.slice(hex ? 2 : 1), hex ? 16 : 10);
      return Number.isFinite(code) ? String.fromCodePoint(Math.min(code, 0x10ffff)) : match;
    }
    return named[entity.toLowerCase()] || match;
  });
}

function normalizeHttpUrl(value) {
  try {
    const raw = decodeHtml(String(value || '').trim());
    if (!raw) return '';
    const url = new URL(raw, 'https://duckduckgo.com');
    const redirected = url.searchParams.get('uddg');
    const target = redirected ? new URL(redirected) : url;
    if (target.protocol !== 'https:' && target.protocol !== 'http:') return '';
    target.username = '';
    target.password = '';
    target.hash = '';
    for (const key of [...target.searchParams.keys()]) {
      if (/^utm_/i.test(key)) target.searchParams.delete(key);
    }
    if (target.hostname === 'github.com') target.search = '';
    return target.toString();
  } catch {
    return '';
  }
}

function resourceKey(value) {
  try {
    const url = new URL(value);
    const host = url.hostname.toLowerCase();
    const parts = url.pathname.split('/').filter(Boolean);
    if (host === 'github.com' && parts.length >= 2) {
      return `${host}/${parts[0].toLowerCase()}/${parts[1].toLowerCase().replace(/\.git$/, '')}`;
    }
    return `${host}${url.pathname}`.replace(/\/$/, '').toLowerCase();
  } catch {
    return String(value || '').replace(/\/$/, '').toLowerCase();
  }
}

function allowedDiscovery(value) {
  try {
    const url = new URL(value);
    if (url.protocol !== 'https:') return false;
    const host = url.hostname.toLowerCase();
    if (host === 'modelcontextprotocol.io' || host.endsWith('.github.io')) return true;
    if (host !== 'github.com') return false;
    const parts = url.pathname.split('/').filter(Boolean);
    if (parts.length < 2) return false;
    return !['search', 'topics', 'login', 'join', 'features', 'marketplace', 'orgs', 'collections'].includes(parts[0].toLowerCase());
  } catch {
    return false;
  }
}

async function fetchWithTimeout(url, options = {}, timeoutMs = DEFAULT_TIMEOUT_MS) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetch(url, { ...options, signal: controller.signal, redirect: 'follow' });
  } finally {
    clearTimeout(timer);
  }
}

function assertAbstractQuery(query) {
  const q = String(query || '').trim();
  if (!q || q.length > 120 || /https?:\/\/|www\.|(?:[a-z0-9-]+\.)+[a-z]{2,}|[@\r\n]/i.test(q)) {
    throw new Error('搜索关键词不符合抽象资源查询约束');
  }
  return q;
}

function parseDuckDuckGo(html) {
  const results = [];
  const anchors = String(html || '').match(/<a\b[^>]*>[\s\S]*?<\/a>/gi) || [];
  for (const anchor of anchors) {
    const opening = /^<a\b[^>]*>/i.exec(anchor)?.[0] || '';
    if (!/class\s*=\s*["'][^"']*result__a/i.test(opening)) continue;
    const href = /href\s*=\s*["']([^"']+)["']/i.exec(opening)?.[1];
    const url = normalizeHttpUrl(href);
    const title = cleanText(decodeHtml(anchor.replace(/^<a\b[^>]*>/i, '').replace(/<\/a>$/i, '')), 100);
    if (!url || !title) continue;
    results.push({ title, url, description: '' });
    if (results.length >= MAX_RESULTS) break;
  }
  return results;
}

async function searchWeb(query, options = {}) {
  const q = assertAbstractQuery(query);
  const provider = options.provider || 'duckduckgo';
  const timeoutMs = options.timeoutMs || DEFAULT_TIMEOUT_MS;
  if (provider === 'brave') {
    const apiKey = String(options.searchApiKey || '').trim();
    if (!apiKey) throw new Error('Brave Search 需要单独配置搜索 API Key');
    const endpoint = options.searchEndpoint || 'https://api.search.brave.com/res/v1/web/search';
    const url = new URL(endpoint);
    url.searchParams.set('q', q);
    url.searchParams.set('count', String(MAX_RESULTS));
    const response = await fetchWithTimeout(url, {
      headers: { Accept: 'application/json', 'X-Subscription-Token': apiKey },
    }, timeoutMs);
    if (!response.ok) throw new Error(`Brave Search 返回 ${response.status}`);
    const data = await response.json();
    return (data.web?.results || []).slice(0, MAX_RESULTS).map((item) => ({
      title: cleanText(item.title, 100),
      url: normalizeHttpUrl(item.url),
      description: cleanText(item.description, 180),
    })).filter((item) => item.title && item.url);
  }

  if (provider !== 'duckduckgo') throw new Error(`不支持的搜索服务：${provider}`);
  const endpoint = options.searchEndpoint || 'https://html.duckduckgo.com/html/';
  const url = new URL(endpoint);
  url.searchParams.set('q', q);
  const response = await fetchWithTimeout(url, {
    headers: { Accept: 'text/html', 'User-Agent': 'TraceBit/0.2 (local resource search)' },
  }, timeoutMs);
  if (!response.ok) throw new Error(`DuckDuckGo 返回 ${response.status}`);
  const html = await response.text();
  return parseDuckDuckGo(html.slice(0, 1500000));
}

async function verifyResource(resource) {
  const response = await fetchWithTimeout(resource.url, {
    method: 'HEAD',
    headers: { 'User-Agent': 'TraceBit/0.2 (local link check)' },
  }, 6000);
  try { await response.body?.cancel(); } catch {}
  if (!response.ok) {
    resource.verification = `链接返回 ${response.status}，请手动检查`;
    return false;
  }
  resource.verifiedAt = Date.now();
  resource.verification = resource.source === 'curated'
    ? '官方来源可访问'
    : '搜索链接可访问，仍需人工审核';
  return true;
}

async function enrichRecommendations(localRecommendations, ai = {}, dependencies = {}) {
  const recommendations = localRecommendations.map((item) => ({
    ...item,
    resources: item.resources.map((resource) => ({ ...resource })),
  }));
  const search = {
    enabled: ai.webSearch === true,
    provider: ai.searchProvider || 'duckduckgo',
    queries: 0,
    added: 0,
    verified: 0,
    errors: [],
  };
  if (!search.enabled || !recommendations.length) return { recommendations, search };

  const searchImpl = dependencies.searchImpl || searchWeb;
  await Promise.all(recommendations.map(async (opportunity) => {
    search.queries++;
    try {
      const results = await searchImpl(opportunity.searchQuery, {
        provider: search.provider,
        searchApiKey: ai.searchApiKey || '',
        searchEndpoint: ai.searchEndpoint || '',
        timeoutMs: 8000,
      });
      const existing = new Set(opportunity.resources.map((resource) => resourceKey(resource.url)));
      const candidate = results.find((item) => allowedDiscovery(item.url) && !existing.has(resourceKey(item.url)));
      if (!candidate) return;
      opportunity.resources.push({
        id: `web-${opportunity.id}`,
        name: cleanText(candidate.title, 80),
        type: '联网候选',
        url: candidate.url,
        description: cleanText(candidate.description, 180) || '联网搜索发现的候选资源，需要人工核对与上述操作的匹配度。',
        benefit: '补充本地精选之外的候选，不保证适用。',
        setupCost: '待评估',
        permissions: '只访问公开页面；安装前审阅源码、权限、发布方和维护状态。',
        source: 'web',
        verification: '联网搜索发现，尚未人工复核',
        verifiedAt: null,
      });
      search.added++;
    } catch (error) {
      search.errors.push(cleanText(error.message, 160) || '搜索失败');
    }
  }));

  if (ai.verifySources !== false) {
    const unique = new Map();
    for (const opportunity of recommendations) {
      for (const resource of opportunity.resources) {
        const key = resourceKey(resource.url);
        if (!unique.has(key)) unique.set(key, []);
        unique.get(key).push(resource);
      }
    }
    await Promise.all([...unique.values()].map(async (copies) => {
      try {
        const verified = await verifyResource(copies[0]);
        for (let i = 1; i < copies.length; i++) {
          copies[i].verification = copies[0].verification;
          copies[i].verifiedAt = copies[0].verifiedAt;
        }
        if (verified) search.verified += copies.length;
      } catch {
        for (const resource of copies) resource.verification = '链接暂时无法验证，请手动检查';
      }
    }));
  }
  return { recommendations, search };
}

module.exports = {
  searchWeb,
  enrichRecommendations,
  parseDuckDuckGo,
  normalizeHttpUrl,
  resourceKey,
  allowedDiscovery,
  assertAbstractQuery,
};
