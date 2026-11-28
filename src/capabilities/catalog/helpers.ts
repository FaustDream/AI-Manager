import type { CatalogMedia, CatalogRating, CatalogSourceStatus } from '@/lib/types';

export const KIND_ORDER = ['variety', 'anime', 'manga', 'tv', 'movie', 'documentary'] as const;
export const KIND_LABELS: Record<string, string> = {
  variety: '综艺', anime: '动漫', manga: '漫画', tv: '电视剧', movie: '电影',
  documentary: '纪录片', other: '其他', unknown: '类型未知',
};
export const MEDIA_STATUSES: Record<string, string> = {
  airing: '更新中', completed: '已完结', upcoming: '未发行', hiatus: '暂停',
  cancelled: '已取消', unknown: '状态未知',
};
const SOURCE_LABELS: Record<string, string> = {
  tvmaze: 'TVmaze', anilist: 'AniList', mangadex: 'MangaDex', bangumi: 'Bangumi',
  simkl: 'SIMKL', kitsu: 'Kitsu', tmdb: 'TMDb', imdb: 'IMDb', omdb: 'OMDb',
  rotten_tomatoes: 'Rotten Tomatoes', rottentomatoes: 'Rotten Tomatoes', rt: 'Rotten Tomatoes',
  metacritic: 'Metacritic', douban: '豆瓣', mal: 'MyAnimeList', myanimelist: 'MyAnimeList',
  jikan: 'Jikan', tvdb: 'TheTVDB', trakt: 'Trakt', bilibili: '哔哩哔哩',
  tencent: '腾讯视频', iqiyi: '爱奇艺', youku: '优酷', mgtv: '芒果TV', netflix: 'Netflix',
};
const REGION_LABELS: Record<string, string> = {
  CN: '中国大陆', JP: '日本', KR: '韩国', US: '美国', GB: '英国', TH: '泰国',
  HK: '中国香港', TW: '中国台湾', EU: '欧洲', FR: '法国', DE: '德国', IT: '意大利',
  ES: '西班牙', PT: '葡萄牙', NL: '荷兰', BE: '比利时', AT: '奥地利', CH: '瑞士',
  SE: '瑞典', NO: '挪威', DK: '丹麦', FI: '芬兰', IS: '冰岛', IE: '爱尔兰',
  PL: '波兰', CZ: '捷克', SK: '斯洛伐克', HU: '匈牙利', RO: '罗马尼亚',
  BG: '保加利亚', GR: '希腊', HR: '克罗地亚', SI: '斯洛文尼亚', RS: '塞尔维亚',
  BA: '波黑', AL: '阿尔巴尼亚', ME: '黑山', MK: '北马其顿', EE: '爱沙尼亚',
  LV: '拉脱维亚', LT: '立陶宛', LU: '卢森堡', MT: '马耳他', CY: '塞浦路斯',
  UA: '乌克兰', BY: '白俄罗斯', MD: '摩尔多瓦', RU: '俄罗斯', TR: '土耳其',
  CA: '加拿大', AU: '澳大利亚', NZ: '新西兰', IN: '印度', SG: '新加坡',
  MY: '马来西亚', ID: '印度尼西亚', VN: '越南', PH: '菲律宾', BR: '巴西',
};
const JOB_LABELS: Record<string, string> = {
  ready: '就绪', running: '采集中', success: '成功', completed: '完成', partial: '部分完成',
  failed: '失败', disabled: '已停用', unconfigured: '未配置', unavailable: '未接入',
  idle: '未运行', pending: '等待中',
};
export const SYNC_MODES = { incremental: '增量采集', full: '全量检查', history: '历史回填' } as const;
export type SyncMode = keyof typeof SYNC_MODES;

export function kindLabel(value: string): string { return KIND_LABELS[value] ?? (value || '类型未知'); }
export function sourceLabel(value: string): string { return SOURCE_LABELS[value.toLowerCase()] ?? (value || '来源未知'); }
export function statusLabel(value: string): string { return MEDIA_STATUSES[value] ?? (value || '状态未知'); }
export function jobStatusLabel(value: string): string { return JOB_LABELS[value] ?? (value || '状态未知'); }
export function regionLabel(value: string): string {
  const code = value.trim().toUpperCase();
  if (!code || ['UNKNOWN', 'UNK', 'ZZ', 'OTHER'].includes(code)) return '地区未知';
  return REGION_LABELS[code] ?? value;
}
export function regionsLabel(values: string[]): string {
  return values.length ? values.map(regionLabel).join(' / ') : '地区未知';
}
export function textOrUnknown(value: string | null | undefined): string { return value?.trim() || '暂无资料'; }
export function listOrUnknown(values: string[]): string { return values.filter(Boolean).join(' / ') || '暂无资料'; }
export function jobTone(status: string): 'ok' | 'warn' | 'bad' | 'muted' | 'info' {
  if (['success', 'completed', 'ready'].includes(status)) return 'ok';
  if (status === 'failed') return 'bad';
  if (['partial', 'unconfigured'].includes(status)) return 'warn';
  return status === 'running' ? 'info' : 'muted';
}
export function canRetry(status: string): boolean { return status === 'failed' || status === 'partial'; }

type SourceSuccess = Pick<CatalogSourceStatus, 'status' | 'last_success'>;
type SourceIdentity = Pick<CatalogSourceStatus, 'source'>;

/** 当前成功才算成功源：历史 last_success 不能掩盖后来的失败。 */
export function isSuccessfulSource(row: SourceSuccess): boolean {
  return row.status === 'success' && row.last_success !== null;
}

/**
 * 采集管理的可见来源：移除优先；只看成功源时，显式新增的源即使还没成功也保留。
 * 只管界面可见性，不影响后端调度与任务记录。
 */
export function visibleCatalogSources<T extends SourceSuccess & SourceIdentity>(
  sources: readonly T[],
  hidden: ReadonlySet<string>,
  added: ReadonlySet<string>,
  successfulOnly: boolean,
): T[] {
  return sources.filter((row) => {
    if (hidden.has(row.source)) return false;
    return !successfulOnly || added.has(row.source) || isSuccessfulSource(row);
  });
}

export function todayISO(date = new Date()): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
}
export function isISODate(value: string): boolean {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(value)) return false;
  const stamp = Date.parse(`${value}T00:00:00Z`);
  return Number.isFinite(stamp) && new Date(stamp).toISOString().slice(0, 10) === value;
}
// 日历是来源的当地日期；日期运算用 UTC，避免浏览器夏令时把一天移成 23/25 小时。
export function shiftISO(value: string, days: number): string {
  if (!isISODate(value)) return '';
  const date = new Date(`${value}T00:00:00Z`);
  date.setUTCDate(date.getUTCDate() + days);
  return date.toISOString().slice(0, 10);
}
export function weekdayISO(value: string): number | null {
  return isISODate(value) ? new Date(`${value}T00:00:00Z`).getUTCDay() || 7 : null;
}
export function mondayISO(value = todayISO()): string {
  const weekday = weekdayISO(value);
  return weekday === null ? '' : shiftISO(value, 1 - weekday);
}
export function weekRange(value: string): { start: string; end: string; last: string } {
  const start = mondayISO(value);
  return { start, end: shiftISO(start, 7), last: shiftISO(start, 6) };
}
export function formatDay(value: string): string {
  const weekday = weekdayISO(value);
  return weekday === null ? value : `${value} 周${'一二三四五六日'[weekday - 1]}`;
}
export function syncDateError(mode: SyncMode, start: string, end: string): string | undefined {
  if (!start && !end && mode !== 'history') return undefined;
  if (!isISODate(start) || !isISODate(end)) return '请选择有效的开始日期和结束日期';
  if (end <= start) return '结束日期须晚于开始日期（结束日期不含当天）';
  return undefined;
}

function numberText(value: number): string { return Number.isInteger(value) ? String(value) : String(Number(value.toFixed(2))); }
export function ratingCategory(value: string): string {
  const labels: Record<string, string> = {
    user: '用户评分', users: '用户评分', user_score: '用户评分', audience: '观众评分', audience_score: '观众评分',
    critic: '专业评论评分', critics: '专业评论评分', positive: '好评率', positive_rate: '好评率',
  };
  return labels[value] ?? (value || '类别未知');
}
export function originalRating(rating: Pick<CatalogRating, 'value' | 'scale'>): string {
  const { value, scale } = rating;
  if (value === null || !Number.isFinite(value) || !Number.isFinite(scale) || scale <= 0 || value < 0 || value > scale) return '暂无评分';
  return `${numberText(value)} / ${numberText(scale)}`;
}
// CatalogMedia.rating 保留所选来源原值；后端仅 SQL 筛选索引归一化。
// 量表依据已知来源契约，不根据分值大小猜量表；完整原始量表仍在详情 ratings 中呈现。
const AUDIENCE_SCALES: Record<string, number> = {
  tvmaze: 10, anilist: 100, kitsu: 100, bangumi: 10, simkl: 10, tmdb: 10,
  imdb: 10, douban: 10, mal: 10, myanimelist: 10, rt: 100, rotten_tomatoes: 100, rottentomatoes: 100,
  bilibili: 10, iqiyi: 10, mgtv: 100,
};
export function audienceRating(media: Pick<CatalogMedia, 'rating' | 'rating_source'>): string {
  if (media.rating === null || !Number.isFinite(media.rating) || media.rating < 0 || !media.rating_source) return '暂无评分';
  const scale = AUDIENCE_SCALES[media.rating_source.toLowerCase()];
  if (!scale) return `${numberText(media.rating)} · ${sourceLabel(media.rating_source)}（原值，量表未知）`;
  if (media.rating > scale) return '暂无评分';
  return `${numberText(media.rating * 10 / scale)} / 10 · ${sourceLabel(media.rating_source)}`;
}

// 只有已知资料站的资料路径可打开；旧平台播放地址和未知网址仍作为不可点击的溯源文字。
export function safeMetadataUrl(source: string, value: string | null | undefined): string | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    if (!['https:', 'http:'].includes(url.protocol) || url.username || url.password || url.port) return null;
    const path = decodeURIComponent(url.pathname);
    if (/(?:play|v_show|\/watch(?:\/|$)|\/video(?:\/|$))/i.test(path)) return null;
    // 不打开带查询参数的地址，既避免跳转到资源，也避免凭据进入外部命令日志。
    if (url.search || url.hash) return null;
    const host = url.hostname.toLowerCase().replace(/^www\./, '');
    const id = source.toLowerCase();
    const rules: Record<string, [string[], RegExp]> = {
      tvmaze: [['tvmaze.com'], /^\/(?:shows|episodes)\/\d+(?:\/[^/]+)?\/?$/],
      anilist: [['anilist.co'], /^\/(?:anime|manga)\/\d+(?:\/[^/]+)?\/?$/],
      mangadex: [['mangadex.org'], /^\/title\/[\da-f-]{36}(?:\/[^/]+)?\/?$/i],
      bangumi: [['bgm.tv', 'bangumi.tv', 'chii.in'], /^\/(?:subject|ep)\/\d+\/?$/],
      kitsu: [['kitsu.io', 'kitsu.app'], /^\/(?:anime|manga)\/[^/]+\/?$/],
      simkl: [['simkl.com'], /^\/(?:anime|tv|movies)\/\d+(?:\/[^/]+)?\/?$/],
      tmdb: [['themoviedb.org'], /^\/(?:movie|tv)\/\d+(?:-[^/]+)?\/?$/],
      imdb: [['imdb.com'], /^\/title\/tt\d+(?:\/(?:ratings|reviews))?\/?$/],
      rotten_tomatoes: [['rottentomatoes.com'], /^\/(?:m|tv)\/[^/]+\/?$/],
      metacritic: [['metacritic.com'], /^\/(?:movie|tv)\/[^/]+\/?$/],
      douban: [['movie.douban.com', 'book.douban.com'], /^\/subject\/\d+\/?$/],
      mal: [['myanimelist.net'], /^\/(?:anime|manga)\/\d+(?:\/[^/]+)?\/?$/],
      bilibili: [['bilibili.com'], /^\/bangumi\/media\/md\d+\/?$/],
      tencent: [['v.qq.com'], /^\/detail\/[a-z]\/[^/]+\.html$/],
      iqiyi: [['iqiyi.com'], /^\/lib\/m_\d+\.html$/],
    };
    const aliases: Record<string, string> = { rottentomatoes: 'rotten_tomatoes', rt: 'rotten_tomatoes', myanimelist: 'mal', jikan: 'mal' };
    const rule = rules[aliases[id] ?? id];
    return rule && rule[0].includes(host) && rule[1].test(path) ? url.href : null;
  } catch { return null; }
}
export function provenanceText(value: string | null | undefined): string {
  if (!value) return '暂无资料';
  try {
    const url = new URL(value);
    if (url.username || url.password) return '地址含认证信息，已隐藏';
    const sensitive = [...url.searchParams.keys()].filter((key) => /token|key|secret|password|passwd|credential|auth|signature|cookie|code/i.test(key));
    for (const key of sensitive) url.searchParams.set(key, '已隐藏');
    if (/token|key|secret|password|credential|auth/i.test(url.hash)) url.hash = '已隐藏';
    return url.href;
  } catch { return '非有效网址（不可打开）'; }
}

/** 追剧状态文案（want=想看 / watching=在追 / shelve=搁置 / done=看完）。 */
export const FOLLOW_LABELS: Record<string, string> = {
  want: '想看', watching: '在追', shelve: '搁置', done: '看完',
};
export function followLabel(value: string): string { return FOLLOW_LABELS[value] ?? value; }

/** 已确认可安全跳转的国内平台搜索页：没有详情直链时的「去哪看」兜底。 */
export function platformSearchUrl(platform: string, title: string): string | null {
  const keyword = encodeURIComponent(title.trim());
  if (!keyword) return null;
  switch (platform) {
    case 'bilibili': return `https://search.bilibili.com/all?keyword=${keyword}`;
    case 'iqiyi': return `https://so.iqiyi.com/so/q_${keyword}`;
    case 'tencent': return `https://v.qq.com/x/search/?q=${keyword}`;
    case 'youku': return `https://so.youku.com/search_video/q_${keyword}`;
    case 'mgtv': return `https://search.mgtv.com/all?q=${keyword}`;
    default: return null;
  }
}
