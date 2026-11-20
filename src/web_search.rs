use crate::{
    i18n::{Lang, fill},
    recommendations::{Recommendation, Resource, SearchSummary},
    settings::AiSettings,
    store::now_ms,
    tr,
};
use anyhow::{Result, bail};
use regex::{Captures, Regex};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, sync::LazyLock, time::Duration};
use url::Url;

const MAX_RESULTS: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct EnrichedRecommendations {
    pub recommendations: Vec<Recommendation>,
    pub search: SearchSummary,
}

fn truncate_chars(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

pub fn clean_text(value: &str, max: usize) -> String {
    static TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^>]*>").unwrap());
    static MARKUP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[\[\]()*_`#|<>\\]"#).unwrap());
    static CONTROL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\x00-\x1f\x7f]").unwrap());
    static SPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
    let value = TAGS.replace_all(value, " ");
    let value = MARKUP.replace_all(&value, " ");
    let value = CONTROL.replace_all(&value, " ");
    truncate_chars(SPACE.replace_all(&value, " ").trim(), max)
}

pub fn decode_html(value: &str) -> String {
    static ENTITY: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)&(#x[\da-f]+|#\d+|amp|quot|apos|lt|gt|nbsp);").unwrap());
    ENTITY
        .replace_all(value, |captures: &Captures| {
            let entity = &captures[1];
            if let Some(number) = entity.strip_prefix('#') {
                let (hex, digits) = if number.to_ascii_lowercase().starts_with('x') {
                    (true, &number[1..])
                } else {
                    (false, number)
                };
                return u32::from_str_radix(digits, if hex { 16 } else { 10 })
                    .ok()
                    .and_then(char::from_u32)
                    .map(|ch| ch.to_string())
                    .unwrap_or_else(|| captures[0].to_owned());
            }
            match entity.to_ascii_lowercase().as_str() {
                "amp" => "&",
                "quot" => "\"",
                "apos" => "'",
                "lt" => "<",
                "gt" => ">",
                "nbsp" => " ",
                _ => &captures[0],
            }
            .to_owned()
        })
        .into_owned()
}

pub fn normalize_http_url(value: &str) -> String {
    let raw = decode_html(value.trim());
    if raw.is_empty() {
        return String::new();
    }
    let base = Url::parse("https://duckduckgo.com").unwrap();
    let Ok(url) = Url::parse(&raw).or_else(|_| base.join(&raw)) else {
        return String::new();
    };
    let redirected = url
        .query_pairs()
        .find(|(key, _)| key == "uddg")
        .map(|(_, value)| value.into_owned());
    let mut target = if let Some(redirected) = redirected {
        let Ok(target) = Url::parse(&redirected) else {
            return String::new();
        };
        target
    } else {
        url
    };
    if !matches!(target.scheme(), "http" | "https") {
        return String::new();
    }
    let _ = target.set_username("");
    let _ = target.set_password(None);
    target.set_fragment(None);
    let query = target
        .query_pairs()
        .filter(|(key, _)| !key.to_ascii_lowercase().starts_with("utm_"))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    target.set_query(None);
    if !query.is_empty() {
        let mut pairs = target.query_pairs_mut();
        for (key, value) in query {
            pairs.append_pair(&key, &value);
        }
    }
    if target
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("github.com"))
    {
        target.set_query(None);
    }
    target.to_string()
}

pub fn resource_key(value: &str) -> String {
    let Ok(url) = Url::parse(value) else {
        return value.trim_end_matches('/').to_lowercase();
    };
    let host = url.host_str().unwrap_or_default().to_lowercase();
    let parts = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if host == "github.com" && parts.len() >= 2 {
        return format!(
            "{}/{}/{}",
            host,
            parts[0].to_lowercase(),
            parts[1].trim_end_matches(".git").to_lowercase()
        );
    }
    format!("{host}{}", url.path())
        .trim_end_matches('/')
        .to_lowercase()
}

pub fn allowed_discovery(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    if url.scheme() != "https" {
        return false;
    }
    let host = url.host_str().unwrap_or_default().to_lowercase();
    if host == "modelcontextprotocol.io" || host.ends_with(".github.io") {
        return true;
    }
    if host != "github.com" {
        return false;
    }
    let parts = url
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() < 2 {
        return false;
    }
    ![
        "search",
        "topics",
        "login",
        "join",
        "features",
        "marketplace",
        "orgs",
        "collections",
    ]
    .contains(&parts[0].to_lowercase().as_str())
}

pub fn assert_abstract_query(query: &str, lang: Lang) -> Result<String> {
    static PRIVATE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)https?://|www\.|(?:[a-z0-9-]+\.)+[a-z]{2,}|[@\r\n]").unwrap()
    });
    let query = query.trim();
    if query.is_empty() || query.len() > 120 || PRIVATE.is_match(query) {
        bail!(tr!(
            lang,
            "搜索关键词不符合抽象资源查询约束",
            "Search keywords must stay an abstract resource query",
            "検索キーワードが抽象的なリソース検索の条件を満たしていません",
            "검색 키워드가 추상 리소스 검색 조건에 맞지 않습니다"
        ));
    }
    Ok(query.to_owned())
}

pub fn parse_duckduckgo(html: &str) -> Vec<SearchResult> {
    static ANCHOR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?is)<a\b[^>]*>[\s\S]*?</a>").unwrap());
    static OPENING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)^<a\b[^>]*>").unwrap());
    static RESULT_CLASS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?i)class\s*=\s*[\"'][^\"']*result__a"#).unwrap());
    static HREF: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?i)href\s*=\s*[\"']([^\"']+)[\"']"#).unwrap());
    static CLOSING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)</a>$").unwrap());
    let mut results = Vec::new();
    for anchor in ANCHOR.find_iter(html).map(|match_| match_.as_str()) {
        let opening = OPENING
            .find(anchor)
            .map(|match_| match_.as_str())
            .unwrap_or_default();
        if !RESULT_CLASS.is_match(opening) {
            continue;
        }
        let href = HREF
            .captures(opening)
            .and_then(|captures| captures.get(1))
            .map(|value| value.as_str())
            .unwrap_or_default();
        let url = normalize_http_url(href);
        let body = OPENING.replace(anchor, "");
        let body = CLOSING.replace(&body, "");
        let title = clean_text(&decode_html(&body), 100);
        if url.is_empty() || title.is_empty() {
            continue;
        }
        results.push(SearchResult {
            title,
            url,
            description: String::new(),
        });
        if results.len() >= MAX_RESULTS {
            break;
        }
    }
    results
}

fn client(timeout_ms: u64) -> Result<Client> {
    Ok(Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .build()?)
}

pub fn search_web(query: &str, ai: &AiSettings, lang: Lang) -> Result<Vec<SearchResult>> {
    let query = assert_abstract_query(query, lang)?;
    let provider = if ai.search_provider.is_empty() {
        "duckduckgo"
    } else {
        &ai.search_provider
    };
    if provider == "brave" {
        let api_key = ai.search_api_key.trim();
        if api_key.is_empty() {
            bail!(tr!(
                lang,
                "Brave Search 需要单独配置搜索 API Key",
                "Brave Search needs its own search API key",
                "Brave Search には専用の検索 API キー設定が必要です",
                "Brave Search에는 별도의 검색 API 키 설정이 필요합니다"
            ));
        }
        let endpoint = ai
            .search_endpoint
            .as_deref()
            .filter(|value| !value.is_empty())
            .unwrap_or("https://api.search.brave.com/res/v1/web/search");
        let response = client(8_000)?
            .get(endpoint)
            .query(&[("q", query), ("count", MAX_RESULTS.to_string())])
            .header("Accept", "application/json")
            .header("X-Subscription-Token", api_key)
            .send()?;
        if !response.status().is_success() {
            bail!(fill(
                tr!(
                    lang,
                    "Brave Search 返回 {}",
                    "Brave Search returned {}",
                    "Brave Search が {} を返しました",
                    "Brave Search가 {}을(를) 반환했습니다"
                ),
                &[&response.status()]
            ));
        }
        let data: Value = response.json()?;
        return Ok(data
            .get("web")
            .and_then(|web| web.get("results"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(MAX_RESULTS)
            .filter_map(|item| {
                let result = SearchResult {
                    title: clean_text(
                        item.get("title")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        100,
                    ),
                    url: normalize_http_url(
                        item.get("url").and_then(Value::as_str).unwrap_or_default(),
                    ),
                    description: clean_text(
                        item.get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        180,
                    ),
                };
                (!result.title.is_empty() && !result.url.is_empty()).then_some(result)
            })
            .collect());
    }
    if provider != "duckduckgo" {
        bail!(fill(
            tr!(
                lang,
                "不支持的搜索服务：{}",
                "Unsupported search provider: {}",
                "未対応の検索サービス：{}",
                "지원하지 않는 검색 서비스: {}"
            ),
            &[&provider]
        ));
    }
    let endpoint = ai
        .search_endpoint
        .as_deref()
        .filter(|value| !value.is_empty())
        .unwrap_or("https://html.duckduckgo.com/html/");
    let response = client(8_000)?
        .get(endpoint)
        .query(&[("q", query)])
        .header("Accept", "text/html")
        .header("User-Agent", "TraceBit/0.3 (local resource search)")
        .send()?;
    if !response.status().is_success() {
        bail!(fill(
            tr!(
                lang,
                "DuckDuckGo 返回 {}",
                "DuckDuckGo returned {}",
                "DuckDuckGo が {} を返しました",
                "DuckDuckGo가 {}을(를) 반환했습니다"
            ),
            &[&response.status()]
        ));
    }
    let html = response.text()?;
    Ok(parse_duckduckgo(&truncate_chars(&html, 1_500_000)))
}

fn verify_resource(resource: &mut Resource, lang: Lang) -> bool {
    let result = client(6_000).and_then(|client| {
        client
            .head(&resource.url)
            .header("User-Agent", "TraceBit/0.3 (local link check)")
            .send()
            .map_err(Into::into)
    });
    match result {
        Ok(response) if response.status().is_success() => {
            resource.verified_at = Some(now_ms());
            resource.verification = if resource.source == "curated" {
                tr!(
                    lang,
                    "官方来源可访问",
                    "Official source is reachable",
                    "公式ソースに接続できます",
                    "공식 출처에 접속할 수 있습니다"
                )
                .to_owned()
            } else {
                tr!(
                    lang,
                    "搜索链接可访问，仍需人工审核",
                    "Search link is reachable but still needs manual review",
                    "検索リンクには接続できますが、人の確認が必要です",
                    "검색 링크에 접속되지만 직접 검토가 필요합니다"
                )
                .to_owned()
            };
            true
        }
        Ok(response) => {
            resource.verification = fill(
                tr!(
                    lang,
                    "链接返回 {}，请手动检查",
                    "Link returned {}; please check manually",
                    "リンクが {} を返しました。手動で確認してください",
                    "링크가 {}을(를) 반환했습니다. 직접 확인하세요"
                ),
                &[&response.status()],
            );
            false
        }
        Err(_) => {
            resource.verification = tr!(
                lang,
                "链接暂时无法验证，请手动检查",
                "Link could not be verified right now; please check manually",
                "現在リンクを検証できません。手動で確認してください",
                "지금은 링크를 검증할 수 없습니다. 직접 확인하세요"
            )
            .to_owned();
            false
        }
    }
}

pub fn enrich_recommendations(
    local: &[Recommendation],
    ai: &AiSettings,
    lang: Lang,
) -> EnrichedRecommendations {
    enrich_with_search(local, ai, lang, |query, ai| search_web(query, ai, lang))
}

pub fn enrich_with_search(
    local: &[Recommendation],
    ai: &AiSettings,
    lang: Lang,
    search_impl: impl Fn(&str, &AiSettings) -> Result<Vec<SearchResult>>,
) -> EnrichedRecommendations {
    let mut recommendations = local.to_vec();
    let mut search = SearchSummary {
        enabled: ai.web_search,
        provider: if ai.search_provider.is_empty() {
            "duckduckgo".to_owned()
        } else {
            ai.search_provider.clone()
        },
        ..Default::default()
    };
    if search.enabled {
        for opportunity in &mut recommendations {
            search.queries += 1;
            match search_impl(&opportunity.search_query, ai) {
                Ok(results) => {
                    let existing = opportunity
                        .resources
                        .iter()
                        .map(|resource| resource_key(&resource.url))
                        .collect::<BTreeSet<_>>();
                    let candidate = results.into_iter().find(|item| {
                        allowed_discovery(&item.url) && !existing.contains(&resource_key(&item.url))
                    });
                    if let Some(candidate) = candidate {
                        opportunity.resources.push(Resource {
                            id: format!("web-{}", opportunity.id),
                            name: clean_text(&candidate.title, 80),
                            kind: tr!(
                                lang,
                                "联网候选",
                                "Web candidate",
                                "Web 候補",
                                "웹 후보"
                            )
                            .to_owned(),
                            url: candidate.url,
                            description: {
                                let description = clean_text(&candidate.description, 180);
                                if description.is_empty() {
                                    tr!(
                                        lang,
                                        "联网搜索发现的候选资源，需要人工核对与上述操作的匹配度。",
                                        "Candidate found by web search; check by hand how well it fits the workflow above.",
                                        "Web 検索で見つかった候補です。上記の操作との相性を人の目で確認する必要があります。",
                                        "웹 검색에서 찾은 후보로, 위 작업과의 적합성을 직접 확인해야 합니다."
                                    )
                                    .to_owned()
                                } else {
                                    description
                                }
                            },
                            benefit: tr!(
                                lang,
                                "补充本地精选之外的候选，不保证适用。",
                                "Adds candidates beyond the local picks; usefulness isn't guaranteed.",
                                "ローカルの厳選候補を補うもので、適合は保証されません。",
                                "로컬 엄선 후보를 보완하며 적합성은 보장되지 않습니다."
                            )
                            .to_owned(),
                            setup_cost: tr!(
                                lang,
                                "待评估",
                                "To be assessed",
                                "要評価",
                                "평가 필요"
                            )
                            .to_owned(),
                            permissions: tr!(
                                lang,
                                "只访问公开页面；安装前审阅源码、权限、发布方和维护状态。",
                                "Accesses public pages only; review source, permissions, publisher and maintenance status before installing.",
                                "公開ページのみにアクセスします。インストール前にソース、権限、公開元、保守状況を確認してください。",
                                "공개 페이지만 접근합니다. 설치 전에 소스, 권한, 배포자, 유지보수 상태를 검토하세요."
                            )
                            .to_owned(),
                            source: "web".to_owned(),
                            verification: tr!(
                                lang,
                                "联网搜索发现，尚未人工复核",
                                "Found by web search, not yet manually reviewed",
                                "Web 検索で見つかったもので、まだ人の確認はありません",
                                "웹 검색에서 찾은 것으로 아직 직접 검토하지 않았습니다"
                            )
                            .to_owned(),
                            verified_at: None,
                        });
                        search.added += 1;
                    }
                }
                Err(error) => search.errors.push({
                    let message = clean_text(&error.to_string(), 160);
                    if message.is_empty() {
                        tr!(
                            lang,
                            "搜索失败",
                            "Search failed",
                            "検索に失敗しました",
                            "검색 실패"
                        )
                        .to_owned()
                    } else {
                        message
                    }
                }),
            }
        }
    }

    if ai.verify_sources != Some(false) {
        let mut keys = BTreeSet::new();
        for opportunity in &recommendations {
            for resource in &opportunity.resources {
                keys.insert(resource_key(&resource.url));
            }
        }
        for key in keys {
            let mut first_result: Option<(String, Option<i64>, bool)> = None;
            for opportunity in &mut recommendations {
                for resource in &mut opportunity.resources {
                    if resource_key(&resource.url) == key {
                        if let Some((verification, verified_at, verified)) = &first_result {
                            resource.verification = verification.clone();
                            resource.verified_at = *verified_at;
                            if *verified {
                                search.verified += 1;
                            }
                        } else {
                            let verified = verify_resource(resource, lang);
                            if verified {
                                search.verified += 1;
                            }
                            first_result = Some((
                                resource.verification.clone(),
                                resource.verified_at,
                                verified,
                            ));
                        }
                    }
                }
            }
        }
    }
    EnrichedRecommendations {
        recommendations,
        search,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abstract_queries_cannot_contain_private_locations() {
        assert_eq!(
            assert_abstract_query("open source Git GUI Windows", Lang::Zh).unwrap(),
            "open source Git GUI Windows"
        );
        assert!(assert_abstract_query("https://private.example.com/secret", Lang::Zh).is_err());
        assert!(assert_abstract_query("secret@example.com", Lang::Zh).is_err());
        assert!(assert_abstract_query("private.example.com", Lang::Zh).is_err());
    }

    #[test]
    fn urls_and_duckduckgo_results_are_sanitized() {
        assert_eq!(
            normalize_http_url("https://user:pass@github.com/example/oss?utm_source=test"),
            "https://github.com/example/oss"
        );
        assert!(!allowed_discovery("http://github.com/example/oss"));
        assert!(allowed_discovery("https://github.com/example/oss"));
        let parsed = parse_duckduckgo(
            r#"<a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fgithub.com%2Fexample%2Foss">Example &amp; OSS</a>"#,
        );
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].url, "https://github.com/example/oss");
        assert_eq!(parsed[0].title, "Example & OSS");
    }

    #[test]
    fn failed_online_search_keeps_local_recommendations() {
        let recommendation = Recommendation {
            id: "test".to_owned(),
            title: "Test".to_owned(),
            evidence: "4 次".to_owned(),
            summary: "Test".to_owned(),
            search_query: "abstract tools".to_owned(),
            replaces: vec![],
            score: 1.0,
            resources: vec![],
        };
        let ai = AiSettings {
            web_search: true,
            verify_sources: Some(false),
            ..Default::default()
        };
        let result = enrich_with_search(&[recommendation], &ai, Lang::Zh, |_, _| bail!("offline"));
        assert_eq!(result.search.errors.len(), 1);
        assert_eq!(result.recommendations.len(), 1);
        assert!(result.recommendations[0].resources.is_empty());
    }
}
